---
applyTo: "**"
description: "Use independent Xray reviewers for implemented changes and substantial reviews without redundant delegation."
---

# Separate review agent

- Use a separate reviewer before finalizing changes implemented by the coordinator,
  or a substantial code review involving multiple controlling bodies, shared
  contracts, persistence, installer behavior, or destructive-state risks.
- A small read-only review whose decisive evidence fits one direct read may be
  handled directly unless the user requests an independent review.
- A review already performed by the appropriate specialist satisfies this rule;
  do not launch another agent merely to verify that reviewer.
- Use `xray-script-reviewer` for installer, setup, Git-filter, shell changes, and
  their documentation. Use `xray-code-reviewer` for Rust, Cargo, MCP/CLI, other
  documentation/configuration, and CI changes.
- For mixed surfaces, give each reviewer a disjoint applicable scope. Wait for
  results and include findings and verdicts in the final response. Reuse unchanged
  coverage; repeat only after relevant changes or new evidence.
- This requirement applies to the coordinating assistant. Reviewer agents must
  not start another reviewer for the same task.
