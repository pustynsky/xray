# Bug Report: index freshness is ambiguous after file/branch changes (`xray_reindex` loads cache, `branchWarning` stays stale)

**Date:** 2026-04-28
**Author:** GitHub Copilot
**Area:** xray MCP / index freshness / branch diagnostics / reindex semantics
**Type:** Correctness + tooling observability bug
**Severity:** High
**Status:** Pending investigation

---

## TL;DR

During normal VS Code MCP usage, xray can present contradictory freshness signals after file or branch changes:

- `xray_info` / `xray_branch_status` can report the current Git branch as `main`.
- `xray_grep` / `xray_definitions` summaries can still emit `branchWarning` for an older branch captured at server startup.
- Manual `xray_reindex` can return `indexAction: "loaded_cache"`, meaning it may not rebuild from the live filesystem despite being documented/understood as a force rebuild.

This makes it hard for agents and users to know whether search/definition results reflect the current workspace or a stale cache. The immediate symptom is misleading diagnostics, but the deeper risk is stale content surviving when operators believe they forced a refresh.

---

## Observed Evidence

### 1. Live branch status and git-history say `main`

Observed in the same VS Code session:

```json
{
  "currentBranch": "main",
  "isMainBranch": true,
  "aheadOfMain": 0,
  "behindMain": 0
}
```

`xray_info` also reported a git-history index for `C:/Repos/Xray` with:

```json
{
  "type": "git-history",
  "branch": "main"
}
```

### 2. Search/definition summaries still warned about an old branch

Despite the above, `xray_grep` / `xray_definitions` summaries included:

```text
Index is built on branch 'feat/purge-perf-reverse-index', not on main/master. Results may differ from production.
```

That branch was no longer the checked-out branch.

### 3. `xray_reindex` may only load cache

A manual `xray_reindex` call returned:

```json
{
  "status": "ok",
  "indexAction": "loaded_cache"
}
```

That means the tool did not necessarily rebuild the content index from disk. For a command named and documented as reindex/force rebuild, this is surprising and can preserve stale state.

### 4. Watcher drift has occurred in this session

`xray_info` exposed watcher stats similar to:

```json
{
  "eventsTotal": 109253,
  "periodicRescanDriftEvents": 32,
  "periodicRescanEnabled": true
}
```

This indicates the periodic rescan has already detected missed notify events in this environment. That does not prove the current branch-warning bug, but it reinforces that freshness needs to be observable and reliable.

---

## Current Code Pointers

### Stale branch warning source

`branchWarning` is produced from `HandlerContext.current_branch`:

- `src/mcp/handlers/utils.rs` — `branch_warning(ctx)` reads `ctx.current_branch`.
- `src/mcp/handlers/utils.rs` — `inject_branch_warning(summary, ctx)` writes `summary.branchWarning`.
- `src/mcp/handlers/grep.rs` — `build_grep_base_summary(...)` calls `inject_branch_warning(...)`.

`current_branch` is captured once at server startup:

- `src/cli/serve.rs` — `git rev-parse --abbrev-ref HEAD` populates `current_branch`.
- `src/mcp/handlers/mod.rs` — `HandlerContext.current_branch` is documented as "detected at server startup".

There is no obvious update path when the user runs `git checkout`, `git pull`, or uses `xray_branch_status`.

### `xray_reindex` cache-load path

`xray_reindex` eventually calls `build_or_load_content_index(...)`:

- `src/mcp/handlers/mod.rs` — `handle_xray_reindex_inner(...)` calls `build_or_load_content_index(...)`.
- `src/mcp/handlers/mod.rs` — `build_or_load_content_index(...)` first tries `load_content_index(...)` and `find_content_index_for_dir(...)`.
- If either cache load succeeds, it returns `(idx, "loaded_cache")` without rebuilding.

This conflicts with the expectation that `xray_reindex` is the operator's escape hatch for stale indexes.

### Separate sync-edit path

`xray_edit` uses a different path:

- `src/mcp/watcher.rs` — `reindex_paths_sync(...)` updates the in-memory content/definition indexes for touched paths.
- `xray_edit` responses expose `contentIndexUpdated`, `defIndexUpdated`, `fileListInvalidated`, and `reindexElapsedMs`.

That path is intended to make edits immediately visible. The current bug report does not assert that this path is broken, but it should be included in regression coverage because users perceive all of these issues as "index not fresh after file changes".

---

## Why This Is A Bug

`xray_*` tools are used as source-of-truth workspace inspection tools by coding agents. If their own metadata says both "main" and "not main" in the same session, trust is damaged.

More importantly, if a user runs `xray_reindex` after suspecting stale search results, the command must not silently reuse the same stale disk cache. Otherwise the most natural recovery action fails while reporting success.

This can lead to:

- agents making decisions from stale search/definition results;
- repeated unnecessary manual `xray_reindex` calls that do not repair freshness;
- false belief that a file change has not landed;
- confusion between three different freshness layers: live Git status, content index, and startup branch metadata.

---

## Expected Behavior

### Branch diagnostics

After the repository branch changes, index-based tool summaries should not keep reporting a branch captured at server startup.

Acceptable designs:

1. Recompute branch lazily when emitting `branchWarning`.
2. Store branch in an updatable shared field and refresh it when `xray_branch_status` runs.
3. Update branch metadata on watcher events involving `.git/HEAD` / `.git/refs/*`.
4. Remove `branchWarning` from index summaries if its source is startup-only and can be stale.

The key requirement: a tool response must not simultaneously report current branch `main` elsewhere and warn that the index is on an old feature branch.

### Reindex semantics

`xray_reindex` should be a real rebuild by default for the current workspace. It should walk the live filesystem and replace the in-memory content index from current disk state.

If cache loading is retained, it should be explicit, for example:

- a separate `loadCache`/`useCache` flag;
- only used during workspace switch fast-paths;
- clearly documented and reported as not a force rebuild.

The default operator mental model should be: "I called reindex; stale content is gone if the live filesystem has the right content."

---

## Suggested Repro: Stale Branch Warning

### Preconditions

- Start `xray serve` while on a non-main branch, e.g. `feat/example`.
- Use the same long-running VS Code MCP server session.

### Steps

1. Call `xray_grep` with any valid query.
2. Observe `summary.branchWarning` mentioning `feat/example`.
3. In a terminal, run `git checkout main && git pull`.
4. Call `xray_branch_status`.
5. Call `xray_grep` or `xray_definitions` again.

### Actual

`xray_branch_status` reports `currentBranch: "main"`, but `xray_grep` / `xray_definitions` still include a `branchWarning` for `feat/example`.

### Expected

Once branch status is `main`, index-based tool summaries do not warn about the old branch, or they explicitly state that branch metadata is startup-only/stale.

---

## Suggested Repro: `xray_reindex` Reuses Stale Cache

### Preconditions

- Existing content index cache for a workspace.
- A tracked/indexed file whose content can be changed externally.
- Watcher disabled, missed, or intentionally bypassed for the test.

### Steps

1. Build or load a content index for the workspace.
2. Modify an indexed file externally so the disk content differs from the cached index.
3. Call `xray_grep` for a token that exists only in the new disk content and confirm it is missing.
4. Call `xray_reindex` with no special flags.
5. Inspect the response `indexAction`.
6. Repeat `xray_grep` for the new token.

### Actual Risk

If `indexAction == "loaded_cache"`, `xray_reindex` may reload the stale disk cache and the new token may still be missing.

### Expected

`xray_reindex` rebuilds from live disk and the new token becomes searchable.

---

## Suspected Root Causes

### Root Cause A: `current_branch` is static runtime context

`HandlerContext.current_branch` is captured at startup and then reused for every branch warning. This cannot stay correct across branch changes in a long-running VS Code session.

### Root Cause B: `xray_reindex` is actually build-or-load

`handle_xray_reindex_inner(...)` calls `build_or_load_content_index(...)`; this helper prioritizes cache loading. That is appropriate for startup or workspace-switch latency, but inappropriate for a user-invoked force-refresh command.

### Root Cause C: freshness concepts are conflated

The response surfaces several independent freshness dimensions without making their source clear:

- live git branch (`xray_branch_status`);
- startup branch metadata (`summary.branchWarning`);
- git-history cache branch (`xray_info` git-history index);
- content/definition index cache freshness;
- watcher/periodic-rescan freshness.

When they disagree, the user has no authoritative indication of which value is stale.

---

## Proposed Fix Plan

### Fix 1: Make branch warning live or remove it

Preferred: replace startup-only `current_branch` warning with a cheap live branch probe, possibly cached with a short TTL and invalidated on `.git` watcher events.

Alternative: update `ctx.current_branch` when `xray_branch_status` runs, and add tests showing `inject_branch_warning` reflects the update.

Minimum acceptable: stop emitting `branchWarning` when the source is known to be startup-only and potentially stale.

### Fix 2: Split force rebuild from cache load

Change `xray_reindex` current-workspace behavior to force rebuild. Keep cache-load behavior only for startup/workspace switch paths or behind an explicit parameter.

Potential shape:

- `xray_reindex` default: rebuild from disk, return `indexAction: "rebuilt"`.
- Optional future flag: `useCache: true` or `loadCache: true`, return `indexAction: "loaded_cache"`.
- Workspace switch cross-load can keep `loaded_cache` as an optimization, because that path is about fast binding, not explicit freshness repair.

### Fix 3: Add freshness diagnostics

Add enough metadata to help diagnose disagreement:

- `summary.branchSource`: `live`, `startup`, or omitted.
- `xray_info` content/definition entries: `createdAt` or `createdAtUnix`, not only rounded `ageHours`.
- `xray_reindex` response: explicit `rebuiltFromDisk: true/false` or a stronger `indexAction` enum.

---

## Acceptance Criteria

1. After `git checkout main`, `xray_branch_status` and `xray_grep`/`xray_definitions` no longer disagree about the current branch warning.
2. A test demonstrates that startup branch metadata cannot produce a stale `branchWarning` after branch refresh/update.
3. `xray_reindex` for the current workspace rebuilds from live disk by default and does not return `loaded_cache` unless an explicit cache-load mode is requested.
4. A regression test modifies an indexed file after a cached index exists, runs `xray_reindex`, and verifies the new token/definition is visible.
5. `xray_info` or `xray_reindex` exposes enough freshness metadata to diagnose whether a result came from a live rebuild or cache load.
6. Existing workspace-switch fast paths remain performant and still report when they loaded cache instead of rebuilding.

---

## Non-Goals

- Do not rewrite the watcher architecture in this bug fix.
- Do not remove periodic rescan.
- Do not change `xray_edit` sync reindex semantics unless tests prove that path is also stale.
- Do not merge this with the `lineRegexScan` telemetry story; that story only adds diagnostics for lineRegex scan phases.

---

## Priority

**High.**

This is not a direct data-loss bug, but it undermines the reliability of the MCP tools as an agent-facing source of truth. The current behavior also weakens the main recovery command (`xray_reindex`) exactly when users suspect stale search/definition results.

---

## Workaround

Until fixed:

1. Treat `summary.branchWarning` as suspect after any branch change in the same VS Code session.
2. Use `xray_branch_status` for live branch state, not `summary.branchWarning`.
3. Do not assume `xray_reindex` rebuilt from disk; inspect `indexAction`.
4. If freshness matters and `indexAction == "loaded_cache"`, restart the MCP server / reload VS Code window or force a rebuild through CLI if available.
5. For files modified via `xray_edit`, prefer verifying with a direct `xray_grep` query for a unique new token because `xray_edit` is expected to sync-update in-memory indexes.
