# Design: Git History Cache (Compact In-Memory Cache)

> **Main design:** [`git-history-integration-design.md`](git-history-integration-design.md)
> **Performance debate:** [`debate-07-git-performance.md`](debate-07-git-performance.md)
> **Design review (debate):** [`debate-08-git-history-cache.md`](debate-08-git-history-cache.md)
> **Scope and depth strategy:** [`debate-09-cache-scope-strategy.md`](debate-09-cache-scope-strategy.md)
> **Remaining areas:** [`debate-10-remaining-design-areas.md`](debate-10-remaining-design-areas.md)

## 1. Motivation

### Problem
Git CLI (`git log -- <file>`) on a large monorepo (Monorepo: ~50K commits, ~65K files) takes 2-6 sec per file. For an LLM agent making several sequential queries (history → authors → diff), the total wait is ~10-15 sec.

### Goal
Sub-millisecond responses for `xray_git_history`, `xray_git_authors`, `xray_git_activity`. Only `xray_git_diff` remains via git CLI (diffs cannot be cached — too large and rarely repeated).

### Measurements (large monorepo)

| Metric | Value |
|--------|-------|
| `git log --name-only --no-renames master` | 59 sec, 163 MB raw output |
| `git log --name-only --all` | 127 sec, 414 MB raw output |
| RAM (naive HashMap with String) | ~200-400 MB |
| RAM (compact representation) | **~5-10 MB** |

### 1.1 Decision on Cache Scope and Depth

> Full debate: [`debate-09-cache-scope-strategy.md`](debate-09-cache-scope-strategy.md)

The cache stores the **full history** of the default branch (status quo confirmed by debate, unanimous decision by all participants).

**Rationale:**
- 7.5 MB RAM = **1.8%** of total server memory (~410 MB). RAM savings are not a motivation.
- 59 sec is a **one-time** cost for the first build. Disk cache ensures subsequent restarts in ~100 ms.
- All limited approaches (by time, depth, directory, tiered, lazy) trade **query coverage** for build time savings that the disk cache already provides.
- A limited cache creates a **risk of misleading answers**: the cache doesn't know what it doesn't know. Example: `xray_git_authors` on a file created outside the time window will show an incomplete author list. The LLM agent will treat this as fact.

**Escape hatch for the future:** CLI flag `--git-cache-since` for repositories with >200K commits (~4 min build time). Documented but **NOT implemented in MVP** — until a specific user request.

---

## 2. Architecture

> **[debate-09](debate-09-cache-scope-strategy.md):** Git cache lives in search-index (same MCP server, same process).
> Git cache adds **+1.8% RAM** and **+1 background thread** — negligible overhead.
> A separate MCP server for git tools **rejected**: doubles configuration (2 servers in MCP config),
> doubles processes (problem from [`shared-server-design.md`](shared-server-design.md)),
> UX degradation for LLM agents (tools from two servers in different contexts).
> **Recommendation:** extract generic "background index builder" pattern from [`serve.rs`](../src/cli/serve.rs)
> to reduce copy-paste when adding new index types.

### 2.1 Compact Cache Structure

```rust
/// Git history cache — compact in-memory representation.
/// 50K commits × ~65K files ≈ 5-10 MB RAM.
///
/// Storage type: `Arc<RwLock<Option<GitHistoryCache>>>`.
/// `None` means "cache not ready yet" → queries go through CLI fallback.
/// `Some(cache)` → cache ready for queries.
/// Build-then-swap pattern: new cache is built in a separate allocation,
/// then swapped under write lock in microseconds (pointer swap).
pub struct GitHistoryCache {
    /// Cache format version. On mismatch → full rebuild.
    /// [debate-10]: used instead of custom magic bytes (GHC1).
    format_version: u32,
    
    /// SHA-1 of HEAD when cache was built.
    /// Used for invalidation: if HEAD changed → incremental update.
    head_hash: String,
    
    /// Branch name the cache was built for (main/master/develop/trunk).
    /// Auto-detected on first build.
    branch: String,
    
    /// Timestamp when cache was built.
    built_at: u64,
    
    /// All commits (not necessarily sorted by time — sorted at query time).
    /// Index into this vec is the "commit ID" used everywhere else.
    commits: Vec<CommitMeta>,
    
    /// Author pool — deduplicated author strings.
    /// CommitMeta stores index into this vec.
    authors: Vec<AuthorEntry>,
    
    /// Subject pool — all commit subjects concatenated.
    /// CommitMeta stores offset + length into this string.
    subjects: String,
    
    /// Main index: normalized file path → vec of commit IDs.
    /// Keys are as-is from git output (forward slashes, repo-relative).
    /// Query input normalization (\ → /, strip ./, absolute paths) is
    /// a separate function applied BEFORE lookup.
    file_commits: HashMap<String, Vec<u32>>,
}

/// 38 bytes per commit (vs ~200 bytes with String fields).
/// [debate-08](debate-08-git-history-cache.md): subject_len expanded to u32.
struct CommitMeta {
    /// SHA-1 hash as raw bytes (not hex string).
    hash: [u8; 20],       // 20 bytes
    
    /// Unix timestamp (seconds since epoch).
    timestamp: i64,        // 8 bytes
    
    /// Index into GitHistoryCache::authors.
    /// u16 → max 65,535 unique (name, email) pairs.
    /// Runtime check: if authors.len() > 65535 → CLI fallback + warning.
    /// 99.9% of repositories: 500-2000 authors (30-130× margin).
    author_idx: u16,       // 2 bytes (supports up to 65K unique authors)
    
    /// Offset into GitHistoryCache::subjects string pool.
    subject_offset: u32,   // 4 bytes
    
    /// Length of subject in subjects pool.
    /// [debate-08]: u32 instead of u16 — eliminates overflow on subjects >65535 bytes.
    /// Cost: +2 bytes per commit = +100 KB for 50K commits (1.3% increase).
    subject_len: u32,      // 4 bytes
}
// Total: 38 bytes per commit

/// Author name + email (deduplicated).
struct AuthorEntry {
    name: String,
    email: String,
}
```

### 2.2 RAM Estimate

| Component | Formula | Shared repo |
|-----------|---------|-------------|
| commits | 50K × 38 bytes | **1.9 MB** |
| authors | ~500 unique × ~60 bytes | **30 KB** |
| subjects | 50K × ~50 chars average | **2.5 MB** |
| file_commits | ~65K files × ~5 commit_ids × 4 bytes + HashMap overhead | **~3 MB** |
| **Total RAM** | | **~7.6 MB** |

For comparison:
- Content index (xray_grep): ~200 MB RAM
- Definition index (xray_definitions): ~200 MB RAM
- **Git history cache: ~7.6 MB** — negligible

### 2.3 On-Disk Format

> **[debate-10](debate-10-remaining-design-areas.md):** Using existing [`save_compressed()`](../src/index.rs:25)/[`load_compressed()`](../src/index.rs:46)
> from [`src/index.rs`](../src/index.rs) and [`src/definitions/storage.rs`](../src/definitions/storage.rs).
> Format: bincode v1 + lz4_flex — **same crates already in the codebase** (zero new dependencies).
> Magic bytes: `LZ4S` (shared with content/def indexes), **not** custom `GHC1`.
> Versioning: `format_version: u32` field in `GitHistoryCache` struct.
> On deserialization error → log warning + full rebuild (no crash).
>
> **Atomic write:** `format!("{}.tmp", path.display())`, **NOT** `path.with_extension("tmp")`
> (`.with_extension()` replaces the extension instead of appending: `cache.git-history` → `cache.tmp`).

| Format | Size (estimate) |
|--------|-----------------|
| bincode (raw) | ~10-15 MB |
| bincode v1 + lz4_flex (via `save_compressed()`) | **~3-5 MB** |

---

## 3. Cache Lifecycle

### 3.1 Startup Flow

> **Query input normalization** ([debate-08](debate-08-git-history-cache.md), area 3):
> Keys in `file_commits` are stored as-is from git output. Before lookup, the input path must be normalized:
> - `\` → `/` (Windows backslashes)
> - Remove leading `./`
> - Handle absolute paths (strip repo prefix)
> - Collapse `//` → `/`
> - `.trim()` whitespace
>
> For non-ASCII paths: add `-c core.quotePath=false` to git commands when building the cache,
> to get raw UTF-8 paths without git quoting (see [`mod.rs`](../src/git/mod.rs)).

```
MCP SERVER STARTUP
    │
    ├── Try loading cache from disk
    │   ├── File exists → bincode+LZ4 deserialization (~100 ms)
    │   │   ├── git cat-file -t cache.head_hash → object exists?
    │   │   │   ├── No → repo was re-cloned, FULL REBUILD
    │   │   │   └── Yes ↓
    │   │   ├── git rev-parse <branch> → matches cache.head_hash?
    │   │   │   ├── Yes → cache is current ✅ (ready in ~100 ms)
    │   │   │   └── No ↓
    │   │   ├── git merge-base --is-ancestor cache.head_hash <branch>?
    │   │   │   ├── Yes → incremental update (background)
    │   │   │   │   └── git log --name-only cache.head_hash..<branch> (~sec)
    │   │   │   │       └── Append new commits to cache (append-to-end)
    │   │   │   │       └── Save to disk
    │   │   │   │       └── Mark "ready" ✅
    │   │   │   └── No → force push / rebase / amend → FULL REBUILD
    │   │   └── (while updating — queries go through CLI fallback)
    │   │
    │   └── File not found → full build (background)
    │       └── Detect default branch (main → master → develop → trunk)
    │       └── git log --name-only --no-renames <branch> (~59 sec)
    │           └── Streaming parse (don't accumulate the full 163 MB in RAM)
    │           └── Build GitHistoryCache (~7.6 MB)
    │           └── Save to disk (~3-5 MB, atomic write)
    │           └── Mark "ready" ✅
    │
    └── (while building — queries go through CLI fallback, Phase 1)
```

### 3.2 Query Flow

> **[debate-10](debate-10-remaining-design-areas.md):** Query performance.
> - `file_commits` — **HashMap** (not BTreeMap). Directory scan of 65K keys = 1-3 ms — sufficient.
> - **Order of operations:** filter by date → sort by timestamp → truncate to `maxResults` (correctness first).
> - Hot file (5K commits): O(5K) scan = ~50 μs — adequate.
> - Directory scan (50K keys): 1-3 ms — adequate for sub-10ms target.
> - Worst case: `xray_git_activity(path="")` without date filter — existing behavior; hint "use date filter for large results".

```
QUERY: xray_git_history(repo=".", file="src/main.rs", maxResults=10)
    │
    ├── Cache ready?
    │   ├── YES → cache.file_commits.get("src/main.rs")
    │   │       → Vec<u32> commit IDs
    │   │       → filter by date (from/to → commit.timestamp)
    │   │       → sort by timestamp descending
    │   │       → .take(maxResults)
    │   │       → map to CommitInfo (hash hex, date, author, subject)
    │   │       → JSON response
    │   │       → **<1 ms** ✅
    │   │
    │   └── NO → FALLBACK: git log (Phase 1)
    │             → **~2-3 sec**
    │
    └── Filters applied in-memory:
        ├── from/to → filter by commit.timestamp
        ├── sort by timestamp → correct ordering
        ├── maxResults → truncate AFTER filtering and sorting
        └── (follow NOT supported in cache — only exact path match)
```

### 3.3 Invalidation

> **[debate-08](debate-08-git-history-cache.md):** Incremental update (`OLD..NEW`) works ONLY if OLD_HEAD is an ancestor of NEW_HEAD (linear history). On force push, rebase, amend — OLD_HEAD may be unreachable.

| Event | Action |
|-------|--------|
| `git rev-parse <branch>` ≠ `cache.head_hash` | `merge-base --is-ancestor` check → incremental OR full rebuild |
| `git merge-base --is-ancestor` = true | `git log --name-only OLD..NEW`, append to cache |
| `git merge-base --is-ancestor` = false | Force push / rebase → **full rebuild** |
| `git cat-file -t cache.head_hash` = error | Repo was re-cloned → **full rebuild** |
| ~~`xray_reindex` called~~ | ~~Rebuild cache~~ — **cancelled**: git cache NOT tied to reindex (see §3.5) |
| Watcher: `.git/refs/heads/<branch>` changed | Check HEAD, incrementally update |
| New day (>24h) | Check HEAD |

**Default branch auto-detection:**
```rust
fn detect_default_branch(repo_path: &str) -> String {
    for branch in ["main", "master", "develop", "trunk"] {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", branch])
            .current_dir(repo_path)
            .output();
        if output.map(|o| o.status.success()).unwrap_or(false) {
            return branch.to_string();
        }
    }
    "HEAD".to_string() // fallback
}
```

### 3.4 Streaming Parse

When building the cache, do **NOT** load the entire 163 MB output into a String. Parse it as a stream.

> **[debate-08](debate-08-git-history-cache.md):** Using `␞` (U+241E, FIELD_SEP) instead of `|` as the field separator.
> **Reason:** pipe (`|`) appears in commit subjects and author names (e.g., ADO merge commits `Merged PR 12345: Feature A | Feature B`), which leads to **silent data loss** — the subject gets truncated without error.
> **Precedent:** [`mod.rs`](../src/git/mod.rs) already uses `␞` for this purpose.
> **Subject is the last field:** parsed as `fields[4..].join(sep)` for additional robustness in case `␞` appears in data.

```rust
const FIELD_SEP: &str = "␞"; // U+241E — same as src/git/mod.rs

let branch = detect_default_branch(repo_path);
let mut child = Command::new("git")
    .args([
        "-c", "core.quotePath=false",  // raw UTF-8 paths without quoting
        "log", "--name-only", "--no-renames",
        &format!("--format=COMMIT:%H␞%aI␞%aE␞%aN␞%s"),
        &branch,
    ])
    .current_dir(repo_path)
    .stdout(Stdio::piped())
    .spawn()?;

let reader = BufReader::new(child.stdout.take().unwrap());
for line in reader.lines() {
    let line = line?;
    if line.starts_with("COMMIT:") {
        // Parse: line[7..].split("␞") → [hash, date, email, name, subject...]
        // Subject = fields[4..].join("␞") — protection against ␞ in subject
    } else if !line.is_empty() {
        // File path — add to file_commits
    }
}
```

RAM usage during parsing: ~7.6 MB (final cache) + ~4 KB BufReader buffer. No 163 MB in memory.

### 3.5 Integration with xray_reindex

> **Reference:** [`debate-12-parallel-git-log.md`](debate-12-parallel-git-log.md)

- `xray_reindex` rebuilds **ONLY** the content index (no changes)
- `xray_reindex_definitions` rebuilds **ONLY** the definition index (no changes)
- Git cache is **NOT tied** to reindex — three reasons:
  1. **Orthogonal data** — content/def indexes work with files in the working tree, git cache works with `.git/` (commit history)
  2. **Different build times** — content/def index: 5-30 sec, git cache: ~59 sec. Tying them would mean 59 sec wait on every reindex
  3. **Different triggers** — content/def indexes are invalidated on file changes, git cache is invalidated on new commits (push/pull)

**Automatic git cache updates:**
- Startup → load from disk + check HEAD
- Watcher → `.git/refs/heads/<branch>` changed → incremental update
- Lazy HEAD check → >24h since last check

**`xray_reindex_git` — NOT needed in MVP** (YAGNI). Emergency reset: delete `.git-history` file + restart server.

**No conflicts during parallel operation:** each index lives in its own `Arc<RwLock>`, builds happen in separate threads. Content reindex, definition reindex, and git cache rebuild can run simultaneously without mutual blocking.

**Git cache status:** shown in `xray_info` (general server information), **NOT** in `xray_reindex` response (which relates only to the content index).

### 3.6 Git Pull / Checkout Scenarios

| Scenario | What happens | Action | Time |
|----------|-------------|--------|------|
| `git pull` (fast-forward) | Watcher: `.git/refs/heads/<branch>` changed → `merge-base --is-ancestor` = YES | Incremental append `old..new` | ~seconds |
| `git pull --rebase` / force push | `merge-base --is-ancestor` = NO | Full rebuild in background, CLI fallback | ~59 sec |
| `git checkout feature-branch` | HEAD → feature-branch, but `cache.branch` = master. `git rev-parse master` unchanged | Cache remains valid for master. Feature-only files → CLI fallback | 0 (cache not rebuilt) |
| `git checkout main` (back) | HEAD → main, cache valid | Nothing to do | 0 |
| `git merge feature → main` | `refs/heads/main` changed → incremental | Append merge commit(s) | ~seconds |

> **Note:** Multi-branch cache — confirmed **NOT needed for MVP**. Feature-branch file queries use CLI fallback.

**Typical LLM agent scenario:** "What changed in file X from date A to date B?"

1. **`xray_git_history`** (cache, <1 ms) → list of commits (hash, date, author, subject) — change metadata
2. **`xray_git_diff`** (git CLI, 2-3 sec) → actual diff for a specific commit — added/removed lines

The cache stores **"which commits touched a file"** (reverse index [`file_commits`](user-stories/git-history-cache-design.md:95)). The actual changes (diff) are always via git CLI, since diffs are unpredictable in size and rarely repeated.

---

## 4. Which Tools Use the Cache

| Tool | Cache | CLI fallback | Notes |
|------|-------|-------------|-------|
| `xray_git_history` | ✅ file_commits lookup | git log -- file | Subject only from cache. For fullMessage → git show |
| `xray_git_authors` | ✅ aggregate by author_idx | git log -- file | Aggregation O(N) over commit IDs: ~50 μs for 5K commits |
| `xray_git_activity` | ✅ filter file_commits by path prefix + date | git log --name-only | Prefix matching: `== path \|\| starts_with(path + "/")` |
| `xray_git_diff` | ❌ Always CLI | git diff hash^..hash -- file | Diffs cannot be cached (~KB-MB per commit) |

> **[debate-10](debate-10-remaining-design-areas.md):** Path matching and normalization.
>
> **Prefix matching for `xray_git_activity`:**
> ```rust
> fn matches_path_prefix(file_path: &str, query_path: &str) -> bool {
>     if query_path.is_empty() { return true; } // entire repo
>     file_path == query_path || file_path.starts_with(&format!("{}/", query_path))
> }
> ```
>
> **Query path normalization:**
> - `""` → match all (entire repo)
> - `"."` → normalize to `""` (root)
> - Strip trailing `/`
> - `\` → `/` (Windows backslashes)
> - Collapse `..` segments, strip `./`
>
> **Rename tracking:** the cache does not support `--follow`. Cache responses for `xray_git_history` and `xray_git_authors`
> include a hint: `"Rename tracking not available from cache."` `follow: true` is NOT applicable to the cache (CLI-only in Phase 1).
>
> **Deleted files:** history of deleted files is available from the cache — they exist in `file_commits` (git log records their paths).

### What the Cache Does NOT Support

| Feature | Reason | Fallback |
|---------|--------|----------|
| `--follow` (rename tracking) | Cache stores exact file paths; hint in response | CLI with `--follow` (Phase 1) |
| `fullMessage` (full commit body) | Bodies too large for cache | `git show <hash> --format=%B` |
| Diff/patch | Even more data | `git diff` |

---

## 5. Phase 1 and Phase 2 Compatibility

### 5.1 Concurrency Model

> **[debate-08](debate-08-git-history-cache.md):** Build-then-swap pattern for minimizing write lock.

**Storage type:** `Arc<RwLock<Option<GitHistoryCache>>>` + separate `Arc<Mutex<CacheStatus>>`.

```rust
enum CacheStatus {
    NotStarted,
    Building { started_at: Instant },
    Ready,
    Failed { error: String },
}
```

**Full build (59 sec):**
1. Background thread: `let new_cache = build_cache_from_git_log();` — **59 sec, no lock**
2. `{ let mut guard = cache.write(); *guard = Some(new_cache); }` — **~μs (pointer swap)**
3. Write lock held for **microseconds** (only pointer swap)

**Incremental update (5-500 commits):**
1. Background thread: parse `git log OLD..NEW` → `Vec<CommitMeta>`, new file mappings
2. Write lock: extend `cache.commits`, extend `file_commits` entries, update `head_hash` — **~μs**
3. Commits don't need to be sorted in Vec → sorted by timestamp at query time

**Separate `CacheStatus`:** `Arc<Mutex<CacheStatus>>` — lock-free status check from handlers without reading the main `RwLock`. Read lock on status = ~nanoseconds.

### 5.2 Phase Diagram

```
┌─────────────────────────────────────────┐
│          xray_git_history/            │
│          xray_git_authors/            │
│          xray_git_activity            │
│                                         │
│  ┌──── Cache ready? ────┐              │
│  │                       │              │
│  YES                     NO             │
│  │                       │              │
│  ▼                       ▼              │
│  HashMap lookup    git log (Phase 1)    │
│  <1 ms             ~2-3 sec            │
│                    + hint: cache        │
│                      building           │
│                                         │
├─────────────────────────────────────────┤
│          xray_git_diff               │
│                                         │
│  ALWAYS: git diff (Phase 1)            │
│  ~2-3 sec                              │
└─────────────────────────────────────────┘
```

**Phase 1 NEVER becomes irrelevant:**
1. CLI fallback while cache is building (~59 sec on first start)
2. CLI for `xray_git_diff` (always)
3. CLI for `follow: true` (cache does not support rename tracking)
4. CLI for `fullMessage: true` (cache stores only subject)

---

## 6. Risks and Mitigations

| Risk | Probability | Mitigation |
|------|------------|-----------|
| 59 sec background build blocks resources | Low | Don't add nice priority — IO-bound single thread, no precedent in codebase ([debate-10](debate-10-remaining-design-areas.md)) |
| HEAD changed during build | Medium | After build: re-check HEAD. If changed → incremental append |
| Repo with 200K+ commits | Medium | ~30 MB RAM (7% of total) — linear scaling. Escape hatch `--git-cache-since` documented ([debate-10](debate-10-remaining-design-areas.md)) |
| file_commits HashMap memory fragmentation | Low | `shrink_to_fit()` is sufficient: peak 15 MB during build, then ~7.5 MB ([debate-10](debate-10-remaining-design-areas.md)) |
| Race condition: cache updating + query | **Eliminated** | build-then-swap + `RwLock` + bounds checks in parser. Poison handling: `unwrap_or_else` ([debate-10](debate-10-remaining-design-areas.md)) |
| Watcher: file in .git/ changed but not HEAD | Low | packed-refs doesn't change SHA; branch updates create loose ref files → watcher will notice ([debate-10](debate-10-remaining-design-areas.md)) |
| **Force push / rebase breaks incremental update** | Medium | `git merge-base --is-ancestor` check → full rebuild if not ancestor ([debate-08](debate-08-git-history-cache.md)) |
| **Repo re-cloned to same path** | Low | `git cat-file -t cache.head_hash` on cache load → full rebuild if object not found |
| **Pipe `\|` in commit subjects** | High | Using `␞` (U+241E) as separator — precedent in [`mod.rs`](../src/git/mod.rs) |
| **subject_len overflow (>65535 bytes)** | Low | `subject_len: u32` instead of `u16` (+100 KB for 50K commits) |
| **>65K unique authors** | Very low | Runtime check + CLI fallback with actionable warning |
| Disk full when saving cache | Medium | Atomic write: temp file via `format!("{}.tmp", path.display())` + rename |
| **Missing commit-graph in repo** | Medium | Without commit-graph `git log` runs 2-5× slower. At startup: check `.git/objects/info/commit-graph`. If missing → hint in log: `"Hint: run 'git commit-graph write --reachable' to speed up git history building by 2-5×"`. **NOT auto-created** — search tool is read-only with respect to `.git/`. See [`debate-12-parallel-git-log.md`](debate-12-parallel-git-log.md) |

---

## 7. Cache File Format on Disk

> **[debate-10](debate-10-remaining-design-areas.md):** Format updated — using shared `save_compressed()`/`load_compressed()`.

```
[4 bytes: magic "LZ4S"]  ← shared magic, same as content/def indexes
[LZ4-Frame-compressed bincode v1 of GitHistoryCache]
```

Format versioning: `format_version: u32` field in `GitHistoryCache` struct (not custom magic bytes).
On deserialization error → log warning + full rebuild (no crash).

File name: `<semantic_prefix>_<hash>.git-history` (analogous to `.word-search` and `.code-structure`).

Example: `repos_mainproject_a1b2c3d4.git-history`

> **[debate-08](debate-08-git-history-cache.md) + [debate-10](debate-10-remaining-design-areas.md):** Atomic write — write via temp file + rename to prevent corrupt cache on disk-full or crash:
> ```rust
> let tmp_name = format!("{}.tmp", cache_path.display());
> let tmp = PathBuf::from(tmp_name);
> save_compressed(&tmp, &cache, "git-history")?;
> std::fs::rename(&tmp, &cache_path)?;
> ```
> **NOT** using `path.with_extension("tmp")` — replaces the extension instead of appending
> (`cache.git-history` → `cache.tmp` instead of `cache.git-history.tmp`).

---

## 8. What is NOT Included in Cache MVP

- [ ] Watcher integration (auto-update on git fetch)
- [ ] `fullMessage` via git show (separate RPC for commit body)
- [ ] Parallel build (single thread is sufficient)
- [ ] Merge commit filtering (`--first-parent`)
- [ ] Branch-aware cache (cache only one branch — auto-detected)
- [ ] SHA-256 support (when git ecosystem transitions)
- [ ] Garbage collection for orphaned `.git-history` files

---

## 9. Decisions from Debates

> Full debate: [`debate-08-git-history-cache.md`](debate-08-git-history-cache.md)

| # | Area | Decision | Rationale |
|---|------|----------|-----------|
| 1 | Parser format | `␞` (U+241E) separator instead of `\|` | Existing precedent in [`mod.rs`](../src/git/mod.rs), zero collision probability. Pipe in subjects is common, leads to silent data loss |
| 2 | Subject field | Last in format string, rejoin via `fields[4..].join(sep)` | Protection against separator in data |
| 3 | `subject_len` | `u32` instead of `u16` | +100 KB for 50K commits, eliminates overflow on long subjects |
| 4 | `author_idx` | `u16` with runtime check + CLI fallback | 65K sufficient for 99.9% of repositories, fail-loud if exceeded |
| 5 | HashMap key | Raw git output, query input normalization separate | Git already normalizes paths. Input normalization: `\ → /`, strip `./`, absolute paths |
| 6 | Git quoting | `-c core.quotePath=false` | Prevents mismatches for non-ASCII paths |
| 7 | Invalidation | HEAD-based + `merge-base --is-ancestor` check | Handles force push, rebase correctly |
| 8 | Branch detection | Try main/master/develop/trunk, store in cache | Explicit, deterministic detection |
| 9 | Re-clone detection | `git cat-file -t cache.head_hash` on load | Detects orphaned cache files |
| 10 | Concurrency | `Arc<RwLock<Option<GitHistoryCache>>>` + build-then-swap | Write lock ~μs, explicit "building" state via `None` |
| 11 | Incremental update | Append-to-end + sort-at-query-time | O(1) append, no index shifting |
| 12 | CLI fallback | Silent fallback + hint in summary JSON | Better UX during 59-sec build window |
| 13 | Cache status | Separate `Arc<Mutex<CacheStatus>>` | Lock-free status checks for handlers |
| 14 | File name | `<prefix>_<hash>.git-history`, reuse existing functions | Consistency with `.word-search`, `.code-structure` |
| 15 | Magic bytes | ~~`GHC1`~~ → `LZ4S` (shared), `format_version: u32` in struct | Unified magic for all index files; versioning via struct field ([debate-10](debate-10-remaining-design-areas.md)) |
| 16 | Commit ordering | Not required to be sorted in Vec | Sorted at query time, trivial incremental append |
| 17 | Full default branch history | Cache **entire** history, no depth/time/directory limits in MVP | 7.5 MB = 1.8% RAM; 59 sec is one-time cold start (disk cache solves it). Limited approaches give misleading answers ([debate-09](debate-09-cache-scope-strategy.md)) |
| 18 | Git cache in search-index | Single process, single MCP server | +1.8% RAM, +1 thread. Separate server = doubles configuration and processes without technical justification ([debate-09](debate-09-cache-scope-strategy.md)) |
| 19 | Path prefix matching | `== path \|\| starts_with(path + "/")` | Correct for files and directories, excludes false positives like `src2/` ([debate-10](debate-10-remaining-design-areas.md)) |
| 20 | Path normalization | `""` → match all, `"."` → `""`, strip trailing `/`, `\` → `/` | Unified function for cache lookup and CLI ([debate-10](debate-10-remaining-design-areas.md)) |
| 21 | Rename tracking hint | Hint in cache response: "Rename tracking not available from cache" | `follow: true` not applicable to cache (CLI-only in Phase 1). MVP compromise ([debate-10](debate-10-remaining-design-areas.md)) |
| 22 | Disk format | Reuse `save_compressed()`/`load_compressed()` (bincode v1 + lz4_flex) | Zero new dependencies, proven for 200 MB indexes ([debate-10](debate-10-remaining-design-areas.md)) |
| 23 | `format_version: u32` | Field in struct instead of custom magic bytes | Deserialization fail → log warning + full rebuild ([debate-10](debate-10-remaining-design-areas.md)) |
| 24 | Atomic write | `format!("{}.tmp", path.display())`, **NOT** `with_extension("tmp")` | `.with_extension()` replaces extensions instead of appending ([debate-10](debate-10-remaining-design-areas.md)) |
| 25 | Query order | filter by date → sort by timestamp → truncate to `maxResults` | Correctness first; sort 5K = ~20 μs ([debate-10](debate-10-remaining-design-areas.md)) |
| 26 | HashMap for `file_commits` | HashMap, not BTreeMap | 1-3 ms directory scan for 65K keys — sufficient ([debate-10](debate-10-remaining-design-areas.md)) |
| 27 | serve.rs refactor | **Do NOT refactor** when adding git cache | Copy-paste block is simpler and lower-risk. YAGNI ([debate-10](debate-10-remaining-design-areas.md)) |
| 28 | Nice priority | **Do not add** | IO-bound single thread, no precedent in codebase ([debate-10](debate-10-remaining-design-areas.md)) |
| 29 | Race conditions | **Eliminated** via build-then-swap + RwLock + bounds checks | Poison handling: `unwrap_or_else` ([debate-10](debate-10-remaining-design-areas.md)) |
| 30 | Git cache NOT tied to xray_reindex | Orthogonal data (files vs `.git/`), auto-invalidation via watcher + HEAD check | Different build times, different triggers. Tying them = 59 sec wait on every reindex ([debate-12](debate-12-parallel-git-log.md)) |
| 31 | xray_reindex_git not needed in MVP | **YAGNI**. Emergency case = delete `.git-history` file + restart | No real use case for manual rebuild with auto-invalidation available ([debate-12](debate-12-parallel-git-log.md)) |
| 32 | Multi-branch cache not needed | Feature-branch queries = CLI fallback, confirmed by customer | Cache only default branch; checkout another branch doesn't invalidate cache ([debate-12](debate-12-parallel-git-log.md)) |
| 33 | commit-graph hint at startup | Log hint when `.git/objects/info/commit-graph` is missing, **NOT auto-created** | search tool is read-only with respect to `.git/`. 2-5× speedup ([debate-12](debate-12-parallel-git-log.md)) |
| 34 | Git cache status in xray_info | Show in `xray_info`, **NOT** in `xray_reindex` response | `xray_reindex` relates only to content index; git cache is an orthogonal system ([debate-12](debate-12-parallel-git-log.md)) |
| 35 | Maximum isolation from existing modules | Git cache = standalone module [`src/git/cache.rs`](../src/git/cache.rs), zero imports from `index.rs`/`definitions/`/`mcp/`. Touch points: 4 files, ~95 lines of changes. **0 changes** to content index, definition index, other handlers, existing tests | Customer requirement. Details in §10 |

---

## 10. Module Isolation

> **Customer requirement:** maximum isolation of the git cache from existing modules. New code must not break any existing functionality.

### 10.1 File Structure

| File | Purpose | Status |
|------|---------|--------|
| [`src/git/cache.rs`](../src/git/cache.rs) | `GitHistoryCache` struct, builder, streaming parser, serialization, query API | **NEW** |
| [`src/git/cache_tests.rs`](../src/git/cache_tests.rs) | Cache unit tests | **NEW** |
| [`src/git/mod.rs`](../src/git/mod.rs) | Phase 1 CLI functions (`file_history`, `top_authors`, `repo_activity`) | **NO CHANGES** |
| [`src/mcp/handlers/git.rs`](../src/mcp/handlers/git.rs) | MCP handlers for git tools | Minimal changes (~30 lines per handler) |
| [`src/mcp/handlers/mod.rs`](../src/mcp/handlers/mod.rs) | `HandlerContext`, tool dispatcher | +3 lines (`git_cache` field) |
| [`src/mcp/server.rs`](../src/mcp/server.rs) | `HandlerContext` creation, dependency injection | +2 lines |
| [`src/cli/serve.rs`](../src/cli/serve.rs) | Server startup, background threads | +~20 lines (spawn thread, `AtomicBool`) |
| [`src/lib.rs`](../src/lib.rs) | Module declarations | **NO CHANGES** (`git` module already declared) |

### 10.2 Dependency Direction

```
┌─────────────────────────────────────────────────────────────────┐
│                    src/git/cache.rs                              │
│                                                                 │
│  - Depends ONLY on std + serde + bincode + lz4_flex             │
│  - Does NOT import anything from src/index.rs                   │
│  - Does NOT import anything from src/definitions/               │
│  - Does NOT import anything from src/mcp/                       │
│  - Self-contained module with public API                        │
│                                                                 │
│  pub fn build_from_git_log(repo: &str) -> GitHistoryCache       │
│  pub fn load_from_disk(path: &Path) -> Result<GitHistoryCache>  │
│  pub fn save_to_disk(&self, path: &Path) -> Result<()>          │
│  pub fn query_file_history(&self, file: &str, ...) -> Vec<...>  │
│  pub fn query_authors(&self, file: &str, ...) -> Vec<...>       │
│  pub fn query_activity(&self, prefix: &str, ...) -> Vec<...>    │
└────────────────────────────┬────────────────────────────────────┘
                             │ imports git::cache
                             ▼
┌───────────────────────────────────────────────────┐
│           src/mcp/handlers/git.rs                 │
│                                                   │
│  Single integration point:                        │
│  cache-or-fallback routing in each handler        │
│                                                   │
│  if let Some(cache) = ctx.git_cache.read() {      │
│      cache.query_file_history(...)                │
│  } else {                                         │
│      git::file_history(...)  // Phase 1 fallback  │
│  }                                                │
└───────────────────────────────────────────────────┘
                             │
                             ▼
┌───────────────────────────────────────────────────┐
│           src/cli/serve.rs                        │
│                                                   │
│  Single lifecycle manager:                        │
│  spawn background thread for cache build/load     │
│  Copy-paste pattern from content/def index build  │
└───────────────────────────────────────────────────┘
```

**Rule:** dependency arrows go **only downward** (from integration layer to cache module). The cache does not know about its consumers.

### 10.3 Touch Points with Existing Code (minimized)

| File | Change | Size |
|------|--------|------|
| [`src/cli/serve.rs`](../src/cli/serve.rs) | Add `Arc<RwLock<Option<GitHistoryCache>>>` + spawn build thread + `AtomicBool` readiness flag. Copy-paste pattern from content/def index build (lines [86-141](../src/cli/serve.rs:86) and [193-233](../src/cli/serve.rs:193)) | **~20 lines** |
| [`src/mcp/handlers/mod.rs`](../src/mcp/handlers/mod.rs) | Add `git_cache: Arc<RwLock<Option<GitHistoryCache>>>` field to [`HandlerContext`](../src/mcp/handlers/mod.rs:363) | **~3 lines** |
| [`src/mcp/server.rs`](../src/mcp/server.rs) | Pass `git_cache` when creating [`HandlerContext`](../src/mcp/server.rs:26) in [`run_server()`](../src/mcp/server.rs:15) | **~2 lines** |
| [`src/mcp/handlers/git.rs`](../src/mcp/handlers/git.rs) | Add cache-or-fallback routing in [`handle_git_history()`](../src/mcp/handlers/git.rs:162), [`handle_git_authors()`](../src/mcp/handlers/git.rs:227), [`handle_git_activity()`](../src/mcp/handlers/git.rs:282) | **~30 lines per handler** |

### 10.4 Files PROHIBITED from Modification

> **Zero blast radius** — the following files must not contain a single modified byte:

| Category | Files |
|----------|-------|
| Content index | [`src/index.rs`](../src/index.rs) |
| Definition index | [`src/definitions/mod.rs`](../src/definitions/mod.rs), [`src/definitions/incremental.rs`](../src/definitions/incremental.rs), [`src/definitions/storage.rs`](../src/definitions/storage.rs), [`src/definitions/types.rs`](../src/definitions/types.rs), [`src/definitions/parser_csharp.rs`](../src/definitions/parser_csharp.rs), [`src/definitions/parser_typescript.rs`](../src/definitions/parser_typescript.rs), [`src/definitions/parser_sql.rs`](../src/definitions/parser_sql.rs) |
| Other MCP handlers | [`src/mcp/handlers/grep.rs`](../src/mcp/handlers/grep.rs), [`src/mcp/handlers/fast.rs`](../src/mcp/handlers/fast.rs), [`src/mcp/handlers/find.rs`](../src/mcp/handlers/find.rs), [`src/mcp/handlers/definitions.rs`](../src/mcp/handlers/definitions.rs), [`src/mcp/handlers/callers.rs`](../src/mcp/handlers/callers.rs), [`src/mcp/handlers/utils.rs`](../src/mcp/handlers/utils.rs) |
| MCP protocol | [`src/mcp/protocol.rs`](../src/mcp/protocol.rs) |
| Watcher | [`src/mcp/watcher.rs`](../src/mcp/watcher.rs) |
| Git Phase 1 | [`src/git/mod.rs`](../src/git/mod.rs) — Phase 1 CLI functions remain unchanged, cache is additive |
| Core modules | [`src/error.rs`](../src/error.rs), [`src/main.rs`](../src/main.rs), [`src/lib.rs`](../src/lib.rs), [`src/tips.rs`](../src/tips.rs) |
| Existing tests | [`src/git/git_tests.rs`](../src/git/git_tests.rs), [`src/mcp/handlers/handlers_tests.rs`](../src/mcp/handlers/handlers_tests.rs), all `*_tests*.rs` files |

### 10.5 Blast Radius Diagram

```
UNTOUCHED — 0 changes:                 MINIMAL CHANGES — <5 lines:          NEW FILES:
──────────────────────                 ───────────────────────────           ──────────
src/index.rs                           src/cli/serve.rs (+20 lines)         src/git/cache.rs
src/definitions/*                      src/mcp/handlers/mod.rs (+3)         src/git/cache_tests.rs
src/mcp/handlers/grep.rs               src/mcp/server.rs (+2)
src/mcp/handlers/fast.rs
src/mcp/handlers/find.rs               MODERATE CHANGES — ~30 lines/handler:
src/mcp/handlers/definitions.rs        ──────────────────────────────────────
src/mcp/handlers/callers.rs            src/mcp/handlers/git.rs (+90 total)
src/mcp/handlers/utils.rs
src/mcp/protocol.rs
src/mcp/watcher.rs
src/error.rs
src/main.rs
src/lib.rs
src/tips.rs
src/git/mod.rs
src/git/git_tests.rs
src/mcp/handlers/handlers_tests.rs
src/definitions/definitions_tests.rs
```

### 10.6 Test Isolation

| Level | File | What it tests | Dependencies |
|-------|------|---------------|-------------|
| **Unit** | [`src/git/cache_tests.rs`](../src/git/cache_tests.rs) | Cache struct, parser, serialization, query API | In-memory only, no git repo |
| **Unit** | [`src/git/cache_tests.rs`](../src/git/cache_tests.rs) | Streaming parser | Mock git output (hardcoded strings) |
| **Unit** | [`src/git/cache_tests.rs`](../src/git/cache_tests.rs) | Path normalization | String transformations |
| **Unit** | [`src/git/cache_tests.rs`](../src/git/cache_tests.rs) | Bincode + LZ4 roundtrip | Serialization/deserialization |
| **E2E** | [`e2e-test.ps1`](../e2e-test.ps1) | Full pipeline with real git repo | Running MCP server |

**Test isolation principles:**
- Cache unit tests **do not require** a real git repository
- Cache unit tests **do not depend** on content/def index tests
- Existing tests **are not modified** — not a single `assert` changes
- E2E tests are added **additively** at the end of `e2e-test.ps1`