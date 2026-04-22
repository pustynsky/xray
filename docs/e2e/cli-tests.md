# CLI Tests

CLI commands tested directly via `cargo run --` (or installed binary).

---

## `index` / `fast` — File Name Index

### T06: `index` — Build file index

**Command:**

```powershell
cargo run -- index -d $TEST_DIR
```

**Expected:**

- Exit code: 0
- stderr: `Indexing ...`, `Indexed N entries in X.XXXs`, `Index saved to ... (X.X MB)`
- A `.file-list` file created in the index directory

**Validates:** File index build and persistence.

---

### T07: `fast` — Search file name index

**Command:**

```powershell
cargo run -- fast "main" -d $TEST_DIR -e $TEST_EXT
```

**Expected:**

- Exit code: 0
- stdout: at least 1 file path
- stderr: match count, index load/search timing

**Validates:** File index loading + search. Auto-builds index if missing.

---

### T08: `fast` — Regex + case-insensitive

**Command:**

```powershell
cargo run -- fast ".*handler.*" -d $TEST_DIR -e $TEST_EXT --regex -i
```

**Expected:**

- Exit code: 0
- stdout: file paths matching the pattern

**Validates:** Regex and ignore-case in fast search.

---

### T09: `fast` — Dirs-only / files-only filters

**Command:**

```powershell
cargo run -- fast "" -d $TEST_DIR --dirs-only
cargo run -- fast "" -d $TEST_DIR --files-only
```

**Expected:**

- `--dirs-only`: only `[DIR]` entries
- `--files-only`: no `[DIR]` entries

**Validates:** Type filtering.

---

### T09a: `fast` — Comma-separated multi-term search (OR logic)

**Command:**

```powershell
cargo run -- fast "main,lib,handler" -d $TEST_DIR -e $TEST_EXT
```

**Expected:**

- Exit code: 0
- stdout: file paths matching ANY of the comma-separated terms
- Returns more results than searching for a single term

**Validates:** Comma-separated patterns are split and matched with OR logic.

---

## `content-index` / `grep` — Content Index

### T10: `content-index` — Build content index

**Command:**

```powershell
cargo run -- content-index -d $TEST_DIR -e $TEST_EXT
```

**Expected:**

- Exit code: 0
- stderr: `Building content index...`, `Indexed N files, M unique tokens (T total) in X.XXXs`
- stderr: `Content index saved to ... (X.X MB)`
- A `.word-search` file created in the index directory

**Validates:** Content index build, tokenization, persistence.

---

### T11: `grep` — Single term search

**Command:**

```powershell
cargo run -- grep "tokenize" -d $TEST_DIR -e $TEST_EXT
```

**Expected:**

- Exit code: 0
- stdout: TF-IDF ranked file list with scores, occurrences, lines
- stderr: summary with file count, token count, timing

**Validates:** Inverted index lookup, TF-IDF scoring, ranking.

---

### T12: `grep` — Multi-term OR

**Command:**

```powershell
cargo run -- grep "tokenize,posting" -d $TEST_DIR -e $TEST_EXT
```

**Expected:**

- Exit code: 0
- stdout: files containing EITHER term, `terms_matched` shows `1/2` or `2/2`
- stderr: `[OR]` mode indicated

**Validates:** Comma-separated OR search.

---

### T13: `grep` — Multi-term AND

**Command:**

```powershell
cargo run -- grep "tokenize,posting" -d $TEST_DIR -e $TEST_EXT --all
```

**Expected:**

- Exit code: 0
- stdout: only files containing BOTH terms (fewer results than T12)
- All results show `2/2` terms matched
- stderr: `[AND]` mode indicated

**Validates:** AND mode filtering.

---

### T14: `grep` — Regex token matching

**Command:**

```powershell
cargo run -- grep ".*stale.*" -d $TEST_DIR -e $TEST_EXT --regex
```

**Expected:**

- Exit code: 0
- stderr: `Regex '...' matched N tokens`
- stdout: files containing tokens matching the pattern

**Validates:** Regex expansion against index keys.

---

### T15: `grep` — Phrase search

**Command:**

```powershell
cargo run -- grep "pub fn" -d $TEST_DIR -e $TEST_EXT --phrase --show-lines
```

**Expected:**

- Exit code: 0
- stderr: `Phrase search: ...` with token list and regex
- stdout: matching lines showing `pub fn` as exact phrase

**Validates:** Phrase tokenization, AND candidate narrowing, regex verification.

---

### T15c: `grep` — Multi-phrase OR search (comma-separated phrases)

**Command:**

```powershell
cargo run -- grep "pub fn,pub struct" -d $TEST_DIR -e $TEST_EXT --phrase
```

**Expected:**

- Exit code: 0
- stdout: files containing EITHER `pub fn` OR `pub struct` as exact phrases
- `searchMode` = `"phrase-or"` (when 2+ comma-separated phrases)

**With AND mode:**

```powershell
cargo run -- grep "pub fn,pub struct" -d $TEST_DIR -e $TEST_EXT --phrase --all
```

**Expected:**

- Only files containing BOTH phrases
- `searchMode` = `"phrase-and"`

**Regression — single phrase (no comma):**

```powershell
cargo run -- grep "pub fn" -d $TEST_DIR -e $TEST_EXT --phrase
```

**Expected:**

- Behavior unchanged: single phrase search
- `searchMode` = `"phrase"` (not `"phrase-or"`)

**Unit tests:** `test_multi_phrase_or_auto_switch`, `test_multi_phrase_or_explicit_phrase`, `test_multi_phrase_and_explicit_phrase`, `test_single_phrase_regression_no_comma`, `test_multi_phrase_fn_signatures`, `test_multi_phrase_count_only`, `test_multi_phrase_explicit_count_only`, `test_tokens_no_spaces_stays_substring`

**Status:** ✅ Implemented

---

### T16: `grep` — Show lines with context

**Command:**

```powershell
cargo run -- grep "is_stale" -d $TEST_DIR -e $TEST_EXT --show-lines -C 2 --max-results 2
```

**Expected:**

- Exit code: 0
- stdout: matching lines marked with `>`, context lines marked with ` `, separators `--`
- At most 2 files shown
- Each match has 2 lines before and 2 lines after

**Validates:** Context lines, max-results truncation, match markers.

---

### T17: `grep` — Exclude dir / exclude pattern

**Command:**

```powershell
cargo run -- grep "ContentIndex" -d $TEST_DIR -e $TEST_EXT --exclude-dir bench --exclude test
```

**Expected:**

- Exit code: 0
- stdout: no paths containing "bench" or "test" (case-insensitive)

**Validates:** Exclusion filters.

---

### T18: `grep` — Count-only mode

**Command:**

```powershell
cargo run -- grep "fn" -d $TEST_DIR -e $TEST_EXT -c
```

**Expected:**

- Exit code: 0
- stdout: empty (no file list)
- stderr: `N files, M occurrences matching...`

**Validates:** Count-only suppresses file output.

---

### T24: `grep` — Before/After context lines

**Command:**

```powershell
cargo run -- grep "is_stale" -d $TEST_DIR -e $TEST_EXT --show-lines -B 1 -A 3
```

**Expected:**

- Exit code: 0
- 1 line before each match, 3 lines after
- Match lines marked with `>`

**Validates:** Asymmetric context (-B/-A) vs symmetric (-C).

---

## `info` / `cleanup` — Index Management

### T19: `info` — Show all indexes

**Command:**

```powershell
cargo run -- info
```

**Expected:**

- Exit code: 0
- stderr: `Index directory: ...`
- stdout: list of `[FILE]`, `[CONTENT]`, and `[GIT]` entries with age, size, staleness
- `[GIT]` entries show: branch, commit count, file count, author count, HEAD hash (first 8 chars), size, age

**Validates:** Index discovery, deserialization of all index types including git-history cache.

---

### T19a: `cleanup` — Remove orphaned index files

**Setup:**

```powershell
$tmp = New-Item -ItemType Directory -Path "$env:TEMP\search_cleanup_test_$(Get-Random)"
cargo run -- index -d $tmp
Remove-Item -Recurse -Force $tmp
```

**Command:**

```powershell
cargo run -- cleanup
```

**Expected:**

- Exit code: 0
- stderr: `Scanning for orphaned indexes in ...`
- stderr: `Removed orphaned index: ... (root: ...search_cleanup_test...)`
- stderr: `Removed N orphaned index file(s).`

**Validates:** Orphaned index detection, safe removal.

---

### T19b: `cleanup --dir` — Remove indexes for a specific directory

**Setup:**

```powershell
$tmp = New-Item -ItemType Directory -Path "$env:TEMP\search_cleanup_dir_test_$(Get-Random)"
Set-Content -Path "$tmp\hello.cs" -Value "class Hello {}"
cargo run -- index -d $tmp
cargo run -- content-index -d $tmp -e cs
```

**Command:**

```powershell
cargo run -- cleanup --dir $tmp
```

**Expected:**

- Exit code: 0
- stderr: `Removing indexes for directory '...' from ...`
- stderr: `Removed N index file(s) for '...'.`
- Indexes for other directories remain untouched

**Validates:** Targeted index cleanup by directory, case-insensitive path comparison.

---

### T19c: `info` — Error reporting for missing index file

**Validates:** `info` command reports a clear warning when an index file is missing, rather than silently ignoring it.

---

### T19d: `info` — Error reporting for corrupt index file

**Validates:** `info` command reports a clear warning when an index file is corrupt (deserialization failure), rather than crashing.

---

### T19e: `info` — Normal operation with valid indexes

**Validates:** `info` command shows index statistics normally when all indexes are valid (baseline/regression test).

---

### T19f: `info` — Git history cache displayed

**Validates:** `info` includes `[GIT]` entry with branch, commits, files, authors, HEAD hash, size, age.

**Unit tests:** `test_info_json_includes_git_history`, `test_info_json_empty_dir_no_git_history`, `test_info_json_nonexistent_dir`, `test_info_json_git_history_corrupt_file_skipped`

---

## `def-index` — Definition Index

### T20: `def-index` — Build definition index

**Command:**

```powershell
cargo run -- def-index -d $TEST_DIR -e $TEST_EXT
```

**Expected:**

- Exit code: 0
- stderr: `[def-index] Found N files to parse`
- stderr: `[def-index] Parsed N files in X.Xs, extracted M definitions`
- A `.code-structure` file created

**Validates:** Tree-sitter parsing, definition extraction, persistence.

**Note:** For `.rs` files, Rust parser is used. For C# or TypeScript projects, expect hundreds/thousands of definitions. For `.sql` files, definitions include stored procedures, tables, views, functions, types, and indexes (regex-based parser).

---

## Error Handling

### T21: `grep` — Invalid regex error handling

**Command:**

```powershell
cargo run -- grep "[invalid" -d $TEST_DIR -e $TEST_EXT --regex
```

**Expected:**

- Exit code: 1
- stderr: `Invalid regex '[invalid': ...`

**Validates:** Graceful error on bad regex.

---

### T22: `find` — Nonexistent directory

**Command:**

```powershell
cargo run -- find "test" -d /nonexistent/path
```

**Expected:**

- Exit code: 1
- stderr: `Directory does not exist: /nonexistent/path`

**Validates:** Graceful error on missing directory.

---

### T23: `grep` — No index available

**Command:**

```powershell
cargo run -- grep "test" -d /tmp/empty_dir_no_index -e xyz
```

**Expected:**

- Exit code: 1
- stderr: `No content index found for ...`

**Validates:** Graceful error when no index exists.

---

## CLI Default Substring Search

### T61: `grep` — Default substring search via trigram index

**Command:**

```powershell
cargo run -- grep "contentindex" -d $TEST_DIR -e $TEST_EXT
```

**Expected:**

- Exit code: 0
- stderr: `Substring 'contentindex' matched N tokens: ...`
- Mode shown as `SUBSTRING-OR`

**Validates:** CLI grep uses substring search by default (same as MCP).

---

### T62: `grep --all` — Default substring AND mode

**Command:**

```powershell
cargo run -- grep "contentindex,tokenize" -d $TEST_DIR -e $TEST_EXT --all
```

**Expected:**

- Exit code: 0
- Only files containing BOTH terms returned
- Mode shown as `SUBSTRING-AND`

**Validates:** CLI default substring search with AND mode.

---

### T63: `grep --exact` — Exact token matching (opt-out of substring)

**Command:**

```powershell
cargo run -- grep "contentindex" -d $TEST_DIR -e $TEST_EXT --exact
```

**Expected:**

- Exit code: 0
- Mode shown as `OR` (not SUBSTRING)
- Only exact token matches (no compound matches like `contentindexargs`)

**Validates:** `--exact` flag disables default substring search.

---

### T64: `grep --regex` — Regex auto-disables substring

**Command:**

```powershell
cargo run -- grep ".*stale.*" -d $TEST_DIR -e $TEST_EXT --regex
```

**Expected:**

- Exit code: 0
- Mode shown as `REGEX` (not SUBSTRING)

**Validates:** `--regex` automatically disables substring mode.

---

## `def-audit` — Definition Index Audit (CLI)

### T-DEF-AUDIT: Definition index audit CLI command

**Command:**

```powershell
xray def-index --dir $TEST_DIR --ext rs
xray def-audit --dir $TEST_DIR --ext rs
```

**Expected:**

- Exit code: 0
- stderr contains `[def-audit] Index:` with total files count
- stderr contains `with definitions` count > 0
- stderr contains `without definitions` count ≥ 0

**When no index exists:**

```powershell
xray def-audit --dir C:\nonexistent --ext cs
```

- stderr contains `No definition index found`
- Exit code: 0

---

## Extension Filtering

### T60: `def-index` — Extension filtering (no unnecessary parsers)

**Command (C# only):**

```powershell
cargo run -- def-index -d $TEST_DIR -e cs
```

**Expected:**

- Exit code: 0
- Only `.cs` files counted
- No TypeScript grammar loading errors

**Validates:** Extension-based parser filtering prevents unnecessary grammar loading.