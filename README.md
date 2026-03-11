# Search — High-Performance Code Search Engine

Inverted index + AST-based code intelligence engine for large-scale codebases. Millisecond content search, structural code navigation (classes, methods, call trees), and native MCP server for AI agent integration — in a single statically-linked Rust binary.

**Measured on a real production codebase with 66K files, 878K definitions ([full benchmarks](docs/benchmarks.md)):**

| Metric | Value |
|---|---|
| Indexed content search (MCP, in-memory) | **1.7–2.3ms** per query (substring, typical) |
| Content search — high-frequency term | **~15ms** (208K occurrences) |
| Call tree — callees (direction=down) | **0.5ms** |
| Call tree — callers (direction=up, depth 3) | **3–11ms** |
| Find implementations (baseType) | **1.3ms** |
| Find by attribute | **0.4ms** |
| Index build | **7–16s** (content), **16–32s** (AST definitions) — varies by CPU |
| Incremental update | **<1s** per file change (content + AST re-parse) |
| Index load from disk | **0.7–1.6s** (242 MB content index) |
| Binary size | Single static binary, zero runtime dependencies |

> Built on the same [`ignore`](https://crates.io/crates/ignore) crate used by [ripgrep](https://github.com/BurntSushi/ripgrep), with [`tree-sitter`](https://tree-sitter.github.io/) for language-aware code parsing.

## What Can You Do With It?

| Scenario | Without search-index | With search-index* |
|---|---|---|
| 🐛 **Debug a stack trace** — find the exact method, trace all callers to the API entry point | ~5 min per stack frame | **< 15 seconds** |
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
| [E2E Test Plan](docs/e2e-test-plan.md) | End-to-end test cases (CLI + MCP) with automation script |
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
- **MCP Server** — native Model Context Protocol server for AI agents (Roo Code, Cline, or any MCP-compatible client) with async startup, named `SEARCH_INDEX_POLICY` initialization guidance, and per-response policy reminders to reduce tool-selection drift
- **Code definition index** — tree-sitter AST parsing for structural code search *(C# and TypeScript/TSX)*, regex-based parsing for *SQL* (.sql files: stored procedures, tables, views, functions, types, indexes, columns, and call sites from SP bodies). Angular components enriched with template metadata (selector, child components from HTML templates)
- **Code complexity metrics** — 7 metrics computed during AST indexing: cyclomatic complexity, cognitive complexity (SonarSource), max nesting depth, parameter count, return/throw count, call count, lambda count. Query with `includeCodeStats`, sort by any metric, filter with `min*` thresholds
- **Parallel tokenization** — content index tokenization parallelized across all CPU cores
- **Parallel parsing** — multi-threaded tree-sitter parsing with lazy grammar loading
- **File watcher** — incremental index updates on file changes (<1s per file)
- **Substring search** — trigram-indexed substring matching within tokens (~0.07ms vs ~44ms for regex)
- **LZ4 index compression** — all index files compressed on disk with backward-compatible loading
- **Branch awareness** — automatic `branchWarning` in search responses when working on non-main branches
- **Graceful shutdown** — handles Ctrl+C (SIGTERM/SIGINT) by saving indexes to disk before exit, preserving incremental watcher updates
- **Git history cache** — background-built compact in-memory cache for sub-millisecond git queries (`search_git_history`, `search_git_authors`, `search_git_activity`, `search_git_blame`). LZ4-compressed on disk for instant restart. See [Architecture](docs/architecture.md) for details

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
| `lang-rust` | `tree-sitter`, `tree-sitter-rust` | Rust AST (tree-sitter) |

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

For caller tree verification details (DI resolution, type inference, false-positive filtering) and Angular template metadata, see [Architecture](docs/architecture.md).

## Dependencies

| Crate | Purpose |
|---|---|
| [similar](https://crates.io/crates/similar) | Unified diff generation for `search_edit` tool |
| [ignore](https://crates.io/crates/ignore) | Parallel directory walking (from ripgrep) |
| [clap](https://crates.io/crates/clap) | CLI argument parsing |
| [regex](https://crates.io/crates/regex) | Regular expression support |
| [serde](https://crates.io/crates/serde) + [bincode](https://crates.io/crates/bincode) | Fast binary serialization for indexes and git cache |
| [serde_json](https://crates.io/crates/serde_json) | JSON serialization for MCP protocol |
| [notify](https://crates.io/crates/notify) | Cross-platform filesystem notifications |
| [dirs](https://crates.io/crates/dirs) | Platform-specific data directory paths |
| [tree-sitter](https://crates.io/crates/tree-sitter) | Incremental parsing for code definition extraction (C#, TypeScript/TSX, Rust) |
| [lz4_flex](https://crates.io/crates/lz4_flex) | LZ4 frame compression for index files on disk |
| [mimalloc](https://crates.io/crates/mimalloc) | High-performance memory allocator |
| [thiserror](https://crates.io/crates/thiserror) | Ergonomic error type definitions |
| [tracing](https://crates.io/crates/tracing) + [tracing-subscriber](https://crates.io/crates/tracing-subscriber) | Structured diagnostic logging |
| [ctrlc](https://crates.io/crates/ctrlc) | Graceful shutdown signal handling |
| [criterion](https://crates.io/crates/criterion) | Statistical benchmarking (dev) |
| [proptest](https://crates.io/crates/proptest) | Property-based testing (dev) |

## Testing

```bash
# Run all unit tests (~1200+)
cargo test

# Run benchmarks
cargo bench

# Run E2E tests (~60+ CLI + MCP tests)
pwsh -File e2e-test.ps1
```

Test files are split by language module for maintainability — see [Architecture](docs/architecture.md) for the full module structure. Key test categories:

| Category | Coverage |
|---|---|
| Unit tests | Tokenizer, path normalization, staleness, serialization roundtrips, TF-IDF ranking |
| Integration | Build + search ContentIndex, build FileIndex, MCP server end-to-end |
| MCP Protocol | JSON-RPC parsing, initialize, tools/list, tools/call, notifications, errors |
| Substring/Trigram | Trigram generation, index build, substring search, integration tests |
| Definitions | C# (tree-sitter), TypeScript/TSX (tree-sitter), Rust (tree-sitter), SQL (regex-based), incremental update |
| Callers | Call tree up/down, DI resolution, overloads, cycles, impact analysis |
| Git cache | Streaming parser, path normalization, query API, serialization roundtrip, disk persistence, HEAD validation |
| Property tests | Tokenizer invariants, posting roundtrip, index consistency, TF-IDF ordering |
| Benchmarks | Tokenizer throughput, index lookup latency, TF-IDF scoring, regex scan |

## Author

Sergey Pustynsky

## License

Licensed under either of:

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)