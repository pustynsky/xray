# Concurrency Model

## Overview

The system uses three distinct concurrency strategies depending on the operation phase:

| Phase           | Strategy                                     | Primitives                                            | Why                                                |
| --------------- | -------------------------------------------- | ----------------------------------------------------- | -------------------------------------------------- |
| Index build     | Thread pool (parallel walk + parallel parse) | `WalkBuilder::build_parallel()`, `std::thread::scope` | CPU-bound, embarrassingly parallel                 |
| Async startup   | Background thread(s) + atomic flags          | `std::thread::spawn`, `Arc<AtomicBool>`               | Event loop starts immediately, no client timeout   |
| MCP server      | Single-threaded event loop                   | Sequential stdin line reads                           | JSON-RPC is inherently sequential                  |
| File watcher    | Dedicated OS thread + shared state           | `Arc<RwLock<T>>`, `mpsc::channel`                     | Must not block the event loop                      |

```mermaid
graph TB
    subgraph "Build Phase (multi-threaded)"
        W1[Walker Thread 1]
        W2[Walker Thread 2]
        WN[Walker Thread N]
        W1 --> M1["Mutex<Vec<FileEntry>>"]
        W2 --> M1
        WN --> M1
        M1 --> CHUNK[Chunk files across N threads]
        CHUNK --> T1[Tokenizer Thread 1<br/>local HashMap]
        CHUNK --> T2[Tokenizer Thread 2<br/>local HashMap]
        CHUNK --> TN[Tokenizer Thread N<br/>local HashMap]
        T1 --> MERGE[Sequential Merge]
        T2 --> MERGE
        TN --> MERGE
    end

    subgraph "Async Startup (if no index on disk)"
        BG1[Background Thread 1<br/>build_content_index] -->|write lock + AtomicBool| IDX2["Arc<RwLock<ContentIndex>>"]
        BG2[Background Thread 2<br/>build_definition_index] -->|write lock + AtomicBool| DIDX2["Arc<RwLock<DefinitionIndex>>"]
    end

    subgraph "Server Phase"
        STDIN[stdin reader<br/>single thread] -->|read lock| IDX["Arc<RwLock<ContentIndex>>"]
        STDIN -->|read lock| DIDX["Arc<RwLock<DefinitionIndex>>"]
        STDIN -->|check| FLAGS["AtomicBool: content_ready, def_ready"]

        WT[Watcher Thread] -->|write lock| IDX
        WT -->|write lock| DIDX
    end
```

## Phase 1: Parallel Index Build

### File Walk (ignore crate)

The `ignore` crate (from ripgrep) provides `WalkBuilder::build_parallel()` which spawns N threads, each walking a subtree of the directory. Results are collected via a `Mutex<Vec<T>>`:

```rust
let entries: Mutex<Vec<FileEntry>> = Mutex::new(Vec::new());

builder.build_parallel().run(|| {
    Box::new(move |result| {
        // Each thread pushes to shared vec
        entries.lock().unwrap().push(entry);
        ignore::WalkState::Continue
    })
});
```

**Lock contention:** Minimal. Each thread holds the mutex only for ~100ns (one `Vec::push`). With 24 threads and ~49K files, the mutex is acquired ~2K times per thread. The `ignore` crate's internal work distribution ensures threads process different directory subtrees, so lock acquisitions are spread over time.

### Content Index Build

Both file walk and tokenization are parallelized. After the parallel walk collects `Vec<(path, content)>`, the files are chunked and tokenized in parallel using `std::thread::scope`:

```
[Parallel Walk] → Vec<(path, content)> → [Parallel Tokenize: chunk files across N threads]
                                           → per-thread local HashMap
                                           → [Sequential Merge: combine local indexes]
```

```rust
let num_tok_threads = thread_count.max(1);
let tok_chunk_size = file_count.div_ceil(num_tok_threads).max(1);

let chunk_results: Vec<_> = std::thread::scope(|s| {
    let handles: Vec<_> = file_data.chunks(tok_chunk_size)
        .enumerate()
        .map(|(chunk_idx, chunk)| {
            let base_file_id = (chunk_idx * tok_chunk_size) as u32;
            s.spawn(move || {
                let mut local_index: HashMap<String, Vec<Posting>> = HashMap::new();
                // tokenize each file in chunk into local_index
            })
        }).collect();
    handles.into_iter().map(|h| h.join().unwrap()).collect()
});

// Sequential merge: ~50ms for 57M tokens
for (local_files, local_counts, local_index, local_total) in chunk_results {
    for (token, postings) in local_index {
        index.entry(token).or_default().extend(postings);
    }
}
```

**Design:** Each thread builds a completely independent local `HashMap<String, Vec<Posting>>` — no shared mutable state, no locks during tokenization. The merge step is sequential but fast (~50ms) because it only moves pre-built `Vec<Posting>` entries. Memory overhead is bounded: each thread's local index is a subset of the global index, and the merge transfers ownership rather than cloning.

**Benchmark (65K files, 57M tokens, 24-core CPU):** Parallel tokenization reduced content index build from 44s to 22s (2× speedup). The merge step is <1% of total time.

### Definition Index Build

Definition parsing IS parallelized because tree-sitter parsing is CPU-intensive (~16-32s for ~48K files depending on CPU):

```rust
let chunks: Vec<Vec<(u32, String)>> = files.chunks(chunk_size).collect();

std::thread::scope(|s| {
    for chunk in chunks {
        s.spawn(move || {
            let mut cs_parser = tree_sitter::Parser::new();
            // TS/TSX parsers are lazy-initialized only when needed
            let mut ts_parser: Option<Parser> = None;
            let mut tsx_parser: Option<Parser> = None;

            for (file_id, path) in chunk {
                match extension {
                    "cs" => parse_csharp(&mut cs_parser, ...),
                    "ts" => {
                        let p = ts_parser.get_or_insert_with(|| make_ts_parser());
                        parse_typescript(p, ...);
                    }
                    "tsx" => {
                        let p = tsx_parser.get_or_insert_with(|| make_tsx_parser());
                        parse_typescript(p, ...);
                    }
                }
            }
        });
    }
});
// Merge: sequential, ~50ms for ~846K definitions + ~2.4M call sites
```

**Key details:**

- tree-sitter `Parser` is `!Send` (contains internal mutable state). Each thread creates its own parser instance. This is intentional — tree-sitter parsers reuse internal memory allocations across parse calls, making per-thread parsers more efficient than a shared pool.
- **Lazy parser initialization:** TS/TSX parsers are created via `Option<Parser>` + `get_or_insert_with()` only when a thread encounters a file with that extension. For C#-only projects (the common case), TypeScript grammars are never loaded, saving ~2s per parser per thread. The `def_exts` parameter in `serve.rs` filters to the intersection of `--ext` and supported languages (`cs`, `ts`, `tsx`, `sql`), so unnecessary grammars are never even considered. SQL files use a regex-based parser (no tree-sitter grammar needed).

## Phase 2: MCP Server Event Loop

The server uses a deliberately simple single-threaded model:

```rust
for line in stdin.lock().lines() {
    let request: JsonRpcRequest = serde_json::from_str(&line)?;
    let response = handle_request(&ctx, &request.method, &request.params, id);
    writeln!(stdout, "{}", serde_json::to_string(&response)?);
}
```

**Why not async/tokio?**

- MCP over stdio is inherently sequential — one request at a time on stdin
- Each query takes ~0.6ms (HashMap lookup + TF-IDF scoring, measured) — async overhead would exceed query time
- No I/O multiplexing needed — single input source (stdin), single output (stdout)
- Adding tokio would increase binary size and compile time significantly

**Read lock acquisition:** Each query acquires a read lock on the index. Multiple concurrent reads are allowed by `RwLock`, but since we're single-threaded, there's never actual read-read contention. The lock exists to synchronize with the watcher thread and the background build thread (during async startup).

## Phase 2.5: Async Startup (Background Index Build)

When no pre-built index exists on disk (first run), the server spawns background threads to build indexes without blocking the event loop:

```
cmd_serve()
  ├── empty ContentIndex in Arc<RwLock>
  ├── empty DefinitionIndex in Arc<RwLock>
  ├── content_ready = Arc<AtomicBool>(false)
  ├── def_ready = Arc<AtomicBool>(false)
  ├── try load from disk → if found: swap + set ready flag (synchronous, < 3s)
  │   else: std::thread::spawn → build + swap + set ready flag (30-300s)
  └── run_server() ← starts immediately
        └── dispatch_tool checks AtomicBool before each search tool
```

**Synchronization:**

- `AtomicBool` with `Release`/`Acquire` ordering gates tool readiness — cheap (no lock contention)
- Background thread acquires a single write lock to swap the fully-built index into the `Arc<RwLock>`, then sets the `AtomicBool` flag
- Tools like `search_help`, `search_info`, `search_find` bypass the readiness check (they don't use content/def indexes)
- `search_reindex` during background build returns "already building" error to prevent double-builds

## Phase 3: File Watcher

The watcher runs on a dedicated OS thread spawned at server startup:

```mermaid
sequenceDiagram
    participant OS as OS (ReadDirectoryChangesW)
    participant Watcher as Watcher Thread
    participant Channel as mpsc::channel
    participant Index as "Arc<RwLock<ContentIndex>>"

    OS->>Channel: FileCreate event
    OS->>Channel: FileModify event
    OS->>Channel: FileModify event

    Note over Watcher: recv_timeout(500ms)
    Note over Watcher: Debounce window expires

    Watcher->>Watcher: Batch: 3 dirty files
    Watcher->>Index: write lock acquired
    Note over Index: Server reads blocked (~5ms)
    Watcher->>Index: incremental update
    Watcher->>Index: write lock released
    Note over Index: Server reads resume
```

### Debounce Strategy

File events are collected into a `HashSet<PathBuf>` (deduplicating rapid saves of the same file) and processed in batch after the debounce window:

```rust
loop {
    match rx.recv_timeout(Duration::from_millis(debounce_ms)) {
        Ok(event) => {
            // Collect into dirty_files / removed_files sets
            dirty_files.insert(path);
        }
        Err(Timeout) => {
            // Process batch
            if dirty_files.len() + removed_files.len() > bulk_threshold {
                // Full reindex
            } else {
                // Incremental update
            }
        }
    }
}
```

### Lock Holding Duration

The write lock is held for the entire batch, not per-file. This minimizes lock acquisition overhead and ensures atomic batch updates:

| Batch Size | Lock Duration         | Impact on Queries |
| ---------- | --------------------- | ----------------- |
| 1 file     | ~50-100ms             | Brief pause       |
| 10 files   | ~500ms-1s             | Noticeable pause  |
| 100 files  | Full reindex (~7-16s) | Significant pause |
| >100 files | Full reindex (~7-16s) | Significant pause |

The bulk threshold (default: 100) triggers full reindex instead of incremental updates for large batches (git checkout, branch switch). Full reindex is actually faster than 100+ individual incremental updates because it rebuilds the entire index from scratch.

> **Memory optimization note:** The forward index (`file_id → Vec<token>`) was removed to save ~1.5 GB of RAM. Incremental updates now use a brute-force scan of the inverted index to remove stale postings (~50-100ms per file, acceptable for watcher debounce windows).

### Dual Index Updates

When `--definitions` is enabled, the watcher updates both indexes in sequence:

```rust
// Content index update
match index.write() {
    Ok(mut idx) => {
        for path in &dirty_clean { update_file_in_index(&mut idx, path); }
    }
}

// Definition index update
if let Some(ref def_idx) = def_index {
    match def_idx.write() {
        Ok(mut idx) => {
            for path in &dirty_clean { update_file_definitions(&mut idx, path); }
        }
    }
}
```

**Important:** The two indexes are updated sequentially, not atomically. There's a brief window where the content index is updated but the definition index is stale. This is acceptable because:

1. The window is <5ms per file
2. Queries that use both indexes (search_callers) will see slightly stale definition data, which at worst means a caller might be missing from the tree until the next update cycle
3. True atomicity would require either a single lock for both indexes (reducing read concurrency) or a transaction log (complexity not justified)

## Thread Safety Guarantees

| Data              | Owner                          | Synchronization                             | Invariant                                                                                |
| ----------------- | ------------------------------ | ------------------------------------------- | ---------------------------------------------------------------------------------------- |
| `ContentIndex`    | `Arc<RwLock<ContentIndex>>`    | Read: server thread. Write: watcher thread, background build thread (once at startup). | Forward index + inverted index always consistent within a single write lock acquisition. |
| `DefinitionIndex` | `Arc<RwLock<DefinitionIndex>>` | Same as ContentIndex.                       | Multi-indexes (name, kind, attr, etc.) always consistent within a single write.          |
| `content_ready`   | `Arc<AtomicBool>`              | Write: background build thread (once). Read: server thread (every dispatch). | `Ordering::Release` on write, `Ordering::Acquire` on read — guarantees index data is visible. |
| `def_ready`       | `Arc<AtomicBool>`              | Same as `content_ready`.                    | Same guarantee.                                                                          |
| stdin/stdout      | MCP server thread (exclusive)  | No sharing.                                 | All JSON-RPC I/O on single thread.                                                       |
| stderr            | Any thread                     | OS-level line buffering.                    | Log lines may interleave but each `eprintln!` is atomic per line.                        |

## Potential Issues and Mitigations

### RwLock Poisoning

If the watcher thread panics while holding a write lock, the `RwLock` becomes poisoned. All subsequent read/write attempts will fail. Current behavior: the server logs an error and continues operating with the last good index state. The only recovery is restarting the server.

### Watcher Thread Crash

If the watcher thread panics (e.g., out of memory during reindex), the `_watcher` handle is dropped, which stops the file notifications. The server continues operating with a stale index. Detection: no `[watcher]` log messages after a file change.

### Backpressure

If the server is processing a long query (e.g., `search_callers` with depth=10), incoming file events queue up in the `mpsc::channel`. The channel is unbounded, so events are never lost. They'll be processed in the next debounce window after the query completes.
