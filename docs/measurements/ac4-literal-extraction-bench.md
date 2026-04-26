# AC-4 — `xray_grep` `lineRegex` literal-trigram prefilter, end-to-end measurement

This document records cold/warm `searchTimeMs` for the three canonical
calls cited in
[`user-story_xray-grep-lineRegex-perf-hints_2026-04-26.md`](../../user-story_xray-grep-lineRegex-perf-hints_2026-04-26.md),
measured on the Shared (60k C# file) repository against:

- **Baseline binary** — `main` HEAD just before the AC-4 merge commit.
- **Feature binary** — this branch (`feat/grep-literal-extraction`).

Numbers are produced by [`scripts/measure-ac4-shared.ps1`](../../scripts/measure-ac4-shared.ps1).
The script spawns a fresh `xray serve` per call-run, drops the cold-cache
run, and reports the median of the remaining two warm runs. To reproduce:

```pwsh
# Baseline
git checkout <main HEAD before merge>
cargo build --release
Copy-Item .\target\release\xray.exe .\target\release\xray-baseline.exe -Force

# Feature
git checkout feat/grep-literal-extraction
cargo build --release
Copy-Item .\target\release\xray.exe .\target\release\xray-feat.exe -Force

# Measure each, write JSON for diff
pwsh scripts/measure-ac4-shared.ps1 -Repo C:\path\to\Shared `
    -XrayBin .\target\release\xray-baseline.exe -Runs 3 `
    -JsonOut .\docs\measurements\ac4-baseline.json
pwsh scripts/measure-ac4-shared.ps1 -Repo C:\path\to\Shared `
    -XrayBin .\target\release\xray-feat.exe -Runs 3 `
    -JsonOut .\docs\measurements\ac4-feat.json
```

## Calls measured

| # | Label                  | Pattern                                  | Mode      |
|---|------------------------|------------------------------------------|-----------|
| 1 | `OrgAppTypeId_constant`| `OrgAppTypeId\s*=\s*\d+`                 | lineRegex |
| 2 | `App_constant`         | `App\s*=\s*[0-9]+`                       | lineRegex |
| 3 | `OrgApp_OR_App_typeid` | `OrgApp.*TypeId|App.*TypeId\s*=\s*\d`    | lineRegex |

## Results — Shared repo (60k+ files)

> **TODO** — fill this table by running the script on the real Shared
> repo with both binaries. Until then the column values are placeholders.
> The story's reference baseline (pre-AC-4) is `76 362 ms` cold for
> call 1 and `48 136 ms` warm for call 2.

| Call                  | Baseline cold (ms) | Baseline warm (ms) | Feat cold (ms) | Feat warm (ms) | Speedup (warm) | `prefilterUsed` | `candidateFiles` / `totalFiles` | `perfHint` |
|-----------------------|--------------------|--------------------|----------------|----------------|----------------|-----------------|---------------------------------|------------|
| `OrgAppTypeId_constant` | TBD              | TBD                | TBD            | TBD            | TBD            | TBD             | TBD                             | TBD        |
| `App_constant`          | TBD              | TBD                | TBD            | TBD            | TBD            | TBD             | TBD                             | TBD        |
| `OrgApp_OR_App_typeid`  | TBD              | TBD                | TBD            | TBD            | TBD            | TBD             | TBD                             | TBD        |

## Interpretation guide

- **Speedup ≈ 1×** with `prefilterUsed=false` → expected: the regex had no
  extractable literal of `MIN_LITERAL_LEN = 3` chars, OR the candidate
  set covered `> 50%` of the corpus and was discarded by the
  short-circuit guard. `summary.literalPrefilter.reason` explains which.
- **Speedup ≈ 1×** with `prefilterUsed=true` and `candidateFiles ≈ totalFiles`
  → unexpected: the prefilter triggered but selected almost every file.
  Investigate why the literal `extractor` chose a near-universal trigram.
- **Speedup `>= 10×`** with small `candidateFiles` → working as designed.
  The user-story target is `~100×–10 000×` for calls 1 and 3 on Shared.
- **`perfHint` still firing on the feature binary** with the prefilter
  applied means the post-prefilter set was still large enough to take
  `≥ 2 s`. The hint copy on the feature path explicitly mentions
  *"even with the literal-trigram prefilter applied"* so readers know
  the slow scan is genuine, not a missed prefilter opportunity.

## Microbench (per-call primitive cost)

`benches/search_benchmarks.rs::bench_line_regex_literal_extraction`
measures the dominant new per-call cost in isolation: the
`regex-syntax::ParserBuilder` parse + `hir::literal::Extractor` walk on
the user's pattern. Five patterns probe the cost spectrum:

| Pattern                      | Expected outcome                                              |
|------------------------------|---------------------------------------------------------------|
| `App\s*=\s*\d+`              | One literal extracted, fast happy path                        |
| `^\s*pub\s+fn\s+\w+`         | Anchored + word literal, fast                                 |
| `OrgAppTypeId|AppType\d+`    | OR composition with both branches prefilterable               |
| `\d+`                        | No extractable literal, exits early                           |
| `(foo|bar|baz|...)*`         | Long OR chain, worst-case literal-set walk                    |

Runtime budget: each pattern should parse + extract in `< 50 µs` on a
modern CI runner. If a future regression pushes any pattern over `1 ms`
the prefilter becomes a net loss for small repos and the gating
threshold needs to be re-tuned.

## Provenance

- Story: [`user-story_xray-grep-lineRegex-perf-hints_2026-04-26.md`](../../user-story_xray-grep-lineRegex-perf-hints_2026-04-26.md), §AC-4
- CHANGELOG entry: [2026-04-26 — Performance](../../CHANGELOG.md)
- Implementation: [`src/mcp/handlers/grep_literal_extract.rs`](../../src/mcp/handlers/grep_literal_extract.rs), [`src/mcp/handlers/grep.rs`](../../src/mcp/handlers/grep.rs)
- Differential test (parity, not perf): `test_xray_grep_line_regex_prefilter_differential_parity` in [`src/mcp/handlers/handlers_tests_grep.rs`](../../src/mcp/handlers/handlers_tests_grep.rs)
