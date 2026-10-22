---
name: autonomous-fixes
description: "Implement or resume an authorized queue of fixes autonomously: three independent Xray-backed actors agree, implementation and tests pass QA/product acceptance, then independent SHIP review, branch, commit, push, PR, green CI, merge, and verified closure."
argument-hint: "Start or resume an authorized task queue; identify its source and any scope limits."
---

# Autonomous Fix Queue

Use this workflow only when the user authorizes implementing the queue, not for
questions, planning, or configuring these instructions. Proceed autonomously
within the authorized scope. Do not start tasks merely by loading this skill.

## Resume From Reality

1. Read the task source identified by the user, repository instructions, and
   applicable skills. Check the working tree, branch/HEAD, remote main, related issues,
   existing PRs, and CI through Git and `gh`. Fetch full issue descriptions.
   Treat plan revision references and previous session summaries as historical
   evidence, not current state or proof of completion.
2. Preserve unrelated work. Never reset, clean, overwrite, stage, or commit
   someone else's changes. If work overlaps the task, resolve ownership before
   proceeding. Finish or resume an existing task PR rather than duplicating it.
3. Follow the authorized queue's existing order and explicit PR grouping. Default to one
   task/issue per PR; combine issues only where the plan explicitly groups them.
   Every issue in a grouped PR still needs its own diagnosis, acceptance criteria,
   regression evidence, and closure check. Do not expand into deferred tasks.
4. Record task state and dependencies using session tracking. Keep a concise
   resumption record with the current task, base/head, agreed solution, actor
   decisions, validation, reviewer verdict, PR/CI links, and exact blockers.
   Use the existing task source or session state; do not create another tracking
   document. Do not commit local or ignored planning files without explicit
   authorization. This workflow does not depend on a particular filename.

## Three Real Independent Actors

Create fresh, separate agents for each task or explicitly grouped unit:

| Actor | Responsibility |
| --- | --- |
| Programmer / architect | Validate the cause, compare solutions, propose the simplest complete fix, identify affected contracts and deterministic regressions. |
| Critic / QA | Independently challenge the proposal, trace failure modes and edge cases, assess test strength and acceptance criteria. |
| Product | Independently assess user-visible behavior, scope, compatibility, errors, migration/recovery, and whether the proposed fix is sufficient. |

- These must be independent agents, not three simulated voices in one response.
  Do not force disagreement, invent objections, or infer agreement from silence.
- Explicitly require **Xray** in every actor prompt for workspace code discovery,
  search, bodies, definitions, and callers. Each actor must inspect relevant code
  itself, not merely endorse another actor's summary. Git/`gh` are for repository
  state, history, issues, PRs, and CI; shell commands may run validation.
- If Xray cannot cover relevant code or has stale evidence, report the limitation
  and resolve it within applicable tool rules. Do not bypass access restrictions
  or pretend the actor inspected unavailable code. Non-code documents not indexed
  by Xray may be read with permitted document tools.
- Use configured agent models; do not invent overrides. Keep the three actors
  available for same-task discussion when the runtime permits follow-ups.
- First obtain the architect's evidence and alternatives, without code edits.
  Then give that proposal to critic/QA and product; their assessments may run in
  parallel. Share supported objections and revise the proposal with the actors.
- Require explicit acceptance by **all three** of the same identified solution
  revision and its acceptance tests before implementation. Record disagreements
  and their resolution. Announce the selected solution briefly, then continue
  without asking for routine confirmation.
- Consensus does not replace evidence. If a material disagreement remains after
  a bounded exchange, stop with the unresolved decision instead of choosing a
  majority, fabricating consensus, or repeatedly relaunching the same analysis.

## Implement And Verify

1. Start from an updated clean main. Isolate work at its recorded base, using a
   detached checkout or an isolated detached worktree so main is not committed to.
   Do not create the publication branch until the independent review is SHIP.
   On resume, preserve and inspect an existing task branch instead of recreating it.
2. Add deterministic regressions and show an actual baseline failure where
   feasible. A compile failure or zero selected tests is not a reproduced defect.
   Test the required result or measurable threshold, not a proxy for it.
3. Implement the agreed narrow fix and directly related documentation/CHANGELOG.
   Preserve valid empty results, negative controls, public behavior, errors,
   cache/API compatibility, and failure recovery unless a change was agreed.
4. Run the smallest existing tests covering the change, then required broader
   checks for shared contracts and affected features/platforms. Use the current
   review playbook and repository gates. Never run `cargo fmt` or `rustfmt`,
   including check modes, even if an old plan lists those commands.
5. Record exact commands, selected/passed counts, exit codes, red/green evidence,
   and unverified platforms. Surface errors; do not skip a failing test, weaken
   an assertion, or silently treat incomplete validation as success.
6. If implementation or evidence materially changes the agreed behavior, return
   that decision to all three actors before proceeding.

## Post-Implementation Acceptance

Before independent code review, return the actual patch and validation evidence
to the same critic/QA and product actors when possible. This is a short acceptance
pass, not another full design discussion or a replacement for code review:

- Critic/QA checks the implemented acceptance criteria, red/green regressions,
  negative controls, real public entry points, and error/recovery behavior.
  Independently run focused checks when needed; distinguish executed tests from
  supplied revision-matched results. Missing required evidence is not acceptance.
- Product checks the actual user-visible behavior, compatibility, errors, and
  documentation against the agreed contract. Reread affected code through Xray
  where necessary, without repeating unrelated investigation.
- Require explicit acceptance from both for the identified patch. If changes are
  needed, fix them and repeat only affected acceptance. Material contract changes
  return to all three actors; unchanged behavior needs no new design round.
- If the runtime cannot resume the original actors, give fresh independent
  replacements their roles, agreement record, exact patch and evidence. Do not
  substitute the implementer or coordinator for either acceptance role.
- Later reviewer or CI fixes invalidate acceptance only for affected behavior or
  evidence; repeat those checks and retain unchanged coverage.

## Independent Review Before Publication

- Use a **different, read-only reviewer agent**, not an actor or implementer.
  Use `xray-code-reviewer` for Rust/Cargo/MCP/CLI/general docs/config/CI and
  `xray-script-reviewer` for installer/setup/Git-filter/shell and related docs.
  For mixed surfaces, give them disjoint scopes and require both approvals.
- Require Xray code investigation, the current
  [review playbook](../xray-review-playbook/SKILL.md), the exact diff source,
  base/head, sorted paths including relevant untracked inputs, diff/content
  fingerprints, and revision-matched `VALIDATION_EVIDENCE`.
- Require explicit **SHIP** before publication. Do not silently promote
  SHIP-WITH-NITS, BLOCK, partial coverage, or a missing reviewer into SHIP.
  Resolve supported comments in a consolidated fix, run affected checks, and
  request scoped follow-up. Reuse unchanged verified coverage; never launch a
  second reviewer merely to recheck another reviewer's approval.
- Do not loop for cosmetic reassurance. If SHIP requires an unresolved external
  decision, unavailable validation, or unrelated changes, report the blocker.
  Reviewer unavailability is not permission to self-approve or change its model.

## Branch, PR, CI, Merge

1. After SHIP, create a new task branch carrying only the reviewed changes from
   the recorded base. Stage named related files, not `git add -A`. Check that the
   staged content is the reviewed snapshot. Commit with the required co-author
   trailer, push, and open a separate PR with `Fixes #NNN` for every related issue.
   Include the agreed behavior, tests, review verdict, and remaining limitations.
2. Wait for CI on the **exact latest head**, including required final gates and
   affected OS/feature jobs. No merge with failed, pending, missing, or unresolved
   checks. Do not use admin bypass, remove checks, or relabel a failure as a flake.
3. Diagnose CI failures, fix task-related causes, rerun affected validation, and
   obtain scoped review of relevant changes before pushing follow-up commits.
   Preserve original review coverage for unchanged surfaces. If fixing CI changes
   the agreed contract, consult the three actors again.
4. If main changes, integrate it without losing either side's work. Revalidate
   affected contracts and review any new task changes; require green CI for the
   resulting head. Do not force-push, amend, or use destructive Git shortcuts.
5. Merge only the verified head using normal repository policy. Independently
   check PR=MERGED, every related issue=CLOSED, and merge ancestry on updated
   local main. If someone else merges, verify final CI too; do not claim that an
   external early merge satisfied the workflow's gate.
6. Mark the task unit complete only after its acceptance criteria and closure
   checks pass; record PR/evidence using existing conventions. Clean up only
   recorded task fixtures, caches, and processes. Preserve unrelated changes.
   Update main and verify a clean task worktree before starting the next unit.

## Autonomy And Stopping

Routine investigation, implementation, validation, commits, pushes, PR creation,
CI fixes, and normal merges within this authorized queue need no extra approval.
Continue to the next unit only after the previous one is verified complete.
Stop on real access/tooling blockers, unresolved consensus, conflicting user
changes, unavailable required validation, or decisions outside the authorized
scope. Report the exact blocker; do not skip the task or invent a fallback.

After interruption, reacquire live state and resume the unfinished phase; do not
trust old agent IDs or old validation for a changed patch. Finish with a table of
task/issues, PR, tests/review/CI, merge/closure status, and genuine limitations.
