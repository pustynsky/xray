# Git Tools Tests

Tests for `xray_git_history`, `xray_git_diff`, `xray_git_authors`, `xray_git_activity`, `xray_git_blame`, `xray_branch_status`, and the git history cache.

---

## Git History

### T-GIT-01: `xray_git_history` — File commit history

**Expected:**

- `commits` array (non-empty), each with `hash`, `date`, `author`, `email`, `message`
- `summary.totalCommits` ≥ 1, `summary.returned` ≤ maxResults
- No `patch` field (history mode)

**Unit tests:** `test_file_history_returns_commits`, `test_commit_info_has_all_fields`

---

### T-GIT-02: `xray_git_diff` — Patches

**Expected:**

- Each commit has `patch` field with +/- lines
- Patches truncated to ~200 lines per commit

**Unit test:** `test_file_history_with_diff`

---

### T-GIT-03: `xray_git_authors` — Ranked authors

**Expected:**

- `authors` array ranked by commit count descending
- Each author has `rank`, `name`, `email`, `commits`, `firstChange`, `lastChange`

**Unit tests:** `test_top_authors_returns_ranked`, `test_top_authors_limits_results`

---

### T-GIT-04: `xray_git_activity` — Repo-wide changes

**Expected:**

- `activity` array with `path`, `commits`, `commitCount`
- Sorted by commit count descending

**Unit test:** `test_repo_activity_returns_files`

---

### T-GIT-04b: `xray_git_activity` with `path` filter

**Expected:**

- All entries have `path` starting with specified directory
- Nonexistent path → `warning` field

**Unit tests:** `test_repo_activity_with_path_filter`, `test_git_activity_nonexistent_path_has_warning`

**Status:** ✅ Implemented

---

### T-GIT-05: `xray_git_history` with date filter

**Expected:**

- Date filtering narrows results correctly
- Impossible date → 0 results

**Unit tests:** `test_file_history_date_filter_narrows_results`

---

### T-GIT-06: Missing required parameter

**Expected:** `isError: true`, message: "Missing required parameter: file"

---

### T-GIT-07: Bad repo path

**Expected:** `isError: true`, repository not found

**Unit tests:** `test_file_history_bad_repo`, `test_repo_activity_bad_repo`

---

### T-GIT-08: Git tools available without --definitions flag

**Expected:**

- `tools/list` contains all 16 tools including 6 git tools
- No `--git` flag needed

---

## Git Blame

### T-GIT-BLAME-01: Basic line blame

**Expected:**

- Non-empty `BlameLine` entries with `hash`, `author_name`, `date`, `content`

**Unit tests:** `test_blame_lines_returns_results`, `test_blame_lines_single_line`, `test_blame_lines_has_content`

---

### T-GIT-BLAME-02: Error handling

**Expected:**

- Nonexistent file → `Err`
- Nonexistent repo → `Err`

**Unit tests:** `test_blame_lines_nonexistent_file`, `test_blame_lines_bad_repo`

---

### T-GIT-BLAME-03: Porcelain parser

**Unit tests:** `test_parse_blame_porcelain_basic`, `test_parse_blame_porcelain_repeated_hash`

---

### T-GIT-BLAME-04: Date formatting with timezone offset

**Unit tests:** `test_format_blame_date`, `test_format_blame_date_positive_offset`, `test_format_blame_date_nepal_offset`

---

### T-GIT-BLAME-05: Timezone offset parsing

**Unit test:** `test_parse_tz_offset`

---

## Empty Results Validation

### T70: `xray_git_history` — File never tracked in git

**Expected:**

- `totalCommits: 0`, `commits: []`
- `warning` field starts with: "File never tracked in git: ..."
- The warning advises to check the path spelling
- Distinguishes 'never existed' from 'deleted' (deleted files now succeed via T-DELETED-* below)

---

### T70b: `xray_git_history` — File exists, no commits in range

**Expected:**

- `totalCommits: 0`, `commits: []`
- No `warning` field (file is tracked)

---

## Branch Status

### T-BRANCH-WARNING: `branchWarning` in index-based tool responses

**Expected:**

- Feature branch → `summary.branchWarning` present with branch name
- Main/master → `summary.branchWarning` absent

**Unit tests:** `test_branch_warning_feature_branch`, `test_branch_warning_main_branch`

---

### T-BRANCH-STATUS: `xray_branch_status` — Branch info

**Expected:**

- `currentBranch`, `isMainBranch`, `mainBranch`, `behindMain`, `aheadOfMain`
- `dirtyFiles`, `dirtyFileCount`, `lastFetchTime`, `fetchAge`, `fetchWarning`, `warning`

**Unit tests:** `test_branch_status_returns_current_branch`, `test_branch_status_detects_main_branch`, `test_is_main_branch`, `test_format_age`, `test_compute_fetch_warning_thresholds`

---

## Date Handling

### T-GIT-DATE-UTC: CLI date filtering uses UTC

**Expected:**

- `--after=2025-12-16T00:00:00Z` (appends UTC suffix)
- CLI and cache behavior match

**Unit tests:** `test_date_2024_12_16_start`, `test_commit_1734370112_is_2024_not_2025`

---

### T-VAL-04: Reversed date range returns error

**Expected:**

- `from > to` → `isError: true`, message mentions both dates

**Unit tests:** `test_parse_date_filter_reversed_range_returns_error`

---

## noCache Parameter

### T-NOCACHE: `noCache` bypasses git history cache

**Expected:**

- `noCache: true` → CLI path (no `"(from cache)"` in hint)
- Without `noCache` + cache available → `"(from cache)"` in hint

**Applies to:** `xray_git_history`, `xray_git_authors`, `xray_git_activity`

**Unit tests:** `test_git_history_no_cache_bypasses_cache`, `test_git_history_default_uses_cache`

---

## Deleted Files Support (added 2026-04-17)

The git tools fully support deleted files: history is preserved via an internal
`--follow` → no-follow fallback in `file_history`, and `xray_git_activity`
accepts `includeDeleted=true` to list files that were removed in the date range.

No separate `git log --all --diff-filter=D` invocation is needed — these tools
cover the case in a single call.

### T-DELETED-01: `xray_git_history` returns full history for a deleted file

**Setup:** repo with a file that was added in commit A, modified in commit B,
deleted in commit C.

**Call:** `xray_git_history repo='.' file='<deleted_path>'`

**Expected:**

- `totalCommits >= 3` (add + modify + delete commits all returned)
- `commits` array contains the delete commit (message includes "delete" or similar)
- `info` field present, mentioning "not in current HEAD" and "NOT an error"
- `warning` field is ABSENT (this is not a wrong-path case)

**Unit tests:** `git::tests::test_file_history_returns_commits_for_deleted_file`,
`git::tests::test_file_ever_existed_in_git_accepts_deleted`

---

### T-DELETED-02: `xray_git_history` distinguishes 'never existed' from 'deleted'

**Call:** `xray_git_history repo='.' file='never_committed.txt'`

**Expected:**

- `totalCommits: 0`, `commits: []`
- `warning` present ("File never tracked in git: ...")
- `info` ABSENT

**Unit test:** `git::tests::test_file_history_returns_empty_for_never_existed_file`

---

### T-DELETED-03: `xray_git_activity` with `includeDeleted=true` filters to deleted files only

**Call:** `xray_git_activity repo='.' from='2024-01-01' includeDeleted=true`

**Expected:**

- `summary.includeDeleted: true`
- `summary.hint` mentions "NOT in current HEAD"
- The activity list contains ONLY files that are not in current HEAD
  (compared to the same call without `includeDeleted`, the count is strictly lower
  when at least one returned file still exists)

**Performance invariant:** the implementation MUST issue exactly ONE `git ls-files`
spawn (single call to `git::list_tracked_files_under`), regardless of how many
files are in the activity result. A per-file existence check would scale poorly
for large repos.

**Unit tests:**
- `mcp::handlers::tests_git::test_git_activity_include_deleted_default_false`
- `mcp::handlers::tests_git::test_git_activity_include_deleted_true_sets_field_and_hint`
- `mcp::handlers::tests_git::test_git_activity_include_deleted_filters_existing_files_in_real_repo`
- `git::tests::test_list_tracked_files_under_excludes_deleted`
- `git::tests::test_list_tracked_files_under_bad_repo_returns_empty`

---

### T-DELETED-04: `file_exists_in_current_head` rejects deleted files

**Setup:** temp repo where `legacy.txt` is added then deleted.

**Expected:**

- `file_exists_in_current_head(repo, "legacy.txt") == false`
- `file_exists_in_current_head(repo, "survivor.txt") == true`
- `file_ever_existed_in_git(repo, "legacy.txt") == true`
- `file_ever_existed_in_git(repo, "never_added.txt") == false`

**Unit tests:** `git::tests::test_file_exists_in_current_head_rejects_deleted`,
`git::tests::test_file_ever_existed_in_git_accepts_deleted`

---


## Git History Cache — Unit Tests

### T-CACHE-01: Parser — Multi-commit git log output

**Unit tests:** `test_parser_multi_commit`, `test_parser_commit_fields`

---

### T-CACHE-02: Parser — Edge cases

**Unit tests:** `test_parser_empty_input`, `test_parser_empty_subject`, `test_parser_subject_with_field_sep`, `test_parser_empty_file_list`, `test_parser_malformed_line_skipped`, `test_parser_bad_hash_skipped`

---

### T-CACHE-03: Path normalization

**Unit tests:** `test_normalize_path_backslash`, `test_normalize_path_dot_slash`, `test_normalize_path_empty`

---

### T-CACHE-04: Query — File history with filters

**Unit tests:** `test_query_file_history_basic`, `test_query_file_history_max_results`, `test_query_file_history_from_date_filter`, `test_query_with_backslash_path`

---

### T-CACHE-05: Query — Authors aggregation

**Unit tests:** `test_query_authors_single_file`, `test_query_authors_directory`, `test_query_authors_empty_path_matches_all`

---

### T-CACHE-06: Query — Activity with path prefix matching

**Unit tests:** `test_query_activity_directory_prefix`, `test_query_activity_prefix_no_false_positive`, `test_query_activity_sorted_by_last_modified`

---

### T-CACHE-07: Cache validity

**Unit tests:** `test_is_valid_for_matching_head`, `test_is_valid_for_non_matching_head`, `test_is_valid_for_checks_format_version`

---

### T-CACHE-09: CommitMeta struct size

**Unit test:** `test_commit_meta_size` (≤ 48 bytes)

---

### T-CACHE-10: Serialization roundtrip

**Unit tests:** `test_cache_serialization_roundtrip`, `test_cache_lz4_compressed_roundtrip`

---

### T-CACHE-11: Author deduplication

**Unit test:** `test_author_deduplication`

---

### T-CACHE-12: Hex hash conversion

**Unit tests:** `test_hex_to_bytes_roundtrip`, `test_hex_to_bytes_invalid_length`

---

### T-CACHE-13: Bad timestamp parsing

**Unit test:** `test_parser_bad_timestamp_skipped`

---

### T-CACHE-14: Author pool overflow boundary

**Unit tests:** `test_author_pool_overflow_via_parser`, `test_author_pool_boundary_65535_succeeds`

---

### T-CACHE-15: cache_path_for() different directories

**Unit test:** `test_cache_path_for_different_dirs_produce_different_paths`

---

### T-CACHE-17: Date boundary — exact-day filter

**Unit tests:** `test_query_file_history_exact_date_boundary`, `test_query_activity_vs_file_history_consistency`

---

### T-CACHE-18: Path case sensitivity

**Unit test:** `test_query_file_history_path_case_sensitivity`

---

### T-CACHE-19: Authors timestamps always non-zero

**Unit tests:** `test_query_authors_first_last_timestamps_nonzero`, `test_query_authors_single_commit_timestamps_equal`

---

### T-CACHE-20: Author/message filtering — query_file_history

**Unit tests:** `test_query_file_history_author_filter`, `test_query_file_history_message_filter`, `test_query_file_history_author_and_message_combined`

---

### T-CACHE-21: Author/message filtering — query_activity

**Unit tests:** `test_query_activity_author_filter`, `test_query_activity_message_filter`

---

### T-CACHE-22: Author/message filtering — query_authors

**Unit tests:** `test_query_authors_with_message_filter`, `test_query_authors_with_date_filter`

---

## Cache Integration

### T-CACHE-FALLBACK: Git handlers fall back to CLI when cache is None

**Validates:** Cache-or-fallback routing with zero behavioral regression.

---

### T-CACHE-ROUTING: Git handlers use cache when populated

**Expected:**

- `xray_git_history`, `xray_git_authors`, `xray_git_activity`: `"(from cache)"` in hint
- `xray_git_diff`: always CLI (no cache for patches)

---

### T-CACHE-BACKGROUND: Background build and disk persistence

**Expected:**

- First run: background build, save to `.git-history` file
- Second run: load from disk (~100ms vs ~59s rebuild)
- After git pull: HEAD validation triggers rebuild

**Unit tests:** `test_save_load_disk_roundtrip`, `test_cache_path_for_deterministic`

---

### T-CACHE-PROGRESS: Build emits progress logging

**Expected:**

- stderr: `[git-cache] Progress: 10000 commits parsed (X.Xs elapsed)...`

---

### T-CACHE-AUTHORS-TIMESTAMPS: Authors query returns first and last commit timestamps

**Unit test:** `test_query_authors_timestamps`
