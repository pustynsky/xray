# Storage Model

## Index file layout

All indexes are stored under a platform-specific data directory:

| OS      | Path                                          |
| ------- | --------------------------------------------- |
| Windows | `%LOCALAPPDATA%\xray\`                |
| macOS   | `~/Library/Application Support/xray/` |
| Linux   | `~/.local/share/xray/`                |

```
xray/
├── Repos_MyProject_a1b2c3d4.file-list           ← FileIndex
├── Repos_MyProject_f0e1d2c3.word-search          ← ContentIndex (cs,xml)
├── Repos_MyProject_12345678.code-structure        ← DefinitionIndex (cs,xml)
├── rust_search_9876fedc.word-search                    ← ContentIndex (rs)
└── rust_search_aabb0011.code-structure                 ← DefinitionIndex (rs)
```

### File naming scheme

Each index file is named with a human-readable semantic prefix and a truncated hash:

```
{semantic_prefix}_{hash8}.{file-list|word-search|code-structure}
```

Where:
- `semantic_prefix` — derived from the last 1-2 path components (sanitized for Windows filenames)
- `hash8` — first 8 hex characters of the 64-bit FNV-1a hash (truncated to 32 bits)
- Extension — index type

### Extensions

| Extension           | Index Type      | Purpose                                    |
|---------------------|-----------------|--------------------------------------------|
| `.file-list`        | FileIndex       | File name lookup (`xray_fast`)           |
| `.word-search`      | ContentIndex    | Full-text token search (`xray_grep`)     |
| `.code-structure`   | DefinitionIndex | AST definitions & callers (`xray_definitions`, `xray_callers`) |
| `.git-history`      | GitHistoryCache | Git commit history cache (`xray_git_history`, `xray_git_authors`, `xray_git_activity`) |

### Semantic prefix rules

The prefix is extracted from the canonicalized directory path by `extract_semantic_prefix()`:

| # Normal path components | Rule                              | Example                              |
|--------------------------|-----------------------------------|--------------------------------------|
| 0 (drive root)           | Drive letter                      | `C:\` → `C`                         |
| 1                        | `{drive_letter}_{name}`           | `C:\test` → `C_test`                |
| 2+                       | `{second_to_last}_{last}`         | `C:\Repos\App` → `Repos_App`        |

Each component is sanitized via `sanitize_for_filename()`:
1. Characters not in `[a-zA-Z0-9_-]` → replaced with `_`
2. Windows reserved names (CON, NUL, etc.) → prefixed with `_`
3. Empty → `_`
4. Truncated to 50 characters

### Hash Identity

```rust
// FileIndex: FNV-1a hash of canonical directory path
let hash = stable_hash(&[canonical_dir.as_bytes()]);
let filename = format!("{}_{:08x}.file-list", prefix, hash as u32);

// ContentIndex: FNV-1a hash of canonical dir + extension string
let hash = stable_hash(&[canonical_dir.as_bytes(), exts.as_bytes()]);
let filename = format!("{}_{:08x}.word-search", prefix, hash as u32);

// DefinitionIndex: FNV-1a hash of canonical dir + extension string + "definitions"
let hash = stable_hash(&[canonical_dir.as_bytes(), exts.as_bytes(), b"definitions"]);
let filename = format!("{}_{:08x}.code-structure", prefix, hash as u32);

// GitHistoryCache: FNV-1a hash of canonical dir + "git-history"
let hash = stable_hash(&[canonical_dir.as_bytes(), b"git-history"]);
let filename = format!("{}_{:08x}.git-history", prefix, hash as u32);
```

**Implication:** Indexing the same directory with different extension sets produces different files. `xray content-index -d C:\Projects -e cs` and `xray content-index -d C:\Projects -e cs,sql` create two separate `.word-search` files.

### Collision Handling

FNV-1a provides 64-bit hashes, truncated to 32 bits for the filename. Hash collisions are possible but extremely unlikely for realistic use — birthday bound is ~77K directories for 50% collision probability. No collision detection is implemented — a collision would silently overwrite the previous index.

## Serialization format

All indexes use [bincode](https://docs.rs/bincode/1/bincode/) v1 for serialization, wrapped in [LZ4 frame compression](https://crates.io/crates/lz4_flex) for reduced disk usage and faster I/O.

### Two on-disk layouts

`xray` writes one of two LZ4-based layouts depending on the index type:

| Layout | Magic | Used by | Reason |
| --- | --- | --- | --- |
| Single LZ4 frame | `LZ4S` | `FileIndex` (`.file-list`), `GitHistoryCache` (`.git-history`) | Small payloads (a few MB). Single-thread decode is already fast. |
| Sharded LZ4 (N independent frames) | `XRYS` | `ContentIndex` (`.word-search`), `DefinitionIndex` (`.code-structure`) | Hundreds of MB on large repos. Decoded in parallel during hot-start. |

Header readers (`read_root_from_index_file`, `read_format_version_from_index_file`)
dispatch by magic so both layouts coexist on disk and the lightweight metadata
lookups (`xray info`, find-by-root scans) stay zero-copy regardless of layout.

### Single LZ4 frame layout (legacy / small indexes)

```rust
let file = File::create(path)?;
let mut writer = BufWriter::new(file);
writer.write_all(b"LZ4S")?;                       // magic
let mut encoder = lz4_flex::frame::FrameEncoder::new(writer);
bincode::serialize_into(&mut encoder, &index)?;   // whole struct in one stream
encoder.finish()?.flush()?;
```

Load goes through `load_compressed::<T>(&path, label)` which streams
`FrameDecoder` -> `bincode::deserialize_from`.

### Sharded LZ4 layout (`SHARD_MAGIC = b"XRYS"`)

Large indexes carry a heavy entries payload (the `index: HashMap` of
`ContentIndex`, the `definitions: Vec` of `DefinitionIndex`) that dominates
bincode decode wall time. The sharded layout splits that payload into N
independent LZ4 frames whose byte offsets are recorded up front. The loader
retains one opened file and uses bounded positioned reads (`read_at` on Unix,
`seek_read` on Windows) to decode each shard on its own OS thread with an
independent logical offset.

Byte layout written by `save_sharded`:

```text
[4 bytes]                magic = SHARD_MAGIC ("XRYS")
[8 bytes le u64]         head_size — length of the head bincode payload
[head_size bytes]        bincode of `head` (PLAIN, NOT LZ4-compressed).
                         Field order MUST start with `root: String` then
                         `format_version: u32` so the lightweight header
                         readers can extract those two fields without
                         touching shard payloads.
[4 bytes le u32]         shard_count
[8 * shard_count bytes]  le u64[] shard_compressed_sizes
[shard 0 bytes]          one independent LZ4 frame containing bincode `Vec<T>`
[shard 1 bytes]          ...
[shard N-1 bytes]
```

- `SHARD_COUNT = 4` today (`src/index.rs`).
- The head carries every persisted field EXCEPT the heavy entries payload
  (`ContentIndexHead` / `DefinitionIndexHead` in `src/lib.rs` and
  `src/definitions/types.rs`).
- `definitions` is split into sequential chunks and merged in shard order
  on load, so `def_idx` values stored in the secondary `*_index` HashMaps
  remain valid across save→load.

Load path:

1. `load_content_index_at_path` / `load_definition_index_at_path` open the
   file once, check its format version before full deserialization, and rewind
   the same file to inspect its magic.
2. `LZ4_MAGIC` -> single-frame decoding from that opened file; legacy plain
   bincode remains supported.
3. `SHARD_MAGIC` -> parallel sharded decoding from that opened file ->
   `(head, Vec<T>)` ->
   `ContentIndex::from_head_and_entries` /
   `DefinitionIndex::from_head_and_entries`.

Version checks, magic dispatch, header/table reads, and payload decoding all use
the same opened snapshot, including legacy loads. An atomic pathname replacement
during loading does not mix the old file table with new shards: the in-flight
load can return the complete older snapshot, and a later load opens the
replacement. This does not guarantee the newest snapshot or repair caches
already corrupted before loading. Corrupt or incompatible snapshots remain
explicit load errors. Cache formats and version numbers are unchanged.

Per-shard decode is timed and reported in the debug log via
`indexShardCount`, `indexShardDecodeMaxMs`, `indexMergeMs`, `indexHeaderMs`,
`indexHeadReadMs`, `indexHeadDecodeMs`, `indexHeadBytes`, `indexEntries`.

### Bincode Properties

| Property    | Value                                                                                   |
| ----------- | --------------------------------------------------------------------------------------- |
| Format      | Little-endian, variable-length integers                                                 |
| Schema      | Implicit — derived from Rust struct layout                                              |
| Versioning  | `format_version: u32` field on every index struct; bump on layout change invalidates older layouts |
| Compression | LZ4 frame compression (`lz4_flex`); single-frame magic `LZ4S` or sharded magic `XRYS`   |
| Atomicity   | Whole-file write to a unique temp sibling (`.<file>.xray_tmp.<pid>.<nanos>.<counter>`) then `rename` over the target |

### Sizes on disk

Measured on a real codebase (LZ4-compressed disk size; uncompressed in-memory is larger — see [benchmarks.md](benchmarks.md) for raw bincode sizes):

| Index Type      | Files Indexed   | Content                              | On-Disk (LZ4)  | Uncompressed bincode |
| --------------- | --------------- | ------------------------------------ | -------------- | -------------------- |
| FileIndex       | 333,875 entries | Paths + metadata                     | 47.8 MB        | n/a (single frame)   |
| ContentIndex    | 48,599 files    | 33M tokens, 754K unique              | ~223.7 MB      | ~350 MB              |
| DefinitionIndex | ~48,600 files   | ~846K definitions + ~2.4M call sites | ~103.1 MB      | ~324–328 MB          |

In-memory size is larger than the uncompressed bincode column due to HashMap overhead and struct alignment, but has not been separately measured.

## Data structures on disk

### FileIndex

```rust
struct FileIndex {
    root: String,           // Canonical directory path
    format_version: u32,    // Current: 3; root/version must remain the first two fields
    created_at: u64,        // Unix timestamp (seconds)
    max_age_secs: u64,      // Staleness threshold
    entries: Vec<FileEntry>, // All files and directories
    respect_git_exclude: bool,
    hidden: bool,
    no_ignore: bool,
}

struct FileEntry {
    path: String,           // Full file path
    size: u64,              // File size in bytes
    modified: u64,          // Last modified timestamp
    is_dir: bool,           // Directory flag
}
```

### ContentIndex

```rust
struct ContentIndex {
    root: String,
    format_version: u32,                          // Current: 5 (also ContentIndexHead)
    created_at: u64,
    max_age_secs: u64,
    files: Vec<String>,                          // file_id → path
    index: HashMap<String, Vec<Posting>>,         // token → postings (sharded on disk)
    total_tokens: u64,                           // Total tokens indexed
    extensions: Vec<String>,                     // Extensions indexed
    file_token_counts: Vec<u32>,                 // file_id → token count (TF denom)
    trigram: TrigramIndex,                       // Substring search (rebuilt lazily)
    trigram_dirty: bool,
    file_tokens: Vec<Vec<String>>,               // file_id → tokens (watch mode; #[serde(skip)])
    file_tokens_authoritative: bool,             // True only when file_tokens is maintained
    path_to_id: Option<HashMap<PathBuf, u32>>,   // path → file_id (watch mode)
    respect_git_exclude: bool,                   // Persisted user choice for auto-rebuild
    hidden: bool,
    no_ignore: bool,
    // ...plus diagnostic counters: read_errors, lossy_file_count, worker_panics
}

struct Posting {
    file_id: u32,           // Index into ContentIndex.files
    lines: Vec<u32>,        // Line numbers where token appears
}
```

**Watch mode fields:** `file_tokens` and `path_to_id` are only populated when the MCP server starts with `--watch`. Both are marked `#[serde(skip)]` so they are not written to disk — they are rebuilt on demand when watch mode enters. `file_tokens` (the reverse `file_id → token-names` map) is only authoritative when `file_tokens_authoritative=true`; sync-reindex paths skip building it to save memory.

### DefinitionIndex

```rust
struct DefinitionIndex {
    root: String,
    format_version: u32,                               // Bumped on layout change → triggers rebuild
    created_at: u64,
    extensions: Vec<String>,
    files: Vec<String>,                                // file_id → path
    definitions: Vec<DefinitionEntry>,                  // All definitions (sharded on disk)
    name_index: HashMap<String, Vec<u32>>,              // name → def indices
    kind_index: HashMap<DefinitionKind, Vec<u32>>,      // kind → def indices
    attribute_index: HashMap<String, Vec<u32>>,         // attribute → def indices
    base_type_index: HashMap<String, Vec<u32>>,         // base type → def indices
    file_index: HashMap<u32, Vec<u32>>,                 // file_id → def indices (source of truth)
    selector_index: HashMap<String, Vec<u32>>,          // Angular selector → component def indices
    template_owners: HashMap<String, Vec<u32>>,         // normalized external template path → owners
    template_parents: HashMap<String, Vec<u32>>,        // child selector → parent component def indices
    path_to_id: HashMap<PathBuf, u32>,                  // path → file_id
    method_calls: HashMap<u32, Vec<CallSite>>,          // def_idx → call sites (xray_callers "down")
    code_stats: HashMap<u32, CodeStats>,                // def_idx → complexity metrics
    // def_idx → AST metadata source
    angular_components: HashMap<u32, AngularComponentRecord>,
    template_children: HashMap<u32, Vec<String>>,       // Angular component def_idx → child selectors
    extension_methods: HashMap<String, Vec<String>>,    // C# extension method → host static classes
    respect_git_exclude: bool,
    definition_generation: u64,                         // Complete definition-delta generation
    input_fingerprints: HashMap<String, DefinitionInputFingerprint>, // normalized input path → snapshot
    #[serde(skip)]
    pending_definition_inputs: HashMap<PathBuf, PendingDefinitionInput>, // retry state + observed revision; never persisted
    // ...plus diagnostic counters: parse_errors, lossy_file_count, empty_file_ids, worker_panics
}

struct DefinitionEntry {
    file_id: u32,
    name: String,
    kind: DefinitionKind,
    line_start: u32,
    line_end: u32,
    parent: Option<String>,       // Containing class/struct
    signature: Option<String>,    // Full signature text
    modifiers: Vec<String>,       // public, static, async, etc.
    attributes: Vec<String>,      // C# attributes
    base_types: Vec<String>,      // Implemented interfaces, base class
}

struct CallSite {
    method_name: String,           // Name of the called method
    receiver_type: Option<String>, // Resolved type of receiver (e.g., "IUserService")
    line: u32,                     // Line number of the call site
    receiver_is_generic: bool,     // `new List<int>()` → true; used to filter
                                   // generic-arity mismatches with non-generic name collisions
}
```

## Staleness model

Each index stores `created_at` and `max_age_secs`. Staleness check:

```rust
fn is_stale(&self) -> bool {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    now - self.created_at > self.max_age_secs
}
```

| Behavior                            | `xray fast` / `xray grep`                                | `xray serve`                   |
| ----------------------------------- | ------------------------------------------------------------ | -------------------------------- |
| Index stale, `--auto-reindex` passed  | Rebuild automatically                                        | N/A (index stays in RAM)         |
| Index stale, `--auto-reindex` omitted | Print warning, use stale                                     | N/A                              |
| Index missing                       | Build automatically (`xray fast`) or error (`xray grep`) | Build in background (async startup) |
| With `--watch`                      | N/A                                                          | Incremental updates, never stale |

Default max age: 24 hours (`--max-age-hours 24`). `--auto-reindex` is an opt-in flag that takes no value. Omit it and xray uses the stale index, with a warning.

For the content index, `created_at` is also the full-scan watermark used by
startup reconciliation and periodic rescans (`mtime > created_at - 2s`).
Targeted watcher and synchronous-edit updates leave it unchanged. A reconciliation
advances it to the walk's start time only when no selected, already-indexed file
fails tokenization. A failed read of an indexed file keeps the previous postings
and watermark, even when other files are updated or removed successfully.
Autosave can persist that partial progress without hiding the failed file after
restart. No index-format change is needed.

Unreadable files without an index entry remain candidates for addition on every
scan, regardless of mtime. They do not block the watermark: there are no old
postings to protect, and advancing the threshold cannot hide their next retry.

Until the failed file can be read, later rescans may retokenize other files newer
than the retained watermark. This is incremental reconciliation, not a full index
rebuild. Content-index age reports the last full build or complete reconciliation,
not the latest individual watcher update.

Caches saved by an older version after it already advanced the watermark past
a failed file may need a one-time rebuild; the lost retry information cannot be
recovered from that cache.

## Index discovery

When loading a content index, the system tries two strategies:

### 1. Exact match

Hash the directory + extensions to get the exact filename:

```rust
fn load_content_index(dir: &str, exts: &str) -> Result<ContentIndex, SearchError> {
    let path = content_index_path_for(dir, exts);  // Deterministic hash
    load_content_index_at_path(&path)              // Dispatch by magic
}
```

### 2. Directory scan (fallback)

If exact match fails (e.g., user indexed with `cs` but queries without specifying extensions), scan all `.word-search` files and check the `root` field:

```rust
fn find_content_index_for_dir(
    dir: &str,
    index_base: &Path,
    expected_exts: &[String],
) -> Result<Option<ContentIndex>, SearchError>;
```

Discovery tries the exact key first, then sorted compatible extension supersets.
Header roots (or optional sidecars when a root is unreadable) attribute alternate
candidates; unrelated roots are skipped. An unreadable alternate with neither root
nor sidecar is unattributable and is skipped with a warning, not blamed on the
queried directory. An exact-key failure remains attributable even without a readable
header. Failures are retained until all candidates have been exhausted, so a good
superset can still satisfy the query. Only `Ok(None)` means no usable cache exists;
`Err` is not a missing-cache signal.

### CLI upgrade and recovery

File v3 and content v5 store the original `hidden` and `no_ignore` traversal flags
in the binary payload (including the sharded content head). JSON `.meta` files are
optional discovery hints, not an authority for those flags. Older v2/v4 layouts
cannot recover them. CLI `fast` and `grep` refuse to overwrite an existing
incompatible or corrupt selected cache and report an error on stderr with exit 1.
Rebuild it explicitly, choosing the desired scope rather than guessing from its
paths:

```text
xray index -d ROOT --hidden --no-ignore
xray content-index -d ROOT -e rs,cs --hidden --no-ignore
```

Omit either option when its inclusion is not intended. Keep any original
`--respect-git-exclude` choice when rebuilding explicitly. Stale compatible
indexes preserve all three traversal settings, original extensions and age on
automatic rebuild. True missing-cache behavior is unchanged (`fast` builds;
`grep` needs explicit extensions); valid empty indexes remain usable.

## Incremental update mechanics

### Content index update (single file)

```
1. Read new file content from disk; on failure, keep the existing entry
2. Tokenize → new_tokens with line numbers (outside the write lock)
3. Under the write lock: path_to_id[path] → file_id, forward[file_id] → old_tokens
4. For each old_token:
     inverted_index[old_token].retain(|p| p.file_id != file_id)
     if posting list empty: inverted_index.remove(old_token)
5. For each new_token:
     inverted_index[new_token].push(Posting{file_id, lines})
6. forward[file_id] = new_tokens.keys()
7. file_token_counts[file_id] = new_total
```

**Time complexity:** O(old_tokens + new_tokens + Σ posting_list_scans). The ~5ms per file figure is from watcher log output during development, not a formal benchmark.

The watcher prepares replacement tokens before purging old postings. If reading
or tokenization fails, it keeps the old postings and counts for a later retry.

**Rename reconciliation:** watcher rename events preserve path roles instead of treating every `Modify(Name)` path as modified content. For paired rename events, the source path is removed and the destination path is updated in the same debounced batch; split `From`/`To` events follow the same rule. Ambiguous rename modes classify existing paths as updates and missing paths as removals. This keeps file-list, content, definition, and call-graph identities aligned without a full reindex, including renames across indexed-extension boundaries.

### Definition index update (single file)

```
1. path_to_id[path] → file_id
2. file_index[file_id] → old_def_indices
3. Remove old_def_indices from: name_index, kind_index, attribute_index, base_type_index
4. Parse file with tree-sitter → new_definitions
5. Assign new indices, insert into all secondary indexes
```

**Note:** Removed definitions leave "holes" in the `definitions` Vec (indices are not reused). This is acceptable because the Vec is only accessed via the secondary indexes, and the memory overhead of a few hundred empty slots is negligible compared to the total index size.

The `method_calls` entries for removed definitions are also cleaned up during `remove_file_definitions`.

Path removal uses the path-aware definition cleanup, which also tombstones the `files[file_id]` slot, removes `path_to_id`, and drops orphaned source/template fingerprints. In watcher and synchronous-edit updates, transient `WouldBlock`, `PermissionDenied`, or `Interrupted` source reads preserve the existing graph and requeue the dirty path. The runtime-only `pending_definition_inputs` registry forces retries independently of mtime for transient reads and optimistic snapshot conflicts; a third transient-read failure performs path-aware tombstoning and leaves a runtime quarantine marker with the observed `(size, mtime_ns)` revision. Periodic scans skip that marker while the disk revision is unchanged; a changed revision or a new filesystem/`xray_edit` event reactivates the path. The marker is not persisted, so a process restart/cold rebuild evaluates the file again. A genuine disappearance or other non-transient parse failure tombstones immediately. Periodic reconciliation removes stale definitions after a non-transient parse failure but preserves the path mapping so the next filesystem scan can retry it.

Definition index v11 stores a monotonic `definition_generation` and `DefinitionInputFingerprint { size, modified_nanos, content_hash }` for every definition-indexed source and each available external Angular template. Source hashes cover the decoded parser input after BOM handling or lossy UTF-8 conversion; external-template hashes cover the validated raw UTF-8 bytes. Incremental updates compare the prepared baseline under the write lock before mutating any index, then publish definitions, Angular records/edges, source states, and fingerprints together. A format-version mismatch triggers the normal automatic rebuild.

### GitHistoryCache

```rust
struct GitHistoryCache {
    format_version: u32,              // Format version for cache invalidation
    head_hash: String,                // SHA-1 of HEAD when cache was built
    branch: String,                   // Default branch name (main/master/develop/trunk)
    built_at: u64,                    // Unix timestamp (seconds)
    commits: Vec<CommitMeta>,         // All commits (compact representation)
    authors: Vec<AuthorEntry>,        // Deduplicated author pool
    subjects: String,                 // Concatenated commit subjects (pool)
    file_commits: HashMap<String, Vec<u32>>,  // file path → commit indices
}

struct CommitMeta {
    hash: [u8; 20],           // SHA-1 hash as raw bytes (not hex string)
    timestamp: i64,           // Unix timestamp (seconds since epoch)
    author_idx: u16,          // Index into GitHistoryCache::authors
    subject_offset: u32,      // Offset into subjects pool
    subject_len: u32,         // Length of subject in subjects pool
}
// Size: 40 bytes per commit (vs ~200 bytes with String fields)

struct AuthorEntry {
    name: String,             // Author display name
    email: String,            // Author email
}
```

**Key properties:**

- **Not extension-dependent:** the git cache is scoped to the repository directory only (no extension in hash), unlike ContentIndex/DefinitionIndex which include extensions in their hash.
- **HEAD validation:** on load, the cache checks if `head_hash` matches current HEAD via `git rev-parse`. Mismatches trigger a rebuild.
- **Atomic write:** saved via temp file (`path.tmp`) + rename to prevent corruption on crash/disk-full.
- **Background build:** built in a separate thread on server startup, ~59 sec for 50K commits. Does not block the event loop.

**Memory vs Disk:**

| Component | In-memory (50K commits) | On disk (LZ4 compressed) |
|---|---|---|
| commits (50K × 40 bytes) | ~2.0 MB | — |
| authors (~500 × ~60 bytes) | ~30 KB | — |
| subjects (50K × ~50 chars) | ~2.5 MB | — |
| file_commits (~65K files) | ~3.0 MB | — |
| **Total** | **~7.6 MB** | **~3–5 MB** |

## Disk I/O Patterns

| Operation          | I/O Pattern                                                       | Duration                |
| ------------------ | ----------------------------------------------------------------- | ----------------------- |
| Index build (content) | Sequential read of all matching files, one large sequential write | ~7–16s (measured)    |
| Index build (definitions) | Sequential read + tree-sitter parse, one large sequential write | ~16–32s (measured) |
| Index load         | One large sequential read + decompress + deserialize              | 0.055–0.689s (measured) |
| Search query       | Pure in-memory (no disk I/O)                                      | 0.5-44ms (measured)     |
| Incremental update | One small random read (file content) + in-memory update           | ~5ms (from logs)        |
| Index save         | One large sequential write (only on full reindex)                 | ~2s (estimated)         |
| Git cache build    | Streaming read of `git log` output (~163 MB for 50K commits)      | ~59s (measured)         |
| Git cache load     | One sequential read + decompress + deserialize (~3-5 MB)          | ~100ms (measured)       |
| Git cache save     | Serialize + compress + atomic write                               | ~100ms (estimated)      |

The MCP server never touches disk during normal query processing. All searches are in-memory.
