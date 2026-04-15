# MCP `xray_fast`, `xray_edit`, `xray_find` Tests

Tests for file name search (`xray_fast`), file editing (`xray_edit`), and filesystem search (`xray_find`) MCP tools.

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

**Expected:**

- `dir: "subA"` (relative) → resolves against server_dir, scopes grep to subdirectory
- Results contain only files from the specified subdirectory

**Unit test:** `test_grep_with_relative_subdir_filter`

**Status:** ✅ Implemented

**Expected:**

- No new `.file-list` index for subdirectory
- Results scoped to requested subdirectory
- `maxDepth` relative to subdirectory, not root

**Unit tests:** `test_xray_fast_subdir_reuses_parent_index`, `test_xray_fast_subdir_max_depth_relative_to_dir`

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

## xray_find

### T43: `serve` — xray_find directory validation (security)

**Expected:**

- Directory outside `server_dir` → error
- Subdirectory of `server_dir` → accepted
- No `dir` parameter → uses `server_dir` as default

**Unit tests:** `test_validate_search_dir_subdirectory`, `test_validate_search_dir_outside_rejects`

**Status:** ✅ Implemented

---

### T82: `xray_find` — Combined parameters (countOnly, maxDepth, ignoreCase+regex)

**Unit test:** `test_xray_find_combined_parameters`

---

### T106: `xray_find` — Contents mode

**Expected:**

- `contents: true` searches file content, not names
- Results include file path and line number

**Unit test:** `test_xray_find_contents_mode`

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

### T-EDIT-17: Whitespace normalization — CRLF and trailing whitespace auto-retry

**Expected:**

- CRLF in search text normalized at parse time
- Trailing whitespace auto-retry with warning
- All-whitespace search text fails gracefully

**Status:** ✅ Covered by 22 unit tests


---

### T-EDIT-18: Blank line trimming — leading/trailing blank lines in search text

**Expected:**

- Search `"\n## Heading"` matches `"## Heading"` in file → edit applied with warning
- Search `"text\n\n"` matches `"text"` in file → edit applied with warning
- Anchor `"\nline one"` matches `"line one"` → insert applied with warning
- No blank lines in search text → exact match, no warning

**Unit tests:** `test_blank_line_trim_search_leading_newline`, `test_blank_line_trim_search_trailing_newlines`, `test_blank_line_trim_anchor_leading_newline`, `test_blank_line_trim_no_change_needed`

**Status:** ✅ Covered by 4 unit tests

---

### T-EDIT-19: Flex-space matching — whitespace-collapsed search

**Expected:**

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

### T-EDIT-21: Helper functions — trim_blank_lines, collapse_spaces, search_to_flex_pattern

**Expected:**

- `trim_blank_lines` strips leading/trailing `\n`, preserves interior
- `collapse_spaces` collapses runs of spaces/tabs to single space per line
- `search_to_flex_pattern` produces valid regex matching padded text, returns `None` for all-whitespace input

**Unit tests:** `test_trim_blank_lines`, `test_collapse_spaces`, `test_search_to_flex_pattern`

**Status:** ✅ Covered by 3 unit tests

### T-EDIT-22: Flex-space matching — markdown table separator dash count mismatch

**Expected:**

- Search `"| Cluster | Status |\n|---|---|\n| East | OK |"` matches file with `"|---------|-------------|"` separator → edit applied with flex-space warning
- Search `"|---|---|"` as anchor matches `"|---------|-------------|"` via `insertAfter` → insert applied
- Separator with alignment colons in file (`|:---|---:|`) matches plain dash search (`|---|---|`)
- En dash (`–`) and em dash (`—`) in search match regular hyphens in file
- Column count preserved: `|---|---|---|` does NOT match `|---|---|`

**Unit tests:** `test_flex_space_markdown_separator_dash_count_mismatch`, `test_flex_space_markdown_separator_with_alignment`, `test_flex_space_markdown_separator_anchor_insert`, `test_search_to_flex_pattern` (extended with separator patterns and Unicode dash assertions)

**Status:** ✅ Covered by 4 integration tests + unit test extensions

---
