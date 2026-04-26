# `xray_grep` advisory: split top-level alternation when prefilter extracts a low-selectivity common prefix

**Date**: 2026-04-26
**Origin**: Live cross-session validation of AC-4 (literal-trigram prefilter)
on `C:\Repos\Shared` (66 743 .cs files). Full measurement at
[`docs/measurements/ac4-literal-extraction-bench.md`](docs/measurements/ac4-literal-extraction-bench.md)
(section "Alternation-split impact").
**Type**: UX / advisory hint (no behaviour change in prefilter machinery).
**Severity**: Medium — silent ~45× pessimization for a common regex shape.

---

## Problem

When a user passes a `lineRegex` term that contains top-level alternation
(`A|B`) where `A` and `B` start with disjoint literals, the
`regex-syntax::hir::literal::Extractor` returns the **common prefix**
of those literals — typically short and frequent.

Real example from production-scale validation:

| Variant | extractedFragments | candidateFiles / total | warm elapsedMs |
|---|---|---|---|
| `terms=["OrgApp.*TypeId\|App.*TypeId\\s*=\\s*\\d"]` | `["app","orgapp"]` | 22 580 / 66 743 (34 %) | **44 323** |
| `terms=["OrgApp.*TypeId", "AppTypeId\\s*=\\s*\\d"]` | `["apptypeid","orgapp"]` | **598 / 66 743** (0.9 %) | **1 016** |

Identical match semantics on this dataset. **45× speedup just by
restructuring the call** — but the user has no way of knowing that.
The current `summary.literalPrefilter` block reports `used: true`,
`extractedFragments: ["app","orgapp"]`, and provides no actionable
guidance.

## Why this is invisible

- `used: true` looks like the prefilter "worked" — the failure mode
  is hidden inside the `candidateFiles / totalFiles` ratio.
- `perfHint` only fires for `used=false + slow` (round-3 fix) or
  `used=true + slow + already-narrow-set` (existing applied-but-slow
  hint). Neither current branch surfaces the alternation-split
  opportunity.
- LLM agents experimenting with `xray_grep` will retry with
  `dir=`/`file=`/`ext=` narrowing (which the existing hints suggest)
  but won't think to split the regex itself.

## Proposed advisory

When **all** of the following are true after a `lineRegex` call:

1. `summary.literalPrefilter.used == true`
2. `candidateFiles / totalFiles > 0.20` (configurable threshold)
3. The original regex contains a top-level `|` alternation
   (detect via `regex-syntax::hir::Hir::Alternation` at root or
   single-element capture group)
4. `search_elapsed_ms >= LINE_REGEX_SLOW_MS` (reuse existing slow gate)

→ Append to (or set) `summary.perfHint`:

> "lineRegex took {ms}ms. The prefilter applied with fragments
> `{fragments}` but kept {ratio}% of files as candidates. Your regex
> contains a top-level `|` alternation; the literal extractor returns
> only the common prefix of OR branches. Splitting the alternation
> across separate `terms[]` lets each branch contribute its own literal
> independently, often yielding a much more selective fragment set.
> Example: instead of `terms=[\"A.*X|B.*X\"]`, try
> `terms=[\"A.*X\", \"B.*X\"]`. See `summary.literalPrefilter.extractedFragments`
> for the current set."

## Acceptance criteria

- [ ] **AC-1** Detector function `regex_has_top_level_alternation(&Hir) -> bool`
      in `src/mcp/handlers/grep_literal_extract.rs` (or sibling), with
      unit tests covering: pure `A|B`, `(A|B)` capture-wrapped,
      `^(A|B)$`, nested alternation `A|(B|C)`, no-alternation, char-class
      `[ab]` (NOT alternation), `\d|x` (alternation but with non-literal
      branch — extractor would still return weak fragments).
- [ ] **AC-2** New advisory branch in `apply_literal_prefilter_summary`
      (or `line_regex_perf_hint`) gated on the four conditions above.
      Existing `perfHint` text is replaced (not appended) when the
      alternation-split branch fires — it's strictly more actionable
      than the generic "applied but slow" hint.
- [ ] **AC-3** Threshold (`LINE_REGEX_PREFILTER_LOW_SELECTIVITY_RATIO`,
      default `0.20`) defined as a module-level `const` next to the
      existing `LINE_REGEX_SLOW_MS` and `LINE_REGEX_LARGE_INDEX_FILES`.
- [ ] **AC-4** Two integration tests in `handlers_tests_grep.rs`:
      one positive (regex with alternation triggers the new hint),
      one negative (regex without alternation falls through to existing
      hint).
- [ ] **AC-5** CHANGELOG entry under "Performance" / "Hints":
      "Added advisory hint when `lineRegex` uses top-level `|`
      alternation and the literal-trigram prefilter extracted only a
      low-selectivity common prefix. Suggests splitting across
      `terms[]` for up to 45× speedup on production-scale data
      (measured on 66 743-file C# repo)."
- [ ] **AC-6** Docs: append a "Top-level alternation pessimization"
      subsection to `docs/mcp-guide.md` (or wherever `xray_grep` tips
      live) with the OrgApp/App example and before/after numbers.

## Out of scope

- **Auto-splitting** the alternation server-side. We deliberately keep
  the user's `terms[]` as the source of truth — splitting would change
  occurrence-count semantics for OR-overlapping branches and surprise
  callers who rely on per-term aggregation.
- **Improving extractor selectivity directly** (e.g., switching to
  `regex-syntax`'s `prefixes` + `suffixes` + `inner_literals` beyond
  what we already extract). Tracked separately if measurements
  warrant.
- **Detecting alternation inside lookarounds / nested groups beyond
  one level** — diminishing returns; covers the "I wrote `A|B`
  manually" case which is the dominant intent.

## Provenance

- Trigger measurement: see "Alternation-split impact" table in
  [`docs/measurements/ac4-literal-extraction-bench.md`](docs/measurements/ac4-literal-extraction-bench.md).
- Parent story: [`user-story_xray-grep-lineRegex-perf-hints_2026-04-26.md`](user-story_xray-grep-lineRegex-perf-hints_2026-04-26.md)
  (AC-4 implementation that uncovered this gap).
- Validating session: cross-session MCP-agent run on `C:\Repos\Shared`,
  binary `xray 0.1.0 (sha=c0dd75c-dirty)`, 2026-04-26.
