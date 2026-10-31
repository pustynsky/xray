# Development Guide

Building Xray from source, choosing language parsers, running tests, and the crates it depends on. For end-user setup, see the [Installation guide](installation.md).

## Build from source

```bash
git clone https://github.com/pustynsky/xray
cd xray
cargo build --release
```

Needs [Rust](https://rustup.rs/) 1.91+ (MSRV; see `rust-version` in [Cargo.toml](../Cargo.toml)). Binary: `target/release/xray.exe` (Windows) or `target/release/xray` (Linux/macOS). Pre-built releases are published for Windows x64 only, so Linux and macOS users build from source.

The update-check HTTPS client uses native TLS. Linux builds also need OpenSSL
development libraries and `pkg-config` (for example, `libssl-dev` and
`pkg-config` on Ubuntu); the executable needs the corresponding OpenSSL runtime.
Windows/macOS use their system TLS providers.
Update-check regressions use local HTTP fixtures; no routine test downloads
releases or executes an update script.

## Build with feature flags

Tree-sitter language parsers are configurable via Cargo features. The SQL parser is always built in (regex-based, no tree-sitter dependency) and isn't feature-gated. Default features enable C#, TypeScript/TSX, Rust, and on-demand XML parsing:

```bash
# Default: C#, TypeScript/TSX, Rust, XML on-demand (SQL is always on)
cargo build --release

# C# only. Drops TypeScript/Rust/XML tree-sitter grammars. SQL still works.
cargo build --release --no-default-features --features lang-csharp

# C# and Rust. No TypeScript, no XML.
cargo build --release --no-default-features --features lang-csharp,lang-rust

# Smallest binary: no tree-sitter at all. SQL regex parser plus content/file indexes only.
cargo build --release --no-default-features
```

| Feature | Dependencies | Parser |
|---|---|---|
| `lang-csharp` *(default)* | `tree-sitter`, `tree-sitter-c-sharp` | C# AST (tree-sitter) |
| `lang-typescript` *(default)* | `tree-sitter`, `tree-sitter-typescript` | TypeScript/TSX AST (tree-sitter) |
| `lang-rust` *(default)* | `tree-sitter`, `tree-sitter-rust` | Rust AST (tree-sitter) |
| `lang-xml` *(default)* | `tree-sitter`, `tree-sitter-xml` | XML / `.csproj` / `.config` / `.props` / `.targets` / `.resx` / `.nuspec` / `.vsixmanifest` / `.appxmanifest` / `.manifestxml` (on-demand, tree-sitter) |
| *(always built-in, no feature)* | *(none)* | SQL DDL (regex-based: stored procedures, tables, views, functions, types, indexes, columns, call sites) |

See [Architecture — Language support](architecture.md#language-support) for the definition kinds each parser extracts.

## Testing

```bash
# Run all unit tests (~3000+; cargo test --list reports 2,982 in xray bin + 108 in lib)
cargo test

# Run benchmarks
cargo bench

# Run E2E tests (~35 CLI + MCP tests in e2e-test.ps1; the full E2E spec catalog in docs/e2e/ is larger)
pwsh -File e2e-test.ps1
```

Test files are split by language module to stay maintainable. See [Architecture](architecture.md#module-structure) for the module layout, [E2E test plan](e2e/README.md) for end-to-end scenarios, and [Benchmarks](benchmarks.md) for performance methodology. Test categories:

| Category | Coverage |
|---|---|
| Unit tests | Tokenizer, path normalization, staleness, serialization roundtrips, content relevance ranking |
| Integration | Build + search ContentIndex, build FileIndex, MCP server end-to-end |
| MCP protocol | JSON-RPC parsing, initialize, tools/list, tools/call, notifications, errors |
| Substring/Trigram | Trigram generation, index build, substring search, integration tests |
| Definitions | C# (tree-sitter), TypeScript/TSX (tree-sitter), Rust (tree-sitter), SQL (regex-based), incremental update |
| Callers | Call tree up/down, DI resolution, overloads, cycles, impact analysis |
| Git cache | Streaming parser, path normalization, query API, serialization roundtrip, disk persistence, HEAD validation |
| Property tests | Tokenizer invariants, posting roundtrip, index consistency, TF-IDF ordering |
| Benchmarks | Tokenizer throughput, index lookup latency, TF-IDF scoring, regex scan |

## Dependencies

| Crate | Purpose |
|---|---|
| [similar](https://crates.io/crates/similar) | Unified diff generation for `xray_edit` |
| [ignore](https://crates.io/crates/ignore) | Parallel directory walking (from ripgrep) |
| [clap](https://crates.io/crates/clap) | CLI argument parsing |
| [regex](https://crates.io/crates/regex) | Regular expressions |
| [serde](https://crates.io/crates/serde) + [bincode](https://crates.io/crates/bincode) | Binary serialization for indexes and git cache |
| [serde_json](https://crates.io/crates/serde_json) | JSON serialization for the MCP protocol |
| [notify](https://crates.io/crates/notify) | Cross-platform filesystem notifications |
| [dirs](https://crates.io/crates/dirs) | Platform-specific data directory paths |
| [ureq](https://crates.io/crates/ureq) | Bounded background HTTPS release checks with native TLS |
| [semver](https://crates.io/crates/semver) | Semantic release-version comparison |
| [tree-sitter](https://crates.io/crates/tree-sitter) | Incremental parsing for code definition extraction (C#, TypeScript/TSX, Rust) |
| [lz4_flex](https://crates.io/crates/lz4_flex) | LZ4 frame compression for index files on disk |
| [mimalloc](https://crates.io/crates/mimalloc) | Memory allocator |
| [thiserror](https://crates.io/crates/thiserror) | Error type definitions |
| [tracing](https://crates.io/crates/tracing) + [tracing-subscriber](https://crates.io/crates/tracing-subscriber) | Structured diagnostic logging |
| [ctrlc](https://crates.io/crates/ctrlc) | Graceful shutdown signal handling |
| [criterion](https://crates.io/crates/criterion) | Statistical benchmarking (dev) |
| [proptest](https://crates.io/crates/proptest) | Property-based testing (dev) |
