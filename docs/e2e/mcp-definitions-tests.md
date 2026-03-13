# MCP `xray_definitions` Tests

Tests for the `xray_definitions` MCP tool: body extraction, containsLine, auto-summary, hints, auto-correction, code stats, ranking, audit, and related features.

---

## Basic Definitions

### T28: `serve` — MCP xray_definitions (requires --definitions)

**Expected:**

- For Rust codebase: results with Rust definitions
- For C# or TypeScript codebase: results with `name`, `kind`, `file`, `lines`
- For SQL codebase: results with `kind` (storedProcedure, table, view, etc.)

**Note:** Requires `--definitions` flag.

---

## Body Extraction (`includeBody`)

### T28a: `serve` — xray_definitions with `includeBody: true`

**Expected:**

- Each definition object contains `"bodyStartLine"` and `"body"` array (string array of source lines)
- `summary` includes `"totalBodyLinesReturned"` field

---

### T28b: `serve` — xray_definitions with `includeBody: true, maxBodyLines: 5`

**Expected:**

- Each definition's `"body"` array has at most 5 entries
- If longer: `"bodyTruncated": true` and `"totalBodyLines"` present

---

### T28c: `serve` — Backward compatibility (default `includeBody: false`)

**Expected:**

- Definition objects do NOT contain a `"body"` field

---

### T28d: `serve` — xray_definitions with `containsLine` + `includeBody: true`

**Expected:**

- Result includes `"containingDefinitions"` array
- Body emitted ONLY for the innermost definition; parent gets `bodyOmitted` hint

**Unit tests:** `test_contains_line_body_only_for_innermost`, `test_contains_line_single_match_gets_body_normally`

---

### T28e: `serve` — xray_definitions with `maxTotalBodyLines` budget exhaustion

**Expected:**

- First few definitions have `"body"` arrays with content
- Later definitions have `"bodyOmitted"` marker
- Total body lines ≤ budget

---

### T28f-doc: `serve` — xray_definitions with `includeDocComments: true`

**Expected:**

- `body` array starts with doc-comment lines
- `bodyStartLine` points to the first doc-comment line
- `docCommentLines` field present
- Budget respected: doc-comment lines count against `maxBodyLines`

**Unit tests:** `test_find_doc_comment_start_csharp_triple_slash`, `test_inject_body_with_doc_comments_csharp`

**Status:** ✅ Implemented

---

### T28g-body-range: `serve` — xray_definitions with `bodyLineStart`/`bodyLineEnd`

**Expected:**

- `body` array contains only lines within the [bodyLineStart, bodyLineEnd] range
- `bodyStartLine` reflects the filtered start
- Range outside method → empty array (no panic)
- Doc comment expansion skipped when `bodyLineStart` is set
- Also supported in `xray_callers` for `rootMethod` body

**Unit tests:** `test_inject_body_body_line_range_filter`, `test_xray_definitions_body_line_range_filter`

**Status:** ✅ Implemented

---

## Attribute & Dedup

### T28f: `serve` — xray_definitions by attribute returns no duplicates

**Expected:**

- No duplicate entries: each class appears at most once per attribute name
- `totalResults` matches unique definitions count

---

### T28g: `serve` — xray_definitions with `maxResults: 0` (unlimited)

**Expected:**

- `summary.totalResults` equals `summary.returned` (no capping at 100)
- `definitions` array contains ALL matching definitions

---

## Auto-Summary

### T-AS1: `xray_definitions` — Auto-summary triggered on broad query

**Expected:**

- Response contains `autoSummary` object (not `definitions` array)
- `autoSummary.groups` is a non-empty array with `directory`, `total`, `counts`, `topDefinitions`
- `summary.autoSummaryMode` is `true`

---

### T-AS2: `xray_definitions` — Auto-summary NOT triggered with name filter

**Expected:**

- Response contains `definitions` array (normal format)
- No `autoSummary` field

---

### T-AS3: `xray_definitions` — Auto-summary NOT triggered when results fit

**Expected:**

- Response contains `definitions` array (normal format)

---

## Response Truncation

### T52: `serve` — Response truncation for `xray_definitions` broad queries

**Expected (if > 16KB):**

- `summary.responseTruncated` = `true`
- `summary.truncationReason` contains `"truncated 'definitions' array"`
- `summary.hint` mentions `"name, kind, file, or parent filters"`

---

## Relevance Ranking

### T-RANK-01: `best_match_tier()` — Match tier classification

- Tier 0: exact match (case-insensitive)
- Tier 1: prefix match
- Tier 2: contains/default

**Status:** ✅ 9 unit tests in `utils.rs`

---

### T-RANK-02: `kind_priority()` — Definition kind tiebreaker

- Priority 0: class, interface, enum, struct, record
- Priority 1: method, function, property, field, etc.

**Status:** ✅ 16 unit tests

---

### T-RANK-03: `xray_definitions` — Relevance ranking (exact → prefix → contains)

**Expected order:** UserService (exact) → UserServiceFactory (prefix) → IUserService (contains)

**Unit tests:** `test_xray_definitions_ranking_exact_first`, `test_xray_definitions_ranking_prefix_before_contains`

**Status:** ✅ Implemented

---

### T-RANK-06: `xray_definitions` — Parent relevance ranking

**Expected:** Exact parent match (tier 0) before prefix (tier 1) before contains (tier 2)

**Unit tests:** `test_parent_ranking_exact_parent_before_substring_parent`

**Status:** ✅ Implemented

---

## Hints (Zero-Result Diagnostics)

### T-ZERO-HINTS: `xray_definitions` — Zero-result hints for LLM self-correction

- **(A) Wrong kind** — definitions exist with different kind → `"Did you mean kind='function'?"`
- **(B) Nearest name match** — Jaro-Winkler ≥80% → `"Nearest match: 'getuser'"`
- **(C) File has definitions** — file matches but name/kind/parent too narrow
- **(D) Name in content index** — name exists as text but not as AST definition

**Unit tests:** `test_hint_wrong_kind`, `test_hint_nearest_name_match`, `test_hint_file_has_defs_but_name_not_found`, `test_hint_name_in_content_not_in_defs`

**Status:** ✅ Implemented

---

### T-HINT-E: Unsupported file extension hint

**Expected:** `.xml` suggests `xray_grep`; unknown ext suggests `read_file`

**Unit tests:** `test_hint_e_xml_extension_suggests_xray_grep`

**Status:** ✅ Implemented

---

### T-HINT-NAME-KIND: Name+kind mismatch hint

**Expected:** `name=UserService kind=method` → hint suggests `parent='UserService'`

**Unit tests:** `test_hint_name_kind_mismatch_class_with_method_kind`

**Status:** ✅ Implemented

---

### T-HINT-F-FILE-FUZZY: File path fuzzy-match hint

**Expected:** Near-miss file path detected and suggested

**Unit tests:** `test_hint_f_file_fuzzy_match_slash_mismatch`

**Status:** ✅ Implemented

---

## Auto-Correction

### T-AUTO-CORRECT: Kind mismatch and name typo auto-correction

- **(A) Kind mismatch** — removes kind filter, finds correct kind, re-runs
- **(B) Nearest name** — ≥85% Jaro-Winkler → re-runs with corrected name
- Response includes `autoCorrection` object in summary

**Unit tests:** `test_auto_correct_kind_method_to_function`, `test_auto_correct_name_typo`

**Status:** ✅ Implemented

---

### T-AUTO-CORRECT-LENGTH-RATIO: Length ratio guard

**Expected:** ≥60% length ratio required in addition to ≥80% similarity

**Unit tests:** `test_auto_correct_name_blocked_by_length_ratio`

**Status:** ✅ Implemented

---

## Code Stats

### T-CODESTATS-01: `includeCodeStats=true` returns metrics

**Expected:** Methods include `codeStats` with `cyclomaticComplexity`, `cognitiveComplexity`, `maxNestingDepth`, `paramCount`, `returnCount`, `callCount`, `lambdaCount`

---

### T-CODESTATS-02: `sortBy` sorts by metric descending

**Expected:** Results sorted in descending order by specified metric

---

### T-CODESTATS-03: `min*` filters restrict results

**Expected:** `minComplexity: 5` returns only methods with cyclomatic complexity ≥ 5

---

### T-CODESTATS-04: Invalid `sortBy` value returns error

**Expected:** Error listing valid `sortBy` values

---

### T-CODESTATS-05: `xray_reindex_definitions` includes `codeStatsEntries`

**Expected:** Response JSON contains `codeStatsEntries` field

---

### T-CODESTATS-06: Backward compatibility with old index (no stats)

**Expected:** `summary.codeStatsAvailable` = `false`, no error

---

### T-CODESTATS-07: `sortBy` with old index returns error

**Expected:** Error recommending `xray_reindex_definitions`

---

## Audit Mode

### T-AUDIT: Definition index audit mode (MCP)

**Expected:** Response contains `audit` object with `totalFiles`, `filesWithDefinitions`, `suspiciousFiles`

---

## Handler-Level Unit Tests

### T69: `xray_definitions` — Regex name filter

**Unit test:** `test_xray_definitions_regex_name_filter`

---

### T70: `xray_definitions` — Audit mode

**Unit test:** `test_xray_definitions_audit_mode`

---

### T71: `xray_definitions` — `excludeDir` filter

**Unit test:** `test_xray_definitions_exclude_dir`

---

### T72: `xray_definitions` — Combined name + parent + kind filter

**Unit test:** `test_xray_definitions_combined_name_parent_kind_filter`

---

### T73: `xray_definitions` — Nonexistent name returns empty

**Unit test:** `test_xray_definitions_nonexistent_name_returns_empty`

---

### T74: `xray_definitions` — Invalid regex error

**Unit test:** `test_xray_definitions_invalid_regex_error`

---

### T75: `xray_definitions` — `kind="struct"` filter

**Unit test:** `test_xray_definitions_struct_kind`

---

### T76: `xray_definitions` — `baseType` filter

**Unit test:** `test_xray_definitions_base_type_filter`

---

### T77: `xray_definitions` — `kind="enumMember"` filter

**Unit test:** `test_xray_definitions_enum_member_kind`

---

### T78: `xray_definitions` — File filter slash normalization

**Unit tests:** `test_xray_definitions_file_filter_forward_slash`, `test_xray_definitions_file_filter_backslash`

---

## Multi-Term Features

### T-TERM-BREAKDOWN: `termBreakdown` in summary for multi-term name queries

**Expected:** Keys are lowercased terms, values are counts

**Unit tests:** `test_term_breakdown_multi_term_shows_per_term_counts`

---

### T-MULTI-KIND: Multi-kind filter

**Expected:** `kind="class,method"` returns both classes and methods

**Unit tests:** `test_collect_candidates_multi_kind_filter`

---

### T-MISSING-TERMS: `missingTerms` in summary for multi-name + kind queries

**Expected:** Each entry has `term` and `reason` (e.g., `"kind mismatch"`)

**Unit tests:** `test_compute_missing_terms_kind_mismatch`

**Status:** ✅ Implemented

---

### T-COMMA-FILE-PARENT: Comma-separated `file` and `parent` parameters

**Expected:** OR matching for comma-separated values

**Unit tests:** `test_file_filter_comma_separated_matches_multiple_files`, `test_parent_filter_comma_separated_matches_multiple_classes`

---

## Base Type Transitive

### T-BFS-CASCADE: `baseTypeTransitive` BFS no longer cascades

**Expected:** < 5000 results and < 500ms for common base types

**Unit tests:** `test_base_type_transitive_no_cascade_with_dangerous_names`

**Status:** ✅ Implemented

---

### T-TRANSITIVE-HINT: Hint for large transitive hierarchies

**Expected:** > 5000 results → hint suggesting `kind` or `file` filters

**Unit test:** `test_base_type_transitive_hint_for_large_hierarchy`

---

## Input Validation

### T-VAL-01: Empty name treated as no filter

**Unit test:** `test_xray_definitions_empty_name_treated_as_no_filter`

---

### T-VAL-02: Negative `containsLine` returns error

**Expected:** `isError: true`, message: `"containsLine must be >= 1"`

**Unit tests:** `test_xray_definitions_contains_line_negative_returns_error`
