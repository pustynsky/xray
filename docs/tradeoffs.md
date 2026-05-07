# Design Trade-offs

Every architectural decision has alternatives. This document captures what was chosen, what was rejected, and why.

## 1. Index Storage: Bincode + LZ4

### Chosen: Bincode (binary serialization) with LZ4 compression

**Why:**

- Zero-config — serialize any Rust struct with `#[derive(Serialize, Deserialize)]`
- Fast — near-zero overhead deserialization, close to raw memory layout
- Single-file — each index is one `.file-list`/`.word-search`/`.code-structure`/`.git-history` file
- No runtime dependencies — no database process, no WAL, no compaction
- LZ4 compression reduces disk footprint by **~35-45%** (corpus-dependent; e.g. the 48,599-file C# benchmark corpus: ~350 MB uncompressed → 223.7 MB LZ4 — see [benchmarks.md](benchmarks.md#index-build-times)) with negligible decompression overhead (~100ms)
- Backward-compatible — auto-detects legacy uncompressed files on load

**Rejected alternatives:**

| Alternative                   | Why Not                                                                                                                        |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| **SQLite**                    | Adds 1MB+ dependency, slower for bulk reads (entire index loaded at once), row-level access unnecessary for our access pattern |
| **RocksDB**                   | C++ dependency, complex build, designed for incremental writes — overkill for batch-build-then-read pattern                    |
| **Cap'n Proto / FlatBuffers** | Zero-copy is appealing but requires schema files, more complex API, marginal gain when entire index fits in RAM                |
| **JSON**                      | 5-10x larger on disk, 10-50x slower to parse for large indexes                                                                 |
| **MessagePack**               | Similar to bincode but less Rust-native, no meaningful advantage                                                               |
| **zstd compression**          | Higher compression ratio but slower decompression (~3x slower than LZ4). Not worth the trade-off for indexes that are loaded once at startup |
| **No compression**            | ~1.6× larger on disk (e.g. ~350 MB uncompressed vs 223.7 MB LZ4 on the 48,599-file C# benchmark corpus). LZ4 decompression is fast enough to be negligible           |

**Known limitations:**

- Bincode format is not stable across major versions — index files are not portable between bincode 1.x and 2.x
- No incremental writes to disk — entire index must be serialized/deserialized atomically (in-memory incremental updates via watcher are supported)
- No memory-mapped I/O — full deserialization into heap on load

**When to reconsider:** If indexes exceed available RAM (>4GB), a memory-mapped approach (e.g., FST for the token map + mmap'd postings) would be necessary.

## 2. FileIndex: Vec Scan vs Inverted Lookup

### Chosen: `Vec<FileEntry>` with O(n) linear scan

**Why:**

- **90× faster than a live filesystem walk** — `xray_fast` scans the in-memory vec in ~35ms for 100K files. A live walk over the same files takes ~3s. The vec scan replaces disk I/O, not a faster algorithm.
- Simple — no secondary data structures to build, maintain, or invalidate
- Cache-friendly — sequential scan over a contiguous `Vec` has excellent CPU cache locality
- Flexible matching — supports substring, regex, case-insensitive, comma-separated multi-term OR — all patterns that are hard to pre-index efficiently
- Small index — `Vec<FileEntry>` is **~2.6 MB on disk (LZ4) for ~99K entries** (e.g. a Shared C# repo) and tens of MB in-memory due to per-entry `String` allocations; still significantly less than a content inverted index (~224 MB LZ4 for the same repo)

**Rejected alternatives:**

| Alternative | Why Not |
|---|---|
| **Inverted index on filename tokens** | Would enable O(1) exact-token lookup, but file name search is primarily substring-based (`UserService` matches `IUserServiceFactory.cs`). An inverted index would still need a trigram or suffix layer for substrings, adding complexity for marginal gain over 35ms. |
| **Trie on file paths** | Good for prefix matching but not for substring/contains matching. File search patterns like `UserService` are rarely anchored to the start of the filename. |
| **Trigram index on filenames** | Would reduce scan to O(k) candidates, but the trigram index alone adds ~56MB for content tokens. For filenames (which are much shorter), the benefit vs the ~35ms baseline is minimal. |

**Known limitations:**

- O(n) means performance degrades linearly with file count — 1M files would take ~350ms
- No pre-filtering — every entry is checked even if the pattern could be pre-narrowed

**When to reconsider:** If file counts exceed ~500K and the ~175ms+ scan time becomes noticeable in interactive use, adding a trigram index on filenames (similar to `ContentIndex.trigram`) would reduce search to O(k) candidates.

## 3. Inverted Index: HashMap vs FST/Trie

### Chosen: `HashMap<String, Vec<Posting>>`

**Why:**

- O(1) exact token lookup — the primary query pattern
- Simple to implement incremental updates (insert/remove postings)
- Rust's `HashMap` is high-performance (SwissTable implementation since 1.36)
- Easy to serialize with bincode

**Rejected alternatives:**

| Alternative         | Why Not                                                                                                                                                                                                                 |
| ------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **FST (fst crate)** | Excellent for prefix/range queries and memory efficiency, but immutable — cannot do incremental updates without full rebuild. Tantivy uses FST, but they have a segment-merge architecture we don't need at this scale. |
| **Trie**            | Better for prefix matching, but higher memory overhead per node, slower for exact lookups, complex to serialize                                                                                                         |
| **BTreeMap**        | Sorted iteration is unnecessary for our queries, 2-3x slower than HashMap for exact lookups                                                                                                                             |
| **Tantivy**         | Full-featured search engine — adds 10MB+ to binary, brings its own segment management, query parser, etc. Overkill for single-directory code search.                                                                    |

**Known limitations:**

- No prefix/fuzzy search without scanning all keys (regex mode does this, measured 44ms for 754K tokens)
- Memory usage is O(unique_tokens × avg_posting_size) — for the 48,599-file C# benchmark corpus this is 223.7 MB LZ4 on disk (~350 MB uncompressed)
- Hash collisions can degrade under adversarial inputs (not a concern for code search)

**When to reconsider:** If we add fuzzy/typo-tolerant search, an FST or Levenshtein automaton would be much more efficient than regex scanning all keys.

## 4. Ranking: TF-IDF vs BM25

### Chosen: Classic TF-IDF

```
score = (occurrences / file_token_count) × ln(total_files / files_with_term)
```

**Why:**

- Simple — one formula, no tunable parameters
- Effective for code search — code is more structured than natural language, simple TF-IDF works well
- Fast — single pass over postings, no normalization constants to precompute
- Predictable — developers can reason about why a result ranks higher

**Rejected alternatives:**

| Alternative         | Why Not                                                                                                                                                                                                                                                                                      |
| ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **BM25**            | Adds two tunable parameters (k1, b) that require corpus-specific tuning. Marginal improvement for code search where documents (files) have similar structure. BM25's document length normalization helps with variable-length prose documents but code files are already relatively uniform. |
| **PageRank-style**  | Would require call graph analysis. Expensive to compute, unclear benefit for code search.                                                                                                                                                                                                    |
| **Embedding-based** | Requires ML model, GPU/large CPU, ~100x slower per query. Out of scope for a CLI tool.                                                                                                                                                                                                       |

**Known limitations:**

- No field boosting — a match in class name vs. method body has equal weight
- No position proximity — `HttpClient` on line 1 and line 500 contribute equally
- TF normalization by file size means a 10-line file mentioning `HttpClient` once will rank above a 1000-line file mentioning it 5 times

**When to reconsider:** If user feedback shows ranking quality issues, BM25 with default parameters (k1=1.2, b=0.75) would be a minimal-effort upgrade.

## 5. Concurrency: RwLock vs Lock-Free

### Chosen: `Arc<RwLock<ContentIndex>>`

**Why:**

- Simple correctness — Rust's type system enforces exclusive writes
- Appropriate for the access pattern: many reads (search queries), rare writes (watcher updates)
- `RwLock` allows concurrent reads with no contention
- Single writer (watcher thread) means no write contention

**Rejected alternatives:**

| Alternative                                | Why Not                                                                                                                                                             |
| ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Lock-free (crossbeam SkipMap, dashmap)** | Adds dependency, more complex code, marginal benefit — we have exactly 1 writer and writes are infrequent (debounced to every 500ms). Lock contention is near-zero. |
| **Copy-on-write (Arc swap)**               | Would require cloning the entire index on update (~400MB). Only viable with an immutable/persistent data structure.                                                 |
| **Actor model (channels)**                 | Adds complexity. The MCP server is single-threaded on stdin, so actor model doesn't provide concurrency benefit.                                                    |
| **No locking (single-threaded)**           | Not possible — watcher, git cache build, and background index build threads run on separate OS threads by design.                                                   |

**Known limitations:**

- Writer starvation is theoretically possible if search queries are continuous, but MCP queries are human-driven (~1/sec max) so this doesn't happen in practice
- `RwLock` on Windows is not fair — but our usage pattern (rare writes) makes this irrelevant
- Poisoned lock handling: watcher thread exits gracefully on poisoned lock instead of looping forever (fixed 2026-02-28)

## 6. Tree-sitter vs Regex for Code Parsing

### Chosen: tree-sitter AST parsing (C#, TypeScript/TSX, Rust, XML) + regex parsing (SQL)

**Why:**

- Full syntactic understanding — correctly handles nested classes, partial classes, multi-line signatures
- Modifiers, attributes, base types extracted as structured data
- Line range tracking enables `containsLine` queries (which method is on line N?)
- Call-graph extraction — AST walk of method bodies extracts `CallSite` data (method name, receiver type, line) for `xray_callers` "down" direction. Resolves field types (e.g., `_userService` → `IUserService`) for DI-aware call trees. This would be impossible with regex.
- Code complexity metrics — cyclomatic/cognitive complexity, nesting depth, params, returns, calls, lambdas computed during AST walk via shared `walk_code_stats()` with per-language `CodeStatsConfig`
- Language grammars maintained by the community, handle edge cases we'd never cover with regex
- SQL uses a regex-based parser (no tree-sitter grammar needed) — sufficient for DDL/DML extraction where the syntax is regular enough

**Supported languages:**

| Language        | Parser      | Feature Flag      | Default Build | Indexing mode             |
| --------------- | ----------- | ----------------- | ------------- | ------------------------- |
| C#              | tree-sitter | `lang-csharp`     | ✅            | persisted definition index |
| TypeScript/TSX  | tree-sitter | `lang-typescript` | ✅            | persisted definition index |
| Rust            | tree-sitter | `lang-rust`       | ✅            | persisted definition index |
| XML             | tree-sitter | `lang-xml`        | ✅            | on-demand (per MCP query)   |
| SQL             | regex       | _(always built)_  | ✅            | persisted definition index |

XML parsing is on-demand only — no XML definitions are persisted to the
code-structure index. The SQL parser is regex-based and always compiled
(no Cargo feature gate).

**Rejected alternatives:**

| Alternative                        | Why Not                                                                                                                                                |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Regex patterns**                 | Cannot handle nesting, multi-line constructs, or distinguish between definition and usage. Would miss partial classes, expression-bodied members, etc. Works well for SQL DDL where syntax is regular — hence the SQL parser uses regex. |
| **LSP (Language Server Protocol)** | Requires running the actual language server (Roslyn for C#). 10-100x slower, requires .NET SDK installed, heavy process.                               |
| **ctags/universal-ctags**          | External tool dependency. Less structured output. Cannot extract attributes, base types, or modifiers.                                                 |
| **syn (Rust AST crate)**          | Only works for Rust. tree-sitter provides a unified API across all 5 supported languages with consistent definition/call-site extraction. Using `syn` for Rust alone would mean a different code path, different data structures, and different testing approach. |

**Known limitations:**

- tree-sitter grammars are large (C# grammar adds ~2MB to binary)
- Adding a new language requires a new tree-sitter grammar crate (or regex parser) + parser implementation (~200 LOC per language)
- Each language parser is an optional Cargo feature — custom builds can exclude unused parsers
- SQL parser (regex-based) cannot extract call sites from dynamic SQL strings or complex procedural logic

## 7. Tokenization: Simple Split vs Language-Aware

### Chosen: Character-class split + lowercase

```rust
line.split(|c: char| !c.is_alphanumeric() && c != '_')
    .filter(|s| s.len() >= min_len)
    .map(|s| s.to_lowercase())
```

**Why:**

- Language-agnostic — works for C#, SQL, Rust, Python, JavaScript, prose
- Fast — no regex, no Unicode normalization, single pass
- Predictable — developers know exactly what tokens are indexed
- Preserves underscores — `_client` stays as one token
- Case-insensitive via lowercase — `HttpClient` and `httpclient` are the same token

**Rejected alternatives:**

| Alternative                | Why Not                                                                                                                                                                |
| -------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **camelCase splitting**    | `HttpClient` → `http`, `client` would enable partial word matching but causes false positives. `HttpClient` would match searches for just `client`.                    |
| **N-gram**                 | Enables fuzzy matching but massively increases index size (3-gram of `HttpClient` = 8 grams). Not worth the trade-off for code search where exact tokens are the norm. |
| **Stemming/lemmatization** | Designed for natural language. Code identifiers don't follow natural language morphology. `async` should not match `asynchronous`.                                     |
| **Unicode-aware (ICU)**    | Adds heavy dependency. Code identifiers are ASCII in >99% of codebases.                                                                                                |

**Known limitations:**

- `HttpClient` becomes one token `httpclient` — cannot search for files using specifically `Http` but not `HttpClient`
- Numbers are included: `int32` is one token. This is usually desirable for code.
- Very short tokens (1 char) are excluded by default (min_len=2)
- Terms containing punctuation (e.g., `#[cfg(test)]`, `System.IO`) are auto-switched to phrase search since no indexed token contains non-alphanumeric characters

## 8. MCP Transport: Stdio vs HTTP

### Chosen: Stdio (stdin/stdout)

**Why:**

- Zero configuration — no port conflicts, no firewall rules, no TLS
- Lowest latency — direct pipe, no TCP overhead
- VS Code native — built-in process spawning, no external server to manage
- Security — no network exposure, process isolation by OS

**Rejected alternatives:**

| Alternative   | Why Not                                                                                                                                     |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| **HTTP/SSE**  | Network overhead, requires port management, firewall issues in corporate environments. VS Code MCP spec supports both but stdio is simpler. |
| **WebSocket** | Same issues as HTTP plus connection management complexity.                                                                                  |
| **gRPC**      | Adds protobuf dependency, code generation step. Overkill for single-client, low-QPS scenario.                                               |

**Known limitations:**

- Single client — only one process can read stdin at a time
- No remote access — must run on same machine as the AI agent
- Debugging requires stderr logging — cannot use stdout for diagnostics

## 9. Positioning: xray vs Existing Code Search Tools

### Chosen: Custom single-binary engine (inverted index + trigram + tree-sitter AST + call graph + git cache + code stats)

The question is not just "which data structure?" but "why not use an existing tool?"

**Rejected alternatives:**

| Tool | What It Does Well | Why Not Sufficient |
|---|---|---|
| **Zoekt** (Google Code Search successor) | Trigram-based code search, used at Google/Sourcegraph scale. Fast substring search via trigrams. | No AST awareness — cannot search by class/method structure, no call graph, no `containsLine`, no `includeBody`. It's a text search engine, not a code intelligence engine. Requires Go runtime. |
| **Sourcegraph** | Full code intelligence platform with navigation, cross-repo search, precise code intel via SCIP/LSIF. | Heavy infrastructure: requires Docker, PostgreSQL, multiple services. Designed as a web application, not a local CLI/MCP tool. Code intel requires language-specific SCIP indexers (separate build step). Latency: network round-trips to HTTP API vs stdio pipe. |
| **GitHub Code Search** | Excellent web-based search across repositories with language-aware ranking. | Cloud-only, requires network, cannot run locally. No call graph. No MCP integration. Cannot index private repos that aren't on GitHub. |
| **LSP (Language Server Protocol)** | Full semantic understanding: type inference, cross-file references, rename refactoring. The "gold standard" for code intelligence. | Requires running the actual language server (Roslyn for C#, tsserver for TS). Index build: minutes to hours for large repos. Memory: GBs for large solutions. Only one language per server. Not designed for broad search — designed for single-file navigation. Cannot answer "find all classes with attribute X across 49K files". |
| **Roslyn Analyzers / CodeQL** | Deep semantic analysis: data flow, taint tracking, vulnerability detection. | Build times: hours for large codebases. Requires full compilation. CodeQL databases are 10-50GB. Not interactive — batch analysis tools. Completely different use case (static analysis vs code navigation). |
| **ripgrep (rg)** | Fastest regex-based file search. Gold standard for live filesystem grep. | No index — every query is a full filesystem scan (32s for 49K files). No AST awareness. No call graph. No ranking. No trigram substring search. Linear cost per query. |
| **ctags / universal-ctags** | Fast tag-based navigation. Works with 50+ languages. | Tags only — no call graph, no base types, no attributes, no modifiers. External tool dependency. Limited structured data. No search API — produces a file that editors consume. |

### What xray uniquely provides

The combination of these capabilities in a single tool is what makes it distinct:

| Capability | Zoekt | Sourcegraph | LSP | ripgrep | xray |
|---|---|---|---|---|---|
| Text search <1ms | ✅ | ✅ | ❌ | ❌ (32s) | ✅ |
| Substring search | ✅ (trigram) | ✅ | ❌ | ✅ (regex) | ✅ (trigram) |
| AST-aware definitions | ❌ | ✅ (SCIP) | ✅ | ❌ | ✅ (tree-sitter, 4 languages + regex SQL) |
| Call graph / callers | ❌ | ✅ (precise) | ✅ | ❌ | ✅ (AST-based) |
| DI-aware interface resolution | ❌ | ❌ | ✅ | ❌ | ✅ |
| `containsLine` (line → method) | ❌ | ❌ | ✅ | ❌ | ✅ |
| `includeBody` (inline source) | ❌ | ❌ | ❌ | ❌ | ✅ |
| Code complexity metrics | ❌ | ❌ | ❌ | ❌ | ✅ (7 metrics) |
| Impact analysis (test coverage) | ❌ | ❌ | ❌ | ❌ | ✅ |
| Git history/blame (sub-ms cache) | ❌ | ✅ | ❌ | ❌ | ✅ |
| Angular component trees | ❌ | ❌ | ❌ | ❌ | ✅ |
| Doc-comment extraction | ❌ | ❌ | ✅ | ❌ | ✅ |
| Single binary, zero deps | ❌ (Go) | ❌ (Docker) | ❌ (.NET/Node) | ✅ | ✅ |
| MCP native (AI agent) | ❌ | ❌ | ❌ | ❌ | ✅ |
| Index build time | ~minutes | ~minutes | ~minutes | n/a | **16-32s** |
| Query latency | ~1-10ms | ~50-200ms | ~10-100ms | ~32s | **<1ms** |
| Local, offline, no network | ✅ | ❌ | ✅ | ✅ | ✅ |

**Key insight:** No existing tool combines fast text search + AST structural queries + call graph + code stats + git history + MCP integration in a single zero-dependency binary. Each tool excels at a subset:

- ripgrep = fastest text search but no structure
- LSP = most accurate code intelligence but heavy and slow
- Zoekt = fast indexed search but no AST
- Sourcegraph = most complete but requires infrastructure

xray occupies the **"fast enough for interactive AI agents, structured enough for code navigation"** niche that none of the above serve.

**When to reconsider:** If precise type inference becomes critical (not just call-site heuristics), integrating SCIP/LSIF indexes alongside the current AST index would be the path — keeping the fast text search layer while adding semantic precision where needed.

## 10. Interface Resolution Depth in Caller Trees

### Chosen: `resolveInterfaces` expands sibling implementations only at root level (depth 0)

**Why:**

- At depth 0, the user's target method may be defined on an interface (e.g., `IUserService.GetUser`). Expanding to all implementations (`UserService.GetUser`, `MockUserService.GetUser`) is essential — without it, the caller tree would be empty since no code calls the interface method directly.
- At deeper levels (depth > 0), callers are found via `verify_call_site_target()` which already handles interface-based calls through multiple heuristics:
  - **Direct naming match** — `IFoo`↔`Foo` prefix matching
  - **Inheritance via `base_types`** — if `UserService` lists `IUserService` in its base types, calls to `IUserService.GetUser` match `UserService.GetUser`
  - **Fuzzy DI matching** — `is_implementation_of()` detects dependency injection patterns (field types, constructor parameters) to resolve `_userService.GetUser()` → `IUserService.GetUser`
- Expanding sibling implementations at every depth level risks **combinatorial explosion**: an interface with 5 implementations would multiply the tree width by 5× at each level, consuming the `max_total_nodes` budget on breadth instead of depth

**The gap:**

The only scenario NOT covered: a sibling implementation (e.g., `AlternativeUserService.GetUser`) with no naming relationship to its interface, called by concrete type at depth > 0. This is a narrow edge case — in practice, if a class implements `IUserService`, `verify_call_site_target()` will match it via `base_types` or naming heuristics.

**Rejected alternative:**

| Alternative | Why Not |
|---|---|
| **`resolveInterfaces` at all depths** | For an interface with N implementations, the tree width multiplies by N at each level. With depth=5 and N=5, this creates up to 5^5 = 3125 nodes from a single root — exhausting the default `max_total_nodes=200` budget immediately. Result quality degrades because the budget is consumed by sibling implementations rather than the actual call chain the user is investigating. |

**When to reconsider:** If users report missing callers at depth > 0 due to unconventional DI patterns, an optional `interfaceDepth` parameter could allow controlled expansion at deeper levels (e.g., `interfaceDepth: 2` would expand interfaces at depths 0-2).

## 11. Git History: Compact In-Memory Cache vs CLI-Only

### Chosen: Custom compact in-memory cache with disk persistence

**Why:**

- **Sub-millisecond queries** vs 2-6 seconds per `git log` CLI call
- Compact representation — ~7.6 MB for 50K commits × 65K files (`CommitMeta` at 40 bytes/commit)
- Streaming parser — parses `git log --name-only` output line-by-line, no 163 MB intermediate buffer
- Background build — does not block server startup; tools fall back to CLI until cache is ready
- Disk persistence — LZ4-compressed bincode, loads in ~100ms vs ~59 sec full rebuild
- HEAD validation — detects stale caches (force push, rebase, re-clone) and triggers rebuild

**Rejected alternatives:**

| Alternative | Why Not |
|---|---|
| **CLI-only (no cache)** | 2-6 seconds per query. For LLM agents making multiple git queries per task (history → blame → authors), cumulative latency of 10-20 seconds is unacceptable for interactive use. |
| **SQLite for git data** | Same reasons as §1 — adds dependency, row-level access unnecessary. The cache is built once and queried by key lookup, not ad-hoc SQL. |
| **libgit2 / git2 crate** | C dependency (libgit2), complex build chain, HTTPS cert issues in corporate environments. The `git` CLI is universally available and handles all auth/proxy configurations. |
| **In-memory only (no disk)** | 59-second rebuild on every server restart. Disk persistence reduces cold start to ~100ms. |

**Known limitations:**

- No patch/diff data in cache — `xray_git_diff` always uses CLI (patches are too large to cache efficiently)
- Author pool capped at 65,535 unique authors (u16 index) — sufficient for any single repository
- Cache is per-repository — multi-repo setups have separate caches

**When to reconsider:** If repos exceed ~500K commits, the linear scan in `query_file_history()` may need optimization (e.g., per-file posting lists instead of per-commit file lists).

## 12. Incremental Updates: Tombstones + Auto-Compaction

### Chosen: Leave stale `DefinitionEntry` as tombstones in Vec, auto-compact when waste exceeds 67%

**Why:**

- **O(1) removal** — removing a file's definitions only requires updating secondary indexes (HashMap lookups), not shifting Vec elements
- No reindexing needed per file change — secondary indexes (`name_index`, `kind_index`, etc.) are updated in-place
- Compaction is rare and fast — ~100ms for 846K definitions, <1MB additional memory
- Simple correctness — `file_index` is the source of truth for active definitions; tombstones are invisible to queries

**Rejected alternatives:**

| Alternative | Why Not |
|---|---|
| **Immediate Vec removal** | Requires O(n) shift of all subsequent elements + remapping ALL secondary indexes (9 HashMaps of u32 indices). For a 846K-element Vec, this would be ~1ms per removal — acceptable for single files but problematic for `git pull` scenarios with 300+ file changes. |
| **Slot reuse with free list** | Would avoid Vec growth by reusing tombstone slots for new definitions. Adds complexity (free list management, fragmentation) for marginal benefit — compaction already handles the memory concern. |
| **Immutable rebuild on every change** | Simplest correctness model but means rebuilding the entire index (~16s) for every file save. Incompatible with interactive watch mode. |

**Known limitations:**

- Between compactions, `definitions.len()` can be up to 3× the active definition count
- All 9 secondary indexes must be remapped during compaction (atomic operation, not interruptible)
- Compaction holds a write lock for ~100ms — blocks concurrent reads during this time

**When to reconsider:** If watch-mode sessions exceed 24 hours with continuous high-churn changes, the auto-compaction threshold (67% waste) may need tuning. In practice, VS Code restarts rebuild the index cleanly.

## 13. Code Complexity: AST Walker vs External Tools

### Chosen: Compute during tree-sitter AST walk via shared `walk_code_stats()` + per-language `CodeStatsConfig`

**Why:**

- **Zero additional dependencies** — complexity metrics are computed as part of the existing AST traversal
- Minimal overhead — ~2-5% additional CPU during index build (already walking the AST for definitions/call-sites)
- 7 metrics per method: cyclomatic complexity, cognitive complexity (SonarSource algorithm), max nesting depth, parameter count, return/throw count, call count (fan-out), lambda count
- Language-agnostic walker — adding a new language requires only a static `CodeStatsConfig` struct with AST node names, no code duplication
- Queryable — `sortBy`, `minComplexity`, `minCognitive`, `minNesting`, `minParams` enable instant "find worst methods" queries

**Rejected alternatives:**

| Alternative | Why Not |
|---|---|
| **SonarQube / SonarCloud** | Heavy infrastructure (Java process, database, web UI). Cannot be embedded in a CLI tool. Designed for CI/CD pipelines, not interactive queries. |
| **CodeQL** | Requires full compilation + database build (10-50 GB, hours). Overkill for code metrics — designed for vulnerability detection and data flow analysis. |
| **Custom regex-based counter** | Cannot handle nesting depth, control flow, or language-specific constructs (C# `?.`, Rust `?` operator, TypeScript `??`). Would produce inaccurate metrics. |
| **External linter integration** | Each language has its own linter (ESLint, Clippy, Roslyn analyzers). Would require 4 external tool dependencies and 4 different output formats. |

**Known limitations:**

- No data flow complexity — metrics are syntactic (AST-based), not semantic
- Cognitive complexity algorithm differs slightly from SonarSource's reference implementation for some edge cases (e.g., C# and TypeScript handle else-if nesting differently due to tree-sitter grammar differences)
- Metrics are per-method — no file-level or class-level aggregation

**When to reconsider:** If users need coupling metrics (afferent/efferent coupling, instability), these require cross-file analysis beyond what the current per-method walker provides.

## 14. Response Truncation: Progressive Budget vs Fixed Limits

### Chosen: Progressive byte-budget truncation with tool-specific limits

**Why:**

- **LLM context window efficiency** — large responses waste tokens. A 1MB response consumes ~250K tokens, leaving little room for reasoning.
- Tool-specific budgets: 16KB default, 64KB for `includeBody=true`, 32KB for `xray_help`
- Progressive — truncates results one-by-one from the end, preserving the most relevant (highest-scored) results
- `maxResults`, `maxBodyLines`, `maxTotalBodyLines` give users fine-grained control before truncation kicks in

**Rejected alternatives:**

| Alternative | Why Not |
|---|---|
| **No truncation** | Responses can exceed 1MB for broad queries (e.g., `xray_definitions` with no filters on 846K definitions). Wastes LLM context window and can cause timeouts. |
| **Fixed line limits only** | Line count doesn't correlate well with token count. A line with a long base64 string uses far more tokens than a line with `}`. Byte budget is more predictable. |
| **Client-side truncation** | MCP clients (VS Code, Roo) may truncate arbitrarily, potentially cutting mid-JSON. Server-side truncation ensures valid JSON response structure. |
| **Pagination (offset/limit)** | Adds protocol complexity (stateful cursors). LLM agents rarely paginate — they refine queries instead. `maxResults` provides equivalent functionality without state. |

**Known limitations:**

- Truncation is opaque to the LLM — it sees fewer results but may not realize more exist (mitigated by `totalResults` field in summary)
- Budget thresholds are hardcoded — not configurable via MCP protocol

**When to reconsider:** If LLM context windows grow significantly (>500K tokens), the default budgets should be increased proportionally. If MCP protocol adds pagination support, server-side truncation could be replaced with client-driven pagination.

## Summary Matrix

| Decision           | Chosen                        | Key Reason                         | Reconsider When                      |
| ------------------ | ----------------------------- | ---------------------------------- | ------------------------------------ |
| Storage            | Bincode + LZ4                 | Fast, simple, single-file, −42% disk | Index > 4GB RAM                    |
| File index         | Vec + O(n) scan               | 90× faster than FS walk, simple    | File count > 500K                    |
| Content index      | HashMap                       | O(1) lookup, easy updates          | Need fuzzy search                    |
| Ranking            | TF-IDF                        | Simple, effective for code         | Ranking quality complaints           |
| Concurrency        | RwLock                        | Correct, minimal contention        | High-throughput multi-client         |
| Code parsing       | tree-sitter + regex (SQL)     | Full AST, 4 languages              | n/a (correct choice)                 |
| Tokenization       | char-split + lowercase        | Fast, language-agnostic            | Need camelCase/fuzzy                 |
| Transport          | Stdio                         | Zero-config, lowest latency        | Need remote access                   |
| Positioning        | Custom single-binary           | Only tool combining all capabilities + MCP | Need precise type inference (add SCIP/LSIF) |
| Interface resolution | Root-level only              | Prevents combinatorial explosion   | Users report missing deep callers    |
| Git history        | Compact in-memory cache       | Sub-ms queries vs 2-6s CLI         | Repos > 500K commits                 |
| Incremental updates | Tombstones + auto-compaction | O(1) removal, rare compaction      | 24h+ watch sessions with high churn  |
| Code complexity    | AST walker + CodeStatsConfig  | Zero deps, ~2-5% overhead          | Need coupling/data-flow metrics      |
| Response truncation | Progressive byte budget      | LLM context window efficiency      | Context windows > 500K tokens        |
