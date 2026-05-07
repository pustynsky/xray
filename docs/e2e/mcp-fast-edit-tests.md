# MCP `xray_fast`, `xray_edit` Tests

Tests for file name search (`xray_fast`) and file editing (`xray_edit`) MCP tools.

---

## xray_fast

### T79: `xray_fast` — `dirsOnly` and `filesOnly` filters

**Expected:**

- `dirsOnly: true` — all results are directories
- `filesOnly: true` — all results are files

**Unit test:** `test_xray_fast_dirs_only_and_files_only`

---

### T79a: `xray_fast` — `dirsOnly` + wildcard returns `fileCount` sorted by size

**Expected:**

- Each directory includes `fileCount` (integer ≥ 0)
- Sorted by `fileCount` descending (largest modules first)
- `fileCount` is included for all `dirsOnly` requests (wildcard and filtered patterns)

**Unit tests:** `test_xray_fast_dirsonly_wildcard_filecount`, `test_xray_fast_dirsonly_non_wildcard_has_filecount`, `test_xray_fast_multi_pattern_dirs_only_filecount`

**Status:** ✅ Implemented

---

### T-FAST-GLOB-RANKING: `xray_fast` — Glob pattern ranking uses literal prefix

**Expected:**
- `pattern: "Order*"` ranks `Order.cs` (exact match for literal prefix "Order") before `OrderProcessor.cs` (prefix match) before `OrderValidatorFactory.cs` (prefix match, longer stem)
- Non-glob patterns are unaffected (ranking uses original search terms)
- `pattern: "*Helper*"` produces no ranking terms (empty literal prefix) → falls back to length-based sort

**Unit tests:** `test_extract_glob_literal_no_glob`, `test_extract_glob_literal_star_suffix`, `test_extract_glob_literal_star_prefix`, `test_extract_glob_literal_question_mark`, `test_extract_glob_literal_mixed`, `test_xray_fast_glob_ranking_uses_literal_prefix`

**Status:** ✅ Implemented

---

### T79a-fix: `xray_fast` — `fileCount` correct when `dir` is a subdirectory (regression)

**Expected:**

- `fileCount > 0` for directories containing files (not always 0)
- Only files under specified `dir` are counted

**Unit tests:** `test_xray_fast_filecount_with_subdir`, `test_xray_fast_filecount_with_absolute_dir`

**Status:** ✅ Implemented (regression fix)

---

### T-FAST-SUBDIR: `xray_fast` — Subdirectory reuses parent index

**Expected:**

- No new `.file-list` index for subdirectory
- Results scoped to requested subdirectory
- `maxDepth` relative to subdirectory, not root

**Unit tests:** `test_xray_fast_subdir_reuses_parent_index`, `test_xray_fast_subdir_max_depth_relative_to_dir`

**Status:** ✅ Implemented

---

### T-FAST-DIRTY-FLAG: `xray_fast` — In-memory dirty-flag invalidation

**Expected:**

- First `xray_fast` call builds file index in memory (dirty=true by default)
- Second call uses cached index (~0ms, no rebuild)
- After file creation (watcher sets dirty flag): next call rebuilds and finds new files
- After file deletion (watcher sets dirty flag): next call rebuilds and removed files disappear
- `xray_reindex` invalidates file index cache (sets to None + dirty)
- Outside-server-dir requests use disk-cached indexes (not in-memory cache)

**Unit tests:** `test_xray_fast_dirty_flag_rebuild`, `test_xray_fast_dirty_flag_detects_deletion`, `test_xray_fast_invalidate_via_none`

**Live E2E verification:** Create files → search → delete → search (verified via MCP xray_fast calls)

**Status:** ✅ Implemented

---

### T-FAST-RELDIR: `xray_fast` — Relative dir parameter resolution

**Expected:**

- `dir: "src/services"` (relative) → resolves against server_dir, finds files in subdirectory
- `dir: "src/services"` + `pattern: "User"` → scoped search (only finds files in that subdir)
- Relative dir does NOT create orphan index files

**Unit tests:** `test_xray_fast_relative_dir_subdir_search`, `test_xray_fast_relative_dir_pattern_search`

**Status:** ✅ Implemented

---

### T-FAST-GLOB: `xray_fast` — Glob pattern auto-detection

**Expected:**

- `pattern: "Order*"` → auto-converts to regex `^Order.*$`, finds OrderProcessor.cs and OrderValidator.cs
- `pattern: "*Tracker*"` → finds InventoryTracker.cs
- `pattern: "Order?alidator*"` → `?` matches single char, finds OrderValidator.cs but NOT OrderProcessor.cs
- `pattern: "*.cs"` → finds all .cs files, excludes .txt
- `pattern: "Order"` (no glob chars) → unchanged substring behavior
- Glob detection does NOT interfere with `pattern: "*"` (wildcard-all)
- Glob detection does NOT interfere with `regex: true` (user-specified regex)

**Unit tests:** `test_xray_fast_glob_star_suffix`, `test_xray_fast_glob_star_prefix`, `test_xray_fast_glob_question_mark`, `test_xray_fast_glob_with_ext_filter`, `test_xray_fast_no_glob_unchanged_behavior`

**Status:** ✅ Implemented

---

### T-GREP-RELDIR: `xray_grep` — Relative dir parameter resolution

(Cross-cut from `mcp-grep-tests.md` — kept here next to `T-FAST-RELDIR` because both exercise the same relative-dir resolution path.)

**Expected:**

- `dir: "subA"` (relative) → resolves against server_dir, scopes grep to subdirectory
- Results contain only files from the specified subdirectory

**Unit test:** `test_grep_with_relative_subdir_filter`

**Status:** ✅ Implemented

---

### T79b: `xray_fast` — `dirsOnly` with `ext` filter (ext ignored)

**Expected:**

- `dirsOnly: true` + `ext: "cs"` returns directories (ext ignored)
- `summary.hint` contains `"ext filter ignored when dirsOnly=true"`

**Unit tests:** `test_xray_fast_dirs_only_ignores_ext_filter`

**Status:** ✅ Implemented

---

### T79c: `xray_fast` — `maxDepth` limits directory depth

**Expected:**

- `maxDepth=1` returns only immediate children
- No `maxDepth` → full recursion

**Unit test:** `test_xray_fast_max_depth`

**Status:** ✅ Implemented

---

### T79d: `xray_fast` — `dirsOnly` truncation hint for large results

**Expected:**

- `match_count > 150` without `maxDepth` → `summary.hint` recommends `maxDepth=1`

**Unit test:** `test_xray_fast_dirsonly_truncation_hint`

**Status:** ✅ Implemented

---

### T80: `xray_fast` — Regex mode

**Unit test:** `test_xray_fast_regex_mode`

---

### T81: `xray_fast` — Empty pattern handling

**Expected:**

- `pattern: ""` without `dir` → error
- `pattern: ""` with `dir` → wildcard listing
- `pattern: "*"` → wildcard listing

**Unit tests:** `test_xray_fast_empty_pattern_returns_error`, `test_xray_fast_empty_pattern_with_dir`, `test_xray_fast_wildcard_star`

---

### T09b: `fast` — Comma-separated multi-term search via MCP `xray_fast`

**Expected:**

- `summary.totalMatches` > 1
- `files` array contains paths matching ANY of the terms

---

### T-RANK-04: `xray_fast` — Relevance ranking (exact stem → prefix → contains)

**Expected order:** `UserService.cs` (exact stem) → `UserServiceFactory.cs` (prefix) → `IUserService.cs` (contains)

**Unit tests:** `test_xray_fast_ranking_exact_stem_first`, `test_xray_fast_ranking_shorter_stem_first`

**Status:** ✅ Implemented

---

### T-DIR-SECURITY: Directory validation — Security boundary tests (4 tests)

**Unit tests:** `test_validate_search_dir_subdir_accepted`, `test_validate_search_dir_outside_rejected`, `test_validate_search_dir_path_traversal_rejected`, `test_validate_search_dir_windows_absolute_outside_rejected`

---

## xray_edit

### T-EDIT-01: Mode A — Line-range replace

**Expected:**

- Line replaced, response contains `applied: 1` and unified diff

**Unit tests:** `test_mode_a_replace_single_line`, `test_mode_a_replace_range`, `test_mode_a_multiple_operations_bottom_up`

---

### T-EDIT-02: Mode A — Insert before line

**Expected:**

- `endLine < startLine` inserts content before `startLine`

**Unit test:** `test_mode_a_insert_before_line`

---

### T-EDIT-03: Mode A — Delete lines

**Expected:**

- Empty `content` with valid range deletes those lines

**Unit test:** `test_mode_a_delete_lines`

---

### T-EDIT-04: Mode B — Text find-replace (all occurrences)

**Expected:**

- All occurrences replaced, response contains `totalReplacements` count

**Unit tests:** `test_mode_b_literal_replace_all`, `test_mode_b_literal_replace_specific_occurrence`

---

### T-EDIT-05: Mode B — Regex replace

**Expected:**

- Capture groups `$1`, `$2` work in replacement

**Unit test:** `test_mode_b_regex_replace`

---

### T-EDIT-06: dryRun — Preview without writing

**Expected:**

- `dryRun: true` returns diff but does NOT modify the file

**Unit test:** `test_dry_run_does_not_write`

---

### T-EDIT-07: Error handling

**Expected errors:**

- File not found
- Both operations + edits → error
- Neither → error
- Line out of range
- `expectedLineCount` mismatch
- Overlapping operations
- Search text not found

**Unit tests:** `test_file_not_found_error`, `test_both_operations_and_edits_error`, `test_mode_a_out_of_range_error`, `test_mode_a_expected_line_count_mismatch`, `test_mode_a_overlap_error`, `test_mode_b_text_not_found_error`

---

### T-EDIT-08: CRLF preservation

**Unit test:** `test_crlf_preservation`

---

### T-EDIT-10: Append mode (Mode A insert at end of file)

**Unit test:** `test_mode_a_insert_at_end_of_file`

---

### T-EDIT-11: Multi-file editing (`paths` parameter)

**Expected:**

- All files modified atomically (transactional: if any fails, none are written)
- `paths` + `path` together → error
- More than 20 paths → error

**Unit tests:** `test_multi_file_all_succeed`, `test_multi_file_one_fails_aborts_all`, `test_multi_file_max_limit`

---

### T-EDIT-12: Insert after/before anchor text

**Expected:**

- `insertAfter` inserts on line after anchor
- `insertBefore` inserts on line before anchor
- `occurrence: 2` targets 2nd occurrence

**Unit tests:** `test_insert_after_found`, `test_insert_before_found`, `test_insert_after_specific_occurrence`

---

### T-EDIT-13: expectedContext safety check

**Expected:**

- Edit applied only if context exists within ±5 lines of match
- Context not found → error

**Unit tests:** `test_expected_context_match`, `test_expected_context_mismatch`

---

### T-EDIT-14: skipIfNotFound — silently skip missing edits

**Expected:**

- `skipIfNotFound: true` → files without match silently skipped
- Response includes `skippedEdits` count

**Unit tests:** `test_skip_if_not_found_single_file`, `test_skip_if_not_found_multi_file_partial_match`

---

### T-EDIT-15: Nearest match hint on "text not found" error

**Expected:**

- Error message includes `"Nearest match at line N (similarity M%)"`
- Suppressed when file > 500KB or best similarity < 40%

**Unit tests:** `test_nearest_match_hint_different_quotes`, `test_nearest_match_hint_partial_overlap`

---

### T-EDIT-16: skippedDetails in response

**Expected:**

- `skippedDetails` array with `editIndex`, `search`, `reason` for each skipped edit

**Unit tests:** `test_skipped_details_contains_edit_info`, `test_skipped_details_multiple_skips`

---

### T-EDIT-17: Whitespace normalization — CRLF normalized at parse time

**Expected:**

- CRLF in search text normalized at parse time (preserved through write).
- Trailing-whitespace drift in `search` or `anchor` is **NOT** auto-stripped — it surfaces as a `Text not found` error with a categorised `Nearest match` hint. Silent retry was removed in PR #1 (2026) because it could match a semantically different block.
- All-whitespace search text fails gracefully.

**Unit tests:** `test_crlf_preservation`, `test_no_silent_match_on_trailing_whitespace_drift`, `test_no_silent_match_trailing_whitespace_in_search`, `test_no_silent_match_trailing_whitespace_in_anchor`, `test_no_silent_match_trailing_ws_cascade`

**Status:** ✅ Implemented (cascade simplified — see T-EDIT-23)


---

### T-EDIT-18: Blank-line drift surfaces as `Text not found` (auto-trim removed)

**Expected:**

- Search `"\n## Heading"` against file `"## Heading"` (search has leading blank line that the file does not) → `Text not found` error with `Nearest match` hint pointing at `"## Heading"`. Auto-trim was removed in PR #1.
- Search `"text\n\n"` against `"text"` (search has trailing blank lines) → `Text not found` with `Nearest match` hint.
- Anchor `"\nline one"` against `"line one"` → `Text not found` (insert is rejected with hint).
- No blank lines in search text → exact match (no warning, no error).

**Unit tests:** `test_no_silent_match_search_leading_newline`, `test_no_silent_match_search_trailing_newlines`, `test_no_silent_match_anchor_leading_newline`, `test_no_silent_match_blank_lines_cascade`, `test_no_silent_match_on_blank_lines_drift`, `test_blank_line_trim_no_change_needed`

**Status:** ✅ Implemented (PR #1 — see T-EDIT-23)

---

### T-EDIT-19: Flex-space matching — whitespace-collapsed search (opt-in)

**Important:** Flex-space matching is **opt-in** — it runs only when an `expectedContext` is also supplied (the safety guard). Without `expectedContext`, whitespace drift surfaces as `Text not found` with a `Nearest match` hint (see T-EDIT-17 / T-EDIT-18). This avoids silent matches against similar-looking blocks elsewhere in the file.

**Expected (when `expectedContext` is set):**

- Search `"| A | B |"` matches file content `"| A       | B     |"` (padded table) → edit applied with warning mentioning "flexible whitespace"
- Multi-line search with padding → matched correctly
- Anchor `"| Bug 1 | 5 |"` matches padded version → insertAfter/insertBefore applied
- `occurrence: 2` with flex-space → correct occurrence replaced
- Exact match preferred → no warning when file matches exactly
- Regex mode (`is_regex: true`) → flex-space NOT applied (error returned)
- Replacement text with `$` → treated literally (no regex expansion)

**Unit tests:** `test_flex_space_table_padding`, `test_flex_space_multiline_table`, `test_flex_space_exact_match_preferred`, `test_flex_space_anchor_insert_after`, `test_flex_space_anchor_insert_before`, `test_flex_space_with_occurrence`, `test_flex_space_not_used_for_regex_mode`, `test_flex_space_replacement_dollar_sign_safety`

**Status:** ✅ Covered by 8 unit tests

---

### T-EDIT-20: expectedContext flex-space fallback

**Expected:**

- Padded `expectedContext` like `"| Issue | Count |"` matches file content `"| Issue       | Count     |"` → edit not rejected
- Exact context still works
- Completely wrong context still fails

**Unit tests:** `test_expected_context_flex_space`, `test_expected_context_exact_match_still_works`, `test_expected_context_wrong_context_still_fails`

**Status:** ✅ Covered by 3 unit tests

---

### T-EDIT-21: Helper functions — collapse_spaces, search_to_flex_pattern

**Expected:**

- `collapse_spaces` collapses runs of spaces/tabs to single space per line
- `search_to_flex_pattern` produces valid regex matching padded text, returns `None` for all-whitespace input

**Unit tests:** `test_collapse_spaces`, `test_search_to_flex_pattern`

**Status:** ✅ Implemented (note: the previous `trim_blank_lines` helper was removed alongside the auto-trim cascade in PR #1 — see T-EDIT-18 / T-EDIT-23)

### T-EDIT-22: Flex-space matching — markdown table separator dash count mismatch (opt-in)

**Note:** As with all flex-space matches (T-EDIT-19), this requires `expectedContext` to be set on the edit — the cascade is opt-in.

**Expected:**

- Search `"| Cluster | Status |\n|---|---|\n| East | OK |"` matches file with `"|---------|-------------|"` separator → edit applied with flex-space warning
- Search `"|---|---|"` as anchor matches `"|---------|-------------|"` via `insertAfter` → insert applied
- Separator with alignment colons in file (`|:---|---:|`) matches plain dash search (`|---|---|`)
- En dash (`–`) and em dash (`—`) in search match regular hyphens in file
- Column count preserved: `|---|---|---|` does NOT match `|---|---|`

**Unit tests:** `test_flex_space_markdown_separator_dash_count_mismatch`, `test_flex_space_markdown_separator_with_alignment`, `test_flex_space_markdown_separator_anchor_insert`, `test_search_to_flex_pattern` (extended with separator patterns and Unicode dash assertions)

**Status:** ✅ Covered by 4 integration tests + unit test extensions

### T-EDIT-23: Retry cascade — current 2-stage shape and regression coverage

**Context:** The `apply_text_edits` retry-cascade was simplified in PR #1 (2026). The original 4-stage cascade (exact → strip-trailing-WS → trim-blank-lines → flex-space) was reduced to **2 stages**:

  1. **Exact literal match.**
  2. **Flex-space regex match (collapse whitespace) — OPT-IN via `expectedContext`.**

Stages 2 (strip trailing WS) and 3 (trim blank lines) were **removed** because silent retries could match a semantically different block. Drift in either dimension now surfaces as a `Text not found` error with a categorised `Nearest match` hint (see T-EDIT-17 / T-EDIT-18).

**Expected:**

- Trailing-WS drift in search/anchor → `Text not found` (no silent fix-up).
- Leading/trailing blank-line drift in search/anchor → `Text not found` (no silent fix-up).
- Flex-space match runs only when `expectedContext` is set; emits a warning string `"matched with flexible whitespace (spaces collapsed) [fallbackApplied:flexWhitespace]"` when used.
- `expectedContext` validation runs after the flex-space match is found.
- Error message distinguishes literal vs flex search shape.

**Unit tests (in `src/mcp/handlers/edit_tests.rs::retry_cascade_tests`):** `test_no_silent_match_trailing_ws_cascade`, `test_no_silent_match_blank_lines_cascade`, `test_retry_cascade_flex_regex`, `test_error_message_literal_vs_flex`, `test_expected_context_after_flex_match`

**Companion tests (other modules):** `test_no_silent_match_search_leading_newline`, `test_no_silent_match_search_trailing_newlines`, `test_no_silent_match_anchor_leading_newline`, `test_no_silent_match_on_trailing_whitespace_drift`, `test_no_silent_match_on_blank_lines_drift`, `test_no_silent_match_trailing_whitespace_in_search`, `test_no_silent_match_trailing_whitespace_in_anchor`, `test_no_silent_match_skip_if_not_found_does_not_resurrect_match`

**Status:** ✅ Implemented (current cascade shape; PR #1 simplification)

---


## T-SYNC-* — Synchronous Reindex After `xray_edit`

**Context:** `xray_edit` was extended (2026-04-19, user story `todo_approved_2026-04-19_xray-edit-sync-reindex.md`) to perform an in-process reindex of the written file(s) immediately after a successful real write. Before this change, a follow-up `xray_grep` / `xray_definitions` call could miss content for up to ~500ms (FS-watcher debounce window). These E2E tests verify the new behavior end-to-end over MCP JSON-RPC.

### T-SYNC-GREP: edit-then-grep sees new content with no wait

**Setup:** Server running with `--ext rs --definitions`. Pre-existing fixture file `e2e/fixtures/sync_reindex_target.rs` containing `pub fn placeholder_marker_alpha() {}`.

**Steps:**
1. Send MCP `xray_grep` with `terms='placeholder_marker_zeta_42'` (a unique token NOT yet in the file). Expect `totalOccurrences: 0`.
2. Send MCP `xray_edit` with `path='e2e/fixtures/sync_reindex_target.rs'`, `edits=[{search:'placeholder_marker_alpha', replace:'placeholder_marker_zeta_42'}]`.
   - Verify response contains `contentIndexUpdated: true`, `defIndexUpdated: true`, `reindexElapsedMs` is a string parseable as float, `fileListInvalidated: false` (existing file).
3. **Immediately** (no sleep) send MCP `xray_grep` with `terms='placeholder_marker_zeta_42'`. Expect `totalOccurrences: 1` and the new file path in `files`.

**Status:** ✅ Required.

### T-SYNC-DEFS: edit-then-definitions sees new symbol with no wait

**Steps:**
1. `xray_definitions` with `name='ImaginaryStruct_X'` → expect 0 results.
2. `xray_edit` with `path='e2e/fixtures/sync_reindex_target.rs'`, `edits=[{search:'placeholder_marker_zeta_42', replace:'pub struct ImaginaryStruct_X {}\n// placeholder_marker_zeta_42'}]`.
   - Verify response: `contentIndexUpdated: true`, `defIndexUpdated: true`.
3. **Immediately** `xray_definitions` with `name='ImaginaryStruct_X'` → expect exactly 1 result with `kind: "struct"`.

**Status:** ✅ Required.

### T-SYNC-MULTI: multi-file edit batches a single reindex with summary metric

**Steps:**
1. `xray_edit` with `paths=['e2e/fixtures/sync_a.rs', 'e2e/fixtures/sync_b.rs']`, `edits=[{search:'BatchTokenOld', replace:'BatchTokenNew_$RANDOM', skipIfNotFound:true}, {search:'BTwoOld', replace:'BTwoNew_$RANDOM', skipIfNotFound:true}]`.
   - Verify per-file `contentIndexUpdated: true` for each in-scope file in `results[]`.
   - Verify `summary.reindexElapsedMs` is present (single batched reindex call).
2. Immediately `xray_grep terms='BatchTokenNew_$RANDOM,BTwoNew_$RANDOM' mode=or` → expect both tokens found.

**Status:** ✅ Required.

### T-SYNC-FAST: file-creation invalidates the file-list cache

**Steps:**
1. `xray_fast pattern='brand_new_sync_file'` → expect 0 results (file does not exist yet).
2. `xray_edit path='e2e/fixtures/brand_new_sync_file_$RANDOM.rs' operations=[{startLine:1, endLine:0, content:'fn newly_created() {}\n'}]`.
   - Verify response: `fileCreated: true`, `fileListInvalidated: true`, `contentIndexUpdated: true`.
3. Immediately `xray_fast pattern='brand_new_sync_file_$RANDOM'` → expect exactly 1 result. (The `xray_fast` cache rebuild is triggered lazily by the dirty flag set in step 2.)

**Status:** ✅ Required.

### T-SYNC-DRYRUN: `dryRun` omits ALL reindex fields

**Steps:**
1. `xray_edit path='e2e/fixtures/sync_reindex_target.rs' dryRun=true edits=[{search:'pub fn',replace:'pub fn dryrun_marker'}]`.
2. Verify response JSON does NOT contain ANY of: `contentIndexUpdated`, `defIndexUpdated`, `fileListInvalidated`, `reindexElapsedMs`, `skippedReason`, `reindexWarning`, `fileCreated`.
3. Verify the file on disk is unchanged.
4. `xray_grep terms='dryrun_marker'` → 0 results (dryRun didn't pollute the index).

**Status:** ✅ Required.

### T-SYNC-OUTSIDE-DIR: edit to file outside `--dir` is skipped (file written, index untouched)

**Setup:** create a temp file OUTSIDE the server's `--dir` (e.g., `$env:TEMP\xray_sync_outside.rs`).

**Steps:**
1. `xray_edit path='<absolute path outside server dir>' edits=[{search:'fn',replace:'pub fn outside_marker_$RANDOM'}]`.
2. Verify response: `contentIndexUpdated: false`, `skippedReason: "outsideServerDir"`.
3. Verify the OUTSIDE file ON DISK was actually edited (file write must succeed even when reindex is skipped).
4. `xray_grep terms='outside_marker_$RANDOM'` against the server → 0 results (server's index correctly excluded the foreign file).

**Status:** ✅ Required.

### T-SYNC-EXT-NOT-INDEXED: edit to wrong-extension file is skipped (file written, index untouched)

**Setup:** Server started with `--ext rs`. Target file `e2e/fixtures/notes.txt` (not in `--ext`).

**Steps:**
1. `xray_edit path='e2e/fixtures/notes.txt' edits=[{search:'PLAIN', replace:'TXT_TOKEN_$RANDOM'}]`.
2. Verify response: `contentIndexUpdated: false`, `skippedReason: "extensionNotIndexed"`.
3. Verify `notes.txt` on disk WAS modified (write succeeds).
4. `xray_grep terms='TXT_TOKEN_$RANDOM'` → 0 results (the .txt file was never in scope).

**Status:** ✅ Required.

### T-SYNC-RECONCILE-PRESERVED: FS-watcher reconciliation still works for external edits

**Setup:** Server running. Edit a tracked .rs file via OS-level write (NOT through `xray_edit`) — e.g., PowerShell `Add-Content` or `notepad.exe save`.

**Steps:**
1. Pre-condition: `xray_grep terms='ExternalReconcileToken'` → 0 results.
2. Use OS write (PowerShell `Set-Content`) to add `// ExternalReconcileToken` to a tracked .rs file in the server's --dir.
3. **Wait 1 second** (cover the 500ms watcher debounce).
4. `xray_grep terms='ExternalReconcileToken'` → 1 result (proves the FS watcher path is NOT broken by the new sync-reindex code; both paths coexist).

**Status:** ✅ Required — guards against regression where the new sync-reindex accidentally disables or interferes with the watcher.

---
