---
name: xray-review-playbook
description: "Review procedures for Xray Rust, installer, and git-filter diffs. Use with xray-code-reviewer or xray-script-reviewer for evidence, validation selection, and verdicts. Not for ordinary code lookup, implementation, or general writing."
user-invocable: false
---

# Xray Review Playbook

Review the requested change for concrete defects and regressions. These notes
guide review; they do not authorize edits, installs, commits, or wider work.
The calling agent's tool and execution restrictions still apply.

## Load Only Relevant References

- Rust source, tests, Cargo, or MCP/CLI changes: [Rust hazards](./rust-hazards.md).
- Installer, filters, or shell tests: [Script hazards](./script-hazards.md).
- After identifying the changed surface: [Validation matrix](./validation-matrix.md).
- Before reporting: [Output format](./output-format.md).

Check repository facts against the current checkout when they affect a finding.
Do not treat remembered versions, suite counts, or historical incidents as proof.
If a required reference is unavailable, report the specific coverage gap.

## Evidence

1. Acquire repository state, diff/stat, and the target revision yourself. Reconcile
   supplied patches with that diff. Include staged changes when requested; name
   relevant untracked files, which ordinary `git diff` does not include. Record
   the exact diff source, HEAD/base, and selected paths. If the requested target
   is ambiguous, report a scope blocker instead of choosing an empty or wider diff.
2. Read changed controlling bodies and their direct consumers. Trace only the
   additional paths needed to resolve a concrete risk or a requested question.
3. When `xray_callers` is available, cross-check an empty result with scoped
   `xray_grep`. Trait objects, local variables, closures, and macros can hide
   callers from the AST index.
4. Check caller-side validation before claiming a callee accepts invalid input.
5. Distinguish inspected facts, inferences, and unknowns. Partial, stale, truncated,
   or revision-mismatched evidence cannot support an exhaustive safety claim.

Scope checks concern the affected contract, not the whole product on every review.
For a changed invariant, identify applicable public entry points, runtime modes,
consumers, and user-visible postconditions independently of the supplied tests.
Take an adjacent adversarial case when it could falsify the proposed fix.
An unrelated pre-existing problem belongs outside the active findings.

## Review Modes

Use FULL for the first pass or when previous coverage cannot be established.
Use FOLLOW-UP when prior findings, reviewed scope/revision, and one consolidated
fix set are supplied. Reacquire the diff in either mode.

- FULL: inspect the changed surface and direct consumers, select relevant hazards,
  then run or reuse risk-scoped validation.
- FOLLOW-UP: resolve prior findings together; inspect fixed bodies and consumers
  of the changed invariant, including one adjacent mode for a correctness fix.
  Reuse unchanged, verified coverage and matching validation evidence.
- Expand to FULL when scope or shared contracts change, the earlier review was
  partial, or the prior reviewed state cannot be reconciled. A validation-hash
  mismatch alone requires fresh validation, not rereading unrelated code.
- Batch fixes and follow-up reviews. SHIP-WITH-NITS does not require another pass;
  optional nit fixes get at most one consolidated follow-up unless they change behavior.

Stop when the requested surface is covered, actionable findings are supported,
and applicable checks have results or explicit blockers. Do not repeat checks for
reassurance. For a large diff (roughly 1,500 changed lines or 30 files), prioritize
public contracts, persistence, safety, core logic, then tests/docs. If coverage
must be sampled, list unread files and report PARTIAL rather than implying completion.

## Findings Versus Gaps

- A finding needs a concrete failure mode, affected scenario, and evidence.
- A missing test is a finding only when tied to changed behavior and a concrete
  regression it would fail to catch. Otherwise record it as a coverage gap.
- Missing tools or runtime access are validation gaps, not invented code defects.
  Required gates can still block SHIP; identify the gate and why it is required.
- Test a regression test's strength through counterfactual analysis: would it fail
  if the guard were inverted or the fallback used? Never edit code to mutate it.
- Do not increase severity to compensate for uncertainty or to appear thorough.
- Use the output reference's `Block reasons` field to separate defects, unavailable
  validation, and unresolved scope. A missing tool does not authorize installation,
  weaker gates, code changes, or repeated identical review passes.