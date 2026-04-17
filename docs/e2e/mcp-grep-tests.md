# MCP `xray_grep` Tests

Tests for the `xray_grep` MCP tool: substring search, phrase search, truncation, auto-switch, and related features.

---

## Basic MCP Grep

### T27: `serve` — MCP xray_grep via tools/call

**Command:**

```powershell
$msgs = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"xray_grep","arguments":{"terms":"tokenize"}}}'
) -join "`n"
echo $msgs | cargo run -- serve --dir $TEST_DIR --ext $TEST_EXT
```

**Expected:**

- stdout: JSON-RPC response with `"result"` containing search results
- Result content includes `files` array and `summary` object
- `summary.totalFiles` > 0

**Validates:** MCP tool dispatch, xray_grep handler, JSON-RPC tools/call.

---

### T27a: `serve` — xray_grep with `showLines: true` (compact grouped format)

**Expected:**

- Each file object contains a `"lineContent"` array
- Each element in `lineContent` is a group with:
  - `"startLine"` (integer, 1-based)
  - `"lines"` (string array)
  - `"matchIndices"` (integer array, 0-based, optional)
- No old-format fields (`line`, `text`, `isMatch`)

**Validates:** `showLines` returns compact grouped format.

---

### T27b: `serve` — xray_grep phrase search with `showLines: true`

**Validates:** Phrase search code path produces identical compact grouped format to token search path.

---

### T30: `serve` — MCP xray_grep with subdirectory `dir` parameter

**Expected:**

- Subdirectory of `--dir`: results contain only files within that subdirectory
- Directory outside `--dir`: error response with `isError: true`

**Validates:** Directory validation for `xray_grep`.

---

## Substring Search

### T33: `serve` — xray_grep with `substring: true` (basic)

**Expected:**

- Files containing tokens that have the query as a substring
- `matchedTokens` field listing matched index tokens
- `summary.totalFiles` > 0

**Status:** ✅ Implemented (covered by `e2e_substring_search_full_pipeline` unit test)

---

### T34: `serve` — xray_grep with `substring: true` + short query warning

**Expected:**

- Result includes a `"warning"` field about short substring queries (<4 chars)

**Status:** ✅ Implemented

---

### T35: `serve` — xray_grep with `substring: true` + `showLines: true`

**Validates:** Substring search combined with `showLines`.

**Status:** ✅ Implemented

---

### T36: `serve` — xray_grep `substring: true` mutually exclusive with `regex`

**Expected:**

- Error response indicating `substring` and `regex` are mutually exclusive

**Status:** ✅ Implemented

---

### T37: `serve` — xray_grep `substring: true` mutually exclusive with `phrase`

**Expected:**

- Error response indicating `substring` and `phrase` are mutually exclusive

**Status:** ✅ Implemented

---

### T37a: `serve` — xray_grep defaults to substring mode (no explicit param)

**Expected:**

- `searchMode` containing `"substring"` (not `"or"` or `"and"`)
- Results include compound token matches

**Validates:** `substring` defaults to `true`.

**Status:** ✅ Implemented

---

### T37b: `serve` — regex auto-disables substring (no error)

**Expected:**

- JSON-RPC response with search results (NOT an error)
- `searchMode` does NOT contain `"substring"`

**Validates:** `regex: true` without explicit `substring: false` auto-disables substring.

**Status:** ✅ Implemented

---

### T37c: `serve` — xray_grep substring AND-mode correctness

**Expected:**

- Results only include files matching BOTH search terms as substrings
- `terms_matched` counts distinct search terms, not matching tokens

**Validates:** Fix for AND-mode correctness bug in substring search.

**Status:** ✅ Implemented

---

### T37d: `serve` — xray_grep phrase post-filter for raw content matching

**Expected:**

- Only files containing the **literal** string are returned
- Tokenized matches without the literal content are filtered out

**Validates:** Phrase raw content matching for non-alphanumeric characters (XML tags, etc.).

**Status:** ✅ Implemented

### T37e: `serve` — xray_grep phrase matches full XML tags literally

**Input:** `terms='<MaxRetries>3</MaxRetries>', phrase=true`

**Expected:**

- Finds exactly 1 file containing the literal XML tag
- No false positives from files containing `MaxRetries` or `3` separately

**Validates:** Phrase search handles XML angle brackets and full tag matching without escaping.

**Unit test:** `test_phrase_postfilter_xml_full_tag`

**Status:** ✅ Implemented

---

---

## Auto-Switch to Phrase

### T-US16-SPACE: `serve` — xray_grep auto-switches to phrase for spaced terms

**Expected:**

- `summary.totalFiles` ≥ 1 (previously returned 0)
- `summary.searchMode` = `"phrase"` (auto-switched)
- `summary.searchModeNote` contains `"spaces"` and `"auto-switched"`
- Non-spaced terms stay in substring mode

**Unit tests:** `test_substring_space_in_terms_auto_switches_to_phrase`, `test_substring_no_space_stays_substring`

**Status:** ✅ Implemented

---

### T-US16-PUNCT: `serve` — xray_grep auto-switches to phrase for punctuation terms

**Expected:**

- `summary.searchMode` = `"phrase"` (auto-switched)
- `summary.searchModeNote` contains `"non-token characters"` and `"auto-switched"`
- When triggered by punctuation (dots/brackets), `searchModeNote` contains `"Tip:"` and `"~100x slower"`
- When triggered by spaces only, `searchModeNote` does NOT contain `"Tip:"` (phrase is correct for spaces)

**Unit tests:** `test_auto_switch_phrase_hint_is_actionable`

### T-COUNTONLY-NO-TOKENS: `serve` — xray_grep countOnly=true does NOT include matchedTokens

**Input:** `{"terms": "service", "substring": true, "countOnly": true}`

**Expected:**

- `summary.totalFiles` ≥ 1
- `summary.totalOccurrences` ≥ 1
- `summary.matchedTokens` is ABSENT (not just empty)
- No `responseTruncated` from matchedTokens capping

**Unit tests:** `test_substring_count_only_no_matched_tokens`, `test_substring_non_count_only_still_has_matched_tokens`
- Alphanumeric+underscore terms stay in substring mode

**Unit tests:** `test_auto_switch_with_punctuation_returns_some`, `test_has_non_token_chars_brackets`

**Status:** ✅ Implemented

---

## Response Truncation

### T42: `serve` — Response size truncation for broad queries

Progressive truncation to stay within ~32KB:

1. Cap `lines` arrays per file to 10 entries
2. Remove `lineContent` blocks
3. Cap `matchedTokens` to 20 entries
4. Remove `lines` arrays entirely
5. Reduce file count

**Expected:**

- `summary.responseTruncated` = `true`
- `summary.truncationReason` contains truncation phases applied
- `summary.originalResponseBytes` > 32768
- `summary.totalFiles` and `summary.totalOccurrences` reflect FULL result set
- Small queries are NOT truncated

**Validates:** Progressive response truncation, LLM context budget protection.

---

## Non-Code File Search

### T41: `grep` — Non-code file search (csproj, xml, config)

**Validates:** `xray_grep` works with non-code file extensions like `.csproj`.

---

### T41a: `serve` — MCP xray_grep with ext='csproj' override

**Validates:** MCP `xray_grep` `ext` parameter works with non-code extensions.

---

## Reindex

### T38: `serve` — xray_reindex rebuilds trigram index

**Expected:**

- Reindex response: success
- Subsequent substring search works correctly

**Status:** ✅ Implemented

---

## Handler-Level Unit Tests (xray_grep)

### T65: `xray_grep` — Response truncation via small budget

**Unit test:** `test_xray_grep_response_truncation_via_small_budget`

---

### T66: `xray_grep` — SQL extension filter

**Unit test:** `test_xray_grep_sql_extension_filter`

---

### T67: `xray_grep` — Phrase search with showLines from SQL files

**Unit test:** `test_xray_grep_phrase_search_with_show_lines`

---

### T68: `xray_grep` — `maxResults=0` means unlimited

**Unit test:** `test_xray_grep_max_results_zero_means_unlimited`

---

## Input Validation & Fixes

### T-VAL-06: `xray_grep` — `contextLines` auto-enables `showLines`

**Expected:**

- `contextLines: 3` without `showLines: true` auto-enables `showLines`
- Response includes `lineContent` arrays

**Unit test:** `test_xray_grep_context_lines_auto_enables_show_lines`

---

### T-VAL-07: `xray_grep` — `matchedTokens` filtered by dir/ext/exclude

**Expected:**

- `matchedTokens` only contains tokens from files that passed all filters

**Unit tests:** `test_substring_matched_tokens_filtered_by_dir`, `test_substring_matched_tokens_filtered_by_ext`

---

### T-CR-02: `xray_grep` — Multi-extension ext filter

**Expected:**

- `ext: "cs,sql"` returns files with both extensions

**Unit tests:** `test_matches_ext_filter_single`, `test_matches_ext_filter_multi`

---

### T-CR-05: `xray_grep` — Warnings array

**Expected:**

- Short substring queries return `summary.warnings` (array) instead of `summary.warning` (string)

---

### T-CR-07: `xray_grep` — Empty terms in normal mode returns error

**Expected:**

- `terms: ",,,"` returns `isError: true`, message: `"No search terms provided"`

---

### T-GREP-DIR-FILE: `xray_grep` — dir= pointing to a file auto-converts to parent + file filter

**Input:** `{"terms":"httpclient","dir":"<absolute-path-to-Service.cs>"}`

**Expected:**

- `isError: false` (no error — the request succeeds)
- `summary.dirAutoConverted` is a string containing:
  - The word `auto-converted`
  - The filename (e.g., `Service.cs`)
  - The next-time pattern `file='<name>'`
- Results are scoped to files whose basename/path contains the auto-populated file filter

**Rationale:** LLMs frequently pass a file path in `dir=`. Instead of hard-failing, the tool transparently splits it into `dir=<parent>` + `file=<basename>`, runs the search, and teaches the correct pattern via the `dirAutoConverted` note so the LLM self-corrects next turn.

**Unit tests:** `test_parse_grep_args_dir_as_file_path_auto_converts_by_heuristic`, `test_parse_grep_args_dir_as_real_file_auto_converts`, `test_grep_dir_as_file_auto_converts_to_parent_plus_file_filter`

**Status:** ✅ Implemented

---

### T-GREP-FILE-FILTER: `xray_grep` — explicit `file=` filter narrows results

**Input:** `{"terms":"httpclient","file":"Service"}`

**Expected:**

- `isError: false`
- Every returned file path contains the `file=` substring (case-insensitive, matched against both full path and basename)
- Narrowed `totalFiles` ≤ baseline `totalFiles` (for the same `terms` without `file=`)
- Comma-separated values (`file="Service,Client"`) use OR semantics: each returned path matches at least one of the terms
- Combines with `dir`/`ext`/`excludeDir` via AND

**Rationale:** Replaces the old pattern of searching the whole dir and post-filtering — now the filter is applied during index traversal (cheaper, no allocations for non-matching files).

**Unit tests:** `test_parse_grep_args_explicit_file_filter`, `test_parse_grep_args_explicit_file_wins_over_autoconvert`, `test_grep_explicit_file_filter_narrows_results`, `test_grep_file_filter_exact_name_matches_single_file`, `test_grep_file_filter_comma_separated_or_semantics`

**Status:** ✅ Implemented

---

## Performance

### T-WARMUP: Trigram pre-warming eliminates cold-start penalty

**Expected:**

- stderr contains `[warmup] Starting trigram pre-warm...` and completion timing
- First substring query completes in < 100ms (not ~3.4s cold-start)

**Unit tests:** `test_warm_up_empty_index`, `test_warm_up_with_data`

**Status:** ✅ Implemented

---

### T-SUBSTRING-TRACE: Substring search emits timing traces to stderr

**Expected:**

- stderr contains `[substring-trace]` lines for each processing stage
- stdout (JSON-RPC response) is NOT affected

**Status:** ✅ Implemented

---

## Tips — grep-related

### T41b: `tips` / `xray_help` — Non-code file tip present

**Validates:** Tip about searching non-code file types is visible.

---

### T-RANK-05: `xray_grep` phrase mode — Sort by occurrence count descending

**Expected:**

- Files with more occurrences appear first

**Unit test:** `test_xray_grep_phrase_sort_by_occurrences`

**Status:** ✅ Implemented
