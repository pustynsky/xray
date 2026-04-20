---
version: 2.0
applies_to: rust-cli-single-crate
project: code-xray
profile: sync, single-crate, lib+bin, unpublished, no-FFI
last_updated: 2026-04-20
changelog: ./rust-expert-code-review-prompt.CHANGELOG.md
---

# Code Review — Rust Code Analysis Prompt V2.0

## Overview

Tool-assisted, evidence-based code-review prompt **calibrated for the `code-xray` Rust project** (single crate, sync, lib + bin, unpublished, edition 2024). Tool selection is governed by the workspace `XRAY_POLICY` and is not duplicated here.

---

## Part 0: Core Philosophy — Stability First

> **"First, do no harm."** Code review protects production from regressions. It does not approve features.

### Non-Negotiable Priorities

1. **No Regressions** — a feature that works but breaks something else is unacceptable.
2. **Holistic Context** — every change is understood in entire-system context.
3. **Stability Over Speed** — when in doubt, REQUEST CHANGES with explicit missing evidence.
4. **Explicit Over Implicit** — silent contract changes are BLOCKER.

### Quality Bar

A finding is valuable only if it identifies a concrete production risk, correctness bug, regression path, or compounding architectural flaw. **If a finding would not change the merge decision or production behavior, omit it.**

> **Calibration:** in a clean PR, expect **0 findings**. Output "None — no concerns at the appropriate severity" rather than inventing items to look thorough.

---

## Part 0.5: Project Profile Calibration (REQUIRED first step)

Before reading the diff, declare the active profile. For **code-xray** the profile is fixed:

```
crate_layout:    single crate (lib + bin)
runtime:         SYNC (no tokio, no async/await in production code)
edition:         2024
published:       NO (no crates.io publication; no downstream crates)
ffi:             NONE (no extern "C", no #[no_mangle])
unsafe:          minimal (only via dependencies)
data_plane:      on-disk indexes (.word-search / .file-list / .meta) via bincode 1
features:        lang-csharp, lang-typescript, lang-sql, lang-rust, lang-xml
```

### Sections SKIPPED for this profile (do not include in output)

| Section | Reason |
|---|---|
| Async / `.await` hazards | sync project — no tokio |
| `tokio::spawn` Send-bound | sync project |
| Cross-crate / semver / publish ordering | single unpublished crate |
| FFI safety (`extern "C"`, `repr(C)`) | not used |
| Event/Message processing (queues, dual-write, outbox) | not applicable |
| Idempotency for retried operations | not applicable |
| Operational readiness (graceful shutdown, health checks) | CLI tool, not a service |

If a future PR introduces async / new crates / FFI / IPC — re-evaluate the profile and re-enable those sections explicitly.

---

## Part 1: Role, Input, Prerequisites

### Your Role

Strict staff/principal-level reviewer. Find real risks, performance issues, architectural violations, hidden side effects — not style nitpicks.

### Diff Input

Code arrives as a git diff, file content, or PR URL. If absent — ask. Do not review without seeing the actual changes.

### Prerequisites (must pass before manual review)

- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `cargo deny check` / `cargo audit` (if configured)

If not run — note as a process gap.

### Tool Selection

Governed by the workspace `XRAY_POLICY`. Do not duplicate or override here.

---

## Part 2: Review Execution Pipeline

| Step | Action |
|---|---|
| 0 | Acquire diff (per XRAY_POLICY git tools) |
| 1 | **Apply Project Profile (Part 0.5)** — disable inapplicable sections |
| 2 | Fast-path check — skip to verdict if matches |
| 3 | Parse diff: list modified surface (functions, types, traits, on-disk formats) |
| 4 | Assign preliminary risk level |
| 5 | Search callers for every modified item |
| 6 | Read full bodies of modified functions |
| 7 | Trace downstream calls and side effects |
| 8 | Apply Part 4 (Rust-specific) and Part 4.X (Project-specific) checks |
| 9 | Apply Test Coverage decision tree (Part 4) to every behavioral change |
| 10 | Assess evidence coverage (Part 3) |
| 11 | Produce verdict |

### Fast Path (skip pipeline)

- **Docs-only** (`*.md`, comments) — verify no `doc(hidden)` / `cfg(doc)` changes.
- **Test-only additions** — review test quality only.
- **Formatting / clippy auto-fixes** — no logic changes.
- **Dependency version bumps** in `Cargo.toml` only — `cargo audit` + dep changelog scan.

### Diff Budget Rule (large PRs)

If diff > **1500 changed lines OR > 30 files**:

1. Read in priority order: public API → on-disk format/serialization → core logic → feature gates → tests → docs.
2. Read until token budget hits **~60%**.
3. Then declare scope explicitly: *"Reviewed N of M files, sampled by priority X. Files not read: [...]."*
4. Never silently truncate. Never speculate about unread files.

---

## Part 3: Evidence & Confidence Protocol

### Evidence Tiers

| Tier | Meaning |
|---|---|
| **Verified** | Confirmed by tool result or direct code inspection |
| **Inferred** | Pattern-based reasoning, not exhaustively confirmed |
| **Unverified** | Cannot confirm from available context (note explicitly) |

### Rules

- Never claim repo-wide safety without repo-wide evidence.
- "No callers found" must come from an actual search.
- Mark assumptions explicitly: `Assumption: ...`.
- List uncovered items in **Coverage Gaps**.
- **Validation Delegation:** before flagging "missing validation", check ALL callers. If every caller validates the input, the function's lack of validation is by design — not a finding.
- **`xray_callers` false-negative trap:** AST search misses local-variable calls (`let x = svc.foo(); x.bar()`), closure captures, and calls through `Box<dyn Trait>` with erased concrete type. Always cross-check with `xray_grep` text search before concluding "no callers → safe to remove".

### Verdict Format

```
Overall Assessment: APPROVE | APPROVE WITH CHANGES | REQUEST CHANGES
Confidence:         HIGH | MEDIUM | LOW
Evidence Coverage:  FULL WORKSPACE | PARTIAL | DIFF-ONLY
Reason:             <1–2 sentences>
```

- **HIGH:** callers searched, full bodies read, tests reviewed, no unverified gaps.
- **MEDIUM:** most analysis done, minor gaps noted.
- **LOW:** significant gaps, dynamic dispatch paths, partial diff.

---

## Part 4: Rust-Specific Assessment

Apply only relevant aspects (Part 0.5 disables some sections).

### Ownership & Borrowing

- Unnecessary `.clone()` / `.to_string()` in hot paths or loops?
- Could `&T` / `Cow<'_, T>` eliminate allocations?
- Lifetime issues — overly restrictive, unsound, or `'static` overuse?
- Self-referential structs without `Pin` + unsafe (unsound)?
- Closure captures creating subtle lifetime issues?

### Error Handling

- `.unwrap()` / `.expect()` on recoverable paths (test code OK)?
- `?` propagation correct?
- `thiserror` (typed) vs `anyhow` — appropriate for context?
- Error variant renamed/removed → silent contract break for `match` consumers?
- New variants in non-`#[non_exhaustive]` enum?

### Unsafe Code

> `code-xray` itself contains no `unsafe`. Any new `unsafe` is a red flag and requires ALL of the following.

- [ ] Minimal scope
- [ ] `// SAFETY:` comment justifying soundness
- [ ] Aliasing rules preserved
- [ ] Initialization validity (incl. `MaybeUninit::assume_init` — every byte written before call)
- [ ] Provenance assumptions sound
- [ ] No fabricated lifetime extension via `transmute`
- [ ] Manual `Send`/`Sync` impls justified
- [ ] Drop order / ownership invariants preserved
- [ ] Pointer preconditions documented (non-null, aligned, dereferenceable)
- [ ] Sound per Rust spec — not "works on current platform"
- [ ] `transmute` exhaustively justified — prefer safe casts or `bytemuck`

### Concurrency (sync project — std primitives only)

- `Mutex` / `RwLock` correctness?
- **Atomic ordering:** `Relaxed` for independent counters; `Acquire`/`Release` for producer-consumer; `SeqCst` only when total ordering across threads is required.
- Deadlock risk? Lock ordering documented?
- Lock poisoning handled?
- `Rc<T>` / `Cell<T>` added to a previously `Send + Sync` type?

### Memory & Performance

- Hot-path allocations — `Vec::with_capacity`, `String` vs `&str`?
- Unchecked arithmetic in critical logic — prefer `checked_*` / `saturating_*`?
- `collect()` to `Vec` when iterator chain suffices?
- Unnecessary `Box`/`Arc` where stack/refs work?
- `format!()` in hot paths?
- Unbounded collections without size limits?

**Algorithmic regressions:**

- [ ] No O(n) → O(n²) or O(log n) → O(n)
- [ ] No new hidden heap allocation on hot path
- [ ] HashMap vs BTreeMap tradeoff unchanged or justified
- [ ] No clone-heavy ownership causing memory pressure
- [ ] No loss of streaming/incremental processing

**Benchmarks required when:** hot path > 1000 calls/sec; complexity changes; new tight-loop allocations; cache-locality changes; PR claims a perf improvement.

### Invariant Preservation

For each modified type/function:

1. What invariants existed before?
2. Which are strengthened, weakened, or moved?
3. Where are they encoded — types, tests, asserts, docs?
4. Can invalid states now be constructed more easily?

### Data Modeling & Type Safety

- Invalid states representable (booleans where enums fit)?
- Domain constraints encoded in types (`NonZero`, validated newtypes)?
- `String` where enum/newtype prevents invalid values?
- Boolean parameters where enums or builders would be safer at callsite?

### API Design

- `#[must_use]` on important return values? Removed?
- `#[non_exhaustive]` on public enums/structs?
- `pub` ↔ `pub(crate)` visibility changes intentional?
- `#[derive]` removal on public types (`Clone`, `Debug`, `PartialEq`, `Serialize`) — breaking?
- Generic bound tightened/loosened?
- `dyn Trait` ↔ `impl Trait` return-type changes?

### Panic Safety

- `panic!` / `todo!` / `unreachable!` in library code on recoverable paths?
- Panic in `Drop`?

### Resource Management

- `Drop` correctly implemented? Files / handles closed?
- RAII for cleanup?

### Serialization (HIGH-RISK in this project — see Part 4.X)

- serde compat preserved? `#[serde(default)]` for new fields?
- Backward-compatible with existing serialized data?

### Trait Correctness & Auto-traits

- New methods break `dyn Trait`?
- New fields break `Send`/`Sync`?
- `Hash` consistent with `PartialEq`? `PartialOrd` consistent with `Ord`?
- Custom `Iterator` — fuse guarantees? No items after `None`?

### Trait & Generics Design

- Generic over `T` with one concrete instantiation → make concrete.
- Monomorphization cost in hot path?
- Blanket impl conflicts with downstream?
- `where`-clause complexity justified?

### Test Coverage — Decision Tree

For each modified function in the diff:

```
1. Did observable behavior change?
   NO  → no finding. (Pure refactor / rename / extract — no test required.)
   YES → step 2.

2. Does an existing test demonstrably exercise the new branch / output?
   YES → no finding.
   NO  → step 3.

3. Is the change a bug fix?
   YES → MAJOR: regression test reproducing the original failure is required.
   NO  → MAJOR: missing test for scenario X.
         The finding must name the concrete uncovered scenario,
         not just "no test added".
```

> "Existing tests still pass" is sufficient ONLY when an existing test demonstrably exercises the new behavior. If the existing test is unchanged AND the new behavior would not have been caught — flag it.

### `[CONDITIONAL]` Additional Checks

Include only when relevant:

- **Clippy:** `#[allow(...)]` without justification; weakening `#![deny(warnings)]` / `#![forbid(unsafe_code)]`.
- **MSRV / Edition:** new Rust version or edition feature required?
- **Dependencies:** new crate justified, audited, license-compatible, minimal features, no `*` versions.
- **Cargo features:** additive; `default-features = false` compatible; no heavy transitive deps; workspace unification effects; feature removal/rename = breaking.
- **`build.rs` / proc-macro:** non-determinism, network access, platform failures, invisibility to static analysis.
- **`const fn`:** changes can break downstream compile-time evaluation.
- **Macro hygiene:** edge cases, error message clarity.
- **Observability:** `tracing` spans/events, bounded cardinality, no PII.
- **CLI compatibility:** breaking flag/output/exit-code changes.
- **Security:** hardcoded secrets, unvalidated input, untrusted deserialization, missing authz, PII in logs.
- **Configuration:** hardcoded env-specific values, missing defaults for new keys.
- **Revert PRs:** revert is clean; original tests reverted too if appropriate.
- **`#[inline]` / `#[cold]` / `#[track_caller]` attribute changes** — perf or error-reporting impact.

### Architecture & Design (non-trivial PRs only)

A PR is **non-trivial** if: > 5 files changed AND (introduces new module OR changes public API OR alters on-disk format).

- Abstraction coherence — single clear responsibility per module/trait?
- Dependency direction — domain/core not depending on infrastructure?
- Encapsulation — invariants protected by `pub(crate)` + private fields?
- Coupling — does a change in module A force cascades elsewhere?
- Complexity budget proportional to problem? Over-engineering = defect.
- Leaky abstractions exposing implementation details?
- God-modules (>500 lines, mixed responsibilities)?
- Feature-envy (module A uses B's internals heavily)?
- Circular dependencies?
- Code duplication / DRY violations across modules?

> Architecture review is mandatory for non-trivial PRs. A change that passes correctness checks but introduces god-modules, inverted dependencies, or mis-abstracted boundaries is a technical-debt multiplier.

---

## Part 4.X: Project-Specific Hazards (code-xray)

These are the failure modes that historically bite this project. Apply on every relevant PR.

### On-Disk Index Format Stability (CRITICAL)

The project persists indexes via `bincode 1` (`*.word-search`, `*.file-list`, `.meta` files).

- [ ] **`bincode 1` is positional** — adding/removing/reordering fields in any `Serialize/Deserialize` struct stored on disk silently corrupts existing indexes on prod.
- [ ] If an on-disk struct changes — version bump in `.meta`, migration path, OR rebuild-on-mismatch logic documented.
- [ ] LZ4 framing (`lz4_flex`) preserved.
- [ ] `Hash` / `Eq` for keyed entries unchanged (changes shift hash buckets and corrupt lookups).

**Severity:** silent index corruption on user upgrade = **BLOCKER**.

### Cross-Platform Path Handling (CRITICAL)

> The current branch fixes a class of bugs in this area. Treat path code as high-risk.

- [ ] No string-level path comparison (`==`, `starts_with`) without normalization.
- [ ] Symlink resolution: `canonicalize` vs `read_link` chosen consciously; symlink loops handled.
- [ ] Windows-specific: `\` vs `/`, drive letters, case-insensitive FS, UNC paths (`\\?\`), long paths (>260 chars).
- [ ] No assumption that `Path` round-trips through `String` (non-UTF-8 paths exist).
- [ ] `relative_path.join(absolute)` semantics understood (absolute wins on Unix; drive-relative on Windows).

### tree-sitter Grammar / ABI Compatibility

- [ ] Grammar version pinned (`=x.y.z` or `~x.y`) — see the T-SQL incompatibility comment in `Cargo.toml`.
- [ ] If updating `tree-sitter` core: verify all enabled `tree-sitter-*` grammars support the new ABI.
- [ ] New grammar requires updating `Cargo.toml` features AND code paths gating it.

### Feature Flag Combinations (`lang-*`)

- [ ] Builds with `--no-default-features` succeed.
- [ ] Builds with each individual `lang-*` feature in isolation succeed.
- [ ] No code path assumes a non-default feature is enabled.
- [ ] New language added: feature flag + dep + parser registration + tests gated by `#[cfg(feature = "lang-X")]`.

### File-System Watcher Races (`notify`)

- [ ] No race between watcher event and re-index (delete-during-read, write-during-parse).
- [ ] Bounded event queue — bursts of saves don't OOM the process.
- [ ] Watcher restart behavior after error documented.

### `mimalloc` Global Allocator

- [ ] No assumption about allocator behavior (zero-init, deterministic addresses).
- [ ] Tests don't measure allocation patterns that mimalloc changes vs system allocator.

### `build.rs` Determinism

- [ ] No network access at build time.
- [ ] No reliance on environment variables that vary across machines.
- [ ] Output deterministic (no timestamps, no random).
- [ ] Cache invalidation correct (proper `cargo:rerun-if-changed`).

### CLI Output Contract

- [ ] Stdout reserved for machine-readable output; logs go to stderr.
- [ ] Exit codes: 0 success, non-zero failure; new exit codes documented.
- [ ] JSON schema additions backward-compatible (additive).

### MCP Protocol Compliance

- [ ] JSON-RPC envelope unchanged.
- [ ] Tool schema additions backward-compatible (no required new fields).
- [ ] Response size limits respected (LLM context budgets).

---

## Part 5: Severity Model

| Level | Definition | Merge Impact |
|---|---|---|
| **BLOCKER** | Soundness hole, data corruption (incl. on-disk index), security vulnerability, silent data loss, UB | Cannot merge |
| **MAJOR** | Correctness bug, contract violation, regression risk, missing test for changed behavior, breaking change without migration note | Must fix before merge |
| **MINOR** | Suboptimal but safe — non-hot-path perf, missing edge-case test, weak error message | Should fix |
| **NIT** | Style, naming, readability. Max 3 per review. **Omit entirely when any BLOCKER/MAJOR exists.** | Optional |

**Rule:** every BLOCKER/MAJOR must name the **concrete failure mode** and **affected scope**. "Could be a problem" is not a justification.

**Anti-inflation rule:** do not escalate by pattern alone. Pattern presence is a signal to investigate, not an automatic severity. Escalate only if you can explain (a) concrete failure mode, (b) affected scope, (c) why existing invariants/tests don't already prevent it.

---

## Part 6: Deep-Dive Patterns

### 6.1 Idempotency Breaking Change

Operation goes from "success on duplicate" to "error on duplicate". **MAJOR** — any client with retry logic fails on retry. Require "Breaking Changes" in PR description.

### 6.2 Error Type / Contract Stability

Error enum variants renamed/removed/restructured. **MAJOR** — `match` consumers break silently. Prefer adding variants; use `#[non_exhaustive]`.

### 6.3 Fallback / Default Behavior Contract

| Scenario | Severity |
|---|---|
| Silent empty return masking misconfig (no log/metric) | MAJOR |
| Empty return with throttled warning, when empty is valid | MINOR |
| Fallback is expected mode with `fallback_used` metric | OK |

Key criterion: can operators distinguish "no data" from "misconfiguration"?

### 6.4 Behavioral Change Impact on Callers

Function changes return cardinality (empty ↔ non-empty). **MAJOR** — callers with `is_empty()` branches silently change behavior. Find all callers; verify empty-handling.

### 6.5 Test Code Quality

| Issue | Severity |
|---|---|
| No assertions / meaningless asserts | MAJOR |
| Always-pass tests masking regressions | MAJOR |
| Flaky patterns (sleep, time, shared state) | MAJOR |
| Tests removed without justification | MAJOR |
| Missing edge-case / error-path coverage | MINOR |

**Expected test types by change category:**

| Change | Expected |
|---|---|
| Parser / serialization / codec | Property tests (`proptest`), round-trip, malformed input |
| Public API change | Integration test from consumer perspective |
| Bug fix | Regression test reproducing the bug |
| Performance change | `criterion` bench proving the improvement |
| Error handling change | Test per error variant + propagation chain |
| CLI change | Integration test invoking actual process; verify stdout/stderr/exit code |
| On-disk format change | Round-trip test + backward-compat test against fixture file |

### 6.6 Downstream Contract Change

Implementation of a function calls different downstream functions / arguments / order. **MAJOR** — callers may depend on specific side effects. Use `xray_callers direction='down'` to map the call tree before vs after.

---

## Part 7: Review Discipline

### DO

- Be specific and verifiable.
- Cite tool results as evidence.
- Mark assumptions explicitly.
- Read full bodies for ownership/lifetime/contract analysis.
- Prioritize by severity and production impact.

### DON'T

- Suggest cosmetic changes without justification.
- Duplicate the same issue across severity levels.
- Use vague language ("might", "generally OK").
- Claim repo-wide safety without repo-wide evidence.
- Escalate severity by pattern alone.
- Add filler — say "none found" if clean.
- Invent findings to look thorough.

### DISTINGUISH

- **Defect** — incorrect / unsound / violates documented invariant. Always report.
- **Trade-off** — design choice with known drawbacks; the alternative also has drawbacks. Note as trade-off only if the current choice is clearly suboptimal here. Do not flag as defect.
- **Preference** — a different but equally valid way. Do NOT include.

### PERMIT (justified deviations)

- `.clone()` avoiding worse lifetime / architectural complexity — note as trade-off, not defect.
- `.expect("invariant: ...")` where the invariant is type-guaranteed or test-covered.
- `unwrap()` in `main()`, CLI startup, or test code.
- Documented deviation when the alternative is worse for this specific case.
- Simpler over "more correct" when the difference has no production impact.

> **Anti-dogmatism rule:** for each finding ask "Does the rationale behind this rule actually apply here?" If not — the rule does not apply.

---

## Part 8: Pre-Completion Checklist

- [ ] Project Profile applied (Part 0.5) — irrelevant sections explicitly skipped
- [ ] Project-Specific Hazards (Part 4.X) checked when applicable
- [ ] Invariant preservation checked for each modified type/function
- [ ] Every BLOCKER/MAJOR has concrete failure mode + affected scope
- [ ] Callers searched (incl. `xray_callers` ↔ `xray_grep` cross-check) when public/shared surface modified
- [ ] Test Coverage decision tree applied to every behavioral change
- [ ] PR description audit — if breaking changes detected (error variants, signatures, derive removal, visibility narrowing, feature removal, behavioral contract change, on-disk format change), PR description must contain a "Breaking Changes" / "Migration notes" section. Missing → MAJOR.
- [ ] Each finding has the mandatory fields from Part 9 Finding Format

---

## Part 9: Output Template

### Output Scaling

- **Small** (<5 files, <200 lines): Verdict + Findings only. Skip sections 3–4.
- **Medium** (5–20 files): full template; skip sections marked N/A.
- **Large** (>20 files OR >1500 lines): full template + explicit scope statement (Diff Budget Rule, Part 2).

### Finding Format (mandatory fields)

```
[BLOCKER|MAJOR|MINOR|NIT] <short title>
Where:                    <file:line(s)>
Concrete failure mode:    <what breaks, in 1 sentence>
Affected scope:           <function | module | public API | on-disk index | CLI contract | ...>
Evidence tier:            Verified | Inferred | Unverified
Evidence:                 <tool result citation or 1–10 lines of code>
Why existing invariants/tests don't already prevent this:
                          <1 sentence>
Recommendation:           <what to change>
```

### Template

```markdown
# Code Review: <BRANCH/PR>

**Date:** <YYYY-MM-DD>
**Profile:** code-xray (sync, single-crate, lib+bin, unpublished)
**Files Changed:** <N>  |  **Lines:** <±X / ±Y>
**Risk Level:** HIGH | MEDIUM | LOW — <criterion that triggered it>
**Scope:** Reviewed all changed files | Sampled per Diff Budget Rule (N of M, by priority X)

---

## 1. Verdict

**Assessment:** APPROVE | APPROVE WITH CHANGES | REQUEST CHANGES
**Confidence:** HIGH | MEDIUM | LOW
**Evidence Coverage:** FULL | PARTIAL | DIFF-ONLY
**Reason:** <1–2 sentences>

### Coverage Gaps
<dynamic dispatch / macro-generated / proc-macro / build.rs / unread files — or "None">

### Questions to Author
<specific evidence requested — or "None">

---

## 2. Findings

### BLOCKER
<list using Finding Format above — or "None">

### MAJOR
<list — or "None">

### MINOR
<brief one-line per item — or "None">

### NIT  *(omit this subsection entirely when any BLOCKER/MAJOR exists; max 3 items)*
<brief one-liner per item>

---

## 3. Architectural Note  *(non-trivial PRs only — see Part 4 criteria; otherwise omit)*
<2–4 sentences on cohesion / coupling / dependency direction>

---

## 4. Open Questions  *(only if genuinely uncertain; otherwise omit)*
<items that would change the assessment if resolved>
```

---

_Changelog moved to [`rust-expert-code-review-prompt.CHANGELOG.md`](./rust-expert-code-review-prompt.CHANGELOG.md). When using this prompt with an LLM, the changelog file does not need to be loaded._
