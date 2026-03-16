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

- 16 tools: `xray_grep`, `xray_find`, `xray_fast`, `xray_info`, `xray_reindex`, `xray_reindex_definitions`, `xray_definitions`, `xray_callers`, `xray_edit`, `xray_help`, `xray_git_history`, `xray_git_diff`, `xray_git_authors`, `xray_git_activity`, `xray_git_blame`, `xray_branch_status`
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

### T-ASYNC-05: `xray_reindex` returns "already building" during background build

**Unit test:** `test_dispatch_reindex_while_content_index_building`

---

### T-ASYNC-06: `xray_help` and `xray_info` work during index build

**Unit test:** `test_dispatch_help_works_while_index_building`

---

### T-ASYNC-07: `xray_find` works during index build

**Unit test:** `test_dispatch_find_works_while_index_building`

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
