# User Story: Performance hints and docs for `xray_grep` `lineRegex` mode

**Date:** 2026-04-26
**Reported by:** Sergey Pustynsky (via Copilot session, Shared repo PR review)
**Severity:** Medium — correctness OK, but UX trap that wastes 30–80 seconds per misuse
**Component:** xray MCP server, `xray_grep` tool

---

## Problem

`xray_grep` in `lineRegex=true` mode bypasses the trigram (ngram) index and runs a
full file-content scan when the regex doesn't reduce to ngram candidates. On
Shared (66 694 indexed files, ~3.5 M tokens) two real calls measured:

| Call | Mode | Time | Result |
|---|---|---|---|
| `terms=["OrgApp.*TypeId\|...\|App.*TypeId\\s*=\\s*\\d"]` | lineRegex | **76 362 ms** | 0 files |
| same call, second time (warm cache) | lineRegex | **46 937 ms** | 0 files |
| `terms=["App\\s*=\\s*[0-9]+"]` | lineRegex | **48 136 ms** | 38 files / 59 occurrences |
| `terms=["OrgAppTypeId","AppTypeId","OrganizationalAppTypeId"]` | substring | **1 ms** | 0 files |
| `terms=["LakewarehouseTypeId"]` | substring | **3.97 ms** | 5 files |

Substring → lineRegex slowdown for the same intent: **~10 000–80 000×**.

The agent (me) reflexively picked `lineRegex` because the search target (a constant
declaration like `OrgAppTypeId = 12345`) "looks like" a regex problem. Nothing in
the tool response surfaced the cost until the call had already completed.

## Root cause

For substring `terms`, the trigram index reduces candidate set to ~10–100 files in
sub-millisecond, then matches the literals against those files only. For
`lineRegex`, if the pattern lacks a fixed literal of length ≥3 (or has only
literals that match millions of lines like `App`), the index can't prefilter and
every indexed file's lines are scanned. With `.*` and alternations, even
shorter literals lose discriminative power.

This is fundamentally how trigram-indexed regex engines work — same trade-off
exists in ripgrep, codesearch, livegrep. **What's different**: ripgrep does
*literal extraction* — it parses the regex AST, extracts mandatory fixed
literals (e.g. `App` from `App\s*=\s*\d+`), prefilters via ngram, then runs
the regex only on candidate files. xray currently does not.

## Acceptance criteria

### AC-1 (must) — adaptive performance hint in response

When `searchMode == "lineRegex"` and `searchTimeMs > 2000`, include a `hint`
field in the response:

```jsonc
"hint": "lineRegex took 48s on 66694 files (full-scan: regex has no fixed-literal ≥3 chars to prefilter via ngram index). Try (in order): (a) extract a fixed substring into terms=['App'] for ngram prefilter, then re-validate with regex client-side; (b) narrow with dir=[...] / file=[...]; (c) anchor the regex (^App\\s*= instead of App\\s*=). For pure substring search, drop lineRegex (~1000x faster)."
```

Triggers: lineRegex mode AND `searchTimeMs > 2000` AND `totalFiles > 1000`.
Threshold values negotiable; the point is the hint should fire on real-world
slow scans, not on small repos where lineRegex is fine.

### AC-2 (must) — docs section in `xray_help topic=xray_grep`

Add a "When NOT to use lineRegex" subsection to the `bestPractices` array in
the help response:

> **lineRegex bypasses the trigram index when the pattern has no fixed
> literal of length ≥3 chars.** Patterns like `\w+\s*=\s*\d+` or
> `App.*=.*\d` cause full-scan and take seconds on large repos. Rule of
> thumb: if you can express the search as substring-or, use `terms=[...]`
> instead. lineRegex is the right tool **only** when:
>
> 1. **Anchoring is essential** — `^foo` (line start), `bar$` (line end).
> 2. **Char classes are essential** — `[A-Z]{3}` to find 3-letter uppercase tokens.
> 3. **Lookarounds are essential** — rare; usually a sign you should restructure.
>
> If the pattern is "find function calls / declarations / property
> assignments matching a shape" — substring is almost always faster:
> `terms=["OrgApp", "TypeId"]` then visually scan the (few) results.

### AC-3 (should) — preflight warning when lineRegex pattern looks unbounded

Before executing, if the regex pattern contains `.*` AND no anchor (`^`, `$`,
`\b`) AND no fixed-literal substring of length ≥4, return a `warning` (not
error) and run anyway:

```jsonc
"warning": "Regex pattern 'App.*=.*\\d' has no ngram-prefilter prefix. Estimated full-scan over 66694 files. Continuing — but consider terms=['App'] substring + client-side regex for ~1000x speedup."
```

This is cheap to compute (regex AST inspection, no scan) and gives the caller
an immediate decision point.

### AC-4 (nice-to-have) -- literal extraction optimization

Implement ripgrep-style literal extraction: parse the regex AST, find required
literals, prefilter via the existing trigram index, run regex only on the
resulting candidate file set. Fall back to the current full-scan path whenever
extraction is not provably safe -- correctness must never regress (no false
negatives).

#### Design (agreed via 3-actor team review, 2026-04-26)

**Extraction wrapper** -- new module `src/mcp/handlers/grep_literal_extract.rs`:

```rust
pub(super) struct ExtractedLiterals {
    /// Lowercased required-substring literals. Every match of the regex MUST
    /// contain at least one of these as substring (Seq semantics from
    /// `regex_syntax::hir::literal::Extractor`).
    pub literals: Vec<String>,
    /// True only when the Seq is finite, non-empty, AND every literal has
    /// length >= 3 (trigram-indexable). False = caller must fall back to
    /// full scan for this pattern.
    pub usable: bool,
}

pub(super) fn extract_required_literals(pattern: &str) -> Option<ExtractedLiterals>;
```

- Uses `regex_syntax::ParserBuilder::new().build().parse(pattern)` then
  `hir::literal::Extractor::new()` (default `Kind::Prefix`) -> `Seq`.
- Returns `None` only on `regex_syntax` parse error (caller falls back).
- Returns `Some({usable: false, literals: []})` when `Seq::is_inexact()`,
  `Seq` is empty/infinite, or any literal length < 3 (`MIN_LITERAL_LEN`).
- Otherwise `Some({usable: true, literals})` with each literal lowercased to
  match the trigram tokens (which are lowercased at index time).

**Per-pattern candidate set computation** -- for each `usable` pattern:

1. For each literal: `generate_trigrams(literal)` -> intersect `trigram_map`
   posting lists -> verify candidate tokens (same idea as
   `find_matching_tokens_for_term`) -> walk `index.index[token]` postings ->
   collect `file_id`s.
2. Pattern's candidate set = **UNION** of per-literal file sets (Seq
   guarantees each match contains at least one literal -- never
   intersection).
3. **Common-literal short-circuit (50% threshold).** If a single literal's
   candidate count exceeds `LITERAL_PREFILTER_MAX_RATIO * total_files`
   (constant 0.5) -> mark the pattern as un-prefilterable and fall back.
   Rationale: above ~50% candidates the HashSet build + intersection cost
   exceeds the saved scan, and concurrent queries amplify memory pressure.

**Multi-pattern composition** (mirrors current OR/AND semantics):

- `mode_and`: final candidate set = INTERSECTION of per-pattern sets.
  A pattern with no usable literals contributes "all files" (no narrowing),
  so AND mode still benefits when at least one pattern is extractable.
- `mode_or` (default): final candidate set = UNION of per-pattern sets.
  **If ANY pattern is un-prefilterable in OR mode -> full scan fallback**,
  because the unconstrained pattern could match any file.

**Wiring in `handle_line_regex_search` (`src/mcp/handlers/grep.rs:1924`):**

1. After regex compile, before main loop: `ensure_trigram_index(ctx)`.
2. Compute prefilter (above). Set `prefilter_used: bool` and
   `final_candidates: Option<HashSet<u32>>`.
3. Iterate `index.files.iter().enumerate()`; if `prefilter_used`, skip files
   whose `file_id` is not in `final_candidates`. Existing
   `passes_file_filters` + `read_file_lossy` + whole-content precheck +
   per-line scan are unchanged (precheck stays -- it is still useful against
   trigram over-approximation false positives).

**Differential correctness check** (in lieu of a proptest dep):

Under `#[cfg(test)]`, `handle_line_regex_search` runs BOTH the prefilter
path and the full-scan path and asserts the result sets match. Every
existing `tests_line_regex` integration test transparently becomes a
differential equivalence test -- zero new fuzz cases needed for coverage.
Production builds run only the prefilter path.

**Observability -- new summary field** (emitted only on lineRegex paths):

```jsonc
"literalPrefilter": {
  "used": true,
  "candidateFiles": 1200,
  "totalFiles": 66694,
  "extractedLiterals": ["app"],     // capped to 5 entries, 32 bytes each
  "shortCircuited": false,            // present only when relevant
  "reason": "..."                     // present only when used=false
}
```

**`line_regex_perf_hint` re-tune** -- accepts `prefilter_used: bool`. Hint text
branches: `used=true` slow scan now blames "common literal matched X% of
files" rather than "no fixed literal". `used=false` path keeps the existing
guidance (which is already correct for that case).

#### Acceptance numbers

- Scan time for `App\s*=\s*\d+` on Shared (66 694 files) drops from ~48 s
  to <500 ms (target 100x+ speedup; user-story originally cited <200 ms).
- Pattern with no extractable literal (e.g. `\w+_\d+`) stays at baseline
  full-scan time -- explicit non-regression invariant.
- `mode=and` with at least one extractable pattern shows speedup
  proportional to its narrowest literal.
- All existing `tests_line_regex` cases pass under the differential
  `#[cfg(test)]` wrapper (correctness invariant: no false negatives).

#### Dependencies

- Add `regex-syntax = "0.8"` explicitly to `[dependencies]` in `Cargo.toml`.
  Already present transitively via `regex`; bundle-size impact is zero.
  MSRV 1.91 compatible.

#### Tests

- ~18 unit tests on `extract_required_literals` covering: anchored patterns,
  alternation, case-insensitive flag, char-class start, empty alternation
  branch, lookaround, backreference (graceful fallback on regex-syntax
  reject), unicode literals, very long alternations.
- Integration tests on the new `literalPrefilter` summary field across OR /
  AND modes and the short-circuit branch.
- Differential `#[cfg(test)]` wrapper provides equivalence coverage for free
  on every existing lineRegex integration test.

#### Bench / measurements

- Criterion bench `bench_line_regex_literal_extraction` in
  `benches/search_benchmarks.rs` with synthetic 10k-file `.cs` fixture for
  CI regression detection.
- PowerShell repro script `scripts/measure-ac4-shared.ps1` (3 canonical
  calls x 3 runs, drop cold-cache run, median of remaining two) for
  user-side validation on the 66k-file Shared repo.
- Numbers recorded in `docs/measurements/ac4-literal-extraction-bench.md`
  (baseline + after).


## Out of scope

- Limiting lineRegex to small repos (would break legitimate use).
- Auto-rewriting user regex (too magic; warn and let caller decide).

## Validation

- Unit test: invoke `xray_grep` with `lineRegex=true terms=["foo.*bar"]`
  on a 10k+ file fixture; assert response contains `hint` and `warning`
  fields.
- Manual: re-run the two slow calls above; confirm hint appears.
- For AC-4: differential `#[cfg(test)]` equivalence check on every existing
  lineRegex integration test (no false negatives invariant); synthetic
  criterion bench for CI regression detection; manual replay of the three
  canonical Shared calls via `scripts/measure-ac4-shared.ps1`, expect
  100x+ speedup recorded in `docs/measurements/ac4-literal-extraction-bench.md`.

## Notes

This is a sibling story to
`user-story_xray-grep-ext-param-schema-mismatch_2026-04-26.md` (also filed
2026-04-26). Both came out of one PR-review session in the Shared repo.
