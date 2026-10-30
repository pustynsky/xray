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

📖 **More:** [Use cases & LLM workflows](docs/use-cases.md), including a case study where we reverse-engineered a 3,800-line system in 5 minutes.

## Quick start

**Install** (Windows x64, PowerShell 5.1+). The [setup script](scripts/setup-xray.ps1) downloads the latest `xray.exe`, detects your project's file extensions, and registers Xray for VS Code Copilot Chat and GitHub Copilot CLI:

```powershell
& ([scriptblock]::Create((Invoke-WebRequest 'https://raw.githubusercontent.com/pustynsky/xray/main/scripts/setup-xray.ps1' -UseBasicParsing).Content)) -UserMcp
```

| What you need | Command |
|---|---|
| One repository | `setup-xray.ps1 -RepoPath <path> -UserMcp` (add `-MigrateLegacy` to migrate old workspace registrations) |
| Update only the executable | `setup-xray.ps1 -UpdateOnly` |
| Existing Xray setups under a directory | [`setup-xray-repos.ps1`](scripts/setup-xray-repos.ps1) `-Root <path> -DryRun`, then without `-DryRun` |
| All repositories under a directory | `setup-xray-repos.ps1 -Root <path> -IncludeNewRepos -DryRun`, then without `-DryRun` |

The [Installation guide](docs/installation.md) covers CMD and download-then-run variants, what the script changes, personal vs. workspace MCP modes, bulk setup, updates, legacy migration, and manual configuration for Copilot Chat, Roo Code, and Cline. Pre-built binaries are on the [releases page](https://github.com/pustynsky/xray/releases/latest); to build from source (Rust 1.91+, any OS) see the [Development guide](docs/development.md).

**Make your agent prefer Xray.** Connecting the MCP server makes Xray available but does not guarantee the agent picks it over built-in search. For a quick trial, tell your agent:

> Use Xray first for code discovery in this workspace: file lookup, content
> search, symbol exploration, source reading, and caller/callee navigation.
> If Xray is unavailable or cannot cover the operation, briefly explain why
> before using an alternative. Keep existing editing, build, and test workflows.

To persist this across sessions, follow the [Xray-first setup](docs/installation.md#make-xray-preferred-for-code-discovery).

**Use from the command line or as an MCP server:**

```bash
xray content-index -d C:\Projects -e cs        # build content index for C# files
xray grep "HttpClient" -d C:\Projects -e cs     # TF-IDF-ranked token search
xray fast "UserService" -d C:\Projects -e cs    # instant file-name search
xray serve --dir C:\Projects --ext cs --watch --definitions   # MCP server
```

See the [CLI reference](docs/cli-reference.md) and the [MCP server guide](docs/mcp-guide.md) for all commands, tools, and response fields.

## Features

- **Fast indexes**: parallel filesystem walk that respects `.gitignore`; instant file-name index; language-agnostic inverted content index with TF-IDF ranking, regex, and trigram-backed substring search (~0.07ms vs ~44ms for regex). LZ4-compressed on disk.
- **Code definitions**: tree-sitter AST parsing for C#, TypeScript/TSX, and Rust; regex-based SQL (procedures, tables, views, functions, types, indexes, columns, call sites); on-demand XML / `.csproj` / `.config` / `.props` / `.resx` and similar. Angular components carry template metadata. Symbol results rank exact > prefix > contains.
- **DI-aware call trees**: `xray_callers` resolves callers through injected interfaces, I-prefix conventions, base types, DI field names, and local type inference. Works across MEDI / Autofac / Lamar / SimpleInjector / DryIoc / Ninject / MEF without parsing container registrations. See [DI support](docs/di-support.md).
- **Code complexity metrics**: cyclomatic and cognitive complexity, nesting depth, parameter/return-or-throw/call/lambda counts. Sort, filter by thresholds, or match an exact `paramCount`.
- **Git history**: background-built in-memory cache for sub-millisecond history, authors, activity, and blame queries, plus diffs and branch status.
- **MCP server for AI agents**: works with Copilot Chat, Copilot CLI, Roo Code, Cline, and any MCP client. Tool-selection guidance on initialization, per-response reminders, response size limits, and ready-to-run continuation arguments.
- **Safe edits**: `xray_edit` applies atomic line-range or text-match edits and reindexes synchronously, so follow-up searches see the change immediately.
- **Always current**: file watcher updates indexes incrementally (<1s per file); Ctrl+C saves indexes before exit; responses warn when you're on a non-main branch.

## How it works

| Index | File | Searched by | Stores |
|---|---|---|---|
| File name | `.file-list` | `xray fast` / `xray_fast` | File paths, sizes, timestamps |
| Content | `.word-search` | `xray grep` / `xray_grep` | Token → (file, line numbers) map |
| Definitions | `.code-structure` | `xray_definitions` / `xray_callers` | AST-extracted classes, methods, call sites |
| Git history | `.git-history` | `xray_git_*` / `xray_branch_status` | Commit metadata, file-to-commit mapping |

Indexes live in `%LOCALAPPDATA%\xray\`. Content search works on any text file; definitions need a supported parser. See [Architecture](docs/architecture.md) for components, data flow, and caller-tree verification.

## Documentation

| Document | Description |
|---|---|
| [Installation guide](docs/installation.md) | Setup script options, personal MCP profiles, bulk setup, updates, manual client configuration |
| [Use cases & LLM workflows](docs/use-cases.md) | Scenarios, ready-made LLM tool chains, case studies |
| [CLI reference](docs/cli-reference.md) | All commands with examples and options |
| [MCP server guide](docs/mcp-guide.md) | Tools API, parameters, response fields, JSON-RPC examples |
| [Architecture](docs/architecture.md) | System overview, component design, data-flow diagrams |
| [DI support](docs/di-support.md) | What's resolved automatically across DI containers, known gaps, and recipes |
| [Storage model](docs/storage.md) | Index formats, serialization, staleness, incremental updates |
| [Concurrency](docs/concurrency.md) | Thread model, lock strategy, watcher design |
| [Trade-offs](docs/tradeoffs.md) | Design decisions and the alternatives we rejected |
| [Benchmarks](docs/benchmarks.md) | Performance data, scaling estimates, industry comparison |
| [Development guide](docs/development.md) | Build from source, Cargo feature flags, tests, dependencies |
| [E2E test plan](docs/e2e/README.md) | End-to-end test cases (CLI + MCP), split by tool |
| [Changelog](CHANGELOG.md) | Changes grouped by category (features, fixes, performance) |

## Author

Sergey Pustynsky

## License

Licensed under either of:

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)
