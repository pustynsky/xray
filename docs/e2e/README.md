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

The E2E test script uses **`Start-Job`** to run independent MCP tests in parallel, reducing total execution time by ~50%.

### Test Classification

| Group | Tests | Parallelizable | Reason |
|-------|-------|---------------|--------|
| **Sequential CLI** | T01-T22, T24, T42/T42b, T49, T54, T61-T64, T65(fast), T76, T80, T82, T-RESPECT-GIT-EXCLUDE | ❌ No | Share index files in `%LOCALAPPDATA%/xray/` for current directory |
| **Sequential state** | T-EXT-CHECK, T-DEF-AUDIT, T-SHUTDOWN | ❌ No | T-EXT-CHECK depends on T20; T-SHUTDOWN modifies global state |
| **MCP callers** | T65-66, T67, T68, T69, T-FIX3-EXPR-BODY, T-FIX3-VERIFY, T-FIX3-LAMBDA, T-OVERLOAD-DEDUP-UP, T-SAME-NAME-IFACE, T-ANGULAR | ✅ Yes | Each creates isolated temp directory with own indexes |
| **Git MCP** | T-BRANCH-STATUS, T-GIT-FILE-NOT-FOUND, T-GIT-NOCACHE, T-GIT-TOTALCOMMITS, T-GIT-CACHE | ✅ Yes | Read-only queries against current repo |
| **Serve help** | T-SERVE-HELP-TOOLS | ✅ Yes | Read-only, no index state |

### Implementation

- **16 parallel tests** launched via `Start-Job` (PowerShell 5.1+)
- Each job receives: absolute binary path, absolute project directory, file extension
- Each job returns: `@{ Name; Passed; Output }` hashtable
- **120-second timeout** per batch (individual tests typically complete in 3-5 seconds)
- Binary path resolved to absolute before job launch (jobs run in different working directory)
- Git tests use absolute repo path (not `"."`) to avoid working directory issues in jobs

### Estimated Speedup

| Metric | Sequential | Parallel |
|--------|-----------|----------|
| MCP callers (9 tests × ~4s) | ~36s | ~5s |
| Git MCP (5 tests × ~3s) | ~15s | ~4s |
| Serve help (1 test) | ~1s | included |
| **Parallel batch total** | **~52s** | **~6s** |
| Total E2E (with sequential) | ~2 min | **~1 min** |

## When to Run

- ✅ After every major refactoring or structural change
- ✅ After dependency upgrades (`cargo update`)
- ✅ Before creating a PR
- ✅ After merging a large PR
- ✅ When switching Rust toolchain versions