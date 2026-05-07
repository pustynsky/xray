# MCP `xray_callers` Tests

Tests for the `xray_callers` MCP tool: call trees (up/down), DI resolution, overloads, type inference, false positive filtering, and related features.

---

## Basic Callers

### T29: `serve` — MCP xray_callers (requires --definitions)

**Expected:**

- Result includes `callTree` array, `query` object, `summary` object
- Rust call-graph extraction is supported (`extract_rust_call_sites` in `src/definitions/parser_rust.rs`); see [language-tests.md](language-tests.md) for the supported call-site shapes (method calls, static calls, free function calls).

---

### T30: `serve` — MCP xray_callers with class filter and direction=down

**Expected:**

- `query.direction` = "down"
- `query.class` = specified class name
- Result includes `callTree`, `summary`

---

## Body in Call Trees (`includeBody`)

### T29a: `serve` — xray_callers with `includeBody: true` (direction=up)

**Expected:**

- Each node in `callTree` contains `body` and `bodyStartLine`
- Response budget automatically raised to at least `INCLUDE_BODY_MIN_RESPONSE_BYTES = 65_536` bytes (~64 KB)

---

### T29b: `serve` — xray_callers with `includeBody: true` and `maxBodyLines` limit

**Expected:**

- Each node's `body` array has at most N entries
- `bodyTruncated: true` if longer

---

### T29c: `serve` — xray_callers with `maxTotalBodyLines` budget

**Expected:**

- Later nodes have `bodyOmitted` instead of `body`
- Total body lines ≤ budget

---

### T29d: `serve` — xray_callers with `includeBody: true` direction=down

**Expected:**

- Callee nodes contain `body` and `bodyStartLine`

---

### T29e: `serve` — Backward compatibility (default `includeBody: false`)

**Expected:**

- Call tree nodes do NOT contain `body`, `bodyStartLine`, or `bodyOmitted`

**Unit tests:** `test_xray_callers_include_body_default_false`, `test_xray_callers_include_body_up`, `test_xray_callers_include_body_down`

**Status:** ✅ Implemented

---

### T29f: `serve` — Global response budget increase for `includeBody=true`

**Expected:**

- `includeBody=true` → at least 64 KB (`INCLUDE_BODY_MIN_RESPONSE_BYTES = 65_536`)
- `includeBody=false` → default 16 KB (`DEFAULT_MAX_RESPONSE_BYTES = 16_384`)
- Multi-method `xray_callers` adds a per-method scaler of `MULTI_METHOD_RESPONSE_BYTES_PER = 32_768`, capped at `MULTI_METHOD_RESPONSE_MAX = 131_072` (128 KB)

**Status:** ✅ Implemented

---

## Edge Cases

### T31: `serve` — xray_callers finds callers through prefixed fields (C# only)

**Expected:**

- `callTree` includes callers using `m_className`, `_className` patterns
- Uses trigram index for substring matching

---

### T32: `serve` — xray_callers works with multi-extension `--ext` flag

**Expected:**

- `callTree` is NOT empty when ext contains commas
- Files with `.cs` extension NOT filtered out despite multi-ext string

**Validates:** Fix for ext_filter comma-split bug.

---

### T59: `serve` — xray_callers ambiguity warning truncated for common methods

**Expected:**

- Warning lists at most 10 class names followed by "…"
- Total warning length stays under ~500 bytes

**Status:** ✅ Implemented

---

## Generic Methods

### T-GENERIC-CALLERS: Generic method calls correctly matched with class filter

**Expected:**

- `callTree` includes caller for `SearchAsync<Document>("q")`
- Generic type arguments stripped from stored `method_name`

**Unit tests:** `test_generic_method_call_via_member_access`, `test_verify_call_site_target_generic_method_call`

**Status:** ✅ Implemented

---

## Chained Calls

### T-CHAINED-CALLS: Chained method calls extracted (C# and TypeScript)

**Expected:**

- `callTree` includes callers from chained calls like `.ConfigureAwait(false)`
- Inner calls in chains are extracted

**Status:** ✅ Implemented

---

## DI Resolution

### T-CTOR-ASSIGN: Constructor body assignment-based DI field resolution

**Unit tests (4):** `test_call_site_extraction_constructor_assignment_m_prefix`, `test_call_site_extraction_constructor_assignment_this_prefix`, etc.

---

### T-OWNER-FIELD: Owner.m_field nested class DI resolution

**Unit tests (2):** `test_owner_m_field_nested_class_receiver_resolution`, `test_owner_m_field_inner_class_field_takes_precedence`

---

### T-FUZZY-DI: Fuzzy DI interface matching

**Expected:**

- Stem "DataModelService" (from `IDataModelService`) is substring of `DataModelWebService`
- Stem must be ≥ 4 characters

**Unit tests:** `test_verify_call_site_target_fuzzy_interface_match`, `test_is_implementation_of_suffix_tolerant`

---

## Type Inference (C#)

### T-TYPE-INFER: Type inference improvements for xray_callers

**23 unit tests covering:**

- Cast expressions: `var x = (Type)expr`
- `as` expressions: `var x = expr as Type`
- Method return type: `var x = GetStream()` (same-class)
- `await` + Task<T> unwrap: `await GetStreamAsync()` → Stream
- Pattern matching: `obj is PackageReader reader`
- Switch case patterns: `case StreamReader reader:`
- Extension methods: `static class` + `this` param

**Status:** All covered by unit tests.

---

## Local Variable Type Extraction

### T65: Local Variable Type Extraction — TypeScript

**Covers:** Explicit type annotations, `new` expressions, generic `new`, unresolved local variables

**Unit tests:** `test_ts_local_var_explicit_type_annotation`, `test_ts_local_var_new_expression`

---

### T66: Local Variable Type Extraction — C#

**Covers:** Explicit type, `var = new`, `var` without `new`, generic types, `using var`

**Unit tests:** `test_csharp_local_var_explicit_type`, `test_csharp_local_var_new_expression`

---

### T-TYPED-LOCAL-DOWN: Explicit type annotations — direction=down resolves callees through typed locals

**Unit test:** `test_ts_direction_down_with_typed_local_variable`

---

## False Positive Filtering

### T67: Direction=up — Receiver type mismatch filtering

**Expected:**

- `path.resolve()` does NOT appear as caller of `TaskRunner.resolve()`
- Receiver type mismatch correctly filters false positives

---

### T68: Direction=up — Graceful fallback when no call-site data

**Expected:**

- Callers without call-site data are NOT filtered out (no false negatives)

---

### T69: Direction=up — Comment-line false positive filtered

**Expected:**

- Comment lines containing method name are NOT treated as call sites

**Status:** ✅ Covered by E2E test and unit tests

---

### T-BUILTIN-BLOCKLIST: Built-in type blocklist (direction=down)

**Expected:**

- `Promise.resolve()` does NOT match user-defined `Deferred.resolve()`
- Built-in types: Promise, Array, Map, Set, console, Math, JSON, Task, List, etc.

**Unit tests:** `test_builtin_promise_resolve_not_matched`, `test_non_builtin_type_still_matches`

---

## Fix 3 — Bypass Gate Closure

### T-FIX3-VERIFY: No false positives from missing call-site data

**Expected:**

- `verify_call_site_target()` rejects callers without call-site data

**Status:** ✅ Covered by unit tests

---

### T-FIX3-EXPR-BODY: Expression body property call sites (C#)

**Expected:**

- `callTree` includes expression body properties (e.g., `=> _provider.GetName()`)

**Status:** ✅ Covered by unit tests

---

### T-FIX3-LAMBDA: Lambda calls in arguments captured (C#)

**Expected:**

- Lambda body call sites attributed to the enclosing method

**Status:** ✅ Covered by unit tests

---

### T-FIX3-PREFILTER: Base types removed from caller pre-filter

**Expected:**

- Pre-filter only greps for target class + DI interface, not transitive base types

**Status:** ✅ Covered by unit tests

---

## Overload Deduplication

### T-OVERLOAD-DEDUP-UP: Overloaded callers not collapsed (direction=up)

**Expected:**

- TWO entries for overloaded `Process` (different `lines` values)

**Unit test:** `test_xray_callers_overloads_not_collapsed_up`

**Status:** ✅ Implemented

---

### T-OVERLOAD-DEDUP-DOWN: Overloaded callees not collapsed (direction=down)

**Expected:**

- TWO entries for overloaded `Execute` (different `lines` values)

**Unit test:** `test_xray_callers_overloads_not_collapsed_down`

**Status:** ✅ Implemented

---

## Same-Name Interface Resolution

### T-SAME-NAME-IFACE: No cross-contamination between unrelated interfaces

**Expected:**

- Searching for `ServiceA.Execute()` callers does NOT include `IServiceB.Execute()` callers

**Unit test:** `test_xray_callers_same_name_different_receiver_interface_resolution`

**Status:** ✅ Implemented

---

## Recursion Fix

### T-F10-CLASS-FILTER-RECURSION: Class filter preserved during recursion (depth > 0)

**Expected:**

- Callers of `Consumer.Run` at depth > 0 don't include false positives from `ServiceB.Process`

**Unit test:** `test_caller_tree_preserves_class_filter_during_recursion`

**Status:** ✅ Implemented

---

## Multi-Method Batch

### T-MULTI-METHOD: Multi-method batch returns results array

**Expected:**

- `method: "process,validate"` → `results` array with 2 entries
- Each entry has `method` name and `callTree`
- Single method (no comma) returns existing format with `callTree` at top level

**Unit tests:** `test_multi_method_returns_results_array`, `test_single_method_no_comma_returns_calltree_directly`

**Status:** ✅ Implemented

---

### T-BATCH-PARITY: Batch callers warning/hint/truncated parity (2026-03-16)

**Scenario:** Batch callers include per-method warning, hint, and truncated fields

1. Call `xray_callers` with `method="Foo,NonExistent"` where `Foo` exists in 2+ classes
2. Verify per-method results:
   - `Foo` result has `warning` field (ambiguity: found in N classes)
   - `NonExistent` result has `hint` field (nearest match suggestion)
   - Both results have `truncated` field (boolean)
   - `nodesVisited` present for `up` direction
3. Call with `maxTotalNodes=1` → verify `summary.truncated = true`

**Expected:** Batch callers return the same diagnostic fields as single-method callers.

**Regression:** Previously, batch path had no warning/hint/truncated/nodesVisited.

**Status:** ✅ Implemented

---

## Caller Sorting & Deprioritization

### T-CALLERS-DEPRIORITIZE: Test caller deprioritization (5 tests)

**Expected:**

- Production callers appear before test callers
- Within each group, sorted by popularity (posting count DESC)
- `impactAnalysis=true` preserves all test callers

**Status:** ✅ Covered by 16 unit tests

---

## Hints

### T-CALLERS-HINT: Hint when 0 results with class filter

**Expected:**

- Empty tree + class filter → `hint` field suggesting possible reasons
- No hint without class filter or when results found

**Unit tests:** `test_xray_callers_hint_when_empty_with_class_filter`

**Status:** ✅ Implemented

---

## Handler-Level Unit Tests

### T83: `xray_callers` — `excludeDir` and `excludeFile` filters

**Unit test:** `test_xray_callers_exclude_dir_and_file`

---

### T84: `xray_callers` — Cycle detection (direction=down)

**Unit test:** `test_xray_callers_cycle_detection_down`

---

### T-CALLERS-CYCLE-UP: Cycle detection (direction=up)

**Unit test:** `test_xray_callers_cycle_detection`

---

### T-CALLERS-EXT-COMMA: Comma-split `ext` filter

**Unit test:** `test_xray_callers_ext_filter_comma_split`

---

## Input Validation

### T-VAL-03: `depth: 0` returns error

**Expected:** `isError: true`, message: `"depth must be >= 1"`

**Unit test:** `test_xray_callers_depth_zero_returns_error`

---

### T-CR-03: `maxTotalNodes: 0` means unlimited

**Expected:** Returns results (treated as `usize::MAX`)

---

### T-CR-04: Invalid direction returns error

**Expected:** `direction: "sideways"` → error; `direction: "UP"` → accepted (case-insensitive)

---

### T-CR-01: Fuzzy DI matching works via `is_implementation_of`

**Unit tests:** `test_verify_fuzzy_di_without_base_types`, `test_verify_reverse_fuzzy_di_without_base_types`
