# Project Rules

## Post-Change Checklist

After every code change, before completing the task, verify ALL of the following:

1. **Unit tests** — the change has test(s) covering the new/modified behavior
2. **E2E test plan** — `docs/e2e-test-plan.md` is updated with test scenarios for the change
3. **E2E test script** — evaluate whether `e2e-test.ps1` should also get new test cases for the change (CLI-testable scenarios). If yes — add them
4. **CLI & MCP discoverability** — for every new feature:
  - CLI: verify `--help` output includes the new flag/command (check `src/cli/args.rs`)
  - MCP: verify tool descriptions in `src/tips.rs` include the new parameter/tool
  - LLM instructions: verify `search_help` output covers the new capability
  - Principle: keep LLM instructions concise — add only what helps tool selection, not exhaustive docs
5. **Documentation** — `README.md` and the rest relevant GIT-tracked documents are updated
6. **Changelog** — `CHANGELOG.md` is updated with a concise entry describing the change (categorized as Features, Bug Fixes, Performance, or Internal)
7. **Neutral names** — all class/method names in docs, tests, and tool descriptions are generic (e.g., `UserService`, `OrderProcessor`) — never expose internal/proprietary names
8. **All tests pass** — run `cargo test --bin search-index` and confirm 0 failures
9. **Ask user to stop MCP server** — before reinstalling the binary, ask the user to stop the MCP server (restart VS Code or stop the search-index server)
10. **Reinstall binary** — `cargo install --path . --force`
11. **Run E2E tests** — after the binary is installed, run `.\e2e-test.ps1` and confirm 0 failures

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

## Lessons Learned

- **Verify facts, don't assume** — always run `git status` / `git log` before stating whether a file is tracked, staged, or committed. Never claim a file's git state from memory — check it.
- **UX consistency across interfaces** — when implementing a feature that exists in one interface (e.g., MCP), ensure the same defaults apply to other interfaces (e.g., CLI). Users expect consistent behavior. If MCP defaults to substring search, CLI should too.
- **Follow the post-change checklist strictly** — do not skip steps or reorder them. The checklist exists to prevent regressions and ensure quality. When in doubt, re-read the checklist.
- **Documentation is a contract** — if docs describe a flag/feature, the code MUST support it. If a doc says `--substring` exists as a CLI flag but the code doesn't have it, that's a bug — either fix the code or fix the docs. Never leave docs and code out of sync.
- **Use PS script files for complex commands** — when a PowerShell command is too complex for inline execution (escaping issues, multi-line, regex with special chars), write it to a `.ps1` file, execute it, then delete the file. Inline PowerShell via `powershell -Command "..."` breaks on colons, backticks, and nested quotes. Script files avoid all escaping issues.
- **Stop MCP server before reinstall** — before running `cargo install --path . --force`, propose running `taskkill /IM search-index.exe /F` to stop any running search-index.exe processes. If the user agrees, run it yourself. Don't ask the user to restart VS Code — just kill the process directly.
- **Always run product name check before staging** — run `scripts/check-product-names.ps1` before `git add -u`. If product-specific names are found in documentation or code, replace them with neutral equivalents before committing. This prevents accidental exposure of internal/proprietary names in the public repository.
- **Feature discoverability across interfaces** — every new feature must be exposed in BOTH CLI help and MCP tool descriptions. If a feature exists in code but isn't in `--help` or tool descriptions, users/LLMs can't discover it. Review `src/cli/args.rs` (CLI), `src/tips.rs` (MCP descriptions), and `search_help` output after every feature addition.