# `literalPrefilter` reports global counts when `dir`/`file` filter scopes the query

**Date:** 2026-04-26
**Status:** Proposed (incidental finding from cross-validation of feat/grep-alternation-split-hint)
**Severity:** MINOR (cognitively misleading, not a correctness bug)

## Background

During cross-validation of the alternation-split advisory on the Shared
66 743-file repo (2026-04-26), we ran the same `xray_grep` query both
unscoped and scoped to a sub-directory:

```jsonc
// A:  unscoped
xray_grep terms=["OrgApp.*TypeId|App.*TypeId\\s*=\\s*\\d"]
         lineRegex=true ext=["cs"]
// → searchTimeMs=80053, literalPrefilter.totalFiles=66764, candidateFiles=22581

// A': same query scoped to a sub-tree
xray_grep terms=["OrgApp.*TypeId|App.*TypeId\\s*=\\s*\\d"]
         dir="Sql/CloudBI/AS/src/PowerBI" lineRegex=true ext=["cs"]
// → searchTimeMs=6204, literalPrefilter.totalFiles=66764, candidateFiles=22581
```

Note that **A' has the same `totalFiles` (66 764) and `candidateFiles`
(22 581) as A**, even though the query was scoped to a much smaller
subtree. The `dir` filter is applied **after** the trigram-based
prefilter — so the candidate set the prefilter computes covers the
whole indexed corpus, then the per-file scan honours `dir`.

## The misleading user perception

A user inspecting `summary.literalPrefilter` for a scoped query sees:

- `candidateFiles: 22581 / totalFiles: 66764` — implying ~34% of the
  corpus survived the prefilter.

But the actual files ultimately scanned by the per-line regex pass are
only those *under* `Sql/CloudBI/AS/src/PowerBI`, intersected with the
candidate set. The reported numbers neither reflect:

- The number of files **actually scanned** (intersection of candidate
  set with `dir`/`file`/`ext` filters), nor
- The selectivity **within the scoped subtree** (would let the user
  judge whether the prefilter helped *for their query*).

Net effect: the literalPrefilter block looks the same for scoped and
unscoped queries despite vastly different effective behaviour. Users may
conclude "prefilter isn't narrowing my scoped query" when the truth is
"the scoping itself already narrowed it; the prefilter operates on the
pre-scope corpus."

## Proposed fix (sketch)

Two non-mutually-exclusive options:

### Option A: Add post-filter counters

Surface additional fields in `literalPrefilter`:

- `candidateFilesAfterScope` — candidate set ∩ (dir/file/ext filters).
- `totalFilesAfterScope` — total files matching the scope filters
  (without the prefilter).

This preserves backward compatibility (existing fields untouched) and
gives clients/LLMs the per-query selectivity view.

### Option B: Document current semantics

Add a paragraph to `docs/mcp-guide.md`'s "Summary Fields (grep-specific)"
table clarifying that `candidateFiles` / `totalFiles` reflect the
**indexed corpus pre-scope**, and that scoped queries should look at
`searchTimeMs` to gauge effectiveness.

Option B is zero-effort and immediately honest; Option A is the right
long-term answer if we want the perfHint advisory logic itself to
become scope-aware (e.g. don't suggest `dir=` narrowing if the user
already provided one).

## Acceptance criteria (sketch)

- AC-1: When a scoped query is issued, `literalPrefilter` carries
  enough information for the user (or the perfHint heuristic) to
  distinguish "prefilter narrowed the indexed corpus" from "scope
  narrowed the indexed corpus".
- AC-2: Existing fields (`candidateFiles`, `totalFiles`,
  `extractedFragments`, `hasTopLevelAlternation`) keep their current
  semantics — clients of PR #222 must not break.
- AC-3: Documentation in `docs/mcp-guide.md` describes the new fields
  (or, in Option B, the limitation of the existing fields).
- AC-4: At least one integration test asserts the new field
  semantics on a scoped query.

## Non-goals

- Changing the order of prefilter vs scope filtering in the actual
  search pipeline (current order is correct: trigram lookup happens
  on the indexed posting lists, scope is enforced when materialising
  hits).
- Auto-suppressing the alternation-split advisory in scoped queries
  (separate concern; depends on whether scope already narrowed enough
  to make the advisory moot).

## Source of finding

Independent LLM cross-validation against the Shared repo, 2026-04-26
(see PR #222 review thread for the full session log).
