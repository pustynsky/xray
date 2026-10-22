---
name: review
description: "Review and fix an explicit Xray diff: route changes to scoped reviewers, validate fixes, and stop on SHIP or a precise scope/validation blocker."
disable-model-invocation: true
argument-hint: "What to review, e.g. current branch, staged diff, PR, or changed files"
---
# Review Xray Until It Can Ship

You are running the local `xray` review loop.

Route Rust, Cargo, MCP/CLI, general repository docs/config, and CI workflows to `xray-code-reviewer`. Route installer, git-filter, and their tests or documentation to `xray-script-reviewer`. Invoke both for a mixed diff. Each reviewer returns `SHIP`, `SHIP-WITH-NITS`, or `BLOCK` for its scope.

## Resolve The Target

Inspect branch/HEAD, staged and unstaged changes, and untracked paths before
making fixes. Use the explicit target when supplied:

- Branch/PR: record the base, merge-base, and head commits. Review the branch
  range, not unrelated local changes.
- Staged: review the index with `git diff --cached`. Tests of a different working
  tree do not validate that snapshot. Ask before staging fixes; do not silently
  change the target to the working tree.
- Working tree: review tracked staged and unstaged changes against `HEAD`.
  Enumerate untracked files separately and include only paths selected by the user
  or clearly named by the request; `git diff` does not contain them.
- Named files: record both the paths and which of the above diff sources applies.

Without an explicit target, use the branch range against `main` only when the
checkout is clean, the branch is not `main`, and it has commits ahead of `main`.
If local changes make the intended target ambiguous, show the candidate scopes
and ask before reviewing or fixing. On a clean checkout with no branch changes,
report that there is no diff. Never treat an empty `main` versus `main` comparison
as approval of local changes.

## Loop

1. Classify the selected files and invoke only the applicable reviewers. Supply
   the target, HEAD/base, diff source, exact paths including selected untracked
   files, Xray discovery requirement, and request the
   [shared output contract](../xray-review-playbook/output-format.md).
2. If every applicable scope is `SHIP` or `SHIP-WITH-NITS`, stop. Nits are
   optional unless the user asks to address them.
3. Handle each `BLOCK` reason separately:
   - `defect`: fix supported blocking findings within the selected scope.
   - `validation`: run the missing check only if authorized and available.
     Otherwise stop with the exact check, affected scope, and required user action.
     Do not install tools, weaken gates, or edit code to hide a validation gap.
   - `scope`: clarify the target or missing coverage before making further fixes.
   A report may contain more than one reason; each must be resolved before SHIP.
4. Batch related fixes, preserve unrelated user changes, and run focused checks
   from the [validation matrix](../xray-review-playbook/validation-matrix.md).
   Fix failures caused by these changes. Record unrelated failures and whether
   they prevent validation of the selected target.
5. Request FOLLOW-UP only for scopes affected by the fixes or new evidence.
   Supply prior findings, reviewed state, consolidated fixes, and the matrix's
   `VALIDATION_EVIDENCE` block with commands, exit codes, HEAD/base, diff source,
   paths, and fingerprint. Include content fingerprints for untracked inputs;
   a tracked diff hash alone cannot identify them.
   For branch/PR fixes, review the original branch range plus this task's
   uncommitted fix overlay, identifying and fingerprinting both sources. Do not
   review the unchanged commit range alone or include unrelated local changes.
6. Retain an unchanged scope's verdict only when its reviewed state and relevant
   shared contracts/validation inputs still match. Otherwise request a fresh review.
7. Repeat only after relevant changes or new evidence. If a pass makes no progress,
   stop and report the unresolved blocker rather than launching the same review again.

## Test Defaults

Prefer the smallest command that covers the changed behavior, then broaden when shared contracts are touched:

```powershell
cargo test --bin xray <specific_test_or_module>
cargo test --bin xray
```

For MCP/tooling changes, include the relevant e2e or handler tests when available.

## Output

When the loop finishes, report:

- Final reviewer verdict
- Files changed by fixes
- Tests run and results
- Remaining risks or user decisions, if any

Do not commit unless the user explicitly asks.
