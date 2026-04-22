# Infrastructure Tests

Tests for MCP server protocol, async startup, graceful shutdown, LZ4 compression, memory management, tool routing, format versioning, and other cross-cutting concerns.

---

## MCP Server Protocol

### T25: `serve` — MCP server starts and responds to initialize

**Expected:**

- JSON-RPC response with `"serverInfo"` and `"capabilities"`
- Response includes `"tools"` capability

---

### T26: `serve` — MCP tools/list returns all tools

**Expected:**

- 15 tools: `xray_grep`, `xray_fast`, `xray_info`, `xray_reindex`, `xray_reindex_definitions`, `xray_definitions`, `xray_callers`, `xray_edit`, `xray_help`, `xray_git_history`, `xray_git_diff`, `xray_git_authors`, `xray_git_activity`, `xray_git_blame`, `xray_branch_status`
- Each tool has `name`, `description`, `inputSchema`

---

### T39: `serve` — MCP initialize includes `instructions` field

**Expected:**

- `result.instructions` mentions `xray_fast`, `xray_callers`, `includeBody`, `countOnly`
- Provides LLM-readable best practices

**Status:** ✅ Implemented

---

### T39a: `serve` — MCP initialize instructions adapt to `--ext` configuration

**Expected:**

- `--ext sql` → `"NEVER READ .sql FILES DIRECTLY"` (only sql)
- `--ext xml` (no parsers) → `"xray_definitions is not available"`
- `--ext cs,ts,sql` → all three in NEVER READ rule

**Unit tests:** `test_render_instructions_empty_extensions`, `test_initialize_def_extension_filtering`

**Status:** ✅ Implemented

---

### T39b: `serve` — MCP initialize instructions include named policy wrapper

**Expected:**

- `result.instructions` starts with `=== XRAY_POLICY ===`
- Ends with `================================`

---

### T39c: `serve` — Successful JSON tool responses include policy reminder and next-step hint

**Expected:**

- `summary.policyReminder` contains `XRAY_POLICY` and `Indexed extensions:`
- Selected tools include `summary.nextStepHint`
- Error responses do NOT get `policyReminder`

---

### T39d: `serve` — MCP initialize instructions contain intent-first tool-selection sections (INTENT → TOOL MAPPING, MANDATORY PRE-FLIGHT CHECK, COST REALITY)

**Goal:** Validate that the rendered `XRAY_POLICY` in MCP `initialize.result.instructions` contains the three positive-framing sections that shortcut intent-first models to xray tools before NEVER-rules.

**Expected:**

- `result.instructions` contains `INTENT -> TOOL MAPPING` section with at least these pairs:
  - `xray_grep showLines=true` (context-around-match intent)
  - `xray_definitions name='X' includeBody=true` (read source intent)
  - `containsLine=N` (stack-trace intent)
  - `xray_edit with multiple edits` (replace-in-files intent)
  - `xray_fast pattern='*' dir='<path>' dirsOnly=true` (list-dir intent)
- `result.instructions` contains `MANDATORY PRE-FLIGHT CHECK` with `Q1:`, `Q2:`, and the word `UNJUSTIFIED`
- `result.instructions` contains `COST REALITY` with `5x fewer tokens`, `24x fewer tokens`, and `2 built-in calls in a row`
- Section order: `INTENT -> TOOL MAPPING` appears BEFORE `MANDATORY PRE-FLIGHT CHECK`, which appears BEFORE `COST REALITY`, which appears BEFORE `NEVER READ` and `ANTI-PATTERNS` (positive triggers first, negative rules after)
- `STRATEGY RECIPES` block contains only the top-3 recipes (`[Architecture Exploration]`, `[Call Chain Investigation]`, `[Stack Trace / Bug Investigation]`) and NOT the remaining 4 (`[Code History Investigation]`, `[Code Health Scan]`, `[Code Review / Story Evaluation]`, `[Angular Component Hierarchy (TypeScript only)]`)
- `result.instructions` contains `call xray_help for the full catalog`

**Unit tests:** `test_instructions_has_intent_mapping`, `test_instructions_has_preflight_check`, `test_instructions_has_cost_reality`, `test_instructions_section_order`, `test_instructions_strategy_recipes_trimmed`

**Status:** ✅ Implemented (unit test coverage)

---

### T39e: `serve` — policyReminder in tool responses contains INTENT->TOOL oneliner

**Goal:** Every successful JSON MCP tool response embeds a compact `INTENT->TOOL:` oneliner inside `summary.policyReminder`, providing re-entrancy of tool-selection rules between tool calls (system-prompt rules may be "forgotten" as context grows).

**Expected:**

- `summary.policyReminder` contains the substring `INTENT->TOOL:` on ANY successful JSON tool response (`xray_grep`, `xray_definitions`, `xray_callers`, `xray_edit`, `xray_fast`, etc.)
- The oneliner lists at least these intent→tool pairs:
  - `context-around-match->xray_grep showLines`
  - `read-method-body->xray_definitions includeBody`
  - `stack-trace (file:line)->xray_definitions containsLine`
  - `replace-in-files->xray_edit`
  - `list-dir->xray_fast dirsOnly`
  - `find-callers->xray_callers`
- The oneliner is present regardless of whether `--ext` (indexed extensions) is configured
- Error responses still get `policyReminder` including the INTENT oneliner

**Unit tests:** `test_build_policy_reminder_has_intent_oneliner`, `test_build_policy_reminder_intent_oneliner_without_extensions`

**Status:** ✅ Implemented (unit test coverage)


### T39f: `serve` — MCP initialize instructions contain v3 read/search/edit symmetry (TERMS block, tool-name-agnostic edit rule, MISCONCEPTION ALERT, Q3 pre-flight, EXCEPTIONS)

**Goal:** Validate that the rendered `XRAY_POLICY` symmetrically enforces read/search/edit at the same severity, uses tool-name-agnostic formulations for edit, and provides explicit misconception-alert / exceptions / self-audit hooks for edit-tool drift (user story `todo_approved_2026-04-17_xray-edit-policy-symmetry.md`).

**Expected:**

- `result.instructions` contains a `=== TERMS ===` definitions block that:
  - defines `"xray tools"` and `"your built-in tools"`
  - explicitly notes that built-in tool names differ per LLM host
  - appears BEFORE the `CRITICALLY IMPORTANT` (CRITICAL OVERRIDE) section
- The edit rule is **tool-name-agnostic**:
  - headline reads `NEVER USE your built-in edit tools for EDITING existing text files. ALWAYS use xray_edit — regardless of file extension.`
  - explicitly says `xray_edit works on ALL text files, NOT only on indexed extensions`
  - explains `xray_edit operates on BYTES, not on AST` (or equivalent)
- The edit rule contains a `MISCONCEPTION ALERT` block that:
  - quotes the exact wrong pattern (`"this file is not indexed"`)
  - explicitly says `WRONG` and `has NO extension filter`
- The edit rule contains explicit `EXCEPTIONS`:
  - CREATING new files — built-in whole-file-write tool acceptable
  - FULL FILE REWRITE >200 lines — built-in whole-file-write tool acceptable
  - BINARY files / byte-exact preservation — built-in tool with justification
- `MANDATORY PRE-FLIGHT CHECK` has a `Q3 (justification)` question (not just Q1/Q2) that:
  - enumerates valid reasons (a)-(e)
  - explicitly marks `"habit"` / `"familiarity"` as `UNJUSTIFIED`
  - addresses READ, SEARCH, EDIT operations separately inside Q2
- The pre-flight block contains an `ENFORCEMENT` clause stating `omitting the <thinking> block before a built-in call is itself a violation`
- The pre-flight block contains a `SELF-AUDIT HOOK` for post-call recovery
- `COST REALITY` includes the 8-block example: `8 SEARCH/REPLACE blocks` vs `xray_edit(8 edits)` → `8x fewer round-trips`, `atomic rollback`, and an explicit note that `xray_edit does NOT care about --ext for editing`
- `ANTI-PATTERNS` contains the extension-based edit-tool entry: `NEVER choose a built-in edit tool based on file extension`
- All three operation rules (READ / SEARCH / EDIT) continue to use `NEVER` at symmetric severity:
  - READ: `NEVER READ .{ext} FILES DIRECTLY`
  - EDIT: `NEVER USE your built-in edit tools for EDITING`
  - SEARCH: `NEVER USE search_files`
- The edit rule appears at full strength even when `--ext` is empty (xray_edit is extension-agnostic)
- The `=== TERMS ===` block is present regardless of `--ext` configuration

**Unit tests:** `test_instructions_has_terms_block`, `test_instructions_terms_block_before_critical_override`, `test_instructions_edit_rule_is_tool_name_agnostic`, `test_instructions_edit_rule_has_misconception_alert`, `test_instructions_edit_rule_has_exceptions`, `test_instructions_preflight_has_q3_justification`, `test_instructions_preflight_q1_q2_q3_symmetric`, `test_instructions_anti_pattern_extension_based_edit`, `test_instructions_cost_reality_has_multiblock_example`, `test_instructions_symmetric_severity_across_operations`, `test_instructions_no_hardcoded_builtin_names_in_edit_rule`, `test_instructions_edit_rule_works_without_def_extensions`, `test_instructions_terms_block_always_present`

**Status:** ✅ Implemented (unit test coverage)

---


### T40a: `serve` — `xray_help` includes policy reminder but omits next-step hint

**Expected:**

- `summary.policyReminder` present
- `summary.nextStepHint` absent

---

## Tips / Help

### T40: `serve` — MCP xray_help returns best practices

**Expected:**

- `bestPractices` array (6 items)
- `performanceTiers` object
- `toolPriority` array

---

### T42: `tips` / `xray_help` — Strategy recipes present

**Expected:**

- "Architecture Exploration", "Call Chain Investigation", "Stack Trace / Bug Investigation" recipes
- `strategyRecipes` array with 3+ entries

---

### T42b: `tips` / `xray_help` — Query budget and multi-term tips

---

### T42c: `tips` / `xray_help` — Code Review strategy recipe

---

### T-DYNAMIC-HELP: `xray_help` dynamic language scope

**Expected:**

- `bestPractices` contains actual language names ("Rust") instead of generic text

**Covered by:** `test_tips_no_hardcoded_language_lists`

---

### T107: `xray_help` — Response structure validation

**Unit test:** `test_xray_help_response_structure`

---

### T108: `xray_info` — Response structure validation

**Unit test:** `test_xray_info_response_structure`

---

## Dynamic Tool Descriptions

### T-DYNAMIC-DESCS: Dynamic tool descriptions based on active extensions

**Expected:**

- `--ext rs` → descriptions contain "Rust", not "C#" or "TypeScript"
- `--ext xml` (no parsers) → descriptions contain "not available"
- `--ext cs,rs,sql` → descriptions contain "C# and Rust" + SQL note

**Unit tests:** `test_tool_definitions_rust_only`, `test_tool_definitions_empty_extensions`, `test_format_supported_languages_*` (12 tests)

**Status:** ✅ Implemented

---

## Token Budget

### T-TOKEN-BUDGET: Tool definitions stay within token budget

**Expected:**

- Total word count < 4,125 words (~5,500 tokens)
- `xray_help` has `parameterExamples`

**Unit tests:** `test_tool_definitions_token_budget`, `test_render_json_has_parameter_examples`

**Status:** ✅ Implemented

---

## LZ4 Compression

### T-LZ4: LZ4 index compression and backward compatibility

**Expected:**

- Index files start with `LZ4S` magic bytes
- stderr shows compression ratio log
- Legacy uncompressed files still loadable

**Unit tests:** `test_save_load_compressed_roundtrip`, `test_load_compressed_legacy_uncompressed`, `test_compressed_file_smaller_than_uncompressed`

---

## Async MCP Server Startup

### T-ASYNC-01: `xray_grep` returns "building" when content index not ready

**Expected:** `isError: true`, message: "being built in the background"

**Unit test:** `test_dispatch_grep_while_content_index_building`

---

### T-ASYNC-02: `xray_definitions` returns "building" when def index not ready

**Unit test:** `test_dispatch_definitions_while_def_index_building`

---

### T-ASYNC-03: `xray_callers` returns "building" when def index not ready

**Unit test:** `test_dispatch_callers_while_def_index_building`

---

### T-ASYNC-04: `xray_fast` returns "building" when content index not ready

**Unit test:** `test_dispatch_fast_while_content_index_building`

---

### T-ASYNC-05: `xray_reindex` blocked only by `content_building` flag (not `content_ready`)

After the workspace-switch fix, `xray_reindex` is NOT blocked by `content_ready=false` —
it can run even when no index is loaded (needed for workspace switch). Concurrent build
protection uses a separate `content_building` flag with `compare_exchange`.

**Unit tests:** `test_dispatch_reindex_not_blocked_when_content_not_ready`, `test_dispatch_reindex_blocked_when_content_building`, `test_dispatch_reindex_definitions_blocked_when_def_building`

---

### T-ASYNC-06: `xray_help` and `xray_info` work during index build

**Unit test:** `test_dispatch_help_works_while_index_building`

---

### T-ASYNC-08: Tools work normally after background build completes

---

### T-ASYNC-09: Pre-built index loads synchronously

---

## Graceful Shutdown

### T-SHUTDOWN: Save-on-shutdown — indexes persist after graceful server stop

**Expected:**

- stderr: `saving indexes before shutdown`, `Content index saved on shutdown`
- `.word-search` file has recent modification timestamp

**Unit test:** `test_watch_index_survives_save_load_roundtrip`

---

### T-CTRLC: Graceful shutdown on Ctrl+C (SIGTERM/SIGINT)

**Expected:**

- Server prints save message, exits with code 0
- No panic or error messages

**Note:** Manual test only.

---

## Serialization Safety

### T-F07-SERIALIZATION: MCP server handles serialization errors gracefully

**Expected:**

- Server does NOT panic on serialization failure
- Returns JSON-RPC `-32603` internal error

**Unit tests:** `test_serialize_response_error_returns_internal_error`, `test_serialize_tool_result_error_returns_internal_error`

---

## Memory Management

### T-DEBUG-LOG: `serve --debug-log` — Debug logging

**Expected:**

- stderr contains `[debug-log]` and `[memory]` lines
- File `%LOCALAPPDATA%/xray/<prefix>.debug.log` created

**Unit tests:** `test_enable_debug_log_creates_file`, `test_log_request_format`, `test_log_response_format`

---

### T-MEMORY-ESTIMATE: `xray_info` — Memory estimates in response

**Expected:**

- `memoryEstimate.contentIndex` with `invertedIndexMB`, `trigramTokensMB`, etc.
- `memoryEstimate.definitionIndex` with `definitionsMB`, `callSitesMB`, etc.
- `memoryEstimate.process` with `workingSetMB`, `peakWorkingSetMB`, `commitMB`

**Unit tests:** `test_estimate_content_index_memory_empty`, `test_estimate_content_index_memory_nonempty`

---

### T-MI-COLLECT: `mi_collect(true)` — Memory decommit after build+drop+reload

**Expected:**

- WS after `mi_collect` < WS after `drop`
- WS after `reload` < WS at `finished`

**Unit test:** `test_force_mimalloc_collect_does_not_panic`

---

### T-MI-COLLECT-REINDEX: Memory decommit after MCP reindex

**Expected:**

- `rebuildTimeMs` includes drop+reload overhead
- Working Set returns to near-baseline after reindex

---

## Format Versioning

### T-FORMAT-VERSION: Index format version validation

**Expected:**

- Old indexes (wrong `format_version`) rejected → auto-rebuild
- Newly built indexes set `format_version` to current constant

**Unit tests:** 7 tests in `index_tests.rs` and `storage_tests.rs`

---

### T-STALE-CACHE: Stale index cache skipped when extensions change

**Expected:**

- stderr: `Skipping — extensions mismatch`
- Server rebuilds with correct extensions

**Unit tests:** `test_find_def_index_skips_stale_extensions`, `test_find_content_index_skips_stale_extensions`

**Status:** ✅ Covered by 8 unit tests

---

## Watcher / Incremental Updates

### T-TOMBSTONE: Definition index tombstone compaction during `--watch`

**Expected:**

- `totalDefinitions` reflects active definitions only (not Vec length with tombstones)
- Auto-compaction when tombstone ratio exceeds 3×

**Unit tests:** `test_compact_removes_tombstones`, `test_compact_auto_triggers_at_threshold`

---

### T-RECONCILE: Watcher startup reconciliation — catches stale cache files

### T-WATCHER-DEBOUNCE: Watcher debounce starvation fix (2026-03-16)

**Scenario:** Watcher flushes within 3s even under continuous file changes

1. Start server with `--watch`
2. Create 100 files in rapid succession (interval < 500ms each)
3. Verify that the content index is updated within 3-5 seconds (not waiting for complete silence)

**Expected:** `MAX_ACCUMULATE` (3s) forces batch processing even when events arrive continuously.
**Regression:** Previously, continuous events prevented the debounce timeout from firing.


**Expected:**

- Added/modified/deleted files while server was offline are detected and fixed
- stderr: `Definition index reconciliation complete` with `added`, `modified`, `removed` counts
- Lock-free parsing (Phase 3 without lock, Phase 4 write lock <500ms)

**Unit tests:** `test_reconcile_adds_new_file`, `test_reconcile_removes_deleted_file`, `test_reconcile_detects_modified_file`

---

## Changes Not CLI-Testable (Covered by Unit Tests)

| Change | Unit Test | Description |
|--------|-----------|-------------|
| `.git/` filtering in watcher | `test_is_inside_git_dir` | Watcher skips `.git/` directory |
| `batch_purge_files` | `test_batch_purge_files_*` (3 tests) | Single-pass batch purge for git pull scenarios |
| `shrink_to_fit()` after `retain()` | (behavioral) | Release excess capacity |
| `sorted_intersect` | (behavioral) | Better cache locality in `callers.rs` |

---

## Chunked Build

### T-CHUNKED-BUILD: Chunked build for peak memory reduction

**Expected:**

- Files processed in macro-chunks of 4096
- Same results as non-chunked build
- `force_mimalloc_collect()` after each chunk

**Unit tests:** `test_chunked_def_build_multiple_files_correct_counts`, `test_chunked_def_build_single_vs_multi_thread_consistency`, `test_chunked_content_build_multiple_files_correct_file_ids`, `test_chunked_content_build_single_vs_multi_thread`

---

## Tool Routing

### T-ROUTING-01: Source code exploration → xray_definitions

**Expected tool:** `xray_definitions` with `includeBody=true`, NOT built-in file reading

---

### T-ROUTING-02: Call chain investigation → xray_callers

**Expected tool:** `xray_callers` with `class`, NOT `xray_grep`

---

### T-ROUTING-03: File editing → xray_edit

**Expected tool:** `xray_edit`, NOT `apply_diff`

---

### T-ROUTING-04: Content search → xray_grep

**Expected tool:** `xray_grep`, NOT built-in regex search

---

### T-ROUTING-05: Non-indexed file → xray_grep or built-in

**Expected:** `xray_grep` for content index files, built-in for others

---

### T-ROUTING-06: Instructions structure validation (automated)

**Unit tests:** `test_task_routing_*`, `test_routing_tool_names_exist_in_definitions`, `test_instructions_token_budget`

---

## Reindex Operations

### T85: `xray_reindex_definitions` — Successful reindex

**Expected:**

- `status: "ok"` with metrics

**Unit test:** `test_reindex_definitions_success`

---

### T86: `xray_reindex` — Invalid directory error

**Expected:**

- `isError: true` for non-existent directory

**Unit test:** `test_xray_reindex_invalid_directory`

---

### T-REINDEX-SECURITY: Invalid or outside directory

**Expected:**

- `isError: true` for directory outside `--dir` scope

---

### T-VAL-05: `xray_fast` — Empty pattern returns error

**Unit test:** `test_xray_fast_empty_pattern_returns_error`


---

## Workspace Switch (2026-03-16)

### T-WS-UNRESOLVED: Server started without `--dir`, CWD has no source files

**Scenario (manual — requires Cline/Roo):**
1. Configure MCP without `--dir`: `{ "command": "xray", "args": ["serve", "--ext", "rs", "--definitions"] }`
2. Start server from a CWD without `.rs` files (e.g., VS Code install dir)

**Expected:**
- stderr: `Workspace UNRESOLVED`
- `xray_definitions` returns `WORKSPACE_UNRESOLVED` error with hint
- `xray_reindex` is NOT blocked (can be called to fix workspace)
- `content_ready = false`, `def_ready = false`
- No indexes built, no watcher started, no git cache built

**Unit tests:** `test_determine_initial_binding_dot_without_sources`, `test_dispatch_reindex_not_blocked_when_content_not_ready`

---

### T-WS-ROOTS-LIST: Workspace switch via roots/list resets ready flags

**Scenario (manual — requires MCP client with roots support):**
1. Start server with `--dir .` (Unresolved)
2. Client responds to `roots/list` with project directory
3. LLM calls `xray_reindex`

**Expected:**
- After roots/list: `ws.status = Reindexing`, `content_ready = false`, `def_ready = false`
- `xray_definitions` returns `WORKSPACE_REINDEXING` error (not 0 results!)
- After `xray_reindex`: content + def indexes loaded, `ws.status = Resolved`
- Response includes `defIndexAction` (loaded_cache or background_build)

---

### T-WS-CROSS-LOAD: xray_reindex cross-loads definition index on workspace switch

**Scenario (manual):**
1. Server running on Project A with both indexes loaded
2. LLM calls `xray_reindex(dir=ProjectB)`

**Expected:**
- Content index loaded for Project B
- Definition index cross-loaded from cache (if available)
- If no def cache: background build started, `def_ready = false`
- Watcher restarted for new directory (if `--watch`)
- Git cache cleared and rebuilt in background
- File-list index invalidated

---

### T-WS-WATCHER-RESTART: Watcher generation-based restart

**Scenario (manual):**
1. Server running with `--watch` on Project A
2. LLM calls `xray_reindex(dir=ProjectB)`
3. Modify a file in Project B

**Expected:**
- Old watcher stops (generation mismatch detected at next timeout)
- New watcher starts for Project B
- File changes in Project B are tracked
- stderr: `Watcher generation changed, exiting` + `File watcher restarted for new workspace`

---

### T-WS-SEQUENTIAL: Multiple sequential workspace switches

**Scenario (manual):**
1. `xray_reindex(dir=A)` → workspace A
2. `xray_reindex(dir=B)` → workspace B
3. `xray_reindex(dir=C)` → workspace C

**Expected:**
- Each switch loads correct indexes
- Old watchers exit (generation counter increments)
- No resource leaks (old threads exit cleanly)

---

### T-AGE-HOURS-FRESHNESS: `ageHours` reflects data freshness after reconciliation and watcher updates

**Scenario:**
1. Start server with `--watch`, load cached index with old `created_at`
2. Modify a file → watcher incremental update fires
3. Call `xray_info` → check `ageHours`

**Expected:**
- After reconciliation with changes: `ageHours` < 1 minute
- After watcher incremental update: `ageHours` < debounce interval
- Reconciliation without changes: `ageHours` unchanged (no reset)
- Reconciliation uses `walk_start` (not `now()`) to avoid race condition with files modified during tokenization phase

**Unit tests:** `test_reconcile_adds_new_file` (created_at assertion), `test_reconcile_skips_unchanged_files` (created_at unchanged), `test_reconcile_nonblocking_adds_new_files` (created_at updated from 0), `test_reconcile_nonblocking_no_changes` (created_at stays 0), `test_process_batch_dirty_file` (created_at recent after batch)

**E2E tests:** `T-RECONCILE` (watcher-startup-reconciliation), `T-BATCH-WATCHER` (batch-watcher-multi-file-update)

---

### T-WORKER-PANICS: worker_panics observability in xray_info (P0-1, 2026-04-18)

**What changed:** `ContentIndex` and `DefinitionIndex` now track `worker_panics: usize` — incremented when a worker thread panics during parallel index build. `xray_info` exposes `workerPanics` and `degraded: true` for any index with panics > 0.

**Manual test scenario:**
1. Start xray server normally
2. Call `xray_info` — verify `workerPanics` field is absent (or 0) and `degraded` is absent
3. (Inject panic via unit test) verify `workerPanics=N` and `degraded=true` appear in content index entry
4. Save/load index round-trip — verify `worker_panics` value is preserved

**Unit tests:** `test_worker_panics_preserved_in_serialization_roundtrip`, `test_worker_panics_default_is_zero`, `test_xray_info_worker_panics_shows_degraded`, `test_xray_info_no_degraded_when_no_panics`

---

### T-RENAME-INVALIDATION: File rename triggers file index rebuild (P0-2, 2026-04-18)

**What changed:** `should_invalidate_file_index()` helper in `watcher.rs` now correctly matches `Modify(Name(_))` (cross-platform rename event) in addition to `Create`, `Remove`, and `Modify(Any)`. Previously only `Modify(Any)` was matched, so rename events on Linux/inotify were silently dropped.

**Manual test scenario:**
1. Start xray server in watch mode: `xray serve --dir <dir> --watch`
2. Rename a file: `mv old.rs new.rs`
3. Call `xray_fast pattern='new'` — verify `new.rs` appears in results
4. Call `xray_fast pattern='old'` — verify `old.rs` no longer appears

**Unit tests:** `test_should_invalidate_file_index_create`, `test_should_invalidate_file_index_remove`, `test_should_invalidate_file_index_rename_triggers_rebuild`, `test_should_invalidate_file_index_modify_any`, `test_should_invalidate_file_index_data_change_does_not_invalidate`, `test_should_invalidate_file_index_access_does_not_invalidate`

---

### T-REINDEX-ROLLBACK: Workspace state rollback on failed reindex (P0-3, 2026-04-18)

**What changed:** `handle_xray_reindex_inner()` now calls `rollback_workspace_state()` on both error paths (first build failure and reload→rebuild failure). Previously a failed reindex left the server bound to the new (invalid) directory with a corrupt index.

**Manual test scenario:**
1. Start xray server bound to `dir_A`
2. Call `xray_reindex dir=<nonexistent_path>` — expect error response
3. Verify server is still bound to `dir_A` (call `xray_info` and check `directory` field)
4. Verify subsequent `xray_grep` still returns results from `dir_A`

**Unit tests:** `test_xray_reindex_invalid_directory`

---

### T-CLIPPY-GATE: CI clippy gate (P0-4, 2026-04-18)

**What changed:** `.github/workflows/clippy.yml` added — runs `cargo clippy --workspace -- -D warnings` on every push and PR. Also fixed all existing clippy warnings: empty doc comment lines, unnecessary_unwrap, explicit_counter_loop, manual_unwrap_or_default, too_many_arguments.

**Verification:** Push any branch — GitHub Actions runs clippy job. Any new warning fails the build.

**Unit tests:** N/A (CI gate)

---

### T-SYMLINK-SUBDIR: Symlinked subdirectory operations across MCP path checks (2026-04-20)

**What changed:** Five MCP-tool path checks (`classify_for_sync_reindex` in `edit.rs`, `dir_is_outside` in `fast.rs`, `validate_search_dir` + `resolve_dir_to_absolute` in `utils.rs`, `subdir_entry_filter` in `fast.rs`) previously called `std::fs::canonicalize()` for path comparison. Since the indexer uses logical paths via `WalkBuilder::follow_links(true)`, canonicalize-based checks falsely rejected symlinked subdirectories (the canonical real path lies outside the workspace root). All five sites now use logical-first comparison via the new `code_xray::is_path_within` helper (with canonical fallback only for 8.3 short names and `..` traversal protection). `subdir_entry_filter` builds its prefix from logical `clean_path`, with a canonical-equivalence fallback only for the boolean "is dir ≡ root?" decision.

**Manual test scenario:**
1. Create a symlinked subdirectory under the workspace root:
   ```powershell
   # As Administrator (or with developer mode enabled)
   New-Item -ItemType SymbolicLink -Path C:\Repos\Xray\docs\personal -Target D:\Personal\xray-notes
   ```
2. Add a `.md` file inside the symlink target.
3. With xray serving `C:\Repos\Xray`, run from an MCP client:
   - `xray_edit path='docs/personal/note.md' operations=[...]` — must succeed (no `skippedReason: outsideServerDir`); response includes `contentIndexUpdated: true`.
   - `xray_fast pattern='*' dir='docs/personal'` — must return the file (no `dir_is_outside`).
   - `xray_grep terms='...' dir='docs/personal'` — must search the symlinked tree (no `directory is outside workspace`).

**Expected:** All three calls succeed against the symlinked subdir, mirroring behavior on regular subdirectories. The `xray_edit` real-write path also performs sync reindex (response includes `contentIndexUpdated: true`).

**Unit tests:** `test_is_path_within_logical_match`, `test_is_path_within_through_symlink`, `test_is_path_within_traversal_protection`, `test_classify_for_sync_reindex_through_symlinked_subdir`, `test_dir_is_outside_through_symlinked_subdir`, `test_validate_search_dir_through_symlinked_subdir`, `test_resolve_dir_to_absolute_through_symlinked_subdir`, `test_xray_fast_subdir_filter_through_symlinked_subdir` (9 total, gated on `#[cfg(windows)]` where they create real symlinks via `std::os::windows::fs::symlink_dir`).

---

