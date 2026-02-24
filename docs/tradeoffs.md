# Design Trade-offs

Every architectural decision has alternatives. This document captures what was chosen, what was rejected, and why.

## 1. Index Storage: Bincode vs Alternatives

### Chosen: Bincode (binary serialization)

**Why:**

- Zero-config — serialize any Rust struct with `#[derive(Serialize, Deserialize)]`
- Fast — near-zero overhead deserialization, close to raw memory layout
- Single-file — each index is one `.file-list`/`.word-search`/`.code-structure` file
- No runtime dependencies — no database process, no WAL, no compaction

**Rejected alternatives:**

| Alternative                   | Why Not                                                                                                                        |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| **SQLite**                    | Adds 1MB+ dependency, slower for bulk reads (entire index loaded at once), row-level access unnecessary for our access pattern |
| **RocksDB**                   | C++ dependency, complex build, designed for incremental writes — overkill for batch-build-then-read pattern                    |
| **Cap'n Proto / FlatBuffers** | Zero-copy is appealing but requires schema files, more complex API, marginal gain when entire index fits in RAM                |
| **JSON**                      | 5-10x larger on disk, 10-50x slower to parse for large indexes                                                                 |
| **MessagePack**               | Similar to bincode but less Rust-native, no meaningful advantage                                                               |

**Known limitations:**

- Bincode format is not stable across major versions — index files are not portable between bincode 1.x and 2.x
- No incremental writes — entire index must be serialized/deserialized atomically
- No memory-mapped I/O — full deserialization into heap on load

**When to reconsider:** If indexes exceed available RAM (>4GB), a memory-mapped approach (e.g., FST for the token map + mmap'd postings) would be necessary.

## 2. FileIndex: Vec Scan vs Inverted Lookup

### Chosen: `Vec<FileEntry>` with O(n) linear scan

**Why:**

- **90× faster than the alternative** — `search_fast` scans the in-memory vec in ~35ms for 100K files. The alternative (`search_find`) does a live filesystem walk taking ~3s for the same files. The vec scan replaces disk I/O, not a faster algorithm.
- Simple — no secondary data structures to build, maintain, or invalidate
- Cache-friendly — sequential scan over a contiguous `Vec` has excellent CPU cache locality
- Flexible matching — supports substring, regex, case-insensitive, comma-separated multi-term OR — all patterns that are hard to pre-index efficiently
- Small index — `Vec<FileEntry>` is ~48MB for 334K entries, significantly less than a content inverted index (~242MB)

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
- Memory usage is O(unique_tokens × avg_posting_size) — for 49K files this is 242MB on disk
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
| **No locking (single-threaded)**           | Not possible — watcher and background build threads run on separate OS threads by design.                                                                           |

**Known limitations:**

- Writer starvation is theoretically possible if search queries are continuous, but MCP queries are human-driven (~1/sec max) so this doesn't happen in practice
- `RwLock` on Windows is not fair — but our usage pattern (rare writes) makes this irrelevant

## 6. Tree-sitter vs Regex for Code Parsing

### Chosen: tree-sitter AST parsing

**Why:**

- Full syntactic understanding — correctly handles nested classes, partial classes, multi-line signatures
- Modifiers, attributes, base types extracted as structured data
- Line range tracking enables `containsLine` queries (which method is on line N?)
- Call-graph extraction — AST walk of method bodies extracts `CallSite` data (method name, receiver type, line) for `search_callers` "down" direction. Resolves field types (e.g., `_userService` → `IUserService`) for DI-aware call trees. This would be impossible with regex.
- Language grammar maintained by the community, handles edge cases we'd never cover with regex

**Rejected alternatives:**

| Alternative                        | Why Not                                                                                                                                                |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Regex patterns**                 | Cannot handle nesting, multi-line constructs, or distinguish between definition and usage. Would miss partial classes, expression-bodied members, etc. |
| **LSP (Language Server Protocol)** | Requires running the actual language server (Roslyn for C#). 10-100x slower, requires .NET SDK installed, heavy process.                               |
| **ctags/universal-ctags**          | External tool dependency. Less structured output. Cannot extract attributes, base types, or modifiers.                                                 |
| **syn (Rust AST)**                 | Only works for Rust. We need C# and SQL.                                                                                                               |

**Known limitations:**

- tree-sitter grammars are large (C# grammar adds ~2MB to binary)
- Adding a new language requires a new tree-sitter grammar crate + parser implementation (~200 LOC per language)
- SQL grammar (tree-sitter-sequel-tsql) may not cover all T-SQL dialects perfectly

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

## 9. Positioning: search-index vs Existing Code Search Tools

### Chosen: Custom single-binary engine (inverted index + trigram + tree-sitter AST + call graph)

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

### What search-index uniquely provides

The combination of these capabilities in a single tool is what makes it distinct:

| Capability | Zoekt | Sourcegraph | LSP | ripgrep | search-index |
|---|---|---|---|---|---|
| Text search <1ms | ✅ | ✅ | ❌ | ❌ (32s) | ✅ |
| Substring search | ✅ (trigram) | ✅ | ❌ | ✅ (regex) | ✅ (trigram) |
| AST-aware definitions | ❌ | ✅ (SCIP) | ✅ | ❌ | ✅ (tree-sitter) |
| Call graph / callers | ❌ | ✅ (precise) | ✅ | ❌ | ✅ (AST-based) |
| DI-aware interface resolution | ❌ | ❌ | ✅ | ❌ | ✅ |
| `containsLine` (line → method) | ❌ | ❌ | ✅ | ❌ | ✅ |
| `includeBody` (inline source) | ❌ | ❌ | ❌ | ❌ | ✅ |
| Single binary, zero deps | ❌ (Go) | ❌ (Docker) | ❌ (.NET/Node) | ✅ | ✅ |
| MCP native (AI agent) | ❌ | ❌ | ❌ | ❌ | ✅ |
| Index build time | ~minutes | ~minutes | ~minutes | n/a | **16-32s** |
| Query latency | ~1-10ms | ~50-200ms | ~10-100ms | ~32s | **<1ms** |
| Local, offline, no network | ✅ | ❌ | ✅ | ✅ | ✅ |

**Key insight:** No existing tool combines fast text search + AST structural queries + call graph + MCP integration in a single zero-dependency binary. Each tool excels at a subset:

- ripgrep = fastest text search but no structure
- LSP = most accurate code intelligence but heavy and slow
- Zoekt = fast indexed search but no AST
- Sourcegraph = most complete but requires infrastructure

search-index occupies the **"fast enough for interactive AI agents, structured enough for code navigation"** niche that none of the above serve.

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

## Summary Matrix

| Decision        | Chosen                 | Key Reason                  | Reconsider When              |
| --------------- | ---------------------- | --------------------------- | ---------------------------- |
| Storage         | Bincode                | Fast, simple, single-file   | Index > 4GB RAM              |
| File index      | Vec + O(n) scan        | 90× faster than FS walk, simple | File count > 500K        |
| Content index   | HashMap                | O(1) lookup, easy updates   | Need fuzzy search            |
| Ranking         | TF-IDF                 | Simple, effective for code  | Ranking quality complaints   |
| Concurrency     | RwLock                 | Correct, minimal contention | High-throughput multi-client |
| Code parsing    | tree-sitter            | Full AST, structured output | n/a (correct choice)         |
| Tokenization    | char-split + lowercase | Fast, language-agnostic     | Need camelCase/fuzzy         |
| Transport       | Stdio                  | Zero-config, lowest latency | Need remote access           |
| Positioning     | Custom single-binary   | Only tool combining all 4 index types + MCP | Need precise type inference (add SCIP/LSIF) |
