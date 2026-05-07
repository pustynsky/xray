# E2E Test Plan — Xray Search Engine

## Overview

This directory contains end-to-end tests for the `xray` binary. These tests exercise
real CLI commands against a real directory to verify the full pipeline: indexing, searching,
output format, and all feature flags.

**Run these tests after every major refactoring, before merging PRs, and after dependency upgrades.**

> **Note:** MCP `xray_grep` defaults to `substring: true` since v0.2. Tests that expect exact-token behavior must pass `substring: false` explicitly.

## Test Plan Files

| File | Scope | Key Test IDs |
|------|-------|-------------|
| [cli-tests.md](cli-tests.md) | CLI commands: fast, grep, index, info, cleanup, def-index | T01–T24, T61–T64 |
| [mcp-grep-tests.md](mcp-grep-tests.md) | MCP `xray_grep`: substring, phrase, truncation, auto-switch | T27, T33–T42, T65–T68 |
| [mcp-definitions-tests.md](mcp-definitions-tests.md) | MCP `xray_definitions`: body, hints, auto-correct, code stats, ranking | T28, T69–T78, T-AS*, T-RANK*, T-CODESTATS* |
| [mcp-callers-tests.md](mcp-callers-tests.md) | MCP `xray_callers`: call trees, DI, overloads, type inference | T29–T31, T53–T59, T83–T84, T-FIX3* |
| [mcp-fast-edit-tests.md](mcp-fast-edit-tests.md) | MCP `xray_fast`, `xray_edit` | T79–T82, T-EDIT* |
| [git-tests.md](git-tests.md) | Git tools, cache, blame, branch status | T-GIT*, T-CACHE*, T-BRANCH*, T70 |
| [language-tests.md](language-tests.md) | SQL, TypeScript, Angular parser-specific tests | T-SQL*, T44–T51, T-ANGULAR*, T-PARSER* |
| [infrastructure-tests.md](infrastructure-tests.md) | Server protocol, async startup, shutdown, compression, memory, routing | T25–T26, T39–T40, T-ASYNC*, T-LZ4, T-SHUTDOWN* |

## Configuration

| Variable   | Default              | Description                                                    |
| ---------- | -------------------- | -------------------------------------------------------------- |
| `TEST_DIR` | `.` (workspace root) | Directory to index and search                                  |
| `TEST_EXT` | `rs`                 | File extension to index                                        |
| `BINARY`   | `cargo run --`       | Path to the binary (use `./target/release/xray` for release)   |

To run against a different directory:

```powershell
$env:TEST_DIR = "C:\Projects\MyApp"
$env:TEST_EXT = "cs"
```

## Prerequisites

```powershell
# Build the binary
cargo build

# Ensure unit tests pass first
cargo test
```

## Automation Script

The automated E2E tests are in [`e2e-test.ps1`](../../e2e-test.ps1) at the workspace root.

**Usage:**

```powershell
# Default (current workspace, .rs files)
./e2e-test.ps1

# Custom directory
./e2e-test.ps1 -TestDir "C:\Projects\MyApp" -TestExt "cs"

# With release binary
./e2e-test.ps1 -Binary "./target/release/xray"
```

## Test Parallelization

The E2E test script uses **`Start-Job`** to run independent MCP tests in parallel, reducing total execution time substantially compared to a fully sequential run.

### Test Classification

The categories below describe which kinds of tests can run concurrently — the
full, exhaustive list lives only in [`e2e-test.ps1`](../../e2e-test.ps1)
(currently ~50 parallel `Start-Job` blocks). Use the IDs as examples, not as
a complete inventory.

| Group | Examples | Parallelizable | Reason |
|-------|----------|---------------|--------|
| **Sequential CLI** | T01-T24, T42/T42b, T49, T54, T61-T64, T65 (fast), T76, T80, T82, T-RESPECT-GIT-EXCLUDE | ❌ No | Share index files in `%LOCALAPPDATA%/xray/` for the current directory |
| **Sequential state** | T-EXT-CHECK, T-DEF-AUDIT, T-SHUTDOWN | ❌ No | T-EXT-CHECK depends on T20; T-SHUTDOWN modifies global state |
| **MCP callers (parallel)** | T-FIX3-EXPR-BODY, T-FIX3-VERIFY, T-FIX3-LAMBDA, T-OVERLOAD-DEDUP-UP, T-SAME-NAME-IFACE, T-MULTI-METHOD, T-CALLERS-PER-LEVEL-TRUNC, T-CLASS-ARRAY-REJECT, T-ANGULAR | ✅ Yes | Each creates an isolated temp directory with its own indexes |
| **MCP definitions / hints (parallel)** | T-SORTBY-NO-AUTOSUMMARY, T-HINT-F-FUZZY, T-SQL | ✅ Yes | Read-only queries against fixtures or temp directories |
| **MCP edit (parallel)** | T-EDIT, T-EDIT-CREATE, T-EDIT-FLEX-GATE, T-EDIT-INSERT-APPEND-HINT, T-EDIT-LINE-ENDING, T-EDIT-MULTI, T-EDIT-NO-SILENT-TRAILING-WS | ✅ Yes | Each test edits inside its own temp scratch directory |
| **Sync-reindex (parallel)** | T-SYNC-DEFS, T-SYNC-DRYRUN, T-SYNC-EXT-NOT-INDEXED, T-SYNC-FAST, T-SYNC-GREP, T-SYNC-MULTI, T-SYNC-NARROW-GREP, T-SYNC-OUTSIDE-DIR, T-SYNC-RECONCILE-PRESERVED | ✅ Yes | Each spins up its own server in a temp directory |
| **Watcher (parallel)** | T-RECONCILE, T-CHECKPOINT-AFTER-RECONCILE, T-BATCH-WATCHER | ✅ Yes | Isolated temp directory per test |
| **Grep / args (parallel)** | T-ARGS-ALIAS-WARN, T-ARGS-STRICT-ERROR, T-LINE-REGEX-MD | ✅ Yes | Read-only or temp-scoped |
| **Info / serve-help (parallel)** | T-INFO-NO-DEGRADED, T-INFO-NO-STALE-FILES-AFTER-REMOVAL, T-SEARCH-INFO-MCP, T-SERVE-HELP-TOOLS, T-RESCAN-CLI-FLAGS, T-RESCAN-INFO-COUNTERS | ✅ Yes | Read-only, no shared state |
| **Fast (parallel, scoped)** | T-FAST-OUTSIDE-DIR, T-FAST-SUBDIR | ✅ Yes | Each operates against an isolated temp directory |
| **Git MCP (parallel)** | T-BRANCH-STATUS, T-GIT-FILE-NOT-FOUND, T-GIT-INCLUDE-DELETED, T-GIT-NOCACHE, T-GIT-TOTALCOMMITS, T-GIT-CACHE | ✅ Yes | Read-only queries against the current repo |
| **MCP instructions / policy (parallel)** | T-INTENT-MAPPING, T-POLICY-REMINDER | ✅ Yes | Read-only assertions on response payload |

### Implementation

- **~50 parallel test blocks** launched via `Start-Job` (PowerShell 5.1+); the
  exact count grows with every new isolated scenario — see
  `$testBlocks += { ... }` in [`e2e-test.ps1`](../../e2e-test.ps1).
- Each job receives: absolute binary path, absolute project directory, file extension
- Each job returns: `@{ Name; Passed; Output }` hashtable
- **120-second timeout** for the whole batch (individual tests typically complete in 3-5 seconds)
- Binary path resolved to absolute before job launch (jobs run in different working directory)
- Git tests use absolute repo path (not `"."`) to avoid working directory issues in jobs

### Estimated Speedup

Rough order-of-magnitude only — actual numbers vary with CPU thread count
and which tests are part of the parallel batch in the current revision.
With the parallel batch hovering around ~50 isolated MCP tests:

| Metric | Sequential equivalent | Parallel batch |
|--------|----------------------|----------------|
| ~50 isolated MCP tests × ~3-5 s each | **~3 min** | **~30-60 s** |
| Total E2E (sequential CLI + parallel batch) | **~5 min** | **~1.5-2 min** |

If the batch starts taking noticeably longer than that, the 120-second
batch timeout in `e2e-test.ps1` is the first thing to revisit.

## When to Run

- ✅ After every major refactoring or structural change
- ✅ After dependency upgrades (`cargo update`)
- ✅ Before creating a PR
- ✅ After merging a large PR
- ✅ When switching Rust toolchain versions