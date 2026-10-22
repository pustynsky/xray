# Xray — code intelligence engine

[![VirusTotal](https://badges.cssnr.com/vt/id/d7f58d1ff45bafee77112b4459ff0352aa79e74081c9624cf51df3417d13169f?label=xray.exe)](https://www.virustotal.com/gui/file/d7f58d1ff45bafee77112b4459ff0352aa79e74081c9624cf51df3417d13169f)

_Scanned by VirusTotal — no threats found._

An inverted index and AST-based code intelligence engine for large codebases. It runs millisecond content searches, walks call trees, analyzes git history, and exposes a native MCP server for AI agents. One statically linked Rust binary, no runtime deps. Agents get direct structural access to the code instead of guessing through shallow text search.

**Measured on a real production codebase with 66K files and 878K definitions ([full benchmarks](docs/benchmarks.md)):**

| Metric | Value |
|---|---|
| Indexed content search (MCP, in-memory) | 1.7–2.3ms per query (substring, typical) |
| Content search, high-frequency term | ~15ms (208K occurrences) |
| Call tree, callees (direction=down) | 0.5ms |
| Call tree, callers (direction=up, depth 3) | 3–11ms |
| Find implementations (baseType) | 1.3ms |
| Find by attribute | 0.4ms |
| Index build | 7–16s (content), 16–32s (AST definitions); depends on CPU |
| Incremental update | <1s per file change (content + AST re-parse) |
| Index load from disk | 0.7–1.6s (242 MB content index) |
| Binary size | Single static binary, zero runtime dependencies |

Built on the same [`ignore`](https://crates.io/crates/ignore) crate that backs [ripgrep](https://github.com/BurntSushi/ripgrep), with [`tree-sitter`](https://tree-sitter.github.io/) for language-aware code parsing.

## What can you do with it?

| Scenario | Without xray | With xray\* |
|---|---|---|
| 🐛 **Debug a stack trace** — find the exact method, trace callers up to the API entry point | ~5 min per stack frame | < 15 seconds |
| 🏗️ **Understand unfamiliar code** — map classes, call trees, and dependencies of a module you've never seen | ~40 min of manual exploration | 2 minutes |
| 📝 **Review a PR** — check who else calls changed methods, spot missing patterns | ~8 min of searching | <1 second |
| 🔄 **Refactor safely** — find every caller, every implementation, every DI registration | several manual searches | one `xray_callers` call |
| 📊 **Estimate task scope** — "how many files use this feature?" | ~5 min | 30 seconds |
| 🧪 **Write tests** — find existing test patterns, discover dependencies to mock | ~10 min of browsing | <1 second |
| 🕵️ **Investigate file history** — who changed this file? What was modified last week? Show me the diff from a specific commit. | ~5 min of git log | <1 second |

\* Times in the "With xray" column are pure tool execution time (index lookup + response). In practice, add ~1–2 seconds of LLM latency (model thinking + MCP round-trip), which is outside the tool's control.

📖 **More:** [Use cases & LLM workflows](docs/use-cases.md). Detailed scenarios, AI-driven architecture exploration, automated impact analysis, and a case study where we reverse-engineered a 3,800-line system in 5 minutes.

## Make your agent prefer Xray

Connecting the MCP server makes Xray available, but does not guarantee that your
agent will choose it over built-in search tools. Xray sends tool-selection
guidance during MCP initialization and reminders in responses.

For a quick trial, tell your agent:

> Use Xray first for code discovery in this workspace: file lookup, content
> search, symbol exploration, source reading, and caller/callee navigation.
> If Xray is unavailable or cannot cover the operation, briefly explain why
> before using an alternative. Keep existing editing, build, and test workflows.

To keep this preference across sessions, follow the
[manual Xray-first setup](docs/installation.md#make-xray-preferred-for-code-discovery).
It includes a copyable instruction block, client-specific guidance, verification,
and removal steps. This is a preference, not an exclusive-tool guarantee.

## Documentation

| Document | Description |
|---|---|
| [Installation guide](docs/installation.md) | Download `xray.exe`, configure VS Code Copilot Chat / Roo Code / Cline as an MCP client (Windows) |
| [Use cases & LLM workflows](docs/use-cases.md) | Scenarios, ready-made LLM tool chains, case studies |
| [CLI reference](docs/cli-reference.md) | All commands with examples and options |
| [MCP server guide](docs/mcp-guide.md) | Setup, tools API, JSON-RPC examples |
| [Architecture](docs/architecture.md) | System overview, component design, data-flow diagrams |
| [DI support](docs/di-support.md) | What's resolved automatically across MEDI / Autofac / Lamar / SimpleInjector / etc., what isn't, and recipes for DI-shaped codebases |
| [Storage model](docs/storage.md) | Index formats, serialization, staleness, incremental updates |
| [Concurrency](docs/concurrency.md) | Thread model, lock strategy, watcher design |
| [Trade-offs](docs/tradeoffs.md) | Design decisions and the alternatives we rejected |
| [Benchmarks](docs/benchmarks.md) | Performance data, scaling estimates, industry comparison |
| [E2E test plan](docs/e2e/README.md) | End-to-end test cases (CLI + MCP), split by tool: 8 spec files plus a README |
| [Changelog](CHANGELOG.md) | Changes grouped by category (features, fixes, performance) |

## Features

- **Parallel filesystem walk**: uses every CPU core for max throughput.
- **File name index**: pre-built index for instant file lookups, like [Everything](https://www.voidtools.com/).
- **Inverted content index**: language-agnostic tokenizer maps tokens to files for full-text search across any text file, like a small Elasticsearch.
- **Content relevance ranking**: TF-IDF orders matches, with a normalized file-stem signal for single exact-token searches.
- **Relevance ranking for symbols**: `xray_definitions` and `xray_fast` rank by match quality. Exact > prefix > contains, with kind and name length as tiebreakers.
- **Regex support**: full Rust regex syntax.
- **Respects `.gitignore`**: ignored files are skipped automatically.
- **Extension filtering**: limit search to specific file types.
- **MCP server**: native Model Context Protocol server for AI agents (Roo Code, Cline, any MCP-compatible client). Async startup, named `XRAY_POLICY` initialization guidance, and per-response policy reminders to keep tool selection on rails.
- **GenAI grounding**: agents get structural access to code, git history, call trees, and a safe edit path. They answer questions instead of chaining greps and hoping for the best.
- **Synchronous reindex after `xray_edit`**: file-edit responses refresh the in-memory content and definition indexes before returning. A follow-up `xray_grep`, `xray_definitions`, `xray_callers`, or `xray_fast` call sees the new content right away, with no wait for the 500ms watcher debounce.
- **`xray_edit` is workspace-scope-agnostic by design**: unlike the read/index tools, `xray_edit` takes both relative paths (resolved against `--dir`) and absolute paths anywhere on disk. That's intentional. One server instance can handle edits across multiple workspaces, scratch dirs, or tooling configs without a relaunch. Read-only tools (`xray_grep`, `xray_definitions`, `xray_callers`, `xray_fast`) stay workspace-bound so the in-memory indexes stay scoped and workspace topology doesn't leak to disk.
- **Code definition index**: tree-sitter AST parsing for C#, TypeScript/TSX, and Rust. Regex-based parsing for SQL (.sql files: stored procedures, tables, views, functions, types, indexes, columns, and call sites from SP bodies). On-demand tree-sitter parsing for XML / .csproj / .config / .props / .targets / .resx / .nuspec / .vsixmanifest / .appxmanifest / .manifestxml. Angular components carry template metadata (selector, child components from HTML templates).
- **DI-aware call trees**: `xray_callers` resolves callers through interface receivers (constructor, property, method, or field injection), I-prefix conventions, declared `base_types`, fuzzy field-name matching for DI-injected fields (`_userService`, `m_userService`), and per-method local-variable type inference (var, cast, `as`, await, pattern matching). Works across MEDI / Autofac / Lamar / SimpleInjector / DryIoc / Ninject / MEF / source generators without parsing container registrations. See [DI support](docs/di-support.md) for the matrix and known gaps.
- **Code complexity metrics**: 7 metrics computed during AST indexing — cyclomatic complexity, cognitive complexity (SonarSource), max nesting depth, parameter count, return/throw count, call count, lambda count. Query with `includeCodeStats`, sort by any metric, filter by exact `paramCount` for C#/TypeScript/TSX/Rust, or use `min*` thresholds.
- **Exact parameter-count search**: `xray_definitions {"name":["Route"],"parent":["Router"],"exactNameOnly":true,"paramCount":2}` matches the indexed parameter count for callables with code stats. This counts declarations, not required call arguments; Rust `self` is excluded. Accepts integers `0..=254` (255 is rejected because C#/TypeScript counts can saturate there), combines with `minParams` using AND, and enables `includeCodeStats`. SQL is excluded ([tracked separately](https://github.com/pustynsky/xray/issues/458)); XML on-demand, `containsLine`, and `audit` combinations return an error.
- **Parallel tokenization**: content index tokenization runs on all CPU cores.
- **Parallel parsing**: multi-threaded tree-sitter parsing with lazy grammar loading.
- **File watcher**: incremental index updates on file changes (<1s per file).
- **Substring search**: trigram-indexed substring matching within tokens (~0.07ms vs ~44ms for regex).
- **LZ4 index compression**: all index files compressed on disk, loading stays backward-compatible.
- **Branch awareness**: search responses include a `branchWarning` when you're on a non-main branch.
- **Graceful shutdown**: Ctrl+C (SIGTERM/SIGINT) saves indexes to disk before exit, preserving incremental watcher updates.
- **Git history cache**: background-built compact in-memory cache for sub-millisecond git queries (`xray_git_history`, `xray_git_authors`, `xray_git_activity`, `xray_git_blame`). LZ4-compressed on disk for instant restart. See [Architecture](docs/architecture.md) for details.

## Quick start

### Installation

Choose the setup scope:

| What you need | Use | When to use it |
|---|---|---|
| One repository | [setup-xray.ps1](scripts/setup-xray.ps1) with `-RepoPath <path> -UserMcp` | First install, update, or migration for one checkout. Add `-MigrateLegacy` to migrate old workspace registrations. |
| Existing Xray repositories under one directory | [setup-xray-repos.ps1](scripts/setup-xray-repos.ps1) with `-Root <path>` | Update the shared executable and migrate existing setups without enrolling other repositories. |
| All discovered repositories under one directory | The bulk script with `-Root <path> -IncludeNewRepos` | Also create workspace profiles for checkouts that do not have Xray yet. |

**Option A — automated setup (recommended).** Run the [setup script](scripts/setup-xray.ps1). It downloads the latest `xray.exe`, detects your project's file extensions, and configures both VS Code and GitHub Copilot CLI.

The examples below use `-UserMcp` for VS Code SDK Agent Host, ordinary
Copilot Chat, and CLI. This opt-in mode registers Xray in personal client configs and
keeps separate workspace profiles outside Git, without a CLI wrapper.
It requires a binary with `serve-workspace` support.
Without `-UserMcp`, setup still uses the legacy workspace-local mode.
Personal mode always includes the CLI/SDK registration used by Agent Host;
`-EnableCopilotCli` selects CLI/SDK-only setup. `-EnableVSCode` also includes
CLI/SDK in personal mode. In workspace mode, either switch selects only that client.
See [personal MCP setup](docs/installation.md#personal-mcp-with-workspace-profiles)
for profile selection and verification, and
[updating](docs/installation.md#5-updating-to-a-new-version) for updates and first-time migration.

Requirements: Windows x64 and Windows PowerShell 5.1+ or PowerShell 7+. Public releases download directly over HTTPS, so GitHub CLI is not required. Git-managed operations, including restore, require working Git. Non-Git folders remain supported without exclusion protection. No bash/perl filter runtime is needed.

Three ways to launch it. Pick whichever fits. Same script, same parameters.

**A1. One-liner (download and run inline).** Shortest path. The script is fetched and run in memory; nothing is written to disk. The script will prompt for the target repo path; pass `-RepoPath <path>` to skip the prompt.

```powershell
& ([scriptblock]::Create((Invoke-WebRequest 'https://raw.githubusercontent.com/pustynsky/xray/main/scripts/setup-xray.ps1' -UseBasicParsing).Content)) -UserMcp
```

**A1 from Command Prompt (`cmd.exe`).** CMD has no PowerShell host, so launch the same installer through the built-in `powershell.exe`:

```cmd
powershell.exe -NoLogo -NoProfile -Command "& ([scriptblock]::Create((Invoke-WebRequest 'https://raw.githubusercontent.com/pustynsky/xray/main/scripts/setup-xray.ps1' -UseBasicParsing).Content)) -UserMcp"
```

Put installer parameters inside the outer double quotes. In CMD, wrap parameter values containing spaces in PowerShell single quotes, not additional double quotes:

```cmd
powershell.exe -NoLogo -NoProfile -Command "& ([scriptblock]::Create((Invoke-WebRequest 'https://raw.githubusercontent.com/pustynsky/xray/main/scripts/setup-xray.ps1' -UseBasicParsing).Content)) -RepoPath 'C:\My Repo' -UserMcp -EnableVSCode"
```

**A2. Download, then run.** Saves the script to `%TEMP%` so you can read it before running.

```powershell
$tmp = Join-Path $env:TEMP 'setup-xray.ps1'
Invoke-WebRequest 'https://raw.githubusercontent.com/pustynsky/xray/main/scripts/setup-xray.ps1' -UseBasicParsing -OutFile $tmp
& $tmp -UserMcp
```

**A3. From a clone of this repo.** Use this if you want to pin a specific commit or tag, or if you're already iterating on the script locally.

```powershell
.\scripts\setup-xray.ps1 -UserMcp
```

Pin to a release tag instead of `main` for reproducibility: replace `main` in the URL with e.g. `v0.4.0`. Pass `-RepoPath <path>` to target a specific repo non-interactively. Add `-EnableCopilotCli` and `-EnableVSCode` to skip the interactive client prompts. `-UseBasicParsing` is required on Windows PowerShell 5.1 (no-op on PowerShell 7+) and avoids the IE-engine security prompt.

The script does this:

1. Downloads the latest `xray.exe` from [GitHub releases](https://github.com/pustynsky/xray/releases) to `%LOCALAPPDATA%\xray\`.
2. Scans the repo and suggests file extensions to index.
3. Creates `.vscode/mcp.json` with `servers.xray` and `.github/mcp.json` with `mcpServers.xray`. Both use the same executable and arguments. Other servers remain intact.
4. Excludes local configs and backups through Git's local `info/exclude`, including in worktrees. If repository rules re-include a config, an untracked per-directory ignore helper protects it. Tracked target files are skipped, not hidden with index flags or filters.
5. Offers persistent Copilot CLI folder trust interactively (Enter means Yes). `COPILOT_HOME` is respected; `-Force` and non-interactive hosts never grant trust silently.

Ordinary setup leaves the root `.mcp.json` untouched. Legacy entries and filters require explicit migration consent, either an interactive confirmation or `-MigrateLegacy`. `-GitVisibility` is deprecated and accepted with a warning; it no longer selects an installation strategy.

See the [Installation guide](docs/installation.md) for legacy migration, manual setup, Cline configuration, and the Roo Code note (the `-EnableRoo` switch is currently a no-op).

**A4. Set up or migrate a directory of repositories.** Use the
[bulk setup script](scripts/setup-xray-repos.ps1) when you have several checkouts,
such as `C:\Repos\Service`, `C:\Repos\WebClient`, and `C:\Repos\Xray`.
Run these commands from an Xray checkout, or download both setup scripts into
the same directory and adjust the script path:

```powershell
# Existing Xray setups only: inspect the plan, then apply it.
.\scripts\setup-xray-repos.ps1 -Root C:\Repos -DryRun
.\scripts\setup-xray-repos.ps1 -Root C:\Repos

# Also enroll repositories without Xray: inspect this broader plan first.
.\scripts\setup-xray-repos.ps1 -Root C:\Repos -IncludeNewRepos -DryRun
.\scripts\setup-xray-repos.ps1 -Root C:\Repos -IncludeNewRepos
```

Both bulk modes use one shared executable and separate workspace profiles
outside Git. Existing supported settings are preserved; conflicting setups are
reported rather than overwritten. Discovery stops at each checkout, so this
does not include nested repositories inside it.

`-DryRun` makes no changes or downloads. Applying the plan downloads the latest
published executable once, affecting **all workspaces using it**, even outside
`-Root`. Add `-SkipDownload` to keep a local/custom build. Close active Xray
connections before replacing the executable, or explicitly allow `-KillRunning`;
`-Force` only approves the plan, not process stops.
See [bulk setup details](docs/installation.md#setup-across-a-repository-directory)
for discovery rules, conflicts, and extension selection.

**Option B — pre-built binary (manual).** Grab `xray.exe` from the [GitHub releases page](https://github.com/pustynsky/xray/releases) ([direct link](https://github.com/pustynsky/xray/releases/latest)), drop it into a folder, and follow the [Installation guide](docs/installation.md) for manual VS Code Copilot Chat / Roo Code / Cline setup.

**Option C — build from source.**

```bash
git clone https://github.com/pustynsky/xray
cd xray
cargo build --release
```

Needs [Rust](https://rustup.rs/) 1.91+ (MSRV; see `rust-version` in [Cargo.toml](Cargo.toml)). Binary: `target/release/xray.exe` (Windows) or `target/release/xray` (Linux/macOS).

### Build with feature flags

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

### CLI usage

```bash
# Build content index for C# files
xray content-index -d C:\Projects -e cs

# Search by token (TF-IDF ranked)
xray grep "HttpClient" -d C:\Projects -e cs

# Search file names (instant)
xray fast "UserService" -d C:\Projects -e cs
```

See [CLI reference](docs/cli-reference.md) for all commands and options.

### MCP server (AI agent integration)

```bash
# Start MCP server with file watching and code definitions
xray serve --dir C:\Projects --ext cs --watch --definitions
```

For end-user setup (download binary, configure Copilot Chat / Roo Code / Cline) see the [Installation guide](docs/installation.md). For the tools API, JSON-RPC schemas, and protocol details see the [MCP server guide](docs/mcp-guide.md).

During a dynamic workspace switch, an already-running content build follows the latest binding; `contentIndexAction: "build_in_progress"` does not require another request to schedule it. Poll `xray_info` until the content entry has no `status` field (an empty index is also ready). If a build fails for the current workspace, content tools return the JSON error `INDEX_BUILD_FAILED`, and `xray_info` reports content `status: "failed"` with a retry hint. Fix the underlying cause and call `xray_reindex`; polling alone does not retry a failed build. A roots change awaiting explicit reindexing is not a build failure.

### Response delivery

Set `XRAY_TRANSPORT_MAX_BYTES` in the MCP server environment when a client needs a hard response-text limit. For example, `16384` caps the combined UTF-8 text blocks, including guidance and metrics, at 16 KiB. The value must be an integer of at least 512. It is read when finalizing each response; unset leaves the existing per-tool budgets in place. This is a text-content limit, not a limit on the surrounding JSON-RPC envelope. If useful content cannot fit, Xray returns a compact error instead of claiming a complete result.

With `includeBody=true`, `containsLine` centers the available body window on the requested line. Explicit `bodyLineStart`/`bodyLineEnd` ranges take precedence. `bodyStartLine` and `bodyEndLine` describe the actual delivered fragment; `bodyAnchorVisible` reports whether it contains the anchor. `bodyComplete` covers the whole definition (and requested doc comments), while `bodyRangeComplete` covers only the requested range.

Partial bodies provide `bodyContinuation.beforeArgs` and/or `nextArgs` for `xray_definitions`. Pass these argument objects unchanged. They target one exact source file and definition range through `bodyTarget`, with a SHA-256 hash of the decoded file snapshot used for the read. A changed source or missing/ambiguous definition requires a fresh lookup. Cross-tool filters that cannot be represented by `xray_definitions` produce `bodyContinuationUnavailable` instead of a broader query.

Paginated lists provide `resultStatus.page.nextArgs`, including the original query and the final continuation token after byte fitting. Use it with the same tool and stop when it is absent. Unknown argument names must be corrected before Xray can generate ready-to-run continuation arguments.

Definition discovery can return up to three `recommendedNextQueries` with `operation: "readSelectedDefinition"`. Each reads one selected file/name/kind/parent using `exactNameOnly=true`, `autoCorrect=false`, and a snapshot-bound `bodyTarget`. Applicable filters, ranges, enrichment, and body budgets are retained; the original discovery query is unchanged. Ambiguous identities or unavailable snapshots do not produce an exact read. Body continuation takes priority over optional hints.

Cross-tool navigation uses `summary.nextStepQuery` when the selected target and original restrictions can be represented. Otherwise, the hint explains why no equivalent query was generated. Search alternatives, including punctuation cleanup, token intersection, and literal source-line matching, carry `semanticsChanged: true` outside `args`; they are not equivalent rewrites. Execute only the nested `args` object with its named tool. All hints count toward the final transport limit and may be omitted to preserve result delivery.

## Architecture overview

The engine uses three independent index types and a git history cache:

| Index | File | Created by | Searched by | Stores |
|---|---|---|---|---|
| File name | `.file-list` | `xray index` | `xray fast` | File paths, sizes, timestamps |
| Content | `.word-search` | `xray content-index` | `xray grep` | Token → (file, line numbers) map |
| Definitions | `.code-structure` | `xray def-index` | `xray_definitions` / `xray_callers` | AST-extracted classes, methods, call sites |
| Git history | `.git-history` | Background (auto) | `xray_git_history` / `xray_git_diff` / `xray_git_authors` / `xray_git_activity` / `xray_git_blame` / `xray_branch_status` | Commit metadata, file-to-commit mapping, branch status |

Indexes live in `%LOCALAPPDATA%\xray\`. Content search is language-agnostic. Definitions are language-specific: C#, TypeScript/TSX, and Rust through tree-sitter; SQL through a built-in regex parser; XML / `.csproj` / `.config` / `.props` / `.targets` / `.resx` / `.nuspec` / `.appxmanifest` / `.vsixmanifest` / `.manifestxml` parsed on-demand through tree-sitter (not bulk-indexed). The git history cache builds in the background whenever a `.git` directory is present. See [Architecture](docs/architecture.md) for the full picture.

Caller tree verification details (DI resolution, type inference, false-positive filtering) and Angular template metadata are covered in [Architecture](docs/architecture.md) too.

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
| [tree-sitter](https://crates.io/crates/tree-sitter) | Incremental parsing for code definition extraction (C#, TypeScript/TSX, Rust) |
| [lz4_flex](https://crates.io/crates/lz4_flex) | LZ4 frame compression for index files on disk |
| [mimalloc](https://crates.io/crates/mimalloc) | Memory allocator |
| [thiserror](https://crates.io/crates/thiserror) | Error type definitions |
| [tracing](https://crates.io/crates/tracing) + [tracing-subscriber](https://crates.io/crates/tracing-subscriber) | Structured diagnostic logging |
| [ctrlc](https://crates.io/crates/ctrlc) | Graceful shutdown signal handling |
| [criterion](https://crates.io/crates/criterion) | Statistical benchmarking (dev) |
| [proptest](https://crates.io/crates/proptest) | Property-based testing (dev) |

## Testing

```bash
# Run all unit tests (~3000+; cargo test --list reports 2,982 in xray bin + 108 in lib)
cargo test

# Run benchmarks
cargo bench

# Run E2E tests (~35 CLI + MCP tests in e2e-test.ps1; the full E2E spec catalog in docs/e2e/ is larger)
pwsh -File e2e-test.ps1
```

Test files are split by language module to stay maintainable. See [Architecture](docs/architecture.md) for the module layout. Test categories:

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

## Author

Sergey Pustynsky

## License

Licensed under either of:

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)
