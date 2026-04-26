# Smart `.*`-aware suppression of alternation-split advisory

**Date:** 2026-04-26
**Status:** Proposed (follow-up from cross-validation of feat/grep-alternation-split-hint)
**Related:** PR #222 (alternation-split advisory landed in main)

## Background

PR #222 added a `summary.perfHint` advisory that tells the user to split a
top-level `|` alternation across separate `terms[]` entries when the
literal-trigram prefilter fails to narrow the candidate set. The advisory
detection is correct (4/4 oracles pass on real Shared 66 743-file repo
cross-validation, 2026-04-26), but the **rewrite the advisory recommends
does not always deliver the speedup the hint implies**.

### Cross-validation evidence (2026-04-26, Shared repo, 66 743 .cs files)

| Rewrite | `searchTimeMs` | `candidateFiles` | Notes |
|---|---|---|---|
| Single OR'd term `OrgApp.*TypeId\|App.*TypeId\s*=\s*\d` | 80 053 | 22 581 | Baseline; advisory fires here. |
| Split, semantics-preserving (`.*` in both branches) | 27 491 | 22 581 | **~2.9× — gain comes from simpler per-line regex, NOT prefilter** |
| Split, narrower second term `AppTypeId\s*=\s*\d` (drops `.*`) | ~1 000 | 598 | ~80× — but **changes match semantics** |

### Root cause

`regex-syntax::hir::literal::Extractor` stops at `.*` / `.+` boundaries.
For a regex like `App.*TypeId\s*=\s*\d`, it derives the literal `app`,
not `apptypeid` — the `.*` blocks bridging across to the next contiguous
literal span. Splitting `A.*X|B.*X` into `terms=["A.*X","B.*X"]` therefore
yields the same `["a","b"]`-style fragment set as the union OR'd form,
producing no prefilter improvement.

For OR'd terms whose branches have **contiguous** literals (e.g.
`error|warning|fatal` → `terms=["error","warning","fatal"]`) the split is
genuinely effective — each branch contributes its own ≥3-char literal.

### Current mitigation (round-1 / round-2 fixup commits)

The hint text in `apply_literal_prefilter_summary` (grep.rs) was updated
to carry an explicit caveat: *"Note: speedup depends on each branch
containing a contiguous literal of ≥3 chars; patterns like `A.*X|B.*X`
whose branches have `.*` between literals may see a smaller speedup
because the extractor still only derives the shared `a`/`b`-style
prefixes."* Docs updated likewise.

This is honest but suboptimal — we still surface the advisory even when
the rewrite won't materially help.

## Proposed enhancement

Detect at advisory-evaluation time whether the alternation's branches
have contiguous literals long enough to materially improve selectivity.
If not, **suppress the alternation-split advisory** and fall through to
the generic "applied but slow" hint.

### Design sketch

1. Walk the regex HIR for the single alternation pattern.
2. For each top-level branch, ask the literal extractor what fragment(s)
   it would produce *in isolation* (current code only knows the union
   fragments).
3. If every branch's per-branch fragment set is identical to (or a
   superset of) the union fragment set — i.e. splitting wouldn't unlock
   new selectivity — suppress the advisory.
4. Otherwise emit the advisory (and optionally enrich the hint with the
   per-branch fragment list to make the recommended rewrite concrete).

### Acceptance criteria (sketch)

- AC-1: Pure-literal branches case (`error|warning|fatal`): each branch
  produces its own ≥3-char literal → advisory fires (unchanged behavior).
- AC-2: `.*`-blocked branches case (`A.*X|B.*Y`): per-branch fragments
  equal the union → advisory suppressed; generic hint stands.
- AC-3: Mixed case (one branch contributes new selectivity, another
  doesn't) — advisory fires, but hint mentions only the contributing
  branches as the rewrite target.
- AC-4: AND-mode and mixed-batch suppression unchanged (PR #222 round-1).
- AC-5: New unit + integration tests covering the three regimes against
  the helper introduced for per-branch literal extraction.

### Non-goals

- Auto-rewriting the user's `terms[]` server-side (still rejected, see
  PR #222 deliberate non-goals).
- Reformulating the user's regex (advisory only suggests splitting, never
  semantic edits).

## Open questions

- Does `regex-syntax::hir::literal::Extractor` have a stable API for
  per-branch extraction, or do we need to reconstruct one HIR per branch
  and re-run the extractor? The latter is straightforward but
  per-advisory-evaluation cost; mitigate by short-circuiting to
  no-suppression when the regex has only 2-3 branches (cheap re-extract).
- For `A|B.*X`-shaped regexes (one literal branch, one `.*`-bearing) —
  what's the right oracle? Probably advisory should fire because the
  literal branch IS new selectivity, even though one branch isn't.
