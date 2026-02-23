# Storage Model

## Index File Layout

All indexes are stored under a platform-specific data directory:

| OS      | Path                                          |
| ------- | --------------------------------------------- |
| Windows | `%LOCALAPPDATA%\search-index\`                |
| macOS   | `~/Library/Application Support/search-index/` |
| Linux   | `~/.local/share/search-index/`                |

```
search-index/
├── Repos_MyProject_a1b2c3d4.file-list           ← FileIndex
├── Repos_MyProject_f0e1d2c3.word-search          ← ContentIndex (cs,xml)
├── Repos_MyProject_12345678.code-structure        ← DefinitionIndex (cs,xml)
├── rust_search_9876fedc.word-search                    ← ContentIndex (rs)
└── rust_search_aabb0011.code-structure                 ← DefinitionIndex (rs)
```

### File Naming Scheme

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
| `.file-list`        | FileIndex       | File name lookup (`search_fast`)           |
| `.word-search`      | ContentIndex    | Full-text token search (`search_grep`)     |
| `.code-structure`   | DefinitionIndex | AST definitions & callers (`search_definitions`, `search_callers`) |
| `.git-history`      | GitHistoryCache | Git commit history cache (`search_git_history`, `search_git_authors`, `search_git_activity`) |

### Semantic Prefix Rules

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

**Implication:** Indexing the same directory with different extension sets produces different files. `search-index content-index -d C:\Projects -e cs` and `search-index content-index -d C:\Projects -e cs,sql` create two separate `.word-search` files.

### Collision Handling

FNV-1a provides 64-bit hashes, truncated to 32 bits for the filename. Hash collisions are possible but extremely unlikely for realistic use — birthday bound is ~77K directories for 50% collision probability. No collision detection is implemented — a collision would silently overwrite the previous index.

## Serialization Format

All indexes use [bincode](https://docs.rs/bincode/1/bincode/) v1 for serialization, wrapped in [LZ4 frame compression](https://crates.io/crates/lz4_flex) for reduced disk usage and faster I/O:

```rust
// Write (LZ4-compressed)
let file = File::create(path)?;
let mut writer = BufWriter::new(file);
writer.write_all(b"LZ4S")?;  // magic bytes
let mut encoder = lz4_flex::frame::FrameEncoder::new(writer);
bincode::serialize_into(&mut encoder, &index)?;
encoder.finish()?.flush()?;

// Read (auto-detects compressed vs legacy uncompressed)
let result = load_compressed::<ContentIndex>(&path, "content-index");
```

### Bincode Properties

| Property    | Value                                                                                   |
| ----------- | --------------------------------------------------------------------------------------- |
| Format      | Little-endian, variable-length integers                                                 |
| Schema      | Implicit — derived from Rust struct layout                                              |
| Versioning  | None — format changes require reindex                                                   |
| Compression | LZ4 frame compression (`lz4_flex`); magic bytes `LZ4S` prefix; backward-compatible with legacy uncompressed files |
| Atomicity   | Whole-file write (`fs::write`) — atomic on most FSes if < 4KB, otherwise not guaranteed |

### Sizes on Disk

Measured on a real codebase (from `search-index info` and build logs):

| Index Type      | Files Indexed   | Content                 | Disk Size |
| --------------- | --------------- | ----------------------- | --------- |
| FileIndex       | 333,875 entries | Paths + metadata        | 47.8 MB   |
| ContentIndex    | 48,599 files    | 33M tokens, 754K unique | 241.7 MB  |
| DefinitionIndex | ~48,600 files   | ~846K definitions + ~2.4M call sites | ~324 MB   |

In-memory size is larger than on-disk due to HashMap overhead and struct alignment, but has not been separately measured.

## Data Structures on Disk

### FileIndex

```rust
struct FileIndex {
    root: String,           // Canonical directory path
    created_at: u64,        // Unix timestamp (seconds)
    max_age_secs: u64,      // Staleness threshold
    entries: Vec<FileEntry>, // All files and directories
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
    created_at: u64,
    max_age_secs: u64,
    files: Vec<String>,                          // file_id → path
    index: HashMap<String, Vec<Posting>>,         // token → postings
    total_tokens: u64,                           // Total tokens indexed
    extensions: Vec<String>,                     // Extensions indexed
    file_token_counts: Vec<u32>,                 // file_id → token count (TF denom)
    forward: Option<HashMap<u32, Vec<String>>>,  // file_id → tokens (watch mode)
    path_to_id: Option<HashMap<PathBuf, u32>>,   // path → file_id (watch mode)
}

struct Posting {
    file_id: u32,           // Index into ContentIndex.files
    lines: Vec<u32>,        // Line numbers where token appears
}
```

**Watch mode fields:** `forward` and `path_to_id` are only populated when the MCP server starts with `--watch`. They are serialized as `None` when saving to disk (not needed for persistent storage, rebuilt on load).

### DefinitionIndex

```rust
struct DefinitionIndex {
    root: String,
    created_at: u64,
    extensions: Vec<String>,
    files: Vec<String>,                               // file_id → path
    definitions: Vec<DefinitionEntry>,                 // All definitions
    name_index: HashMap<String, Vec<u32>>,             // name → def indices
    kind_index: HashMap<DefinitionKind, Vec<u32>>,     // kind → def indices
    attribute_index: HashMap<String, Vec<u32>>,        // attribute → def indices
    base_type_index: HashMap<String, Vec<u32>>,        // base type → def indices
    file_index: HashMap<u32, Vec<u32>>,                // file_id → def indices
    path_to_id: HashMap<PathBuf, u32>,                 // path → file_id
    method_calls: HashMap<u32, Vec<CallSite>>,         // def_idx → call sites (for search_callers "down")
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
    method_name: String,          // Name of the called method
    receiver_type: Option<String>, // Resolved type of receiver (e.g., "IUserService")
    line: u32,                    // Line number of the call site
}
```

## Staleness Model

Each index stores `created_at` and `max_age_secs`. Staleness check:

```rust
fn is_stale(&self) -> bool {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    now - self.created_at > self.max_age_secs
}
```

| Behavior                            | `search-index fast` / `search-index grep`                                | `search-index serve`                   |
| ----------------------------------- | ------------------------------------------------------------ | -------------------------------- |
| Index stale, `--auto-reindex true`  | Rebuild automatically                                        | N/A (index stays in RAM)         |
| Index stale, `--auto-reindex false` | Print warning, use stale                                     | N/A                              |
| Index missing                       | Build automatically (`search-index fast`) or error (`search-index grep`) | Build in background (async startup) |
| With `--watch`                      | N/A                                                          | Incremental updates, never stale |

Default max age: 24 hours (`--max-age-hours 24`).

## Index Discovery

When loading a content index, the system tries two strategies:

### 1. Exact Match

Hash the directory + extensions to get the exact filename:

```rust
fn load_content_index(dir: &str, exts: &str) -> Option<ContentIndex> {
    let path = content_index_path_for(dir, exts);  // Deterministic hash
    let data = fs::read(&path).ok()?;
    bincode::deserialize(&data).ok()
}
```

### 2. Directory Scan (Fallback)

If exact match fails (e.g., user indexed with `cs` but queries without specifying extensions), scan all `.word-search` files and check the `root` field:

```rust
fn find_content_index_for_dir(dir: &str) -> Option<ContentIndex> {
    for entry in fs::read_dir(index_dir()) {
        if path.extension() == "word-search" {
            let index: ContentIndex = load_compressed(&path)?;
            if index.root == canonical_dir {
                return Some(index);
            }
        }
    }
    None
}
```

This scan reads and deserializes each `.word-search` file header — slow if many indexes exist. In practice, users have 1-5 indexes.

## Incremental Update Mechanics

### Content Index Update (single file)

```
1. path_to_id[path] → file_id
2. forward[file_id] → old_tokens
3. For each old_token:
     inverted_index[old_token].retain(|p| p.file_id != file_id)
     if posting list empty: inverted_index.remove(old_token)
4. Read new file content from disk
5. Tokenize → new_tokens with line numbers
6. For each new_token:
     inverted_index[new_token].push(Posting{file_id, lines})
7. forward[file_id] = new_tokens.keys()
8. file_token_counts[file_id] = new_total
```

**Time complexity:** O(old_tokens + new_tokens + Σ posting_list_scans). The ~5ms per file figure is from watcher log output during development, not a formal benchmark.

### Definition Index Update (single file)

```
1. path_to_id[path] → file_id
2. file_index[file_id] → old_def_indices
3. Remove old_def_indices from: name_index, kind_index, attribute_index, base_type_index
4. Parse file with tree-sitter → new_definitions
5. Assign new indices, insert into all secondary indexes
```

**Note:** Removed definitions leave "holes" in the `definitions` Vec (indices are not reused). This is acceptable because the Vec is only accessed via the secondary indexes, and the memory overhead of a few hundred empty slots is negligible compared to the total index size.

The `method_calls` entries for removed definitions are also cleaned up during `remove_file_definitions`.

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

- **Not extension-dependent** — the git cache is scoped to the repository directory only (no extension in hash), unlike ContentIndex/DefinitionIndex which include extensions in their hash
- **HEAD validation** — on load, the cache checks if `head_hash` matches current HEAD via `git rev-parse`. Mismatches trigger a rebuild
- **Atomic write** — saved via temp file (`path.tmp`) + rename to prevent corruption on crash/disk-full
- **Background build** — built in a separate thread on server startup, ~59 sec for 50K commits. Does not block the event loop

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
| Index build        | Sequential read of all matching files, one large sequential write | 7-16s (measured)        |
| Index load         | One large sequential read + deserialize                           | 0.055-0.689s (measured) |
| Search query       | Pure in-memory (no disk I/O)                                      | 0.5-44ms (measured)     |
| Incremental update | One small random read (file content) + in-memory update           | ~5ms (from logs)        |
| Index save         | One large sequential write (only on full reindex)                 | ~2s (estimated)         |
| Git cache build    | Streaming read of `git log` output (~163 MB for 50K commits)      | ~59s (measured)         |
| Git cache load     | One sequential read + decompress + deserialize (~3-5 MB)          | ~100ms (measured)       |
| Git cache save     | Serialize + compress + atomic write                               | ~100ms (estimated)      |

The MCP server never touches disk during normal query processing. All searches are in-memory.
