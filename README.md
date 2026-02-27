# Search — High-Performance Code Search Engine

Inverted index + AST-based code intelligence engine for large-scale codebases. Millisecond content search, structural code navigation (classes, methods, call trees), and native MCP server for AI agent integration — in a single statically-linked Rust binary.

**Measured on a real C# codebase with 49,000 files, 846K definitions ([full benchmarks](docs/benchmarks.md)):**

| Metric | Value |
|---|---|
| Indexed content search (MCP, in-memory) | **0.6–4ms** per query (**250–6,500×** faster than live scan/ripgrep) |
| Call tree (3 levels) | **0.5ms** |
| Find interface implementations | **0.6ms** |
| Index build | **7–16s** (content), **16–32s** (AST definitions) — varies by CPU |
| Incremental update | **<1s** per file change (content + AST re-parse) |
| Index load from disk | **0.7–1.6s** (242 MB content index) |
| Binary size | Single static binary, zero runtime dependencies |

> Built on the same [`ignore`](https://crates.io/crates/ignore) crate used by [ripgrep](https://github.com/BurntSushi/ripgrep), with [`tree-sitter`](https://tree-sitter.github.io/) for language-aware code parsing.

## What Can You Do With It?

| Scenario | Without search-index | With search-index* |
|---|---|---|
| 🐛 **Debug a stack trace** — find the exact method, trace all callers to the API entry point | ~5 min per stack frame | **3 seconds** |
| 🏗️ **Understand unfamiliar code** — map classes, call trees, and dependencies of a module you've never seen | ~40 min of manual exploration | **2 minutes** |
| 📝 **Review a PR** — check who else calls changed methods, spot missing patterns | ~8 min of searching | **<1 second** |
| 🔄 **Refactor safely** — find every caller, every implementation, every DI registration | multiple manual searches | **one `search_callers` call** |
| 📊 **Estimate task scope** — "how many files use this feature?" | ~5 min | **30 seconds** |
| 🧪 **Write tests** — find existing test patterns, discover all dependencies to mock | ~10 min browsing | **<1 second** |
| 🕵️ **Investigate file history** — who changed this file? What was modified last week? Show me the diff from a specific commit. | ~5 min of git log | **<1 second** |
> \* Times in the "With search-index" column are **pure tool execution time** (index lookup + response). In practice, add ~1–2 seconds of LLM latency (model thinking + MCP round-trip) which is outside the tool's control.

> 📖 **More:** [Use Cases & Vision](docs/use-cases.md) — detailed scenarios including AI-powered architecture exploration, automated impact analysis, and a real-world case study where we reverse-engineered a 3,800-line system in 5 minutes.

## Documentation

| Document | Description |
|---|---|
| [Use Cases & Vision](docs/use-cases.md) | Real-world scenarios, future ideas, and case studies |
| [CLI Reference](docs/cli-reference.md) | All commands with examples and options |
| [MCP Server Guide](docs/mcp-guide.md) | Setup, tools API, JSON-RPC examples |
| [Architecture](docs/architecture.md) | System overview, component design, data flow diagrams |
| [Storage Model](docs/storage.md) | Index formats, serialization, staleness, incremental updates |
| [Concurrency](docs/concurrency.md) | Thread model, lock strategy, watcher design |
| [Trade-offs](docs/tradeoffs.md) | Design decisions with alternatives considered |
| [Benchmarks](docs/benchmarks.md) | Performance data, scaling estimates, industry comparison |
| [2026-02-21 Feature Release](docs/llm-testing-guide.md) | Design & implementation summary: git tools, branch awareness, call graph fixes, code metrics, performance |
| [E2E Test Plan](docs/e2e-test-plan.md) | 40+ end-to-end test cases (24 CLI + 16 MCP) with automation script |
| [Git History Cache Design](user-stories/git-history-cache-design.md) | Cache architecture, data structures, lifecycle, invalidation strategy |
| [Changelog](CHANGELOG.md) | All notable changes organized by category (features, fixes, performance) |

## Features

- **Parallel filesystem walk** — uses all available CPU cores for maximum throughput
- **File name index** — pre-built index for instant file lookups (like [Everything](https://www.voidtools.com/))
- **Inverted content index** — language-agnostic tokenizer maps tokens to files for instant full-text search across any text file (like Elasticsearch)
- **TF-IDF ranking** — content search results sorted by relevance, most relevant files first
- **Relevance ranking** — `search_definitions` and `search_fast` results sorted by match quality: exact match → prefix → contains, with kind and name-length tiebreakers
- **Regex support** — full Rust regex syntax for pattern matching
- **Respects `.gitignore`** — automatically skips ignored files
- **Extension filtering** — limit search to specific file types
- **MCP Server** — native Model Context Protocol server for AI agents (Roo Code, Cline, or any MCP-compatible client) with async startup
- **Code definition index** — tree-sitter AST parsing for structural code search *(C# and TypeScript/TSX)*, regex-based parsing for *SQL* (.sql files: stored procedures, tables, views, functions, types, indexes, columns, and call sites from SP bodies). Angular components enriched with template metadata (selector, child components from HTML templates)
- **Code complexity metrics** — 7 metrics computed during AST indexing: cyclomatic complexity, cognitive complexity (SonarSource), max nesting depth, parameter count, return/throw count, call count, lambda count. Query with `includeCodeStats`, sort by any metric, filter with `min*` thresholds
- **Parallel tokenization** — content index tokenization parallelized across all CPU cores
- **Parallel parsing** — multi-threaded tree-sitter parsing with lazy grammar loading
- **File watcher** — incremental index updates on file changes (<1s per file)
- **Substring search** — trigram-indexed substring matching within tokens (~0.07ms vs ~44ms for regex)
- **LZ4 index compression** — all index files compressed on disk with backward-compatible loading
- **Branch awareness** — automatic `branchWarning` in search responses when working on non-main branches
- **Graceful shutdown** — handles Ctrl+C (SIGTERM/SIGINT) by saving indexes to disk before exit, preserving incremental watcher updates
- **Git history cache** — background-built compact in-memory cache (~7.6 MB for 50K commits) for sub-millisecond `search_git_history`, `search_git_diff`, `search_git_authors`, `search_git_activity`, and `search_git_blame` queries. Saved to disk (LZ4 compressed) for instant restart (~100 ms load vs ~59 sec rebuild). Auto-detects HEAD changes for cache invalidation. Supports `author` and `message` filters on history/diff/activity/authors tools. Use `noCache: true` to bypass the cache and query git CLI directly when cache may be stale. `search_git_authors` accepts file paths, directory paths, or no path (entire repo). `search_git_blame` provides line-level attribution via `git blame`

## Quick Start

### Installation

```bash
git clone <repo-url>
cd search
cargo build --release
```

Requires [Rust](https://rustup.rs/) 1.85+. Binary: `target/release/search-index.exe` (Windows) or `target/release/search-index` (Linux/Mac).

### Build with Feature Flags

Language parsers are configurable via Cargo features. By default, all parsers are included:

```bash
# Default: all parsers (C#, TypeScript/TSX, SQL)
cargo build --release

# C# only (no TypeScript/SQL parsers, no tree-sitter-typescript dependency)
cargo build --release --no-default-features --features lang-csharp

# SQL only (no tree-sitter dependency at all — smallest binary)
cargo build --release --no-default-features --features lang-sql

# C# + SQL, no TypeScript
cargo build --release --no-default-features --features lang-csharp,lang-sql

# No parsers (grep/content index only)
cargo build --release --no-default-features
```

| Feature | Dependencies | Parser |
|---|---|---|
| `lang-csharp` | `tree-sitter`, `tree-sitter-c-sharp` | C# AST (tree-sitter) |
| `lang-typescript` | `tree-sitter`, `tree-sitter-typescript` | TypeScript/TSX AST (tree-sitter) |
| `lang-sql` | *(none)* | SQL DDL (regex-based) |
| `lang-rust` | `tree-sitter`, `tree-sitter-rust` | Rust AST (tree-sitter) — **not in default features** |

### CLI Usage

```bash
# Build content index for C# files
search-index content-index -d C:\Projects -e cs

# Search by token (TF-IDF ranked)
search-index grep "HttpClient" -d C:\Projects -e cs

# Search file names (instant)
search-index fast "UserService" -d C:\Projects -e cs

# Live filesystem search (no index needed)
search-index find "TODO" -d C:\Projects --contents -e cs
```

See [CLI Reference](docs/cli-reference.md) for all commands and options.

### MCP Server (AI Agent Integration)

```bash
# Start MCP server with file watching and code definitions
search-index serve --dir C:\Projects --ext cs --watch --definitions
```

See [MCP Server Guide](docs/mcp-guide.md) for VS Code setup, tools API, and examples.

## Architecture Overview

The engine uses three independent index types plus a git history cache:

| Index | File | Created by | Searched by | Stores |
|---|---|---|---|---|
| File name | `.file-list` | `search-index index` | `search-index fast` | File paths, sizes, timestamps |
| Content | `.word-search` | `search-index content-index` | `search-index grep` | Token → (file, line numbers) map |
| Definitions | `.code-structure` | `search-index def-index` | `search_definitions` / `search_callers` | AST-extracted classes, methods, call sites |
| Git history | `.git-history` | Background (auto) | `search_git_history` / `search_git_diff` / `search_git_authors` / `search_git_activity` / `search_git_blame` / `search_branch_status` | Commit metadata, file-to-commit mapping, branch status |

Indexes are stored in `%LOCALAPPDATA%\search-index\` and are language-agnostic for content search, language-specific (C#, TypeScript/TSX via tree-sitter; SQL via regex parser) for definitions. The git history cache builds automatically in the background when a `.git` directory is present. See [Architecture](docs/architecture.md) for details.

### Caller Tree Verification

The `search_callers` tool builds call trees by tracing method invocations through AST-parsed call-site data. Key design points:

- **Call-site verification is mandatory** — methods without parsed call-site data are filtered out (no false-positive fallback)
- **Expression body properties supported** — C# expression body properties (`public string Name => _service.GetName();`) have their call sites extracted and verified
- **Lambda / arrow function calls captured** — call sites inside lambdas (C#) and arrow functions (TypeScript) in argument lists are recursively parsed
- **Pre-filter uses class name and method name only** — base types and interfaces are not expanded during the pre-filter phase; inheritance verification happens during call-site validation via receiver type matching
- **`direction=down` cross-class scoping** — when building callee trees, unqualified calls without a receiver type resolve only to methods in the caller's own class (prevents cross-class pollution at depth ≥ 2)
- **Generic arity mismatch filter** — `new Foo<T>()` call sites skip non-generic classes with the same name (e.g., `new List<CatalogEntry>()` won't resolve to a non-generic `List` class)
- **Built-in type blocklist** — 60+ built-in receiver types (Promise, Array, Map, String, Object, etc.) are excluded from `direction=down` resolution, preventing false positives like `Promise.resolve()` matching `Deferred.resolve()`
- **Fuzzy DI interface matching** — finds callers through non-standard interface naming conventions (e.g., `IDataModelService` → `DataModelWebService`) using suffix-tolerant matching against the `base_type_index`
- **Type inference for local variables** — cast expressions (`(Type)expr`), `as` expressions (`expr as Type`), method return types (`var x = GetStream()`), `await`/`Task<T>` unwrap (`var x = await GetStreamAsync()`), pattern matching (`if (obj is Type name)`, `case Type name:`), and extension method detection. Cross-class method return types are NOT resolved (only same-class methods)
- **Local variable limitation** — calls through local variables (e.g., `var x = service.GetFoo(); x.Bar()`) may not be detected when the return type cannot be inferred. DI-injected fields, `this`/`base` calls, and direct receiver calls are fully supported

### Angular Template Metadata

Angular `@Component` class definitions are automatically enriched with template metadata during definition indexing:

- **What it does** — extracts `selector` and `templateUrl` from the `@Component()` decorator, reads the paired `.html` file, and scans for custom elements (tags with hyphens) used in the template
- **`search_definitions`** — Angular components include `selector` (e.g., `"app-user-profile"`) and `templateChildren` (list of child component selectors found in the HTML)
- **Component tree navigation via `search_callers`**:
  - `direction='down'` with a component class name → shows child components from the HTML template (recursive with `depth`)
  - `direction='up'` with a selector (e.g., `"app-footer"`) → finds parent components that use it in their templates
- **HTML content search** — add `html` to `--ext` / `ext` parameter for `search_grep` to search HTML template content

**Limitations:** Only external templates (`templateUrl`), not inline `template:`. Only tags with hyphens (custom elements per HTML spec). `ng-*` tags are excluded (Angular built-ins). Template metadata updates on full `def-index` rebuild, not incrementally on `.html` changes.

## Dependencies

| Crate | Purpose |
|---|---|
| [ignore](https://crates.io/crates/ignore) | Parallel directory walking (from ripgrep) |
| [clap](https://crates.io/crates/clap) | CLI argument parsing |
| [regex](https://crates.io/crates/regex) | Regular expression support |
| [serde](https://crates.io/crates/serde) + [bincode](https://crates.io/crates/bincode) | Fast binary serialization for indexes and git cache |
| [serde_json](https://crates.io/crates/serde_json) | JSON serialization for MCP protocol |
| [notify](https://crates.io/crates/notify) | Cross-platform filesystem notifications |
| [dirs](https://crates.io/crates/dirs) | Platform-specific data directory paths |
| [tree-sitter](https://crates.io/crates/tree-sitter) | Incremental parsing for code definition extraction |
| [tracing](https://crates.io/crates/tracing) | Structured diagnostic logging |
| [criterion](https://crates.io/crates/criterion) | Statistical benchmarking (dev) |
| [proptest](https://crates.io/crates/proptest) | Property-based testing (dev) |

## Testing

```bash
# Run all tests (474 unit tests + 47 E2E tests)
cargo test

# Run benchmarks
cargo bench
```

Test files are split by language module for maintainability:

| Module | Test files |
|---|---|
| `src/mcp/handlers/` | `handlers_tests.rs` (77 general), `handlers_tests_csharp.rs` (31 C#), `handlers_tests_typescript.rs` (TS), `handlers_tests_rust.rs` (6 Rust, `lang-rust` feature) |
| `src/definitions/` | `definitions_tests.rs` (12 general), `definitions_tests_csharp.rs` (19 C#), `definitions_tests_typescript.rs` (32 TS), `definitions_tests_sql.rs` (SQL), `definitions_tests_rust.rs` (18 Rust, `lang-rust` feature) |
| `src/git/` | `cache_tests.rs` (49 cache), `git_tests.rs` (git CLI) |
| `src/` | `main_tests.rs` (35 general) |

| Category | Tests |
|---|---|
| Unit tests | `clean_path` (path separator normalization), `tokenize`, staleness, serialization roundtrips, TF-IDF ranking |
| Integration | Build + search ContentIndex, build FileIndex, MCP server end-to-end |
| MCP Protocol | JSON-RPC parsing, initialize, tools/list, tools/call, notifications, errors |
| Substring/Trigram | Trigram generation, index build, substring search, 13 e2e integration tests |
| Definitions | C# parsing (tree-sitter), TypeScript/TSX parsing (tree-sitter), SQL parsing (regex-based), incremental update |
| Git cache | Streaming parser, path normalization, query API, serialization roundtrip, disk persistence, HEAD validation |
| Property tests | Tokenizer invariants, posting roundtrip, index consistency, TF-IDF ordering |
| Benchmarks | Tokenizer throughput, index lookup latency, TF-IDF scoring, regex scan |

## Author

Sergey Pustynsky

## License

Licensed under either of:

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)