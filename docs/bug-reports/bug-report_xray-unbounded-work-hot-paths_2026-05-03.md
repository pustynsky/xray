# Bug Report: Unbounded-work hot paths in MCP handlers can stall the single-threaded stdio loop

**Date:** 2026-05-03
**Author:** GitHub Copilot
**Area:** xray MCP / `xray_edit` failure-hint path / `xray_definitions` not-found-hint path / unified-diff renderer / single-threaded `run_server` loop
**Type:** Performance / availability bug class
**Severity:** **IMPORTANT — High** (one critical case can stall the entire MCP session for minutes; three secondary cases stall it for ~1–3 s)
**Status:** Pending fix
**Supersedes:** [bug-report_xray-edit-find-nearest-match-quadratic-hang_2026-05-02.md](docs/bug-reports/bug-report_xray-edit-find-nearest-match-quadratic-hang_2026-05-02.md) — same root, broader scope after audit.
**Related code:**
[src/mcp/handlers/edit.rs](src/mcp/handlers/edit.rs#L2482) `find_nearest_match`,
[src/mcp/handlers/edit.rs](src/mcp/handlers/edit.rs#L2872) `generate_unified_diff`,
[src/mcp/handlers/definitions.rs](src/mcp/handlers/definitions.rs#L2248) `hint_file_fuzzy_match`,
[src/mcp/handlers/definitions.rs](src/mcp/handlers/definitions.rs#L1952) `try_name_correction`,
[src/mcp/handlers/definitions.rs](src/mcp/handlers/definitions.rs#L2300) `hint_nearest_name`,
[src/mcp/server.rs](src/mcp/server.rs#L229) `run_server_with_io`.

---

## TL;DR

The MCP `run_server` event loop is single-threaded: one request is dispatched inline, the next request waits for the response to be flushed. Several hint / diff helpers invoked on this loop have no work cap relative to input size:

| # | Helper | Worst-case input shape | Wall-clock observed / estimated | Severity |
|---|---|---|---|---|
| 1 | `find_nearest_match` (multi-line) | 100 KB+ file × multi-line stale `search` | ~5 min (reproduced 2026-05-02 × 2) | **Critical** |
| 2 | `find_nearest_match` (single-line) | minified 100 KB single-line file × 1 KB `search` | ~1 s (estimated, same DP shape) | **High** |
| 3 | `generate_unified_diff` | full-file rewrite of multi-thousand-line file | ~1–3 s (estimated; runs on every successful large edit) | **High** |
| 4 | `hint_file_fuzzy_match` | 100K-file repo × deep paths × zero-result file filter | ~50–500 ms (estimated) | Medium |
| 5 | `try_name_correction` / `hint_nearest_name` | 100K unique definition names | ~50–200 ms (estimated) | Medium |

Because the dispatcher is FIFO over stdin (`run_server_with_io` reads, dispatches, writes, loops — no spawn-per-request), **any one** of these hangs blocks unrelated tool calls from any other client on the same Shared MCP server. Bug #1 is the trigger that has been observed twice now; bugs #2–#5 are the same shape and were found by audit.

---

## Severity rationale

- The single-threaded loop ([src/mcp/server.rs](src/mcp/server.rs#L229)) gives any one stall request the full server: a 5-minute hint computation freezes a parallel `xray_info` from a different VS Code session.
- The trigger inputs are **routine LLM edit shapes**, not adversarial. Bug #1 has fired twice in 24 h on real edits to `CHANGELOG.md` and `handlers_tests_grep.rs`.
- There is **no per-request cancel today** — the only recovery is killing and restarting the MCP server, which discards the in-memory hot indexes (~3 GB warm cache on the Shared workspace).
- A previous partial fix on 2026-04-27 ([src/mcp/handlers/edit.rs](src/mcp/handlers/edit.rs#L2618)) replaced the swap-detection scan with `str::contains`. The note in that commit explicitly flagged the surviving `nearest_match_hint` scan as a known follow-up. This bug report cashes that follow-up plus the broader audit findings.

---

## The shared root cause

Four of the five helpers share the same shape:

```rust
for each candidate in unbounded_collection(input) {
    expensive_per_candidate_work(input, candidate);
}
```

- **Unbounded collection**: number of sliding windows in a file; number of indexed file paths; number of indexed definition names.
- **Expensive work**: `similar::TextDiff::from_chars` Myers DP (O(M·N)); nested `String::replace` + `contains` over every path segment; `strsim::jaro_winkler` over every name.
- **No work-cell budget**: the only guards in place are file-size guards (#1: 500 KB) or none (#2–#5). File size is the wrong axis — cost is `len(input) * len(candidate) * #candidates`.

The MCP loop has no per-request timeout, no spawn-per-request, no cancellation. So unbounded helper work → unbounded server stall.

---

## Bug #1 — `find_nearest_match` multi-line: O(W·N·M) hint scan

### Code

[src/mcp/handlers/edit.rs](src/mcp/handlers/edit.rs#L2482):

```rust
fn find_nearest_match(content: &str, search_text: &str) -> Option<NearestMatch> {
    if content.len() > NEAREST_MATCH_MAX_FILE_SIZE {  // 500 KB — the only cap
        return None;
    }
    let search_lines: Vec<&str> = search_text.split('\n').collect();
    let content_lines: Vec<&str> = content.split('\n').collect();
    let search_line_count = search_lines.len();
    // ...
    } else if content_lines.len() >= search_line_count {
        for i in 0..=(content_lines.len() - search_line_count) {
            let window = content_lines[i..i + search_line_count].join("\n");
            let ratio = similar::TextDiff::from_chars(search_text, &window).ratio();
            // best update ...
        }
    }
    // ...
}
```

### Reproduction (2026-05-02, second occurrence on `CHANGELOG.md`)

```text
file size:           ~140 000 bytes (~137 KB)         ← under 500 KB guard
line count:          ~3 000
search line count:   ~14 (≈1.6 KB)
```

A single failed `xray_edit` did not return for ~5 minutes. A parallel `xray_info` from a second VS Code session also did not return, confirming the stdio loop was busy serving the original request. Recovery required `taskkill /F /IM xray.exe`.

### Cost model

```
windows                ≈ line_count − search_line_count          ≈ ~3 000
per-window Myers cells ≈ search_chars × window_chars            ≈ 1 600 × 1 600 ≈ 2.5 × 10⁶
total Myers cells      ≈ 7.5 × 10⁹
Windows debug-allocator throughput on this machine: ~25 × 10⁶ cells/s
estimated wall-clock   ≈ 300 s   →   matches observed ~5 min
```

### Why the existing 500 KB guard does not help

It caps `content.len()`, not `windows × search_chars × window_chars`. A file at 100 KB with a 1 KB multi-line search is already pessimal.

---

## Bug #2 — `find_nearest_match` single-line: same DP, single window

### Code

[src/mcp/handlers/edit.rs](src/mcp/handlers/edit.rs#L2497):

```rust
if search_line_count <= 1 {
    for (i, line) in content_lines.iter().enumerate() {
        let ratio = similar::TextDiff::from_chars(search_text, line).ratio();
        // ...
    }
}
```

### Worst-case shape

A minified asset (e.g. one-line JS bundle), 100 KB on disk → `content_lines.len() == 1` → one giant window: `100 000 × 1 000 = 10⁸` Myers cells ≈ ~1–2 s on the same allocator profile.

### Status

Not yet observed in the wild but trivially reachable. Any LLM that retries an edit against a minified file with a stale snippet will hit it. Same fix vector as bug #1.

---

## Bug #3 — `generate_unified_diff` on full-file rewrites

### Code

[src/mcp/handlers/edit.rs](src/mcp/handlers/edit.rs#L2872):

```rust
fn generate_unified_diff(path: &str, original: &str, modified: &str) -> String {
    if original == modified {
        return String::new();
    }
    similar::TextDiff::from_lines(original, modified)
        .unified_diff()
        .header(&format!("a/{}", path), &format!("b/{}", path))
        .to_string()
}
```

### Worst-case shape

`from_lines` runs the same Myers DP on lines instead of chars. For a successful rewrite of a 5 000-line file with most lines changed:

```
DP cells       ≈ 5 000 × 5 000 = 2.5 × 10⁷
per-cell cost  ≈ line equality (~80 byte memcmp) → ~few hundred ns
estimated      ≈ 1–3 s wall-clock
```

### Why it matters

- Runs on **every successful** edit, not just the failure path.
- The diff is then **embedded in the response** — a 5 000-line diff easily exceeds 1 MB of JSON, which itself bloats stdio write time and LLM context usage.
- Mode B `paths` (multi-file edits) amplifies: N files × per-file diff cost. There is no `--no-diff` opt-out.

### Status

Not yet observed as a hang, but at the boundary. The CHANGELOG.md edit that triggered bug #1 today (~3 000 lines, fully overwritten) would have produced a multi-second diff response on success too.

---

## Bug #4 — `hint_file_fuzzy_match`: triple-nested path-segment scan

### Code

[src/mcp/handlers/definitions.rs](src/mcp/handlers/definitions.rs#L2248):

```rust
for (file_id, def_indices) in &index.file_index {
    // ... normalize path ...
    if path_normalized.contains(&ff_normalized) ... {
        let segments: Vec<&str> = path_lower.split('/').collect();
        for window_size in 1..=segments.len() {
            for start in 0..=(segments.len() - window_size) {
                let segment = segments[start..start + window_size].join("/");
                // normalize + contains both ways
            }
        }
    }
}
```

### Worst-case shape

For a path with `D` segments, the inner loop runs `D × (D+1) / 2` times. On the Shared workspace (~100K indexed files, average ~8 path segments) the inner loop is ~36 iterations × ~10⁴ path-matching files ≈ ~3.6 × 10⁵ normalize+contains pairs. Each `String::replace` allocates. Estimated ~50–500 ms on a hot `xray_definitions file='nope'` call.

### Status

Does not freeze the server, but inflates p99 of the not-found hint path by orders of magnitude over the ~1 ms baseline of an `xray_definitions` index lookup.

---

## Bug #5 — `try_name_correction` / `hint_nearest_name`: Jaro-Winkler over all names

### Code

[src/mcp/handlers/definitions.rs](src/mcp/handlers/definitions.rs#L1952) and [src/mcp/handlers/definitions.rs](src/mcp/handlers/definitions.rs#L2310):

```rust
for index_name in index.name_index.keys() {
    let score = name_similarity(&search_lower, index_name);  // strsim::jaro_winkler
    // ...
}
```

[src/mcp/handlers/utils.rs](src/mcp/handlers/utils.rs#L1839):

```rust
pub(crate) fn name_similarity(a: &str, b: &str) -> f64 {
    strsim::jaro_winkler(a, b)
}
```

### Worst-case shape

On the Shared workspace `index.name_index` has roughly 100K–500K unique definition names. Jaro-Winkler is O(`|a| + |b|`) but with non-trivial constants and char-iteration. Estimated ~50–200 ms per `xray_definitions name='typo'` call when no exact match is found. `try_name_correction` has a partial mitigation (`AUTO_CORRECT_MIN_LENGTH_RATIO`) but it is applied **after** the per-name Jaro-Winkler, so it does not help the cost.

### Status

Observability-only impact today. Same shape, smaller magnitude than #4. Worth fixing in the same pass with a single length-prefilter.

### Implementation scope (revised after code review 2026-05-03)

Shipping a length-ratio prefilter on Jaro-Winkler is **not** semantics-preserving — Jaro-Winkler can score `cheeseburger` vs `cheese` at ~0.90 even with `length_ratio = 0.5` (Winkler boost on shared 4-char prefix). Three sites were therefore handled differently:

| Site | Action | Why |
|---|---|---|
| `try_name_correction` (definitions.rs ~L1953) | **Move** existing `length_ratio < AUTO_CORRECT_MIN_LENGTH_RATIO` guard from AFTER `name_similarity()` to BEFORE | Pure perf refactor — guard already existed, behavior unchanged. ~80–90 % of names skipped without Jaro-Winkler |
| `hint_nearest_name` (definitions.rs ~L2315) | **Do not** add prefilter | Path is observability-only; adding the guard here would be a **fresh** behavior change (pre-fix code had no length guard). Loss of valid hints for short shared-prefix matches outweighs the ~50–200 ms saving |
| `suggested_name_matches` (definitions.rs ~L1123) | **Defer** | Needs a Jaro-aware prefilter (not pure length ratio) to be safe. Same risk as `hint_nearest_name`. Tracked as follow-up |

A safer prefilter would require (a) deriving an upper bound on Jaro-Winkler from `(|a|, |b|, common_prefix_len)`, or (b) a cheap fingerprint (e.g. trigram intersection) instead of length. Out of scope for this fix.

---

## Cross-cutting: no per-request budget on `run_server`

[src/mcp/server.rs](src/mcp/server.rs#L229) `run_server_with_io`:

```rust
let response = handle_request(&ctx, &request.method, &request.params, id.clone());

let resp_str = match serde_json::to_string(&response) { ... };
if let Err(e) = writeln!(writer, "{}", resp_str) { ... }
```

No timeout, no spawn, no cancellation, no `serverBusy` advisory. Whatever `handle_request` does, the next stdin line waits. This is the multiplier that turns each of bugs #1–#5 from "slow tool" into "server stall".

The MCP-protocol-correct fix is harder (request IDs would need to be tracked, cancellable tasks introduced, ordering re-thought). The pragmatic fix is to make the helpers themselves bounded so the loop never sits on one for more than a few hundred milliseconds. That is what this report proposes.

---

## Proposed fix

One shared technique with two layers, applied per helper. The technique is the same as the 2026-04-27 swap-path fix: cheap O(N) prefilter + O(1) work-cell budget.

### Shared helper module (new)

```rust
// src/mcp/handlers/work_budget.rs (new)

/// Hard work-cell budget per hint computation. Sized so even a pessimal
/// input cannot exceed ~500 ms wall-clock on the slowest measured profile
/// (Windows debug-allocator, ~25M Myers cells/s).
pub(crate) const HINT_WORK_BUDGET: usize = 50_000_000;

/// Byte-bag profile of `s` — 256-bin histogram. O(s.len()).
pub(crate) fn byte_histogram(s: &str) -> [u32; 256] {
    let mut h = [0u32; 256];
    for &b in s.as_bytes() { h[b as usize] = h[b as usize].saturating_add(1); }
    h
}

/// Histogram intersection — upper bound on Myers char-similarity.
pub(crate) fn histogram_intersection(profile: &[u32; 256], window: &str) -> u32 {
    let win = byte_histogram(window);
    (0..256).map(|i| profile[i].min(win[i])).sum()
}
```

### Apply to bugs #1 + #2 (`find_nearest_match`, both branches)

```rust
fn find_nearest_match(content: &str, search_text: &str) -> Option<NearestMatch> {
    if content.len() > NEAREST_MATCH_MAX_FILE_SIZE { return None; }
    let search_lines: Vec<&str> = search_text.split('\n').collect();
    let content_lines: Vec<&str> = content.split('\n').collect();
    if content_lines.is_empty() || search_text.is_empty() { return None; }

    let search_profile = byte_histogram(search_text);
    let prefilter_min = (search_text.len() as u32 * 60) / 100;
    let mut work_used: usize = 0;
    let mut best = NearestMatch { line: 0, similarity: 0.0, text: String::new() };

    let mut try_window = |i: usize, window_text: &str, best: &mut NearestMatch| -> bool {
        // (1) Cheap prefilter — byte-bag intersection is an upper bound on Myers.
        if histogram_intersection(&search_profile, window_text) < prefilter_min {
            return true; // continue
        }
        // (2) Work-cell budget — hard wall on cumulative Myers cells.
        let cells = search_text.len().saturating_mul(window_text.len());
        if work_used.saturating_add(cells) > HINT_WORK_BUDGET {
            return false; // break
        }
        work_used += cells;
        let ratio = similar::TextDiff::from_chars(search_text, window_text).ratio();
        if ratio > best.similarity {
            *best = NearestMatch { line: i + 1, similarity: ratio, text: window_text.to_string() };
        }
        true
    };

    let search_line_count = search_lines.len();
    if search_line_count <= 1 {
        for (i, line) in content_lines.iter().enumerate() {
            if !try_window(i, line, &mut best) { break; }
        }
    } else if content_lines.len() >= search_line_count {
        for i in 0..=(content_lines.len() - search_line_count) {
            let window = content_lines[i..i + search_line_count].join("\n");
            if !try_window(i, &window, &mut best) { break; }
        }
    }

    if best.similarity < NEAREST_MATCH_MIN_SIMILARITY as f32 { return None; }
    Some(best)
}
```

**Why it works**
- `byte_intersection >= byte_LCS >= char_LCS` because every char in the LCS is encoded by its UTF-8 bytes in BOTH strings (1–4 bytes per char), so the byte intersection is a true upper bound on the char-level LCS that `similar::TextDiff::from_chars(...).ratio()` measures.
- The shipped formula uses **char counts** in both numerator and denominator: drop when `byte_intersection < (RATIO * (search.chars().count() + window.chars().count())) / 200`. This is a correct upper bound on `ratio = 2·char_LCS / (a_chars + b_chars)`.
- The first draft used `search.len() * RATIO / 100` (unsafe for short windows fully contained in long search → caught in review finding #2). The interim per-pair fix used **byte** lengths in the formula but compared against a byte-histogram intersection, so for non-ASCII input the two units diverged and valid hints were dropped (caught in re-review). Two regression tests cover both classes: `test_find_nearest_match_prefilter_keeps_short_window_in_long_search` (ASCII length-skewed) and `test_find_nearest_match_prefilter_handles_non_ascii_search` (Unicode boundary case from reviewer's exact counterexample).
- The budget caps cumulative DP cells, not wall-clock, which keeps the test deterministic. 10M cells ≈ 200–400 ms on the slowest measured profile (production); under parallel test load on Win debug ~1.5–2 s.
- `best` is **kept** when budget is exhausted, so a useful hint that was found in the first ~50 windows still gets returned.

### Apply to bug #3 (`generate_unified_diff`)

Two changes:

```rust
const UNIFIED_DIFF_MAX_LINES: usize = 2_000;
const UNIFIED_DIFF_MAX_BYTES: usize = 256 * 1024; // 256 KB cap

fn generate_unified_diff(path: &str, original: &str, modified: &str) -> String {
    if original == modified { return String::new(); }
    let max_lines = original.lines().count().max(modified.lines().count());
    if max_lines > UNIFIED_DIFF_MAX_LINES {
        return format!(
            "--- a/{}\n+++ b/{}\n# diff omitted: {} lines (cap {}). \
            new_line_count and lines_added/lines_removed in the response \
            are still authoritative.\n",
            path, path, max_lines, UNIFIED_DIFF_MAX_LINES,
        );
    }
    let diff = similar::TextDiff::from_lines(original, modified)
        .unified_diff()
        .header(&format!("a/{}", path), &format!("b/{}", path))
        .to_string();
    if diff.len() > UNIFIED_DIFF_MAX_BYTES {
        let truncated = &diff[..diff.floor_char_boundary(UNIFIED_DIFF_MAX_BYTES)];
        return format!("{}\n# diff truncated at {} bytes", truncated, UNIFIED_DIFF_MAX_BYTES);
    }
    diff
}
```

The response keeps `lines_added` / `lines_removed` / `new_line_count` (already authoritative for caller decisions), so dropping the inline diff text on giant rewrites loses zero information the caller cannot recompute.

### Apply to bug #4 (`hint_file_fuzzy_match`)

Replace the `for window_size in 1..=segments.len()` cubic loop with a flat segment scan:

```rust
for segment in path_lower.split('/') {
    let seg_normalized = normalize(segment);
    if seg_normalized.contains(&ff_normalized) || ff_normalized.contains(&seg_normalized) {
        // best update ...
    }
}
```

Drops O(D²) to O(D) per file. The original code's intent ("match path subsequences") was already covered by the outer `path_normalized.contains(&ff_normalized)` gate.

### Apply to bug #5 (`try_name_correction` / `hint_nearest_name`)

Add a length-ratio prefilter **before** Jaro-Winkler:

```rust
for index_name in index.name_index.keys() {
    let len_ratio = search_lower.len().min(index_name.len()) as f64
                  / search_lower.len().max(index_name.len()) as f64;
    if len_ratio < AUTO_CORRECT_MIN_LENGTH_RATIO { continue; }  // O(1) skip
    let score = name_similarity(&search_lower, index_name);
    // ...
}
```

The `length_ratio` check already exists in `try_name_correction` but **after** the expensive call. Moving it before drops ~80–90 % of names without changing the result (Jaro-Winkler with `length_ratio < 0.6` cannot reach the 0.8 acceptance threshold).

---

## Acceptance criteria

| # | Scenario | Today | After fix |
|---|---|---|---|
| 1 | failing `xray_edit` against 200 KB / 5 000-line file with 50-line `search` | ~5 min | **≤ 1 s wall-clock** |
| 2 | failing `xray_edit` against 100 KB single-line minified file with 1 KB `search` | ~1–2 s | **≤ 1 s** |
| 3 | successful `xray_edit` rewriting a 5 000-line file end-to-end | ~1–3 s + ~1 MB diff in response | **≤ 200 ms**, response carries `diffOmitted` marker |
| 4 | `xray_definitions file='nonexistent'` on 100K-file workspace | ~50–500 ms | **≤ 50 ms** |
| 5 | `xray_definitions name='typo'` on 100K-name workspace | ~50–200 ms | **≤ 30 ms** |
| All | Existing `test_nearest_match_hint_*` (6 tests in `edit_tests.rs`) and definition-hint tests | green | **still green, byte-identical hint text** |

Non-functional:
- A second MCP request issued during a worst-case `xray_edit` call must be served within `≤ 2 × worst_case_helper_time` (sequential lower bound), proving the loop is unblocked. (Cannot be tested deterministically inside `cargo test`; deferred to manual e2e — see below.)

---

## Test plan

### Unit tests (must be added, all in [src/mcp/handlers/edit_tests.rs](src/mcp/handlers/edit_tests.rs) or `definitions_tests.rs`)

Each test asserts both **correctness** (the hint that the existing code would have produced is still produced when budget is not exhausted) and **bound** (`Instant::now()` envelope on the failing path).

```text
#[test] test_find_nearest_match_multiline_budget_caps_runtime
  Setup:  200 KB file ("fn foo() { /* line */ }\n" * 8 000), 50-line absent search
  Assert: result.is_error
  Assert: elapsed < Duration::from_secs(2)
  Assert: !text.contains("Nearest match")  // budget exhausted, no spurious hint

#[test] test_find_nearest_match_singleline_budget_caps_runtime
  Setup:  one-line 100 KB file ("a".repeat(100_000)), 1 KB absent search
  Assert: result.is_error
  Assert: elapsed < Duration::from_secs(2)

#[test] test_find_nearest_match_prefilter_keeps_useful_hint
  Setup:  small file with one line that is a near-typo of search
  Assert: text.contains("Nearest match at line N")  // prefilter doesn't drop useful match

#[test] test_generate_unified_diff_caps_at_2000_lines
  Setup:  apply edit that rewrites 5 000 lines
  Assert: response.diff.contains("diff omitted: 5000 lines")
  Assert: response.lines_added + response.lines_removed > 0  // counts still accurate
  Assert: elapsed < Duration::from_millis(500)

#[test] test_generate_unified_diff_truncates_oversize_byte_payload
  Setup:  rewrite producing > 256 KB of diff text but < 2 000 lines (very wide lines)
  Assert: response.diff.contains("# diff truncated at")

#[test] test_hint_file_fuzzy_match_linear_in_segments
  Setup:  index with 1 000 deeply-nested paths (12 segments each), file='nope'
  Assert: elapsed < Duration::from_millis(50)
  Assert: result.hint.is_none() OR returns same nearest as old code on small fixture

#[test] test_try_name_correction_skips_length_mismatch_before_jaro
  Setup:  index with 100 names, query with extreme length mismatch (e.g. 'x' vs 30-char names)
  Assert: no auto-correction applied (length ratio gate)
  Assert: regression-equivalent to existing nearest_match_suggests_close_typos
```

All existing tests below must remain green **without modification**:

- `test_nearest_match_hint_different_quotes` ([src/mcp/handlers/edit_tests.rs](src/mcp/handlers/edit_tests.rs#L1765))
- `test_nearest_match_hint_partial_overlap` ([src/mcp/handlers/edit_tests.rs](src/mcp/handlers/edit_tests.rs#L1785))
- `test_nearest_match_hint_no_good_match` ([src/mcp/handlers/edit_tests.rs](src/mcp/handlers/edit_tests.rs#L1803))
- `test_nearest_match_hint_multiline_search` ([src/mcp/handlers/edit_tests.rs](src/mcp/handlers/edit_tests.rs#L1821))
- `test_nearest_match_hint_anchor_not_found` ([src/mcp/handlers/edit_tests.rs](src/mcp/handlers/edit_tests.rs#L1840))
- `test_nearest_match_hint_regex_not_found` ([src/mcp/handlers/edit_tests.rs](src/mcp/handlers/edit_tests.rs#L1858))
- `test_swap_hint_silent_for_near_miss_replace_text` ([src/mcp/handlers/edit_tests.rs](src/mcp/handlers/edit_tests.rs#L6104))
- `test_swap_hint_substring_based_fast_on_large_file` ([src/mcp/handlers/edit_tests.rs](src/mcp/handlers/edit_tests.rs#L6043))
- `test_search_boundary_whitespace_byte0_mismatch_hints_trim` ([src/mcp/handlers/edit_tests.rs](src/mcp/handlers/edit_tests.rs#L5453))
- `nearest_match_suggests_close_typos` ([src/mcp/handlers/arg_validation.rs](src/mcp/handlers/arg_validation.rs#L424))
- `test_hint_nearest_name_match` (definitions_tests.rs)
- All `test_name_similarity_*` (utils_tests.rs)

### Local repro test (manual, before merging)

The inputs that triggered bug #1 in production should fail-fast after the fix:

```powershell
# 1. Reset to a clean copy of the known-bad inputs
cp .\CHANGELOG.md $env:TEMP\xray-bug1-changelog.md

# 2. Issue the exact failing edit (pasted from 2026-05-02 transcript) via
#    a one-shot harness binary or a `cargo test --bin xray ...` driver:
cargo test --release --bin xray test_repro_bug1_changelog_md_5min_hang -- --nocapture

# Pass criterion: completes in < 2 s with `is_error: true` and no `Nearest match` hint.
```

A `tests/repro_unbounded_helpers.rs` file (or `#[cfg(test)]` harness inside `edit.rs`) is the right place — small, isolated, no MCP roundtrip.

### End-to-end test (manual, must be done once before declaring fixed)

The single-threaded-loop unblock claim is e2e by definition. Two terminals:

```powershell
# Terminal A — trigger the worst-case helper.
# Use the existing PowerShell harness, the same one used in scripts/measure-ac4-shared.ps1.
.\scripts\repro-bug1-edit-hang.ps1   # to be added; sends the failing CHANGELOG.md edit

# Terminal B — within ~200 ms of Terminal A, send a trivial unrelated request.
.\scripts\time-xray-info.ps1         # already exists; calls xray_info via stdio
```

**Pass criterion**: Terminal B's `xray_info` returns within `≤ 2 s` of Terminal A finishing (not `≥ 5 min` as today). Write the transcript to `docs/measurements/unbounded-helpers-fix_<date>.md` for the record.

### Regression test on real workspace (Shared / 76K-file)

After cargo install, run the standard "failed-edit storm" regression:

```powershell
cargo install --path . --force --bin xray

# Restart MCP server (VS Code: "MCP: Restart all servers").
# Issue 10 deliberately stale edits against CHANGELOG.md and a 200 KB source file:
.\scripts\stress-edit-failures.ps1 -Iterations 10 -File CHANGELOG.md

# Pass criterion: 95th-percentile failed-edit latency < 1 s,
#                 99th < 2 s,
#                 max  < 3 s.
#                 No xray.exe instance ever consumes > 50 % CPU for > 1 s.
```

### Existing test sweep (must pass before commit)

```powershell
# Full suite, no pre-filter on first run (per cross-project rule).
$ErrorActionPreference = 'Continue'
cargo test --bin xray --no-fail-fast 2>&1 | Tee-Object test-out.log | Out-Null
Get-Content test-out.log -Tail 60

# Targeted re-runs after green on full suite:
cargo test --bin xray nearest_match
cargo test --bin xray unified_diff
cargo test --bin xray hint_file_fuzzy
cargo test --bin xray name_correction
```

---

## Out of scope (deferred to follow-up reports)

These were considered during the audit and rejected for this fix because they widen the diff without changing the user-visible failure mode:

- **Per-tool wall-clock telemetry on `xray_edit` failure responses.** Useful for future incident investigation, but not blocking the immediate hang fix.
- **`serverBusy` advisory on `xray_info` when dispatcher hasn't read a new line in > 5 s.** Requires loop refactor; the helper-budget fix removes the trigger condition.
- **Spawn-per-request / cancellable tasks in `run_server`.** Larger architectural change; the helper-budget fix makes it non-urgent.
- **Replacing `similar::TextDiff::from_chars` with token / n-gram similarity.** Touches all calibrated thresholds (`>= 0.40`, `>= 0.80`, `>= 0.90`, `>= 0.97`) and breaks every test oracle. The prefilter+budget approach gives ~10×–10⁴× speedup on the patho cases without touching any threshold.

---

## Workaround for callers (until fixed)

Unchanged from the 2026-05-02 report:

- Keep `search` to **3–5 unique anchor lines** when targeting files ≥ ~100 KB.
- For full-file rewrites, prefer Mode A `operations` with `[{startLine: 1, endLine: <total>, content: ...}]` over Mode B's giant `search` block — Mode A skips the `find_nearest_match` path entirely.
- If a hang is observed: `taskkill /F /IM xray.exe`, restart MCP. There is no per-request cancel today.

