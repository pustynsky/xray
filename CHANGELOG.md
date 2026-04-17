# Changelog

### Features

- **XML on-demand parsing — Phase 2 hardening and extension set expansion** (code review: `docs/code-reviews/review-2026-04-17-xml-on-demand.md`) — Followed up on the Phase 1 on-demand XML parser with a full Phase 2 round of structural hardening and documentation. Walker rewrite in `src/definitions/parser_xml.rs`: (1) **WALKER-1** — ancestry tracking moved from per-level `Vec<String>` clone to a persistent push/pop stack inside a new `WalkCtx` struct; signature materialization now clones the stack once per emitted definition instead of once per tree level. (2) **WALKER-2** — when tree-sitter-xml cannot recover an element name from a malformed `element` node (unterminated tag, junk before opening angle bracket), the walker now records a `parseWarnings` entry and still recurses into children, so nested well-formed elements are no longer silently dropped. (3) **WALKER-3** — new `MAX_RECURSION_DEPTH = 1024` tripwire with a well-formed warning message (`"XML nesting exceeded N levels; subtree at line L truncated."`) protects against adversarial deeply-nested inputs without stack-overflow. (4) **WALKER-7** — `extract_text_content` now early-exits on the first child element/EmptyElemTag, so block elements no longer pay the cost of a full CharData sweep. Non-fatal issues are surfaced via a new `ParseResult { definitions, warnings }` struct and the `parse_xml_on_demand_with_warnings()` entry point (the old `parse_xml_on_demand() -> Vec<XmlDefinition>` is preserved for backward compatibility). Handler refactor: `handle_xml_name_filter` (~270 lines) decomposed into `classify_matches`, `build_result_buckets`, `assemble_promoted_results`, and `attach_body` for readability; a new `SourceLines` helper caches the `source.lines()` iterator (`MINOR-4`) so multiple body extractions inside a single request no longer re-split the file. All XML-specific handler logic (~550 lines) extracted from `src/mcp/handlers/definitions.rs` into a new dedicated module `src/mcp/handlers/xml_on_demand.rs` with four entry points (`try_intercept`, `handle_contains_line`, `handle_name_filter`, `hint_for_xml_extension`). Every response now includes a `summary.parseWarnings` array so LLM callers can detect and surface "results may be incomplete". **Extension set expanded** beyond the Phase 1 defaults: added `vcxproj`, `vbproj`, `fsproj`, `nuspec`, `vsixmanifest`, `appxmanifest` (the full list is now `xml, config, csproj, vbproj, fsproj, vcxproj, props, targets, nuspec, vsixmanifest, manifestxml, appxmanifest, resx`). Documentation: new `### XML On-Demand Parsing` section in `docs/mcp-guide.md` under `xray_definitions` documenting supported extensions, parameter behavior, response shape, limitations (entity escapes NOT decoded, CDATA preserved verbatim, namespaces kept as literal prefixes, 1024-level depth cap, whole-file memory model), and "when to prefer xray_grep" guidance. 8 new regression tests in `src/definitions/definitions_tests_xml.rs` (`test_walker_malformed_recursion`, `test_warnings_empty_on_wellformed`, `test_deeply_nested_no_stack_overflow`, and 5 tests for the new extensions). All 1827 unit tests + 68 E2E tests pass.

### Internal

- **Track `Cargo.lock` in git for reproducible binary builds** — Removed `Cargo.lock` from `.gitignore` and committed the current lockfile. **Rationale**: the Cargo Book explicitly recommends tracking `Cargo.lock` for **binary** crates (executables, CLI tools, servers) to guarantee reproducible builds — every developer, CI job, and `cargo install` consumer resolves the exact same transitive dependency graph. The previous setup (lockfile ignored) followed the **library** pattern, which is incorrect for xray — xray is a binary crate shipped as `xray.exe` (MCP server + CLI). Consequences of the old setup that are now fixed: (1) different contributors could build with different transitive dependency versions leading to "works on my machine" drift; (2) `cargo audit` on CI would run against whatever `cargo update` resolved at job time rather than the exact versions any given contributor tested; (3) security-relevant dependency bumps (e.g., `lz4_flex 0.11.5` → `0.11.6` below) could not be propagated through the repo — they existed only in each contributor's local lockfile. The lockfile will change frequently via routine `cargo update` / `cargo add`; that is expected for binary crates and the noise is a fair cost for reproducibility.

- **`cargo audit` security pass for tree-sitter-xml 0.7** — Ran `cargo audit` as part of the XML on-demand hardening review to verify the new dependency does not introduce vulnerabilities. Result: **zero** new advisories from tree-sitter-xml 0.7 or its transitive tree. Pre-existing baseline advisories (addressed by the tracked lockfile above): **RUSTSEC-2026-0041 (HIGH, 8.2)** on `lz4_flex 0.11.5` — **FIXED** in this PR via `cargo update -p lz4_flex` → `0.11.6` (now propagated through the committed `Cargo.lock`); `bincode 1.3.3 unmaintained` (RUSTSEC-2025-0141) and `rand 0.9.2 unsound` (transitive via proptest, RUSTSEC-2026-0097) remain tracked for a future dependency-maintenance pass — these need upstream crate updates or a `Cargo.toml` version bump and are out of scope for a single-file-parser PR. All 1827 unit tests + 68 E2E tests pass.

### Internal

- **XRAY_POLICY symmetry for read/search/edit — tool-name-agnostic edit rule, Q3 pre-flight, MISCONCEPTION ALERT, TERMS block** (user story: `todo_approved_2026-04-17_xray-edit-policy-symmetry.md`) — Tightened the XRAY_POLICY edit rule in `src/tips.rs::render_instructions()` to match the severity of the `.rs` read rule. The root cause was asymmetric framing: `NEVER READ .rs FILES DIRECTLY` read as a hard ban, while `NEVER USE apply_diff ... when xray_edit is available` read as a soft preference — LLMs habitually chose built-in edit tools for non-`.rs` files (`.md`, `.toml`, `.json`), assuming `xray_edit` was limited to indexed extensions. Changes: (1) added a `=== TERMS ===` definitions block at the top of the policy declaring `"xray tools"` and `"your built-in tools"` so the rest of the policy can speak tool-name-agnostically (portable across Roo / Claude Desktop / Cursor / GitHub Copilot / Cline); (2) rewrote the edit rule to lead with `NEVER USE your built-in edit tools for EDITING existing text files. ALWAYS use xray_edit — regardless of file extension. xray_edit works on ALL text files, NOT only on indexed extensions. ... xray_edit operates on BYTES, not on AST.`, followed by a `MISCONCEPTION ALERT` quoting the exact wrong pattern (`"this file is not indexed — I'll use my built-in edit tool" — WRONG`), a `DECISION TRIGGER`, and explicit `EXCEPTIONS` for creating new files, full-file rewrites >200 lines, and binary/byte-exact preservation; (3) redesigned `MANDATORY PRE-FLIGHT CHECK` into a symmetric Q1/Q2/Q3 (Q1 intent, Q2 file scope addressing READ/SEARCH/EDIT separately, Q3 justification enumerating valid/invalid reasons) with explicit `ENFORCEMENT` ("omitting the <thinking> block is itself a violation") and a `SELF-AUDIT HOOK` for after-the-fact recovery; (4) added a `COST REALITY` example for 8-block built-in patch/diff vs 1-call atomic `xray_edit(8 edits)` (8x fewer round-trips, zero whitespace risk); (5) added two `ANTI-PATTERNS` against extension-based edit-tool selection. All three `NEVER ...` blocks (read/search/edit) remain as separate sections but now share a symmetric severity tone. 14 new unit tests in `src/tips_tests.rs` (`test_instructions_has_terms_block`, `test_instructions_terms_block_before_critical_override`, `test_instructions_edit_rule_is_tool_name_agnostic`, `test_instructions_edit_rule_has_misconception_alert`, `test_instructions_edit_rule_has_exceptions`, `test_instructions_preflight_has_q3_justification`, `test_instructions_preflight_q1_q2_q3_symmetric`, `test_instructions_anti_pattern_extension_based_edit`, `test_instructions_cost_reality_has_multiblock_example`, `test_instructions_symmetric_severity_across_operations`, `test_instructions_no_hardcoded_builtin_names_in_edit_rule`, `test_instructions_edit_rule_works_without_def_extensions`, `test_instructions_terms_block_always_present`, `test_instructions_preflight_invalid_reasons_is_extension_agnostic`, `test_instructions_habit_clause_is_coherent`) verify TERMS presence/order, tool-name-agnostic wording, MISCONCEPTION ALERT + EXCEPTIONS, Q3 + ENFORCEMENT + SELF-AUDIT, and symmetric severity across operations. `build_policy_reminder` in `src/mcp/handlers/utils.rs` unchanged — the short reminder keeps its compact form. Backward-compatible with Opus 4.6; expected to reduce edit-tool drift on Opus 4.7+ (measurable via `scripts/analyze-transcript.ps1`). Self-review (2026-04-17) caught two hidden bugs in the initial Q3 `Invalid reasons` line: (a) a hardcoded `.rs` fragment (`'this is not a .rs file so xray probably doesn't apply'`) that broke the extension-agnostic contract for non-Rust servers, and (b) a malformed dangling suffix (`, Just habit / familiarity -> UNJUSTIFIED.`) where the habit phrase was tacked onto the quoted examples list instead of forming a coherent standalone clause. Both were fixed to `'this file extension is not indexed so xray probably does not apply to editing' — these are all UNJUSTIFIED. Just habit / familiarity is NEVER a valid reason.`, with regression tests `test_instructions_preflight_invalid_reasons_is_extension_agnostic` (asserts no `.rs` hardcoding in policy text when rendered for non-Rust extensions) and `test_instructions_habit_clause_is_coherent` (asserts the habit clause ends with `NEVER a valid reason` and the old broken fragment is absent).


- **Intent-first XRAY_POLICY — INTENT -> TOOL MAPPING, MANDATORY PRE-FLIGHT CHECK, COST REALITY, intent-aware policyReminder** — Expanded `render_instructions()` in `src/tips.rs` with three new positive-framing sections to address intent-first tool-selection failures observed in Claude 4.7 sessions (~15% xray / ~85% built-in baseline, 19 policy violations per session): (1) `INTENT -> TOOL MAPPING` — compact intent→tool lookup placed AFTER `CRITICAL OVERRIDE` and BEFORE `TASK ROUTING`, so intent-first models see it before reaching NEVER-rules; (2) `MANDATORY PRE-FLIGHT CHECK` — Q1/Q2/Q3 questions forcing a conscious justification in `<thinking>` before any built-in tool call, with explicit "habit/familiarity → UNJUSTIFIED" rule; (3) `COST REALITY` — measured token/round-trip ratios (5x, 24x, 3x fewer) instead of abstract prohibitions. `ANTI-PATTERNS` trimmed by ~3 rows (removed entries duplicating new sections). `STRATEGY RECIPES` trimmed from 7 recipes to top-3 (Architecture Exploration, Call Chain Investigation, Stack Trace / Bug Investigation); the remaining four recipes remain available via `xray_help` (render_json/render_cli iterate the full `strategies()` list). `build_policy_reminder` in `src/mcp/handlers/utils.rs` gained one `INTENT->TOOL:` oneliner (~200 chars, ~50 tokens per response) for re-entrancy between tool calls — existing policyReminder reinforcement argument applies. Existing NEVER-rules, TASK ROUTING, RESPONSE HINTS, ERROR RECOVERY, Git tools sections unchanged (backward-compatible with Opus 4.6 which already handles NEVER-rules correctly). 7 new unit tests (`test_instructions_has_intent_mapping`, `test_instructions_has_preflight_check`, `test_instructions_has_cost_reality`, `test_instructions_section_order`, `test_instructions_strategy_recipes_trimmed`, `test_build_policy_reminder_has_intent_oneliner`, `test_build_policy_reminder_intent_oneliner_without_extensions`) + updated `test_render_instructions_contains_key_terms` (Roo-specific-names rule narrowed to `list_code_definition_names` only, since built-in tool names are now legitimately listed in PRE-FLIGHT CHECK). All 1793 unit tests + 68 E2E tests pass.

- **Phase 2: Default-refactor for CLI Args structs** — Added `#[derive(PartialEq)]` and `impl Default` to all 5 CLI Args structs (`IndexArgs`, `ContentIndexArgs`, `ServeArgs`, `FastArgs`, `GrepArgs`) in `src/cli/args.rs`. The `Default` impls mirror the clap `default_value` attributes, locked in by 5 new drift-tests in `args_defaults_tests` module comparing `T::default()` to `T::parse_from([...])` (catches future drift between `Default` and clap defaults). Test code across `src/main_tests.rs`, `src/index_tests.rs`, `src/mcp/handlers/handlers_tests_fast.rs`, `src/mcp/handlers/handlers_tests_grep.rs`, `src/mcp/handlers/definitions_tests.rs` replaced ~60 struct literals with `..Default::default()` — reduces future field-addition churn from ~60 sites to 1 site per struct. Production code (`serve.rs`, `handlers/mod.rs`, `handlers/fast.rs`, non-test code in `cli/mod.rs`) retains explicit field literals for compiler-enforced conscious field assignment. All 1786 unit tests (was 1781 — +5 drift tests) + 68 E2E tests pass.

### Bug Fixes

- **`--respect-git-exclude` silently dropped on index rebuild** — The `--respect-git-exclude` flag on `xray serve` was only honored during the initial content-index build. Any subsequent rebuild path silently reverted to `respect_git_exclude=false`, meaning files listed in `.git/info/exclude` leaked back into the index after `xray_reindex`, workspace switch (via `roots/list` or manual `xray_reindex dir=new`), the MCP file-list auto-rebuild, or the CLI stale-index auto-rebuild for `xray fast` / `xray grep`. The flag is now propagated end-to-end:
  - `HandlerContext` gained a `respect_git_exclude: bool` field, populated from `ServeArgs.respect_git_exclude` at server start. All 5 MCP rebuild call-sites (`mod.rs` reindex × 2, workspace switch background build, `fast.rs` auto-rebuild × 2) now read from `ctx.respect_git_exclude` instead of hardcoded `false`.
  - `FastArgs` and `GrepArgs` gained `--respect-git-exclude` flags (default `false`, matching `IndexArgs` / `ContentIndexArgs` / `ServeArgs`). All 4 CLI auto-rebuild call-sites in `cmd_fast` and `load_grep_index` now read from `args.respect_git_exclude`.
  - The only remaining hardcoded `false` is in the hidden test helper `cmd_test_create_stale_index` (E2E test utility), with an explicit comment documenting the intent.
  - 8 new regression tests: CLI parser tests for `FastArgs`, `GrepArgs`, `ServeArgs` (default + flag set) and `HandlerContext::default()` / field-settable tests. All 1781 unit tests pass.

### Features

- **Symlink support and `.git/info/exclude` handling** — Three changes to improve indexing of symlinked directories and locally-excluded files:
  1. **`follow_links(true)`** — All 7 `WalkBuilder` instances now follow directory symlinks. Files accessible through symlinks (e.g., `docs/personal` → `ai-boosters/repos/<repo>/docs/personal`) are indexed by all three indexes (content, definitions, file-list) and the file watcher.
  2. **`.git/info/exclude` not respected by default** — `git_exclude(false)` is now the default for all walkers. Files listed in `.git/info/exclude` (personal notes, local configs) are indexed. `.gitignore` is still respected (node_modules, bin, obj are excluded). Rationale: `.gitignore` contains project-level build artifacts; `.git/info/exclude` contains user's local files they want to search.
  3. **`--respect-git-exclude` CLI flag** — New opt-in flag for `IndexArgs`, `ContentIndexArgs`, and `ServeArgs`. When set, restores the old behavior of respecting `.git/info/exclude`. Default: `false`.
  - Circular symlinks are safe — the `ignore` crate handles cycle detection via inode tracking.
  - Propagation through all rebuild paths (MCP reindex, workspace switch, CLI auto-rebuild) was added in the 2026-04-17 Bug Fix entry above.
  - 2 new symlink unit tests. All 1773 unit tests + 68 E2E tests pass.

- **Improved `phrase` parameter discoverability in `xray_grep`** — Updated tool schema description and `xray_help` tips to explicitly document that `phrase=true` performs literal string matching on raw file content, including XML tags, angle brackets, and other punctuation without escaping. Added XML example (`<MaxRetries>3</MaxRetries>`). This prevents LLMs from falling back to built-in search tools when searching XML/config content. Bumped `XRAY_HELP_MIN_RESPONSE_BYTES` from 32KB to 48KB to prevent response truncation with longer descriptions. New test: `test_phrase_postfilter_xml_full_tag`.

### Features

- **`xray_edit` flex-matching for markdown table separators** — When `xray_edit` Mode B (text-match) fails to find an exact match and falls through to Step 4 (flex-space matching), markdown table separator lines are now handled specially. Previously, separator rows like `|---------|-------------|` were treated as a single non-whitespace token (no spaces to flex), so searching for `|---|---|` would fail if the file had different dash counts. Now, `search_to_flex_pattern()` detects separator lines (characters from `{|, -, –, —, :, space, tab}` with at least one pipe and one dash) and generates a regex that preserves column count but allows flexible dash/colon/space counts between pipes. Also recognizes en dash (U+2013) and em dash (U+2014) to handle LLM/auto-formatting substitutions. The cascade order (exact → trim trailing WS → trim blank lines → flex-space) is unchanged — exact matches are always preferred. 4 new integration tests + unit test extensions. All 1770 unit tests pass.

### Bug Fixes

- **`ageHours` in `xray_info` showed stale value after reconciliation and watcher updates** — `ageHours` in the MCP `xray_info` response was computed from `created_at`, which was set only during initial index build and never updated after reconciliation or incremental watcher updates. This caused `ageHours` to show values like 579h even though in-memory data was freshly reconciled. Fix: `created_at` is now updated after reconciliation (using `walk_start` — the time when filesystem walk began, not `now()` at the end — to avoid a race condition where files modified during the tokenization phase would be missed by the next reconciliation) and after incremental watcher batch updates (using `now()` — safe because fsnotify detects subsequent changes). No-change reconciliation does NOT update `created_at`. Also fixes CLI `is_stale()` which uses the same field. 4 existing tests updated with `created_at` assertions. Modified files: `src/mcp/watcher.rs` (reconcile_content_index, update_content_index, update_definition_index), `src/definitions/incremental.rs` (reconcile_definition_index, reconcile_definition_index_nonblocking).

### Bug Fixes

- **Workspace switch does not update directory-bound resources** — Fixed 3 bugs (P0+P1+P2) where workspace switch (via `roots/list` or `xray_reindex dir=new`) left definition index, file watcher, and git cache pointing at the old directory. LLM got 0 results from `xray_definitions` without error, silently falling back to `read_file` (10x slower). Root causes and fixes:

  **Pre-fix (P0 blocker): Separated `ready` and `building` flags** — `content_ready`/`def_ready` flags served dual purpose: "index available" AND "no concurrent build". After workspace switch, `ready=false` blocked `xray_reindex` with "already building" even though no build was running (deadlock). Fix: added separate `content_building`/`def_building` `AtomicBool` flags with `compare_exchange` for concurrent build protection. Removed `xray_reindex`/`xray_reindex_definitions` from `requires_content_index`/`requires_def_index` guards. 3 new unit tests (not-blocked-when-not-ready, blocked-when-building, def-blocked-when-building).

  **Bug 1 (P0): Definition index not reloaded on workspace switch** — `handle_xray_reindex` only rebuilt content index. Definition index stayed empty from the old workspace. Fix: cross-load definition index from cache on workspace switch; if no cache, start background build. Mirror fix in `handle_xray_reindex_definitions` (cross-loads content index). Response includes `defIndexAction`/`contentIndexAction` field.

  **Root fix (P0): Don't index Unresolved workspace** — `determine_initial_binding` was called AFTER index building in `cmd_serve`. When CWD was VS Code install dir (no source files), empty indexes were built and `ready=true` masked the problem. Fix: moved `determine_initial_binding` BEFORE index building. When `Unresolved`: skip content/def index build, watcher, git cache. Leave `ready=false`.

  **Fix B (P0): Reset ready flags on roots/list** — `handle_pending_response` set `ws.status=Reindexing` but didn't reset `content_ready`/`def_ready`. Old indexes remained "ready" for queries, returning stale results. Fix: reset both flags when workspace changes via `roots/list`.

  **Bug 2 (P1): File watcher not restarted** — Watcher bound to startup directory, never restarted on workspace switch. Fix: added generation counter (`AtomicU64`) to watcher. On workspace switch, increment generation → old watcher exits on next timeout, new watcher starts for new directory. Supports unlimited sequential workspace switches.

  **Bug 3 (P2): Git cache stale after workspace switch** — Git cache built once at startup for the original directory. Fix: clear cache + start background rebuild for new workspace in `handle_xray_reindex`.

  Modified files: `src/mcp/handlers/mod.rs` (HandlerContext fields, dispatch guards, cross-load logic, watcher restart, git cache rebuild), `src/cli/serve.rs` (early binding, skip-unresolved, building flags), `src/mcp/server.rs` (ready flag reset), `src/mcp/watcher.rs` (generation-based stop). All 1766 unit tests + 68 E2E tests pass. 0 warnings.

- **Unused imports `IndexDetails` and `IndexMeta` in `info.rs`** — Removed two unused imports that produced a compiler warning.

## Unreleased

### Bug Fixes — Deleted Files in Git Tools (Parts 1–2, 2026-04-17)

- **`xray_git_history` returned 0 commits for deleted files** — `file_history()` used `git log --follow <path>` which silently returns nothing if `<path>` is not present in the working tree. Files that were ever deleted (even if recreated later under a different inode) lost their history. Fix: split `file_history()` into a `--follow` first-attempt + a no-follow fallback (`run_file_history_query`). The fallback runs only when the first attempt returns 0 commits AND `file_ever_existed_in_git()` confirms the path is known to git, so it doesn't degrade behavior for typo paths. Modified `src/git/mod.rs`. New tests in `src/git/git_tests.rs`: `test_file_history_returns_commits_for_deleted_file`, `test_file_history_returns_empty_for_never_existed_file`.

- **"File not found in git" warning conflated two distinct cases** — Handlers (`handle_git_history`, `handle_git_authors`, `handle_git_activity`) emitted the same `"File not found in git"` warning for both never-tracked paths (real error: typo) and deleted-but-historical paths (not an error: legitimate query). LLMs treated the deleted-file case as failure and retried with raw `git log` calls. Fix: new helper `annotate_empty_git_result` in `src/mcp/handlers/git.rs` consults `file_ever_existed_in_git()` and emits either `warning` ("File never tracked in git: ...") or `info` ("... is not in current HEAD. This is NOT an error"). Replaced 6 validation points across the 3 handlers. Renamed `file_exists_in_git` → `file_exists_in_current_head` to make the semantics explicit (deprecated alias retained for one cycle). Added `file_ever_existed_in_git()` and `list_tracked_files_under()` (single-spawn HashSet builder for batch existence checks). 4 new tests in `src/git/git_tests.rs` covering the new helpers.

### Features — `includeDeleted` parameter on `xray_git_activity` (Part 3, 2026-04-17)

- **Filter activity to deleted files only** — `xray_git_activity` accepts a new optional `includeDeleted: boolean` parameter. When `true`, the result list is post-filtered to files that are NOT in current HEAD — answers "what was deleted in this date range?" in a single call. Without this parameter, LLMs had to fall back to a manual `git log --all --diff-filter=D ...` invocation. Implementation: a single `git::list_tracked_files_under()` spawn builds a `HashSet<String>` once per request, then each result file is checked with O(1) membership lookup — zero scaling cost per result. Both the cache path and the CLI path apply the same filter. Response includes `summary.includeDeleted: true` (echo) and `summary.hint` ("Filtered to deleted files only (NOT in current HEAD)"). Updated tool schemas in `handlers/git.rs`. Tests: `mcp::handlers::tests_git::test_git_activity_include_deleted_default_false`, `test_git_activity_include_deleted_true_sets_field_and_hint`, `test_git_activity_include_deleted_filters_existing_files_in_real_repo`. Documented in `docs/mcp-guide.md` (parameter table + new "Deleted Files Support" section) and `docs/e2e/git-tests.md` (T-DELETED-01..04). E2E: `T-GIT-INCLUDE-DELETED` in `e2e-test.ps1`. Tips updated in `src/tips.rs` with INTENT mappings, a new strategy ("Deleted File Archaeology"), and parameter examples.

### Internal — Policy hardening (Part 4 of deleted-files story, 2026-04-17)

- **System-prompt slimming (6 cuts to `render_instructions`)** — After Part 4 reinforcement additions pushed the token budget from ~2400 to ~2800 tokens, the full system prompt was audited for redundancy and 6 cuts were applied in a single pass:
  1. **TASK ROUTING table removed** — 100% duplicate of the newer `INTENT -> TOOL MAPPING` block. The task-keyed table and the intent-keyed map were covering the same routes with different phrasing; keeping both was pure noise for the LLM. The `task_routings()` helper (and its 3 tests) were also deleted — no other callers. `INTENT -> TOOL MAPPING` remains as the single routing authority.
  2. **COST REALITY slimmed from 6 lines to 1** — the detailed 5x/24x/8x token ratios, atomic-rollback narrative, and `--ext` clarifications were removed from the system prompt. Kept: a single rule-of-thumb line ("xray tools are 3-24x cheaper... 2 built-in calls in a row on the same file = you should have used xray") + a pointer to `xray_help` for measured numbers. Rationale: the numbers are background context, not decision-critical.
  3. **STRATEGY RECIPES inline bodies removed** — the 3 expanded top-recipe bodies (Architecture Exploration / Call Chain Investigation / Stack Trace) were consuming ~200 tokens of step-by-step prose. Replaced with a single reference line pointing at `xray_help`. The full 7-recipe catalog is still delivered via `xray_help` on-demand — `strategies()` is untouched and still drives `render_json`, `render_cli`, and `handle_xray_help`.
  4. **2 duplicate ANTI-PATTERNS removed** — the "NEVER choose a built-in edit tool based on file extension" bullet and the implicit search_files duplicate were already covered more forcefully by the `FILE EDITING DECISION TRIGGER` block ("regardless of file extension" + `MISCONCEPTION ALERT`) and by the dedicated `NEVER USE search_files` block. Kept: extension-based `xray_definitions` anti-pattern (JSON/YAML/MD routing) and the EXAMPLE VIOLATION block from Part 4.
  5. **TERMS block condensed from 8 lines to 3** — the original block had 8 verbose lines explaining that built-in tool names vary across hosts. Condensed to 3 lines that still name both classes of tools and tell the LLM to map by OPERATION TYPE rather than by tool name.
  6. **PRE-FLIGHT Q2 consolidated from 3 lines to 1** — the three separate READ/SEARCH/EDIT sub-questions were repeating the same mapping pattern; consolidated into a single line naming all three operation types with their xray mappings inline ("READ -> xray_definitions if indexed, else built-in OK. SEARCH -> xray_grep. EDIT -> xray_edit on ANY text file..."). All three scopes and the `UNJUSTIFIED` callout are preserved.
  - **Result**: `render_instructions("cs,ts,tsx,sql,rs")` measured at **~2102 tokens** (1577 words, 11440 chars), down from the ~2800-token peak during Part 4 authoring. The test budget is now `approx_tokens < 2250` (with ~150-token headroom) — down from `<3000`. Net savings: ~25% of the system prompt, with zero loss of enforcement signal. All 1830 unit tests + 68 E2E tests still pass (1 E2E test renamed: `T-TASK-ROUTING` -> `T-INTENT-MAPPING`).
  - **Tests updated**: 12 tests in `src/tips_tests.rs` + 1 in `src/mcp/protocol_tests.rs` — each pointed at specific phrases from the old rendered text ("TASK ROUTING", "5x fewer tokens", "[Architecture Exploration]", "Exact names differ per", etc.). Every updated test retains its invariant but now asserts against the slim equivalent (`INTENT -> TOOL MAPPING`, `3-24x cheaper`, `xray_help for the full catalog`, `names vary per host`, etc.). No test was silently deleted — 3 tests that lost their target (`test_task_routings_not_empty`, `test_routing_tool_names_exist_in_definitions`, `test_task_routing_has_non_code_files_entry`) were removed and replaced by updated `test_task_routing_with_definitions` / `test_task_routing_without_definitions` that verify the same gating via `INTENT -> TOOL MAPPING`.


- **`src/tips.rs` — prompt reinforcement against habit-driven built-in tool selection**
  - INTENT → TOOL MAPPING gained 3 new entries for validation intents (`validate/fact-check`, `quick yes/no`, `confirm absence`) — all route to `xray_grep countOnly=true`.
  - ANTI-PATTERNS block gained `EXAMPLE VIOLATION` block naming the most common failure mode (linguistic coincidence: `search_files` matching the word "search") with ROOT CAUSE + PREVENTION guidance.
  - MANDATORY PRE-FLIGHT CHECK gained `PRE-CALL SELF-AUDIT` — a 3-question check BEFORE formulating a built-in call, complementing the existing post-call SELF-AUDIT HOOK.
  - New tip "Trivial task != trivial policy check" — names the "trivial task trap" where LLMs skip pre-flight on seemingly-small tasks.
  - Token budget test raised from <2800 to <3000 to accommodate the additions (~130 words, justified).
  - Source: live session 2026-04-17 where the LLM writing the parent user story used `search_files` instead of `xray_grep` for a code-validation check — meta-observation turned into a policy reinforcement.

- **`src/mcp/handlers/utils.rs` — imperative `policyReminder` rewrite** — `build_policy_reminder` now emits **enforcement-framed** wording instead of passive advice. Every MCP tool response now carries `=== XRAY_POLICY - ENFORCEMENT ===` with four imperative clauses:
  - `REQUIRED:` (not "prefer") for xray_* tools on read/search/edit operations.
  - `NO EXCEPTIONS for 'familiarity', 'habit', 'quick check', or 'just this once'` — explicitly closes the rationalization lanes observed in live sessions.
  - `If about to call a built-in on an indexed file -> STOP` — action verb for pre-call self-audit.
  - `Built-in calls when xray covers the case = protocol error` — frames violations as protocol errors, not style preferences.
  - Per-response VIOLATION clause now names the required tools: `VIOLATION = calling built-in read_file/search_files/apply_diff on files with extensions [X, Y]. REQUIRED: xray_definitions (read), xray_grep (search), xray_edit (edit).`
  - Motivation: during Part 4 authoring the LLM itself violated the policy multiple times despite the old `policyReminder` being present in every response. The old passive wording ("Prefer xray... Check applicability... Use environment tools with justification") was shown to tolerate built-in fallback via the "just this once / trivial task" rationalization lane. The new imperative framing is expected to give a measurable (not revolutionary) +5–15% compliance lift; physical enforcement (custom VS Code mode with fileRegex) remains available as a future Part 5 if needed.
  - New test `test_build_policy_reminder_is_imperative` asserts the presence of `REQUIRED:`, `NO EXCEPTIONS`, `STOP`, `protocol error`, `ENFORCEMENT` AND the absence of the old passive phrasing (`Prefer xray`, `Check xray applicability`, `with explicit justification`) — prevents regression to passive wording.
  - 4 existing `build_policy_reminder` tests updated to the new wording. 1833 unit tests pass (+1 new).

### Security

- **XML on-demand: workspace sandbox for `file=` resolver** — `resolve_xml_file_path` in `src/mcp/handlers/definitions.rs` now resolves the `file=` parameter via `std::path::Path::canonicalize()` and rejects any path that does not start with the canonicalized `server_dir` prefix. Previously, a relative `file=` was joined to `server_dir` and a substring fallback scanned `index.files` for any entry whose path contained the filter, and as a final "last resort" the function returned the joined path even if the file didn't exist. Two attack/mishap paths were closed: (1) **path traversal via `..` segments** — `file='../../etc/passwd'` (or a Windows equivalent like `file='..\..\Windows\System32\drivers\etc\hosts'`) could resolve to a file outside the workspace and be parsed by the XML handler; (2) **substring-fallback collision** — `file='web.config'` could silently resolve to `webapp.config` (or any other file whose path contains `web.config` as a substring) when the exact path didn't exist under `server_dir`, returning structural data from an unrelated file without any hint to the LLM. Both the substring fallback and the "last resort" non-existent-path return are removed. Absolute paths are still accepted but must canonicalize inside `server_dir`. Returns `Result<String, String>` with precise error messages (`"XML file not found"`, `"XML file path is outside workspace: ..."`). 4 new unit tests in `src/mcp/handlers/definitions_tests.rs`: `test_xml_on_demand_rejects_path_traversal` (dotdot escape), `test_xml_on_demand_rejects_absolute_outside_workspace` (absolute path outside server_dir), `test_xml_on_demand_no_substring_fallback_collision` (`web.config` must not resolve to `webapp.config`), and `test_xml_on_demand_returns_error_for_missing_file` (non-existent path returns `Err`, not silent success).

### Bug Fixes

- **XML on-demand: UTF-8 panic in text content truncation** — `extract_text_content` in `src/definitions/parser_xml.rs` truncated element text via byte-indexing (`&text[..MAX_TEXT_CONTENT_LEN]`) which panicked when the 200-byte boundary fell inside a multi-byte UTF-8 codepoint (Cyrillic letters are 2 bytes each; emoji are 4 bytes). A single well-formed XML element with >100 Cyrillic chars or >50 emoji could crash the MCP tool handler with `byte index N is not a char boundary`. Fix: switched to char-aware truncation via `chars().count()` for the length check and `chars().take(MAX_TEXT_CONTENT_LEN).collect()` for the truncation. 2 new regression tests in `src/definitions/definitions_tests_xml.rs`: `test_text_content_truncation_utf8_cyrillic` (250 Cyrillic chars, no panic, truncated to 200 chars) and `test_text_content_truncation_utf8_emoji` (250 emoji, no panic, truncated to 200 chars).

- **XML on-demand [Windows]: UNC prefix leaks into JSON response** — On Windows, `Path::canonicalize()` returns paths with the `\\?\` UNC prefix (e.g. `\\?\C:\Repos\Xray\app.config`). After adding the sandbox check (see Security entry), the canonicalized path was written verbatim into the `file` field of every XML definition result, exposing an implementation detail that breaks downstream tooling (editor "open file" actions, copy-paste into terminals, path comparison against `xray_grep` output). Fix: `resolve_xml_file_path` now strips the `\\?\` prefix from the returned string on Windows (`#[cfg(windows)]` + `strip_prefix(r"\\?\")`), but only *after* the sandbox validation uses the full canonical form — so the security check remains correct. Regression assertion added to the positive-path test `test_xml_on_demand_resolves_relative_path` to verify the returned path does not start with `\\?\`.

### Internal

- **XML on-demand: typed error enum via `thiserror`** — `parse_xml_on_demand` in `src/definitions/parser_xml.rs` replaced its stringly-typed error return with `XmlParseError` enum (derived via `thiserror`) with two variants: `GrammarLoad(String)` — tree-sitter grammar failed to load (internal bug, should never happen in production; indicates a corrupted or mismatched `tree-sitter-xml` build) — and `TreeSitterReturnedNone` — parser returned `None` for the input, typically a malformed/truncated XML file (expected user-facing error). `try_xml_on_demand` in `src/mcp/handlers/definitions.rs` now `match`es on the variants and emits distinct hints to the LLM (`"internal parser error"` vs `"malformed XML file"`), allowing the caller to distinguish between a bug in xray and bad user input without parsing error strings.

### Features

- **XML on-demand structural context (initial implementation)** — `xray_definitions` now supports on-demand XML parsing for `.xml`, `.config`, `.csproj`, `.manifestxml`, `.props`, `.targets`, `.resx` files via a new `lang-xml` Cargo feature (in default build) backed by `tree-sitter-xml 0.7`. XML files are NOT added to the definition index — they are parsed on-the-fly when `file=` is combined with `containsLine` or `name`. Key capabilities:
  - **Parent Promotion (containsLine)**: When `containsLine` targets a leaf element (no child elements), the result is automatically promoted to the parent block so the LLM sees full structural context. For example, searching for `<ServiceType>Search</ServiceType>` returns the entire `<SearchService>` parent block with all siblings, not just the trivial leaf.
  - **textContent search with Parent Promotion (name filter)**: The `name` filter searches both XML element tag names AND `textContent` of leaf elements. Leaf matches are promoted to their parent block with `matchedBy: "textContent"` and `matchedChild: "<leafTag>"` (or `matchedChildren: [...]` when multiple leaves in the same parent match — de-duplicated into one result). Tag-name matches take priority: if the same parent is already matched by tag name (`matchedBy: "name"`), textContent-promoted duplicates are suppressed. Min-length guard: terms shorter than 3 characters are not searched in text content (only in tag names) to avoid noise. Result ordering: name matches first, then textContent-promoted.
  - **textContent field on leaves**: Leaf elements include a `textContent` field with their text value (truncated to 200 chars — UTF-8 safe, see Bug Fixes).
  - **XPath-like signatures**: Each element has a structural path like `configuration > appSettings > add[@key=DbConnection]`.
  - **`onDemand: true` flag**: Response includes this so the LLM knows data came from on-the-fly parsing, not the definition index.
  - **`xmlHint` in grep results**: When `xray_grep` finds matches in XML files, the response includes an `xmlHint` suggesting `xray_definitions file=<path> containsLine=N` for structural context.
  - **Absolute paths supported**: XML files outside the workspace can be addressed via absolute file paths (subject to Phase 1 sandbox — see Security).
  - **Directory-path error hint**: Passing a directory path to `file=` returns a clear error (`"XML on-demand requires a file path, not a directory"`) with guidance to use `xray_fast` to locate specific files.
  - 25 new XML parser tests + 6 unit tests for textContent V2 + 4 updated hint tests.
  - **Phase 1 security hardening applied to this feature is listed in the Security / Bug Fixes / Internal sections above** (path-traversal sandbox, UTF-8 panic fix in text truncation, Windows UNC-prefix leak fix, `XmlParseError` typed enum).

- **`xray_grep` — `file=` parameter and `dir=` auto-conversion** — Added explicit `file` parameter for substring filtering by file path/basename (case-insensitive, supports comma-separated OR). When a **file path** is passed to `dir=` (either heuristically — path ends in a known extension — or detected via `fs::metadata`), it is now auto-converted to `dir=<parent>` + `file=<basename>` instead of returning an error. The `summary.dirAutoConverted` field in the response teaches the correct `file='<name>'` pattern so the LLM self-corrects on the next turn. Explicit `file=` wins over the auto-populated value. Combines with `dir`/`ext`/`excludeDir` via AND.

### Internal

- **Test boilerplate reduction: dual-field antipattern eliminated, test factories added** — Systematic cleanup of test construction boilerplate across 3 handler structs, reducing future field-addition effort from 10-12 places to 1 place per struct.

  **Dead field removal:**
  - `GrepSearchParams`: removed `exclude_dir` and `exclude` fields (dead code — only `exclude_patterns` and `exclude_lower` were used in production filtering). 12 inline constructions + 1 production constructor updated.
  - `CallerTreeContext`: removed `exclude_dir` and `exclude_file` fields (dead code — assigned to `_` variables, never read). 7 inline constructions + 2 production constructors updated.
  - `DefinitionSearchArgs`: removed `exclude_dir` field (dead code — only used to create `exclude_patterns` at parse time, never read after). 2 test assertions migrated to `exclude_patterns`.
  - Deleted standalone `passes_caller_file_filters()` function (duplicated `CallerTreeContext::passes_file_filters()` logic using raw inputs instead of pre-computed patterns; called only from 9 tests). Tests removed — equivalent coverage exists via handler integration tests and `utils_tests.rs`.

  **Test factory constructors:**
  - `CallerTreeContext::test_default()` — `#[cfg(test)]` constructor with 4 required params (content_index, def_idx, limits, node_count) and sensible defaults for all 10 remaining fields. 7 inline 14-line constructions → 1-3 line struct-update expressions.
  - `make_params_default()` / `make_params()` — consolidated as single GrepSearchParams factory, all inline constructions replaced with `..make_params_default()` overrides.

  **Dead code cleanup:**
  - Removed `ParsedFileResult.was_lossy` field (set but never read — incremental parsing sets it, nothing consumes it). Updated 7 struct literals in tests.
  - Removed `matches_ext_filter_prepared()` utility (inlined equivalent exists in `CallerTreeContext::passes_file_filters()`).
  - Removed `path_matches_exclude_dir()` wrapper — 6 tests rewritten to test `ExcludePatterns::from_dirs()` + `.matches()` directly.
  - `update_file_definitions()`, `reconcile_definition_index()` marked `#[cfg(test)]` — used only from tests (14 callers), never from production code.

  All 1764 unit tests + 68 E2E tests pass. 0 compiler warnings.

### Performance

- **Performance audit: 6 findings fixed (Findings 1-6)** — Systematic performance optimization based on full code audit. Three groups of fixes:

  **Finding 1 (CRITICAL, ~400ms → ~30ms): Two-pass `fileCount` in `xray_fast dirsOnly`** — Replaced O(N × depth) ancestor-walking HashMap for ALL directories (~10K) with a two-pass approach: (1) main loop collects matched directories (~29 typical), (2) post-loop counts files only for matched dirs via `starts_with`. For 29 dirs × 100K entries = 2.9M cheap string comparisons instead of ~800K HashMap operations. Removed ~50 lines of complex `dir_prefix` resolution code.

  **Finding 2 (MODERATE, ~5-40ms → <1ms): Cached `canonical_server_dir`** — Added `canonical_dir: String` field to `WorkspaceBinding`, computed once at bind time via `compute_canonical()`. New `set_dir()` method ensures canonical is always recomputed on workspace changes. `HandlerContext::canonical_server_dir()` provides cached access. Eliminated 1-2 `std::fs::canonicalize()` syscalls per request in `fast.rs dir_is_outside` check (~1-5ms each on Windows).

  **Findings 3+4+5+6 (MODERATE): Pre-computed filter patterns** — Introduced `ExcludePatterns` struct in `utils.rs` with pre-lowercased segment patterns, eliminating thousands of per-file String allocations across grep, definitions, and callers handlers:
  - **Finding 3**: `path_matches_exclude_dir()` — patterns pre-computed once per query instead of per-file × per-exclude-dir
  - **Finding 4**: `apply_entry_filters()` in definitions — `file_filter_terms` and `parent_filter_terms` pre-parsed in `DefinitionSearchArgs` at parse time
  - **Finding 5**: `matches_ext_filter()` — added `prepare_ext_filter()` for pre-split ext lists (used in callers)
  - **Finding 6**: `passes_file_filters()` in grep — pre-lowercased exclude patterns and path normalization computed once per file

  All 1865 unit tests + 68 E2E tests pass. Finding 7 (phrase search file I/O) deferred — inherent cost, needs LRU cache.

### Bug Fixes

- **`xray_grep` substring `countOnly=true` included unnecessary `matchedTokens`** — When `xray_grep` was called with `countOnly=true` in substring mode, the response still included the `matchedTokens` array. This wasted ~200-1000 bytes and could trigger false truncation ("capped matchedTokens to 20") that confused LLMs into thinking results were incomplete. Fix: `build_substring_response()` no longer emits `matchedTokens` when `count_only=true`. The normal token search mode (`build_grep_response`) was already correct. 2 new tests + 1 existing test updated.

- **`xray_grep` phrase auto-switch hint not actionable** — When dotted namespace terms (e.g., `System.Data.SqlClient`) triggered auto-switch to phrase mode (~100x slower), the `searchModeNote` only explained the mechanism but didn't tell the LLM what to do. LLMs continued submitting dotted terms across multiple calls. Fix: when punctuation triggers the auto-switch, the hint now includes actionable advice: "Tip: use last segment only for faster substring search (e.g., 'SqlClient' instead of 'System.Data.SqlClient')". Space-only auto-switches (e.g., "public class") retain the explanatory note since phrase search is the correct semantic for those queries.

### Features

- **`xray_fast` `dirsOnly` now includes `fileCount` for all patterns** — Previously, `fileCount` was only included in `dirsOnly` responses when using wildcard pattern (`pattern='*'`). Now `fileCount` is included for any `dirsOnly` request, including filtered patterns like `pattern='Storage,Redis'`. Results are sorted by `fileCount` descending. The fileCount traversal is O(N) over index entries (~1ms for 66K files) — negligible cost. Tool description updated in `mod.rs` and `tips.rs`. 1 new test + 1 existing test updated.

### Bug Fixes

- **`xray_fast` stale file-list index after file creation/deletion** — `xray_fast` now uses an in-memory file-list index with dirty-flag invalidation, ensuring newly created and deleted files are always visible. Previously, the file-list index (used by `xray_fast`) was loaded from disk on every call and never updated by the file watcher or `xray_reindex`, causing stale results until server restart. Four root causes fixed: (1) FileIndex added to `HandlerContext` as `Arc<RwLock<Option<FileIndex>>>` with lazy initialization; (2) Watcher sets `file_index_dirty` flag on ANY file create/delete event BEFORE the `--ext` filter (FileIndex indexes all files, not just `--ext`); (3) `handle_xray_fast` checks dirty flag → rebuilds from filesystem (~35ms) → caches in memory → resets flag; (4) `handle_xray_reindex` invalidates the cache (sets to `None` + dirty). Outside-server-dir requests use disk-cached indexes (load or build+save) for fast repeated access. `FileIndex` and `FileEntry` structs now derive `Clone`. 3 new unit tests (dirty-flag rebuild, deletion detection, None invalidation) + live E2E verification (create → search → delete → search). All 1769 unit tests + 68 E2E tests pass.

### Bug Fixes (Audit 2026-03-16)

- **OOM safety cap for `git log` in CLI fallback** — `top_authors()` and `repo_activity()` in `src/git/mod.rs` now include `--max-count` safety caps (50K and 10K respectively) to prevent unbounded stdout on huge repos without date filters. Previously, a repo with 500K+ commits could cause OOM when git log read all output into a single String. Mitigated by cache (MCP handler checks cache first) and date/path filters, but the safety cap ensures no crash even in worst case.

- **Incomplete rollback on RwLock write failure in reindex handlers** — `handle_xray_reindex()` now fully rolls back workspace state (dir, mode, generation, status) when `ctx.index.write()` fails (poisoned RwLock). Previously, workspace metadata would remain pointing to the new dir while the in-memory index stayed old. Also enhanced `handle_xray_reindex_definitions()` with the same full rollback (previously only reset `ws.status`).

- **Debounce starvation in file watcher** — `start_watcher()` now has a `MAX_ACCUMULATE` timeout (3 seconds) that forces batch processing even when filesystem events arrive continuously faster than the debounce interval (500ms). Previously, continuous events could prevent the debounce timeout from ever firing, causing `dirty_files` to accumulate indefinitely without being processed.

- **Batch callers parity with single-method path** — `handle_multi_method_callers()` now includes per-method `warning` (ambiguity check), `hint` (nearest match), `truncated`, and `nodesVisited` fields — matching the single-method path behavior. Extracted `check_method_ambiguity()` helper function (DRY refactor) used by both paths. Overall `truncated` flag added to batch summary. 2 new unit tests.

### Features
- **`xray_fast` / `xray_grep` — relative `dir` paths** — The `dir` parameter now accepts relative paths (e.g., `dir: "src/services"`) which are resolved against `server_dir`. Previously, relative paths silently returned 0 results in `xray_fast` and errors in `xray_grep` because `std::fs::canonicalize` resolved them against the process CWD instead of the server directory. New `resolve_dir_to_absolute()` utility handles both absolute and relative paths consistently across both tools. 15 new unit tests.
- **`xray_fast` — glob pattern auto-detection** — The `pattern` parameter now supports glob-style wildcards: `Order*` (starts with), `*Service.cs` (ends with), `Use?Service` (single-char wildcard). Glob characters (`*`, `?`) are auto-detected and converted to anchored regex. Without glob chars, behavior is unchanged (substring matching). 5 new unit tests.

### Bug Fixes
- **CLI `xray grep` fails with "No content index found" when index format is outdated** — When a content index file existed on disk but had an incompatible format version (legacy or version mismatch), `xray grep` returned `Error: No content index found for '.'` instead of rebuilding the index. The misleading `[content-index] ... will rebuild` stderr message was printed by `load_content_index()`, but the function only returned an error — the CLI caller (`load_grep_index`) never actually rebuilt. **Root cause**: `load_grep_index`'s `Err(_)` branch only tried `find_content_index_for_dir()` as a fallback (which also failed for the same version reason), while the MCP `cmd_serve` correctly rebuilt in the same scenario. **Fix**: When `auto_reindex=true` (default) and `load_content_index` fails, `load_grep_index` now reads extensions from the `.meta` sidecar file and calls `build_content_index()` — consistent with `cmd_serve` behavior. If rebuild also fails, falls through to `find_content_index_for_dir` as last resort. Changed misleading "will rebuild" messages to "index outdated" in both content and definition index loaders. 1 new E2E test (`T-GREP-STALE`).

- **`xray_fast` glob pattern ranking degradation** — When glob patterns like `Order*` were auto-converted to regex (`^Order.*$`), the ranking logic in `best_match_tier()` compared file stems against the regex string literal instead of the original search term. This caused all results to land in tier 2 (default), disabling exact-match (tier 0) and prefix-match (tier 1) prioritization. Files were found correctly but their order was suboptimal. Fix: `ranking_terms` are now computed from the original glob patterns via `extract_glob_literal()` (e.g., `"Order*"` → `"Order"`), preserving proper tier ranking. Non-glob patterns are unaffected. 6 new unit tests (5 for `extract_glob_literal`, 1 integration test for ranking order).

- **`def-index` warning message says `search def-audit` instead of `xray def-audit`** — The warning printed during `xray def-index` when files with 0 definitions are found incorrectly suggested running `search def-audit`. Changed to `xray def-audit` in both the runtime message and the `--help` examples.

### Features
- **Smart whitespace auto-retry in `xray_edit` Mode B** — Extended the text-match auto-retry cascade with two new steps to handle common LLM-vs-editor whitespace mismatches. Previously, `xray_edit` only retried with trailing whitespace stripped. Now the cascade is:
  1. Exact match (existing)
  2. Strip trailing whitespace per line (existing)
  3. **NEW: Trim leading/trailing blank lines** — handles cases where the LLM starts search text with `##` but the file has `\n##`. Also strips trailing whitespace per line in combination.
  4. **NEW: Flex-space regex matching** — converts the search text to a regex where each whitespace gap becomes `[ \t]+`, matching text with different amounts of horizontal whitespace (tabs, multiple spaces). Handles VS Code auto-formatted markdown tables where `| Issue |` becomes `| Issue       |`.
  - Flex-space replacement uses `regex::NoExpand` — `$` characters in replacement text are treated literally.
  - `expectedContext` check also gets a flex-space fallback (collapse whitespace in both context window and expected text).
  - Warnings emitted for each non-exact match (e.g., `"edits[0]: text matched with flexible whitespace (spaces collapsed)"`).
  - Flex-space is NOT applied in regex mode (`is_regex: true`) — only for literal text matching.
  - Exact match is always preferred (cascade stops at first success).
  - Anchor matching (`insertAfter`/`insertBefore`) also gets the full 4-step cascade with per-occurrence match length tracking for flex-space.
  - 20 new unit tests. 1 existing test updated for new behavior. All 1720 unit tests pass.


### Performance

- **Memory estimate accuracy improved (`xray_info`)** — Updated memory estimation coefficients in `estimate_content_index_memory()` and `estimate_definition_index_memory()` to more accurately reflect real Working Set consumption. Key changes: (1) HashMap entry overhead increased from 80B to 120B (includes bucket + hash + metadata + alignment padding); (2) Each `Posting.lines` Vec now accounts for 32B allocator overhead per heap allocation; (3) String allocator overhead (32B) added to key estimates; (4) CallSite estimate increased from 60B to 100B (accounts for String headers + allocator overhead); (5) `selector_index` added to secondary index count (was missing); (6) `methodCallsOverheadMB` separated as a distinct field; (7) New `allocatorOverheadMB` field (20% of data size) for mimalloc/jemalloc fragmentation estimate. Previously, `xray_info` reported ~488 MB for a content index that actually consumed ~620 MB in Working Set (3.6× undercount overall). New estimates should be within 50% of actual WS.

- **`shrink_to_fit()` on all HashMaps after index load** — Added `ContentIndex::shrink_maps()` and `DefinitionIndex::shrink_maps()` methods that reclaim excess HashMap capacity after loading indexes from disk. Called automatically in all 4 index load paths in `serve.rs` (direct content load, background content build+reload, direct def load, background def build+reload). Shrinks all 11 HashMaps in `DefinitionIndex` and 3 in `ContentIndex`, plus inner Vecs for maps with non-trivial value sizes. Expected savings: ~20-50 MB for large projects (65K files). Zero latency impact on queries; ~50-80ms one-time cost at startup.

### Bug Fixes (Audit Batch 2026-03-14)

- **Removed `xray_find` tool** — deprecated slow filesystem-walk tool removed entirely. Closes 7 audit findings (L27-L31, M13, M14). Use `xray_fast` for all file lookups (90x+ faster). Removed from: MCP tool definitions, CLI commands, tips, E2E tests. **Breaking change**: `xray_find` MCP tool and `xray find` CLI command no longer exist.

- **TF-IDF division-by-zero guards** (L22, L23) — `score_normal_token_search` in `grep.rs` now guards against `doc_freq == 0` (unreachable but defensive) and `file_token_counts[file_id] == 0` (converts to 1.0 to avoid NaN/Inf). Applied to both scoring paths.

- **Initial commit diff crash** (L34) — `get_commit_diff()` in `git/mod.rs` no longer crashes on initial commits (no parent). Uses `git rev-parse --verify hash^` to detect parentless commits, then diffs against the empty tree hash.

- **Non-UTF8 git output handling** (L35) — `run_git()` now uses `String::from_utf8_lossy` instead of `String::from_utf8`, preventing full failure on non-UTF8 git output (e.g., binary file names, non-Latin commit messages).

- **`excludeDir` segment matching** (L25) — `passes_file_filters` in `grep.rs` now matches directory names by path segment instead of full-path substring. `excludeDir=["test"]` no longer falsely excludes files in directories like "contest" or "testing". Normalizes path separators for cross-platform correctness.

- **`xray_fast` wildcard + regex guard** (L14) — `pattern="*"` with `regex=true` no longer crashes with a regex parse error. When the pattern is a wildcard, `regex=true` is silently ignored.

- **CLI/MCP `maxResults` parity** (L36) — CLI `xray grep --max-results` default changed from 0 (unlimited) to 50 to match the MCP default.

### Bug Fixes (Audit Batch 2 — 2026-03-15)

- **12 findings from audit batch fix** — Second systematic pass from the 2026-03-14 code audit. 5 real bugs (Tier A), 4 UX improvements (Tier B), 3 code quality items (Tier C). Source: `docs/user-stories/todo_approved_2026-03-15_audit-findings-batch-fix.md`.

  **Tier A — Real bugs:**
  1. **[A1] `expand_interface_callers()` kind filter** — Added `DefinitionKind` filter (Method/Constructor/Function/StoredProcedure/SqlFunction) before interface caller expansion. Previously, properties/fields/enum members with the same name as the target method would trigger false caller expansion.
  2. **[A2] `handle_contains_line_mode()` missing filters** — `containsLine` queries now respect `excludeDir`, `kind`, and `parent` filters. Previously all three were silently ignored, returning unfiltered results.
  3. **[A3] `build_grep_references()` definition file noise** — `grepReferences` no longer includes the file where the method is defined. The definition file's name appears in its own declaration, creating a false positive in grep references.
  4. **[A5] Error responses missing guidance** — Error responses from MCP tools now include `policyReminder`, `nextStepHint`, and workspace metadata. Previously, errors returned before `inject_response_guidance()`, causing LLMs to lose tool routing guidance on errors.

  **Tier B — UX improvements:**
  5. **[B1] `filesOnly` + `dirsOnly` mutual exclusion** — `xray_fast` now returns a descriptive error when both flags are set (previously returned 0 results silently).
  6. **[B2] Hint overwrite in `xray_fast`** — Multiple hints (ext_ignored + truncation) are now concatenated instead of the second overwriting the first.
  7. **[B4] `searchTimeMs` preservation** — `inject_metrics()` no longer overwrites handler-specific `searchTimeMs`. Added `totalTimeMs` field for full dispatch-to-response time.

  **Tier C — Code quality:**
  8. **[C1] Phrase search deduplication** — `handle_phrase_search()` refactored to use `collect_phrase_matches()`, eliminating ~82 lines of duplicated tokenization/candidate/verification logic.
  9. **[C2] `caller_popularity()` precache** — Popularity scores pre-cached before sort to avoid O(n log n) string allocations in the sort closure.
  10. **[C3] Trailing newline preservation** — `strip_trailing_whitespace_per_line()` changed from `.lines()` to `.split('\n')` to preserve trailing newlines.

  6 new unit tests. All 1735 unit tests + 68 E2E tests pass.

### Features

- **Workspace Discovery (auto-detect project directory)** — The MCP server now automatically detects the correct workspace directory via three mechanisms: (1) **MCP `roots/list` protocol** — if the client supports roots (Roo Code, VS Code Copilot), the server requests `roots/list` after initialization and uses the first root as the workspace. Handles `roots/list_changed` notifications for live project switching. (2) **`--dir .` auto-detection** — when started with `--dir .`, the server checks if the CWD contains source files matching `--ext`. If yes → `DotBootstrap` mode (works immediately, roots can override). If no → `Unresolved` mode (tools blocked with `WORKSPACE_UNRESOLVED` structured error + hint). (3) **`xray_reindex dir=<path>` workspace switch** — LLMs can explicitly bind the workspace by calling `xray_reindex` with a `dir` parameter. Only blocked in `PinnedCli` mode (`--dir /explicit/path`). Load-first: tries cached index from disk before full rebuild. Response includes `workspaceChanged`, `previousServerDir`, `indexAction` fields.
  - **State machine**: `WorkspaceBinding` with 5 modes (PinnedCli, ClientRoots, ManualOverride, DotBootstrap, Unresolved) and 3 statuses (Resolved, Reindexing, Unresolved). Generation counter for safe concurrent commits.
  - **Workspace metadata in every response**: `summary` now includes `serverDir`, `workspaceStatus`, `workspaceSource`, `workspaceGeneration` in all tool responses.
  - **`xray_info` workspace section**: shows current workspace dir, mode, status, and generation.
  - **Event loop refactored**: parses stdin as `Value` first, routes requests vs responses. Handles server-initiated `roots/list` requests and client responses.
  - **`WORKSPACE_UNRESOLVED` gate**: workspace-dependent tools (`xray_grep`, `xray_definitions`, `xray_callers`, `xray_fast`, `xray_edit`, `xray_git_*`) are blocked with structured error. Workspace-independent tools (`xray_info`, `xray_help`, `xray_reindex`, `xray_reindex_definitions`) always work.
  - New dependency: `url = "2"` for `uri_to_path()` (file:// URI parsing with percent-encoding, drive letters, Unicode).
  - Backward compatible: `--dir /explicit/path` works exactly as before (PinnedCli mode, roots ignored).

- **`xray_definitions` cross-file hint** — When `name` + `file` filters return 0 results because the definition exists in a different file, the hint now suggests the correct file(s) instead of "Use xray_grep". Shows up to 3 matching files. Falls back to xray_grep suggestion when the name doesn't exist anywhere. Saves ~1 LLM tool call per session in architecture exploration scenarios.

- **MCP tool names renamed: `search_*` → `xray_*`** — All 16 MCP tool names renamed from `search_*` prefix to `xray_*` prefix for consistency with the product name (binary = `xray.exe`, MCP server = `xray`). Full mapping: `search_grep` → `xray_grep`, `search_definitions` → `xray_definitions`, `search_callers` → `xray_callers`, `search_edit` → `xray_edit`, `search_fast` → `xray_fast`, `search_find` → `xray_find`, `search_info` → `xray_info`, `search_help` → `xray_help`, `search_reindex` → `xray_reindex`, `search_reindex_definitions` → `xray_reindex_definitions`, `search_git_history` → `xray_git_history`, `search_git_diff` → `xray_git_diff`, `search_git_authors` → `xray_git_authors`, `search_git_blame` → `xray_git_blame`, `search_git_activity` → `xray_git_activity`, `search_branch_status` → `xray_branch_status`. All Rust handler functions, test functions, E2E tests, scripts, documentation, and `.roo/mcp.json` config updated. **Breaking change**: MCP clients using `search_*` tool names must update to `xray_*`.

### Internal

- **Transcript analyzer improvements (`analyze-transcript.ps1`)** — Five fixes and enhancements:
  1. **Kind mismatch detection** — detects when `term_breakdown` shows 0 results for names due to wrong `kind` filter and the model retries with different/removed `kind`. Tagged as `kind_mismatch` with details (requested_kind, zero_names, recovered_names). Recommendation auto-generated: "Consider multi-kind filter support (kind='class,interface') or kind mismatch hints."
  2. **Cleaned truncation cause noise** — removed `no_kind_filter` and `no_name_filter` from truncation causes (normal for exploration sessions). Fallback `response_size_limit (inferred)` no longer added when a specific cause (`definitions_truncated`, `body_truncation`) already exists.
  3. **Fixed dead regex** — `Analyze-TruncationCause` used `"totalDefinitions"` regex which never matched (actual field is `"totalResults"`). Replaced with `returned` vs `totalResults` comparison.
  4. **Token estimation fallback** — when MCP response doesn't include `estimatedTokens`, falls back to `responseSize / 4` approximation instead of 0.
  5. **Extended policy violation detection** — added `list_files` and `list_code_definition_names` as detectable policy violations (should use `xray_fast` and `xray_definitions` respectively when xray MCP is available).

- **`EditMode` enum in `xray_edit` handler** — Replaced `unreachable!()` panic in `apply_edits_to_content()` with compile-time type safety. Introduced `EditMode::Operations` / `EditMode::Edits` enum so the `match` is exhaustive — invalid state (both None) is now unrepresentable at the type level. Updated signatures of `apply_edits_to_content()`, `handle_single_file_edit()`, `handle_multi_file_edit()`. Zero behavioral change — all 1626 unit tests + 71 E2E tests pass.

- **SQL parser defensive coding (`parser_sql.rs`)** — Replaced all `.unwrap()` calls on regex capture groups with safe alternatives. Introduced `extract_schema_name()` helper for the common `(schema.name|name)` regex pattern (6 call sites). Remaining single-group captures use `match caps.get(N)` with early `return`/`continue`. Changed `PARAM_RE` capture from `.map(|c| c.get(1).unwrap())` to `.filter_map(|c| c.get(1))`. SQL parser now gracefully handles corrupted/malformed SQL without panicking. 9 new regression tests for corrupted SQL (truncated CREATE, missing names, garbled bodies, binary content, unmatched brackets).

- **Fixed 9 pre-existing compiler warnings** — 3 tests in `tips_tests.rs` were accidentally nested inside another test function (missing closing `}`) — now properly at module scope and actually run (test count: 1617 → 1626). 2 unused variable warnings in `definitions_tests.rs` fixed with `_` prefix.

- **Refactored god-functions `truncate_large_response` and `generate_zero_result_hints`** — Two high-cognitive-complexity functions decomposed into independently testable units:
  - **`truncate_large_response`** (utils.rs, CC=50, cognitive=179) → 7 extracted functions: `measure_json_size()` (replaces 7 inline `serde_json::to_string` calls), `phase_cap_lines_per_file()`, `phase_cap_matched_tokens()`, `phase_remove_lines_arrays()`, `phase_reduce_file_count()`, `phase_strip_body_fields()`, `phase_truncate_largest_array()`. Orchestrator is now a linear sequence of phase calls with early returns.
  - **`generate_zero_result_hints`** (definitions.rs, CC=62, cognitive=192) → 6 extracted functions returning `Option<String>`: `hint_unsupported_extension()`, `hint_wrong_kind()`, `hint_file_has_defs_but_filters_narrow()`, `hint_file_fuzzy_match()`, `hint_nearest_name()`, `hint_name_in_content_not_defs()`. Orchestrator uses `.or_else()` chain for explicit priority ordering.
  - Behavior-preserving: all 37 existing tests (12 truncation + 25 hints) pass without changes. Public API signatures unchanged.
  - 26 new unit tests: 11 for truncation phases + 15 for hint functions. All 1614 unit tests + 71 E2E tests pass.

### Bug Fixes
- **7 more bugs fixed from code audit, session 2 (2026-03-14)** — Continued systematic fix of audit findings:
  1. **[MEDIUM] `xray_edit` multi-file path dedup** — `paths: ["./foo.rs", "foo.rs"]` could resolve both to the same file, causing Phase 3 to write it twice (second write overwrites first). Added `HashSet<PathBuf>` dedup in Phase 1 with path normalization via `components().collect()`. Returns descriptive error showing both path variants.
  2. **[MEDIUM] `xray_edit` CRLF normalization in Mode A** — `parse_line_operations` content with `\r\n` combined with `write_file_with_endings`'s `replace('\n', "\r\n")` produced `\r\r\n`. Added `normalize_crlf()` call in `parse_line_operations` (Mode B already normalized).
  3. **[MEDIUM] `xray_definitions` sortBy='lines' skipped min* filters** — `needs_code_stats` was false when `sortBy='lines'`, causing ALL min* filters (minComplexity, minCognitive, etc.) to be bypassed. Split into `has_min_filters` (always true when any min* set) + original check.
  4. **[MEDIUM] `xray_callers` build_root_method_info early exit** — `?` operator on definition/file lookup silently returned None on tombstone indices. Replaced with `match/continue` (tries next index) and `match/return None` (invalid file_id).
  5. **[MEDIUM] `xray_callers` is_implementation_of false positives** — `IData` stem "Data" (4 chars) passed `< 4` threshold and matched `DataProcessor` via `contains()`. Raised threshold to `< 5`.
  6. **[MEDIUM] `xray_callers` impactAnalysis collection_limit** — `collection_limit = maxCallersPerLevel × 3` (default 30) could miss test callers. Set to `usize::MAX` when `impact_analysis=true`.
  7. **[MEDIUM] `xray_reindex_definitions` no rollback on poisoned lock** — If `def_index_arc.write()` failed, workspace stayed in Reindexing. Added rollback to `WorkspaceStatus::Resolved` before error return.

- **5 bugs fixed from code audit (2026-03-14)** — Systematic validation and fix of top findings from the 67-item code audit:
  1. **[HIGH] `xray_edit` multi-file write atomicity** — Multi-file `paths` mode now uses a two-phase write strategy: Phase 3a writes all edited content to `.xray_tmp` temp files; Phase 3b renames temp files to targets. If any temp write fails, all temp files are cleaned up and no originals are touched. Previously, I/O failure on file N+1 left files 1..N already written with no rollback.
  2. **[MEDIUM] Watcher no-op after `xray_reindex`** — `handle_xray_reindex` now rebuilds `path_to_id` (via `build_watch_index_from`) when the previous index had `path_to_id` set (indicating `--watch` mode). Previously, the new index had `path_to_id = None`, causing the watcher to silently skip all incremental updates.
  3. **[MEDIUM] `xray_fast` blocked by `content_ready`** — Removed `xray_fast` from `requires_content_index()`. `xray_fast` uses its own file-list index, not the content index, so it should not be blocked during content index build.
  4. **[MEDIUM] `xray_fast` unbounded response** — Added `maxResults` parameter (default: 0 = unlimited). Response includes `truncated: true` and `maxResults` in summary when truncation occurs. Schema updated.
  5. **[MEDIUM] `xray_edit` regex capture group cascade** — Replaced manual `$0`/`$1`/`$2` sequential replacement with `caps.expand()` (regex crate standard API). The manual loop could double-substitute when `$0` expansion text contained `$1` literal.
  - Also fixed 2 pre-existing test issues: `test_xray_info_response_structure` missing `file-list` type handler, `test_audit_cross_validate_no_file_index_returns_skipped` test isolation.


- **`xray_grep` dir= silently returned 0 results when pointing to a file** — When `xray_grep` was called with `dir` = path to a file (not a directory), it silently returned 0 results because `is_under_dir()` appends `/` to the dir prefix, making a file path like `parser_sql.rs/` match nothing. Now `parse_grep_args()` detects file paths via `Path::is_file()` (filesystem check) and `looks_like_file_path()` (heuristic for non-existent paths), returning an error with a helpful hint: try `dir='<parent_dir>'` or `xray_definitions file='<filename>'`. Tool description and `xray_help` parameter examples updated. 8 new unit tests + 1 E2E test.

### Features

- **xray_definitions: Multi-kind filter** — The `kind` parameter now supports comma-separated values for multi-kind OR filtering (e.g., `kind='class,interface,enum'`). Previously, searching for definitions of mixed kinds required separate queries or omitting the `kind` filter entirely. Now `kind='class,interface'` returns both classes and interfaces in a single call. Backward compatible — single values work as before. 4 new unit tests.
- **xray_definitions: Missing terms detection (`missingTerms`)** — When a multi-name query with a `kind` filter returns results but some terms are silently dropped due to kind mismatch, the response `summary` now includes a `missingTerms` array with `{term, reason}` for each dropped term (e.g., `"kind mismatch: found as method, not class"`). Previously, the LLM had no way to know that 2 of 4 terms produced 0 results when total results > 0. This eliminates 1-2 unnecessary round-trips per exploration session. 5 new unit tests.
- **xray_definitions: Name+kind mismatch hint** — When `name=X` + `kind=method/property/field/constructor` returns only type-level definitions (class/interface/struct), the response now includes a hint suggesting `parent=X` instead of `name=X`. This eliminates a common LLM confusion pattern where the model searches for class members using `name` instead of `parent`.
- **xray_definitions: File path fuzzy-match hint (Hint F)** — When `file` filter returns 0 results and no other hints fire, the server normalizes paths (removing slashes, dashes, underscores) and suggests the nearest matching file path. Catches cases like `file='Components/Utils'` when the actual path is `ComponentsUtils`.

All notable changes to **xray** are documented here.

Changes are grouped by date and organized into categories: **Features**, **Bug Fixes**, **Performance**, and **Internal**.

---

## 2026-03-13

### Bug Fixes
- **`xray_fast` created orphan file-list indexes for subdirectories** — When `xray_fast` was called with `dir` pointing to a subdirectory of the server's `--dir` (e.g., `dir="docs/design/rest-api"` while server runs on `C:/Projects/MyApp`), it auto-built a separate `.file-list` index for that subdirectory instead of reusing the parent directory's existing index. This created orphan index files in `%LOCALAPPDATA%/xray/` that were never cleaned up. Root cause: `load_index(dir)` only looked for an exact-path index match, with no fallback to the parent directory's index. Fix: three changes in `handle_xray_fast()`: (1) before auto-building, try loading the server_dir's index as a fallback when `dir` is a subdirectory (verified via `canonicalize` + `clean_path`); (2) add `subdir_entry_filter` path prefix to scope results to the requested subdirectory; (3) adjust `base_depth` for `maxDepth` to be relative to `dir` (not `index.root`) when the parent index is reused. External directories (outside server_dir) still auto-build as before. Found via real user report — LLM exploring `a deep subdirectory` created `an orphan `.file-list` index file`. 4 new unit tests + 1 E2E test (`T-FAST-SUBDIR`).

- **`xray_fast` `fileCount` always 0 when `dir` is a relative path** — When `xray_fast` was called with `dirsOnly=true` and a relative `dir` parameter (e.g., `dir=src`), the `fileCount` field for all directories was always 0. Root cause: the `dir_prefix` used for filtering files was computed from the raw `dir` argument (e.g., `"src/"`), but index entry paths are absolute (e.g., `"C:/Repos/project/src/..."`), so `starts_with("src/")` always failed. This forced LLMs to make N individual `countOnly=true` calls to determine directory sizes — a session analysis showed 12 sequential calls wasted on this pattern. Fix: `dir_prefix` now resolves against `index.root` for relative paths. Also added guard for `dir="."` edge case. Found via MCP transcript analysis (`roo_task_mar-13-2026_12-10-37-pm.md`, 22 progressive refinement chains). 2 new unit tests.

### Internal
- **Transcript analyzer improvements (`analyze-transcript.ps1`)** — Three new detection features:
  1. **Data quality complaints** — detects when the model explicitly flags data quality issues in thinking (e.g., "fileCount is showing 0"), tagged as `data_quality_complaint`
  2. **Forced enumeration chains** — detects N consecutive calls to the same tool where only `dir` changes (model iterating directory-by-directory due to missing aggregation), with specific chain details in recommendations
  3. **Incomplete session warning** — flags when session ends without `attempt_completion`

---

## 2026-03-12

### Features
- **`xray_edit` auto-creates new files** — `xray_edit` now treats non-existent files as empty files instead of returning "File not found" error. Insert operations (Mode A: `startLine: 1, endLine: 0`) succeed and create the file; search/replace operations fail naturally (no text to find in empty content). Parent directories are created automatically. Response includes `fileCreated: true` when a new file was created. This eliminates the need for `write_to_file` as a separate tool — LLMs can use `xray_edit` for both editing and creating files. For Mode B (text-match), search/replace and insertAfter/insertBefore still require the target text to exist, so they fail gracefully on empty files. Multi-file `paths` mode also supports mixed existing + new files. 6 new unit tests.

### Bug Fixes
- **`xray_definitions autoSummary` blocked `sortBy` queries** — When `xray_definitions` was called with `sortBy` (e.g., `sortBy='cognitiveComplexity'`) and no `name` filter, the `autoSummary` mode intercepted the results and returned a directory-grouped summary instead of the top-N sorted individual results. This caused LLMs to waste 3-4 calls trying different filter combinations before resorting to explicit `name` lists. Root cause: `should_auto_summary()` didn't check for `sort_by`. Fix: added `args.sort_by.is_none()` condition — when `sortBy` is set, individual ranked results are returned. Found via MCP transcript analysis (`roo_task_mar-13-2026_1-01-20-am.md`, episodes 26-29). 1 new unit test.

### Features
- **`xray_fast` directory enrichment: `fileCount`, `maxDepth`, sorting** — Three improvements to `xray_fast` for large repository exploration:
  1. **`fileCount` field** — When `dirsOnly=true` with wildcard pattern (`*`), each directory entry now includes `fileCount` — the total number of files recursively contained in that directory. Computed in O(N) via a single pass over the file index with ancestor directory counting.
  2. **Sorting by `fileCount`** — Wildcard `dirsOnly` results are sorted by `fileCount` descending (largest modules first), ensuring the most important directories appear at the top even when truncation occurs on large repos (10K+ directories).
  3. **`maxDepth` parameter** — New integer parameter limits directory depth (1=immediate children only). Eliminates truncation on large repos without losing overview capability.
  4. **Truncation hint** — When >150 directories are returned without `maxDepth`, the summary includes a hint recommending `maxDepth=1` or `xray_definitions file='<dir>'`.
  - Backward compatible: `fileCount` only added for wildcard+dirsOnly; non-wildcard queries unchanged. `maxDepth` is optional with unlimited default. 4 new unit tests. All 1562 unit tests + 68 E2E tests pass.

- **`xray_git_activity` path filter** — `xray_git_activity` now supports an optional `path` parameter to filter activity by file or directory. Previously, `path` worked only via the in-memory cache but was undocumented in the MCP schema and ignored in the CLI fallback path. Three fixes: (1) `path` added to MCP schema so LLMs discover the parameter; (2) CLI fallback passes `path` to `git log -- <pathspec>` for native git-level filtering (efficient — git itself filters commits); (3) tips updated with `path` example. Backward compatible — `path` is optional, omitting it returns whole-repo activity as before. 2 new unit tests. All 1551 unit tests pass.

- **`xray_definitions` auto-summary for broad queries** — When `xray_definitions` finds more results than `maxResults` and no `name` filter is set (and `includeBody` is false), it automatically returns a **directory-grouped summary** (`autoSummary`) instead of truncated entries. Each group shows: subdirectory name, total definition count, counts by kind (class, method, etc.), and top-3 largest classes/interfaces by line count. Includes a contextual `hint` with concrete subdirectory and class name suggestions. This eliminates the "map then read" pattern where LLMs needed `xray_fast dirsOnly=true` + multiple narrowing `xray_definitions` calls to explore large code modules. To get individual definitions, add a `name` filter or narrow the `file` scope. Updated Architecture Exploration strategy recipe and anti-patterns in LLM instructions. 11 new unit tests. All 1547 unit tests pass.

- **Dynamic `policyReminder` with indexed extensions** — `summary.policyReminder` in every successful MCP response now dynamically includes the server's `--ext` file extensions (e.g., `"Indexed extensions: rs, md. For other file types, use read_file or environment tools."`). This proactively prevents LLMs from calling xray tools for non-indexed file types (e.g., `.ps1`), which previously caused wasted round-trips with 0 results. When `--ext` is empty, the extensions line is omitted. 5 new unit tests.

- **`xray_grep` zero-result hint for non-indexed extensions** — When `xray_grep` returns 0 results and the `ext` filter targets a non-indexed extension (not in `--ext`), the response now includes a `summary.hint` explaining that the extension is not in the content index and suggesting `read_file`. Only fires when `ext` filter is explicitly set — no noise on generic zero-result searches. 5 new unit tests.

---


## 2026-03-11

### Features
- **Policy re-materialization for MCP anti-drift** — Successful JSON MCP tool responses now inject `summary.policyReminder` with a compact `SEARCH_INDEX_POLICY` reminder, and selected tools also inject `summary.nextStepHint` from a fixed dictionary. Guidance injection is independent from metrics, applies only to successful JSON responses, auto-creates `summary` when missing, skips non-JSON success responses, and leaves error responses unchanged. MCP `initialize.instructions` is now wrapped in `=== SEARCH_INDEX_POLICY ===` / `================================` so the agent sees a stable named policy anchor at session start. Response truncation preserves `summary.policyReminder` and `summary.nextStepHint`. Tests updated across handlers, utils, protocol, and tips.
- **Hint E: Unsupported file extension detection in `xray_definitions`** — When `xray_definitions` is called with a `file` filter containing an extension not supported by the definition index (e.g., `.xml`, `.json`, `.config`), a new Hint E fires before all other hints. It checks whether the extension is indexed by the content index and gives a targeted recommendation: either "Use xray_grep" (if in content index) or "Use read_file" (if not in any index). This prevents the common LLM pattern of calling `xray_definitions` for non-code files, getting 0 results with no guidance, and falling back to `read_file` instead of `xray_grep`. Guard: skipped when `def_extensions` is empty. 5 new unit tests.

- **Task routing for non-code files** — Added "Read/search non-code files (XML, JSON, config, YAML, MD, txt) → xray_grep" to the TASK ROUTING table in LLM instructions. Previously, non-code files had no explicit routing entry.

- **Anti-pattern for `xray_definitions` on non-code files** — Added "NEVER use xray_definitions for non-.cs/.rs/etc files — it only supports AST parsing for those extensions. Use xray_grep instead" to the ANTI-PATTERNS block. Dynamically lists supported extensions. Only emitted when `def_extensions` is non-empty. 4 new tips_tests.

- **Strengthened LLM tool routing instructions (B+C+D)** — Three prompt improvements to prevent LLM fallback to built-in tools:
  1. **DECISION TRIGGER for search_files** — Added "before calling search_files — STOP. Use xray_grep instead" (previously missing — LLMs defaulted to Roo Code's built-in `search_files`).
  2. **Concrete anti-pattern pairs** — Anti-pattern "NEVER read indexed source files directly" now includes `file='X' includeBody=true maxBodyLines=0` usage example.
  3. **xray_definitions tool description enhanced** — Added "REPLACES read_file for indexed source files" and "Only these extensions are indexed — for other file types use xray_grep" to the tool description. LLMs read tool descriptions at every tool-selection decision.

- **Strengthened `xray_edit` override to prevent LLM fallback to `apply_diff`** — Three changes to reduce the dominant failure mode where LLMs default to Roo Code's built-in `apply_diff` instead of `xray_edit`:
  1. **Tool description front-loaded** — `xray_edit` description now starts with "ALWAYS USE THIS instead of apply_diff, search_and_replace, or insert_content" (moved from the end). LLMs read tool descriptions at every tool-selection decision — front-loading the override matches the proven pattern from `xray_find`'s "[SLOW — USE xray_fast INSTEAD]".
  2. **ANTI-PATTERNS expanded** — Added "NEVER use apply_diff, search_and_replace, or insert_content for ANY file edit" to the anti-patterns block (previously only covered `list_files`/`directory_tree`/`list_directory`).
  3. **DECISION TRIGGER made concrete** — Changed from abstract "before ANY file edit" to naming exact built-in tools: "before calling apply_diff, search_and_replace, insert_content, or write_to_file (for edits) — STOP. Use xray_edit instead."
  4. **maxBodyLines=0 explained** — The EXAMPLE line now includes "(0=unlimited, returns full file)" to prevent LLMs from falling back to `read_file` when they need an entire file (LLMs didn't know `maxBodyLines=0` means unlimited).
  5. **Built-in file search anti-pattern** — Added "NEVER use built-in file search (regex/text search across files) when xray_grep is available" to ANTI-PATTERNS.
  - Root cause: Roo Code's system prompt explicitly tells the LLM to use `apply_diff` in its RULES section, which has higher authority than MCP instructions. These changes maximize the competing signal at every lever available to MCP servers (tool description, anti-patterns, decision trigger). Token budget raised from 1900 to 2000. 4 new unit tests. All 1520 unit tests + 67 E2E tests pass.

---

## 2026-03-10

### Features
- **xray_fast wildcard listing (`pattern='*'` and empty pattern with `dir`)** — `xray_fast` now supports wildcard listing: `pattern='*'` matches all entries, and `pattern=''` with a `dir` parameter also lists all entries in that directory. Use with `dirsOnly=true` to list subdirectories. Previously, `pattern='*'` did a literal search for the `*` character (matching 0 files), and empty pattern always returned an error. The error message for empty pattern without `dir` now includes "Do NOT fall back to built-in list_files or list_directory". Relevance ranking is skipped for wildcard queries. 5 new unit tests.

- **Fallback prevention: RESPONSE HINTS, ERROR RECOVERY, and strengthened ANTI-PATTERNS** — Six improvements to LLM instructions (`src/tips.rs`) to prevent fallback from xray MCP tools to built-in tools:
  1. **ZERO-RESULT HINTS → RESPONSE HINTS** — broadened from "0 results with a hint" to "ANY response with a hint (zero results, errors, warnings, or suggestions)" with explicit "Do NOT fall back to built-in tools" directive
  2. **ERROR RECOVERY** — new rule: when a xray tool returns an error, read the error message for hints, retry with suggested parameters, NEVER fall back to built-in tools
  3. **TASK ROUTING** — added "List files or subdirectories in a folder → xray_fast" mapping
  4. **ANTI-PATTERNS** — added explicit prohibition: "NEVER use list_files, list_directory, or directory_tree for ANY purpose when xray is connected"
  5. **Tool description** — `xray_fast` description now mentions wildcard support and "ALWAYS use this instead of built-in list_files, list_directory"
  6. **Parameter examples** — `xray_fast.pattern` examples updated with wildcard usage
- **Caller deprioritization: production callers before test callers** — When `xray_callers` truncates results by `maxCallersPerLevel` (default: 10), production callers now always appear before test callers. Previously, callers were returned in content index order (file_id), so a method with 10 test callers and 2 production callers would show only test callers, completely hiding production usage. Three improvements:
  1. **Test deprioritization** — callers sorted: non-test first, test last. Test detection uses file path heuristics (`_tests.rs`, `.test.ts`, `.spec.ts`, `/tests/`, `/test/`) and attribute markers (`#[test]`, `[Fact]`, `[Theory]`, `[TestMethod]`)
  2. **Popularity secondary sort** — within each group (production/test), callers sorted by "popularity" (total postings for the caller's name in content index, DESC). More-referenced callers appear first
  3. **impactAnalysis preservation** — when `impactAnalysis=true`, test callers NOT truncated (needed for `testsCovering`). Only non-test callers subject to `maxCallersPerLevel`
  - Safety cap: collection phase uses `maxCallersPerLevel × 3` for sort headroom
  - 16 new unit tests. All 1506 unit tests + 66 E2E tests pass.

- **LLM guidance improvements based on session analysis** — Six improvements to reduce LLM tool call waste when exploring codebases. Based on analysis of a real session log where an LLM made 9 tool calls (5 directory listings + 2 xray_definitions) instead of the optimal 1-2 calls:

- **Cross-index enrichment Phase 1: `includeUsageCount` + `includeGrepReferences`** — Two new optional parameters that enrich tool responses with data from other indexes, saving 1-3 LLM round-trips per task:
  1. **`includeUsageCount` in `xray_definitions`** — adds `usageCount` to each definition — the number of files in the content index containing this name as a token. Useful for dead code detection (`usageCount=0` or `=1`). O(1) HashMap lookup per definition, zero latency overhead. Counts all text occurrences (including comments/strings). Default: false.
  2. **`includeGrepReferences` in `xray_callers`** — adds `grepReferences[]` to the response — files containing the method name as text but NOT present in the AST-based call tree. Catches delegate usage, method groups, reflection, and other patterns invisible to call-site analysis. Skipped for method names shorter than 4 characters to avoid noise. Each entry has `file` + `tokenCount`. Includes `grepReferencesNote` explaining limitations. Default: false.
  - Both parameters optional, default=false — zero overhead for existing queries. 7 new unit tests.

- **Nearest-match hints for `xray_callers`** — When `xray_callers` returns an empty call tree, the response now includes contextual nearest-match hints (using Jaro-Winkler similarity ≥75%) to help LLM agents self-correct typos instead of blind retries. Two hint types: (A) **Method name** — `"Method 'ProcessOrdr' not found. Nearest match: 'processorder' (similarity 95%)"`. Only fires when the method name doesn't exist in the definition index. (B) **Class name** — `"Class 'OrderServise' not found. Nearest: 'orderservice' (similarity 96%)"`. Only fires when the class name doesn't exist (pre-filtered to class/interface/struct kinds). Falls back to the existing generic hint ("try without class parameter") when both names exist but no call sites are found. 3 new unit tests.

- **`containsLine` body optimization** — When `containsLine` is used with `includeBody=true`, body is now emitted **only for the innermost (most specific) definition**. Parent definitions (e.g., the containing class) receive a `bodyOmitted` hint instead of consuming the body budget with potentially hundreds of lines. This eliminates a common UX issue where LLMs needed 3+ retries to get a method's body via `containsLine` because the parent class body consumed the entire `maxTotalBodyLines` budget. Single-match results (no parent) are unaffected. No new parameters — behavior change is automatic in `containsLine` mode. 3 new unit tests.
- **Body truncation size hint (`totalBodyLinesAvailable`)** — When `xray_definitions` or `xray_callers` truncate body output due to `maxTotalBodyLines` or `maxBodyLines` budget, the summary now includes `totalBodyLinesAvailable` — the total body lines that would have been returned without truncation. This eliminates "blind retry" (LLM doesn't know what `maxTotalBodyLines` value to use) and "unnecessary retry" (LLM retries when only 10 of 510 lines were truncated). The field is only present when `totalBodyLinesReturned < totalBodyLinesAvailable`. Also added `totalBodyLinesReturned` to single-method `xray_callers` responses (previously only in multi-method batch). 3 new unit tests.

  1. **ANTI-PATTERNS section in MCP instructions** — Added 4 top anti-patterns directly in `render_instructions()` output (seen at every session start), including "NEVER browse directories to explore code structure" and "Use excludeDir to skip test files"
  2. **New task routing: "Explore a module"** — Added explicit routing "Explore a module / understand directory structure → xray_definitions" to the TASK ROUTING table
  3. **Architecture Exploration recipe: excludeDir** — Step 1 now includes `excludeDir=['test','Test','Mock']` and hints at `file='<dirname>'` for directory-level exploration
  4. **xray_fast empty-pattern guidance** — Error message now suggests `xray_definitions file='<dir>'` for code exploration and `pattern='*'` for file listing
  5. **xray_definitions file parameter enriched** — Tool schema description now says "Use file='<dirname>' to explore an entire module"
  6. **New anti-pattern in Architecture Exploration strategy** — "Don't browse directories (list_files, list_directory, xray_fast with empty pattern)"

  All changes are static strings — zero runtime cost. All 1474 unit tests + 66 E2E tests pass.
### Bug Fixes
- **`xray_fast` `dirsOnly` + `ext` filter returned 0 results** — When `xray_fast` was called with `dirsOnly=true` and `ext="cs"`, it returned 0 results because the `ext` filter was applied to directory entries, which have no file extension. This was the root cause of suboptimal LLM queries observed in session analysis where `xray_fast pattern="Dlp" ext="cs" dirsOnly=true` returned 0 results. Fix: `ext` filter is now skipped when `dirsOnly=true`. Response includes `summary.hint: "ext filter ignored when dirsOnly=true (directories have no file extension)"`. Tool schema description updated. 4 new unit tests. All 1487 unit tests + 66 E2E tests pass.

### Internal
- **`format_version` for ContentIndex and DefinitionIndex** — Added `format_version: u32` field (with `CONTENT_INDEX_VERSION` / `DEFINITION_INDEX_VERSION` constants) to both index types, following the existing pattern in `GitHistoryCache`. Version is validated **before full deserialization** via lightweight `read_format_version_from_index_file()` which reads ~100 bytes (1 LZ4 block) — preventing OOM/process abort that occurred when old indexes with shifted binary layout caused bincode to attempt multi-TB allocations for garbled Vec lengths. As a defense-in-depth, `load_compressed` also uses `bincode::Options::with_limit(2GB)` to cap deserialization size. Field is placed after `root` in the struct to preserve compatibility with `read_root_from_index_file()`. 8 new unit tests (including old-format crash regression test).
- **Fix `.unwrap()` in `is_implementation_of()`** — Replaced `interface_name.chars().nth(1).unwrap()` with safe `match` pattern using `strip_prefix('I').and_then(|s| s.chars().next())`. While the panic was unreachable in practice (guard + ASCII-only identifiers), the fix is idiomatic Rust. 1 new edge-case test.


---

## 2026-03-09

### Features
- **Dynamic `xray_help` based on active extensions (Phase 2)** — `xray_help` output (tips, tool priority, parameter examples) now dynamically reflects the languages configured via `--ext`. `Tip` struct fields migrated to `Cow<'static, str>` (zero-cost for 24 of 25 static tips). Tip #16 "Language scope" shows the actual language list (e.g., "AST = Rust" for `--ext rs`) instead of generic "languages with definition parser support". Tool priority ranks 1-2 (`xray_callers`, `xray_definitions`) show concrete language lists. All five public functions (`tips()`, `tool_priority()`, `parameter_examples()`, `render_json()`, `render_cli()`) now accept `def_extensions: &[String]`. MCP `handle_xray_help()` passes `ctx.def_extensions`. CLI `tips` command passes all compiled extensions via `definition_extensions()`. `ToolPriority.description` migrated to `Cow<'static, str>`. 14 existing tests updated with new signatures. All 1474 unit tests + 66 E2E tests pass.

- **Code Review / Story Evaluation strategy recipe** — Added 7th strategy recipe to `xray_help` output for the common task pattern of reviewing PRs, evaluating user stories, and assessing code change feasibility. The recipe recommends 3 xray calls: (1) `xray_definitions file='...' includeBody=false` to understand current architecture, (2) `xray_definitions name='...' includeBody=true` to validate specific code, (3) `xray_grep countOnly=true` to verify pattern scale. Includes 3 anti-patterns discouraging `read_file` for architecture understanding, occurrence counting, and function existence checks. Automatically included in MCP `initialize` instructions via `render_instructions()`. All 1474 unit tests + 66 E2E tests pass.

- **Dynamic tool descriptions based on active extensions** — Tool descriptions for `xray_definitions`, `xray_callers`, and `xray_reindex_definitions` now dynamically reflect the languages configured via `--ext` instead of hardcoding "C# and TypeScript/TSX". When the server is started with `--ext rs --definitions`, the LLM sees "Supports Rust" instead of "Supports C# and TypeScript/TSX" — eliminating the root cause of LLMs ignoring xray tools for non-C#/TS projects. SQL is correctly labeled as "regex-based parser" (not tree-sitter). Empty extensions produce "not available" descriptions. `HandlerContext` gains `def_extensions: Vec<String>` computed as the intersection of `--ext` and compiled parser support. New `format_supported_languages()` utility separates tree-sitter languages from regex-based. `render_instructions()` adds a concrete EXAMPLE line showing how to replace file reading with `xray_definitions`. CLI `--help` text also dynamicized (removed hardcoded "tree-sitter"). 3-round self-review audit found and fixed 12 bugs (including `initialize`/`tools/list` inconsistency, SQL-only edge case, hardcoded language lists in tips/strategies/tool_priority). 26 new unit tests + 2 new server integration tests. All 1462 unit tests + 66 E2E tests pass.

- **Zero-result hints for `xray_definitions`** — When `xray_definitions` returns 0 results, the response `summary` now includes a contextual `hint` field to help LLM agents self-correct common mistakes instead of blind retries or `read_file` fallback. Four hint types (first matching wins): (A) **Wrong kind** — definitions exist with same name/file but different kind (e.g., `kind='method'` in Rust where standalone functions use `kind='function'`); (B) **Nearest name** — typo/wrong name, suggests closest match by Jaro-Winkler similarity (≥80% threshold); (C) **File has definitions** — file matches but name/kind/parent filters are too narrow, suggests `xray_grep` for content search; (D) **Name in content index** — name exists as text content but not as an AST definition name, redirects to `xray_grep`. Zero overhead for successful queries (hints only generated at 0 results). Existing `kind='property'→'field'` TypeScript hint preserved. New `name_similarity()` utility in `utils.rs` (Jaro-Winkler via `strsim` crate). New `file_matches_filter()` helper. 15 new unit tests. All 1423 unit tests + 66 E2E tests pass. New dependency: `strsim = "0.11"` (pure Rust, 0 transitive deps, ~600 lines).
### Features
- **Task Routing table in MCP instructions** — Replaced three redundant instruction sections (CRITICAL block, Quick Reference, Tool Priority) with a single auto-generated TASK ROUTING table. The table maps user tasks to recommended tools (task-first framing: "Read source code → xray_definitions") and is context-aware: definition-dependent routes (xray_definitions, xray_callers) are filtered out when `def_extensions` is empty. Added fallback rule for uncertainty: "If unsure whether a file type is supported, use xray_info or xray_grep first." Instructions reduced from ~1800 to ~1200 tokens. DECISION TRIGGERs for file reading and editing are preserved (shortened). 12 new unit tests including tool-name validation between routing table and tool_definitions(). Golden scenarios added to E2E test plan for manual behavioral validation.
- **Routing hints in tool descriptions** — Added "Preferred for..." routing hints to `xray_definitions` and `xray_grep` tool descriptions. These hints are read by LLMs at every tool-selection decision point — the most reliable surface for influencing tool choice. `xray_fast` and `xray_edit` already had equivalent hints.
- **Auto-correction for `xray_definitions` zero-result queries** — When `xray_definitions` returns 0 results, the server now automatically attempts to correct the query and return results in a single round-trip, instead of relying on the LLM to follow a hint. Two auto-correction types: (A) **Kind mismatch** — if `kind` filter is set with `name` or `file` filter, removes the kind filter to discover the correct kind, then re-runs with it (e.g., `kind='method'` on Rust code auto-corrects to `kind='function'`). Requires `name` or `file` filter to be present. (B) **Nearest name match** — if name produces 0 results and the closest name in the index has ≥85% Jaro-Winkler similarity, re-runs with the corrected name (e.g., `name='hndl_search'` auto-corrects to `name='handle_xray_find'`). Skipped for regex queries. Both corrections inject an `autoCorrection` object in the response summary with `type`, `original`, `corrected`, `similarity` (for name), and `reason` fields. All other filters (file, parent, excludeDir, stats) are preserved during correction. If auto-correction produces 0 results, falls through to the existing hint system. 10 new unit tests. All 1433 unit tests + 66 E2E tests pass.
- **Expanded zero-result hint auto-follow in LLM instructions** — Strengthened the ZERO-RESULT HINTS rule in `render_instructions()` to cover two previously missing hint types: NEAREST MATCH (re-call same tool with corrected name) and KIND MISMATCH (re-call with suggested kind). Added hard rule: "NEVER ask the user whether to follow a hint." 3 new test assertions.

### Bug Fixes
- **Server startup failure when adding extensions via `--ext`** — Adding a new extension (e.g., `md` to `--ext rs`) in mcp.json crashed the server. **Root cause:** `--ext` was a single-value string parameter. In mcp.json, `["--ext", "rs", "md"]` passed `md` as a separate positional argument, which clap rejected as an unknown argument. **Fix:** Changed `--ext` to accept multiple space-separated values via `num_args = 1..`. Now both `["--ext", "rs", "md"]` and `["--ext", "rs,md"]` work correctly. Comma-separated values within each argument are also split and flattened (e.g., `--ext rs,md ts` → `["rs", "md", "ts"]`).

- **Slow startup when changing extensions (performance)** — When changing extensions (e.g., `rs` → `rs,md`), `find_content_index_for_dir()` and `find_definition_index_for_dir()` fully deserialized **every** cached index file (potentially 500+ MB each) on the main thread before the MCP event loop started. **Fix:** Both functions now use `.meta` sidecar files (~200 bytes) to check `root` and `extensions` without loading the full index. Fallback to lightweight `read_root_from_index_file()` (~100 bytes) when no sidecar exists. Startup overhead reduced from ~660 MB deserialization to ~4 KB metadata reads.

- **Auto-cleanup of stale same-root indexes** — When the server builds a new index after extension change, old index files for the same root directory with different extension hashes are automatically deleted. Prevents disk space accumulation from orphaned indexes. New `cleanup_stale_same_root_indexes()` helper called from `serve.rs` background build threads. 8 new unit tests (3 meta content + 3 meta definition + 2 cleanup). All 1474 unit tests + 66 E2E tests pass.

- **Auto-correction length ratio guard** — Fixed false auto-corrections in `xray_definitions` where partial name matches (e.g., `name="xray_definitions"` → `"search"`) were incorrectly treated as typos due to Jaro-Winkler inflating similarity for shared prefixes (87% similarity, but only 33% length ratio). Added `AUTO_CORRECT_MIN_LENGTH_RATIO = 0.6` constant: auto-correction now requires both ≥80% similarity AND ≥60% length ratio (shorter/longer). Short typos (`"GetUsr"` → `"getuser"`, ratio 86%) and similar-length typos (`"UserServise"` → `"userservice"`, ratio 100%) still auto-correct correctly. Without this fix, the zero-result hint system was bypassed by the false correction, preventing the LLM from receiving the correct hint (e.g., "use xray_grep for content search"). 4 new unit tests. All 1470 unit tests + 66 E2E tests pass.

- **`xray_edit` — CRLF normalization and trailing whitespace auto-retry** — Three improvements to eliminate the most common `xray_edit` false-negative failure mode ("Text not found" at 100% similarity):
  - **Part A (bug fix)**: All text fields in `xray_edit` edits (`search`, `replace`, `insertAfter`, `insertBefore`, `content`, `expectedContext`) now have CRLF line endings normalized to LF before matching. Previously, `read_and_validate_file()` normalized the file content to LF, but the search text from JSON input was used as-is — if the client/LLM sent `\r\n`, exact match was guaranteed to fail.
  - **Part B (UX improvement)**: When a literal text search or anchor lookup finds 0 matches, `xray_edit` now automatically retries with trailing whitespace stripped from each line of the search text. If the trimmed search succeeds, the edit is applied with a `"warnings"` array in the response (e.g., `"edits[0]: text matched after trimming trailing whitespace"`). This eliminates the most common LLM failure mode — invisible trailing spaces added by the model. Auto-retry is skipped for regex mode (trailing whitespace changes regex semantics) and when the trimmed text is empty (prevents `str.matches("")` infinite matches).
  - **Part C (diagnostics)**: When `nearest_match_hint` reports ≥99% similarity, the error now includes a byte-level diff showing the first divergent byte (e.g., `"First difference at byte 47: search has 0x20 (space), file has 0x0A (newline)"`). Also reports length differences (e.g., `"Search text is 3 byte(s) longer than file text"`). Helps diagnose invisible whitespace issues when auto-retry doesn't apply.
  - Self-review found and fixed 2 additional bugs: (1) `expectedContext` field was not CRLF-normalized, (2) all-whitespace search text after trimming could cause empty-string match with infinite results.
  - New response field: `"warnings"` array on single-file and multi-file responses when auto-retry fires.
  - 22 new unit tests. All 1400 unit tests + 65 E2E tests pass.

---

## 2026-03-08

### Features
- **`bodyLineStart`/`bodyLineEnd` parameters for `xray_definitions` and `xray_callers`** — New input parameters that filter the returned body to a precise absolute file line range. Solves the problem where `includeBody=true` with `maxBodyLines=0` on large methods (300+ lines) exceeded the response size budget (~64KB), causing the truncation engine (Phase 5a) to strip ALL body fields entirely. Now the caller can request just the lines they need: `xray_definitions file='Test.cs' containsLine=1335 includeBody=true bodyLineStart=1330 bodyLineEnd=1345` returns only 15 lines instead of 363. For `xray_callers`, the filter applies only to `rootMethod` body (caller node bodies are unaffected). Doc comment expansion (`includeDocComments`) is automatically skipped when `bodyLineStart` is set — the user wants a precise range, not expanded content. Edge case: when the requested range is completely outside the method's line range, returns an empty body array (no panic). 8 new unit tests + 3 integration tests. All 1381 unit tests + 65 E2E tests pass.

- **Multi-method batch in `xray_callers`** — The `method` parameter now accepts comma-separated method names (e.g., `"GetUser,SaveOrder,ValidateInput"`). Each method gets an independent call tree with its own `maxTotalNodes` budget. Body budget (`maxTotalBodyLines`) is shared across all methods. Single method returns the existing format (`{callTree: [...]}`); multiple methods return `{results: [{method, callTree, nodesInTree}, ...]}`. Response budget auto-scales: `max(base, 32KB × N)`, capped at 128KB. Truncation system (Phase 5a) strips bodies from nested `results[].callTree` before truncating entire results. 7 new unit tests + 1 E2E test (`T-MULTI-METHOD`). All 1373 unit tests pass, zero regressions.

### Internal
- **Char-safe truncation in `extract_semantic_prefix`** — Replaced `combined[..MAX_PREFIX_LEN]` byte-slice with `.chars().take(MAX_PREFIX_LEN).collect()` for consistency with `sanitize_for_filename` style. No behavioral change (inputs are always ASCII after sanitization), but eliminates a potential future panic if the sanitization pipeline changes.

### Performance
- **Non-blocking incremental updates for content and definition indexes** — MCP requests are no longer blocked for 12-35 seconds during `git pull` / `git checkout`. Both `update_content_index()` and `update_definition_index()` in the watcher now perform heavy I/O (file reading, tokenization, tree-sitter parsing) OUTSIDE the write lock, holding it only for the brief apply phase. Content index: new `tokenize_file_standalone()` and `apply_tokenized_file()` building blocks move file I/O and tokenization outside the lock. Definition index: `parse_file_standalone()` (already existed) now called outside the write lock. Content startup reconciliation (`reconcile_content_index()`) restructured into 4 phases: FS walk (no lock) → determine changes (read lock) → tokenize (no lock) → apply (write lock). Write lock duration: content from `500ms + N × 5ms` → `500ms + N × 0.1ms`; definition from `N × 30ms` → `N × 0.1ms`. For git pull with 300 files: total MCP blocking reduced from ~12s to ~560ms. `content_ready` flag no longer set to false during reconciliation — MCP requests work on old data. 9 new unit tests. All 1366 unit tests + 64 E2E tests pass.

---

## 2026-03-07

### Performance
- **Chunked build for peak memory reduction (~1 GB)** — Both `build_definition_index()` and `build_content_index()` now process files in macro-chunks of 4096 instead of all-at-once. Definition build: outer loop splits files into macro-chunks, each parsed by N threads in parallel; parse results are merged and freed after each macro-chunk. Content build: `drain(..4096)` loop incrementally moves file contents out of the master Vec, freeing String heap allocations after each chunk is tokenized and merged. `force_mimalloc_collect()` called between chunks to return freed memory to OS. Projected peak memory reduction: ~1.0–1.2 GB for 65K files (from ~4065 MB to ~2800–3000 MB). Build time impact: ≤1% (128 thread spawns + 16 drain memmoves = ~13ms). Output is identical: same definitions, call sites, code stats, tokens, postings, file_ids. 5 new unit tests verify chunked build correctness and single-vs-multi-thread consistency.

### Internal
- **Join-based streaming merge in `build_definition_index()`** — Refactored the parallel parsing + merge pipeline to use join-based streaming instead of collect-all-then-merge. Previously, all thread results were collected into a `Vec<ChunkResult>` via `.map(join).collect()`, then merged in a separate loop. Now each `JoinHandle` is joined and merged immediately inside `thread::scope`, freeing each chunk's memory before processing the next. Extracted `merge_chunk_result()` helper function (reusable for future callers). Added per-chunk `log_memory` diagnostics for memory profiling. Removed unused `ChunkResult` type alias. Note: real peak memory savings are modest (~100-200 MB for 8 threads) because JoinHandles hold thread results regardless — the main benefit is cleaner architecture and incremental memory diagnostics. Content index (`build_content_index`) deliberately NOT changed — its `drop(file_data)` before merge pattern is optimal because `file_data` is borrowed by scope. All 1352 unit tests + 64 E2E tests pass.

### Performance
- **Memory trim after `xray_reindex` / `xray_reindex_definitions`** — Both reindex MCP handlers now use the same drop-reload-mi_collect pattern as the server startup code (`serve.rs`). After building and saving the new index, the build result is dropped, `mi_collect(true)` is called, and the index is reloaded from disk for compact memory layout. A second `mi_collect` call after replacing the old index releases its freed pages to the OS. Previously, the reindex handlers did a simple in-place replacement without calling `force_mimalloc_collect()`, causing Working Set to grow by ~0.5–1 GB and never return to baseline. Memory logging (`log_memory`) added to both handlers for debug-mode verification.
- **Lock-free definition index reconciliation** — During watcher startup reconciliation, file parsing now happens OUTSIDE the write lock. Previously, the entire reconciliation (FS walk + parsing + index update) ran under a single write lock, blocking all `xray_definitions`/`xray_callers` requests for up to 96 seconds. Now: Phase 1 (FS walk, ~3s) and Phase 3 (parsing, ~12-93s) run without any lock. Only Phase 4 (applying results, <500ms) holds a write lock. MCP requests work on old index data during parsing — users won't notice reconciliation. Parallel parsing with `thread::scope` (1 parser per thread) provides ~8× speedup for Phase 3. 15 new unit tests.

### Bug Fixes
- **Extension methods lost during incremental C# updates** — `update_file_definitions()` discarded the 4th return value (`extension_methods`) from `parse_csharp_definitions()` as `_ext`, meaning extension methods were never updated during watcher debounce or reconciliation. Now correctly merged into `index.extension_methods`. Affects `xray_callers` extension method resolution for incrementally-updated files. 1 new regression test.

### Features
- **Atomic index save (crash-safe)** — `save_compressed` now writes to a `.tmp` file first, then renames over the target. If the process is killed mid-write, the original index file survives intact. Previously, `File::create` truncated the existing file before writing — a crash mid-write left a corrupt cache, forcing a full rebuild on next startup (96+ seconds for large repos). 2 new unit tests.

- **Watcher readiness flags during reconciliation** — The file watcher now resets `content_ready`/`def_ready` flags before long reconciliation runs. MCP requests (`xray_definitions`, `xray_callers`, etc.) receive an instant "building, please retry" message instead of blocking for up to 52 seconds on the write lock. Previously, the readiness flags were only used during initial index loading, not during watcher reconciliation.

- **Periodic autosave (every 10 minutes)** — The watcher thread now saves both content and definition indexes to disk every 10 minutes, protecting against data loss from forced process termination (e.g., VS Code killing the MCP server without graceful shutdown). Uses READ locks only — MCP queries are NOT blocked during save. 3 new unit tests.

- **Shutdown debug logging** — Debug-log (`--debug-log`) now records memory metrics at shutdown start and completion, enabling post-mortem analysis of whether graceful shutdown was reached.


- **UX improvements from user feedback (5 changes)** — Five UX improvements based on a code review session feedback:
  1. **`xray_grep` regex + spaces warning** — When `regex=true` and the terms contain spaces (e.g., `"private.*double Percentile"`), the response now includes a `searchModeNote` explaining that regex operates on individual index tokens which never contain spaces. Saves users from silent 0-result confusion.
  2. **`xray_edit` sequential edit hint** — When an occurrence-based edit fails because a previous edit in the same batch reduced the occurrence count, the error message now includes a hint: "edits are applied sequentially — previous edits may have modified the content". Only shown when `edit_index > 0`.
  3. **`truncate_large_response` Phase 5a — strip bodies before truncation** — New intermediate truncation phase that strips `body`/`bodyStartLine`/`bodyTruncated`/`totalBodyLines`/`docCommentLines` fields from array entries before truncating entire entries. Preserves method signatures/metadata in more results. Recursive: handles nested `callers`/`callees`/`children`. Sets `summary.bodiesStrippedForSize=true` when active.
  4. **`xray_callers` `callSites` array** — When a method is called multiple times within the same caller method, all call site lines are now collected and returned in a `callSites` array (e.g., `callSites: [273, 475, 486]`). The existing `callSite` field is preserved for backward compatibility (= first element). `callSites` array only included when >1 call site (saves tokens).
  5. **`xray_help` tip about `using static`** — New tip explaining that `xray_definitions` searches AST definition names, so methods imported via C# `using static` should be searched without `parent` filter or with `parent='DefiningClass'`.
  - 12 new unit tests. All 1327 tests pass. 64 E2E tests pass.

---

## 2026-03-06

### Features

- **`xray_edit` UX improvements — nearest match hints and skippedDetails** — Two diagnostic improvements to the `xray_edit` MCP tool:
  - **Nearest match hint on "text not found" errors** — When text, regex pattern, or anchor text is not found, the error message now includes a hint showing the most similar line in the file with its line number and similarity percentage (e.g., `Text not found: "Девять "израильтян"". Nearest match at line 2 (similarity 92%): "Девять «израильтян»"`). Uses char-level LCS ratio via the `similar` crate. Supports multi-line search with sliding window. Skipped for files > 500KB. Minimum similarity threshold: 40%. Helps LLM agents diagnose Unicode quote mismatches, case differences, and whitespace issues — eliminates 3-5 blind retry attempts.
  - **`skippedDetails` in response for `skipIfNotFound`** — When edits are skipped via `skipIfNotFound=true`, the response now includes a `skippedDetails` array with `editIndex`, `search` text, and `reason` for each skipped edit (in addition to the existing `skippedEdits` count). Enables LLM agents to understand exactly which edits were skipped and why, instead of just seeing a count.
  - 10 new unit tests (6 for nearest match, 4 for skippedDetails). All 1317 tests pass.

- **`xray_edit` improvements — multi-file, insert after/before, expectedContext** — Three enhancements to the `xray_edit` MCP tool:
  - **Multi-file editing (`paths` parameter)** — New `paths` array parameter (mutually exclusive with `path`) applies the same edits/operations to multiple files in a single call. Transactional semantics: if any file fails validation or editing, none are written (all-or-nothing). Max 20 files per call. Response includes per-file `results` array and `summary` object with `filesEdited` and `totalApplied` counts.
  - **Insert after/before (`insertAfter`/`insertBefore`)** — New Mode B edit variant for inserting content relative to anchor text without replacing it. `{insertAfter: "using System.IO;", content: "using System.Linq;"}` inserts on the next line after the anchor. `insertBefore` inserts on the line before. Mutually exclusive with `search`/`replace`. Supports `occurrence` for targeting Nth match.
  - **Expected context safety (`expectedContext`)** — New per-edit safety check for Mode B. Verifies that a given text exists within ±5 lines of the match before applying the edit. Prevents editing the wrong match in files with many similar patterns (e.g., multiple `SemaphoreSlim` instances).
  - **Skip if not found (`skipIfNotFound`)** — New per-edit boolean flag. When `true`, silently skips the edit if search/anchor text is not found (instead of aborting the entire operation). Essential for multi-file `paths` where not all files contain the target text. Default: `false` (preserves existing error behavior).
  - **Append mode documented** — Documented existing append capability via Mode A insert: `{startLine: N+1, endLine: N, content: "appended"}` where N is the file's line count.
  - Refactored handler into composable functions: `read_and_validate_file()`, `apply_edits_to_content()`, `write_file_with_endings()`, `handle_single_file_edit()`, `handle_multi_file_edit()`. 56 unit tests (was 27).

- **`xray_edit` MCP tool — reliable file editing** — New MCP tool providing atomic file editing with two modes: **Mode A (line-range operations)** — replace, insert, or delete lines by line number, applied bottom-up to avoid cascade offset failures that plague `apply_diff`; **Mode B (text-match edits)** — find-and-replace with literal or regex patterns, optional occurrence targeting. Returns unified diff (via `similar` crate). Supports `dryRun` for preview without writing, `expectedLineCount` safety check for stale line numbers, CRLF preservation, binary file detection, and both absolute and relative paths. Works on any text file (not limited to `--dir`). Tool count: 15 → 16 (at budget limit). 27 unit tests + 1 E2E test. New dependency: `similar = "2"` (lightweight, ~50KB, zero transitive deps).

---

## 2026-03-03

### Internal

- **Code review small fixes** — Four low-risk code hygiene improvements from full codebase review:
  - `#[non_exhaustive]` on `SearchError` enum (`src/error.rs`) — semver-safe for future error variant additions
  - `#[must_use]` on `ToolCallResult` struct (`src/mcp/protocol.rs`) — compiler warns if handler results are accidentally discarded
  - Extracted `get_pmc()` helper in `src/index.rs` — deduplicates the 15-line Windows FFI `ProcessMemoryCounters` init + call block shared between `log_memory()` and `get_process_memory_info()`
  - Renamed `CommitInfo` → `CachedCommit` in `src/git/cache.rs` — disambiguates from `git/mod.rs::CommitInfo` (different struct with different fields: string date vs i64 timestamp, optional patch vs subject-only)

---

## 2026-03-01

### Features

- **`includeDocComments` in `xray_definitions` and `xray_callers`** — New parameter that expands body output upward to capture doc-comment blocks above definitions. Supports `///` XML doc comments (C#/Rust) and `/** */` JSDoc blocks (TypeScript/JavaScript). Implies `includeBody=true` — no need to specify both. Response includes `docCommentLines` field showing how many lines are doc-comments. Budget-aware: doc-comment lines count against `maxBodyLines` and `maxTotalBodyLines`. Works in all body injection paths: `xray_definitions` (normal search, containsLine mode), `xray_callers` (caller tree nodes, callee tree nodes, rootMethod). Skips blank lines between comment and declaration, stops at first non-comment line, and does NOT capture comments separated by code. 13 new unit tests. Self-review caught and fixed a bug where `build_root_method_info` was not passing the `include_doc_comments` flag.

---

## 2026-02-28

### Features

- **`impactAnalysis` in `xray_callers`** — New parameter that answers "if I change this method, which tests will break?" in a single call. When `impactAnalysis=true` with `direction=up`, the caller tree traversal identifies test methods via attribute detection ([Test], [Fact], [Theory], [TestMethod] for C#; #[test]/#[tokio::test] for Rust) and file-name heuristics (*.spec.ts, *.test.ts for TypeScript). Test methods are marked with `isTest: true` in the call tree and collected in a `testsCovering` summary array with: full file path (for direct `read_file`/`dotnet test --filter`), `depth` (distance from target — depth 1 = direct test, depth 4+ = transitive via helpers), and `callChain` (array of method names from target to test — enables LLM to assess relevance by reading the intermediate call path). Recursion stops at test methods. Works with all existing `xray_callers` features (DI resolution, interface expansion, includeBody). Returns error if used with `direction=down`. 16 new unit tests.

- **`includeBody` in `xray_callers`** — Each node in the call tree can now include the method's source code inline via `includeBody=true` parameter. Supports `maxBodyLines` (default: 30) and `maxTotalBodyLines` (default: 300) for budget control. Works for both `direction=up` (callers) and `direction=down` (callees). Reuses the existing `inject_body_into_obj()` function from `xray_definitions`. When the total body budget is exceeded, remaining nodes get `bodyOmitted` instead of `body`. Eliminates the need for a separate `xray_definitions` call to read caller source code after getting the call tree — one call instead of two. 8 new unit tests.

- **Global 64KB response budget for `includeBody=true`** — When any tool (`xray_callers` or `xray_definitions`) is called with `includeBody=true`, the response byte budget is automatically increased from 16KB to 64KB. This prevents premature truncation of body-rich responses by the progressive truncation mechanism. Tools without `includeBody` continue to use the default 16KB budget. Also retroactively fixes `xray_definitions` with `includeBody=true` which could be truncated on large result sets.

- **`xray_help` response budget increased to 32KB** — The `xray_help` tool response budget was increased from 20KB to 32KB to accommodate the growing parameter examples and tips. Previously, adding new tool parameters could cause best practices tips to be truncated.

### Bug Fixes

- **`xray_grep` non-UTF-8 files now return `lineContent`** — Replaced 4 instances of `std::fs::read_to_string()` with `read_file_lossy()` in grep handler. Previously, Windows-1252, Shift-JIS, and UTF-16LE files silently returned no `lineContent` in search results. Now uses the same lossy reading that the watcher and content indexer already use.

- **Reindex via MCP used hardcoded `min_token_len: 2`** — `handle_xray_reindex` in `handlers/mod.rs` used a literal `2` instead of `DEFAULT_MIN_TOKEN_LEN`. Fixed to use the constant to prevent index divergence.

- **`eprintln!` diagnostic traces in grep handler** — 11 `eprintln!("[substring-trace] ...")` calls fired on every grep request in production, polluting stderr. Replaced with `tracing::debug!()` so they only appear when `RUST_LOG=debug` is set.

- **Unbounded stdin `read_line` could OOM** — `server.rs` read an entire line into memory before checking size. A malicious/buggy client sending gigabytes without a newline could cause OOM. Fixed with `.take(MAX_REQUEST_SIZE + 1)` to cap reading, plus bounded drain loop to discard remaining bytes.

- **Watcher thread infinite loop on poisoned `RwLock`** — If a panic poisoned the content or definition index RwLock, the watcher thread would loop forever logging errors and discarding all file changes. Now `process_batch()` returns `false` on poisoned lock, causing the watcher thread to exit gracefully. 3 new regression tests.

---

## 2026-02-27

### Bug Fixes

- **`xray_grep` substring mode silently returned 0 for terms with punctuation** — When `xray_grep` received terms containing non-token characters (punctuation, brackets, etc.) in substring mode (e.g., `#[cfg(test)]`, `<summary>`, `@Attribute`, `System.IO`), it silently returned 0 results. Root cause: the inverted index tokenizer splits on all non-alphanumeric, non-underscore characters, so no indexed token contains `#`, `[`, `(`, `)`, `]`, `.`, `<`, `>`, `@`, etc. The existing auto-switch for space-containing terms (`auto_switch_to_phrase_if_spaces`) only handled spaces. Fix: extended auto-switch to detect ANY non-token character via new `has_non_token_chars()` helper. When detected, automatically routes to phrase mode (which does raw substring matching on file content for punctuation-containing phrases). Renamed `auto_switch_to_phrase_if_spaces` → `auto_switch_to_phrase_if_needed`. Response includes `searchModeNote` explaining the auto-switch. 11 new unit tests (6 for `has_non_token_chars`, 5 for auto-switch scenarios including punctuation, angle brackets, underscore-only no-switch).

### Features

- **SQL stored procedure call graph in `xray_callers`** — `xray_callers` now supports SQL stored procedure and function call chains via EXEC statements. `direction=up` finds which SPs call a given SP; `direction=down` shows what SPs/functions a given SP calls via EXEC. Tables and views are deliberately excluded from the call graph (data artifacts, not callable code). The `class` parameter maps to SQL schema name (e.g., `class="dbo"`, `class="Sales"`) for disambiguation. Also set `parent` field on SP/SqlFunction definitions to the schema name in the SQL parser, enabling `resolve_call_site` to match EXEC calls across schemas. 8 new unit tests. Cross-language callers (C# → SQL SP via ADO.NET) remain a known limitation.

### Internal
- **Refactored `build_definition_index()` in `src/definitions/mod.rs`** — Decomposed the 388-line monolith (cognitive complexity 102, cyclomatic 56) into 3 focused helper functions: `collect_source_files()` (parallel file walking), `index_file_defs()` (shared index population), `enrich_angular_templates()` (Angular template enrichment). `index_file_defs()` eliminates ~50 lines of duplicated code between `build_definition_index()` and `update_file_definitions()` in `incremental.rs` — both now call the same shared function. Also added `ChunkResult` type alias for readability and removed dead `file_count` AtomicUsize. 14 new unit tests for extracted functions. No behavioral changes — all 1085 unit tests + 62 E2E tests pass.

- **Refactored `handle_xray_grep()` / `handle_substring_search()` in `src/mcp/handlers/grep.rs`** — Extracted 4 shared helper functions to eliminate duplicated code across grep/substring/phrase search modes: `passes_file_filters()` (4→1 occurrences), `finalize_grep_results()` (2→1), `build_grep_base_summary()` (8→1 readErrors/lossyUtf8Files/branchWarning blocks), `ensure_trigram_index()`. 19 new unit tests for extracted functions. No behavioral changes — all 1071 unit tests + 62 E2E tests pass.

- **Refactored `build_caller_tree()` / `build_callee_tree()` in `src/mcp/handlers/callers.rs`** — Introduced `CallerTreeContext` struct reducing parameter count from 13→6 (`build_caller_tree`) and 11→6 (`build_callee_tree`). Extracted `resolve_parent_file_ids()` (parent class file pre-filtering) and `expand_interface_callers()` (90-line deeply-nested interface resolution block → 4-line call). No behavioral changes — all 1071 unit tests + 62 E2E tests pass.

- **Refactored `IndexMeta` — typed `IndexDetails` enum** — Replaced flat `IndexMeta` struct (15 fields, 10 `Option<T>`) with typed `IndexDetails` enum discriminated by `#[serde(tag = "type")]`. Four variants: `Content`, `Definition`, `FileList`, `GitHistory` — each carries only its relevant fields, eliminating `None`-padding anti-pattern. Updated 4 constructors, `meta_to_json()`, `cmd_info` display, and all tests. Added `test_meta_serde_roundtrip_all_variants` covering JSON round-trip for all 4 variants. Note: old `.meta` sidecar files are incompatible — they will be auto-recreated on next `xray info`.

### Performance
- **`Arc<[String]>` for extensions in `build_content_index()`** — Replaced `Vec<String>.clone()` per parallel walker thread with `Arc::clone()` (O(1) vs O(n)). Minimal real-world impact (small vector, few threads), but eliminates an unnecessary allocation anti-pattern.

- **Cognitive complexity reduction for 3 highest-complexity functions** — Reduced cognitive complexity of the 3 remaining functions above the ≤50 threshold via pure extraction refactoring (no behavioral changes):
  - `build_caller_tree()` (callers.rs): **83→45** cognitive complexity. Extracted 4 helpers: `find_target_line()` (overload disambiguation lookup), `collect_definition_locations()` (definition-site exclusion set), `passes_caller_file_filters()` (ext/dir/file filter check), `build_caller_node()` (JSON node construction). Also reused `find_target_line()` and `passes_caller_file_filters()` in `build_callee_tree()`, eliminating duplicated code.
  - `handle_substring_search()` (grep.rs): **80→10** cognitive complexity. Extracted 4 helpers: `auto_switch_to_phrase_if_spaces()` (space detection + phrase delegation), `find_matching_tokens_for_term()` (trigram intersection + verification), `score_token_postings()` (TF-IDF scoring per token), `build_substring_response()` (JSON building with warnings/matchedTokens).
  - `handle_xray_grep()` (grep.rs): **53→18** cognitive complexity. Extracted 4 helpers: `parse_grep_args()` → `ParsedGrepArgs` struct (parameter parsing + validation), `expand_regex_terms()` (regex pattern expansion), `score_normal_token_search()` (TF-IDF scoring for exact tokens), `build_grep_response()` (JSON building with count_only support).
  - 54 new unit tests for all 12 extracted functions. All 1146 unit tests + 62 E2E tests pass.


- **Refactored `cmd_grep()` in `src/cli/mod.rs`** — Decomposed from 320-line monolith (cognitive complexity 294, cyclomatic 100) into thin orchestrator (~45 lines) calling 10 focused sub-functions: `parse_grep_args()`, `load_grep_index()`, `resolve_grep_dir()`, `dispatch_grep_search()`, `run_exact_token_search()`, `run_substring_search()`, `run_phrase_search()`, `run_regex_search()`, `format_grep_results()`, `print_grep_summary()`. Added 38 new unit tests for extracted functions. No behavioral changes — all 1052 unit tests + 62 E2E tests pass.

- **Test file split: handlers_tests.rs (3,364 lines → 6 files) and handlers_tests_csharp.rs (2,977 lines → 2 files)** — Split two oversized test files into focused modules to improve LLM context efficiency, incremental compilation, and merge conflict risk. Zero behavior change — all 1012 tests pass identically. Bytewise verification confirmed every test line matches the original. Test function name diff against git HEAD confirmed perfect match (94/94 + 51/51).
  - `handlers_tests.rs` (core): 29 tests — tool definitions, dispatch, context, readiness gates
  - `handlers_tests_grep.rs` (NEW): 63 tests — grep, substring, phrase, truncation, unicode
  - `handlers_tests_fast.rs` (NEW): 14 tests — xray_fast
  - `handlers_tests_find.rs` (NEW): 2 tests — xray_find
  - `handlers_tests_git.rs` (NEW): 10 tests — git cache, noCache
  - `handlers_tests_misc.rs` (NEW): 24 tests — metrics, security, ranking, validation
  - `handlers_tests_csharp.rs` (definitions): 36 tests — definitions, includeBody, containsLine, audit, reindex
  - `handlers_tests_csharp_callers.rs` (NEW): 23 tests — callers up/down, DI, cycles, overloads
  - `handlers_test_utils.rs` extended with shared `make_empty_ctx()` helper

### Bug Fixes

- **TypeScript `enumMember` extraction broken for enums with explicit values** — Enums with string or numeric values (e.g., `enum Status { Active = "active", Inactive = 0 }`) produced 0 `enumMember` definitions. Root cause: tree-sitter-typescript emits `enum_assignment` nodes for valued members, but the parser only matched `enum_member` and `property_identifier` patterns. Fix: added `"enum_assignment"` to the match arm in `walk_typescript_node_collecting()`. Simple enums without values (`enum Foo { A, B, C }`) were not affected (they use `property_identifier` nodes). 3 new regression tests (string values, numeric values, mixed members). Discovered via large-scale TypeScript E2E testing (449K definitions, 0 `enumMember` results for `parent:"FilteringState"`).

### Features

- **Hint when `kind:"property"` returns 0 results for TypeScript** — When `xray_definitions` with `kind:"property"` returns 0 results and the index contains `field` definitions, the response now includes a `hint` in the summary: "In TypeScript, class members are indexed as kind='field', while only interface property signatures use kind='property'. Try kind='field' instead." Also updated the `kind` parameter documentation in `xray_help` to clarify the property vs field distinction. 2 new unit tests.

- **Rust parser included in default build** — `lang-rust` feature is now part of the default feature set (`lang-csharp`, `lang-typescript`, `lang-sql`, `lang-rust`). All 4 language parsers are compiled and available out of the box. No more `--features lang-rust` needed for Rust support.

### Performance

- **Lazy-compiled regex in SQL parser** — Converted 19 `Regex::new()` calls in `parser_sql.rs` from per-call compilation to `std::sync::LazyLock` module-level statics, compiled once on first use. Eliminates ~9,000 redundant regex compilations when indexing 500 SQL files with ~3 stored procedures each. 1 dynamic regex (using `format!` for `PROC|FUNCTION` keyword) kept as-is. Estimated 5–15% speedup for SQL-heavy codebases.

- **Pre-lowercased exclude lists in `xray_callers`** — `excludeDir` and `excludeFile` parameters are now lowercased once at parse time instead of re-lowercasing on every file comparison inside the recursive `build_caller_tree`/`build_callee_tree` functions. In a depth-3 tree with 10 callers per level × 5 exclude entries, this eliminates ~150 unnecessary `to_lowercase()` string allocations per query.

- **`handle_xray_definitions` refactored into composable functions** — Split the 634-line monolithic `handle_xray_definitions` function (cognitive complexity 269, cyclomatic 136) into 10 focused functions: `parse_definition_args()` → `DefinitionSearchArgs` struct, `handle_audit_mode()`, `handle_contains_line_mode()`, `collect_candidates()`, `apply_entry_filters()`, `apply_stats_filters()`, `compute_term_breakdown()`, `sort_results()`, `format_definition_entry()`, `build_search_summary()`. The orchestrator is now ~60 lines. Each function is independently testable. 36 new unit tests covering: argument parsing (15 tests), candidate collection (9 tests), entry filtering (8 tests), stats filtering (7 tests), term breakdown (5 tests), sort logic (6 tests), format/summary (13 tests), get_sort_value (4 tests). Total tests: 952 → 988.

### Internal

- **Shared tree-sitter utility module** — Extracted 4 duplicated AST helper functions (`node_text`, `find_child_by_kind`, `find_descendant_by_kind`, `find_child_by_field`) from C#, TypeScript, and Rust parsers into a new shared module `src/definitions/tree_sitter_utils.rs`. Eliminates 12 duplicate function definitions across 3 parser files. TypeScript parser keeps a thin 1-line `node_text` wrapper (accepts `&str` instead of `&[u8]`) to avoid changing 50+ call sites. 7 new unit tests for the shared utilities.

- **Unified data-driven `walk_code_stats`** — Replaced 3 near-identical code complexity walker functions (`walk_code_stats_csharp` 109 lines, `walk_code_stats_typescript` 94 lines, `walk_code_stats_rust` 85 lines) with a single `walk_code_stats()` function + 3 static `CodeStatsConfig` structs containing language-specific AST node names. Config covers: branching nodes, else/else-if handling, logical operators, goto, switch cases, return/throw, lambdas, nesting incrementors, and C#-specific if→if flat nesting. Eliminates ~190 lines of duplicated code. Adding a new language parser now requires only a `CodeStatsConfig` definition — no walker code duplication.

- **Parameter structs for grep and server** — Introduced `GrepSearchParams` struct to consolidate 10 positional parameters shared by `handle_substring_search` and `handle_phrase_search` (`ext_filter`, `exclude_dir`, `exclude`, `show_lines`, `context_lines`, `max_results`, `mode_and`, `count_only`, `search_start`, `dir_filter`). Refactored `run_server` from 12 positional parameters to accept `HandlerContext` directly (the struct was already being constructed on the first line).

- **Watcher `process_batch` extraction** — Extracted the core batch update logic (120 lines) from `start_watcher` into 4 focused functions: `process_batch()`, `update_content_index()`, `update_definition_index()`, `shrink_if_oversized()`. The watcher event loop is now 6 lines instead of 120. Added 6 new unit tests covering: empty batch, dirty file update, file removal, mixed dirty+removed, new file addition, and total_tokens consistency.

- **Shared `count_named_children` utility** — Extracted the duplicate "count named children in parameter list" logic from `count_parameters_csharp` (9 lines) and `count_parameters_typescript` (16 lines) into a shared `count_named_children()` function in `tree_sitter_utils.rs`. Both callers now use `.map(count_named_children)` — eliminating 7 lines of duplicated inline closures.

- **Dead code cleanup** — Removed unused `active_definition_count()` function from `incremental.rs`. Removed spurious `#[allow(dead_code)]` from `DEFAULT_MAX_RESPONSE_BYTES` constant (it IS used in production code via `HandlerContext::default()`). Remaining `#[allow(dead_code)]` markers verified as correct: `storage.rs` functions used by binary crate, `protocol.rs` field required for serde, feature-gated functions.

### Features

- **Rust parser (`lang-rust` optional feature)** — New tree-sitter-based parser for `.rs` files, activated via `--features lang-rust` (NOT in default build). Extracts: structs, enums, traits (`Interface`), `impl` block methods (with parent struct association), constructors (`fn new()`/`fn default()`), trait impls (`base_types`), `const`/`static` variables, type aliases, struct fields, enum variants. Call sites: `self.method()`, `self.field.method()`, `Type::method()`, free function calls. Code stats: cyclomatic/cognitive complexity, match arms, `?` operator (early return), closures, nesting depth, params (excluding `self`). Modifiers: `pub`, `async`, `unsafe`, `const`, `mut`. Attributes: `#[test]`, `#[derive(Debug)]`, `#[cfg(test)]`, `#[serde(default)]`. Build with `cargo build --features lang-rust` or `cargo build --no-default-features --features lang-rust` (Rust-only). 24 new tests (18 parser + 6 handler).

- **Configurable language parsers via Cargo features** — Language parsers are now conditionally compiled via Cargo feature flags: `lang-csharp`, `lang-typescript`, `lang-sql`. Default `cargo build` includes all three (backward compatible). Build with `--no-default-features --features lang-csharp` for C#-only, `--features lang-sql` for SQL-only (no tree-sitter dependency), or `--no-default-features` for grep/content-index-only builds. `tree-sitter`, `tree-sitter-c-sharp`, and `tree-sitter-typescript` are now optional dependencies. All tests are gated with `#[cfg(feature = "...")]` — test counts adjust automatically per feature set: 872 (all), 623 (SQL-only), 593 (none). Future parsers (e.g., Rust) can be added as new features without modifying existing code.

### Bug Fixes

- **`xray_grep` multi-phrase OR search — comma-separated phrases silently returned 0 results** — When `xray_grep` received comma-separated phrases with spaces (e.g., `terms="fn handle_foo,fn build_bar"` or `terms="class UserService,class OrderService"`), the auto-switch to phrase mode passed the ENTIRE comma-separated string as a single phrase to `handle_phrase_search()`, which then searched for the literal string `"fn handle_foo,fn build_bar"` — a string that doesn't exist in any file → 0 results. The same bug affected the explicit `phrase: true` path. Fix: both paths now split by commas and search each phrase independently via new `handle_multi_phrase_search()` function, merging results with OR or AND semantics. Single phrases retain existing behavior. `searchMode` reports `"phrase-or"` or `"phrase-and"` for multi-phrase queries. 8 new unit tests + 1 E2E test.

- **`xray_callers` DI resolution for nested class `Owner.m_field` pattern** — When a nested (inner) class accessed DI-injected fields of its outer (parent) class via `Owner.m_field` (ControllerBlock pattern), the receiver type was resolved to the outer class name (e.g., `"OrderControllerBlock"`) instead of the field's interface type (e.g., `"IQueryManager"`). Root cause: the `field_types` map passed to call site extraction only contained fields from the inner class — outer class fields were invisible. Fix: when building `field_types` for methods in a nested class, the parser now merges the outer class's field types into the map (inner class fields take precedence via `or_insert`). This enables `resolve_receiver_type` to find `m_field` in the merged map and resolve it to the correct DI interface type. 2 new unit tests (basic resolution + inner-class-takes-precedence edge case).

- **`xray_callers` DI resolution gap for constructor field assignments** — When a class used DI fields with non-standard naming conventions (e.g., `m_field`, `fld_field`, `this.field`) WITHOUT explicit field declarations, the C# parser couldn't resolve the receiver type. Root cause: the parser only generated `_paramName` and bare `paramName` mappings from constructor parameters, missing any other naming convention. Fix: the parser now parses the constructor body AST for `field = param` assignments (e.g., `m_orderService = orderService`, `this.myRepo = repository`), mapping the assigned field to the parameter's type. This handles ANY naming convention automatically without hardcoding prefixes. Also fixed a secondary bug in `extract_constructor_param_types` where constructor initializers (`: base(logger)`, `: this(x)`) caused incorrect parameter extraction because `rfind(')')` matched the initializer's closing paren instead of the constructor's. Replaced with depth-tracking paren matching. 4 new unit tests.

- **`baseTypeTransitive` BFS cascade bug** — `collect_transitive_base_type_indices()` used substring matching (`key.contains(&current_type)`) at ALL BFS levels, causing a cascade when a descendant class had a short/common name (e.g., `"Service"` matched `"iservice"`, `"webservice"`, `"serviceprovider"`, etc.). This produced ~42,508 results instead of ~828 and took ~29 seconds. Fix: substring matching is now used only at level 0 (seed) for generic type support (`IAccessTable` → `iaccesstable<model>`); levels 1+ use exact HashMap lookup (O(1)). 3 new unit tests.

### Features

- **`termBreakdown` in `xray_definitions` summary for multi-term name queries** — When `name` contains comma-separated terms (e.g., `name="AccessSource,AccessContracts,IAccessTable"`), the summary now includes a `termBreakdown` object showing how many results each term contributed (computed from the full result set before `maxResults` truncation). This helps LLM agents understand result distribution and decide whether to refine their query with `kind` filters or split into separate queries. Only appears for 2+ terms in non-regex mode. 6 new unit tests.

- **Hint when `xray_callers` returns 0 results with class filter** — When `xray_callers` returns an empty call tree and `class` parameter is set, the response now includes a `hint` field suggesting: try without `class` parameter, or use the interface name. Helps LLM agents diagnose why no callers were found. 3 new unit tests.

- **Hint for large `baseTypeTransitive` hierarchies** — When `baseTypeTransitive=true` and `totalResults > 5000`, the `xray_definitions` response includes a `hint` suggesting `kind` or `file` filters to narrow results. 1 new unit test.

### Internal

- **Complete `..Default::default()` boilerplate cleanup** — Replaced ~60 explicit field enumerations in test code with `..Default::default()` for both `HandlerContext` (33 sites) and `DefinitionIndex` (27 sites) across 8 test files. Also ran `cargo fix` to remove 12 unused imports from 8 files. Final pass: replaced 6 remaining cosmetic sites (4 `ContentIndex` + 2 `DefinitionIndex`) in `search_benchmarks.rs`, `handlers_tests_find.rs`, `handlers_tests_misc.rs`, `handlers_tests_grep.rs`, `handlers_tests_typescript.rs`, and `callers_tests.rs`. Removed 2 unused `TrigramIndex` imports. No behavioral changes — purely mechanical cleanup. All 1214 unit tests + 62 E2E tests pass.

---

## 2026-02-26

### Internal

- **`impl Default for ContentIndex` + test boilerplate reduction** — Added `impl Default for ContentIndex` in `src/lib.rs` with compile-time guard test (`test_content_index_field_count_guard`) and default values test (`test_content_index_default_values`). Replaced ~88 test-only `ContentIndex` struct constructions across 13 files with `..Default::default()`, keeping only test-relevant fields explicit. Also replaced ~30 `DefinitionIndex` test constructions (Default already existed). Production code (`build_content_index()`, `serve.rs` empty index) retains explicit fields — compiler enforces conscious field assignment. Adding a new field now requires 3 changes (Default + guard test + production) instead of ~88. 2 new tests, 852 total pass.

### Features

- **`baseTypeTransitive` parameter for `xray_definitions`** — New boolean parameter enables BFS traversal of the inheritance hierarchy. `baseType="BaseService" baseTypeTransitive=true` finds not just direct inheritors (MiddleService) but also grandchildren (ConcreteService) and deeper descendants, up to depth 10. Uses runtime BFS with visited set for cycle safety. Known limitation: name-only matching (no namespace resolution). 4 new unit tests.

- **`baseType` filter now uses substring matching** — `baseType="IAccessTable"` now matches `IAccessTable<Model>`, `IAccessTable<Report>`, `IAccessTable<Dashboard>`, etc. Previously, exact match required the full generic type name including type parameters. This makes generic interface discovery practical — a single query finds all 15+ implementations instead of requiring per-type-parameter queries. Both direct and transitive modes use substring matching.

- **Cross-index diagnostics (`crossValidate` parameter)** — New `crossValidate=true` parameter in `xray_definitions audit=true` mode. Loads the file-list index from disk and compares with the definition index to identify coverage gaps: files in the file-list but missing from definitions (filtered by definition extensions), and files in definitions but missing from the file-list. Graceful handling when file-list index not found on disk. Results capped at 50 samples. 3 new unit tests.

- **Dynamic MCP instructions adapt to server `--ext` configuration** — The `initialize` → `instructions` field now dynamically generates the "NEVER READ" rule based on the intersection of `DEFINITION_EXTENSIONS` (extensions with parser support: cs, ts, tsx, sql) and the server's `--ext` CLI argument. Previously, the instruction was hardcoded to `.cs/.ts/.tsx`, which was misleading when the server was started with `--ext sql` (SQL was missing from the rule) or `--ext xml` (no parser-supported extensions, but the rule was still present). New behavior: `--ext cs,sql` → `"NEVER READ .cs/.sql FILES DIRECTLY"`, `--ext xml` → fallback note that `xray_definitions` is unavailable. Added `DEFINITION_EXTENSIONS` constant in `definitions/mod.rs` as single source of truth. Empty extensions guard prevents malformed instructions. 3 new unit tests.

### Bug Fixes

- **Content index silently swallowed `read_file_lossy` errors** — `build_content_index()` had `Err(_) => {}` in the file walk loop, silently discarding IO errors with no logging, no counters, and no way for users to know files were skipped. Fix: added `read_errors: usize` and `lossy_file_count: usize` fields to `ContentIndex` (with `#[serde(default)]` for backward-compatible deserialization). During build, `AtomicUsize` counters track errors and lossy conversions, with `eprintln!` warnings per file. The `xray_grep` summary now reports `readErrors` and `lossyUtf8Files` (matching the pattern already used by `xray_definitions`). The `.meta` sidecar file also captures these counters. 5 new unit tests.

### Bug Fixes

- **Watcher bulk reindex after `git pull` — minutes delay reduced to seconds** — Two bugs fixed:
  - **Bug 1: `bulk_threshold=100` triggered full reindex on `git pull`** — When `git pull` changed 100+ files, the watcher's bulk threshold (`--bulk-threshold`, default 100) triggered a full `build_content_index()` that re-scanned all ~48K files, taking **minutes**. The bulk path has been **removed entirely** — the watcher now always uses incremental updates regardless of batch size. Additionally, a new `batch_purge_files()` function purges all stale postings in **O(total_postings)** (single pass) instead of **O(N × total_postings)** (N sequential scans), making even 10K-file `git checkout` operations fast (~10s instead of minutes).
  - **Bug 2: Definition index silently skipped in bulk path** — When the bulk threshold was exceeded, the `continue` statement at the end of the bulk path skipped the definition index update (lines 189-203), leaving `xray_definitions` and `xray_callers` with stale data after `git pull`. New methods were invisible until the next small file change triggered incremental update. Fixed by removing the bulk path — the incremental path always updates both content and definition indexes.
  - **Breaking change**: `--bulk-threshold` CLI parameter removed. If present in configuration, the server will fail to start with "unexpected argument" error.
  - Performance: `git pull` (300 files) ~4s, `git checkout` (10K files) ~25s (previously: minutes for both)
  - 3 new unit tests for `batch_purge_files` (multi-file, empty set, equivalence with single purge)


- **Tombstone growth in definition index during `--watch` mode** — When the file watcher incrementally updated definitions, old `DefinitionEntry` objects were never removed from the `definitions` Vec — they became tombstones occupying memory and inflating `totalDefinitions` count. After 100 file updates, the Vec could grow to 3× its active size. Four fixes applied:
  - **Fix 1 (UX):** `totalDefinitions` in `xray_definitions`, `xray_info`, `xray_reindex_definitions`, CLI `info`, and memory estimates now shows active definition count (from `file_index`) instead of `definitions.len()` which included tombstones
  - **Fix 2 (Correctness):** When no `name`/`kind`/`attribute`/`baseType` filter is set, candidate generation now uses `file_index` (active definitions only) instead of `0..definitions.len()` which included tombstones and could produce duplicate results
  - **Fix 3 (Angular):** `remove_file_definitions()` now cleans `selector_index` and `template_children` (previously stale entries accumulated after incremental updates)
  - **Fix 4 (Memory):** Auto-compaction triggers when tombstone ratio exceeds 3× (67% waste). In-place `compact_definitions()` rebuilds the Vec with only active entries and remaps all 9 secondary indexes. ~100ms for 846K definitions, <1MB additional memory
  - 5 new unit tests for compaction (basic, no-op, method_calls/code_stats remapping, auto-trigger, selector/template cleanup)

- **Watcher startup gap: stale cache files invisible to `--watch` mode** — When the MCP server started with `--watch` and loaded a stale index from disk, files added/modified/deleted while the server was offline were permanently invisible. The `notify` file watcher only fires events for changes AFTER it starts — pre-existing files produce no events. Fix: added **reconciliation scan** at watcher startup that walks the filesystem and compares with the cached index using path diff (added/deleted) + mtime comparison (modified files). Runs once before the event loop, inside the watcher thread — filesystem events during reconciliation are buffered in the mpsc channel. Performance: ~15ms for 130 files, ~3.5s for 30K files (one-time startup cost, zero runtime impact). Also added cache age logging at startup. 4 new unit tests.

- **`xray_definitions` `file` and `parent` parameters now support comma-separated OR** — Previously, `file` and `parent` parameters only accepted a single value (substring match), while `name` supported comma-separated OR. This inconsistency caused LLMs to waste queries when trying to search across multiple files or classes at once (e.g., `file: "UserService.cs,OrderService.cs"` returned 0 results). Both parameters now split on commas and match ANY term (OR logic), consistent with `name`. The relevance ranking for `parent` also supports comma-separated terms. 7 new unit tests.

---

## 2026-02-25

### Bug Fixes

- **Stale index cache loaded when extensions change** — When the MCP server was restarted with different `--ext` parameters (e.g., adding `sql` to a previously `cs`-only setup), the fallback index loader (`find_definition_index_for_dir` / `find_content_index_for_dir`) would find and return an old cached index file that was built with the previous extensions. This caused `xray_definitions` to return 0 results for SQL stored procedures even though `sql` was in `--ext` and the SQL parser was fully functional. Root cause: the fallback functions only checked the root directory match but did NOT validate that the cached index's extensions were a superset of the requested extensions. Fix: both functions now accept `expected_exts` parameter and validate that the cached index contains ALL expected extensions (superset check). Stale caches are skipped with a log message, triggering a full rebuild. Same fix applied to content index fallback. 8 new unit tests.

---

## 2026-02-24

### Bug Fixes

- **MCP Server**: Replace all `unwrap()` calls on response serialization with proper error handling — server now returns JSON-RPC `-32603` internal error instead of panicking (audit finding F-07)
- **Call Tree Builder**: Preserve class filter during recursive caller tree building — prevents false positives for common method names like `Process`, `Execute`, `Handle` at recursion depth > 0 (audit finding F-10)

- **SQL parser: files with comment headers before CREATE not parsed** — SQL files starting with comment banners (dashes, copyright notices) before the `CREATE PROCEDURE/TABLE/VIEW/...` statement produced 0 definitions. Root cause: the `CREATE` regex used `^\s*CREATE` with `^` anchored to the start of the batch text (single-line mode). On files without `GO` delimiters, the entire file is one batch, and `^` only matched the first character. Fix: added `(?m)` multiline flag so `^` matches at the start of every line. 1 new unit test.

- **SQL tool description incorrectly stated "SQL parser retained but disabled"** — The `xray_definitions` MCP tool description and `def-index --help` text both incorrectly claimed SQL parsing was disabled. The regex-based SQL parser has been fully active since 2026-02-23. Fixed both descriptions. The LLM was reading these descriptions and incorrectly telling users SQL wasn't supported.

### Features

- **`--debug-log` now logs full response body** — The debug log (`--debug-log` flag) now writes the complete MCP tool response JSON after each `RESP` line, enabling diagnosis of what the server actually returned. Previously only logged the response size (e.g., `0.3KB`).

- **`--debug-log` replaces `--memory-log` (US-17)** — Renamed `--memory-log` CLI flag to `--debug-log` and extended it into a full debug log for the MCP server. The debug log now captures MCP request/response traces (tool name, arguments, elapsed time, response size, Working Set) in addition to the existing memory diagnostics. Log format: ISO 8601 timestamp + type tag (REQ/RESP/MEM) per line. File extension changed from `.memory.log` to `.debug.log`. Breaking change: `--memory-log` flag removed (use `--debug-log` instead). New helper functions: `log_request()`, `log_response()`, `format_utc_timestamp()`. 4 new tests, 4 renamed tests.

### Documentation

- **Per-server memory.log with semantic prefix** — `--memory-log` now writes per-server log files using the same naming convention as index files (e.g., `repos_shared_00343f32.memory.log` instead of `memory.log`). Multiple MCP servers running simultaneously no longer overwrite each other's memory logs. 3 new tests.

### Documentation

- **3 tips.rs improvements based on LLM agent UX session analysis** — Added new tip "xray_callers 0 results? Try the interface name" warning that DI calls use interface types (IUserService), not concrete classes (UserService), and suggesting `resolveInterfaces=true`. Added NOTE to `xray_definitions` parameter examples clarifying that `name` searches AST definition names only — NOT string literals or values (use `xray_grep` for string content). Enhanced substring search tip with guidance on short-token noise: `exclude=['pattern']` for filtering, with `dsp_` + ODSP example.

### Bug Fixes

- **xray_grep substring auto-switches to phrase for spaced terms (US-16)** — When `xray_grep` receives terms containing spaces in substring mode (e.g., `"CREATE PROCEDURE"`, `"public class"`), it now auto-switches to phrase search instead of silently returning 0 results. The response includes `searchModeNote` explaining the switch. Previously, spaced terms always returned 0 because the tokenizer splits on spaces and no individual token contains spaces. 4 new tests.

### Documentation

- **Bug report: xray_grep substring mode silently returns 0 for terms with spaces** — Documented P1 UX trap where `terms: "CREATE PROCEDURE"` returns 0 results because the tokenizer splits on spaces, so no individual token contains `"create procedure"`. Fixed via Option A (auto-switch to phrase mode). See `docs/bug-reports/substring-space-in-terms-silent-failure.md`.

---

## 2026-02-23

### Bug Fixes

- **SQL excluded from definition index in MCP server** — The `supported_def_langs` array in `serve.rs` was hardcoded to `["cs", "ts", "tsx"]`, blocking SQL from the definition index even though the regex parser was fully implemented. When starting the server with `--ext cs,sql`, SQL files were silently filtered out and only C# definitions were built. Fixed by adding `"sql"` to the supported languages array. No new tests needed — existing SQL parser tests and E2E tests cover the functionality.

### Features

- **SQL parser** — `xray_definitions` now parses `.sql` files, extracting stored procedures, tables, views, functions, user-defined types, indexes, columns, FK constraints, and call sites (EXEC/FROM/JOIN/INSERT/UPDATE/DELETE from stored procedure bodies). Uses a regex-based parser (no tree-sitter dependency for SQL). Definition kinds: `storedProcedure`, `sqlFunction`, `table`, `view`, `userDefinedType`, `sqlIndex`, `column`. GO-separated batches produce correct per-object line ranges. 29 unit tests.

- **Parent relevance ranking in `xray_definitions`** — When `parent` filter is set, results are now sorted by parent match quality: exact parent match (tier 0) ranks before prefix match (tier 1), which ranks before substring/contains match (tier 2). Previously, all parent substring matches were treated equally, so searching with `parent=UserService` could return members of `UserServiceMock` before members of `UserService` itself. The fix activates relevance sorting when `parent_filter` is set (in addition to the existing `name_filter` path), using `best_match_tier()` as the primary sort key for parent, with name match tier as secondary. 5 new unit tests.

### Documentation

- **Method group/delegate limitation documented in `xray_callers`** — Added a new tip and parameter example documenting that `xray_callers` only detects direct method invocations (`obj.Method(args)`), NOT method group references or delegate passes (e.g., `list.Where(IsValid)`, `Func<bool> f = svc.Check`). Workaround: use `xray_grep` to find all textual references. This is a known parser-level limitation requiring AST changes to fix.

---

## 2026-02-22

### Documentation

- **LLM instructions: "NEVER READ .cs/.ts/.tsx FILES DIRECTLY" rule** — Substantially strengthened the MCP `instructions` field to prevent LLMs from defaulting to `read_file` for C#/TS files when `xray_definitions includeBody=true` is faster. The previous "BEFORE reading, try X first" wording was too soft and lost to Roo's built-in "read all related files together" guidance. New approach uses 4 reinforcement mechanisms: (1) **Absolute prohibition**: `"NEVER READ .cs/.ts/.tsx FILES DIRECTLY"` in ALL CAPS. (2) **Decision trigger**: `"before ANY file read, check each file's extension"` — forces the LLM to evaluate extensions before choosing the tool. (3) **Batch split rule**: `"if you need both .cs and .md files, make TWO calls"` — explicitly resolves the conflict with Roo's "batch all files in one read" advice. (4) **Single exception**: only for editing (need exact line numbers for diffs). Also added anti-pattern in Architecture Exploration strategy and expanded the "Read method source" tip. Root cause: LLMs have a "convenience bias" toward built-in tools (`read_file`) over MCP tools (`xray_definitions`), especially when batch reading mixes indexed and non-indexed file types.

### Features

- **Angular Template Metadata** — Enriched Angular `@Component` definitions with template metadata. `xray_definitions` now returns `selector` and `templateChildren` for Angular components. `xray_callers` supports component tree navigation — `direction='down'` shows child components from HTML templates (recursive), `direction='up'` with a selector finds parent components. Custom elements (tags with hyphens) are extracted from external `.html` templates.

### Breaking Changes

- **Removed `search_git_pickaxe` MCP tool** — The `search_git_pickaxe` tool has been removed. Its use cases (finding when code was introduced) are better served by the `xray_grep` → `xray_git_blame` workflow, which is 780x faster (~200ms vs 156 seconds) and handles file renames correctly. The only unique pickaxe capability (finding deleted code) was rare and can be done via `git log -S` directly if needed. Tool count: 16 → 15. Also removed: `next_day_public()`, `run_git_public()`, `FIELD_SEP_STR/CHAR`, `RECORD_SEP_STR/CHAR` (all were pickaxe-only). Updated "Code History Investigation" strategy recipe to use grep+blame workflow. Removed 14 pickaxe unit tests + 3 helper tests.

### Bug Fixes

- **Angular template upward recursion stopped at level 1** — `xray_callers` with `direction='up'` for Angular selectors only returned direct parent components, ignoring the `depth` parameter. For example, searching up from `operation-button` with `depth=3` found `OperationsBarComponent` (level 1) but NOT its 4 grandparent components (level 2). Root cause: `find_template_parents()` was a flat, non-recursive function — it found direct parents but never recursed to find their parents. Fix: rewrote `find_template_parents()` to accept `max_depth`, `current_depth`, and `visited` set (mirroring the working `build_template_callee_tree()` for downward direction). Grandparents are nested in a `"parents"` field on each parent node. Cycle detection via visited set prevents infinite loops. 4 new unit tests (recursive depth, max_depth respect, cyclic components).

- **`totalCommits` in cache showed truncated count instead of actual total** — When `xray_git_history` used the in-memory cache with `maxResults` limiting output, `totalCommits` in the response equaled the returned (truncated) count instead of the actual total. For example, a file with 18 commits queried with `maxResults: 2` showed `totalCommits: 2` instead of `totalCommits: 18`. This misled LLMs into thinking the file had fewer commits than it actually did. Root cause: `query_file_history()` returned only `Vec<CommitInfo>` (after truncation), and the handler used `.len()` for the total count. Fix: `query_file_history()` now returns `(Vec<CommitInfo>, usize)` where the usize is the count BEFORE truncation. The `hint` field now correctly shows "More commits available..." when `totalCommits > returned`. CLI fallback path was already correct. 4 new unit tests. 1 new E2E regression test (T-GIT-TOTALCOMMITS).

- **E2E structural bug: 2 tests hidden inside catch block** — `T-OVERLOAD-DEDUP-UP` and `T-SAME-NAME-IFACE` E2E tests were nested inside the `catch` block of `T-FIX3-LAMBDA`, meaning they only executed when `T-FIX3-LAMBDA` threw an exception. Since `T-FIX3-LAMBDA` passes normally, both tests were silently skipped on every run. Fixed by moving them to top-level `try/catch` blocks. Also strengthened `T-SAME-NAME-IFACE` assertion: now explicitly asserts `totalNodes=0` instead of only checking for specific caller names, catching any unexpected caller in the tree.

- **`xray_info` memory spike fix — 1.8 GB temporary allocation eliminated** — `xray_info` MCP handler was calling `cmd_info_json()` which fully deserialized ALL index files from `%LOCALAPPDATA%/xray/` (including indexes for repos the server doesn't serve). For a multi-repo setup with multiple indexed directories, this loaded ~1.8 GB into memory temporarily. Since the main thread never exits, mimalloc never decommitted these freed segments back to the OS, causing Working Set to stay at ~4.4 GB instead of ~2.5 GB. **Fix:** Rewrote `handle_xray_info()` to read all statistics from already-loaded in-memory structures (`ctx.index`, `ctx.def_index`, `ctx.git_cache`) via read locks — zero additional allocations. Disk file sizes obtained via `fs::metadata()` only. Removed the `cmd_info_json()` call entirely from the MCP path. Also removed the temporary `force_mimalloc_collect()` workaround from `dispatch_tool()` that was added during diagnosis. Memory log (`--memory-log`) confirms: `xray_info` Δ WS went from +1,799 MB to ~0 MB.

- **CLI `xray info` sidecar `.meta` optimization** — CLI `xray info` previously deserialized entire index files from disk (~1.8 GB for multi-repo setups) just to extract metadata (root, files count, tokens, age). Added sidecar `.meta` JSON files (~200 bytes each) that are written alongside every index file on save. `cmd_info()` and `cmd_info_json()` now read `.meta` files first (instant, zero deserialization), falling back to full deserialization only for old indexes without `.meta`. Affected save functions: `save_content_index()`, `save_index()`, `save_definition_index()`, `GitHistoryCache::save_to_disk()`. Cleanup functions (`cleanup_orphaned_indexes`, `cleanup_indexes_for_dir`) also remove `.meta` sidecars. 4 new unit tests.

### Performance

- **E2E test parallelization (~50% speedup)** — Parallelized 15 independent MCP tests (9 callers + 5 git + 1 help) using PowerShell `Start-Job`. Sequential CLI tests (shared index state) run first, then the parallel batch runs concurrently. Each parallel test uses isolated temp directories or read-only git queries, ensuring no race conditions. Parallel batch completes in ~6s instead of ~52s sequential. Total E2E time reduced from ~2 min to ~1 min. Compatible with PowerShell 5.1+ (uses `Start-Job`, not PS7-only `ForEach-Object -Parallel`).

### Internal

- **Test parallelism race conditions fixed (US-9)** — Migrated 14 unit tests from hardcoded `std::env::temp_dir().join("fixed_name")` to `tempfile::tempdir()` across 4 test files (`definitions_tests.rs`, `definitions_tests_csharp.rs`, `definitions_tests_typescript.rs`, `handlers_tests.rs`). The hardcoded temp directory names caused race conditions when `cargo test` ran tests in parallel (default behavior, 24 threads on this machine), as two tests could simultaneously write/delete the same directory. `tempfile::tempdir()` generates unique OS-guaranteed paths with automatic cleanup on drop. No test logic changed — only the temp directory creation mechanism. All 822 tests pass with 0 failures under full parallelism.

- **6 new E2E tests for previously untested MCP features** — Added `T-SERVE-HELP-TOOLS` (verifies `serve --help` lists key tools), `T-BRANCH-STATUS` (smoke test for `xray_branch_status` MCP tool), `T-GIT-FILE-NOT-FOUND` (nonexistent file returns warning, not error), `T-GIT-NOCACHE` (`noCache` parameter returns valid result), `T-GIT-TOTALCOMMITS` (totalCommits > returned regression test for BUG-2 fix). Total E2E tests: 48 → 55.

- **`definition_index_path_for()` made public** — Renamed `def_index_path_for()` → `definition_index_path_for()` and made it `pub` in `src/definitions/storage.rs` for use by `handle_xray_info()` disk size lookup.
- **`read_root_from_index_file_pub()` added** — Public wrapper for header-only index file reading in `src/index.rs`, used by `handle_xray_info()` to get file-list root directory without full deserialization.

---

## 2026-02-21

### Features

- **Memory diagnostics (`--memory-log`)** — New `--memory-log` CLI flag for `xray serve` writes Working Set / Peak WS / Commit metrics to `memory.log` in the index directory (`%LOCALAPPDATA%/xray/`) at every key pipeline stage. Metrics are captured at: server startup, content/definition index build start/finish, drop/reload cycles, trigram builds, git cache init/ready. When disabled (default), `log_memory()` is a single `AtomicBool` check — zero overhead. Windows-only (uses `K32GetProcessMemoryInfo`); no-op on other platforms. 7 new unit tests.

- **Memory estimates in `xray_info`** — `xray_info` MCP response and CLI `xray info` now include a `memoryEstimate` section with calculated per-component memory estimates: inverted index, trigram tokens/map, files, definitions, call sites, git cache, and process memory (Working Set / Peak / Commit). Estimates use sampling (first 1000 keys) for efficiency. Available on all platforms; process memory info is Windows-only.

### Performance

- **`mi_collect(true)` fix for cold-start memory spike** — After `drop(build_index)` and before `load_from_disk()`, the server now calls mimalloc's `mi_collect(true)` to force decommit of freed segments from abandoned thread heaps. This prevents the build+drop+reload pattern from inflating Working Set by ~1.5 GB. Applied in 3 locations: content index build thread, definition index build thread, and watcher bulk reindex path.

### Bug Fixes

- **Chained method calls missing from call-site index (C# and TypeScript)** — Inner calls in method chains like `service.SearchAsync<T>(...).ConfigureAwait(false)` and `builder.Where(...).OrderBy(...).ToList()` were not extracted. Only the outermost call (e.g., `ConfigureAwait`, `ToList`) was found; all inner calls were silently dropped. Root cause: `walk_for_invocations()` (C#) and `walk_ts_for_invocations()` (TypeScript) only recursed into `argument_list` children of `invocation_expression`/`call_expression` nodes, skipping the `member_access_expression` child where nested invocations live in the AST. The fix recurses into ALL children, capturing every call in the chain. This affects `xray_callers` results for any code using `.ConfigureAwait(false)`, fluent APIs, LINQ chains, or promise chains. 2 new regression tests, 1 existing test strengthened.

- **Generic method call-site indexing in C# parser** — Call sites for generic method invocations like `client.SearchAsync<T>(args)` were stored with `method_name = "SearchAsync<T>"` (including type arguments) instead of `"SearchAsync"`. This caused `verify_call_site_target()` to fail matching when `class` filter was used in `xray_callers`, producing 0 callers for any generic method. The fix adds `extract_method_name_from_name_node()` that strips type arguments from `generic_name` AST nodes in both `extract_member_access_call()` and `extract_conditional_access_call()`. Also fixes `direction=down` callee resolution for generic methods. TypeScript parser was NOT affected (different AST structure). 6 new unit tests.

### Internal

- **Independent audit test suite for code stats and call chains** — Added `src/definitions/audit_tests.rs` with 22 golden fixture tests that independently verify the accuracy of tree-sitter-based code complexity metrics and call chain analysis. Each fixture is hand-crafted code where every metric (cyclomatic complexity, cognitive complexity, nesting depth, param count, return count, call count, lambda count) is manually computed line-by-line. The audit covers: C# code stats (7 tests), TypeScript code stats (5 tests), call site accuracy with receiver type verification (2 tests), multi-class call graph completeness (2 tests), edge cases (4 tests), and statistical consistency checks including axiomatic invariants and cross-language parity (3 tests). Documents known tree-sitter grammar differences between C# and TypeScript (else-if handling, try nesting).

### Bug Fixes

- **UTF-16 BOM detection in `read_file_lossy()`** — Files encoded in UTF-16LE or UTF-16BE (with BOM) were previously read as lossy UTF-8, producing garbled content (`��/ / - - - -`). Tree-sitter received garbage instead of valid source code, resulting in 0 definitions for affected files. The fix adds BOM detection to `read_file_lossy()`: UTF-16LE BOM (`FF FE`) → decode as UTF-16LE, UTF-16BE BOM (`FE FF`) → decode as UTF-16BE, UTF-8 BOM (`EF BB BF`) → strip BOM. All three indexes (content, definitions, callers) benefit from this single-function fix. Affects ~44 files previously reported as `lossyUtf8Files` in audit. 15 new unit tests.

### Performance

- **Optimized MCP tool descriptions for LLM token budget** — Shortened parameter descriptions across all 14 MCP tools (~100 parameters total), reducing the system prompt token footprint by ~30% (~2,000 tokens). Concrete examples moved from inline parameter descriptions to a new `parameterExamples` section in `xray_help` (on-demand via 1 extra call). Critical usage hints preserved (e.g., `class` in `xray_callers`). Tool-level descriptions unchanged. Semantic purpose of each parameter preserved (8-15 words). Added `test_tool_definitions_token_budget` test to prevent description bloat from re-accumulating. Added `test_render_json_has_parameter_examples` test to verify examples are accessible via `xray_help`.

### Documentation

- **Fixed inaccurate Copilot MCP claim in docs** — `README.md` and `docs/mcp-guide.md` incorrectly listed "Copilot" as an MCP-compatible client. GitHub Copilot does not read `.vscode/mcp.json`, does not launch local stdio servers, and is not an MCP client. Changed "(VS Code Roo, Copilot, Claude)" → "(Roo Code, Cline, or any MCP-compatible client)" in both files.

- **CLI help, LLM instructions, and documentation updated for new features** — 6 documentation changes across the codebase:
  1. `src/cli/args.rs` — Added 5 missing tools to AVAILABLE TOOLS list (`xray_git_blame`, `xray_branch_status`, `search_git_pickaxe`, `xray_help`, `xray_reindex_definitions`), bringing the list from 11 to 16 tools
  2. `src/tips.rs` — Added 3 new tips (branch status check, pickaxe usage, noCache parameter), 1 new "Code History Investigation" strategy recipe, git tools brief mention in `render_instructions()`, and `xray_branch_status` in tool priority list
  3. `docs/mcp-guide.md` — Added "File Not Found Warning" section documenting the `warning` field in git tool responses when a file doesn't exist in git
  4. `docs/cli-reference.md` — Added `[GIT]` example output line to `xray info` section
  5. `README.md` — Added "Branch awareness" feature mention for `branchWarning`
  6. `docs/use-cases.md` — Added "When Was This Error Introduced?" use case showing `xray_branch_status` → `search_git_pickaxe` → `xray_git_authors` → `xray_git_diff` workflow

### Features

- **Type inference improvements for `xray_callers` (7 user stories)** — Improved recall for `verify_call_site_target()` by adding 6 new type inference paths for local variables in C#:
  1. **Return type inference (US-1)**: `var stream = GetDataStream()` now resolves to the return type of same-class methods via signature parsing. Uses `parse_return_type_from_signature()` with angle-bracket-aware tokenization for generic types.
  2. **Cast expression (US-2)**: `var reader = (PackageReader)obj` → `reader : PackageReader`
  3. **`as` expression (US-3)**: `var reader = obj as PackageReader` → `reader : PackageReader`
  4. **`await` + Task unwrap (US-5)**: `var stream = await GetStreamAsync()` where return type is `Task<Stream>` → unwraps to `stream : Stream`. Handles `Task<T>` and `ValueTask<T>`.
  5. **Extension method detection (US-6)**: Builds extension method index during definition parsing (static classes with `this` parameter methods). `verify_call_site_target()` accepts extension method calls regardless of receiver type.
  6. **Pattern matching (US-7)**: `if (obj is PackageReader reader)` and `case StreamReader reader:` → extracts type from `declaration_pattern` AST node.

  US-4 (`using var`) was verified to already work — tree-sitter C# parses it as `local_declaration_statement`. 23 new unit tests.

- **`search_git_pickaxe` MCP tool** — New tool that finds commits where specific text was added or removed using git pickaxe (`git log -S`/`-G`). Unlike `xray_git_history` which shows all commits for a file, pickaxe finds exactly the commits where a given string or regex first appeared or was deleted. Supports exact text (`-S`) and regex (`-G`) modes, optional file filter, date range filters, and `maxResults` limit. Patch output truncated to 2000 chars per commit. Tool count: 16. 14 new unit tests.

- **`xray_branch_status` MCP tool** — New tool that shows the current git branch status before investigating production bugs. Returns: current branch name, whether it's main/master, how far behind/ahead of remote main, uncommitted (dirty) files list, last fetch timestamp with human-readable age, and a warning if the index is built on a non-main branch or is behind remote. Fetch age warnings use thresholds: < 1 hour (none), 1–24 hours (info), 1–7 days (outdated), > 7 days (recommend fetch). Tool count: 15. 14 new unit tests (6 handler tests + 8 helper function tests).

- **`branchWarning` in index-based tool responses** — When the MCP server is started on a branch other than `main` or `master`, all index-based tool responses (`xray_grep`, `xray_definitions`, `xray_callers`, `xray_fast`) now include a `branchWarning` field in the `summary` object: `"Index is built on branch '<name>', not on main/master. Results may differ from production."` The branch is detected at server startup via `git rev-parse --abbrev-ref HEAD`. Warning is absent on `main`/`master`, when not in a git repo, or when git is unavailable. Git tools are not affected (they query git directly). 7 new unit tests.

- **Empty results validation in `xray_git_history`** — When `xray_git_history` returns 0 commits, the tool now checks whether the queried file is tracked by git. If the file doesn't exist in git, the response includes a `"warning"` field: `"File not found in git: <path>. Check the path."`. This helps users distinguish between "no commits in the date range" and "wrong file path". Works in both cache and CLI fallback paths. New `file_exists_in_git()` helper function. 5 new unit tests, 2 new E2E test scenarios (T70, T70b).

- **`noCache` parameter for git tools** — Added `noCache` boolean parameter to `xray_git_history`, `xray_git_authors`, and `xray_git_activity`. When `true`, bypasses the in-memory git history cache and queries git CLI directly. Useful when cache may be stale after recent commits. Default is `false` (use cache when available). 5 new unit tests.

### Performance

- **Trigram pre-warming on server start** — Added `ContentIndex::warm_up()` method that forces all trigram index pages into resident memory after deserialization. Previously, the first 1-2 substring queries took ~3.4 seconds due to OS page faults on freshly deserialized memory. Pre-warming touches all trigram posting lists, token strings, and inverted index HashMap buckets in a background thread at server startup, eliminating the cold-start penalty without delaying server readiness. Runs after both the disk-load fast path and the background-build path. Stderr logging: `[warmup] Starting trigram pre-warm...` / `[warmup] Trigram pre-warm completed in X.Xms (N trigrams, M tokens)`. 4 new unit tests.

### Internal

- **Substring search timing instrumentation** — Added `[substring-trace]` `eprintln!` timing traces to `handle_substring_search()` in `grep.rs` for diagnosing slow cold-start substring queries (~3.4s on first 1-2 queries). Traces cover 8 stages: terms parsing, trigram dirty check + rebuild, trigram intersection (per term), token verification (`.contains()`), main index lookups, file filter checks, response JSON building, and total elapsed time. Always-on via stderr (no feature flag), does not interfere with MCP protocol on stdout. Also instruments the trigram rebuild path in `handle_xray_grep()`. E2E test plan updated with T-SUBSTRING-TRACE scenario.

### Features

- **Git history cache in `xray info` / `xray_info`** — The `info` CLI command and MCP `xray_info` tool now display `.git-history` cache files alongside existing index types (`.file-list`, `.word-search`, `.code-structure`). CLI output shows `[GIT]` entries with branch, commit count, file count, author count, HEAD hash (first 8 chars), size, and age. MCP JSON output includes `type: "git-history"` entries with full metadata. Previously, `.git-history` cache files existed on disk but were silently skipped by the info command. 4 new unit tests.

### Bug Fixes

- **File-not-found warning in `xray_git_authors` and `xray_git_activity`** — When these tools return 0 results and a `path`/`file` parameter was provided, they now check whether the path exists in git. If not found, the response includes `"warning": "File not found in git: <path>. Check the path."` — matching the existing behavior of `xray_git_history`. Works in both cache and CLI fallback paths. 4 new unit tests.

- **7 bugs found and fixed via code review** — Comprehensive code review of `callers.rs`, `grep.rs`, and `utils.rs` found 7 bugs (2 major, 4 minor, 1 cosmetic). All fixed with tests:
  - **`is_implementation_of` dead code in production (BUG-CR-2, MAJOR)** — `verify_call_site_target()` lowercased both arguments before calling `is_implementation_of()`, which checks for uppercase `'I'` prefix — always returned false. Fuzzy DI matching (e.g., `IDataModelService` → `DataModelWebService`) never worked in the call verification path. Unit tests passed because they called the function with original-case inputs directly. **Fix:** pass original-case values from `verify_call_site_target()`. 2 new regression tests.
  - **`xray_grep` ext filter single-string comparison (BUG-CR-1)** — `xray_grep` compared the ext filter as a whole string (e.g., `"cs" == "cs,sql"` → false), while `xray_callers` correctly split by comma. Extracted shared `matches_ext_filter()` helper. Also fixed misleading doc: schema said "(default: server's --ext)" but actual default was None. 5 new unit tests.
  - **`inject_body_into_obj` uses `read_to_string` (BUG-CR-6)** — Files with non-UTF-8 content (Windows-1252) failed body reads while the definition index was built with `read_file_lossy`. Now uses `read_file_lossy` for consistency. ~44 lossy files no longer show `bodyError`.
  - **Normal grep mode missing empty terms check (BUG-CR-7)** — `terms: ",,,"` silently returned empty results in normal mode but gave an explicit error in substring mode. Added consistent empty terms check.
  - **`maxTotalNodes: 0` returns empty tree (BUG-CR-3)** — `0 >= 0` was always true, causing immediate return. Now treats 0 as unlimited (`usize::MAX`).
  - **`direction` parameter accepts any value as "down" (BUG-CR-4)** — `"UP"`, `"sideways"`, etc. silently ran as "down". Added validation with case-insensitive comparison.
  - **Warnings array shows only first warning (BUG-CR-5, cosmetic)** — Changed from `summary["warning"]` (singular string) to `summary["warnings"]` (array) for future-proofing. **Breaking change** for consumers reading `warning` key.

- **`xray_grep` substring `matchedTokens` data leak (BUG-7)** — `matchedTokens` in substring search responses was populated from the global trigram index before applying `dir`/`ext`/`exclude` filters, showing tokens from files outside the requested scope. Now `matchedTokens` only includes tokens that have at least one file passing all filters. Affects `countOnly` and full response modes.

- **Input validation hardening (6 bugs fixed)** — Systematic input validation improvements across MCP tools, found via manual fuzzing:
  - `xray_definitions`: `name: ""` now treated as "no filter" instead of returning 0 results (BUG-1)
  - `xray_definitions`: `containsLine: -1` now returns error instead of silently returning ALL definitions (BUG-2, most critical)
  - `xray_callers`: `depth: 0` now returns error instead of empty tree (BUG-3)
  - `xray_git_history`/`xray_git_diff`/`xray_git_activity`: reversed date range (`from > to`) now returns descriptive error instead of silently returning 0 results (BUG-4)
  - `xray_fast`: `pattern: ""` now returns error instead of scanning 97K files for 0 results (BUG-5)
  - `xray_grep`: `contextLines > 0` now auto-enables `showLines: true` instead of silently ignoring context (BUG-6)

- **Panic-safety in background threads** — `.write().unwrap()` on `RwLock` in `serve.rs` (4 places) replaced with `.write().unwrap_or_else(|e| e.into_inner())` to handle poisoned locks gracefully (MAJOR-1). `.join().unwrap()` on thread handles in `index.rs` and `definitions/mod.rs` replaced with `unwrap_or_else` + warning log to survive individual worker thread panics during index building (MAJOR-2).

- **Mutex `into_inner().unwrap()` → graceful recovery** — Added `recover_mutex<T>()` helper in `src/index.rs` that handles poisoned mutex with a warning log instead of panicking. Applied to 3 locations: file index build (`src/index.rs`), content index build (`src/index.rs`), and definition index build (`src/definitions/mod.rs`). Consistent with the `.lock().unwrap_or_else(|e| e.into_inner())` pattern already used for mutex lock operations throughout the codebase.

- **`format_blame_date` timezone offset not applied** — `format_blame_date()` in `src/git/mod.rs` now applies the timezone offset string (e.g., `+0300`, `-0500`, `+0545`) to the Unix timestamp before civil date calculation. Previously, the timezone string was displayed but not used in the date math, causing all blame dates to show UTC time regardless of the author's timezone. Added `parse_tz_offset()` helper. 5 new tests for timezone formatting and 9 assertions for offset parsing.

- **`next_day()` broken fallback** — The `next_day()` function in `src/git/mod.rs` previously appended `T23:59:59` to unparseable date strings, producing invalid git date arguments. Now logs a warning and returns the original date string unchanged. This path is unreachable in practice (`validate_date()` is always called first), but the fix prevents silent corruption if the code path is ever reached. 1 new test for malformed date fallback.

---

## 2026-02-20

### Features

- **Git filter by author** — Added `author` parameter to `xray_git_history`, `xray_git_diff`, and `xray_git_activity`. Case-insensitive substring match against author name or email. Works with both cache and CLI fallback paths. Example: `"author": "alice"` returns only commits by Alice.

- **Git filter by commit message** — Added `message` parameter to `xray_git_history`, `xray_git_diff`, `xray_git_activity`, and `xray_git_authors`. Case-insensitive substring match against commit subject. Combinable with `author` and date filters. Example: `"message": "fix bug"` returns only commits with "fix bug" in the message.

- **Directory ownership in `xray_git_authors`** — `xray_git_authors` now accepts a `path` parameter (file or directory path, or omit for entire repo). `file` remains as a backward-compatible alias. Directory paths return aggregated authors across all files under that directory with proper commit deduplication. Omitting `path` entirely returns authors for the entire repository.

- **`xray_git_blame` tool** — New MCP tool for line-level attribution via `git blame --porcelain`. Parameters: `repo` (required), `file` (required), `startLine` (optional, 1-based), `endLine` (optional). Returns commit hash (8-char short), author name, email, date (with timezone), and line content for each blamed line. Always uses CLI. Total tool count: 14.

### Internal

- **Git feature unit tests** — Added 30 new unit tests across 4 feature areas: (1) Author/message filtering for `query_file_history`, `query_authors`, `query_activity` — 18 tests covering case-insensitive author/email matching, message substring filter, combined filters, and date+author combinations; (2) Directory ownership — 1 test for whole-repo `query_authors`; (3) Git blame — 5 tests for `blame_lines()` (success, single line, nonexistent file, bad repo, content verification); (4) Blame porcelain parser — 4 tests for `parse_blame_porcelain()` (basic, repeated hash reuse, empty input) and `format_blame_date()`. Also made `parse_blame_porcelain` and `format_blame_date` `pub(crate)` for test access, fixed pre-existing tool count assertion (13→14), and updated all existing test call sites to match new 6-arg `query_file_history`, 5-arg `query_authors`, 5-arg `query_activity`, 7-arg `file_history`, 5-arg `top_authors`, 4-arg `repo_activity` signatures.

- **Git cache test coverage** — Closed 5 test coverage gaps in the git history cache module (`src/git/cache_tests.rs`): (1) integration test for `build()` with a real temp git repo (`#[ignore]`), (2) bad timestamp parsing — verifies commits with non-numeric timestamps are skipped, (3) author pool overflow boundary — verifies error at 65536 unique authors and success at 65535, (4) `cache_path_for()` different directories produce different paths, (5) E2E test in `e2e-test.ps1` for `xray_git_history` cache routing. Total: 5 new unit tests + 1 E2E test.

### Bug Fixes

- **Git CLI date filtering timezone fix** — The `add_date_args()` function in `src/git/mod.rs` now appends `T00:00:00Z` to `--after`/`--before` date parameters, forcing UTC interpretation. Previously, bare `YYYY-MM-DD` dates were interpreted in the local timezone by git, causing a ±N hour mismatch with the cache path (which always uses UTC timestamps). This could cause `xray_git_history` CLI fallback to miss commits at day boundaries on non-UTC systems. Affects `xray_git_history`, `xray_git_diff`, `xray_git_authors`, and `xray_git_activity` CLI paths. 23 new diagnostic unit tests added for date conversion, timestamp formatting, and cache query boundary conditions.

- **Git cache progress logging** — The git cache background thread now emits `[git-cache]` progress messages during startup and build, preventing the appearance of a "stuck" server when building the cache for large repos (3+ minutes). Messages include: initialization, branch detection, disk cache validation, build progress every 10K commits, and completion summary.

- **`xray_git_authors` missing `firstChange` on cached path** — The cached code path for `xray_git_authors` now correctly returns the `firstChange` timestamp instead of an empty string. Added `first_commit_timestamp` field to `AuthorSummary` in the cache module.

### Features

- **Git history cache background build + disk persistence (PR 2c)** — The git history cache is now built automatically in a background thread on server startup, saved to disk (`.git-history` file, bincode + LZ4 compressed), and loaded from disk on subsequent restarts (~100 ms vs ~59 sec full rebuild). HEAD validation detects stale caches: if HEAD matches → use disk cache; if HEAD changed (fast-forward) → rebuild; if HEAD changed (force push/rebase) → rebuild; if repo re-cloned → rebuild. Commit-graph hint emitted at startup if `.git/objects/info/commit-graph` is missing. Key changes:
  - Background thread in `serve.rs` following existing content/definition index pattern (copy-paste, no refactor)
  - `save_to_disk()` / `load_from_disk()` methods using atomic write (temp file + rename) and shared `save_compressed()`/`load_compressed()`
  - `cache_path_for()` constructs `.git-history` file path matching existing `.word-search`/`.code-structure` naming convention
  - `is_ancestor()` / `object_exists()` helpers for HEAD validation
  - `run_server()` now accepts `git_cache` and `git_cache_ready` Arc handles from `serve.rs`
  - 12 new unit tests for disk persistence, atomic write, corrupt file handling, format version validation

- **Git history cache handler integration (PR 2b)** — Integrated the git history cache into the MCP handler layer with cache-or-fallback routing. When the cache is ready (populated by background thread in PR 2c), `xray_git_history`, `xray_git_authors`, and `xray_git_activity` use sub-millisecond cache lookups instead of 2-6 sec CLI calls. When cache is not ready, handlers transparently fall back to existing CLI code (zero regression). `xray_git_diff` always uses CLI (cache has no patch data). Cache responses include `"(from cache)"` hint in the summary field. Key changes:
  - `HandlerContext` gains `git_cache: Arc<RwLock<Option<GitHistoryCache>>>` and `git_cache_ready: Arc<AtomicBool>` fields
  - Date conversion helpers: YYYY-MM-DD → Unix timestamp (Howard Hinnant algorithm) for cache query compatibility
  - Path normalization applied to `file` parameter before cache lookup
  - Response format matches CLI output exactly (same JSON structure, field names, date format)

- **Git history cache core module (PR 2a)** — Added `src/git/cache.rs` with compact in-memory cache for git history. Designed for sub-millisecond queries (vs 2-6 sec per file via CLI). Key components:
  - `GitHistoryCache` struct: compact representation (~7.6 MB for 50K commits × 65K files)
  - `CommitMeta`: 40-byte per-commit metadata with `[u8;20]` hash, i64 timestamp, u16 author index, u32 subject pool offset/length
  - Streaming parser: parses `git log --name-only` output line-by-line (no 163 MB in RAM)
  - Query API: `query_file_history()`, `query_authors()`, `query_activity()` with date filtering and path prefix matching
  - Path normalization: `\` → `/`, strip `./`, collapse `//`, `"."` → `""`
  - Serialization: `#[derive(Serialize, Deserialize)]` for reuse with existing `save_compressed()`/`load_compressed()` (bincode v1 + lz4_flex)
  - 49 unit tests covering parser, queries, normalization, edge cases, and serialization roundtrip

- **Git history tools** — 4 new MCP tools for querying git history via git CLI with in-memory cache for sub-millisecond repeat queries. Always available — no flags needed:
  - `xray_git_history` — commit history for a file (hash, date, author, message)
  - `xray_git_diff` — commit history with full diff/patch (truncated to ~200 lines per commit)
  - `xray_git_authors` — top authors for a file ranked by commit count
  - `xray_git_activity` — repo-wide activity (all changed files) for a date range

  All tools support `from`/`to`/`date` filters and `maxResults` (default: 50). Performance: ~2 sec for single file, ~8 sec for full year in a 13K-commit repo. Response truncation via existing `truncate_large_response` mechanism.

- **Code complexity metrics (`includeCodeStats`)** — `xray_definitions` now computes and returns 7 code complexity metrics for methods/functions during AST indexing: cyclomatic complexity, cognitive complexity (SonarSource), max nesting depth, parameter count, return/throw count, call count (fan-out), and lambda count. Always computed when `--definitions` is used (~2-5% CPU overhead, ~7 MB RAM). Query with `includeCodeStats=true` to see metrics, or use `sortBy` (e.g., `sortBy='cognitiveComplexity'`) and `min*` filters (e.g., `minComplexity=10`, `minParams=5`) to find complex methods. Supports C# and TypeScript/TSX.

### Internal

- **Lowercase index filenames** — `sanitize_for_filename()` now lowercases all characters, producing consistent lowercase index filenames (e.g., `repos_myproject_a1b2c3d4.word-search` instead of `Repos_MyProject_a1b2c3d4.word-search`). Follows industry best practices (Cargo, npm, Docker all use lowercase). Prevents duplicate index files when the same path is referenced with different casing on case-insensitive filesystems. Old index files with uppercase names will be re-created automatically.

---

## 2026-02-18

### Features

- **Async MCP server startup** — server responds to `initialize` immediately; indexes are built in background threads. Tools that don't need indexes (`xray_help`, `xray_info`, `xray_find`) work instantly. Index-dependent tools return a "building, please retry" message until ready. ([PR #17](https://github.com/pustynsky/xray/pull/17))

- **Save indexes on graceful shutdown** — when the MCP server receives stdin close (VS Code stop), both content and definition indexes are saved to disk, preserving all incremental watcher updates across restarts. ([PR #18](https://github.com/pustynsky/xray/pull/18))

- **Phrase search with punctuation** — `xray_grep` with `phrase: true` now uses raw substring matching when the phrase contains non-alphanumeric characters (e.g., `</Property>`, `ILogger<string>`), eliminating false positives from tokenization stripping XML/code punctuation. Alphanumeric-only phrases continue to use the existing tokenized regex path. ([PR #19](https://github.com/pustynsky/xray/pull/19))

- **TypeScript call-site extraction for `xray_callers`** — `xray_callers` now works for TypeScript/TSX files. Supports method calls (`this.service.getUser()`), constructor calls (`new UserService()`), static calls, `super` calls, optional chaining (`?.`), and DI constructor parameter properties. Direction `"up"` and `"down"` both supported. ([PR #11](https://github.com/pustynsky/xray/pull/11))

- **TypeScript AST parsing** — added tree-sitter-based TypeScript/TSX definition parsing for `xray_definitions`. Extracts classes, interfaces, methods, properties, fields, enums, constructors, functions, type aliases, and variables. ([PR #9](https://github.com/pustynsky/xray/pull/9))

- **`includeBody` for `xray_definitions`** — returns actual source code inline in definition results, eliminating the need for follow-up `read_file` calls. Controlled via `maxBodyLines` and `maxTotalBodyLines` parameters. ([PR #2](https://github.com/pustynsky/xray/pull/2))

- **Substring search** — `xray_grep` now supports substring matching (enabled by default). Search term `"service"` matches tokens like `userservice`, `servicehelper`, etc. Powered by trigram index for fast lookup. ([PR #3](https://github.com/pustynsky/xray/pull/3))

- **`--metrics` CLI flag** — displays index build metrics (file count, token count, definition count, build time) when building indexes. ([PR #4](https://github.com/pustynsky/xray/pull/4))

- **Benchmarks** — added `benches/search_benchmarks.rs` with criterion-based benchmarks for index operations. ([PR #5](https://github.com/pustynsky/xray/pull/5))

- **LZ4 compression for index files** — all index files (`.idx`, `.cidx`, `.didx`) are now LZ4-compressed on disk, reducing total size by ~42% (566 MB → 327 MB). Backward compatible: legacy uncompressed files are auto-detected on load. ([PR #15](https://github.com/pustynsky/xray/pull/15))

- **`xray_callers` caps** — added `maxCallersPerLevel` and `maxTotalNodes` parameters to prevent output explosion for heavily-used methods. ([PR #12](https://github.com/pustynsky/xray/pull/12))

### Bug Fixes

- **Substring AND-mode false positives** — fixed a bug where AND-mode search (`mode: "and"`) returned false positives when a single search term matched multiple tokens via the trigram index. Now tracks distinct matched term indices per file. ([PR #16](https://github.com/pustynsky/xray/pull/16))

- **Lossy UTF-8 file reading** — files with non-UTF8 bytes (e.g., Windows-1252 `0x92` smart quotes) were silently skipped during indexing. Now uses `String::from_utf8_lossy()` with a warning log, preserving all valid content. ([PR #13](https://github.com/pustynsky/xray/pull/13))

- **Modifier bug** — fixed definition parsing issue with C# access modifiers. ([PR #6](https://github.com/pustynsky/xray/pull/6))

- **Code review fixes** — bounds checking, security validation for path traversal, stable hash for index file paths, underflow protection with `saturating_sub`, and monitoring improvements. ([PR #8](https://github.com/pustynsky/xray/pull/8))

- **Version desync** — MCP protocol version now derives from `Cargo.toml` via `env!("CARGO_PKG_VERSION")` instead of a hardcoded string. ([PR #16](https://github.com/pustynsky/xray/pull/16))

### Performance

- **Memory optimization** — eliminated forward index (~1.5 GB savings in steady-state) and added drop+reload pattern after build (~1.5 GB savings during build). Steady-state memory: ~3.7 GB → ~2.1 GB. ([PR #20](https://github.com/pustynsky/xray/pull/20))

- **Lazy parsers + parallel tokenization** — TypeScript grammars loaded lazily (only when `.ts`/`.tsx` files are encountered); content tokenization parallelized across threads. Index build time: ~150s → ~42s (3.6× faster). ([PR #14](https://github.com/pustynsky/xray/pull/14))

- **Eliminated ~100 MB allocation** — `reindex_definitions` response was serializing the entire index just to get its byte size. Replaced with `bincode::serialized_size()`. ([PR #16](https://github.com/pustynsky/xray/pull/16))

### Internal

- **Module decomposition** — extracted `cli/`, `mcp/handlers/`, and other modules from monolithic `main.rs`. ([PR #7](https://github.com/pustynsky/xray/pull/7))

- **Refactor: type safety and error handling** — introduced `SearchError` enum, eliminated duplicate type definitions, extracted `index.rs` and `error.rs` modules, fixed `total_tokens` drift in incremental updates, reduced binary size from 20.4 MB to 9.8 MB by removing incompatible SQL grammar, added 11 unit tests. ([PR #1](https://github.com/pustynsky/xray/pull/1))

- **Tips updated** — updated MCP server system prompt instructions (`src/tips.rs`). ([PR #10](https://github.com/pustynsky/xray/pull/10))

- **Documentation fixes** — various doc corrections and updates. ([PR #21](https://github.com/pustynsky/xray/pull/21))

- **Git history cache documentation and cleanup (PR 2d)** — Updated all documentation (README, architecture, MCP guide, storage model, E2E test plan, changelog) to reflect the git history cache feature. Added git cache to architecture overview table, module structure, and storage format descriptions. Verified no TODO/FIXME comments in cache module. No Rust code changes.

---

## Summary

| Metric                  | Value                       |
| ----------------------- | --------------------------- |
| Unit tests (latest)     | 1676 (with lang-rust)       |
| E2E tests (latest)      | 72                          |
| Binary size reduction   | 20.4 MB → 9.8 MB (−52%)     |
| Index size reduction    | 566 MB → 327 MB (−42%, LZ4) |
| Memory reduction        | 3.7 GB → 2.1 GB (−43%)      |
| Build speed improvement | 150s → 42s (3.6×)           |
