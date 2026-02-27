# E2E Test Plan — Search Engine

## Overview

This document defines end-to-end tests for the `search` binary. These tests exercise
real CLI commands against a real directory to verify the full pipeline: indexing, searching,
output format, and all feature flags (including substring search via trigram index).

**Run these tests after every major refactoring, before merging PRs, and after dependency upgrades.**

> **Note:** MCP `search_grep` defaults to `substring: true` since v0.2. Tests that expect exact-token behavior must pass `substring: false` explicitly.

## Configuration

| Variable   | Default              | Description                                                    |
| ---------- | -------------------- | -------------------------------------------------------------- |
| `TEST_DIR` | `.` (workspace root) | Directory to index and search                                  |
| `TEST_EXT` | `rs`                 | File extension to index                                        |
| `BINARY`   | `cargo run --`       | Path to the binary (use `./target/release/search` for release) |

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

---

## Test Cases

### T01: `find` — Live filesystem search (file names)

**Command:**

```powershell
cargo run -- find "main" -d $TEST_DIR -e $TEST_EXT
```

**Expected:**

- Exit code: 0
- stdout: at least 1 file path containing "main"
- stderr: summary line with `N matches found among M entries in X.XXXs`

**Validates:** Live filesystem walk, file name matching, extension filter.

---

### T02: `find` — Content search

**Command:**

```powershell
cargo run -- find "fn main" -d $TEST_DIR -e $TEST_EXT --contents
```

**Expected:**

- Exit code: 0
- stdout: at least 1 line in format `path:line: content`
- stderr: summary with match count

**Validates:** Content search mode, line-level matching.

---

### T03: `find` — Regex mode

**Command:**

```powershell
cargo run -- find "fn\s+\w+" -d $TEST_DIR -e $TEST_EXT --contents --regex
```

**Expected:**

- Exit code: 0
- stdout: matching lines with function definitions
- stderr: summary

**Validates:** Regex pattern compilation and matching.

---

### T04: `find` — Case-insensitive search

**Command:**

```powershell
cargo run -- find "CONTENTINDEX" -d $TEST_DIR -e $TEST_EXT --contents -i
```

**Expected:**

- Exit code: 0
- stdout: lines containing "ContentIndex" (original case)

**Validates:** Case-insensitive flag.

---

### T05: `find` — Count-only mode

**Command:**

```powershell
cargo run -- find "fn" -d $TEST_DIR -e $TEST_EXT --contents -c
```

**Expected:**

- Exit code: 0
- stdout: empty (no file paths printed)
- stderr: `N matches found among M entries`

**Validates:** Count-only flag suppresses output.

---

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
- stdout: file paths matching ANY of the comma-separated terms (e.g., files containing "main", "lib", or "handler" in their name)
- Returns more results than searching for a single term

**Validates:** Comma-separated patterns are split and matched with OR logic. Each term is matched independently as a substring of the file name.

---

### T09b: `fast` — Comma-separated multi-term search via MCP `search_fast`

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_fast","arguments":{"pattern":"main,lib,handler","ext":"rs"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `"result"` containing search results
- `summary.totalMatches` > 1 (matches files containing ANY of the terms)
- `files` array contains paths matching "main", "lib", or "handler"

**Validates:** MCP `search_fast` tool supports comma-separated multi-term OR search.

---

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
- `termsSearched` contains 2 entries: `["pub fn", "pub struct"]`

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

**Validates:** Comma-separated phrases are searched independently with OR/AND semantics. Single phrases retain existing behavior. Previously, comma-separated phrases were silently treated as one giant phrase, returning 0 results.

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
- Fewer results than unfiltered T11

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

### T19f: `info` — Git history cache displayed

**Command:**

```powershell
cargo run -- info
```

**Prerequisites:** A `.git-history` file must exist in the index directory (built automatically by the MCP server when started in a git repository).

**Expected:**

- Exit code: 0
- stdout: includes a `[GIT]` entry with format:
  `[GIT] branch=main, N commits, M files, K authors, HEAD=abcdef12, X.X MB, Y.Yh ago (filename.git-history)`
- `N` (commits) > 0
- `M` (files) > 0
- `K` (authors) > 0
- HEAD hash is truncated to first 8 characters
- Size is in MB
- Age is in hours

**MCP equivalent (search_info JSON response):**

```json
{
  "type": "git-history",
  "commits": 1234,
  "files": 5678,
  "authors": 42,
  "headHash": "aabbccddee00112233445566778899aabbccddee",
  "branch": "main",
  "sizeMb": 1.2,
  "ageHours": 3.4,
  "filename": "project_12345678.git-history"
}
```

**Validates:** Git history cache file is discovered and deserialized by both CLI `info` and MCP `search_info`. All key cache metadata (commits, files, authors, branch, HEAD hash) is displayed.

**Unit tests:** `test_info_json_includes_git_history`, `test_info_json_empty_dir_no_git_history`, `test_info_json_nonexistent_dir`, `test_info_json_git_history_corrupt_file_skipped`

---

### T19a: `cleanup` — Remove orphaned index files

**Setup:**

```powershell
# Create a temp directory, index it, then delete the directory
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
- After cleanup, `search-index info` should NOT list the deleted temp directory

**Validates:** Orphaned index detection, safe removal, root field extraction from binary index files.

---

### T19b: `cleanup --dir` — Remove indexes for a specific directory

**Setup:**

```powershell
# Create a temp directory and build indexes for it
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
- stderr: `Removed index for dir '...' ...` (one line per removed file)
- stderr: `Removed N index file(s) for '...'.`
- After cleanup, `search-index info` should NOT list the temp directory
- Indexes for other directories remain untouched

**Validates:** Targeted index cleanup by directory, case-insensitive path comparison, preservation of unrelated indexes.

**Note:** The E2E test script (`e2e-test.ps1`) automatically runs `cleanup --dir $TestDir` at the end of the test run to remove indexes created during testing.

---

### T19c: `info` — Error reporting for missing index file

**Setup:**

```powershell
# Find the content index file and delete it
$idxDir = "$env:LOCALAPPDATA\search-index"
$cidxFile = Get-ChildItem $idxDir -Filter *.word-search | Select-Object -First 1
$backupPath = "$cidxFile.bak"
Move-Item $cidxFile.FullName $backupPath
```

**Command:**

```powershell
cargo run -- info
```

**Expected:**

- Exit code: 0
- stderr/stdout: Warning message indicates file not found (not a silent skip)
- Other valid indexes still listed normally

**Cleanup:**

```powershell
Move-Item $backupPath $cidxFile.FullName
```

**Validates:** `info` command reports a clear warning when an index file is missing (e.g., deleted externally), rather than silently ignoring it.

---

### T19d: `info` — Error reporting for corrupt index file

**Setup:**

```powershell
# Find the content index file and overwrite with garbage
$idxDir = "$env:LOCALAPPDATA\search-index"
$cidxFile = Get-ChildItem $idxDir -Filter *.word-search | Select-Object -First 1
$backupPath = "$cidxFile.bak"
Copy-Item $cidxFile.FullName $backupPath
Set-Content -Path $cidxFile.FullName -Value "THIS_IS_GARBAGE_DATA_NOT_A_VALID_INDEX" -Encoding Byte
```

**Command:**

```powershell
cargo run -- info
```

**Expected:**

- Exit code: 0
- stderr/stdout: Warning message indicates deserialization failed for the corrupt file
- Other valid indexes still listed normally

**Cleanup:**

```powershell
Move-Item $backupPath $cidxFile.FullName -Force
```

**Validates:** `info` command reports a clear warning when an index file is corrupt (deserialization failure), rather than crashing or silently skipping it.

---

### T19e: `info` — Normal operation with valid indexes

**Command:**

```powershell
# Ensure indexes are built first
cargo run -- index -d $TEST_DIR
cargo run -- content-index -d $TEST_DIR -e $TEST_EXT
cargo run -- info
```

**Expected:**

- Exit code: 0
- stderr: `Index directory: ...`
- stdout: list of `[FILE]` and `[CONTENT]` entries with age, size, staleness
- No warnings or errors about missing/corrupt files

**Validates:** `info` command shows index statistics normally when all indexes are valid (baseline/regression test for T19c and T19d).

---

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

**Note:** For `.rs` files, 0 definitions is expected (parser supports C#, TypeScript/TSX, and SQL only).
For C# or TypeScript projects, expect hundreds/thousands of definitions.
For `.sql` files, definitions include stored procedures, tables, views, functions, types, and indexes (regex-based parser).

---

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

### T25: `serve` — MCP server starts and responds to initialize

**Command:**

```powershell
$init = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}'
echo $init | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `"result"` containing `"serverInfo"` and `"capabilities"`
- Response includes `"tools"` capability

**Validates:** MCP server startup, JSON-RPC initialize handshake.

---

### T26: `serve` — MCP tools/list returns all tools

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with 15 tools: `search_grep`, `search_find`, `search_fast`, `search_info`, `search_reindex`, `search_reindex_definitions`, `search_definitions`, `search_callers`, `search_help`, `search_git_history`, `search_git_diff`, `search_git_authors`, `search_git_activity`, `search_git_blame`, `search_branch_status`
- Each tool has `name`, `description`, `inputSchema`
- `search_definitions` inputSchema includes `includeBody` (boolean), `maxBodyLines` (integer), and `maxTotalBodyLines` (integer) parameters
- Git tools have `repo` (required) and date filter parameters

**Validates:** Tool discovery, tool schema generation, `search_definitions` schema includes body-related parameters.

---

### T27: `serve` — MCP search_grep via tools/call

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"tokenize"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `"result"` containing search results
- Result content includes `files` array and `summary` object
- `summary.totalFiles` > 0

**Validates:** MCP tool dispatch, search_grep handler, JSON-RPC tools/call.

---

### T27a: `serve` — search_grep with `showLines: true` (compact grouped format)

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"<some_known_token>","showLines":true,"contextLines":2,"maxResults":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with search results
- Each file object contains a `"lineContent"` array
- Each element in `lineContent` is a group with:
  - `"startLine"` (integer, 1-based) — first line number in the group
  - `"lines"` (string array) — source code lines in order
  - `"matchIndices"` (integer array, 0-based, optional) — indices within `lines` where matches occur
- Groups are separated when there are gaps in line numbers
- No old-format fields (`line`, `text`, `isMatch`) are present

**Validates:** `showLines` returns compact grouped format with `startLine`, `lines[]`, and `matchIndices[]`. Context lines appear around matches.

**Note:** Replace `<some_known_token>` with a token that exists in the indexed codebase.

---

### T27b: `serve` — search_grep phrase search with `showLines: true` (compact grouped format)

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"<some_known_phrase>","phrase":true,"showLines":true,"contextLines":1,"maxResults":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with search results
- Each file object contains a `"lineContent"` array with compact grouped format (same as T27a)
- Phrase search code path produces identical format to token search path

**Validates:** Phrase search path also uses compact grouped `lineContent` format (both code paths produce consistent output).

**Note:** Replace `<some_known_phrase>` with an exact phrase that exists in the indexed codebase.

---

### T28: `serve` — MCP search_definitions (requires --definitions)

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"name":"tokenize"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT --definitions
```

**Expected:**

- stdout: JSON-RPC response with definition results
- For Rust codebase: 0 results (tree-sitter supports C#/TypeScript/SQL only)
- For C# or TypeScript codebase: results with `name`, `kind`, `file`, `lines`
- For SQL codebase: results with `name`, `kind` (storedProcedure, table, view, etc.), `file`, `lines`

**Validates:** search_definitions handler, definition index loading, AST-based search.

**Note:** Requires `--definitions` flag. For `.rs` files, 0 results is expected. For TypeScript files, definition kinds include `function`, `typeAlias`, `variable`, etc. For SQL files, definition kinds include `storedProcedure`, `table`, `view`, `sqlFunction`, `userDefinedType`, `sqlIndex`, `column`.

---

### T28a: `serve` — search_definitions with `includeBody: true`

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"name":"<some_known_def>","includeBody":true}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT --definitions
```

**Expected:**

- stdout: JSON-RPC response with definition results
- Each definition object contains a `"bodyStartLine"` (integer, 1-based) and `"body"` array field (string array of source lines)
- `summary` object includes `"totalBodyLinesReturned"` field

**Validates:** `includeBody` flag causes body content to be returned alongside definitions.

**Note:** Replace `<some_known_def>` with a definition name that exists in the indexed codebase.

---

### T28b: `serve` — search_definitions with `includeBody: true, maxBodyLines: 5`

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"name":"<some_known_long_def>","includeBody":true,"maxBodyLines":5}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT --definitions
```

**Expected:**

- stdout: JSON-RPC response with definition results
- Each definition's `"body"` array has at most 5 entries
- If a definition is longer than 5 lines: `"bodyTruncated": true` and `"totalBodyLines"` present in the definition object

**Validates:** `maxBodyLines` caps per-definition body output, truncation metadata is accurate.

**Note:** Replace `<some_known_long_def>` with a definition that has more than 5 lines of body.

---

### T28c: `serve` — search_definitions backward compatibility (default `includeBody: false`)

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"name":"<some_known_def>"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT --definitions
```

**Expected:**

- stdout: JSON-RPC response with definition results
- Definition objects do NOT contain a `"body"` field — same output as before the feature was added

**Validates:** Backward compatibility — omitting `includeBody` (or defaulting to `false`) produces the original response format.

---

### T28d: `serve` — search_definitions with `containsLine` + `includeBody: true`

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"file":"<known_file>","containsLine":<known_line>,"includeBody":true}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT --definitions
```

**Expected:**

- stdout: JSON-RPC response with definition results
- Result includes `"containingDefinitions"` array
- Each containing definition has a `"bodyStartLine"` (integer, 1-based) and `"body"` array (string array of source lines)

**Validates:** `includeBody` works together with `containsLine` mode, body is attached to containing definitions.

**Note:** Replace `<known_file>` and `<known_line>` with a file path and line number known to be inside a definition.

---

### T28e: `serve` — search_definitions with `maxTotalBodyLines` budget exhaustion

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"parent":"<class_with_many_methods>","includeBody":true,"maxTotalBodyLines":20}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT --definitions
```

**Expected:**

- stdout: JSON-RPC response with definition results
- First few definitions have `"body"` arrays with content
- Later definitions have `"bodyOmitted"` marker (body budget exhausted)
- Total body lines across all definitions ≤ 20

**Validates:** `maxTotalBodyLines` global budget is enforced, definitions beyond the budget get `bodyOmitted`, budget is reported accurately.

**Note:** Replace `<class_with_many_methods>` with a class/parent that has many method definitions in the indexed codebase.

---

### T28f: `serve` — search_definitions by attribute returns no duplicates

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}}}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"attribute":"<attribute_name>","kind":"class"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT --definitions
```

**Expected:**

- stdout: JSON-RPC response with definition results
- No duplicate entries: each class appears at most once, even if it has the same attribute applied multiple times (e.g., `[ServiceProvider]` and `[ServiceProvider("config")]`)
- `totalResults` matches the count of unique definitions in the `definitions` array

**Validates:** Attribute index deduplication — a class with multiple attributes normalizing to the same name (e.g., `Attr` and `Attr("arg")`) is indexed only once per attribute name.

**Note:** Replace `<attribute_name>` with an attribute that some classes use multiple times with different arguments.

---

### T28g: `serve` — search_definitions with `maxResults: 0` (unlimited)

**Scenario:** `maxResults=0` should return ALL matching definitions without capping at 100.
The tool description states `"0 = unlimited"`, and the truncation safety net (`--max-response-kb`)
handles context size protection independently.

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"kind":"method","maxResults":0}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT --definitions
```

**Expected:**

- `summary.totalResults` equals `summary.returned` (no capping at 100)
- `definitions` array contains ALL matching method definitions
- Response may be subject to `--max-response-kb` truncation, but `maxResults=0` itself does not limit results

**Validates:** `maxResults=0` means unlimited — previously bugged to map 0→100.

---

### T29: `serve` — MCP search_callers (requires --definitions)

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"tokenize","depth":2}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT --definitions
```

**Expected:**

- stdout: JSON-RPC response with call tree
- Result includes `callTree` array, `query` object (method, direction, depth), `summary` object (totalNodes, searchTimeMs)
- For Rust codebase: empty callTree (tree-sitter supports C#/TypeScript/SQL only)

**Validates:** search_callers handler end-to-end, call tree building, JSON output format.

**Note:** For C# codebases, use a method name that exists (e.g., `ExecuteQueryAsync`).

---

### T30: `serve` — MCP search_callers with class filter and direction=down

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"tokenize","class":"SomeClass","direction":"down","depth":2}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT --definitions
```

**Expected:**

- stdout: JSON-RPC response with callee tree
- `query.direction` = "down"
- `query.class` = "SomeClass" (class filter passed through)
- Result includes `callTree`, `summary`

**Validates:** class parameter works for direction=down (bug fix), callee tree building.

---

### T31: `serve` — search_callers finds callers through prefixed fields (C# only)

**Command (C# codebase with field naming like `m_orderProcessor` or `_userService`):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"<MethodName>","class":"<ClassName>","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext cs --definitions
```

**Expected:**

- `callTree` includes callers from files that reference the class only through a prefixed field (e.g., `m_className`, `_className`, `s_className`)
- Uses trigram index for substring matching in the `parent_file_ids` filter
- If trigram index is not built (e.g., fresh startup, never used `substring` search), callers through prefixed fields may be missed — this is expected (no crash, no regression)

**Validates:** Fix for field-prefix bug where `m_orderProcessor.SubmitAsync()` was missed because `m_orderprocessor` token ≠ `orderprocessor` token. Trigram substring matching in `collect_substring_file_ids()`.

---

### T32: `serve` — search_callers works with multi-extension `--ext` flag

**Command (server started with `--ext cs,csproj,xml,config`):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"<MethodName>","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext cs,csproj,xml,config --definitions
```

**Expected:**

- `callTree` is NOT empty (if the method exists and has callers)
- Files with `.cs` extension are NOT filtered out despite `--ext` containing multiple comma-separated extensions
- Previously this was broken: ext_filter compared `"cs"` against the entire string `"cs,csproj,xml,config"` → no match → all files filtered out

**Validates:** Fix for ext_filter comma-split bug. `build_caller_tree` and `build_callee_tree` now split ext_filter on commas before comparing.

---

### T33: `serve` — search_grep with `substring: true` (basic)

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"tokeniz","substring":true}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with search results
- Result content includes files containing tokens that have `tokeniz` as a substring (e.g., `tokenize`)
- Result includes `matchedTokens` field listing matched index tokens
- `summary.totalFiles` > 0

**Validates:** Substring search via trigram index, `matchedTokens` in response.

**Status:** ✅ Implemented (covered by `e2e_substring_search_full_pipeline` unit test)

---

### T34: `serve` — search_grep with `substring: true` + short query warning

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"fn","substring":true}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with search results
- Result includes a `"warning"` field about short substring queries (<4 chars)

**Validates:** Short query warning for substring search.

**Status:** ✅ Implemented (covered by `e2e_substring_search_short_query_warning` unit test)

---

### T35: `serve` — search_grep with `substring: true` + `showLines: true`

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"tokeniz","substring":true,"showLines":true,"maxResults":2}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with search results
- Each file object contains a `"lineContent"` array with compact grouped format
- Lines contain the matched substring

**Validates:** Substring search combined with `showLines`.

**Status:** ✅ Implemented (covered by `e2e_substring_search_with_show_lines` unit test)

---

### T36: `serve` — search_grep `substring: true` mutually exclusive with `regex`

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"test","substring":true,"regex":true}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC error response indicating `substring` and `regex` are mutually exclusive

**Validates:** Mutual exclusivity between substring and regex modes.

**Status:** ✅ Implemented (covered by `e2e_substring_mutually_exclusive_with_regex` unit test)

---

### T37: `serve` — search_grep `substring: true` mutually exclusive with `phrase`

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"pub fn","substring":true,"phrase":true}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC error response indicating `substring` and `phrase` are mutually exclusive

**Validates:** Mutual exclusivity between substring and phrase modes.

**Status:** ✅ Implemented (covered by `e2e_substring_mutually_exclusive_with_phrase` unit test)

---

### T37a: `serve` — search_grep defaults to substring mode (no explicit param)

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"tokenize"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `searchMode` containing `"substring"` (not `"or"` or `"and"`)
- Results should include compound token matches (e.g., `"tokenize_basic"` if present)

**Validates:** `substring` defaults to `true` when no explicit `substring` parameter is passed. This ensures compound C# identifiers (e.g., `IStorageIndexManager`, `m_storageIndexManager`) are always found without the LLM needing to remember to pass `substring: true`.

**Status:** ✅ Implemented (covered by `test_substring_default_finds_compound_identifiers` unit test + T28 in e2e-test.ps1)

---

### T37b: `serve` — regex auto-disables substring (no error)

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":".*stale.*","regex":true}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with search results (NOT an error)
- `searchMode` should NOT contain `"substring"` (regex is used instead)

**Validates:** When `regex: true` is passed without explicit `substring: false`, substring is auto-disabled (not an error). Only explicit `substring: true` + `regex: true` should error.

**Status:** ✅ Implemented (covered by `test_regex_auto_disables_substring` unit test + T29 in e2e-test.ps1)

---

### T37c: `serve` — search_grep substring AND-mode correctness (no false positives from multi-token match)

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"service,controller","substring":true,"mode":"and"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- Results only include files that contain tokens matching BOTH `service` AND `controller` as substrings
- A file containing only `userservice`, `servicehelper`, `servicemanager` (3 tokens matching `service`) but NO token matching `controller` must NOT appear in results
- Previously, `terms_matched` was incremented per matching token (not per search term), so a file with 3 `service`-matching tokens would get `terms_matched=3`, falsely passing the AND filter `terms_matched >= 2`

**Validates:** Fix for AND-mode correctness bug in substring search. `terms_matched` now counts distinct search terms, not matching tokens.

**Status:** ✅ Implemented (covered by `test_substring_and_mode_no_false_positive_from_multi_token_match` unit test)

---

### T37d: `serve` — search_grep phrase post-filter for raw content matching

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"</Property> </Property>","phrase":true}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext xml
```

**Expected:**

- Only files containing the **literal** string `</Property> </Property>` are returned
- Files that contain `property property` as tokenized matches (but not the literal XML) are filtered out
- `searchMode` is `"phrase"`

**Validates:** Phrase raw content matching. When the original phrase contains non-alphanumeric characters (XML tags, angle brackets, etc.), the search uses direct case-insensitive substring matching against raw file content instead of the tokenized phrase regex. This eliminates false positives from tokenizer stripping punctuation.

**Status:** ✅ Implemented (covered by `test_phrase_postfilter_xml_literal_match`, `test_phrase_postfilter_no_punctuation_no_filter`, `test_phrase_postfilter_angle_brackets` unit tests)

---

### T38: `serve` — search_reindex rebuilds trigram index

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_reindex","arguments":{}}}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"tokeniz","substring":true}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- Reindex response: success
- Subsequent substring search: works correctly, `totalFiles` > 0

**Validates:** Reindex flow rebuilds trigram index alongside content index.

**Status:** ✅ Implemented (covered by `e2e_reindex_rebuilds_trigram` unit test)

**Note (Sprint 2):** Trigram index rebuild now uses double-check locking — the trigram is built under a read lock, then swapped under a brief write lock. This eliminates contention during concurrent substring searches while the trigram index is being rebuilt. No behavioral change; same substring search results.

---

### T39: `serve` — MCP initialize includes `instructions` field

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- JSON-RPC response `result` contains `instructions` field (string)
- `instructions` mentions `search_fast`, `search_find`, `substring`, `search_callers`, `class`, `includeBody`, `countOnly`
- Provides LLM-readable best practices for tool selection

**Validates:** MCP server-level instructions for LLM tool selection guidance.

**Status:** ✅ Implemented (covered by `test_initialize_includes_instructions` unit test)

---

### T39a: `serve` — MCP initialize instructions adapt to `--ext` configuration

**Command (server with --ext sql only):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext sql
```

**Expected:**

- JSON-RPC response `result` contains `instructions` field (string)
- `instructions` contains `"NEVER READ .sql FILES DIRECTLY"` (only sql, not cs/ts/tsx)
- `instructions` does NOT contain `.cs` or `.ts` or `.tsx` in the NEVER READ line

**Command (server with --ext xml — no definition parsers):**

```powershell
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext xml
```

**Expected:**

- `instructions` does NOT contain `"NEVER READ"`
- `instructions` contains `"search_definitions is not available"` fallback note

**Command (server with --ext cs,ts,sql):**

```powershell
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext cs,ts,sql
```

**Expected:**

- `instructions` contains `"NEVER READ .cs/.ts/.sql FILES DIRECTLY"` (all three)
- `instructions` contains `"DECISION TRIGGER"` and `"BATCH SPLIT"`

**Validates:** MCP `initialize` instructions dynamically adapt to the server's `--ext` configuration. Only extensions with definition parser support (cs, ts, tsx, sql) appear in the "NEVER READ" rule. Extensions without parsers (xml, json, config, etc.) are excluded.

**Unit tests:** `test_render_instructions_empty_extensions`, `test_render_instructions_single_extension`, `test_initialize_def_extension_filtering`

**Status:** ✅ Implemented (covered by unit tests)

---

### T40: `serve` — MCP search_help returns best practices

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_help","arguments":{}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- JSON response with `bestPractices` array (6 items covering file lookup, substring, call chain, class param, includeBody, countOnly)
- `performanceTiers` object with instant/fast/quick/slow tiers
- `toolPriority` array with recommended tool order

**Validates:** On-demand best practices guide for LLMs.

---

### T41: `grep` — Non-code file search (csproj, xml, config)

**Setup:**

Create a temporary directory with a `.csproj` file:

```powershell
$tmp = New-Item -ItemType Directory -Path "$env:TEMP\search_noncode_test_$(Get-Random)"
@'
<Project Sdk="Contoso.NET.Sdk">
  <ItemGroup>
    <PackageReference Include="Newtonsoft.Json" Version="13.0.3" />
    <PackageReference Include="Serilog" Version="3.1.1" />
  </ItemGroup>
</Project>
'@ | Set-Content "$tmp\TestProject.csproj"
cargo run -- content-index -d $tmp -e csproj
```

**Command:**

```powershell
cargo run -- grep "Newtonsoft.Json" -d $tmp -e csproj
```

**Expected:**

- Exit code: 0
- stdout: `TestProject.csproj` listed as a match
- File contains the NuGet package reference

**Cleanup:**

```powershell
Remove-Item -Recurse -Force $tmp
```

**Validates:** `search_grep` works with non-code file extensions like `.csproj`. Users can search NuGet dependencies, XML configurations, and other non-code files by including the appropriate extension in `--ext`.

---

### T41a: `serve` — MCP search_grep with ext='csproj' override

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"Newtonsoft.Json","ext":"csproj"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $tmp --ext csproj
```

**Expected:**

- JSON-RPC response with matching file(s) containing `Newtonsoft.Json`
- `ext` parameter override filters to `.csproj` files only

**Validates:** MCP `search_grep` `ext` parameter works with non-code extensions.

---

### T41b: `tips` / `search_help` — Non-code file tip present

**Command (CLI):**

```powershell
cargo run -- tips
```

**Expected:**

- Output contains tip about searching non-code file types (XML, csproj, config)
- Mentions `ext='csproj'` or similar example

**Validates:** The new tip for non-code file search is visible in CLI output and MCP `search_help`.

---

### T42: `tips` / `search_help` — Strategy recipes present

**Command (CLI):**

```powershell
cargo run -- tips
```

**Expected:**

- Output contains "STRATEGY RECIPES" section
- Contains "Architecture Exploration" recipe with steps and anti-patterns
- Contains "Call Chain Investigation" recipe
- Contains "Stack Trace / Bug Investigation" recipe

**Command (MCP):**

```powershell
$input = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_help","arguments":{}}}'
) -join "`n"
echo $input | cargo run -- serve -d $TEST_DIR -e $TEST_EXT
```

**Expected:**

- JSON response contains `strategyRecipes` array with 3 entries
- Each recipe has `name`, `when`, `steps`, `antiPatterns` fields

**Validates:** Strategy recipes are exposed in both CLI and MCP outputs.

---

### T42b: `tips` / `search_help` — Query budget and multi-term tips present

**Command (CLI):**

```powershell
cargo run -- tips
```

**Expected:**

- Output contains tip about "Query budget: aim for 3 or fewer search calls"
- Output contains tip about "Multi-term name in search_definitions"
- Multi-term tip mentions comma-separated example: `UserService,IUserService,UserController`

**Validates:** New efficiency guidance tips are visible in CLI output and MCP `search_help`.

---

## SQL Support Tests

### T-SQL-01: `def-index` — Build SQL definition index

**Command:**

```powershell
cargo run -- def-index -d $TEST_DIR -e sql
```

**Expected:**

- Exit code: 0
- stderr: `[def-index] Found N files to parse`
- stderr: `[def-index] Parsed N files in X.Xs, extracted M definitions`
- A `.code-structure` file created
- Definitions include SQL-specific kinds: `storedProcedure`, `table`, `view`, `sqlFunction`, `userDefinedType`, `sqlIndex`, `column`

**Validates:** Regex-based SQL parsing, definition extraction for `.sql` files.

---

### T-SQL-02: `serve` — search_definitions finds SQL stored procedures

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"kind":"storedProcedure"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext sql --definitions
```

**Expected:**

- stdout: JSON-RPC response with definition results
- Results contain SQL stored procedures with `kind: "storedProcedure"`
- Each definition includes `name`, `file`, `lines`, `signature`

**Validates:** `search_definitions` with `kind` filter works for SQL-specific definition kinds.

---

### T-SQL-03: `serve` — search_definitions finds SQL tables

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"kind":"table"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext sql --definitions
```

**Expected:**

- stdout: JSON-RPC response with definition results
- Results contain SQL table definitions with `kind: "table"`
- Each definition includes `name`, `file`, `lines`

**Validates:** `search_definitions` with `kind=table` returns SQL table definitions.

---

### T-SQL-04: `def-index` — SQL file with GO-separated objects

**Setup:**

```powershell
$tmp = New-Item -ItemType Directory -Path "$env:TEMP\search_sql_go_test_$(Get-Random)"
@'
CREATE TABLE dbo.Orders (
    OrderId INT PRIMARY KEY,
    CustomerId INT NOT NULL
);
GO

CREATE PROCEDURE dbo.GetOrders
    @CustomerId INT
AS
BEGIN
    SELECT * FROM dbo.Orders WHERE CustomerId = @CustomerId;
END;
GO

CREATE VIEW dbo.OrderSummary AS
SELECT CustomerId, COUNT(*) AS OrderCount FROM dbo.Orders GROUP BY CustomerId;
GO
'@ | Set-Content "$tmp\schema.sql"
```

**Command:**

```powershell
cargo run -- def-index -d $tmp -e sql
```

**Expected:**

- Exit code: 0
- stderr shows 3+ definitions extracted (table Orders, procedure GetOrders, view OrderSummary)
- Each definition has correct line ranges (not overlapping)

**Cleanup:**

```powershell
cargo run -- cleanup --dir $tmp
Remove-Item -Recurse -Force $tmp
```

**Validates:** SQL parser correctly handles GO-separated batches with multiple object types, assigning correct line ranges to each definition.

---

### T-SQL-05: `serve` — search_callers on SQL table shows stored procedures that reference it

**Setup:**

Create temp `.sql` files with a table and a stored procedure that references it via SELECT/INSERT/UPDATE.

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"Orders","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $tmp --ext sql --definitions
```

**Expected:**

- `callTree` includes stored procedures that reference the `Orders` table via FROM/JOIN/INSERT/UPDATE/DELETE
- Call sites extracted from SQL stored procedure bodies

**Validates:** SQL call-site extraction (EXEC, FROM, JOIN, INSERT, UPDATE, DELETE patterns) enables `search_callers` to find stored procedures that reference SQL tables.

---

### T-SQL-06: `def-index` — Mixed C#/TypeScript/SQL definition index

**Command:**

```powershell
cargo run -- def-index -d $TEST_DIR -e cs,ts,sql
```

**Expected:**

- Exit code: 0
- stderr: `[def-index] Found N files to parse` (N includes `.cs`, `.ts`, and `.sql` files)
- stderr: `[def-index] Parsed N files in X.Xs, extracted M definitions`
- C# definitions (classes, methods), TypeScript definitions (functions, type aliases), and SQL definitions (stored procedures, tables) all present in the same `.code-structure` index

**Validates:** Mixed-language definition indexing including SQL. C# files use tree-sitter, TypeScript files use tree-sitter, SQL files use regex-based parser, and all coexist in the same `.code-structure` index.

---

## TypeScript Support Tests

### T44: `def-index` — Build TypeScript definition index

**Command:**

```powershell
cargo run -- def-index -d $TEST_DIR -e ts
```

**Expected:**

- Exit code: 0
- stderr: `[def-index] Found N files to parse`
- stderr: `[def-index] Parsed N files in X.Xs, extracted M definitions`
- A `.code-structure` file created
- Definitions include TypeScript-specific kinds: `function`, `class`, `interface`, `enum`, `typeAlias`, `variable`

**Validates:** Tree-sitter TypeScript parsing, definition extraction for `.ts` files.

---

### T45: `def-index` — Build TypeScript + TSX definition index

**Command:**

```powershell
cargo run -- def-index -d $TEST_DIR -e ts,tsx
```

**Expected:**

- Exit code: 0
- stderr: `[def-index] Found N files to parse` (N includes both `.ts` and `.tsx` files)
- stderr: `[def-index] Parsed N files in X.Xs, extracted M definitions`
- Definitions extracted from both `.ts` and `.tsx` files

**Validates:** Mixed `.ts` + `.tsx` extension handling in definition indexing. TSX files are parsed using the TSX grammar.

---

### T46: `serve` — MCP search_definitions finds TypeScript functions

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"kind":"function"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext ts --definitions
```

**Expected:**

- stdout: JSON-RPC response with definition results
- Results contain TypeScript function declarations with `kind: "function"`
- Each definition includes `name`, `file`, `lines`, `signature`

**Validates:** `search_definitions` with `kind` filter works for TypeScript-specific definition kinds.

**Note:** Requires a TypeScript project with function declarations.

---

### T47: `serve` — MCP search_definitions finds TypeScript class by name

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"name":"UserService"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext ts --definitions
```

**Expected:**

- stdout: JSON-RPC response with definition results matching `UserService`
- Result includes class definition with correct file path and line range

**Validates:** Name-based search works for TypeScript definitions.

**Note:** Replace `UserService` with a class name that exists in the TypeScript project.

---

### T48: `serve` — MCP search_definitions finds decorated TypeScript classes

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"attribute":"injectable"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext ts --definitions
```

**Expected:**

- stdout: JSON-RPC response with definition results
- Results contain TypeScript classes decorated with `@Injectable()` or similar decorators
- Decorator names are stored as attributes (lowercased, without `@` prefix)

**Validates:** TypeScript decorator extraction and attribute-based search.

**Note:** Replace `injectable` with a decorator name that exists in the TypeScript project.

---

### T49: `def-index` — Mixed C# + TypeScript definition index

**Command:**

```powershell
cargo run -- def-index -d $TEST_DIR -e cs,ts
```

**Expected:**

- Exit code: 0
- stderr: `[def-index] Found N files to parse` (N includes both `.cs` and `.ts` files)
- stderr: `[def-index] Parsed N files in X.Xs, extracted M definitions`
- Both C# definitions (classes, methods, etc.) and TypeScript definitions (functions, type aliases, etc.) are present in the index

**Validates:** Mixed-language definition indexing. C# files use the C# parser, TypeScript files use the TypeScript parser, and both coexist in the same `.code-structure` index.

---

### T50: `serve` — Incremental TypeScript definition update via watcher

**Scenario:** Start the MCP server with `--watch --definitions` for a TypeScript project. Modify a `.ts` file (add or rename a function). The watcher should detect the change and re-parse the file, updating definitions in-place.

**Command:**

```powershell
# Start server in background
$server = Start-Process -PassThru -NoNewWindow cargo -ArgumentList "run -- serve --dir $TEST_DIR --ext ts --watch --definitions"

# Wait for server to initialize
Start-Sleep -Seconds 3

# Modify a .ts file (add a new function)
Add-Content "$TEST_DIR\some_file.ts" "`nexport function newTestFunction(): void { }"

# Wait for watcher debounce
Start-Sleep -Seconds 2

# Query for the new function
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"name":"newTestFunction"}}}'
) -join "`n"
echo $msgs | & $server.Path
```

**Expected:**

- After file modification, stderr shows watcher detecting the change
- `search_definitions` finds `newTestFunction` with correct file and line info

**Validates:** Incremental definition update for TypeScript files via the file watcher.

**Note:** This is a manual test requiring a running server. Clean up the added function after testing.

---

### T50b: `serve` — Incremental content index update without forward index

**Scenario:** Start the MCP server with `--watch`. Create a new file, wait for watcher debounce, then query for the new token via `search_grep`. Then modify the file (replacing a token), wait again, and verify the old token is gone and the new one is found. This validates the brute-force inverted index purge that replaced the forward index (memory optimization saving ~1.5 GB RAM).

**Command:**

```powershell
# Create temp directory with a test file
$tmpDir = New-TemporaryFile | ForEach-Object { Remove-Item $_; New-Item -ItemType Directory -Path $_ }
Set-Content "$tmpDir\initial.cs" "class OriginalClass { OriginalToken field; }"

# Start server with --watch
$server = Start-Process -PassThru -NoNewWindow search -ArgumentList "serve --dir $tmpDir --ext cs --watch"

# Wait for server to initialize and index
Start-Sleep -Seconds 3

# Query for OriginalToken — should find it
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"OriginalToken"}}}' | & search-index serve --dir $tmpDir --ext cs --watch

# Modify the file — replace OriginalToken with UpdatedToken
Set-Content "$tmpDir\initial.cs" "class OriginalClass { UpdatedToken field; }"

# Wait for watcher debounce
Start-Sleep -Seconds 2

# Query for UpdatedToken — should find it
# Query for OriginalToken — should NOT find it (purged via brute-force scan)
```

**Expected:**

- First query: `search_grep` for `OriginalToken` returns 1 file match
- After modification and debounce: `search_grep` for `UpdatedToken` returns 1 match
- After modification: `search_grep` for `OriginalToken` returns 0 matches (old postings purged)

**Validates:** Incremental content index update via brute-force inverted index purge (no forward index). Ensures the memory optimization (~1.5 GB savings) doesn't break incremental watcher updates.

**Note:** This is a manual test requiring a running server. The behavior is also covered by unit tests: `test_purge_file_from_inverted_index_*`, `test_remove_file_without_forward_index`, `test_update_existing_file_without_forward_index`.

---

### T51: `serve` — TypeScript-specific definition kinds (typeAlias, variable)

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"kind":"typeAlias"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext ts --definitions
```

**Expected:**

- stdout: JSON-RPC response with definitions of kind `typeAlias`
- Results contain TypeScript `type` declarations (e.g., `type Props = { ... }`)

**Command (variable kind):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"kind":"variable"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext ts --definitions
```

**Expected:**

- stdout: JSON-RPC response with definitions of kind `variable`
- Results contain exported `const`/`let`/`var` declarations

**Validates:** TypeScript-specific definition kinds (`typeAlias`, `variable`) are correctly extracted and searchable.

**Note:** Requires a TypeScript project with type aliases and exported variables.

---

### T52: `serve` — Response truncation for `search_definitions` broad queries

**Scenario:** When `search_definitions` returns a large result set (e.g., broad `kind: "property"`
query on a large codebase), the response must be truncated to stay within the `--max-response-kb`
budget. Unlike `search_grep` (which uses Phase 1-4 with its `files` array structure),
`search_definitions` uses a `definitions` array — truncation Phase 5 (generic array fallback)
handles this. The `summary` must include truncation metadata with a definitions-specific hint.

**Command (broad query expected to exceed 16KB):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_definitions","arguments":{"kind":"property","maxResults":500}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext cs,ts,tsx --definitions --metrics 2>$null
```

**Expected (if > 16KB):**

- `summary.responseTruncated` = `true`
- `summary.truncationReason` contains `"truncated 'definitions' array"`
- `summary.returned` matches actual `definitions` array length (not the pre-truncation count)
- `summary.totalResults` reflects the full match count (before both `maxResults` and Phase 5 truncation)
- `summary.hint` mentions `"name, kind, file, or parent filters"` (NOT `"countOnly"`)
- `summary.originalResponseBytes` > response budget

**Negative test — narrow query stays under budget:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_definitions","arguments":{"name":"truncate_large_response","kind":"method"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir . --ext rs --definitions --metrics 2>$null
```

**Expected:**

- `summary.responseTruncated` is absent
- `definitions` array contains the full result set

**Validates:** Phase 5 generic array truncation for non-grep response formats, definitions-specific hint, `returned` count accuracy after truncation.

**Note:** Requires a large enough codebase with 500+ properties to trigger truncation. If the test codebase is small, increase `maxResults` or use `--max-response-kb 4` to lower the budget.

---

## TypeScript Callers Tests

### T53: `serve` — search_callers finds TypeScript class method callers

**Command (TypeScript codebase):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"<MethodName>","class":"<ClassName>","depth":2}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext ts --definitions
```

**Expected:**

- stdout: JSON-RPC response with call tree
- `callTree` includes callers from TypeScript files where `this.service.method()` pattern is used
- Caller entries have correct `class` (receiver type resolved from field type map)
- `query.method` matches the requested method name

**Validates:** TypeScript call-site extraction for class method calls via `this.field.method()` pattern. Receiver type is resolved through the field type map built from class fields and constructor parameter properties.

**Note:** Replace `<MethodName>` and `<ClassName>` with a method/class that exists in the TypeScript project.

---

### T54: `serve` — search_callers finds TypeScript standalone function calls

**Command (TypeScript codebase with standalone functions):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"<functionName>","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext ts --definitions
```

**Expected:**

- stdout: JSON-RPC response with call tree
- `callTree` includes callers that invoke the standalone function
- Callers may include both `DefinitionKind::Function` and `DefinitionKind::Method` entries
- Standalone function calls have no receiver type (bare `functionName()` calls)

**Validates:** TypeScript standalone function call-site extraction. Functions are recognized as valid "containing method" entries in the caller tree (via `DefinitionKind::Function` support in `find_containing_method`).

**Note:** Standalone function calls without a receiver may be ambiguous — the callers tool finds them by method name grep, not by import resolution.

---

### T55: `serve` — search_callers with `ext` parameter filters by language

**Command (mixed C#/TypeScript codebase):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"<MethodName>","ext":"ts","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext cs,ts --definitions
```

**Expected:**

- stdout: JSON-RPC response with call tree
- All results are from `.ts` files only (no `.cs` files)
- `ext` parameter filters both the grep search and the definition lookups

**Validates:** `ext` parameter on `search_callers` can filter results to a specific language in a mixed-language project.

**Note:** Server must be started with `--ext cs,ts` to index both languages. The `ext` parameter in the tool call narrows results to TypeScript only.

---

### T56: `serve` — search_callers finds callers in TypeScript arrow function class properties

**Command (TypeScript codebase with arrow function class properties):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"<MethodName>","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext ts --definitions
```

**Expected:**

- stdout: JSON-RPC response with call tree
- `callTree` includes callers from arrow function class properties (e.g., `processItem = (item: Item): void => { this.validate(item); }`)
- The arrow function property is treated as a method for call-site extraction purposes

**Validates:** Arrow function class properties (`public_field_definition` with `arrow_function` initializer) are recognized as call-site sources. Call sites within arrow function bodies are extracted correctly.

**Note:** Replace `<MethodName>` with a method that is called from within an arrow function class property.

---

### T57: `serve` — search_callers tracks TypeScript `new` expression constructor calls

**Command (TypeScript codebase with constructor calls):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"<ClassName>","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext ts --definitions
```

**Expected:**

- stdout: JSON-RPC response with call tree
- `callTree` includes callers that use `new ClassName(...)` expressions
- Constructor calls have `receiver_type` matching the class name

**Validates:** TypeScript `new_expression` nodes are extracted as call sites. `new UserService(logger)` is tracked as a call to `UserService` with receiver type `UserService`.

**Note:** Replace `<ClassName>` with a class that is instantiated via `new` in the TypeScript project.

### T58: `serve` — search_callers resolves Angular `inject()` field types

**Command (Angular/TypeScript codebase with `inject()` usage):**

```powershell
@(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"<MethodName>","depth":1}}}'
) -join "`n"
```

Replace `<MethodName>` with a method called on an `inject()`-resolved field (e.g., `dispatch` if `this.store = inject(Store)` and `this.store.dispatch()` is called).

**Expected:**

- stdout: JSON-RPC response with call tree
- `callTree` includes callers where the receiver type is resolved from `inject(ClassName)` patterns
- Two patterns are supported:
  - **Field initializer**: `private store = inject(Store);` → `this.store.dispatch()` resolves receiver to `Store`
  - **Constructor assignment**: `this.router = inject(Router);` → `this.router.navigate()` resolves receiver to `Router`
- Generic type arguments are stripped: `inject(Store<AppState>)` → receiver type is `Store`

**Validates:** Angular `inject()` function support for field type resolution. The TypeScript parser extracts `inject(ClassName)` calls from both field initializers and constructor assignments, adding them to the per-class field type map used by `resolve_ts_receiver_type()`.

**Note:** Replace `<MethodName>` with a method that is called on an `inject()`-resolved field in the Angular/TypeScript project.

---

### T59: `serve` — search_callers ambiguity warning truncated for common methods

**Command (codebase with many classes implementing the same method, e.g., Angular `ngOnInit`):**

```powershell
@(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"ngOnInit"}}}'
) -join "`n"
```

**Expected:**

- stdout: JSON-RPC response with call tree and a `"warning"` field
- Warning mentions total number of classes (e.g., "found in 1899 classes")
- Warning lists at most 10 class names followed by "…" (truncated)
- Warning does NOT list all classes — total warning length stays under ~500 bytes regardless of how many classes contain the method
- Warning advises using the `class` parameter to scope the search

**Validates:** Ambiguity warning is truncated when a method name (without `class` filter) matches many classes. Previously, the warning listed all class names, producing ~56KB responses (~14K tokens) for common methods like `ngOnInit`.

**Status:** ✅ Implemented (covered by `test_search_callers_ambiguity_warning_truncated` unit test)

---

## Automation Script

Save as `e2e-test.ps1` and run from workspace root:

```powershell
#!/usr/bin/env pwsh
param(
    [string]$TestDir = ".",
    [string]$TestExt = "rs",
    [string]$Binary = "cargo run --"
)

$ErrorActionPreference = "Stop"
$passed = 0
$failed = 0
$total = 0

function Run-Test {
    param([string]$Name, [string]$Command, [int]$ExpectedExit = 0, [string]$StderrContains = "", [string]$StdoutContains = "")

    $script:total++
    Write-Host -NoNewline "  $Name ... "

    $result = Invoke-Expression "$Command 2>&1"
    $exitCode = $LASTEXITCODE

    $output = $result -join "`n"

    if ($exitCode -ne $ExpectedExit) {
        Write-Host "FAILED (exit=$exitCode, expected=$ExpectedExit)" -ForegroundColor Red
        $script:failed++
        return
    }

    if ($StdoutContains -and -not ($output -match [regex]::Escape($StdoutContains))) {
        Write-Host "FAILED (output missing: $StdoutContains)" -ForegroundColor Red
        $script:failed++
        return
    }

    Write-Host "OK" -ForegroundColor Green
    $script:passed++
}

Write-Host "`n=== E2E Tests (dir=$TestDir, ext=$TestExt) ===`n"

# Build first
Write-Host "Building..."
& cargo build 2>$null
if ($LASTEXITCODE -ne 0) { Write-Host "Build failed!" -ForegroundColor Red; exit 1 }

# T01-T05: find
Run-Test "T01 find-filename"       "$Binary find main -d $TestDir -e $TestExt"
Run-Test "T02 find-contents"       "$Binary find `"fn main`" -d $TestDir -e $TestExt --contents"
Run-Test "T04 find-case-insensitive" "$Binary find CONTENTINDEX -d $TestDir -e $TestExt --contents -i"
Run-Test "T05 find-count"          "$Binary find fn -d $TestDir -e $TestExt --contents -c"

# T06-T09: index + fast
Run-Test "T06 index-build"         "$Binary index -d $TestDir"
Run-Test "T07 fast-search"         "$Binary fast main -d $TestDir -e $TestExt"

# T10: content-index
Run-Test "T10 content-index"       "$Binary content-index -d $TestDir -e $TestExt"

# T11-T18: grep
Run-Test "T11 grep-single"         "$Binary grep tokenize -d $TestDir -e $TestExt"
Run-Test "T12 grep-multi-or"       "$Binary grep `"tokenize,posting`" -d $TestDir -e $TestExt"
Run-Test "T13 grep-multi-and"      "$Binary grep `"tokenize,posting`" -d $TestDir -e $TestExt --all"
Run-Test "T14 grep-regex"          "$Binary grep `".*stale.*`" -d $TestDir -e $TestExt --regex"
Run-Test "T15 grep-phrase"         "$Binary grep `"pub fn`" -d $TestDir -e $TestExt --phrase"
Run-Test "T16 grep-context"        "$Binary grep is_stale -d $TestDir -e $TestExt --show-lines -C 2 --max-results 2"
Run-Test "T17 grep-exclude"        "$Binary grep ContentIndex -d $TestDir -e $TestExt --exclude-dir bench"
Run-Test "T18 grep-count"          "$Binary grep fn -d $TestDir -e $TestExt -c"
Run-Test "T24 grep-before-after"   "$Binary grep is_stale -d $TestDir -e $TestExt --show-lines -B 1 -A 3"

# T19: info
Run-Test "T19 info"                "$Binary info"

# T20: def-index
Run-Test "T20 def-index"           "$Binary def-index -d $TestDir -e $TestExt"

# T21-T23: error handling
Run-Test "T21 invalid-regex"       "$Binary grep `"[invalid`" -d $TestDir -e $TestExt --regex" -ExpectedExit 1
Run-Test "T22 nonexistent-dir"     "$Binary find test -d /nonexistent/path/xyz" -ExpectedExit 1

# T20b: def-index with TypeScript (T49 — mixed C#/TS)
# Only runs if TestExt includes ts/tsx or if we detect .ts files
Run-Test "T49 def-index-ts"        "$Binary def-index -d $TestDir -e ts"

# T25-T52: serve (MCP)
# Note: MCP tests require piping JSON-RPC to stdin, which is hard to automate in simple PowerShell.
# These are manual verification tests — run them individually per the test plan.
# Includes: T25-T30 (grep/find MCP), T42 (grep truncation), T44-T51 (TypeScript definitions),
#           T52 (definitions truncation Phase 5).
Write-Host "  T25-T52: MCP serve tests — run manually (see e2e-test-plan.md)"

Write-Host "`n=== Results: $passed passed, $failed failed, $total total ===`n"
if ($failed -gt 0) { exit 1 }
```

**Usage:**

```powershell
# Default (current workspace, .rs files)
./e2e-test.ps1

# Custom directory
./e2e-test.ps1 -TestDir "C:\Projects\MyApp" -TestExt "cs"

# With release binary
./e2e-test.ps1 -Binary "./target/release/search"
```

---

## Test Parallelization

The E2E test script (`e2e-test.ps1`) uses **`Start-Job`** to run independent MCP tests in parallel, reducing total execution time by ~50%.

### Test Classification

| Group | Tests | Parallelizable | Reason |
|-------|-------|---------------|--------|
| **Sequential CLI** | T01-T22, T24, T42/T42b, T49, T54, T61-T64, T65(fast), T76, T80, T82 | ❌ No | Share index files in `%LOCALAPPDATA%/search-index/` for current directory |
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

---

## When to Run

- ✅ After every major refactoring or structural change
- ✅ After dependency upgrades (`cargo update`)
- ✅ Before creating a PR
- ✅ After merging a large PR
- ✅ When switching Rust toolchain versions

### T30: `serve` — MCP search_grep with subdirectory `dir` parameter

**Scenario:** When the MCP server is started with `--dir C:\Repos\MainProject`, a `search_grep` call
with `dir` set to a subdirectory (e.g., `C:\Repos\MainProject\Backend\Services`) should succeed and
return only files within that subdirectory. Previously this returned an error.

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"main","dir":"src/mcp"}}}'
) -join "`n"
$msgs | cargo run -- serve -d . -e rs 2>$null
```

**Expected:**

- No error about "For other directories, start another server instance"
- Results contain only files whose path includes `src/mcp`
- `summary.totalFiles` ≥ 1

**Negative test — directory outside server dir:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"main","dir":"Z:\\other\\path"}}}'
) -join "`n"
$msgs | cargo run -- serve -d . -e rs 2>$null
```

**Expected:**

- Response contains error: "Server started with --dir"
- Tool result `isError: true`

---

### T42: `serve` — Response size truncation for broad queries

**Scenario:** When a search query returns massive results (e.g., short substring query matching
thousands of files), the MCP server automatically truncates the JSON response to stay within
~32KB to prevent filling the LLM context window. Truncation is progressive:

1. Cap `lines` arrays per file to 10 entries
2. Remove `lineContent` blocks
3. Cap `matchedTokens` to 20 entries
4. Remove `lines` arrays entirely
5. Reduce file count

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"fn","substring":true}}}'
) -join "`n"
$msgs | cargo run -- serve -d . -e rs --metrics 2>$null
```

**Expected:**

- `summary.responseTruncated` = `true`
- `summary.truncationReason` contains truncation phases applied
- `summary.originalResponseBytes` > 32768
- `summary.responseBytes` ≤ ~33000 (under budget with small metadata overhead)
- `summary.hint` contains advice to use `countOnly` or narrow filters
- `summary.totalFiles` and `summary.totalOccurrences` reflect the FULL result set (not truncated)
- The `files` array is reduced from 50 to a smaller number

**Negative test — small query is NOT truncated:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"truncate_large_response"}}}'
) -join "`n"
$msgs | cargo run -- serve -d . -e rs --metrics 2>$null
```

**Expected:**

- `summary.responseTruncated` is absent (response under budget)
- `summary.responseBytes` < 32768

**Validates:** Progressive response truncation, LLM context budget protection, summary metadata accuracy.

---

### T43: `serve` — search_find directory validation (security)

**Scenario:** The `search_find` tool now validates the `dir` parameter against `server_dir`,
matching the same security behavior as `search_grep`. Previously, `search_find` accepted any
directory path, allowing filesystem enumeration outside the server's configured scope.

**Test — directory outside `server_dir` is rejected:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_find","arguments":{"pattern":"*","dir":"C:\\Windows"}}}'
) -join "`n"
$msgs | cargo run -- serve -d . -e rs 2>$null
```

**Expected:**

- Response contains error indicating directory is outside allowed scope
- Tool result `isError: true`
- Error message references `--dir` / `server_dir`

**Test — subdirectory of `server_dir` is accepted:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_find","arguments":{"pattern":"*.rs","dir":"src/mcp"}}}'
) -join "`n"
$msgs | cargo run -- serve -d . -e rs 2>$null
```

**Expected:**

- No error
- Results contain file paths within `src/mcp`
- Normal `search_find` output with match count

**Test — no `dir` parameter uses `server_dir` as default:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_find","arguments":{"pattern":"*.rs"}}}'
) -join "`n"
$msgs | cargo run -- serve -d . -e rs 2>$null
```

**Expected:**

- No error
- Results returned from the server's root directory
- Normal `search_find` output

**Validates:** `search_find` directory validation parity with `search_grep`, preventing filesystem enumeration outside allowed scope.

**Status:** ✅ Implemented (covered by `test_validate_search_dir_subdirectory` and `test_validate_search_dir_outside_rejects` unit tests)

---

### T-UTF16: UTF-16 BOM detection in `read_file_lossy()`

**Background:** Files encoded in UTF-16LE or UTF-16BE (with BOM) were read as lossy UTF-8, producing garbled content. Tree-sitter received garbage instead of valid source code, resulting in 0 definitions for affected files. The fix adds BOM detection to `read_file_lossy()`.

**Setup:**

```powershell
# Create a temp directory with a UTF-16LE encoded .cs file
$testDir = "$env:TEMP\search_e2e_utf16"
New-Item -ItemType Directory -Force -Path $testDir | Out-Null
$content = @"
using System;
namespace TestApp
{
    public class HtmlLexer
    {
        public void Parse() { }
    }
}
"@
# Write as UTF-16LE (PowerShell's Unicode encoding = UTF-16LE with BOM)
[System.IO.File]::WriteAllText("$testDir\HtmlLexer.cs", $content, [System.Text.Encoding]::Unicode)
```

**Command:**

```powershell
cargo run -- def-index --dir $testDir --ext cs
```

**Expected:**

- Exit code: 0
- stderr contains: `extracted` with a non-zero definition count (≥ 2: class + method)
- stderr does NOT contain: `WARNING: file contains non-UTF8 bytes` (file is successfully decoded via BOM, not lossy)
- stderr: `0 lossy-utf8` (UTF-16 files are no longer lossy)

**Verify definitions are indexed:**

```powershell
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}' | cargo run -- serve --dir $testDir --ext cs --definitions 2>$null
```

Then send `search_definitions` with `name: "HtmlLexer"` — should return the class definition.

**Cleanup:**

```powershell
Remove-Item -Recurse -Force $testDir
```

**Unit tests:** `test_read_file_lossy_utf16le_bom`, `test_read_file_lossy_utf16be_bom`, `test_read_file_lossy_utf8_bom`, `test_read_file_lossy_utf16le_csharp_code`, `test_read_file_lossy_utf16le_unicode_content`, `test_read_file_lossy_empty_file`, `test_read_file_lossy_utf16le_bom_only`, `test_read_file_lossy_single_byte_file`, `test_decode_utf16le_basic`, `test_decode_utf16be_basic`, `test_decode_utf16le_odd_byte_ignored`, `test_decode_utf16le_empty`, `test_decode_utf16be_empty`, `test_read_file_lossy_plain_utf8`, `test_read_file_lossy_invalid_utf8_still_lossy`

---

### T-LOSSY: Non-UTF8 file indexing (lossy UTF-8 conversion)

**Background:** Files with Windows-1252 encoded characters (e.g., smart quotes `'` = byte `0x92` in comments) were previously silently skipped during definition indexing because `std::fs::read_to_string()` requires valid UTF-8. This test verifies that such files are now indexed via lossy UTF-8 conversion.

**Setup:**

```powershell
# Create a temp directory with a .cs file containing a non-UTF8 byte
$testDir = "$env:TEMP\search_e2e_lossy"
New-Item -ItemType Directory -Force -Path $testDir | Out-Null
$bytes = [System.Text.Encoding]::UTF8.GetBytes(@"
using System;
namespace TestApp
{
    // Comment: you
"@)
$bytes += [byte]0x92  # Windows-1252 right single quote
$bytes += [System.Text.Encoding]::UTF8.GetBytes(@"re a dev
    public class DataProcessor
    {
        public void Process() { }
    }
}
"@)
[System.IO.File]::WriteAllBytes("$testDir\Program.cs", $bytes)
```

**Command:**

```powershell
cargo run -- def-index --dir $testDir --ext cs
```

**Expected:**

- Exit code: 0
- stderr contains: `WARNING: file contains non-UTF8 bytes (lossy conversion applied)`
- stderr contains: `1 lossy-utf8 files`
- stderr contains: `extracted` with a non-zero definition count

**Verify definitions are indexed:**

```powershell
# Start MCP server and query
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}' | cargo run -- serve --dir $testDir --ext cs --definitions 2>$null
```

Then send `search_definitions` with `file: "Program.cs"` — should return `DataProcessor` class and `Process` method.

**Cleanup:**

```powershell
Remove-Item -Recurse -Force $testDir
```

---

### T-AUDIT: Definition index audit mode

**Background:** The `search_definitions` tool supports an `audit` parameter that returns index coverage statistics — how many files have definitions, how many are empty, and which suspicious files (large but 0 definitions) may have parsing issues.

**Prerequisites:** Server running with `--definitions` flag

**Command (MCP JSON-RPC):**

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_definitions","arguments":{"audit":true}}}
```

**Expected response structure:**

```json
{
  "audit": {
    "totalFiles": "<number>",
    "filesWithDefinitions": "<number>",
    "filesWithoutDefinitions": "<number>",
    "readErrors": "<number>",
    "lossyUtf8Files": "<number>",
    "suspiciousFiles": "<number>",
    "suspiciousThresholdBytes": 500
  },
  "suspiciousFiles": ["<array of {file, bytes}>"]
}
```

**Assertions:**

- `audit.totalFiles` > 0
- `audit.filesWithDefinitions` > 0
- `audit.filesWithDefinitions` + `audit.filesWithoutDefinitions` ≤ `audit.totalFiles`
- `audit.readErrors` ≥ 0
- `suspiciousFiles` is an array
- Each entry in `suspiciousFiles` has `file` (string) and `bytes` (number > threshold)

**With custom threshold:**

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "search_definitions",
    "arguments": { "audit": true, "auditMinBytes": 10000 }
  }
}
```

Should return fewer suspicious files (only those >10KB with 0 definitions).

---

### T-DEF-AUDIT: Definition index audit CLI command

**Background:** The `search def-audit` CLI subcommand loads a previously built `.code-structure` file from disk and reports index coverage: how many files have definitions, how many are empty, and which suspicious files (large but 0 definitions) may have parsing issues. This does NOT rebuild the index.

**Prerequisites:** A definition index must already be built via `search-index def-index`.

**Command:**

```powershell
# Build first (if not already built)
search-index def-index --dir $TEST_DIR --ext rs

# Audit (instant — loads from disk)
search def-audit --dir $TEST_DIR --ext rs
```

**Expected:**

- Exit code: 0
- stderr contains `[def-audit] Index:` with total files count
- stderr contains `with definitions` count > 0
- stderr contains `without definitions` count ≥ 0
- stderr contains `definitions,` followed by `read errors` and `lossy-UTF8 files`

**With custom threshold:**

```powershell
search def-audit --dir $TEST_DIR --ext rs --min-bytes 10000
```

- Should show fewer suspicious files (only those >10KB with 0 definitions)

**When no index exists:**

```powershell
search def-audit --dir C:\nonexistent --ext cs
```

- stderr contains `No definition index found`
- Exit code: 0

---

### T60: `def-index` — Extension filtering (no unnecessary parsers)

**Purpose:** Verify that definition index only parses files matching requested extensions, and doesn't load TypeScript grammars for C#-only projects.

**Command (C# only):**

```powershell
cargo run -- def-index -d $TEST_DIR -e cs
```

**Expected:**

- Exit code: 0
- stderr: `[def-index] Found N files to parse` — only `.cs` files counted
- No TypeScript grammar loading errors
- Only C# definitions extracted

**Command (C# + TypeScript):**

```powershell
cargo run -- def-index -d $TEST_DIR -e cs,ts,tsx
```

**Expected:**

- Exit code: 0
- Both C# and TypeScript definitions extracted
- TS/TSX parsers created lazily only in threads that encounter TS/TSX files

**Validates:** Extension-based parser filtering prevents unnecessary grammar loading for single-language projects. Fixes performance regression where TypeScript parsers were eagerly loaded for C#-only repositories.

---

### T61: `grep` — Default substring search via trigram index

**Command:**

```powershell
cargo run -- grep "contentindex" -d $TEST_DIR -e $TEST_EXT
```

**Expected:**

- Exit code: 0
- stderr: `Substring 'contentindex' matched N tokens: ...` showing expanded tokens (e.g. `contentindexargs`, `contentindex`)
- stdout: files containing tokens that have `contentindex` as a substring, ranked by TF-IDF
- Mode shown as `SUBSTRING-OR`

**Validates:** CLI grep uses substring search by default (same as MCP), finding compound identifiers automatically.

---

### T62: `grep --all` — Default substring AND mode

**Command:**

```powershell
cargo run -- grep "contentindex,tokenize" -d $TEST_DIR -e $TEST_EXT --all
```

**Expected:**

- Exit code: 0
- Only files containing BOTH `contentindex` (or compound tokens) AND `tokenize` (or compound tokens) are returned
- Mode shown as `SUBSTRING-AND`

**Validates:** CLI default substring search with AND mode correctly requires all terms to match.

---

### T63: `grep --exact` — Exact token matching (opt-out of substring)

**Command:**

```powershell
cargo run -- grep "contentindex" -d $TEST_DIR -e $TEST_EXT --exact
```

**Expected:**

- Exit code: 0
- Mode shown as `OR` (not SUBSTRING)
- Only files containing the exact token `contentindex` are returned (no compound matches like `contentindexargs`)

**Validates:** `--exact` flag disables default substring search and falls back to exact token matching.

---

### T64: `grep --regex` — Regex auto-disables substring

**Command:**

```powershell
cargo run -- grep ".*stale.*" -d $TEST_DIR -e $TEST_EXT --regex
```

**Expected:**

- Exit code: 0
- Mode shown as `REGEX` (not SUBSTRING)
- Results found via regex token matching

**Validates:** `--regex` automatically disables substring mode (no error, no mutual exclusivity check needed).

---

### T-LZ4: LZ4 index compression and backward compatibility

**Background:** All index files (.file-list, .word-search, .code-structure) are now saved with LZ4 frame compression, prefixed by magic bytes `LZ4S`. The loader auto-detects compressed vs legacy uncompressed formats for backward compatibility.

**Test — compressed index roundtrip:**

```powershell
# Build a content index (will be LZ4-compressed)
cargo run -- content-index -d $TEST_DIR -e $TEST_EXT

# Verify the index file starts with LZ4 magic bytes
$idxDir = "$env:LOCALAPPDATA\search-index"
$cidxFile = Get-ChildItem $idxDir -Filter *.word-search | Select-Object -First 1
$bytes = [System.IO.File]::ReadAllBytes($cidxFile.FullName)
$magic = [System.Text.Encoding]::ASCII.GetString($bytes[0..3])
if ($magic -ne "LZ4S") { throw "Expected LZ4S magic, got: $magic" }

# Verify grep still works (index loads correctly)
cargo run -- grep "fn" -d $TEST_DIR -e $TEST_EXT
```

**Expected:**

- Index file starts with `LZ4S` magic bytes
- stderr shows compression ratio log: `Saved X.X MB → Y.Y MB (Z.Z× compression)`
- grep returns results (index deserializes correctly after compression)

**Test — backward compatibility with legacy uncompressed index:**

```powershell
# Create a legacy uncompressed index manually (for testing)
# This is covered by unit test `test_load_compressed_legacy_uncompressed`
# which writes raw bincode and verifies load_compressed can read it
```

**Expected:**

- `load_compressed` reads both LZ4-compressed and legacy uncompressed files
- No data loss or deserialization errors

**Validates:** LZ4 compression, magic byte detection, backward compatibility, compression ratio logging.

**Status:** ✅ Covered by unit tests: `test_save_load_compressed_roundtrip`, `test_load_compressed_legacy_uncompressed`, `test_load_compressed_missing_file_returns_none`, `test_compressed_file_smaller_than_uncompressed`

---

### T-ASYNC: Async MCP Server Startup

Tests for the async startup feature that allows the MCP server event loop to start immediately
while indexes are built in the background.

#### T-ASYNC-01: `search_grep` returns "building" message when content index not ready

**Scenario:** MCP server starts without a pre-built content index on disk (first run on a new codebase).

**MCP Request (sent immediately after server process starts):**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "2025-03-26",
    "capabilities": {},
    "clientInfo": { "name": "test", "version": "1.0" }
  }
}
```

**Expected:** Immediate response with `protocolVersion`, `serverInfo`, `capabilities`.

**Then send:**

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": { "name": "search_grep", "arguments": { "terms": "HttpClient" } }
}
```

**Expected:** `isError: true`, message contains "being built in the background".

**Validates:** Server responds to `initialize` immediately, `search_grep` returns friendly error during build.

**Status:** ✅ Covered by unit tests: `test_dispatch_grep_while_content_index_building`

---

#### T-ASYNC-02: `search_definitions` returns "building" message when def index not ready

**MCP Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "search_definitions",
    "arguments": { "name": "UserService" }
  }
}
```

**Expected:** `isError: true`, message contains "being built in the background".

**Status:** ✅ Covered by unit tests: `test_dispatch_definitions_while_def_index_building`

---

#### T-ASYNC-03: `search_callers` returns "building" message when def index not ready

**MCP Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "method": "tools/call",
  "params": {
    "name": "search_callers",
    "arguments": { "method": "GetUserAsync" }
  }
}
```

**Expected:** `isError: true`, message contains "being built in the background".

**Status:** ✅ Covered by unit tests: `test_dispatch_callers_while_def_index_building`

---

#### T-ASYNC-04: `search_fast` returns "building" message when content index not ready

**MCP Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "method": "tools/call",
  "params": { "name": "search_fast", "arguments": { "pattern": "UserService" } }
}
```

**Expected:** `isError: true`, message contains "being built in the background".

**Status:** ✅ Covered by unit tests: `test_dispatch_fast_while_content_index_building`

---

#### T-ASYNC-05: `search_reindex` returns "already building" message during background build

**MCP Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 6,
  "method": "tools/call",
  "params": { "name": "search_reindex", "arguments": {} }
}
```

**Expected:** `isError: true`, message contains "already being built".

**Status:** ✅ Covered by unit tests: `test_dispatch_reindex_while_content_index_building`

---

#### T-ASYNC-06: `search_help` and `search_info` work during index build

**MCP Requests:**

```json
{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"search_help","arguments":{}}}
{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"search_info","arguments":{}}}
```

**Expected:** Both return `isError: false` with valid results, even while index is building.

**Status:** ✅ Covered by unit tests: `test_dispatch_help_works_while_index_building`

---

#### T-ASYNC-07: `search_find` works during index build (uses filesystem walk, not content index)

**MCP Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 9,
  "method": "tools/call",
  "params": { "name": "search_find", "arguments": { "pattern": "main" } }
}
```

**Expected:** `isError: false`, returns file system results (not dependent on content index).

**Status:** ✅ Covered by unit tests: `test_dispatch_find_works_while_index_building`

---

#### T-ASYNC-08: Tools work normally after background build completes

**Scenario:** Wait for background build to complete (check logs for "Content index ready"), then retry search.

**MCP Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 10,
  "method": "tools/call",
  "params": { "name": "search_grep", "arguments": { "terms": "HttpClient" } }
}
```

**Expected:** `isError: false`, returns normal search results.

**Validates:** Background build atomically swaps the index and sets `content_ready` flag.

**Status:** ✅ Covered by existing unit tests (all tests run with `content_ready: true`).

---

#### T-ASYNC-09: Pre-built index loads synchronously (no background build needed)

**Scenario:** Server starts with a pre-built index on disk (normal restart scenario).

**Expected:** Content index loaded from disk (< 3s), `content_ready` set immediately before event loop starts. All tools work on the first request.

**Validates:** Fast path — no background thread spawned when index exists on disk.

**Status:** ✅ Covered by existing unit tests + manual verification.

---

### T-SHUTDOWN: Save-on-shutdown — indexes persist after graceful server stop

**Background:** The MCP server's file watcher applies incremental updates to the in-memory content and definition indexes, but prior to this fix these updates were never saved to disk (only bulk reindex saved). On graceful shutdown (stdin closed), the server now saves both indexes to disk so that incremental changes survive a restart.

**Test — save-on-shutdown preserves incremental watcher updates:**

```powershell
# 1. Create a temp directory with a test file
$dir = "$env:TEMP\search_shutdown_test"
mkdir $dir -Force
Set-Content "$dir\Test.cs" "class Original { }"

# 2. Build initial content index
cargo run -- content-index -d $dir -e cs

# 3. Start server with --watch, pipe initialize request
$initReq = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
$proc = Start-Process cargo -ArgumentList "run -- serve --dir $dir --ext cs --watch" `
    -RedirectStandardInput "$dir\stdin.txt" -RedirectStandardOutput "$dir\stdout.txt" `
    -RedirectStandardError "$dir\stderr.txt" -PassThru -NoNewWindow
Set-Content "$dir\stdin.txt" $initReq
Start-Sleep 3

# 4. Modify the file (watcher picks up the change)
Set-Content "$dir\Test.cs" "class Modified { void Execute() { } }"
Start-Sleep 3  # wait for watcher debounce

# 5. Stop the server (close stdin / kill)
if (!$proc.HasExited) { $proc.Kill(); $proc.WaitForExit(5000) }

# 6. Verify stderr contains save-on-shutdown log
$stderr = Get-Content "$dir\stderr.txt" -Raw
# Expected: "saving indexes before shutdown" and/or "Content index saved on shutdown"
```

**Expected:**

- Server stderr contains `saving indexes before shutdown`
- Server stderr contains `Content index saved on shutdown`
- The `.word-search` file in `%LOCALAPPDATA%\search-index` has a recent modification timestamp

**Validates:** Save-on-shutdown in [`server.rs`](../src/mcp/server.rs:107) persists incremental watcher changes.

**Unit test:** `test_watch_index_survives_save_load_roundtrip` in [`watcher.rs`](../src/mcp/watcher.rs) verifies that watch-mode fields (`forward`, `path_to_id`) survive serialization roundtrip.

**Automated:** Test T-SHUTDOWN in [`e2e-test.ps1`](../e2e-test.ps1) runs this scenario automatically.

---

### T-CTRLC: Graceful shutdown on Ctrl+C (SIGTERM/SIGINT)

**Background:** The MCP server registers a `ctrlc` handler that catches SIGTERM/SIGINT signals and triggers a graceful shutdown: saving indexes to disk and printing a "saving indexes" message to stderr before exiting with code 0. This complements the stdin-close shutdown path (T-SHUTDOWN) with signal-based shutdown.

**Test (manual):**

```powershell
# 1. Start the MCP server
search-index serve --dir C:\Projects --ext cs --watch --definitions

# 2. Wait for the server to finish loading indexes (watch stderr for "ready" messages)

# 3. Press Ctrl+C

# 4. Observe stderr output
```

**Expected:**

- Server prints `saving indexes before shutdown` (or similar) to stderr
- Server exits with code 0 (not a crash/panic)
- Index files on disk have recent modification timestamps (indexes were saved)
- No "thread panicked" or other error messages in stderr

**Validates:** Graceful shutdown via SIGTERM/SIGINT using the `ctrlc` crate. The handler sets a shutdown flag that the event loop checks, triggering index save before exit.

**Note:** This is a **manual test only**. Automated testing of signal handling in PowerShell is unreliable (race conditions between process startup, signal delivery, and output capture make the test flaky). The underlying mechanism is the same as T-SHUTDOWN (save-on-shutdown), which is already automated.

**Status:** Manual verification only. The `ctrlc` handler delegates to the same save logic tested by T-SHUTDOWN.

---

## Unit Test Coverage — Handler-Level Scenarios

The following test scenarios are covered by unit tests in
[`handlers_tests.rs`](../src/mcp/handlers/handlers_tests.rs). They validate MCP handler behavior
with real indexes built from test fixture files (C#, TypeScript, SQL). Each entry documents what
the unit test verifies at the E2E-equivalent level.

### search_grep

#### T65: `search_grep` — Response truncation via small budget

**Tool:** `search_grep`

**Scenario:** When the JSON response exceeds the `max_response_bytes` budget, the server
progressively truncates the response (capping lines, removing lineContent, reducing file count).

**Expected:**

- `summary.responseTruncated` = `true`
- `summary.truncationReason` is present and non-empty
- Response byte size is within the configured budget

**Unit test:** [`test_search_grep_response_truncation_via_small_budget`](../src/mcp/handlers/handlers_tests.rs)

---

#### T66: `search_grep` — SQL extension filter

**Tool:** `search_grep`

**Scenario:** Passing `ext: "sql"` returns only `.sql` files from the index, excluding `.cs` and
`.ts` files even if they contain the same tokens.

**Expected:**

- All files in results have `.sql` extension
- `summary.totalFiles` ≥ 1

**Unit test:** [`test_search_grep_sql_extension_filter`](../src/mcp/handlers/handlers_tests.rs)

---

#### T67: `search_grep` — Phrase search with showLines from SQL files

**Tool:** `search_grep`

**Scenario:** Phrase search with `phrase: true` and `showLines: true` against `.sql` files returns
matching line content in the compact grouped format.

**Expected:**

- Results contain `.sql` files
- Each file has `lineContent` array with `startLine`, `lines[]`, `matchIndices[]`
- Matched lines contain the searched phrase

**Unit test:** [`test_search_grep_phrase_search_with_show_lines`](../src/mcp/handlers/handlers_tests.rs)

---

#### T68: `search_grep` — `maxResults=0` means unlimited

**Tool:** `search_grep`

**Scenario:** Setting `maxResults: 0` returns all matching files without any cap (0 = unlimited).

**Expected:**

- `summary.totalFiles` equals the actual number of files in the `files` array
- No artificial capping at the default limit

**Unit test:** [`test_search_grep_max_results_zero_means_unlimited`](../src/mcp/handlers/handlers_tests.rs)

---

### search_definitions

#### T69: `search_definitions` — Regex name filter

**Tool:** `search_definitions`

**Scenario:** Setting `regex: true` with a name pattern (e.g., `^User.*`) returns only definitions
whose name matches the regex pattern.

**Expected:**

- All returned definitions have names matching the regex
- Non-matching definitions are excluded
- `summary.totalResults` reflects only matching definitions

**Unit test:** [`test_search_definitions_regex_name_filter`](../src/mcp/handlers/handlers_tests.rs)

---

#### T70: `search_definitions` — Audit mode

**Tool:** `search_definitions`

**Scenario:** Setting `audit: true` returns an index coverage report instead of search results.

**Expected:**

- Response contains `audit` object with `totalFiles`, `filesWithDefinitions`, `suspiciousFiles` counts
- `audit.totalFiles` > 0
- `suspiciousFiles` is an array (may be empty)

**Unit test:** [`test_search_definitions_audit_mode`](../src/mcp/handlers/handlers_tests.rs)

---

#### T71: `search_definitions` — `excludeDir` filter

**Tool:** `search_definitions`

**Scenario:** Setting `excludeDir: ["some_dir"]` excludes definitions from files in the specified
directory.

**Expected:**

- No definitions from excluded directories appear in results
- Definitions from other directories are returned normally

**Unit test:** [`test_search_definitions_exclude_dir`](../src/mcp/handlers/handlers_tests.rs)

---

#### T72: `search_definitions` — Combined name + parent + kind filter

**Tool:** `search_definitions`

**Scenario:** Passing `name`, `parent`, and `kind` filters simultaneously returns only definitions
matching ALL three criteria.

**Expected:**

- All returned definitions match the specified name substring
- All returned definitions belong to the specified parent class
- All returned definitions have the specified kind
- `summary.totalResults` reflects only definitions matching all filters

**Unit test:** [`test_search_definitions_combined_name_parent_kind_filter`](../src/mcp/handlers/handlers_tests.rs)

---

#### T73: `search_definitions` — Nonexistent name returns empty

**Tool:** `search_definitions`

**Scenario:** Searching for a name that doesn't exist in the index returns an empty result set.

**Expected:**

- `definitions` array is empty
- `summary.totalResults` = 0
- No error (graceful empty response)

**Unit test:** [`test_search_definitions_nonexistent_name_returns_empty`](../src/mcp/handlers/handlers_tests.rs)

---

#### T74: `search_definitions` — Invalid regex error

**Tool:** `search_definitions`

**Scenario:** Passing `regex: true` with an invalid pattern (e.g., `[invalid`) returns an error.

**Expected:**

- Response is an error (`isError: true`)
- Error message mentions invalid regex

**Unit test:** [`test_search_definitions_invalid_regex_error`](../src/mcp/handlers/handlers_tests.rs)

---

#### T75: `search_definitions` — `kind="struct"` filter

**Tool:** `search_definitions`

**Scenario:** Filtering by `kind: "struct"` returns only struct-kind definitions.

**Expected:**

- All returned definitions have `kind: "struct"`
- No classes, interfaces, methods, or other kinds appear

**Unit test:** [`test_search_definitions_struct_kind`](../src/mcp/handlers/handlers_tests.rs)

---

#### T76: `search_definitions` — `baseType` filter

**Tool:** `search_definitions`

**Scenario:** Filtering by `baseType` (e.g., `"ControllerBase"`) returns only classes that inherit
from or implement the specified type.

**Expected:**

- All returned definitions list the specified base type in their inheritance
- Definitions without that base type are excluded

**Unit test:** [`test_search_definitions_base_type_filter`](../src/mcp/handlers/handlers_tests.rs)

---

#### T77: `search_definitions` — `kind="enumMember"` filter

**Tool:** `search_definitions`

**Scenario:** Filtering by `kind: "enumMember"` returns only enum member definitions.

**Expected:**

- All returned definitions have `kind: "enumMember"`
- No enums, classes, or other kinds appear

**Unit test:** [`test_search_definitions_enum_member_kind`](../src/mcp/handlers/handlers_tests.rs)

---

#### T78: `search_definitions` — File filter slash normalization

**Tool:** `search_definitions`

**Scenario:** The `file` parameter normalizes path separators so both forward slashes (`/`) and
backslashes (`\`) work identically for path filtering. This applies to both the general file filter
and the `containsLine` file filter. `clean_path()` normalizes all stored paths to forward slashes,
and the file filter comparison normalizes user input for defense-in-depth.

**Expected:**

- `file: "src/Services/UserService"` matches paths stored as `src\Services\UserService.cs` (backslash) or `src/Services/UserService.cs` (forward slash)
- `file: "src\Services\UserService"` also matches (backslash input normalized)
- Mixed separators like `src/Services\UserService` also match
- Same normalization works for `containsLine` + `file` combination
- All stored paths in indexes use forward slashes (via `clean_path()`)

**Unit tests:**

- [`test_search_definitions_file_filter_forward_slash`](../src/mcp/handlers/handlers_tests_csharp.rs)
- [`test_search_definitions_file_filter_backslash`](../src/mcp/handlers/handlers_tests_csharp.rs)
- [`test_search_definitions_file_filter_mixed_separators`](../src/mcp/handlers/handlers_tests_csharp.rs)
- [`test_search_definitions_file_filter_no_match`](../src/mcp/handlers/handlers_tests_csharp.rs)
- [`test_search_definitions_contains_line_forward_slash`](../src/mcp/handlers/handlers_tests_csharp.rs)
- [`test_search_definitions_contains_line_backslash`](../src/mcp/handlers/handlers_tests_csharp.rs)
- [`test_search_definitions_contains_line_mixed_separators`](../src/mcp/handlers/handlers_tests_csharp.rs)

---

### search_fast

#### T79: `search_fast` — `dirsOnly` and `filesOnly` filters

**Tool:** `search_fast`

**Scenario:** Setting `dirsOnly: true` returns only directory entries; setting `filesOnly: true`
returns only file entries.

**Expected:**

- `dirsOnly: true` — all results are directories (no file entries)
- `filesOnly: true` — all results are files (no directory entries)
- Both modes return valid results from the file name index

**Unit test:** [`test_search_fast_dirs_only_and_files_only`](../src/mcp/handlers/handlers_tests.rs)

---

#### T80: `search_fast` — Regex mode

**Tool:** `search_fast`

**Scenario:** Setting `regex: true` matches file names using regex patterns instead of substring
matching.

**Expected:**

- File names matching the regex pattern are returned
- Non-matching files are excluded
- `summary.totalMatches` reflects regex-matched entries

**Unit test:** [`test_search_fast_regex_mode`](../src/mcp/handlers/handlers_tests.rs)

---

#### T81: `search_fast` — Empty pattern handled gracefully

**Tool:** `search_fast`

**Scenario:** Passing an empty string as the pattern is handled gracefully without panicking.

**Expected:**

- No panic or crash
- Returns 0 matches (or all entries, depending on implementation)
- Clean response with valid JSON structure

**Unit test:** [`test_search_fast_empty_pattern`](../src/mcp/handlers/handlers_tests.rs)

---

### search_find

#### T82: `search_find` — Combined parameters (countOnly, maxDepth, ignoreCase+regex)

**Tool:** `search_find`

**Scenario:** Tests combined parameter usage:

- `countOnly: true` returns only the match count without file paths
- `maxDepth` limits directory traversal depth
- `ignoreCase: true` + `regex: true` performs case-insensitive regex matching

**Expected:**

- `countOnly` mode: response contains count but no file list
- `maxDepth` mode: results limited to specified directory depth
- `ignoreCase + regex` mode: case-insensitive regex patterns match correctly

**Unit test:** [`test_search_find_combined_parameters`](../src/mcp/handlers/handlers_tests.rs)

---

### search_callers

#### T83: `search_callers` — `excludeDir` and `excludeFile` filters

**Tool:** `search_callers`

**Scenario:** Setting `excludeDir` and `excludeFile` filters on `search_callers` excludes
matching entries from the call tree.

**Expected:**

- Call tree nodes from excluded directories are filtered out
- Call tree nodes from excluded file patterns are filtered out
- Remaining nodes are returned normally

**Unit test:** [`test_search_callers_exclude_dir_and_file`](../src/mcp/handlers/handlers_tests.rs)

---

#### T84: `search_callers` — Cycle detection (direction=down)

**Tool:** `search_callers`

**Scenario:** When tracing callees (`direction: "down"`) through a circular call graph
(A calls B, B calls A), the search completes without infinite loop.

**Expected:**

- Search completes in finite time
- No infinite recursion or stack overflow
- Call tree represents the cycle without duplicating nodes indefinitely

**Unit test:** [`test_search_callers_cycle_detection_down`](../src/mcp/handlers/handlers_tests.rs)

---

### search_reindex_definitions

#### T85: `search_reindex_definitions` — Successful reindex

**Tool:** `search_reindex_definitions`

**Scenario:** Calling `search_reindex_definitions` successfully rebuilds the AST definition index
and returns build metrics.

**Expected:**

- Response contains `status: "ok"`
- Response includes metrics: files parsed, definitions extracted, build time
- Definition index is usable for subsequent `search_definitions` queries

**Unit test:** [`test_reindex_definitions_success`](../src/mcp/handlers/handlers_tests.rs)

---

### search_reindex

#### T86: `search_reindex` — Invalid directory error

**Tool:** `search_reindex`

**Scenario:** Calling `search_reindex` with a non-existent directory parameter returns an error.

**Expected:**

- Response is an error (`isError: true`)
- Error message indicates the directory does not exist or is invalid

**Unit test:** [`test_search_reindex_invalid_directory`](../src/mcp/handlers/handlers_tests.rs)

---

## Additional Test Scenarios (from upstream merge)

#### T-SPEC-AUDIT: `search_definitions` — Audit mode with `.spec.ts` files (0 definitions expected)

**Tool:** `search_definitions` (audit mode)

**Scenario:** In TypeScript projects, `.spec.ts` files typically contain `describe()` and `it()`
blocks which are function _calls_, not syntactic definitions (function declarations, class
declarations, etc.). When running `search_definitions` with `audit: true`, these files are
expected to appear in the audit report with 0 definitions. This is **by-design behavior**, not
a bug or a parsing failure.

**Expected:**

- `.spec.ts` files correctly reported with 0 definitions in the `filesWithoutDefinitions` count
- `describe` and `it` are NOT misclassified as `function` definitions — they are call expressions
- `suspiciousFiles` may include large `.spec.ts` files (>500 bytes with 0 definitions), but this
  is expected and not an indication of a parser bug
- Setting `auditMinBytes` higher (e.g., 10000) can filter out small spec files from suspicious list

**Source:** PBIClients E2E report T49

**Note:** This is a documentation-only scenario clarifying expected behavior. No code fix needed.

---

#### T-REINDEX-SECURITY: `search_reindex` / `search_reindex_definitions` — Invalid or outside directory

**Tool:** `search_reindex`, `search_reindex_definitions`

**Scenario:** Calling `search_reindex` or `search_reindex_definitions` with a `dir` parameter
that either doesn't exist or is outside the allowed `--dir` server scope should return a
descriptive error, not crash or silently succeed.

**Expected:**

- Error response with `isError: true`
- Error message mentions the invalid directory path

**Unit test:** [`test_search_reindex_invalid_directory`](../src/mcp/handlers/handlers_tests.rs)

---

#### T-CALLERS-CYCLE-UP: `search_callers` — Cycle detection (direction=up)

**Scenario:** When tracing callers upward and encountering a call cycle (A→B→A),
the search completes without infinite loop.

**Expected:**

- No infinite recursion or stack overflow
- Call tree represents the cycle without duplicating nodes indefinitely
- `summary.truncatedByBudget` may be true if cycle is deep

**Unit test:** [`test_search_callers_cycle_detection`](../src/mcp/handlers/handlers_tests_csharp.rs)

---

#### T-CALLERS-EXT-COMMA: `search_callers` — Comma-split `ext` filter

**Scenario:** The `ext` parameter in `search_callers` accepts comma-separated extensions
(e.g., `"cs,ts"`) for multi-language call tree analysis.

**Expected:**

- Both C# and TypeScript files are included in the call tree
- Single extension filter (e.g., `"cs"`) excludes other languages

**Unit test:** [`test_search_callers_ext_filter_comma_split`](../src/mcp/handlers/handlers_tests_csharp.rs)

---

#### T-DIR-SECURITY: Directory validation — Security boundary tests (4 tests)

**Scenario:** The `validate_search_dir` function ensures all search operations
stay within the configured server directory. Tests cover subdirectory acceptance,
sibling directory rejection, path traversal rejection, and absolute path rejection.

**Unit tests:**

- [`test_validate_search_dir_subdir_accepted`](../src/mcp/handlers/handlers_tests.rs)
- [`test_validate_search_dir_outside_rejected`](../src/mcp/handlers/handlers_tests.rs)
- [`test_validate_search_dir_path_traversal_rejected`](../src/mcp/handlers/handlers_tests.rs)
- [`test_validate_search_dir_windows_absolute_outside_rejected`](../src/mcp/handlers/handlers_tests.rs)

---

#### T-CALLERS-AMBIGUITY: `search_callers` — Ambiguity warning variants (3 tests)

**Scenario:** When searching for callers without specifying the `class` parameter,
the response includes an ambiguity warning showing which classes contain the method.
The warning is truncated for common methods with many implementations.

**Unit tests:**

- [`test_search_callers_no_ambiguity_warning_single_class`](../src/mcp/handlers/handlers_tests_csharp.rs)
- [`test_search_callers_ambiguity_warning_few_classes`](../src/mcp/handlers/handlers_tests_csharp.rs)
- [`test_search_callers_ambiguity_warning_truncated`](../src/mcp/handlers/handlers_tests_csharp.rs)

---

## TypeScript Handler-Level Unit Tests

The following test scenarios are covered by unit tests in
[`handlers_tests_typescript.rs`](../src/mcp/handlers/handlers_tests_typescript.rs). They validate
MCP handler behavior for TypeScript definitions and call graphs using real indexes built from
TypeScript test fixture files.

### search_definitions — TypeScript Kinds

#### T87: `search_definitions` — Finds TypeScript class

**Tool:** `search_definitions`

**Scenario:** Searching for a TypeScript class by name returns the class definition with correct
metadata (file, lines, kind).

**Expected:**

- `definitions` array contains the class with `kind: "class"`
- Definition includes `name`, `file`, `lines`, `signature`

**Unit test:** [`test_ts_search_definitions_finds_class`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T88: `search_definitions` — Finds TypeScript interface

**Tool:** `search_definitions`

**Scenario:** Searching for a TypeScript interface by name returns the interface definition.

**Expected:**

- `definitions` array contains the interface with `kind: "interface"`
- Definition includes correct file path and line range

**Unit test:** [`test_ts_search_definitions_finds_interface`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T89: `search_definitions` — Finds TypeScript method

**Tool:** `search_definitions`

**Scenario:** Searching for a TypeScript class method returns the method definition with its
parent class.

**Expected:**

- `definitions` array contains the method with `kind: "method"`
- Definition has correct `parent` class name

**Unit test:** [`test_ts_search_definitions_finds_method`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T90: `search_definitions` — Finds TypeScript function

**Tool:** `search_definitions`

**Scenario:** Searching for a TypeScript standalone function returns the function definition
with `kind: "function"`.

**Expected:**

- `definitions` array contains the function with `kind: "function"`
- Standalone functions (not class methods) are correctly categorized

**Unit test:** [`test_ts_search_definitions_finds_function`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T91: `search_definitions` — Finds TypeScript enum

**Tool:** `search_definitions`

**Scenario:** Searching for a TypeScript enum returns the enum definition.

**Expected:**

- `definitions` array contains the enum with `kind: "enum"`
- Enum signature includes member names

**Unit test:** [`test_ts_search_definitions_finds_enum`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T92: `search_definitions` — Finds TypeScript enum member

**Tool:** `search_definitions`

**Scenario:** Searching for TypeScript enum members returns individual enum values.

**Expected:**

- `definitions` array contains entries with `kind: "enumMember"`
- Each enum member has its parent enum as the `parent` field

**Unit test:** [`test_ts_search_definitions_finds_enum_member`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T93: `search_definitions` — Finds TypeScript type alias

**Tool:** `search_definitions`

**Scenario:** Searching for a TypeScript type alias returns the type declaration.

**Expected:**

- `definitions` array contains the type alias with `kind: "typeAlias"`
- Type aliases (e.g., `type Props = { ... }`) are correctly categorized

**Unit test:** [`test_ts_search_definitions_finds_type_alias`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T94: `search_definitions` — Finds TypeScript variable

**Tool:** `search_definitions`

**Scenario:** Searching for a TypeScript exported variable/constant returns the variable definition.

**Expected:**

- `definitions` array contains the variable with `kind: "variable"`
- Exported `const`/`let` declarations and `InjectionToken` variables are included

**Unit test:** [`test_ts_search_definitions_finds_variable`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T95: `search_definitions` — Finds TypeScript field

**Tool:** `search_definitions`

**Scenario:** Searching for a TypeScript class field returns the field definition.

**Expected:**

- `definitions` array contains the field with `kind: "field"`
- Field has correct parent class

**Unit test:** [`test_ts_search_definitions_finds_field`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T96: `search_definitions` — Finds TypeScript constructor

**Tool:** `search_definitions`

**Scenario:** Searching for a TypeScript class constructor returns the constructor definition.

**Expected:**

- `definitions` array contains the constructor with `kind: "constructor"`
- Constructor has correct parent class

**Unit test:** [`test_ts_search_definitions_finds_constructor`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

### search_definitions — TypeScript Filters

#### T97: `search_definitions` — TypeScript `baseType` filter (implements)

**Tool:** `search_definitions`

**Scenario:** Filtering by `baseType` returns TypeScript classes that implement the specified
interface.

**Expected:**

- All returned definitions list the specified base type in their `baseTypes`
- Classes using `implements` keyword are matched

**Unit test:** [`test_ts_search_definitions_base_type_implements`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T98: `search_definitions` — TypeScript `baseType` filter (abstract/extends)

**Tool:** `search_definitions`

**Scenario:** Filtering by `baseType` returns TypeScript classes extending an abstract class.

**Expected:**

- All returned definitions list the abstract base class in their `baseTypes`
- Classes using `extends` keyword are matched

**Unit test:** [`test_ts_search_definitions_base_type_abstract`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T99: `search_definitions` — TypeScript `containsLine` finds method

**Tool:** `search_definitions`

**Scenario:** Using `containsLine` with a TypeScript file returns the innermost method
containing the specified line number.

**Expected:**

- `containingDefinitions` array includes the method at that line
- Parent class is also returned in the containing definitions

**Unit test:** [`test_ts_contains_line_finds_method`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T100: `search_definitions` — TypeScript `includeBody`

**Tool:** `search_definitions`

**Scenario:** Setting `includeBody: true` for TypeScript definitions returns source code
inline.

**Expected:**

- Definition objects contain `body` array with source lines
- `bodyStartLine` matches the definition's start line
- Body content is actual TypeScript source code

**Unit test:** [`test_ts_search_definitions_include_body`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T101: `search_definitions` — TypeScript combined name + parent + kind filter

**Tool:** `search_definitions`

**Scenario:** Passing `name`, `parent`, and `kind` filters simultaneously for TypeScript
definitions returns only those matching all three criteria.

**Expected:**

- All returned definitions match the name substring, parent class, and kind
- `summary.totalResults` reflects only matching definitions

**Unit test:** [`test_ts_search_definitions_combined_name_parent_kind`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T102: `search_definitions` — TypeScript regex name filter

**Tool:** `search_definitions`

**Scenario:** Setting `regex: true` with a name pattern matches TypeScript definitions
using regex.

**Expected:**

- All returned definitions have names matching the regex pattern
- Non-matching definitions are excluded

**Unit test:** [`test_ts_search_definitions_name_regex`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

### search_callers — TypeScript

#### T103: `search_callers` — TypeScript callers (direction=up)

**Tool:** `search_callers`

**Scenario:** Finding callers of a TypeScript method returns the call tree with callers
from TypeScript files.

**Expected:**

- `callTree` includes callers from `.ts` files
- Caller entries have correct method name, parent class, file path, and line number

**Unit test:** [`test_ts_search_callers_up_finds_caller`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T104: `search_callers` — TypeScript callees (direction=down)

**Tool:** `search_callers`

**Scenario:** Finding callees of a TypeScript method returns the callee tree showing
what the method calls.

**Expected:**

- `callTree` includes callees (methods called by the target method)
- Callee entries have correct method names and file paths

**Unit test:** [`test_ts_search_callers_down_finds_callees`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

#### T105: `search_callers` — TypeScript nonexistent method

**Tool:** `search_callers`

**Scenario:** Searching for callers of a method that doesn't exist returns an empty call tree
without errors.

**Expected:**

- `callTree` array is empty
- `summary.totalNodes` = 0
- No error (graceful empty response)

**Unit test:** [`test_ts_search_callers_nonexistent_method`](../src/mcp/handlers/handlers_tests_typescript.rs)

---

### search_find — Additional Scenarios

#### T106: `search_find` — Contents mode

**Tool:** `search_find`

**Scenario:** Setting `contents: true` searches file contents instead of file names,
returning matching lines with file path and line number.

**Expected:**

- Results contain file path and line number for each match
- Search term is found in the file content, not the file name
- Response includes match count

**Unit test:** [`test_search_find_contents_mode`](../src/mcp/handlers/handlers_tests.rs)

---

### search_help / search_info — Response Validation

#### T107: `search_help` — Response structure validation

**Tool:** `search_help`

**Scenario:** Calling `search_help` returns a well-structured JSON response containing
best practices, performance tiers, tool priority, and strategy recipes.

**Expected:**

- Response contains `bestPractices` array (non-empty)
- Response contains `performanceTiers` object
- Response contains `toolPriority` array
- Response contains `strategyRecipes` array
- All expected fields are present and well-formed

**Unit test:** [`test_search_help_response_structure`](../src/mcp/handlers/handlers_tests.rs)

---

#### T108: `search_info` — Response structure validation

**Tool:** `search_info`

**Scenario:** Calling `search_info` returns a well-structured JSON response containing
index information (directory, indexes, counts).

**Expected:**

- Response contains `indexDirectory` string
- Response contains `indexes` array
- Response structure is valid JSON with expected fields

**Unit test:** [`test_search_info_response_structure`](../src/mcp/handlers/handlers_tests.rs)

---

## Parser-Level Unit Tests

The following parser-level tests validate TypeScript-specific parsing corner cases in
[`definitions_tests_typescript.rs`](../src/definitions/definitions_tests_typescript.rs).

#### T-PARSER-CONST-ENUM: TypeScript `const enum` parsing

**Scenario:** TypeScript `const enum` declarations (e.g., `const enum Direction { Up, Down }`)
are correctly parsed and extracted as enum definitions.

**Expected:**

- `const enum` is parsed as `kind: "enum"`
- Enum members are extracted as `kind: "enumMember"`
- The `const` modifier does not prevent parsing

**Unit test:** [`test_parse_ts_const_enum`](../src/definitions/definitions_tests_typescript.rs)

---

#### T-PARSER-INJECTION-TOKEN: TypeScript `InjectionToken` variable parsing

**Scenario:** TypeScript `InjectionToken<T>` variable declarations (common in Angular DI) are
correctly parsed as variable definitions.

**Expected:**

- `InjectionToken<T>` variables are parsed as `kind: "variable"`
- The generic type parameter does not prevent parsing
- Variable name and signature are correctly extracted

**Unit test:** [`test_parse_ts_injection_token_variable`](../src/definitions/definitions_tests_typescript.rs)

---

### T-GENERIC-CALLERS: `search_callers` — Generic method calls correctly matched with class filter

**Background:** C# generic method calls like `_service.SearchAsync<T>(args)` were stored with
`method_name = "SearchAsync<T>"` (including type arguments) instead of `"SearchAsync"`. This
caused `verify_call_site_target()` to fail when `class` filter was used, producing 0 callers.
The fix strips type arguments from `generic_name` AST nodes in the C# parser.

**Setup:** Create C# files:

- `SearchService.cs`: `public class SearchService { public Task<T> SearchAsync<T>(string q) { ... } }`
- `Consumer.cs`: `public class Consumer { private ISearchService _svc; void Run() { _svc.SearchAsync<Document>("q"); } }`

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"SearchAsync","class":"SearchService","direction":"up","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext cs --definitions
```

**Expected:**

- `callTree` includes `Consumer.Run` as a caller
- Generic type arguments (`<Document>`) are stripped from the stored `method_name`
- `summary.totalNodes` ≥ 1

**Validates:** `extract_method_name_from_name_node()` in `parser_csharp.rs` correctly strips
type arguments from `generic_name` AST nodes for member access calls (`obj.Method<T>()`) and
conditional access calls (`obj?.Method<T>()`).

**Status:** ✅ Covered by unit tests: `test_generic_method_call_via_member_access`,
`test_generic_method_call_with_multiple_type_args`, `test_generic_method_call_via_this`,
`test_generic_and_nongeneric_calls_coexist`, `test_generic_static_method_call`,
`test_verify_call_site_target_generic_method_call`

---

### T-CHAINED-CALLS: Chained method calls extracted from call-site index (C# and TypeScript)

**Background:** Inner calls in method chains like `service.SearchAsync<T>(...).ConfigureAwait(false)` and `builder.Where(...).OrderBy(...).ToList()` were previously not extracted. Only the outermost call was found. The fix makes `walk_for_invocations()` (C#) and `walk_ts_for_invocations()` (TypeScript) recurse into ALL children of `invocation_expression` / `call_expression` nodes, not just `argument_list`.

**Setup:** Create a C# file with chained calls:

```csharp
public class TestBlock {
    private readonly ISearchClient m_searchClient;
    public TestBlock(ISearchClient searchClient) { m_searchClient = searchClient; }
    public async Task ExecuteSearch() {
        return await m_searchClient.SearchForAllTenantsAsync<object>(1, "index", "query").ConfigureAwait(false);
    }
}
```

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"SearchForAllTenantsAsync","class":"SearchClient","direction":"up","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext cs --definitions
```

**Expected:**

- `callTree` includes `TestBlock.ExecuteSearch` as a caller
- Previously, this caller was missed because `SearchForAllTenantsAsync` was nested inside `.ConfigureAwait(false)` and the parser only recursed into `argument_list` children
- `summary.totalNodes` ≥ 1

**Validates:** `walk_for_invocations()` in `parser_csharp.rs` and `walk_ts_for_invocations()` in `parser_typescript.rs` now recurse into all children of invocation nodes, extracting inner calls from chained method expressions.

**Status:** ✅ Covered by unit tests: `test_chained_call_configure_await_extracts_inner_call`, `test_call_sites_chained_calls` (strengthened to verify all inner calls)

---

### T-CTOR-ASSIGN: Constructor body assignment-based DI field resolution

**Background:** The C# parser resolves receiver types for DI field calls (e.g., `m_service.Method()`) by mapping field names to types from constructor parameters. Previously, only `_paramName` and bare `paramName` conventions were supported. The fix parses the constructor body AST for `field = param` assignments, handling ANY naming convention automatically (e.g., `m_field`, `fld_field`, `this.field`). Also fixes `extract_constructor_param_types` for constructors with `: base(...)` or `: this(...)` initializers.

**Unit tests (4 tests):**

| Test | Pattern | Validates |
|------|---------|-----------|
| `test_call_site_extraction_constructor_assignment_m_prefix` | `m_field = param` | Hungarian notation DI fields |
| `test_call_site_extraction_constructor_assignment_this_prefix` | `this.field = param` | `this.` accessor pattern |
| `test_call_site_extraction_constructor_assignment_arbitrary_prefix` | `fld_field = param`, `s_field = param` | Any naming convention |
| `test_extract_constructor_param_types_with_initializer` | `Ctor(params) : base(args)` | Paren matching with initializer |

---

### T-OWNER-FIELD: Owner.m_field nested class DI resolution (ControllerBlock pattern)

**Background:** In the ControllerBlock pattern, a nested inner class accesses DI-injected fields of its outer (parent) class via `Owner.m_field`. Previously, the receiver type for `Owner.m_queryManager.GetEntriesAsync(...)` was resolved to `"OrderControllerBlock"` (the type of the `Owner` field) instead of `"IQueryManager"` (the type of `m_queryManager` in the outer class). This caused `search_callers` with `class: "QueryManager"` or `class: "IQueryManager"` to return 0 results, even though the call site was correctly indexed when no class filter was used.

**Root cause:** The `field_types` map passed to call site extraction only contained fields from the inner class — outer class fields were invisible. The fix merges outer class field types into the inner class's field_types map (inner class fields take precedence).

**Unit tests (2 tests):**

| Test | Pattern | Validates |
|------|---------|-----------|
| `test_owner_m_field_nested_class_receiver_resolution` | `Owner.m_field.Method()` | Receiver resolves to outer class field's DI type |
| `test_owner_m_field_inner_class_field_takes_precedence` | Inner + outer share field name | Inner class field type wins in merged map |

**Status:** ✅ Covered by unit tests. Not CLI-testable (internal parser behavior).

**Status:** ✅ Covered by unit tests. Not CLI-testable (internal parser behavior).

---

### T-TYPE-INFER: Type inference improvements for search_callers

**Background:** 7 user stories improving local variable type inference in the C# parser. These improvements increase recall for `search_callers` by resolving types from cast expressions, `as` expressions, method return types, `await` + Task<T> unwrap, pattern matching, and extension methods.

**Unit tests (23 tests):**

| Test | Pattern | Validates |
|------|---------|-----------|
| `test_csharp_var_cast_type_inference` | `var x = (Type)expr` | Cast expression type extraction |
| `test_csharp_var_as_type_inference` | `var x = expr as Type` | `as` expression type extraction |
| `test_csharp_using_var_type_inference` | `using var x = new Type()` | `using var` handled by existing path |
| `test_csharp_var_method_return_type_inference` | `var x = GetStream()` | Same-class method return type lookup |
| `test_csharp_var_this_method_return_type_inference` | `var x = this.CreateClient()` | `this.Method()` return type |
| `test_csharp_var_method_return_type_void_not_stored` | `void DoWork()` | Void methods filtered |
| `test_csharp_var_method_return_cross_class_not_resolved` | `var x = _repo.GetById()` | Cross-class NOT resolved |
| `test_csharp_var_method_return_generic_type` | `List<User> GetUsers()` | Generic return types |
| `test_csharp_var_method_return_lowercase_type_not_resolved` | `object GetValue()` | Lowercase types filtered |
| `test_parse_return_type_from_signature_simple` | Signature parsing | Basic signatures |
| `test_parse_return_type_from_signature_generic` | `Task<List<User>>` | Generic signature parsing |
| `test_parse_return_type_from_signature_no_paren` | Edge case | No parentheses |
| `test_unwrap_task_type` | `Task<T>` → T | Task type unwrapping |
| `test_csharp_var_await_task_unwrap` | `await GetStreamAsync()` | Task<Stream> → Stream |
| `test_csharp_var_await_valuetask_unwrap` | `await GetClientAsync()` | ValueTask<T> → T |
| `test_csharp_var_await_nested_generic_unwrap` | `await GetUsersAsync()` | Task<List<User>> → List<User> |
| `test_csharp_var_await_plain_task_no_unwrap` | `await DoWorkAsync()` | Plain Task → no type |
| `test_csharp_var_no_await_task_not_unwrapped` | `GetStreamAsync()` (no await) | Task not unwrapped without await |
| `test_csharp_is_pattern_type_inference` | `obj is PackageReader reader` | Pattern matching type |
| `test_csharp_is_pattern_negated_not_resolved` | `obj is not Type` | Negated pattern safe |
| `test_csharp_switch_case_pattern_type_inference` | `case StreamReader reader:` | Switch pattern type |
| `test_csharp_extension_method_detection` | `static class` + `this` param | Extension detection |
| `test_csharp_extension_method_not_detected_for_non_static_class` | Non-static class | No false positive |
| `test_csharp_extension_method_multiple_classes` | Multiple ext classes | Multiple classes |
| `test_verify_call_site_target_extension_method` | Extension in verify | Handler accepts |
| `test_verify_call_site_target_extension_method_no_match_without_map` | No map | Handler rejects |

**Status:** All covered by unit tests. Not CLI-testable (internal parser behavior).

---

## Changes Not CLI-Testable (Covered by Unit Tests)

The following internal optimizations are covered by unit tests in `src/mcp/watcher.rs` and `src/definitions/incremental.rs`, but have no CLI-observable behavior for E2E testing:

| Change                                               | Unit Test                        | Description                                                                                                                                                                                              |
| ---------------------------------------------------- | -------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `.git/` directory filtering in watcher               | `test_is_inside_git_dir`         | Watcher now skips files inside `.git/` directories to avoid indexing git internals (e.g., `.git/config` matching "config" extension)                                                                     |
| `batch_purge_files` for watcher batches              | `test_batch_purge_files_*` (3 tests)  | Watcher uses single-pass batch purge O(total_postings) instead of N sequential purges O(N × total_postings) for git pull / checkout scenarios                                                           |
| `shrink_to_fit()` after `retain()`                   | (behavioral — no dedicated test) | After incremental updates, all `HashMap`/`Vec` collections call `shrink_to_fit()` to release excess capacity from `retain()` operations                                                                  |
| `sorted_intersect` in `collect_substring_file_ids()` | (behavioral — no dedicated test) | Replaced `HashSet`-based intersection with sorted two-pointer `sorted_intersect` in `callers.rs` for better cache locality and reduced allocations. Same results, faster execution on large file ID sets |

---

### T65: Local Variable Type Extraction — TypeScript

**Background:** The TypeScript parser extracts type information from local variable declarations to resolve `receiver_type` on call sites. This covers:

1. Explicit type annotations: `const result: UserResult = ...` → `result.validate()` has `receiver_type = "UserResult"`
2. `new` expressions: `const v = new OrderValidator()` → `v.check()` has `receiver_type = "OrderValidator"`
3. Generic `new` expressions: `const c = new DataCache<string>()` → `c.get()` has `receiver_type = "DataCache"` (generics stripped)
4. No type info (preserved): `const r = this.calculate()` → `r.process()` has `receiver_type = Some("r")` — the variable name is preserved so that `verify_call_site_target` can correctly reject it as a non-matching receiver (prevents false positives where `r.process()` would otherwise be treated as `this.process()`)
5. Field types take precedence: `this.result.fieldMethod()` resolves to field type, not local var with same name

**Validates:** Local variable type extraction in TypeScript parser, `receiver_type` resolution for call sites using local variables. Unresolved local variables preserve their name to prevent false positive caller matches.

**Status:** ✅ Covered by unit tests: `test_ts_local_var_explicit_type_annotation`, `test_ts_local_var_new_expression`, `test_ts_local_var_new_expression_with_generics`, `test_ts_local_var_no_type_annotation`, `test_ts_local_var_field_types_take_precedence`, `test_ts_local_var_let_declaration_without_initializer`

---

### T66: Local Variable Type Extraction — C#

**Background:** The C# parser extracts type information from local variable declarations to resolve `receiver_type` on call sites. This covers:

1. Explicit type: `UserResult result = ...` → `result.Validate()` has `receiver_type = "UserResult"`
2. `var = new`: `var v = new OrderValidator()` → `v.Check()` has `receiver_type = "OrderValidator"`
3. `var` without `new` (preserved): `var r = Calculate()` → `r.Process()` has `receiver_type = Some("r")` — the variable name is preserved so that `verify_call_site_target` can correctly reject it as a non-matching receiver (prevents false positives where `r.Process()` would otherwise be treated as `this.Process()`)
4. Generic types: `List<User> users = ...` → `users.Add()` has `receiver_type = "List"` (generics stripped)
5. `using var` pattern: `using (var session = OpenSession()) { session.Execute(); }` has `receiver_type = Some("session")` — unresolved receiver name preserved

**Parser fix:** tree-sitter C# 0.23 places `object_creation_expression` as a direct child of `variable_declarator` (not wrapped in `equals_value_clause`). The parser now checks both locations.

**Validates:** Local variable type extraction in C# parser, `receiver_type` resolution for call sites using local variables, `var = new X()` inference. Unresolved local variables preserve their name to prevent false positive caller matches.

**Status:** ✅ Covered by unit tests: `test_csharp_local_var_explicit_type`, `test_csharp_local_var_new_expression`, `test_csharp_local_var_var_without_new`, `test_csharp_local_var_generic_type`, `test_csharp_using_var_receiver_preserved`

---

### T-TYPED-LOCAL-DOWN: Explicit type annotations — direction=down resolves callees through typed locals

**Background:** When `direction=down` traces callees of a method, local variables with explicit type annotations (`const x: Foo = ...`) are used to resolve the receiver type of method calls. This ensures that `x.method()` is correctly attributed to class `Foo` rather than being unresolved.

**Setup:** Create TypeScript files:

- `DataProcessor.ts`: `export class DataProcessor { transform(data: string): string { return data.toUpperCase(); } }`
- `Orchestrator.ts`:
  ```typescript
  export class Orchestrator {
    run() {
      const proc: DataProcessor = this.getProcessor();
      proc.transform("hello");
    }
    getProcessor(): DataProcessor {
      return new DataProcessor();
    }
  }
  ```

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"run","class":"Orchestrator","direction":"down","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext ts --definitions
```

**Expected:**

- `callTree` includes `DataProcessor.transform()` as a callee
- The callee is found because `proc` has explicit type annotation `: DataProcessor`
- `summary.totalNodes` ≥ 1

**Validates:** Explicit type annotations on local variables are used during direction=down callee resolution. The TypeScript parser extracts type info from `const x: Foo = ...` via `extract_ts_local_var_types()`, which feeds into `extract_ts_call_sites()` to set `receiver_type` on call sites.

**Status:** ✅ Covered by unit test `test_ts_direction_down_with_typed_local_variable` in `handlers_tests_typescript.rs`

---

### T67: Direction=up — False Positive Filtering with Receiver Type Mismatch

**Background:** When `search_callers` runs with `direction=up`, the `verify_call_site_target()` function filters out false positive callers where the `receiver_type` on a call site doesn't match the target class. For example, if searching for callers of `TaskRunner.resolve()`, a file containing `path.resolve()` should NOT appear as a caller because the `receiver_type` is `"Path"`, not `"TaskRunner"`.

**Setup:** Create TypeScript files:

- `task.ts`: `export class TaskRunner { resolve() { return true; } }`
- `caller_good.ts`: `import { TaskRunner } from './task'; const t = new TaskRunner(); t.resolve();` (inside a function/method)
- `caller_false.ts`: `import * as path from 'path'; function build() { path.resolve('/tmp'); }` (unrelated `resolve()` call)

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"resolve","class":"TaskRunner","direction":"up","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext ts --definitions
```

**Expected:**

- `callTree` does NOT include `caller_false.ts` (receiver_type mismatch: `path` ≠ `TaskRunner`)
- `callTree` includes `caller_good.ts` (receiver_type matches or is compatible with `TaskRunner`)
- `summary.totalNodes` reflects only verified callers

**Validates:** `verify_call_site_target()` in up-direction correctly filters out false positives based on `receiver_type` mismatch.

**Status:** ✅ Covered by unit tests: `test_ts_search_callers_up_filters_false_positives` (planned), parser-level tests for local var type extraction

---

### T68: Direction=up — Graceful Fallback When No Call-Site Data

**Background:** When `verify_call_site_target()` encounters a caller file that has no call-site data (e.g., the file was not parsed by tree-sitter, or the method call was in a pattern not recognized by the parser), the verification should gracefully fall back to including the caller (no false negatives from missing data).

**Setup:** Create TypeScript files:

- `service.ts`: `export class DataService { fetch() { return []; } }`
- `consumer.ts`: Contains a call to `fetch()` but in a pattern where call-site data may not have `receiver_type` (e.g., via destructuring or dynamic dispatch)

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"fetch","class":"DataService","direction":"up","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext ts --definitions
```

**Expected:**

- `callTree` includes callers even when `receiver_type` is `None` (graceful fallback)
- No false negatives: callers without call-site data are NOT filtered out
- No errors or crashes from missing data

**Validates:** `verify_call_site_target()` gracefully handles missing `receiver_type` (None) — no false negatives from incomplete call-site data.

**Status:** ✅ Covered by unit tests in `handlers_tests_typescript.rs`

---

### T69: Direction=up — Comment-Line False Positive Filtered

**Background:** When the content index matches a method name appearing in a **comment** (not a real call), `verify_call_site_target()` should filter it out. This is language-agnostic (affects both C# and TypeScript). Before the fix, comment lines containing the method name were incorrectly treated as valid call sites, producing false positive callers.

**Setup:** Create TypeScript files:

- `task-runner.ts`: `export class TaskRunner { resolve(): void { console.log("resolved"); } }`
- `consumer.ts`: Contains "resolve" in comments (lines 5-6) AND a real call `runner.resolve()` (line 8)

```typescript
// consumer.ts
import { TaskRunner } from "./task-runner";

export class Consumer {
  processData(): void {
    // We need to resolve the task before proceeding
    // The resolve method handles cleanup
    const runner = new TaskRunner();
    runner.resolve();
  }
}
```

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"resolve","class":"TaskRunner","direction":"up","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext ts --definitions
```

**Expected:**

- `callTree` includes `Consumer.processData` (real call at `runner.resolve()`)
- Result count is exactly 1 — comment lines containing "resolve" are NOT false positives
- No errors or crashes

**Validates:** `verify_call_site_target()` correctly filters comment-line matches from the content index. The content index matches multiple lines containing "resolve" (comments + real call), but only the real call site survives verification.

**Status:** ✅ Covered by E2E test in `e2e-test.ps1` and unit tests in `handlers_tests_typescript.rs`

---

## Fix 3 — Bypass Gate Closure and Lambda/Expression Body Parsing

These test scenarios validate the changes from Fix 3 which closed three bypass gates in the
caller verification pipeline and extended the C# parser to extract call sites from expression
body properties and lambda expressions in arguments.

### T-FIX3-VERIFY: Callers verification — no false positives from missing call-site data

**Background:** Before Fix 3, `verify_call_site_target()` returned `true` (accept) when
`method_calls.get(&caller_di)` returned `None` — meaning a caller method with no parsed
call-site data would pass verification by default. After Fix 3, this bypass is closed:
the function returns `false` (reject) when call-site data is missing, ensuring only callers
with actual call-site evidence are included in the call tree.

**Setup:** Create C# files:

- `Service.cs`: A class with a method (e.g., `DataService.Process()`)
- `RealCaller.cs`: Contains a method that genuinely calls `service.Process()`
- `FalseCaller.cs`: Contains a method that mentions "Process" in a string or comment but has
  no actual call site to `DataService.Process()`

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"Process","class":"DataService","direction":"up","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext cs --definitions
```

**Expected:**

- `callTree` includes `RealCaller` (has actual call site with matching receiver type)
- `callTree` does NOT include `FalseCaller` (no call-site data → rejected by `verify_call_site_target()`)
- `summary.totalNodes` reflects only verified callers

**Validates:** Bypass #2 closure — `verify_call_site_target()` rejects callers without call-site data instead of accepting them by default.

**Status:** ✅ Covered by unit tests in `handlers_tests_csharp.rs`: `test_csharp_callers_no_false_positive_from_missing_call_site_data`

---

### T-FIX3-EXPR-BODY: Expression body property call sites (C#)

**Background:** Before Fix 3, C# expression body properties (e.g., `public string Name => _service.GetName();`)
did not have their call sites extracted because the parser only looked at block bodies (`{ ... }`),
not `arrow_expression_clause` (`=> expr;`). After Fix 3, call sites inside expression body
properties are extracted and included in the caller tree.

**Setup:** Create C# files:

- `NameProvider.cs`: `public class NameProvider { public string GetName() => "test"; }`
- `Consumer.cs`: Contains an expression body property that calls `GetName()`:
  ```csharp
  public class Consumer {
      private NameProvider _provider;
      public string DisplayName => _provider.GetName();
  }
  ```

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"GetName","class":"NameProvider","direction":"up","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext cs --definitions
```

**Expected:**

- `callTree` includes `Consumer.DisplayName` as a caller (expression body property)
- The property is found because `arrow_expression_clause` is now parsed for call sites
- `summary.totalNodes` ≥ 1

**Validates:** C# parser extension for expression body properties (`=> expr;`). Call sites inside
`arrow_expression_clause` nodes are extracted and linked to the containing property definition.

**Status:** ✅ Covered by unit tests in `definitions_tests_csharp.rs`: `test_csharp_expression_body_property_call_sites` and `handlers_tests_csharp.rs`: `test_csharp_callers_expression_body_property`

---

### T-FIX3-LAMBDA: Lambda calls in arguments captured (C#)

**Background:** C# lambda expressions passed as arguments (e.g., `items.ForEach(x => x.Method())`)
contain call sites that should be attributed to the enclosing method. The parser extracts call
sites from lambda bodies, resolving the call to the containing method definition.

**Setup:** Create C# files:

- `Validator.cs`: `public class Validator { public bool Validate(string s) => s.Length > 0; }`
- `Processor.cs`: Contains a method with a lambda that calls `Validate()`:
  ```csharp
  public class Processor {
      private Validator _validator;
      public void ProcessAll(List<string> items) {
          items.ForEach(x => _validator.Validate(x));
      }
  }
  ```

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"Validate","class":"Validator","direction":"up","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext cs --definitions
```

**Expected:**

- `callTree` includes `Processor.ProcessAll` as a caller
- The call site inside the lambda body (`x => _validator.Validate(x)`) is attributed to `ProcessAll`
- `summary.totalNodes` ≥ 1

**Validates:** Lambda expressions in argument lists have their call sites extracted and attributed
to the enclosing method. The C# parser traverses lambda bodies for call expressions.

**Status:** ✅ Covered by unit tests in `definitions_tests_csharp.rs`: `test_csharp_lambda_in_argument_call_sites` and `handlers_tests_csharp.rs`: `test_csharp_callers_lambda_in_arguments`

---

### T-FIX3-PREFILTER: Base types removed from caller pre-filter

**Background:** Before Fix 3, `build_caller_tree()` expanded the pre-filter grep terms to include
base types of the target class (e.g., if `DataService` implements `IDisposable`, the pre-filter
would grep for `IDisposable` too). For classes implementing common interfaces, this caused the
pre-filter to match thousands of irrelevant files (e.g., every file using `IDisposable`), which
then all had to be individually verified — massively degrading performance and sometimes producing
false positives from files that happened to match base type names.

After Fix 3, the pre-filter only greps for the target class name and its direct interface
(DI-aware), not the transitive base types. This dramatically reduces the number of candidate
files without losing real callers.

**Expected behavior:**

- Searching for callers of `DataService.Process()` where `DataService : IService, IDisposable`
  should NOT pre-filter using `IDisposable` — only `DataService` and `IService` (DI pair)
- The call tree should contain only genuine callers, not files that mention `IDisposable`
- Performance: pre-filter should match a small number of files (tens, not thousands)

**Validates:** Bypass #3 closure — base types expansion removed from pre-filter in `build_caller_tree()`.

**Status:** ✅ Covered by unit tests in `handlers_tests_csharp.rs`: `test_csharp_callers_no_base_type_expansion_in_prefilter`. Not CLI-testable because the pre-filter is an internal optimization — the observable effect is fewer false positives and faster execution, verified via unit tests.

---

### T-FIX3-FIND-CONTAINING: find_containing_method returns definition index directly

**Background:** Before Fix 3, `find_containing_method()` returned a `DefinitionInfo` clone, then
`find_method_def_index()` was called separately to look up the definition index. When the index
lookup returned `None`, verification was skipped entirely (bypass #1). After Fix 3,
`find_containing_method()` returns the definition index (`di`) directly, eliminating the
redundant lookup and the bypass where verification was skipped.

**Expected behavior:**

- All callers go through full verification (no bypass when definition index is not found)
- Call tree accuracy is improved: methods that previously bypassed verification are now properly
  verified against their call-site data

**Validates:** Bypass #1 closure — `find_containing_method()` returns `di` directly, removing the
`find_method_def_index()` intermediate step and its `None` bypass.

**Status:** ✅ Internal refactor — covered by all existing caller unit tests which exercise the
full verification pipeline. No separate test needed.

---

### T-OVERLOAD-DEDUP-UP: Overloaded callers not collapsed (direction=up)

**Background:** When multiple overloads of the same method (e.g., `Process(int)` and `Process(string)`)
both call a target method, the caller tree should show BOTH overloads as separate entries. Previously,
the dedup key used `(file_id, method_name)` which collapsed all overloads into one entry. The fix
adds `line_start` to the dedup key: `(file_id, method_name, line_start)`.

**Setup:** Create C# files:

- `Validator.cs`: `public class Validator { public bool Validate() => true; }`
- `Processor.cs`: Contains two overloads of `Process` that both call `Validate()`:
  ```csharp
  public class Processor {
      private Validator _validator;
      public void Process(int x) { _validator.Validate(); }
      public void Process(string s) { _validator.Validate(); }
  }
  ```

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"Validate","class":"Validator","direction":"up","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext cs --definitions
```

**Expected:**

- `callTree` includes TWO entries for `Process` (one per overload), each with different `lines` values
- `summary.totalNodes` = 2
- Previously, only ONE `Process` entry appeared (the other was deduped away)

**Validates:** Overloaded methods are not collapsed in the caller tree. The dedup key includes
`line_start` so overloads at different line positions are treated as distinct callers.

**Status:** ✅ Covered by unit test `test_search_callers_overloads_not_collapsed_up` in `handlers_tests_csharp.rs`

---

### T-OVERLOAD-DEDUP-DOWN: Overloaded callees not collapsed (direction=down)

**Background:** When a method calls two overloads of the same target method (e.g., `Execute(int)`
and `Execute(string)` in the same class), the callee tree should show BOTH overloads as separate
entries. The same dedup fix applies to direction=down.

**Setup:** Create C# files:

- `TaskRunner.cs`: Contains two overloads of `Execute`:
  ```csharp
  public class TaskRunner {
      public void Execute(int id) { }
      public void Execute(string name) { }
  }
  ```
- `Orchestrator.cs`: Contains `RunAll()` that calls both overloads:
  ```csharp
  public class Orchestrator {
      private TaskRunner _runner;
      public void RunAll() {
          _runner.Execute(1);
          _runner.Execute("task");
      }
  }
  ```

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"RunAll","class":"Orchestrator","direction":"down","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext cs --definitions
```

**Expected:**

- `callTree` includes TWO entries for `Execute` (one per overload), each with different `lines` values
- `summary.totalNodes` = 2
- Previously, only ONE `Execute` entry appeared

**Validates:** Overloaded callees are not collapsed in the callee tree (direction=down).
Same dedup key fix as T-OVERLOAD-DEDUP-UP.

**Status:** ✅ Covered by unit test `test_search_callers_overloads_not_collapsed_down` in `handlers_tests_csharp.rs`

---

### T-SAME-NAME-IFACE: Same method name on unrelated interfaces — no cross-contamination

**Background:** When two UNRELATED interfaces define methods with the same name (e.g.,
`IServiceA.Execute()` and `IServiceB.Execute()`), searching for callers of `ServiceA.Execute()`
should NOT include callers that use `IServiceB.Execute()`. Previously, the interface resolution
block expanded ALL interfaces implementing the same method name, causing cross-contamination
between unrelated class hierarchies.

The fix filters the interface resolution to only include interfaces that are actually related
to the target class (i.e., interfaces listed in the class's `base_types`).

**Setup:** Create C# files:

- `IServiceA.cs`: `public interface IServiceA { void Execute(); }`
- `IServiceB.cs`: `public interface IServiceB { void Execute(); }`
- `ServiceA.cs`: `public class ServiceA : IServiceA { public void Execute() { } }`
- `ServiceB.cs`: `public class ServiceB : IServiceB { public void Execute() { } }`
- `Consumer.cs`: Uses `IServiceB.Execute()` only:
  ```csharp
  public class Consumer {
      private IServiceB _serviceB;
      public void DoWork() { _serviceB.Execute(); }
  }
  ```

**Command (MCP) — query ServiceA callers:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"Execute","class":"ServiceA","direction":"up","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext cs --definitions
```

**Expected:**

- `callTree` is EMPTY (ServiceA.Execute has no callers)
- `Consumer.DoWork()` does NOT appear (it calls IServiceB.Execute, not IServiceA.Execute)
- Previously, Consumer.DoWork() falsely appeared because BOTH `IServiceA` and `IServiceB`
  were included in the interface resolution, and `Consumer` mentions `IServiceB` which
  was incorrectly treated as related to `ServiceA`

**Command (MCP) — query ServiceB callers (cross-validation):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"Execute","class":"ServiceB","direction":"up","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext cs --definitions
```

**Expected:**

- `callTree` includes `Consumer.DoWork()` (correct — it calls IServiceB.Execute)
- `summary.totalNodes` = 1

**Validates:** Interface resolution is scoped to related interfaces only. Unrelated interfaces
with the same method name do not cause false positive callers.

**Status:** ✅ Covered by unit test `test_search_callers_same_name_different_receiver_interface_resolution` in `handlers_tests_csharp.rs`

---

### T-BUILTIN-BLOCKLIST: Built-in type blocklist prevents false positives in direction=down

**Background:** When `direction=down` finds a call to `Promise.resolve()`, `Array.map()`, or other
built-in type methods, the `resolve_call_site()` function previously searched for user-defined classes
with the same method name (e.g., `Deferred.resolve()`) and returned them as false positives. The
built-in type blocklist (`BUILTIN_RECEIVER_TYPES`) prevents this by skipping candidate matching when
the receiver type is a known built-in JavaScript/TypeScript or C# type.

**Setup:** Create TypeScript files:

- `deferred.ts`: `export class Deferred { resolve(): void { } }`
- `worker.ts`: `export class Worker { doWork(): void { Promise.resolve(42); } }`

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"doWork","class":"Worker","direction":"down","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext ts --definitions
```

**Expected:**

- `callTree` does NOT include `Deferred.resolve()` (receiver type `Promise` is on blocklist)
- Built-in types like `Promise`, `Array`, `Map`, `Set`, `console`, `Math`, `JSON`, `Task`, `List`, etc. are all blocked
- Non-built-in types (e.g., `MyService.process()`) still resolve normally

**Validates:** `BUILTIN_RECEIVER_TYPES` blocklist in `resolve_call_site()` prevents false positive
callee resolution for built-in type method calls.

**Status:** ✅ Covered by unit tests: `test_builtin_promise_resolve_not_matched`, `test_builtin_array_map_not_matched`, `test_non_builtin_type_still_matches`

---

### T-FUZZY-DI: Fuzzy DI interface matching — search_callers direction=up finds callers through non-standard interface naming

**Background:** DI resolution only works with the exact convention `IFooService` → `FooService`
(strip `I` prefix). If the interface is `IDataModelService` but the implementation is
`DataModelWebService`, the link is NOT established because stripping `I` gives `DataModelService`,
which does NOT equal `DataModelWebService`. The fuzzy DI matching fix uses suffix-tolerant
matching: the stem `DataModelService` (from `IDataModelService`) is checked as a substring of
the implementation class name `DataModelWebService`.

**Setup:** Create TypeScript files:

- `interface.ts`: `export interface IDataModelService { loadModel(): void; }`
- `impl.ts`: `export class DataModelWebService implements IDataModelService { loadModel() { } }`
- `caller.ts`: A method with parameter `svc: IDataModelService` that calls `svc.loadModel()`

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"loadModel","class":"DataModelWebService","direction":"up","depth":1}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext ts --definitions
```

**Expected:**

- `callTree` includes the caller from `caller.ts` (fuzzy match: `IDataModelService` → `DataModelWebService`)
- The match works because the stem `DataModelService` (from `IDataModelService`) is contained in `DataModelWebService`
- `summary.totalNodes` ≥ 1

**Negative test — no false positive for unrelated class:**

- A class named `UnrelatedRunner` with method `run()` should NOT match `IService.run()` callers
  because `UnrelatedRunner` does not contain the stem `Service`

**Validates:** Fuzzy DI interface matching via `is_implementation_of()` in `verify_call_site_target()`.
Stem must be ≥ 4 characters to avoid overly broad matches.

**Status:** ✅ Covered by unit tests: `test_verify_call_site_target_fuzzy_interface_match`, `test_fuzzy_di_no_false_positive`, `test_is_implementation_of_exact_prefix`, `test_is_implementation_of_suffix_tolerant`, `test_is_implementation_of_short_stem_no_match`, `test_is_implementation_of_no_false_positive`

---

## Relevance Ranking Tests

The following test scenarios validate the relevance ranking behavior added to `search_definitions`,
`search_fast`, and `search_grep` (phrase mode). Results are sorted by match quality so that the
most relevant result appears first.

### T-RANK-01: `best_match_tier()` — Unit tests for match tier classification

**Function:** [`best_match_tier()`](../src/mcp/handlers/utils.rs:527)

**Scenario:** The function classifies a name against search terms into three tiers:

- Tier 0: exact match (case-insensitive)
- Tier 1: prefix match (name starts with term)
- Tier 2: contains/default (name contains term or doesn't match)

**Tests (9 tests in `utils.rs`):**

| Test                     | Input                                                                 | Expected |
| ------------------------ | --------------------------------------------------------------------- | -------- |
| Exact match              | `"UserService"` vs `["userservice"]`                                  | 0        |
| Case insensitive         | `"USERSERVICE"` vs `["userservice"]`                                  | 0        |
| Prefix match             | `"UserServiceFactory"` vs `["userservice"]`                           | 1        |
| Contains only            | `"IUserService"` vs `["userservice"]`                                 | 2        |
| No match                 | `"OrderProcessor"` vs `["userservice"]`                               | 2        |
| Multiple terms best wins | `"UserService"` vs `["order", "userservice"]`                         | 0        |
| Empty terms              | `"UserService"` vs `[]`                                               | 2        |
| Exact beats prefix       | `"IUserService"` vs `["iuserservice", "userservice"]`                 | 0        |
| Prefix beats contains    | `"UserService"` vs `["user"]` → 1; `"IUserService"` vs `["user"]` → 2 |

**Status:** ✅ Covered by unit tests `test_best_match_tier_*` in [`utils.rs`](../src/mcp/handlers/utils.rs)

---

### T-RANK-02: `kind_priority()` — Unit tests for definition kind tiebreaker

**Function:** [`kind_priority()`](../src/mcp/handlers/definitions.rs:16)

**Scenario:** The function assigns priority 0 to type-level definitions (class, interface, enum,
struct, record) and priority 1 to everything else (method, function, property, field, constructor,
delegate, event, enumMember, typeAlias, variable). Used as a tiebreaker when match tier is equal.

**Tests (16 tests in `definitions.rs`):**

| Kind                                                                                             | Expected Priority |
| ------------------------------------------------------------------------------------------------ | ----------------- |
| Class, Interface, Enum, Struct, Record                                                           | 0                 |
| Method, Function, Property, Field, Constructor, Delegate, Event, EnumMember, TypeAlias, Variable | 1                 |

**Status:** ✅ Covered by unit tests `test_kind_priority_*` in [`definitions.rs`](../src/mcp/handlers/definitions.rs)

---

### T-RANK-03: `search_definitions` — Relevance ranking (exact → prefix → contains)

**Tool:** `search_definitions`

**Scenario:** When searching for `"UserService"`, results are sorted by:

1. Match tier (exact=0 → prefix=1 → contains=2)
2. Kind priority (class/interface=0 → method/property=1)
3. Name length (shorter first)
4. Alphabetical

**Expected order:**

1. `UserService` (class) — exact match, kind=0
2. `UserServiceFactory` (class) — prefix match, kind=0
3. `UserServiceHelper` (method) — prefix match, kind=1
4. `IUserService` (interface) — contains match, kind=0

**Unit tests (4 tests in `handlers_tests.rs`):**

- [`test_search_definitions_ranking_exact_first`](../src/mcp/handlers/handlers_tests.rs) — exact match appears first
- [`test_search_definitions_ranking_prefix_before_contains`](../src/mcp/handlers/handlers_tests.rs) — prefix matches before contains
- [`test_search_definitions_ranking_kind_and_length_tiebreak`](../src/mcp/handlers/handlers_tests.rs) — class before method among prefix matches
- [`test_search_definitions_ranking_not_applied_with_regex`](../src/mcp/handlers/handlers_tests.rs) — ranking not applied in regex mode

**Status:** ✅ Covered by unit tests

---

### T-RANK-04: `search_fast` — Relevance ranking (exact stem → prefix → contains)

**Tool:** `search_fast`

**Scenario:** When searching for `"UserService"` with `ignoreCase: true`, file results are sorted by:

1. Best match tier on file stem (without extension)
2. Stem length (shorter first)
3. Full path alphabetical

**Expected order:**

1. `UserService.cs` — exact stem match (tier 0)
2. `UserServiceFactory.cs` — prefix match (tier 1), shorter stem
3. `UserServiceHelpers.cs` — prefix match (tier 1), longer stem
4. `IUserService.cs` — contains match (tier 2)

**Unit tests (2 tests in `handlers_tests.rs`):**

- [`test_search_fast_ranking_exact_stem_first`](../src/mcp/handlers/handlers_tests.rs) — exact stem match first, prefix before contains
- [`test_search_fast_ranking_shorter_stem_first`](../src/mcp/handlers/handlers_tests.rs) — shorter stems before longer among same tier

**Status:** ✅ Covered by unit tests

---

### T-RANK-06: `search_definitions` — Parent relevance ranking (exact parent before substring parent)

**Tool:** `search_definitions`

**Scenario:** When searching with `parent` filter, results are sorted by parent match quality:

1. Exact parent match (tier 0) — `parent=UserService` matches `UserService` exactly
2. Prefix parent match (tier 1) — `parent=UserService` matches `UserServiceFactory`
3. Contains parent match (tier 2) — `parent=UserService` matches `IUserService` or `MockUserServiceWrapper`
4. No parent (tier 3) — definitions without a parent field

**Expected order (searching with `parent=UserService`):**

1. Members of `UserService` (exact parent match, tier 0)
2. Members of `UserServiceFactory` (prefix match, tier 1)
3. Members of `IUserService` (contains match, tier 2)
4. Members of `MockUserServiceWrapper` (contains match, tier 2)

**Key behavior:**

- Parent match quality is the PRIMARY sort key (takes precedence over name match quality)
- Name match quality is SECONDARY (within same parent tier)
- Kind priority (class=0 > method=1) and name length are tiebreakers
- Only active when `parent` filter is set (no effect when parent filter is absent)

**Unit tests (5 tests in `definitions.rs`):**

- [`test_parent_ranking_exact_parent_before_substring_parent`](../src/mcp/handlers/definitions.rs) — exact parent ranks before prefix/substring
- [`test_parent_ranking_prefix_parent_before_contains_parent`](../src/mcp/handlers/definitions.rs) — prefix parent (tier 1) ranks before contains parent (tier 2)
- [`test_parent_ranking_takes_precedence_over_name_ranking`](../src/mcp/handlers/definitions.rs) — parent tier beats name tier
- [`test_parent_ranking_no_parent_sorts_last`](../src/mcp/handlers/definitions.rs) — definitions without parent get tier 3
- [`test_parent_ranking_only_active_with_parent_filter`](../src/mcp/handlers/definitions.rs) — no effect when parent filter is absent

**Status:** ✅ Covered by unit tests

---

### T-RANK-05: `search_grep` phrase mode — Sort by occurrence count descending

**Tool:** `search_grep` (phrase mode)

**Scenario:** When using `phrase: true`, results are sorted by number of occurrences (matching
lines) in descending order — files with more matches appear first.

**Expected:**

- File with 3 occurrences appears before file with 2, which appears before file with 1
- `files[i].occurrences >= files[i+1].occurrences` for all i

**Unit test:** [`test_search_grep_phrase_sort_by_occurrences`](../src/mcp/handlers/handlers_tests.rs)

**Status:** ✅ Covered by unit test

---

## Code Stats Tests

### T-AUDIT: Independent Audit Tests for Code Stats and Call Chains

**Background:** The `audit_tests.rs` module provides independent verification of code complexity metrics and call chain analysis accuracy. It uses golden fixtures — hand-crafted code where every metric is manually computed line-by-line. The audit covers 6 areas:

1. **C# Code Stats (7 tests)** — comprehensive method, while/do/try-catch, flat switch, mixed logical operators, lambdas, expression-bodied members, foreach nesting
2. **TypeScript Code Stats (5 tests)** — comprehensive function, arrow function counting, flat else-if chain, switch/case, empty method baseline
3. **Call Site Accuracy (2 tests)** — verifies all call patterns (DI field, this, static, new, local var, lambda) with correct receiver types for both C# and TypeScript
4. **Call Graph Verification (2 tests)** — multi-class call graph completeness for both C# and TypeScript, verifying receiver types across class boundaries
5. **Edge Cases (4 tests)** — nested lambdas nesting depth, constructor stats, no stats for non-method definitions (class/interface/enum)
6. **Statistical Consistency (3 tests)** — invariant checks (CC≥1, CC=1→cognitive=0, call_count matches call_sites.len()), cross-language consistency between C# and TypeScript

**Unit tests:** 22 tests in `src/definitions/audit_tests.rs`:
- `audit_cs_comprehensive_method`, `audit_cs_while_do_try_catch`, `audit_cs_switch_flat`, `audit_cs_mixed_logical_operators`, `audit_cs_lambda_counting`, `audit_cs_expression_body`, `audit_cs_foreach_complexity`
- `audit_ts_comprehensive_function`, `audit_ts_arrow_function_counting`, `audit_ts_else_if_chain_flat`, `audit_ts_switch_case`, `audit_cs_empty_method`, `audit_ts_empty_method`
- `audit_cs_call_site_completeness`, `audit_ts_call_site_completeness`
- `audit_cs_call_graph_multi_class`, `audit_ts_call_graph_multi_class`
- `audit_cs_nested_lambdas_nesting`, `audit_ts_nested_arrows_nesting`, `audit_cs_constructor_stats`
- `audit_cs_no_stats_for_non_methods`, `audit_ts_no_stats_for_non_methods`
- `audit_cs_invariants_comprehensive`, `audit_ts_invariants_comprehensive`, `audit_cross_language_consistency`

**Key findings documented in test comments:**
- C# tree-sitter does NOT emit `else_clause` nodes — else-if is parsed as direct `if_statement → if_statement` child
- `try_statement` adds +1 nesting in C#, so `catch_clause` inside try gets cognitive penalty at nesting=1
- TypeScript parser correctly handles else-if as flat, with `else_clause` wrapper
- Cross-language tests must avoid `else` constructs due to grammar differences

---

### T-CODESTATS-01: `search_definitions` — `includeCodeStats=true` returns metrics

**Tool:** `search_definitions`

**Scenario:** Passing `includeCodeStats: true` returns code complexity metrics for methods/functions.

**Expected:**

- Method definitions include a `codeStats` object with: `lines`, `cyclomaticComplexity`, `cognitiveComplexity`, `maxNestingDepth`, `paramCount`, `returnCount`, `callCount`, `lambdaCount`
- Class/field/enum definitions do NOT have a `codeStats` object
- `summary` does NOT contain `codeStatsAvailable: false` (metrics are available in new indexes)

**Unit tests:** `test_code_stats_empty_method`, `test_code_stats_single_if`, `test_code_stats_nested_if`, `test_code_stats_triple_nested_if`, `test_code_stats_logical_operator_sequence`, `test_code_stats_mixed_logical_operators`, `test_code_stats_for_with_if`, `test_code_stats_lambda_count`, `test_code_stats_return_and_throw_count`, `test_code_stats_call_count_from_parser`, `test_code_stats_param_count`, `test_code_stats_if_else`

---

### T-CODESTATS-02: `search_definitions` — `sortBy` sorts by metric descending

**Tool:** `search_definitions`

**Scenario:** Passing `sortBy: "cognitiveComplexity"` returns methods sorted by cognitive complexity (worst first).

**Expected:**

- Results are sorted in descending order by the specified metric
- `summary.sortedBy` field is present
- `includeCodeStats` is automatically enabled

---

### T-CODESTATS-03: `search_definitions` — `min*` filters restrict results

**Tool:** `search_definitions`

**Scenario:** Passing `minComplexity: 5` returns only methods with cyclomatic complexity ≥ 5.

**Expected:**

- All returned definitions have `codeStats.cyclomaticComplexity >= 5`
- `summary.statsFiltersApplied` = `true`
- `summary.beforeStatsFilter` > `summary.afterStatsFilter` (some filtered out)
- Definitions without code stats (classes, fields) are excluded

---

### T-CODESTATS-04: `search_definitions` — Invalid `sortBy` value returns error

**Tool:** `search_definitions`

**Scenario:** Passing `sortBy: "invalidMetric"` returns an actionable error.

**Expected:**

- Response is an error (`isError: true`)
- Error message lists valid `sortBy` values

---

### T-CODESTATS-05: `search_reindex_definitions` — Response includes `codeStatsEntries`

**Tool:** `search_reindex_definitions`

**Scenario:** After reindexing, the response includes the count of code stats entries.

**Expected:**

- Response JSON contains `codeStatsEntries` field with a number ≥ 0
- For C#/TypeScript projects, `codeStatsEntries` > 0

---

### T-CODESTATS-06: `search_definitions` — Backward compatibility with old index (no stats)

**Tool:** `search_definitions`

**Scenario:** When loaded from an old cache without code_stats, `includeCodeStats: true` returns results without `codeStats` objects and `summary.codeStatsAvailable: false`.

**Expected:**

- Results returned normally (no error)
- `summary.codeStatsAvailable` = `false`
- No `codeStats` objects in definitions

---

### T-CODESTATS-07: `search_definitions` — `sortBy` with old index returns error

**Tool:** `search_definitions`

**Scenario:** When loaded from an old cache without code_stats, `sortBy: "cognitiveComplexity"` returns an error.

**Expected:**

- Response is an error (`isError: true`)
- Error message recommends `search_reindex_definitions`

---

## Git History Tools Tests

### T-GIT-01: `serve` — search_git_history returns file commit history

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_history","arguments":{"repo":".","file":"Cargo.toml","maxResults":5}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `commits` array (non-empty)
- Each commit has `hash`, `date`, `author`, `email`, `message`
- `summary.totalCommits` ≥ 1
- `summary.returned` ≤ 5
- No `patch` field (history mode, not diff)

**Validates:** `search_git_history` tool with maxResults limit.

**Status:** ✅ Covered by unit tests: `test_file_history_returns_commits`, `test_file_history_max_results_limits_output`, `test_commit_info_has_all_fields`

---

### T-GIT-02: `serve` — search_git_diff returns patches

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_diff","arguments":{"repo":".","file":"Cargo.toml","maxResults":3}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `commits` array (non-empty)
- Each commit has `patch` field (non-empty string with +/- lines)
- Patches truncated to ~200 lines per commit
- `summary.tool` = `"search_git_diff"`

**Validates:** `search_git_diff` tool returns actual diff content.

**Status:** ✅ Covered by unit tests: `test_file_history_with_diff`

---

### T-GIT-03: `serve` — search_git_authors returns ranked authors

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_authors","arguments":{"repo":".","file":"Cargo.toml","top":5}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `authors` array (non-empty)
- Authors ranked by commit count (descending)
- Each author has `rank`, `name`, `email`, `commits`, `firstChange`, `lastChange`
- `summary.totalCommits` > 0
- `summary.totalAuthors` > 0

**Validates:** `search_git_authors` tool with top limit.

**Status:** ✅ Covered by unit tests: `test_top_authors_returns_ranked`, `test_top_authors_limits_results`

---

### T-GIT-04: `serve` — search_git_activity returns repo-wide changes

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_activity","arguments":{"repo":".","from":"2020-01-01","to":"2030-12-31"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `activity` array (non-empty)
- Each entry has `path`, `commits` array, `commitCount`
- `summary.filesChanged` > 0
- `summary.commitsProcessed` > 0
- Results sorted by commit count descending

**Validates:** `search_git_activity` tool with date range.

**Status:** ✅ Covered by unit tests: `test_repo_activity_returns_files`

---

### T-GIT-05: `serve` — search_git_history with date filter

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_history","arguments":{"repo":".","file":"Cargo.toml","date":"1970-01-01"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with empty `commits` array (no commits on that date)
- `summary.totalCommits` = 0

**Validates:** Date filtering narrows results correctly.

**Status:** ✅ Covered by unit tests: `test_file_history_date_filter_narrows_results`, `test_repo_activity_empty_date_range`

---

### T-GIT-06: `serve` — search_git_history missing required parameter

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_history","arguments":{"repo":"."}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `isError: true`
- Error message: "Missing required parameter: file"

**Validates:** Required parameter validation for git tools.

---

### T-GIT-07: `serve` — search_git_history bad repo path

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_history","arguments":{"repo":"/nonexistent/repo","file":"main.rs"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `isError: true`
- Error message indicates repository not found

**Validates:** Graceful error handling for invalid repo paths.

**Status:** ✅ Covered by unit tests: `test_file_history_bad_repo`, `test_repo_activity_bad_repo`

---

### T-GIT-08: `serve` — Git tools available without --definitions or --git flag

**Scenario:** Git tools should always appear in `tools/list` regardless of `--definitions` flag.

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- `tools` array contains 15 entries (9 original + 6 git)
- Git tools present: `search_git_history`, `search_git_diff`, `search_git_authors`, `search_git_activity`, `search_git_blame`, `search_branch_status`
- No `--git` flag needed

**Validates:** Git tools are always available, no opt-in needed.

**Status:** ✅ Covered by unit tests: `test_handle_tools_list` (15 tools), `test_tool_definitions_count` (15 tools)

## Git History Cache — Unit Tests (PR 2a)

The following test scenarios are covered by unit tests in
[`cache_tests.rs`](../src/git/cache_tests.rs). They validate the core git history cache module
without requiring a running MCP server or real git repository. All tests use mock data.

### T-CACHE-01: Parser — Multi-commit git log output

**Scenario:** Parse a mock git log with 3 commits, 2 authors, and 3 files. Verify commit count,
author deduplication, file_commits mapping, and subject pool.

**Expected:**

- 3 commits parsed
- 2 unique authors (Alice appears twice, deduplicated)
- `src/main.rs` has 3 commit refs, `Cargo.toml` has 1, `src/lib.rs` has 1
- Commit fields (hash, timestamp, author, subject) are correctly resolved

**Unit tests:** `test_parser_multi_commit`, `test_parser_commit_fields`

---

### T-CACHE-02: Parser — Edge cases

**Scenario:** Various parser edge cases including empty input, empty subject, subject containing
the field separator `␞`, empty file list (merge commit), merge commit with 100 files, malformed
lines, and bad hash values.

**Expected:**

- Empty input → 0 commits
- Empty subject → preserved as ""
- Subject with `␞` → rejoined via `fields[4..].join(sep)`
- Empty file list → commit recorded, no file_commits entries
- 100 files → all recorded
- Malformed/bad hash lines → skipped silently, subsequent good commits still parsed

**Unit tests:** `test_parser_empty_input`, `test_parser_empty_subject`, `test_parser_subject_with_field_sep`,
`test_parser_empty_file_list`, `test_parser_merge_commit_many_files`, `test_parser_malformed_line_skipped`,
`test_parser_bad_hash_skipped`

---

### T-CACHE-03: Path normalization

**Scenario:** The `normalize_path()` function handles Windows/Unix path normalization.

**Expected:**

- `"src\\main.rs"` → `"src/main.rs"` (backslash → forward slash)
- `"./src/main.rs"` → `"src/main.rs"` (strip `./`)
- `""` → `""` (empty preserved)
- `"."` → `""` (dot = root)
- `"src/"` → `"src"` (strip trailing `/`)
- `"src//main.rs"` → `"src/main.rs"` (collapse `//`)
- `"  src/main.rs  "` → `"src/main.rs"` (trim whitespace)

**Unit tests:** `test_normalize_path_backslash`, `test_normalize_path_dot_slash`, `test_normalize_path_empty`,
`test_normalize_path_dot`, `test_normalize_path_trailing_slash`, `test_normalize_path_double_slash`,
`test_normalize_path_whitespace`, `test_normalize_path_mixed`, `test_normalize_path_multiple_dot_slash`

---

### T-CACHE-04: Query — File history with filters

**Scenario:** `query_file_history()` returns commits for a file, sorted by timestamp descending,
respecting `maxResults` and `from`/`to` date filters.

**Expected:**

- Basic lookup: 3 commits for `src/main.rs`, sorted newest first
- `maxResults=2`: returns 2 newest commits
- `from` filter: excludes commits before timestamp
- `to` filter: excludes commits after timestamp
- `from+to`: returns only commits within range
- Nonexistent file: empty result
- Backslash/`./` paths: normalized before lookup (same results)

**Unit tests:** `test_query_file_history_basic`, `test_query_file_history_max_results`,
`test_query_file_history_from_date_filter`, `test_query_file_history_to_date_filter`,
`test_query_file_history_from_to_filter`, `test_query_file_history_nonexistent_file`,
`test_query_file_history_commit_info_fields`, `test_query_with_backslash_path`, `test_query_with_dot_slash_path`

---

### T-CACHE-05: Query — Authors aggregation

**Scenario:** `query_authors()` aggregates authors for a file or directory, deduplicating commits
that touch multiple files in the same directory.

**Expected:**

- File: 2 authors for `src/main.rs` (Alice: 2 commits, Bob: 1)
- Directory: `src` matches `src/main.rs` and `src/lib.rs`, deduplicates shared commits
- Empty path: matches all files

**Unit tests:** `test_query_authors_single_file`, `test_query_authors_directory`, `test_query_authors_empty_path_matches_all`

---

### T-CACHE-06: Query — Activity with path prefix matching

**Scenario:** `query_activity()` returns files changed in a directory, using correct prefix matching
`== path || starts_with(path + "/")` to prevent false positives.

**Expected:**

- `src` matches `src/main.rs` and `src/lib.rs` but NOT `src2/other.rs`
- Date filter narrows results
- Empty path matches all files
- Results sorted by `last_modified` descending

**Unit tests:** `test_query_activity_directory_prefix`, `test_query_activity_prefix_no_false_positive`,
`test_query_activity_date_filter`, `test_query_activity_empty_path_matches_all`,
`test_query_activity_authors_list`, `test_query_activity_exact_file_match`,
`test_query_activity_sorted_by_last_modified`

---

### T-CACHE-07: Cache validity

**Scenario:** `is_valid_for()` checks HEAD hash and format version.

**Expected:**

- Matching HEAD hash → valid
- Different HEAD hash → invalid
- Mismatched format version → invalid

**Unit tests:** `test_is_valid_for_matching_head`, `test_is_valid_for_non_matching_head`,
`test_is_valid_for_checks_format_version`

---

### T-CACHE-08: Detect default branch

**Scenario:** `detect_default_branch()` tries main, master, develop, trunk in order.

**Expected:** Requires real git repo — test marked `#[ignore]`.

**Unit test:** `test_detect_default_branch` (ignored)

---

### T-CACHE-09: CommitMeta struct size

**Scenario:** Verify `CommitMeta` is compact (close to 38-byte design target).

**Expected:** Size ≤ 48 bytes (actual: 40 bytes due to 8-byte alignment from `i64` field).

**Unit test:** `test_commit_meta_size`

---

### T-CACHE-10: Serialization roundtrip

**Scenario:** `GitHistoryCache` survives bincode serialization and LZ4-compressed serialization
(reusing `save_compressed()`/`load_compressed()` from `src/index.rs`).

**Expected:**

- Bincode roundtrip preserves all fields
- LZ4 compressed roundtrip preserves all fields
- Queries work correctly after deserialization

**Unit tests:** `test_cache_serialization_roundtrip`, `test_cache_lz4_compressed_roundtrip`

---

### T-CACHE-11: Author deduplication

**Scenario:** Same author (name + email) across multiple commits is stored once in the author pool.

**Expected:** 2 commits by same author → 1 entry in `authors`, both commits share `author_idx`.

**Unit test:** `test_author_deduplication`

---

### T-CACHE-FALLBACK: Git handlers fall back to CLI when cache is None (PR 2b)

**Scenario:** When the MCP server starts without a git cache (default state until PR 2c spawns the
background builder), all git history tools (`search_git_history`, `search_git_authors`,
`search_git_activity`) transparently fall back to the CLI path. No regression from PR 2b changes.

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_history","arguments":{"repo":".","file":"Cargo.toml","maxResults":3}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `commits` array (non-empty, from CLI fallback)
- `summary.hint` does NOT contain `"(from cache)"` (cache is not populated)
- Same response format as T-GIT-01

**Validates:** Cache-or-fallback routing with `git_cache: None` falls through to existing CLI code
with zero behavioral regression.

**Status:** ✅ Covered by existing git handler tests (all run with `git_cache: None`) + T-GIT-01 through T-GIT-08.

---

### T-NOCACHE: `noCache` parameter bypasses git history cache

**Scenario:** When `noCache: true` is passed to `search_git_history`, `search_git_authors`, or `search_git_activity`, the handler bypasses the in-memory cache and queries git CLI directly, even when the cache is populated and ready.

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_history","arguments":{"repo":".","file":"Cargo.toml","maxResults":2,"noCache":true}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `commits` array (non-empty, from CLI path)
- `summary.hint` does NOT contain `"(from cache)"` — cache was bypassed
- Same response format as T-GIT-01 (CLI path)

**Negative test — without `noCache`, cache is used when available:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_history","arguments":{"repo":".","file":"Cargo.toml","maxResults":2}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- `summary.hint` contains `"(from cache)"` (when cache is populated)

**Applies to:** `search_git_history`, `search_git_authors`, `search_git_activity`

**Unit tests:** `test_git_history_no_cache_bypasses_cache`, `test_git_history_default_uses_cache`, `test_git_authors_no_cache_bypasses_cache`, `test_git_activity_no_cache_bypasses_cache`, `test_git_history_no_cache_false_uses_cache`

---

### T-CACHE-ROUTING: Git handlers use cache when populated (PR 2b)

**Scenario:** When the git cache is populated (simulated by setting `git_cache_ready` to true and
inserting a cache), `search_git_history` and `search_git_authors` use the cache for sub-millisecond
responses. `search_git_diff` always uses CLI (no cache for patches).

**Expected (when cache is available):**

- `search_git_history`: response `summary.hint` contains `"(from cache)"`
- `search_git_authors`: response `summary.hint` contains `"(from cache)"`
- `search_git_activity`: response `summary.hint` contains `"(from cache)"`
- `search_git_diff`: response does NOT contain `"(from cache)"` (always CLI)
- `summary.elapsedMs` < 10 for cache responses (vs 2000+ for CLI)

**Note:** Full integration test requires PR 2c (background cache builder). Until then, this
behavior is verified by reading the handler code logic.

---

### T-CACHE-BACKGROUND: Git cache background build and disk persistence (PR 2c)

**Scenario:** When the MCP server starts with `--dir` pointing to a git repository, a background
thread builds the git history cache, saves it to disk (`<prefix>_<hash>.git-history`), and publishes
it to the `Arc<RwLock<Option<GitHistoryCache>>>`. On subsequent server restarts, the cache is loaded
from disk (~100 ms) instead of rebuilt (~59 sec). HEAD validation ensures the disk cache matches
the current branch HEAD.

**Command (first run — no cache on disk):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_history","arguments":{"repo":".","file":"Cargo.toml","maxResults":3}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir . --ext rs
```

**Expected (first run):**

- stderr: `[git-cache] Building git cache in background...`
- stderr: `[git-cache] Hint: run 'git commit-graph write --reachable' to speed up...` (if no commit-graph)
- stderr: `[git-cache] Built cache: N commits, M authors, K files, subjects=X bytes`
- stderr: `[git-history] Saved X.X MB (compressed) in X.XXs to ...git-history`
- stderr: `Git cache ready` with commit and file counts
- A `.git-history` file is created in the index directory

**Command (second run — cache loaded from disk):**

```powershell
echo $msgs | cargo run -- serve --dir . --ext rs
```

**Expected (second run):**

- stderr: `[git-history] Loaded X.X MB in X.XXXs`
- stderr: `Git cache loaded from disk (HEAD matches)` with commit and file counts
- No `Building git cache` message (cache was loaded from disk)
- Server starts faster (~100 ms cache load vs ~59 sec full rebuild)

**Command (after git pull — HEAD changed):**

```powershell
# Pull new commits, then restart server
git pull
echo $msgs | cargo run -- serve --dir . --ext rs
```

**Expected (HEAD changed):**

- stderr: `HEAD changed (fast-forward), rebuilding git cache` or `HEAD changed (not ancestor), full rebuild`
- Cache is rebuilt and saved to disk
- New cache reflects the updated HEAD

**Validates:** Background cache build thread, disk persistence (save_to_disk/load_from_disk), HEAD validation, commit-graph hint, incremental detection (ancestor check).

**Status:** ✅ Covered by unit tests: `test_save_load_disk_roundtrip`, `test_save_to_disk_atomic_write`, `test_load_from_disk_missing_file`, `test_load_from_disk_corrupt_file`, `test_load_from_disk_wrong_format_version`, `test_cache_path_for_extension`, `test_cache_path_for_deterministic`

---

### T-CACHE-AUTHORS-TIMESTAMPS: Authors query returns first and last commit timestamps

**Scenario:** `query_authors()` returns both `first_commit_timestamp` and `last_commit_timestamp` for each author, enabling the cached `search_git_authors` handler to populate both `firstChange` and `lastChange` fields.

**Expected:**

- Alice with commits at 1700000000 and 1700002000: `first_commit_timestamp=1700000000`, `last_commit_timestamp=1700002000`
- Bob with single commit at 1700001000: `first_commit_timestamp=last_commit_timestamp=1700001000`

**Unit test:** `test_query_authors_timestamps`

---

### T-CACHE-PROGRESS: Git cache build emits progress logging

**Scenario:** When building the git cache from scratch for a large repo, the background thread emits periodic progress messages to stderr so the user knows it's still working.

**Expected:**

- stderr: `[git-cache] Initializing for <dir>...` (immediately on thread start)
- stderr: `[git-cache] Detected branch: <branch>` (after detect_default_branch)
- stderr: `[git-cache] Building cache for branch '<branch>' (this may take a few minutes for large repos)...`
- stderr: `[git-cache] Progress: 10000 commits parsed (X.Xs elapsed)...` (every 10K commits during build)
- stderr: `[git-cache] Ready: N commits, M files in X.Xs`

**Validates:** User-facing progress indication during long git cache builds.

---

### T-CACHE-GIT-ROUTING: Git cache routing — search_git_history returns commits (E2E)

**Scenario:** Verify that `search_git_history` works end-to-end through the MCP server, regardless of whether the cache is ready or the CLI fallback is used. This confirms the cache routing code doesn't break existing git history functionality.

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_git_history","arguments":{"repo":".","file":"Cargo.toml","maxResults":2}}}'
) -join "`n"
echo $msgs | search-index serve --dir . --ext rs
```

**Expected:**

- JSON-RPC response (id=5) contains `commits` field
- Response may come from cache or CLI fallback — both are valid
- No errors or crashes

**Validates:** Cache routing code in git handlers does not break existing search_git_history functionality.

**Automated:** Test T-GIT-CACHE in [`e2e-test.ps1`](../e2e-test.ps1)

---

### T-CACHE-13: Bad timestamp parsing — commit skipped

**Scenario:** When a commit line in git log output has a non-numeric timestamp (e.g., `not_a_number`), the parser should skip that commit and continue parsing subsequent commits. File paths listed after the bad commit should NOT be associated with any commit.

**Expected:**

- The commit with bad timestamp is skipped
- Files listed after the bad commit are NOT in `file_commits`
- Subsequent valid commits are parsed normally

**Unit test:** `test_parser_bad_timestamp_skipped`

---

### T-CACHE-14: Author pool overflow boundary

**Scenario:** The author pool uses `u16` indices, limiting the maximum number of unique authors to 65535. When the 65536th unique author is encountered, `intern_author()` returns an error which propagates through `parse_git_log_stream()`. Exactly 65535 unique authors should succeed.

**Expected:**

- 65535 unique authors: parsing succeeds, cache has 65535 authors
- 65536 unique authors: parsing returns error containing "Too many unique authors"

**Unit tests:** `test_author_pool_overflow_via_parser`, `test_author_pool_boundary_65535_succeeds`

---

### T-CACHE-15: cache_path_for() different directories → different paths

**Scenario:** `cache_path_for()` produces different cache file paths for different input directories.

**Expected:**

- `cache_path_for("ProjectA")` ≠ `cache_path_for("ProjectB")`
- Both have `.git-history` extension

**Unit test:** `test_cache_path_for_different_dirs_produce_different_paths`

---

### T-CACHE-16: Integration test — build() with real temp git repo

**Scenario:** End-to-end test for `GitHistoryCache::build()` using a real git repository created in a temp directory. Creates files, makes commits, then verifies the cache correctly reflects the commit history.

**Expected:**

- 2 commits in cache
- 1 author ("Test")
- `file_a.txt` has 2 commit refs, `file_b.txt` has 1
- Query methods work correctly on the built cache

**Unit test:** `test_build_with_real_git_repo` (marked `#[ignore]` — requires git CLI)

---

### T-CACHE-17: Date boundary — query_file_history exact-day filter

**Scenario:** A commit at `2024-12-16 17:28:32 UTC` (timestamp 1734370112) should be found when querying with the exact date range `[2024-12-16 00:00:00, 2024-12-16 23:59:59]` UTC, and should NOT be found when querying with the wrong year `[2025-12-16 00:00:00, 2025-12-16 23:59:59]`.

**Expected:**

- `from=1734307200, to=1734393599` (2024-12-16 range) → 1 commit found
- `from=1765843200, to=1765929599` (2025-12-16 range) → 0 commits found
- `query_activity` and `query_file_history` return consistent results for the same date range

**Unit tests:** `test_query_file_history_exact_date_boundary`, `test_query_file_history_wrong_year_returns_empty`, `test_query_activity_vs_file_history_consistency`

---

### T-CACHE-18: Path case sensitivity — HashMap exact match

**Scenario:** Git stores file paths case-sensitively. `query_file_history()` uses HashMap exact lookup, so `src/Helpers/File.cs` ≠ `src/helpers/File.cs`. This is by-design behavior, not a bug.

**Expected:**

- Exact case match finds the commit
- Case-mismatched path returns 0 results

**Unit test:** `test_query_file_history_path_case_sensitivity`

---

### T-CACHE-19: Authors query — timestamps always non-zero

**Scenario:** `query_authors()` should always return non-zero `first_commit_timestamp` and `last_commit_timestamp` for files that have commits.

**Expected:**

- Multi-commit author: `first_commit_timestamp` = earliest, `last_commit_timestamp` = latest
- Single-commit author: `first_commit_timestamp == last_commit_timestamp`
- Both values > 0

**Unit tests:** `test_query_authors_first_last_timestamps_nonzero`, `test_query_authors_single_commit_timestamps_equal`

---

### T-GIT-DATE-UTC: CLI date filtering uses UTC

**Scenario:** The `add_date_args()` function appends `T00:00:00Z` to date strings passed to git's `--after`/`--before` flags, ensuring UTC interpretation regardless of local timezone.

**Expected:**

- `--after=2025-12-16T00:00:00Z` (not `--after=2025-12-16`)
- `--before=2025-12-17T00:00:00Z` (not `--before=2025-12-17`)
- CLI behavior matches cache behavior (both use UTC)

**Unit tests:** `test_date_2024_12_16_start`, `test_date_2025_12_16_start`, `test_commit_1734370112_is_2024_not_2025`, `test_format_timestamp_known_value`

---

### T-CACHE-20: Author/message filtering — query_file_history

**Scenario:** `query_file_history()` supports optional `author_filter` and `message_filter` parameters for filtering commits by author name/email and commit message (case-insensitive substring match).

**Expected:**

- `author_filter: "Alice"` returns only commits by Alice
- `author_filter: "bob@"` matches Bob's email
- `author_filter: "alice"` (lowercase) matches `"Alice"` (case-insensitive)
- `message_filter: "bug"` returns only commits with "bug" in the subject
- `message_filter: "FIX BUG"` (uppercase) matches case-insensitively
- Combined `author_filter + message_filter` requires both to match
- Combined `author_filter + from/to` filters work together

**Unit tests:** `test_query_file_history_author_filter`, `test_query_file_history_author_filter_by_email`, `test_query_file_history_author_filter_case_insensitive`, `test_query_file_history_author_filter_no_match`, `test_query_file_history_message_filter`, `test_query_file_history_message_filter_case_insensitive`, `test_query_file_history_message_filter_no_match`, `test_query_file_history_author_and_message_combined`, `test_query_file_history_author_and_date_combined`

---

### T-CACHE-21: Author/message filtering — query_activity

**Scenario:** `query_activity()` supports optional `author_filter` and `message_filter` parameters for filtering file activity by author and commit message.

**Expected:**

- `author_filter: "Bob"` returns only files touched by Bob
- `message_filter: "Initial"` returns only files from the "Initial commit"
- Combined `author_filter + message_filter` narrows results to commits matching both

**Unit tests:** `test_query_activity_author_filter`, `test_query_activity_message_filter`, `test_query_activity_author_and_message_combined`

---

### T-CACHE-22: Author/message filtering — query_authors

**Scenario:** `query_authors()` supports optional `author_filter`, `message_filter`, `from`, and `to` parameters for filtering author aggregations.

**Expected:**

- `message_filter: "feature"` returns only authors who committed with "feature" in the subject
- `from: 1700001500` returns authors whose commits are after that timestamp
- `author_filter: "Alice"` returns only Alice with her full commit count

**Unit tests:** `test_query_authors_with_message_filter`, `test_query_authors_with_date_filter`, `test_query_authors_with_author_filter`, `test_query_authors_whole_repo`

---

### T-GIT-BLAME-01: Git blame — basic line blame

**Scenario:** `blame_lines()` runs `git blame --porcelain` for a line range and returns structured blame data.

**Expected:**

- Blaming `Cargo.toml` lines 1-3 returns 3 `BlameLine` entries
- Each entry has non-empty `hash`, `author_name`, `date`, `content`
- Single-line blame (no `end_line`) returns exactly 1 line
- First line of `Cargo.toml` content contains `[package]`

**Unit tests:** `test_blame_lines_returns_results`, `test_blame_lines_single_line`, `test_blame_lines_has_content`

---

### T-GIT-BLAME-02: Git blame — error handling

**Scenario:** `blame_lines()` handles error cases gracefully.

**Expected:**

- Nonexistent file returns `Err`
- Nonexistent repo path returns `Err`

**Unit tests:** `test_blame_lines_nonexistent_file`, `test_blame_lines_bad_repo`

---

### T-GIT-BLAME-03: Git blame porcelain parser

**Scenario:** `parse_blame_porcelain()` correctly parses git blame `--porcelain` output including the commit metadata caching behavior (first occurrence has full headers, subsequent occurrences reuse cached metadata).

**Expected:**

- Basic single-line: parses hash (short 8-char), author name/email, content
- Repeated hash: second occurrence reuses author info from cache
- Empty input: returns empty vec (no error)

**Unit tests:** `test_parse_blame_porcelain_basic`, `test_parse_blame_porcelain_repeated_hash`, `test_parse_blame_porcelain_empty_input`

---

### T-GIT-BLAME-04: Blame date formatting with timezone offset

**Scenario:** `format_blame_date()` converts Unix timestamp + timezone offset to human-readable local date string, applying the timezone offset to the timestamp before formatting.

**Expected:**

- `format_blame_date(1700000000, "+0000")` → `"2023-11-14 22:13:20 +0000"` (UTC baseline)
- `format_blame_date(1700000000, "+0300")` → `"2023-11-15 01:13:20 +0300"` (crosses midnight)
- `format_blame_date(1700000000, "-0500")` → `"2023-11-14 17:13:20 -0500"` (goes back 5h)
- `format_blame_date(1700000000, "+0545")` → `"2023-11-15 03:58:20 +0545"` (Nepal quarter-hour offset)

**Unit tests:** `test_format_blame_date`, `test_format_blame_date_positive_offset`, `test_format_blame_date_negative_offset`, `test_format_blame_date_nepal_offset`, `test_parse_tz_offset`

---

### T-GIT-BLAME-05: Blame date timezone offset parsing

**Scenario:** `parse_tz_offset()` converts timezone offset strings to seconds.

**Expected:**

- `parse_tz_offset("+0000")` → `0`
- `parse_tz_offset("+0300")` → `10800`
- `parse_tz_offset("-0500")` → `-18000`
- `parse_tz_offset("+0545")` → `20700` (Nepal — 5h45m)
- `parse_tz_offset("")` → `0` (empty)
- `parse_tz_offset("UTC")` → `0` (text zone)
- `parse_tz_offset("+00")` → `0` (truncated)

**Unit test:** `test_parse_tz_offset`

---

### T-CACHE-12: Hex hash conversion

**Scenario:** SHA-1 hex string ↔ `[u8; 20]` byte array conversion.

**Expected:**

- Roundtrip preserves value
- Mixed-case hex → lowercase on output
- Invalid length/chars → Err

**Unit tests:** `test_hex_to_bytes_roundtrip`, `test_hex_to_bytes_mixed_case`,
`test_hex_to_bytes_invalid_length`, `test_hex_to_bytes_invalid_chars`


---

## Input Validation Bug Fix Tests

### T-VAL-01: `search_definitions` — Empty name treated as no filter (BUG-1)

**Tool:** `search_definitions`

**Scenario:** Passing `name: ""` (empty string) should behave identically to not passing `name` at all — returning all definitions filtered only by other parameters.

**Expected:**

- `name: ""` returns the same `totalResults` as omitting `name` entirely
- No error returned
- `definitions` array is non-empty

**Unit test:** [`test_search_definitions_empty_name_treated_as_no_filter`](../src/mcp/handlers/handlers_tests.rs)

---

### T-VAL-02: `search_definitions` — Negative `containsLine` returns error (BUG-2)

**Tool:** `search_definitions`

**Scenario:** Passing `containsLine: -1` should return a validation error instead of silently returning all definitions from the file. This was the most critical bug: negative values caused `as_u64()` to return `None`, skipping the `containsLine` filter entirely.

**Expected:**

- `isError: true`
- Error message: `"containsLine must be >= 1"`
- `containsLine: 0` also returns error

**Unit tests:** [`test_search_definitions_contains_line_negative_returns_error`](../src/mcp/handlers/handlers_tests.rs), [`test_search_definitions_contains_line_zero_returns_error`](../src/mcp/handlers/handlers_tests.rs)

---

### T-VAL-03: `search_callers` — `depth: 0` returns error (BUG-3)

**Tool:** `search_callers`

**Scenario:** Passing `depth: 0` should return a validation error instead of silently returning an empty call tree.

**Expected:**

- `isError: true`
- Error message: `"depth must be >= 1"`

**Unit test:** [`test_search_callers_depth_zero_returns_error`](../src/mcp/handlers/handlers_tests.rs)

---

### T-VAL-04: `search_git_history` — Reversed date range returns error (BUG-4)

**Tool:** `search_git_history`, `search_git_diff`, `search_git_activity`

**Scenario:** Passing `from: "2026-12-31", to: "2026-01-01"` (from > to) should return a descriptive error instead of silently returning 0 results.

**Expected:**

- `isError: true`
- Error message: `"'from' date (2026-12-31) is after 'to' date (2026-01-01)"`
- Works in both cache and CLI paths

**Unit tests:** [`test_parse_date_filter_reversed_range_returns_error`](../src/git/git_tests.rs), [`test_git_history_cached_reversed_dates_returns_error`](../src/mcp/handlers/handlers_tests.rs)

---

### T-VAL-05: `search_fast` — Empty pattern returns error (BUG-5)

**Tool:** `search_fast`

**Scenario:** Passing `pattern: ""` should return an error instead of scanning the entire file index for 0 results.

**Expected:**

- `isError: true`
- Error message mentions "empty"

**Unit test:** [`test_search_fast_empty_pattern_returns_error`](../src/mcp/handlers/handlers_tests.rs)

---

### T-VAL-07: `search_grep` — `matchedTokens` filtered by dir/ext/exclude (BUG-7)

**Tool:** `search_grep`

**Scenario:** In substring search mode, `matchedTokens` should only contain tokens from files that passed all filters (dir, ext, exclude). Previously, `matchedTokens` was populated from the global trigram index before filtering, leaking token names from outside the requested scope.

**Expected:**

- When `dir` restricts search to a subdirectory, `matchedTokens` only contains tokens from files in that subdirectory
- When `ext` filters by extension, `matchedTokens` only contains tokens from files with that extension
- When `exclude` filters out files, `matchedTokens` does not contain tokens exclusively in excluded files
- When no files match, `matchedTokens` is empty (not populated from the global index)

**Unit tests:** [`test_substring_matched_tokens_filtered_by_dir`](../src/mcp/handlers/handlers_tests.rs), [`test_substring_matched_tokens_filtered_by_ext`](../src/mcp/handlers/handlers_tests.rs), [`test_substring_matched_tokens_filtered_by_exclude`](../src/mcp/handlers/handlers_tests.rs), [`test_substring_matched_tokens_empty_when_no_files_match`](../src/mcp/handlers/handlers_tests.rs)

---

### T-CR-01: `search_callers` — Fuzzy DI matching works via `is_implementation_of` (BUG-CR-2)

**Tool:** `search_callers`

**Scenario:** When a class `DataModelWebService` does NOT declare `IDataModelService` in its `base_types` but follows the naming convention (contains stem "DataModelService"), callers through `IDataModelService` should still be found via fuzzy DI matching in `is_implementation_of()`. Previously this was dead code because the function received lowercased inputs.

**Expected:**

- `verify_call_site_target` returns `true` for receiver `IDataModelService` → target `DataModelWebService` even without `base_types`
- The stem "DataModelService" (from `IDataModelService`) is checked as a substring of `DataModelWebService`
- No false positives: `IService` → `UnrelatedRunner` does NOT match (stem "Service" not in "UnrelatedRunner")

**Unit tests:** [`test_verify_fuzzy_di_without_base_types`](../src/mcp/handlers/callers.rs), [`test_verify_reverse_fuzzy_di_without_base_types`](../src/mcp/handlers/callers.rs)

---

### T-CR-02: `search_grep` — Multi-extension ext filter (BUG-CR-1)

**Tool:** `search_grep`

**Scenario:** Passing `ext: "cs,sql"` should match both `.cs` and `.sql` files. Previously, the ext filter compared the entire string (e.g., `"cs" == "cs,sql"` → false), silently returning zero results.

**Expected:**

- `ext: "cs,sql"` returns files with both `.cs` and `.sql` extensions
- `ext: "cs"` still works (single extension)
- Case-insensitive: `ext: "CS"` matches `.cs` files
- Whitespace trimmed: `ext: " cs , sql "` works

**Unit tests:** [`test_matches_ext_filter_single`](../src/mcp/handlers/utils.rs), [`test_matches_ext_filter_multi`](../src/mcp/handlers/utils.rs), [`test_matches_ext_filter_case_insensitive`](../src/mcp/handlers/utils.rs), [`test_matches_ext_filter_with_spaces`](../src/mcp/handlers/utils.rs)

---

### T-CR-03: `search_callers` — `maxTotalNodes: 0` means unlimited (BUG-CR-3)

**Tool:** `search_callers`

**Scenario:** Passing `maxTotalNodes: 0` should treat 0 as unlimited (not return empty tree). Previously, `0 >= 0` was always true, causing immediate return.

**Expected:**

- `maxTotalNodes: 0` returns results (treated as `usize::MAX`)
- Default `maxTotalNodes: 200` still works

---

### T-CR-04: `search_callers` — Invalid direction returns error (BUG-CR-4)

**Tool:** `search_callers`

**Scenario:** Passing `direction: "sideways"` or `direction: "UP"` should be handled. Invalid values return an error. Case-insensitive comparison: `"UP"` is accepted as `"up"`.

**Expected:**

- `direction: "up"` and `direction: "UP"` both work (case-insensitive)
- `direction: "down"` and `direction: "DOWN"` both work
- `direction: "sideways"` returns error: `"Invalid direction 'sideways'. Must be 'up' or 'down'."`

---

### T-CR-05: `search_grep` — Warnings array (BUG-CR-5)

**Tool:** `search_grep`

**Scenario:** Short substring queries now return `summary.warnings` (array) instead of `summary.warning` (string). **Breaking change** for consumers reading the `warning` key.

**Expected:**

- Short query (`terms: "ab"`, `substring: true`) returns `summary.warnings` as a JSON array
- The old `summary.warning` key is no longer present

**Unit tests:** [`test_substring_search_short_query_warning`](../src/mcp/handlers/handlers_tests.rs), [`e2e_substring_search_short_query_warning`](../src/mcp/handlers/handlers_tests.rs)

---

### T-CR-06: `inject_body_into_obj` — Non-UTF-8 files handled via `read_file_lossy` (BUG-CR-6)

**Tool:** `search_definitions` (with `includeBody: true`)

**Scenario:** Files with non-UTF-8 content (e.g., Windows-1252 encoded) should have their body content returned via `read_file_lossy` instead of failing with `bodyError`. Previously, `std::fs::read_to_string` was used which fails on non-UTF-8 files.

**Expected:**

- Non-UTF-8 files return `body` array with lossy-converted content (replacement characters for invalid bytes)
- No `bodyError` for files that were successfully indexed

---

### T-CR-07: `search_grep` — Empty terms in normal mode returns error (BUG-CR-7)

**Tool:** `search_grep`

**Scenario:** Passing `terms: ",,,"` in normal token mode should return an explicit error, consistent with substring mode behavior. Previously, normal mode silently returned empty results.

**Expected:**

- `isError: true`
- Error message: `"No search terms provided"`

---

### T-VAL-06: `search_grep` — `contextLines` auto-enables `showLines` (BUG-6)

**Tool:** `search_grep`

**Scenario:** Passing `contextLines: 3` without `showLines: true` should automatically enable `showLines` and return line content with context.

**Expected:**

- `isError: false`
- Response includes `lineContent` arrays (auto-enabled)
- No need to explicitly pass `showLines: true` when `contextLines > 0`

**Unit test:** [`test_search_grep_context_lines_auto_enables_show_lines`](../src/mcp/handlers/handlers_tests.rs)

### T-WARMUP: Trigram pre-warming eliminates cold-start penalty

**Tool:** `search_grep` (substring mode)

**Background:** After deserializing the content index from disk, the first 1-2 substring queries
take ~3.4 seconds due to OS page faults on memory that hasn't been touched yet. The `warm_up()`
method on `ContentIndex` forces all trigram index pages into resident memory at server startup,
eliminating this cold-start penalty.

**Scenario:** Start the MCP server and verify warm-up completes in the background.

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"tokeniz","substring":true}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT 2>stderr.txt
Get-Content stderr.txt | Select-String "warmup"
```

**Expected stderr output (filtered by `[warmup]`):**

```
[warmup] Starting trigram pre-warm...
[warmup] Trigram pre-warm completed in X.Xms (N trigrams, M tokens)
```

**Assertions:**

- stderr contains `[warmup] Starting trigram pre-warm...`
- stderr contains `[warmup] Trigram pre-warm completed in` with timing and counts
- First substring query completes in < 100ms (not ~3.4s cold-start)
- `warm_up()` runs in a background thread and does not delay server startup

**Validates:** Trigram index pre-warming eliminates cold-start penalty for substring queries.
The `warm_up()` method touches all trigram posting lists, token strings, and inverted index
HashMap bucket pages to force OS page faults before the first real query.

**Unit tests:** `test_warm_up_empty_index`, `test_warm_up_with_data`, `test_warm_up_is_idempotent`, `test_warm_up_then_search_works`

**Status:** ✅ Implemented

---

### T-SUBSTRING-TRACE: Substring search emits timing traces to stderr

**Tool:** `search_grep` (substring mode)

**Background:** Substring search (`search_grep` with `substring: true`, which is the default) now
emits `[substring-trace]` timing instrumentation to stderr at each major processing stage. This
helps diagnose slow cold-start queries (first 1-2 queries take ~3.4s instead of expected
milliseconds). The tracing is always-on and outputs to stderr, which doesn't interfere with the
MCP protocol on stdout.

**Scenario:** Run a substring search and verify timing traces appear in stderr.

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"tokeniz","substring":true}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT 2>stderr.txt
Get-Content stderr.txt | Select-String "substring-trace"
```

**Expected stderr output (filtered by `[substring-trace]`):**

```
[substring-trace] Trigram dirty check: clean in 0.001ms
[substring-trace] Terms parsed: ["tokeniz"] in 0.001ms
[substring-trace] Trigram index: 1234 tokens, 5678 trigrams
[substring-trace] Token verification for 'tokeniz': 3 verified from candidates in 0.010ms
[substring-trace] Trigram intersection for 'tokeniz': 3 candidates in 0.050ms
[substring-trace] Main index lookup for 'tokeniz': 3 tokens, 150 postings checked, 12 files passed in 0.200ms
[substring-trace] Response JSON: 0.100ms
[substring-trace] Total: 0.500ms (12 files, 3 tokens matched)
```

**Assertions:**

- stderr contains at least one line with `[substring-trace]`
- stderr contains `Terms parsed:` with the search term(s)
- stderr contains `Trigram intersection` with candidate count and timing
- stderr contains `Main index lookup` with postings checked and files passed
- stderr contains `Total:` with overall elapsed time
- stdout (JSON-RPC response) is NOT affected — no trace lines in stdout
- When trigram index needs rebuild, stderr also contains `[substring-trace] Trigram rebuild:` with timing

**Validates:** Timing instrumentation in `handle_substring_search()` for diagnosing slow cold-start
substring queries. Traces cover: terms parsing, trigram dirty check + rebuild, trigram intersection
(per term), token verification, main index lookups, file filter checks, response JSON building,
and total elapsed time.

**Status:** ✅ Implemented. Existing tests pass with tracing enabled (628 tests, 0 failures).

---


### T-US16-SPACE: `serve` — search_grep auto-switches to phrase for spaced terms (US-16)

**Tool:** `search_grep`

**Background:** When `search_grep` receives terms containing spaces in substring mode (the default), it auto-switches to phrase search. Previously, spaced terms silently returned 0 results because the tokenizer splits on spaces, so no individual token contains multi-word substrings.

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"pub fn"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- `summary.totalFiles` ≥ 1 (previously returned 0 with spaced terms)
- `summary.searchMode` = `"phrase"` (auto-switched from substring)
- `summary.searchModeNote` contains `"spaces"` and `"auto-switched"` — explains the mode switch
- Results contain files with the exact phrase `"pub fn"`

**Negative test — non-spaced terms stay in substring mode:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"tokenize"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- `summary.searchMode` starts with `"substring"` (no auto-switch)
- `summary.searchModeNote` is absent

**CLI equivalent:**

```powershell
cargo run -- grep "pub fn" -d $TEST_DIR -e $TEST_EXT
```

**Expected:**

- Exit code: 0
- Results found (auto-switches to phrase internally)
- stderr shows `[substring-trace] Terms contain spaces, auto-switching to phrase mode`

**Validates:** US-16 fix — spaced terms auto-switch to phrase mode instead of silently returning 0. The `searchModeNote` field makes the behavior transparent.

**Unit tests:** `test_substring_space_in_terms_auto_switches_to_phrase`, `test_substring_space_in_terms_count_only`, `test_substring_no_space_stays_substring`, `test_substring_space_sql_create_table`

**Status:** ✅ Implemented

---

### T-BRANCH-WARNING: `serve` — `branchWarning` in index-based tool responses

**Background:** When the MCP server is started on a non-main/non-master branch, all index-based tool responses (`search_grep`, `search_definitions`, `search_callers`, `search_fast`) include a `branchWarning` field in the `summary` object. This alerts the AI agent that results may differ from production because the index is built on a feature branch.

**Setup:** Check out a feature branch before starting the server:

```powershell
git checkout -b feature/test-branch-warning
```

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"tokenize"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- `summary.branchWarning` contains `"Index is built on branch 'feature/test-branch-warning', not on main/master. Results may differ from production."`
- The warning appears in ALL index-based tools: `search_grep`, `search_definitions`, `search_callers`, `search_fast`
- The warning does NOT appear in git tools (`search_git_history`, `search_git_diff`, etc.) since they work directly with the git repo

**Negative test — on main branch:**

```powershell
git checkout main
```

Then repeat the command above.

**Expected:**

- `summary.branchWarning` is ABSENT (no warning on main/master)

**Negative test — on master branch:**

Same expectation — no warning when on `master`.

**Validates:** `branchWarning` field in `HandlerContext` is populated at server startup via `git rev-parse --abbrev-ref HEAD`, and injected into summary objects by `inject_branch_warning()` in `utils.rs`.

**Unit tests:** `test_branch_warning_feature_branch`, `test_branch_warning_main_branch`, `test_branch_warning_master_branch`, `test_branch_warning_none_branch`, `test_inject_branch_warning_adds_field`, `test_inject_branch_warning_skips_main`, `test_inject_branch_warning_skips_none`

---

### T-BRANCH-STATUS: `serve` — `search_branch_status` shows branch info

**Tool:** `search_branch_status`

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_branch_status","arguments":{"repo":"."}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response containing:
  - `currentBranch` — non-empty string (current git branch name)
  - `isMainBranch` — boolean (true iff branch is `main` or `master`)
  - `mainBranch` — `"main"` or `"master"` (whichever exists in the repo)
  - `behindMain` — integer or null (commits behind `origin/<mainBranch>`)
  - `aheadOfMain` — integer or null (commits ahead of `origin/<mainBranch>`)
  - `dirtyFiles` — array of modified file paths
  - `dirtyFileCount` — integer matching `dirtyFiles` array length
  - `lastFetchTime` — ISO timestamp string or null
  - `fetchAge` — human-readable age string (e.g., "3 hours ago") or null
  - `fetchWarning` — null if fetch is fresh (< 1 hour), escalating warnings for stale fetch
  - `warning` — null if on main/master and up-to-date; human-readable warning if on feature branch or behind remote
  - `summary.tool` = `"search_branch_status"`
  - `summary.elapsedMs` — positive number

**Error cases:**

- Missing `repo` parameter → `isError: true`, `"Missing required parameter: repo"`
- Non-existent repo path → `isError: true`, error from git

**Validates:** `search_branch_status` tool returns comprehensive branch status for production bug investigation context.

**Unit tests:** `test_branch_status_returns_current_branch`, `test_branch_status_detects_main_branch`, `test_branch_status_dirty_files`, `test_branch_status_missing_repo`, `test_branch_status_bad_repo`, `test_branch_status_has_summary`, `test_is_main_branch`, `test_format_age`, `test_compute_fetch_warning_thresholds`, `test_build_warning_on_main_up_to_date`, `test_build_warning_on_main_behind`, `test_build_warning_on_feature_branch`, `test_build_warning_on_feature_branch_no_behind`, `test_build_warning_on_feature_branch_no_remote`

---

> **Note:** `search_git_pickaxe` was removed in 2026-02-22. Use `search_grep` → `search_git_blame` workflow instead. See CHANGELOG for details.

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_pickaxe","arguments":{"repo":".","text":"search_git_pickaxe","maxResults":5}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `commits` array (non-empty if the text exists in git history)
- Each commit has `hash` (8 chars), `date`, `author`, `email`, `message`, and optionally `patch`
- `summary.tool` = `"search_git_pickaxe"`
- `summary.mode` = `"exact"`
- `summary.searchText` = `"search_git_pickaxe"`
- `summary.totalCommits` ≥ 1

**Validates:** `search_git_pickaxe` tool with exact text mode (`git log -S`).

**Status:** ✅ Covered by unit tests: `test_pickaxe_exact_mode_returns_commits`, `test_pickaxe_with_file_filter`, `test_pickaxe_max_results_limits_output`, `test_pickaxe_with_date_filters`, `test_pickaxe_no_results_narrow_date`

---

### T-GIT-PICKAXE-02: `serve` — search_git_pickaxe with regex mode

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_pickaxe","arguments":{"repo":".","text":"fn\\s+main","regex":true,"maxResults":3}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `commits` array
- `summary.mode` = `"regex"`
- Uses `git log -G` (regex mode) instead of `-S` (exact mode)

**Validates:** `search_git_pickaxe` tool with regex mode (`git log -G`).

**Status:** ✅ Covered by unit test: `test_pickaxe_regex_mode`

---

### T-GIT-PICKAXE-03: `serve` — search_git_pickaxe error handling

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_pickaxe","arguments":{"repo":"."}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `isError: true`
- Error message: "Missing required parameter: text"

**Validates:** Required parameter validation for `search_git_pickaxe`.

**Status:** ✅ Covered by unit tests: `test_pickaxe_missing_repo`, `test_pickaxe_missing_text`, `test_pickaxe_empty_text`, `test_pickaxe_bad_repo`

---

### T70: `serve` — `search_git_history` empty results validation (file not in git)

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_history","arguments":{"repo":".","file":"nonexistent_file_xyz_abc_123.rs"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `"totalCommits": 0` and `"commits": []`
- Response JSON contains a `"warning"` field: `"File not found in git: nonexistent_file_xyz_abc_123.rs. Check the path."`

**Validates:** When `search_git_history` returns 0 commits and the file is not tracked by git, the response includes a warning to help the user identify typos in the file path.

---

### T70b: `serve` — `search_git_history` empty results validation (file exists, no commits in range)

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_git_history","arguments":{"repo":".","file":"Cargo.toml","from":"1970-01-01","to":"1970-01-02"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `"totalCommits": 0` and `"commits": []`
- Response JSON does **NOT** contain a `"warning"` field — the file exists in git, it just has no commits in the specified date range

**Validates:** No false positive warnings when the file is tracked by git but has no commits in the queried date range.

---


### T-DEBUG-LOG: `serve --debug-log` — Debug logging (MCP traces + memory diagnostics)

**Tool:** `search-index serve --debug-log`

**Background:** When `--debug-log` is passed to `search-index serve`, the server writes a `.debug.log` file in the index directory (`%LOCALAPPDATA%/search-index/`) with MCP request/response traces (tool name, arguments, elapsed time, response size, Working Set) and Working Set / Peak WS / Commit memory diagnostics at every key pipeline stage.

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_info","arguments":{}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT --debug-log
```

**Expected:**

- stderr contains `[debug-log] Enabled, writing to`
- stderr contains `[memory]` lines with timing, WS_MB, Peak_MB, Commit_MB, and labels
- stderr contains `[debug-log]` lines with REQ/RESP entries for each tool call
- File `%LOCALAPPDATA%/search-index/<prefix>.debug.log` is created with header + data lines
- Labels include: `serve: startup`, `content-build: starting`, `content-build: finished`, etc.
- REQ lines include: tool name and arguments JSON
- RESP lines include: tool name, elapsed ms, response KB, Working Set MB
- When `--debug-log` is NOT passed, no `[memory]` or `[debug-log]` lines appear in stderr

**Validates:** Debug log file creation, `log_memory()` / `log_request()` / `log_response()` calls at key pipeline stages, no-op when disabled.

**Unit tests:** `test_log_memory_is_noop_when_disabled`, `test_enable_debug_log_creates_file`, `test_get_process_memory_info_returns_json`, `test_force_mimalloc_collect_does_not_panic`, `test_log_request_format`, `test_log_response_format`, `test_debug_log_path_extension`, `test_format_utc_timestamp_format`

---

### T-MEMORY-ESTIMATE: `search_info` — Memory estimates in response

**Tool:** `search_info`

**Background:** `search_info` now includes a `memoryEstimate` section with per-component memory estimates and process memory info.

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_info","arguments":{}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT --definitions
```

**Expected:**

- Response JSON contains `memoryEstimate` object
- `memoryEstimate.contentIndex` has: `invertedIndexMB`, `trigramTokensMB`, `trigramMapMB`, `filesMB`, `totalEstimateMB`, `uniqueTokens`, `totalPostings`, `fileCount`
- `memoryEstimate.definitionIndex` has: `definitionsMB`, `callSitesMB`, `filesMB`, `totalEstimateMB`, `definitionCount`
- `memoryEstimate.process` has (Windows only): `workingSetMB`, `peakWorkingSetMB`, `commitMB`
- All MB values are rounded to 1 decimal place
- All count values are integers

**Validates:** Memory estimation for all index components, process memory info via Windows API.

**Unit tests:** `test_estimate_content_index_memory_empty`, `test_estimate_content_index_memory_nonempty`, `test_estimate_definition_index_memory_empty`

---

### T-MI-COLLECT: `mi_collect(true)` — Memory decommit after build+drop+reload

**Background:** After `drop(build_index)` and before `load_from_disk()`, the server calls `mi_collect(true)` to force mimalloc to decommit freed segments. This prevents abandoned thread heaps from inflating Working Set after the build+drop+reload pattern.

**Scenario:** Start the MCP server with `--memory-log` on a large codebase (no pre-built index on disk). Observe the memory log for the drop → mi_collect → reload sequence.

**Expected memory.log entries (approximate):**

```
    2.50 |    800.0 |    800.0 |    850.0 | content-build: finished
    2.51 |    400.0 |    800.0 |    450.0 | serve: after drop(content build)
    2.52 |    200.0 |    800.0 |    250.0 | serve: after mi_collect (content)
    2.80 |    350.0 |    800.0 |    400.0 | serve: after reload content from disk
```

**Assertions:**

- WS after `mi_collect` < WS after `drop` (freed segments decommitted)
- WS after `reload` < WS at `finished` (reload is more compact than build)
- Peak WS stays at or near the build-time peak (expected — Peak is high-water mark)

**Validates:** `force_mimalloc_collect()` reduces Working Set after dropping build-time allocations. The same pattern applies to definition index and watcher bulk reindex.

**Unit test:** `test_force_mimalloc_collect_does_not_panic`

**Status:** Manual verification via `--memory-log`. The mi_collect call itself is tested for no-panic behavior.

---

### T-TOKEN-BUDGET: Tool definitions stay within token budget

**Tool:** All 15 tools via `tools/list`

**Background:** MCP tool definitions (names, descriptions, parameter schemas) are injected into the LLM system prompt on every turn. To prevent token budget bloat, parameter descriptions are kept concise (semantic purpose + defaults, no concrete examples). Examples are available on-demand via `search_help` → `parameterExamples`.

**Scenario:** Verify that the total token footprint of all tool definitions stays under the budget.

**Expected:**

- Total word count of serialized tool definitions < 4,125 words (~5,500 tokens at 0.75 words/token ratio)
- `search_help` response contains `parameterExamples` object with examples for key tools: `search_definitions`, `search_grep`, `search_callers`, `search_fast`
- `search_callers.class` parameter retains its full "STRONGLY RECOMMENDED" warning (critical hint)
- All parameter descriptions retain semantic purpose (8+ words for non-obvious params)
- No concrete examples in parameter descriptions (moved to `parameterExamples`)

**Unit tests:** `test_tool_definitions_token_budget`, `test_render_json_has_parameter_examples`

**Status:** ✅ Implemented


## Angular Template Metadata Tests

### T-ANGULAR-01: `def-index` — Angular component selector and template metadata indexed

**Command:**

```powershell
cargo run -- def-index -d $TEST_DIR -e ts
```

**Prerequisites:** Directory contains TypeScript files with `@Component({ selector: '...', templateUrl: '...' })` decorators and paired `.html` template files.

**Expected:**

- Exit code: 0
- `.code-structure` file created
- The index contains `selector_index` entries mapping selectors (e.g., `"app-root"`, `"child-widget"`) to their component definition indices
- The index contains `template_children` entries listing custom element tags found in each component's HTML template

**Validates:** Angular `@Component` decorator parsing extracts `selector` and `templateUrl`, reads the paired HTML file, and indexes custom elements (tags containing hyphens) as template children.

---

### T-ANGULAR-02: `serve` — `search_definitions` returns Angular template metadata

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"name":"AppComponent"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext ts --definitions
```

**Expected:**

- stdout: JSON-RPC response with definition results
- Each Angular component definition includes `selector` field (e.g., `"app-root"`)
- Each Angular component definition includes `templateChildren` array listing child component selectors found in its HTML template (e.g., `["child-widget", "loading-spinner"]`)

**Validates:** `search_definitions` exposes Angular template metadata (selector, templateChildren) in the response for component definitions.

---

### T-ANGULAR-03: `serve` — `search_callers` direction=down shows template children

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"AppComponent","class":"AppComponent","direction":"down"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext ts --definitions
```

**Expected:**

- stdout: JSON-RPC response with call tree
- `callTree` includes child components from the HTML template with `templateUsage: true`
- Child entries correspond to selectors found in the component's `.html` file (e.g., `child-widget`, `loading-spinner`)

**Validates:** `search_callers` direction=down includes Angular template children in the call tree, enabling component hierarchy traversal through HTML templates.

---

### T-ANGULAR-04: `serve` — `search_callers` direction=up shows parent components

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"child-widget","direction":"up"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext ts --definitions
```

**Expected:**

- stdout: JSON-RPC response with call tree
- `callTree` includes parent components that use `<child-widget>` in their HTML templates with `templateUsage: true`
- Parent entries correspond to components whose `templateChildren` contain `child-widget`

**Validates:** `search_callers` direction=up resolves selectors to parent components via the `selector_index`, enabling reverse template dependency lookup.

---

### T-ANGULAR-04b: `serve` — `search_callers` direction=up recursive depth shows grandparents

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"grand-child","direction":"up","depth":3}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext ts --definitions
```

**Prerequisites:** Directory contains a 3-level Angular component hierarchy: GrandParent uses `<child-comp>`, ChildComp uses `<grand-child>`.

**Expected:**

- stdout: JSON-RPC response with call tree
- `callTree` includes ChildComp as direct parent (level 1) with `templateUsage: true`
- ChildComp node has a `"parents"` field containing GrandParent (level 2)
- `depth` parameter controls recursion depth (depth=1 returns only direct parent, no grandparents)

**Validates:** `search_callers` direction=up recursively traces parent components beyond level 1, respecting the `depth` parameter. Grandparents are nested in the `"parents"` field of each parent node. Cycle detection prevents infinite loops for circular component references.

---

### T-F07-SERIALIZATION: MCP server handles serialization errors gracefully without crashing

**Tool:** MCP server (`server.rs`)

**Background:** Audit finding F-07 identified that `serde_json::to_string(&resp).unwrap()` and `serde_json::to_value(...).unwrap()` calls could panic the server if serialization failed (e.g., a `Value` containing NaN float). All `unwrap()` calls on response serialization have been replaced with proper error handling that logs the error and returns a JSON-RPC `-32603` internal error response instead of panicking.

**Scenario:** If the server encounters a serialization failure on any response, it should return a valid JSON-RPC error response with code `-32603` rather than crashing.

**Expected:**

- Server does NOT panic on serialization failure
- Server returns `{"jsonrpc":"2.0","id":...,"error":{"code":-32603,"message":"Internal error: ..."}}` on serialization failure
- Normal responses continue to work correctly

**Unit tests:** `test_serialize_response_error_returns_internal_error`, `test_serialize_tool_result_error_returns_internal_error`

**Status:** ✅ Covered by unit tests. Not CLI-testable (serialization failures are triggered by internal edge cases like NaN floats, not by normal user input).

---

### T-F10-CLASS-FILTER-RECURSION: Call tree search with common method names does not produce cross-class false positives at depth > 0

**Tool:** `search_callers`

**Background:** Audit finding F-10 identified that `build_caller_tree` passed `parent_class: None` at recursion depth > 0, causing false positive callers from unrelated classes with common method names like `Process`, `Execute`, `Handle` to appear in the call tree. The fix passes `caller_parent` (the class of the found caller) as the class filter for the next recursion level, matching the pattern already used in `build_callee_tree`.

**Setup:** Create C# files with two unrelated classes that both have a method named `Process`:

- `ServiceA.cs`: `public class ServiceA { public void Process() { } }`
- `ServiceB.cs`: `public class ServiceB { public void Process() { } }`
- `Consumer.cs`: `public class Consumer { private ServiceA _a; public void Run() { _a.Process(); } }`

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"Process","class":"ServiceA","direction":"up","depth":3}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TempDir --ext cs --definitions
```

**Expected:**

- At depth > 0, callers of `Consumer.Run` do NOT include false positives from `ServiceB.Process` callers
- The class filter is preserved during recursion — each deeper level uses the caller's parent class as the filter
- `summary.totalNodes` reflects only verified callers with correct class scope

**Unit test:** `test_caller_tree_preserves_class_filter_during_recursion`

**Status:** ✅ Covered by unit test. CLI-testable only with a multi-class codebase where common method names exist in unrelated classes.

---

### T-TERM-BREAKDOWN: `search_definitions` — `termBreakdown` in summary for multi-term name queries

**Tool:** `search_definitions`

**Scenario:** When `name` contains 2+ comma-separated terms, the summary includes a `termBreakdown`
object showing how many results each term contributed (from the full result set, before `maxResults`
truncation).

**Command (MCP):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"name":"QueryService,ResilientClient"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext cs --definitions
```

**Expected:**

- `summary.termBreakdown` is present (JSON object)
- Keys are lowercased term names: `"queryservice"`, `"resilientclient"`
- Values are counts (integers ≥ 0)
- Sum of all values = `summary.totalResults`
- `termBreakdown` is absent for single-term, regex, or no-name queries

**Negative tests:**

- `name="QueryService"` (single term) → no `termBreakdown` in summary
- `name="Query.*" regex=true` → no `termBreakdown` in summary
- No `name` parameter → no `termBreakdown` in summary

**Unit tests:** `test_term_breakdown_multi_term_shows_per_term_counts`, `test_term_breakdown_single_term_not_present`, `test_term_breakdown_regex_not_present`, `test_term_breakdown_no_name_filter_not_present`, `test_term_breakdown_with_zero_match_term`, `test_term_breakdown_counts_are_pre_truncation`

**Status:** ✅ Implemented

---

### T-COMMA-FILE-PARENT: `search_definitions` — Comma-separated `file` and `parent` parameters

**Tool:** `search_definitions`

**Scenario:** The `file` and `parent` parameters support comma-separated OR (matching `name` behavior).

**Command (comma-separated file):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"file":"UserService.cs,OrderService.cs","kind":"method"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext cs --definitions
```

**Expected:**

- Results include methods from BOTH `UserService.cs` AND `OrderService.cs`
- Each result's `file` path contains one of the comma-separated terms
- `summary.totalResults` ≥ 2

**Command (comma-separated parent):**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_definitions","arguments":{"parent":"UserService,OrderService","kind":"method"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext cs --definitions
```

**Expected:**

- Results include methods from BOTH `UserService` AND `OrderService` classes
- Each result's `parent` field matches one of the comma-separated terms
- `summary.totalResults` ≥ 2

**Validates:** Comma-separated OR for `file` and `parent` parameters, consistent with `name` parameter behavior.

**Unit tests:** `test_file_filter_comma_separated_matches_multiple_files`, `test_file_filter_single_value_still_works`, `test_file_filter_comma_separated_no_match_returns_empty`, `test_parent_filter_comma_separated_matches_multiple_classes`, `test_parent_filter_single_value_still_works`, `test_parent_filter_comma_separated_no_match_returns_empty`, `test_parent_filter_comma_with_spaces_trimmed`

---

### T-STALE-CACHE: Stale index cache skipped when extensions change

**Tool:** `find_definition_index_for_dir`, `find_content_index_for_dir`

**Background:** When the MCP server is restarted with different `--ext` parameters (e.g., adding `sql` to a previously `cs`-only setup), the fallback index loader must validate that the cached index's extensions match the requested ones. Previously, it would load a stale cache with only `cs` definitions, ignoring the newly requested `sql` extension.

**Scenario:** Build a definition index with `--ext cs`, then restart the server with `--ext cs,sql`. The old `cs`-only cache should be skipped and a full rebuild should be triggered.

**Expected:**

- stderr contains `Skipping ... — extensions mismatch (cached: ["cs"], expected: ["cs", "sql"])`
- The server rebuilds the index with both `cs` and `sql` extensions
- `search_definitions` with `kind: "storedProcedure"` returns results (if SQL files exist)
- Same behavior for content index fallback (`find_content_index_for_dir`)

**Unit tests:** `test_find_def_index_skips_stale_extensions`, `test_find_def_index_accepts_superset`, `test_find_def_index_accepts_exact_match`, `test_find_def_index_empty_expected_accepts_any`, `test_find_def_index_case_insensitive_ext_match`, `test_find_content_index_skips_stale_extensions`, `test_find_content_index_accepts_superset`, `test_find_content_index_empty_expected_accepts_any`

**Status:** ✅ Covered by 8 unit tests

---

### T-ANGULAR-05: `def-index` — Graceful handling of missing HTML template

**Command:**

```powershell
cargo run -- def-index -d $TEST_DIR -e ts
```

**Prerequisites:** Directory contains a TypeScript file with `@Component({ templateUrl: './nonexistent.html' })` pointing to a file that does not exist.

**Expected:**

- Exit code: 0 (no crash)
- Component is indexed normally but has no `templateChildren` entries
- stderr does NOT contain errors about missing HTML file (handled gracefully)

**Validates:** `def-index` completes without error when a component's `templateUrl` points to a non-existent file. The component is still indexed for its selector but without template children.


### T-BFS-CASCADE: `search_definitions` — `baseTypeTransitive` BFS no longer cascades

**Tool:** `search_definitions`

**Background:** The `collect_transitive_base_type_indices()` BFS previously used substring matching (`key.contains(&current_type)`) at ALL levels. When a descendant class had a short/common name (e.g., `"Service"`), BFS level 1+ would substring-match many unrelated base_type keys (`"iservice"`, `"webservice"`, `"serviceprovider"`), pulling thousands of definitions into the result set (~42K instead of ~828, ~29 sec instead of <1 sec). The fix uses exact HashMap lookup at levels 1+, keeping substring matching only at level 0 (seed) for generic type support.

**Expected:**

- `baseType="BaseBlock" baseTypeTransitive=true` returns < 5000 results (expected ~800-2000) and completes in < 500ms
- Generic types still work at seed level: `baseType="IRepository" baseTypeTransitive=true` finds classes inheriting `IRepository<Model>`, `IRepository<Report>`, etc.
- Transitive chain works: `BaseService → MiddleService → ConcreteService` all found

**Unit tests:** `test_base_type_transitive_no_cascade_with_dangerous_names`, `test_base_type_transitive_generics_still_work_at_seed_level`, `test_base_type_transitive_finds_indirect_descendants`, `test_base_type_transitive_case_insensitive`, `test_base_type_transitive_no_match_returns_empty`

**Status:** ✅ Covered by unit tests

---

### T-CALLERS-HINT: `search_callers` — Hint when 0 results with class filter

**Tool:** `search_callers`

**Background:** When `search_callers` returns an empty call tree and `class` parameter is set, the response now includes a `hint` field suggesting possible reasons (extension methods, DI wrappers, narrow class filter) and advising to try without `class` or with the interface name.

**Expected:**

- `method="X" class="NonExistent"` → response includes `"hint"` field mentioning class parameter
- `method="X"` (no class filter) → response does NOT include `"hint"` even if tree is empty
- `method="X" class="ValidClass"` with callers found → response does NOT include `"hint"`

**Unit tests:** `test_search_callers_hint_when_empty_with_class_filter`, `test_search_callers_no_hint_without_class_filter`, `test_search_callers_no_hint_when_results_found`

**Status:** ✅ Covered by unit tests

---

### T-TRANSITIVE-HINT: `search_definitions` — Hint for large transitive hierarchies

**Tool:** `search_definitions`

**Background:** When `baseTypeTransitive=true` and `totalResults > 5000`, the summary includes a `hint` suggesting `kind` or `file` filters to narrow results.

**Expected:**

- Small result set (< 5000) → no `hint` in summary
- Large result set (> 5000) → `hint` present mentioning `kind` and `file` filters

**Unit test:** `test_base_type_transitive_hint_for_large_hierarchy`

**Status:** ✅ Covered by unit test (negative case; positive case requires 5000+ definitions)

---

### T-TOMBSTONE: Definition index tombstone compaction during `--watch`

**Tool:** `search-index serve --watch --definitions`

**Background:** When the file watcher incrementally updates definitions, old entries remain in the `definitions` Vec as tombstones. This causes `definitions.len()` to grow monotonically, inflating `totalDefinitions` and wasting memory. The fix adds auto-compaction when tombstone ratio exceeds 3× and reports active count instead of Vec length.

**Scenario (totalDefinitions shows active count):**

1. Start MCP server with `--watch --definitions`
2. Query `search_definitions` — note `totalDefinitions` in summary
3. Modify a `.cs` file (change class name), wait for watcher debounce
4. Query `search_definitions` again — `totalDefinitions` should be ~same (not growing)

**Expected:**

- `totalDefinitions` reflects active definitions only (not Vec length with tombstones)
- After many file updates, `totalDefinitions` stays stable (±1 per file update)
- `search_info` definition count matches `totalDefinitions` from `search_definitions`

**Scenario (auto-compaction):**

1. Trigger many incremental updates to the same file (>4× to exceed 3× threshold)
2. Observe stderr log: `"Definition index tombstone threshold exceeded, compacting"`
3. After compaction: `definitions` Vec length ≈ active count

**Unit tests:** `test_compact_removes_tombstones`, `test_compact_no_tombstones_is_noop`, `test_compact_remaps_method_calls_and_code_stats`, `test_compact_auto_triggers_at_threshold`, `test_compact_remaps_selector_index_and_template_children`

**Status:** ✅ Covered by unit tests. Manual MCP test requires `--watch` mode.

---

### T-RECONCILE: Watcher startup reconciliation — catches stale cache files

**Tool:** `search-index serve --watch --definitions`

**Background:** When the MCP server starts with `--watch` and loads indexes from a stale disk cache, files added/modified/deleted while the server was offline are permanently invisible because the `notify` file watcher only fires events for changes AFTER it starts watching. The reconciliation scan at watcher startup walks the filesystem and compares with the loaded index using path diff (added/deleted) and mtime comparison (modified), fixing all three cases.

**Scenario (added files):**

1. Build definition index for a temp directory with 1 file
2. Add a new `.cs` file to the directory
3. Restart the server with `--watch --definitions`
4. Query `search_definitions` for the new file — should find definitions

**Scenario (modified files):**

1. Build definition index for a temp directory with 1 file
2. Modify the file content (change class name)
3. Restart the server with `--watch --definitions`
4. Query `search_definitions` — should find the new class name, not the old one

**Scenario (deleted files):**

1. Build definition index for a temp directory with 2 files
2. Delete one file
3. Restart the server with `--watch --definitions`
4. Query `search_definitions` for the deleted file — should return 0 results

**Expected (all scenarios):**

- stderr contains reconciliation log: `Definition index reconciliation complete` with `added`, `modified`, `removed` counts
- stderr contains cache age in startup log (e.g., `cache_age=5m`)
- Indexes are accurate after reconciliation without manual `search_reindex_definitions`

**Validates:** Watcher startup reconciliation catches stale cache files. The reconciliation runs once in the watcher thread before the event loop, with filesystem events buffered in the mpsc channel during the scan.

**Unit tests:** `test_reconcile_adds_new_file`, `test_reconcile_removes_deleted_file`, `test_reconcile_detects_modified_file`, `test_reconcile_skips_unchanged_files`

**Status:** ✅ Covered by unit tests. Manual MCP test requires start/stop server workflow.

---
---
