# Project Rules

Talk to user in Russian if not specified otherwise.

## Post-Change Checklist

After every code change, before completing the task, verify ALL of the following:

1. **Unit tests** — the change has test(s) covering the new/modified behavior
2. **All unit tests pass** — run `cargo test --bin search-index` and confirm 0 failures
3. **Ask user to stop MCP server** — before reinstalling the binary, ask the user to stop the MCP server (restart VS Code or stop the search-index server)
4. **Reinstall binary** — `cargo install --path . --force`
5. **Run E2E tests** — after the binary is installed, run `.\e2e-test.ps1` and confirm 0 failures
6. **Self-review for hidden bugs** — BEFORE documenting, critically review ALL changes in this branch:
   - Re-read every modified function. Ask: "Are there code paths where this change is NOT applied consistently?" (e.g., adding a field to one summary builder but missing 5 others)
   - Ask: "Does the new code have a different semantic than the old code in edge cases?" (e.g., exact match vs substring match returning different result sets)
   - Ask: "Are there performance regressions?" (e.g., O(1) HashMap lookup replaced with O(N) scan)
   - Ask: "What tests are MISSING?" — not just "do existing tests pass" but "what NEW behavior is untested?"
   - If bugs found — fix them, **add regression tests for each bug found**, re-run ALL tests, then continue to documentation steps
   - **RECURSIVE**: after fixing bugs found during self-review, review the FIXES themselves for new bugs. Also review the broader context where the fixes run. Repeat until 100% confident no bugs remain. This is not optional — bugs in bugfixes are real and common.
   - **Tests at every round**: each bug found during recursive review needs its own regression test — not just the first round. If round 2 finds a bug in round 1's fix, add a test for that too. The test count grows with each round until the code is clean.
7. **E2E test plan** — `docs/e2e-test-plan.md` is updated with test scenarios for the change
8. **E2E test script** — evaluate whether `e2e-test.ps1` should also get new test cases for the change (CLI-testable scenarios). If yes — add them
9. **CLI & MCP discoverability** — for every new feature:
  - CLI: verify `--help` output includes the new flag/command (check `src/cli/args.rs`)
  - MCP: verify tool descriptions in `src/tips.rs` include the new parameter/tool
  - LLM instructions: verify `search_help` output covers the new capability
  - Docs: update `docs/mcp-guide.md` (parameter tables, examples) and `docs/cli-reference.md` if applicable
  - Principle: keep LLM instructions concise — add only what helps tool selection, not exhaustive docs
10. **Documentation** — `README.md` and the rest relevant GIT-tracked documents are updated
11. **Changelog** — `CHANGELOG.md` is updated with a concise entry describing the change (categorized as Features, Bug Fixes, Performance, or Internal)
12. **Neutral names** — all class/method names in docs, tests, and tool descriptions are generic (e.g., `UserService`, `OrderProcessor`) — never expose internal/proprietary names

**⚠️ Documentation gate before proposing commit** — do NOT propose creating a branch until ALL of the following are verified for every new parameter/feature:

| # | File | What to update | When |
|---|------|----------------|------|
| 1 | `src/mcp/handlers/mod.rs` | Tool schema (`inputSchema` properties) | Always for new MCP parameters |
| 2 | `src/tips.rs` | `parameter_examples()` entries | Always for new MCP parameters |
| 3 | `docs/mcp-guide.md` | Parameter tables + response fields | Always for new MCP parameters |
| 4 | `docs/cli-reference.md` | CLI flag documentation | Only if feature has CLI flags |
| 5 | `docs/e2e-test-plan.md` | Test scenario | Always |
| 6 | `CHANGELOG.md` | Feature/bugfix entry | Always |
| 7 | `README.md` | Feature mention if user-facing | If it's a major feature |

This table is a hard gate — every row must be checked before the commit proposal.

## Git Workflow — After All Tests Pass

After all tests pass and the binary is reinstalled, propose creating a branch and committing:

1. **Ask user** — "Would you like to create a branch and commit these changes?"
2. If yes:
   - Check current branch with `git rev-parse --abbrev-ref HEAD`
   - If on `main`: run `git pull` then `git checkout -b <branch-name>`
   - If NOT on `main`: run `git stash`, `git checkout main`, `git pull`, `git checkout -b <branch-name>`, `git stash pop`
   - Branch name format: `users/<user-alias>/<feature-name>`
3. **Product name check** — run `powershell -File scripts/check-product-names.ps1` and confirm the output says "No product-specific names found". If any product-specific names are reported, stop and make them neutral before proceeding.
4. **Stage tracked changes only** — `git add -u` (never auto-add untracked files)
5. **Prepare commit message** — write a concise commit title
6. **Prepare PR description** — write a detailed description of all changes in Markdown format
7. **Write PR description to file** — save the PR description to `docs/pr-description.md` so the user can copy it easily (this file is NOT tracked in git — it's a temp artifact)
8. **Ask user to commit manually** — present the commit title + PR description and let the user do `git commit` themselves

## Environment Rules

- **Windows environment** — this project runs on Windows (cmd / PowerShell). Never use Unix-only commands like `tail`, `head`, `grep`, `sed`, `awk`, `wc`. Use PowerShell equivalents or native Rust/cargo commands instead.
- **E2E tests require pwsh** — `e2e-test.ps1` uses modern PowerShell syntax (e.g., parentheses in strings) that is incompatible with Windows PowerShell 5.1. Always run E2E tests with `pwsh -File .\e2e-test.ps1`, NOT `powershell -File .\e2e-test.ps1`.
- **Testing is mandatory** — every code change MUST include:
  - **Unit tests** covering the new/modified behavior
  - **E2E test plan update** (`docs/e2e-test-plan.md`) with a test scenario for the change
  - **E2E test script update** (`e2e-test.ps1`) if the change is CLI-testable
- **Never skip tests** — even for "internal" optimizations or refactors. If the behavior is testable, add tests.

## Git Rules

- **Tracked files only** — when committing to branches (via `commit_and_push`, `git add`, or any other tool), always stage only tracked (modified) files. Never auto-add untracked files. Use `git add -u` / `includeUntrackedFiles: false`. Untracked files must be added explicitly by the user.

## MCP Tool Design Rules

- **⚠️ NO new combo tools** — never create a new MCP tool that internally calls multiple existing tools (e.g., a `search_blast_radius` that combines `search_callers` + `search_grep` + `search_git_authors`). Each new tool increases the tool selection burden on the LLM. Currently we have 14 tools — at 20+ the LLM starts to degrade in tool selection accuracy.
- **Extend existing tools with parameters** — if a feature combines data from multiple indexes, add it as an optional parameter to the most relevant existing tool. Example: `crossServiceScan: true` in `search_callers` (internally calls `search_grep`) is correct — the LLM doesn't need to choose a new tool, just add a flag.
- **Before implementing a new tool, ask**: "Can this be a parameter on an existing tool?" If yes — do that instead.
- **If a new tool IS genuinely needed** (new data source, fundamentally different operation), keep it atomic — one index, one data source, one concern. Examples of correct atomic tools: `search_grep` (content index), `search_definitions` (definition index), `search_git_blame` (git CLI).
- **Tool count budget**: aim to stay under 16 tools total. Every tool beyond that should have a strong justification.

## User Story Convention

- **Approved user stories** are saved as `docs/todo_approved_{YYYY-MM-DD}_{feature-name}.md`
- Format: `todo_approved_{date}_{kebab-case-feature-name}.md`
- Example: `docs/todo_approved_2026-02-28_override-caller-tracking.md`
- Language: Russian (unless explicitly requested otherwise)
- Must include: problem description, solution approach, implementation plan with code sketches, acceptance criteria

## MCP Transcript Analysis Workflow

The project includes a transcript analyzer script (`scripts/analyze-transcript.ps1`) that evaluates MCP tool quality from exported session logs.

### When to use

**If the user pastes a path to an `.md` file without any instructions** (especially from Downloads, which looks like an exported Roo Code session log `roo_task_*.md`), **do NOT read it directly** — it can be 100KB+. Instead, ask the user: "Это экспорт сессии Roo Code. Запустить скрипт анализа (`analyze-transcript.ps1`), или ты хочешь чтобы я прочитал файл целиком?"

When the user confirms analysis (or explicitly asks to analyze it):

1. **Run the analyzer**: `pwsh -File scripts/analyze-transcript.ps1 -InputFile <path-to-log.md>`
2. **Read the generated report**: `<path-to-log.md>.report.md` and `<path-to-log.md>.report.json`
3. **Analyze the results**:
   - Review the **Tool Quality Scorecard** — which tools have low utilization_rate, high truncation, many refinement chains?
   - Review **Automated Recommendations** — what does the script suggest?
   - Review **Truncation Root Causes** — what parameters cause truncation?
   - Review **Efficiency** — how many KB wasted on truncated-then-refined responses?
   - Review **Model Self-Analysis** (if present) — what did the model say about tool quality?
4. **Assess action items**: Are there real problems with search-index code, or is this just expected behavior?
5. **If action items exist** — create a user story in Russian at `docs/user-stories/todo_approved_{date}_{feature-name}.md` with:
   - Concrete examples from the analyzed episodes (params, param_diff, thinking_before, reaction_after)
   - Quantitative metrics (truncation rate, wasted bytes, refinement chains)
   - Proposed solutions with code-level implementation hints
   - Acceptance criteria with measurable targets (re-run same prompt, compare metrics)
6. **Assess the analyzer itself**: Can the script output be improved? Are there false positives/negatives? Missing metrics? If yes — fix the script.

### Script location and usage

```
pwsh -File scripts/analyze-transcript.ps1 -InputFile session.md [-OutputJson report.json] [-OutputMd report.md]
```

Output: JSON + Markdown reports with episodes, tool scorecard, recommendations, truncation root causes, efficiency metrics.

### Related artifacts

- User story for the analyzer: `docs/user-stories/todo_approved_2026-03-12_mcp-transcript-analyzer.md`
- Example improvement story derived from analysis: `docs/user-stories/todo_approved_2026-03-12_search-definitions-ux-improvements.md`

## Lessons Learned

- **Verify facts, don't assume** — always run `git status` / `git log` before stating whether a file is tracked, staged, or committed. Never claim a file's git state from memory — check it.
- **UX consistency across interfaces** — when implementing a feature that exists in one interface (e.g., MCP), ensure the same defaults apply to other interfaces (e.g., CLI). Users expect consistent behavior. If MCP defaults to substring search, CLI should too.
- **Follow the post-change checklist strictly** — do not skip steps or reorder them. The checklist exists to prevent regressions and ensure quality. When in doubt, re-read the checklist.
- **Documentation is a contract** — if docs describe a flag/feature, the code MUST support it. If a doc says `--substring` exists as a CLI flag but the code doesn't have it, that's a bug — either fix the code or fix the docs. Never leave docs and code out of sync.
- **Use PS script files for complex commands** — when a PowerShell command is too complex for inline execution (escaping issues, multi-line, regex with special chars), write it to a `.ps1` file, execute it, then delete the file. Inline PowerShell via `powershell -Command "..."` breaks on colons, backticks, and nested quotes. Script files avoid all escaping issues.
- **Stop MCP server before reinstall** — before running `cargo install --path . --force`, propose running `taskkill /IM search-index.exe /F` to stop any running search-index.exe processes. If the user agrees, run it yourself. Don't ask the user to restart VS Code — just kill the process directly.
- **Always run product name check before staging** — run `scripts/check-product-names.ps1` before `git add -u`. If product-specific names are found in documentation or code, replace them with neutral equivalents before committing. This prevents accidental exposure of internal/proprietary names in the public repository.
- **Feature discoverability across interfaces** — every new feature must be exposed in BOTH CLI help and MCP tool descriptions. If a feature exists in code but isn't in `--help` or tool descriptions, users/LLMs can't discover it. Review `src/cli/args.rs` (CLI), `src/tips.rs` (MCP descriptions), and `search_help` output after every feature addition.
- **New response fields must be documented in tool descriptions** — when a feature adds new fields to the response (e.g., `rootMethod`, `bodyOmitted`), the tool schema description and `search_help` parameter examples must explicitly mention them. LLMs read tool descriptions to decide how to use the tool — if a response field isn't mentioned, the LLM will make a separate call to get that data. Example: `includeBody=true` adds a `rootMethod` object to the response — without documenting this, the LLM would still call `search_definitions` separately to get the root method body.
- **Test before documenting** — the post-change checklist runs unit tests → install → E2E tests BEFORE updating documentation/changelog. Rationale: if E2E tests fail, the code needs fixing, which may invalidate documentation written earlier. Testing first avoids documenting features that don't work yet.
- **Self-review catches what tests don't** — after all tests pass, ALWAYS re-read every modified function before documenting. Real example (2026-02-26): adding `readErrors`/`lossyUtf8Files` to grep.rs — the fields were added to 1 of 6 summary builders, passing all tests because no test checked the OTHER 5 code paths. Self-review caught this. Another: `baseType` fast-path (O(1) HashMap) silently hid substring results when an exact key existed — semantically different from the O(N) substring scan. Both bugs were invisible to the test suite but would have caused inconsistent behavior in production.
- **Documentation checklist step 9 is not optional** — every new parameter/feature MUST be documented in ALL relevant places BEFORE proposing commit. The full list: (1) `src/mcp/handlers/mod.rs` tool schema, (2) `src/tips.rs` parameter examples, (3) `docs/mcp-guide.md` parameter tables + response fields, (4) `docs/cli-reference.md` if CLI-facing, (5) `docs/e2e-test-plan.md` test scenario, (6) `CHANGELOG.md`. Real example (2026-03-01): `includeDocComments` was added to code, tests, tips.rs, and mod.rs schema — but `docs/mcp-guide.md` parameter tables were missed until the user caught it. The checklist item 9 already covers this, but it was skipped in the rush to propose commit. Never skip documentation steps.
- **Bump index format_version when adding fields** — `ContentIndex` and `DefinitionIndex` have `format_version` fields with constants `CONTENT_INDEX_VERSION` (in `src/lib.rs`) and `DEFINITION_INDEX_VERSION` (in `src/definitions/types.rs`). When adding, removing, or reordering fields in either struct, increment the corresponding constant. This ensures old indexes on disk are rejected and rebuilt automatically on next startup. Without bumping, old indexes load with `#[serde(default)]` zeros for new fields — causing silent data corruption. The `format_version` field MUST stay after `root` in the struct (never first) because `read_root_from_index_file()` reads `root` as the first bincode field.
- **Keep policyReminder in every MCP response** — `summary.policyReminder` repeats SEARCH_INDEX_POLICY in every successful JSON response. This is NOT redundant despite being in the system prompt — LLMs are prone to forgetting instructions as context grows. The ~200 chars per response cost (~650 tokens per 13-call session) is justified as reinforcement against tool selection drift. Never remove policyReminder to "save tokens".
- **Wait for MCP server reconnection after reinstall** — after stopping (`taskkill`) and reinstalling the binary (`cargo install`), the MCP server is disconnected. Do NOT attempt to use MCP tools or run CLI commands that bypass the user's consent. Instead, STOP and ask the user to reconnect the MCP server (restart VS Code or restart the MCP server). Only proceed with MCP tool verification after the user confirms the server is back.