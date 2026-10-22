---
applyTo: "**"
description: "Use when implementing or resuming an authorized queue of fixes autonomously with three independent actors, Xray analysis, acceptance, SHIP review, CI, and merge gates."
---

# Autonomous Fix Workflow

When the user authorizes autonomous implementation of an ordered task queue, load
and follow [autonomous-fixes](../skills/autonomous-fixes/SKILL.md) before starting.
Apply it throughout that queue, including after context compaction or resumption.
It does not authorize starting the queue when the user only asks to discuss,
plan, or configure the workflow. Unrelated tasks do not require its actors.
