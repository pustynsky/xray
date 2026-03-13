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
- Non-wildcard queries do NOT include `fileCount`

**Unit tests:** `test_xray_fast_dirsonly_wildcard_filecount`, `test_xray_fast_dirsonly_non_wildcard_no_filecount`

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
