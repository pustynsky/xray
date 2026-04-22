> **Status: HISTORICAL DESIGN DOC (pre-implementation, 2026-03)** — kept for context. The shipped implementation may differ; consult the source code in `src/git/` and `CHANGELOG.md` for current behavior.


# Design Document: Git History in Search MCP Server

## 1. Context and Motivation

### Problem

An LLM agent working with a codebase through the search-index MCP server has no access to file change history. To find out "who changed this file?", "what changed in the last week?" or "show me the diff of this file" — the agent is forced to run `git log` / `git diff` through the terminal, parse text output, and lose context.

### Solution

4 new MCP tools directly in the search-index server that query git history **on-demand** via git CLI (`std::process::Command` → `git log` / `git diff`). Always enabled — no flags needed.

### Approach Evolution

| Version        | Approach                                | Result                                                                                 |
| -------------- | --------------------------------------- | -------------------------------------------------------------------------------------- |
| v1 (prototype) | libgit2 via `git2` crate                | Works, but 16-120× slower than git CLI due to lack of bloom filters for path filtering |
| **v2 (final)** | **Git CLI via `std::process::Command`** | **0.2-2.5 sec instead of 25-40 sec**                                                   |

**Why not libgit2:** Git CLI uses commit-graph with bloom filters for path-limited log, allowing O(1) determination of "this commit did NOT touch path X" and skipping it without expensive tree diff. libgit2 does not support bloom filters and must do a full `diff_tree_to_tree` on every commit.

---

## 2. Architecture

### 2.1 Data Flow

```
MCP Client (LLM)
    │
    ▼  JSON-RPC via stdio
search-index MCP Server
    │
    ├── Content Index Tools (xray_grep, xray_fast, ...)
    ├── Definition Index Tools (xray_definitions, xray_callers)
    └── Git History Tools (NEW — 4 tools)
            │
            ▼  std::process::Command
        git CLI (git log / git diff)
            │
            ▼  commit-graph + bloom filters
        .git/ on disk
```

### 2.2 Key Properties

| Property   | Value                                       |
| ---------- | ------------------------------------------- |
| Dependency | `git` in PATH (present on all dev machines) |
| Indexing   | Not needed — git CLI reads .git/ directly   |
| Caching    | Not needed — OS page cache + commit-graph   |
| Readiness  | Available immediately (no readiness check)  |
| Opt-in/out | Always enabled, no flags                    |

---

## 3. MCP Tools

### 3.1 `xray_git_history` — File Commit History

**Purpose:** List of commits that touched a file.

| Parameter    | Type   | Required | Description                        |
| ------------ | ------ | -------- | ---------------------------------- |
| `repo`       | string | yes      | Path to git repository             |
| `file`       | string | yes      | File path (relative to repo root)  |
| `from`       | string | no       | Start date (YYYY-MM-DD, inclusive) |
| `to`         | string | no       | End date (YYYY-MM-DD, inclusive)   |
| `date`       | string | no       | Exact date (overrides from/to)     |
| `maxResults` | int    | no       | Max commits (default: 50)          |

**Implementation:** `git log --format=<fields> --follow -- <file>`

- `--follow` — tracks renames
- commit-graph + bloom filters accelerate path filtering

**Output:** JSON with `commits[]` and `summary`.

### 3.2 `xray_git_diff` — File Diff by Commits

Same as `xray_git_history`, but each commit additionally contains `patch` (added/removed lines).

**Implementation:**

1. `git log` for the commit list (fast)
2. `git diff <hash>^..<hash> -- <file>` for each commit (parallelizable in Phase 2)
3. Patches truncated to ~200 lines to protect LLM context

### 3.3 `xray_git_authors` — Who Changed a File the Most

Author ranking by commit count.

| Additional parameter | Description                         |
| -------------------- | ----------------------------------- |
| `top`                | Number of top authors (default: 10) |

**Implementation:** `git log --format=...` → aggregation in HashMap by author.

### 3.4 `xray_git_activity` — Repository-Wide Activity

Map of `file → commits` for all changed files in a period.

**Implementation:** `git log --format=... --name-only` → parse files after each commit.

---

## 4. Performance

### 4.1 Benchmarks (C:\Repos\MainProject, ~10K commits)

| Query                               | libgit2 (v1) | git CLI (v2) | Speedup  |
| ----------------------------------- | ------------ | ------------ | -------- |
| Directory.Build.props (107 commits) | 25.3 sec     | ~0.2 sec     | **120×** |
| SecurityAuditor.cs (2 commits)      | 40.7 sec     | ~2.5 sec     | **16×**  |
| Repo activity (218 commits, 3 days) | 10.0 sec     | ~2-3 sec     | **3-5×** |

### 4.2 Why Git CLI is Faster

```
git CLI:
  revwalk → commit-graph → bloom filter → "this commit did NOT touch path" → SKIP (O(1))

libgit2:
  revwalk → commit-graph → diff_tree_to_tree(parent, child, pathspec) → full tree comparison (O(tree_size))
```

Bloom filters allow skipping 99%+ of commits without expensive tree diff.

---

## 5. LLM Context Protection

### 5.1 Built-in Limiters

| Measure                     | Description                                                          |
| --------------------------- | -------------------------------------------------------------------- |
| `maxResults`                | Default 50 commits                                                   |
| Patch truncation            | ~200 lines per commit                                                |
| `truncate_large_response()` | Automatic truncation of all responses to `--max-response-kb` (16 KB) |
| `summary.hint`              | Hint for LLM to use from/to filters                                  |

### 5.2 Truncation Example

`xray_git_activity` returned 2.4 MB (736 files × commits) → auto-truncation compressed to 51 KB (5 files out of 736). `summary.responseTruncated = true`.

---

## 6. Error Handling

| Scenario             | Behavior                                                                  |
| -------------------- | ------------------------------------------------------------------------- |
| Not a git repository | `isError: true`, `"fatal: not a git repository"`                          |
| Git not in PATH      | `isError: true`, `"Failed to execute git. Is git installed and in PATH?"` |
| Non-existent file    | Empty `commits[]`, not an error                                           |
| Invalid date         | `isError: true`, `"Invalid date 'xxx': expected YYYY-MM-DD format"`       |

---

## 7. File Structure

```
src/
├── git/
│   ├── mod.rs          # Core: types + git CLI query functions
│   └── git_tests.rs    # 29 unit tests
├── mcp/
│   └── handlers/
│       ├── git.rs      # MCP handlers for 4 tools + tool definitions
│       └── mod.rs      # Registration in tool_definitions() + dispatch_tool()
```

### Changed Files

| File                      | Change                                  |
| ------------------------- | --------------------------------------- |
| `src/main.rs`             | `mod git;`                              |
| `src/mcp/handlers/mod.rs` | `mod git;`, tool registration, dispatch |
| `src/mcp/server.rs`       | Tool count 9 → 13                       |
| `src/cli/args.rs`         | Git tools in help text                  |
| `CHANGELOG.md`            | New feature entry                       |
| `docs/e2e-test-plan.md`   | 8 git test scenarios (T-GIT-01..08)     |

### Not needed (after switching to git CLI)

| Was (v1)                       | Removed in v2 |
| ------------------------------ | ------------- |
| `git2 = "0.20"` in Cargo.toml  | Removed       |
| `chrono = "0.4"` in Cargo.toml | Removed       |
| `advapi32` link in build.rs    | Removed       |
| ~2 MB libgit2 in binary        | Removed       |

---

## 8. Testing

### 8.1 Unit Tests (29 tests)

| Group           | Count | Description                            |
| --------------- | ----- | -------------------------------------- |
| Date validation | 6     | Date parsing, format validation        |
| next_day        | 5     | Date increment for --before            |
| Date filter     | 5     | from/to/date combinations              |
| File history    | 6     | Commits, maxResults, diff, date filter |
| Top authors     | 3     | Ranking, empty file, limit             |
| Repo activity   | 3     | Activity, empty range, bad repo        |
| Commit info     | 1     | All fields populated                   |

### 8.2 E2E Tests

8 scenarios (T-GIT-01..T-GIT-08) in `docs/e2e-test-plan.md`:

- File history with maxResults
- Diff with patches
- Top authors with limit
- Repo activity with date range
- Date filter (empty result)
- Missing parameter error
- Bad repo path error
- Tools always available (13 tools, no flags)

---

## 9. Future Improvements (Phase 2)

| Improvement           | Priority | Description                                                                                        |
| --------------------- | -------- | -------------------------------------------------------------------------------------------------- |
| Parallel diff queries | High     | `git diff` for N commits in parallel (rayon/thread::scope). Git reads .git/ via mmap without locks |
| Multiple files        | Medium   | `files: string[]` parameter for `xray_git_history` — parallel `git log` for each file            |
| git blame             | Medium   | New tool `xray_git_blame` — who wrote each line                                                  |
| Commit-graph cache    | Low      | Call `git commit-graph write` if commit-graph is missing (speeds up first run)                     |

---

## 10. Decisions Made During Development

### 10.1 libgit2 → git CLI

- **Question:** Use libgit2 (direct .git/ access) or git CLI?
- **Initial decision:** libgit2 (no CLI dependency)
- **Problem:** 16-120× slower due to lack of bloom filters
- **Final decision:** git CLI. Git is always present on dev machines. The performance difference is critical.

### 10.2 Opt-in vs Always-on

- **Options:** Cargo feature flag, runtime `--git` flag, always-on
- **Decision:** Always-on. Git tools consume no resources at startup (no indexing), don't slow down the server. If `--dir` is not a git repo — error comes only on tool call.

### 10.3 Context Protection

- **Question:** How to avoid flooding the LLM context window with 2.4 MB responses?
- **Decision:** Existing `truncate_large_response()` is automatically applied to all tools. `maxResults=50` by default. Patches truncated to 200 lines.
