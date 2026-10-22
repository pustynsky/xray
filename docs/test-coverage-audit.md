# Test Coverage Audit

Date: 2026-08-02
Branch: `test/coverage-audit`
Base commit: `67cdc8489cb43aac25bd0a3b341ccef9128571b5`
Scope: unit, integration, persistence, process-boundary E2E, current CI

This audit reads production paths and test assertions. Test names and line coverage are not treated as correctness evidence. The audit branch adds characterization tests only and does not change production behavior. Canonical issues are #394, #395, and #403-#415; closed duplicates #417-#423 are not used or reopened.

## Coverage Levels

- **Full handler integration**: real parser/index data reaches a public MCP handler and response assertions verify the contract.
- **Builder integration**: real files reach index construction or incremental update, but no public handler is called.
- **Parser/helper only**: an internal parser, resolver, serializer, or tree builder is tested directly.
- **Persistence/restart gap**: save/load or process restart is not covered.
- **Process E2E gap**: the real MCP process and JSON-RPC boundary are not covered.
- **Desired, not implemented**: the requested public behavior does not exist on the base commit; a passing characterization test cannot prove it.

## Architectural Invariants

| Invariant | Language | Production path | Existing tests and assertions | Coverage level | Gap | Risk | Needed test | Issue |
|---|---|---|---|---|---|---|---|---|
| A same-file local callback must not bind to a same-named function in another file | TypeScript | `resolve_call_site_with_policy`, `handle_multi_method_callers` in [callers.rs](../src/mcp/handlers/callers.rs) | `test_ts_local_arrow_call_does_not_resolve_to_other_module_function` asserts ambiguous roots and that the local edge resolves to the caller file, not the other module | Full handler integration | An explicit negative assertion for an unresolved local with only a cross-file namesake would make the non-binding contract more direct | P1 false caller | `test_ts_unresolved_local_does_not_bind_cross_file_callee` | #403 only for future module identity; current local invariant is already implemented |
| Dynamic parameters, unresolved locals, imports, and unknown globals reduce exhaustive safety | TypeScript | `CallSiteKind::unresolved_reason`, `record_unresolved_call_reasons`, `attach_unresolved_call_status` | The reason-count block embedded in `test_ts_local_arrow_call_does_not_resolve_to_other_module_function` asserts all four exact counts plus `safeForExhaustiveClaims=false` | Full handler integration | Unknown/untyped member receivers are not represented as unresolved evidence; the existing status baseline is embedded in a broad arrow-call test rather than isolated | P0 trust failure | Failing regression with exact, known-mismatch, and unknown-receiver controls | #394, #410 |
| Responses identify the exact definition snapshot | All definitions/callers languages | Page metadata currently exposes `workspaceGeneration` and `indexEpoch`; no `indexGeneration` field exists | Existing workspace metadata tests assert only presence or `>= 1` | Desired, not implemented | Callers and definitions cannot prove which definition generation answered a query | P0 stale-snapshot ambiguity | Failing callers/definitions compatibility tests for exact `indexGeneration` | #410 |
| Query during an update reports the update state | All | Readiness gate in [mod.rs](../src/mcp/handlers/mod.rs) returns `INDEX_BUILDING` before initial content/definition readiness | `test_dispatch_*_index_building_returns_structured_envelope` assert error, phase, counters, hint, and message | Initial-build handler coverage | No `index_update_in_progress` state for a newer filesystem generation while an older complete snapshot remains visible | P0 false freshness | Deterministic edit/query race test | #410 |
| A clean single-file C# call resolves the expected graph without foreign reasons | C# | C# parser/resolver paths and `build_callers_result_status` in [callers.rs](../src/mcp/handlers/callers.rs) | `test_xray_callers_clean_csharp_result_has_expected_graph_without_foreign_reasons` asserts the sole node is `TypeA.methodB` and the reason list is empty | Full handler graph integration; no safety-flag claim | `safeForExhaustiveClaims` currently aliases structural `complete`, C# resolution uncertainty does not downgrade it, and `safeForExactSemantics` is hard-coded `false` | P0 status misreporting | Failing exact, ambiguous, and unknown-receiver controls before asserting C# safety flags | #410 |
| A synthetic SQL routine index without call edges is non-exhaustive and emits SQL-only reasons | SQL, negative C# reason control | `is_sql_routine_query`, `build_callers_result_status` | `test_sql_routine_without_call_edges_is_non_exhaustive_with_sql_only_reasons` requires both `no_call_graph_matches` and `sql_routine_call_graph_best_effort`, with no other reasons | Handler status with a manually constructed no-edge index | `is_sql_routine_query` marks every SQL routine semantically incomplete by kind; populated-parser and exact-graph controls are absent | P1 blanket-policy limitation | Real-file populated/empty/dynamic SQL controls before treating routine status as permanent | #410 |
| A same-file static Angular edge is navigable through both public direction paths | Angular/TypeScript | Separate DOWN-by-component and UP-by-selector traversal paths in [callers.rs](../src/mcp/handlers/callers.rs) | `test_angular_exact_edge_is_navigable_both_directions_through_public_handler` requires one edge and clean status in each direction for two components in one file | Full handler integration for one same-file, single-edge fixture | Cross-file identity is untested; UP has extra global blind-spot downgrades; depth-limited template traversal is not represented in status | P1 directional/status drift | Cross-file edge plus deeper-than-requested subtree status tests | #410, #411 |
| Inline templates reach derived child/parent indexes and public navigation | Angular/TypeScript | `build_definition_index`, template enrichment, `template_children`, `template_parents` | `test_definition_index_builds_angular_records_and_derived_indexes` asserts both maps; the new public-handler bidirectional navigation test traverses an inline template | Builder plus handler integration | No process restart assertion for inline derived indexes | P1 persistence/process regression | Restart E2E for inline template navigation | #411 |
| External HTML rename waits for matching TS metadata before rebinding | Angular/TypeScript | `reindex_paths_sync_scoped` and template owner refresh | `sync_reindex_html_rename_waits_for_ts_metadata_before_rebinding` proves the old TS owner remains while unavailable and its fingerprint is cleared, then a TS edit removes the old owner, binds the new path, restores derived maps, and advances generation once per phase | Builder/incremental lifecycle integration | OS watcher event delivery and restart are not covered | P1 silent owner misbinding | External OS rename plus TS edit through the real watcher process | #411 |
| Every `AngularTemplateSource` discriminant survives binary load and head reconstruction | Angular/TypeScript | `save_compressed`, `load_definition_index`, `DefinitionIndex::from_head_and_entries` | `test_angular_template_source_variants_survive_storage_roundtrip` uses a coherent manually constructed index and compares all component and derived maps in both paths | Serialization/reconstruction integration, not parser behavior | No separate-process cache discovery or query-after-restart coverage | P1 serialization/startup loss | Retain parser tests and add process restart parity | #411 |
| Old definition-index versions are rejected and rebuilt | All definitions languages | `load_definition_index` validates `DEFINITION_INDEX_VERSION`; startup cross-load decides load vs build | Storage tests reject legacy and previous versions and pin the current version | Parser/helper persistence only | No process test proves rejection followed by a successful rebuild and healthy query; future-version rejection is not named separately | P1 startup durability | Real process stale-version rejection/rebuild test | #411 |
| Feature combinations without TS/C#/SQL do not panic on load/query | Feature matrix | `def_index: None` paths, feature-gated parsers, handler dispatch | Watcher and cross-load tests cover `def_index=None`; CI runs seven `cargo check` feature variants | Helper/build coverage | No public `xray_definitions`/`xray_callers` runtime test with definitions disabled; SQL has no separate Cargo feature | P1 feature-only runtime panic | No-definition-index handler smoke tests | None; audit infrastructure gap |
| `xray_info` reports content, definitions, and file-list tiers independently | All | Info metadata and per-file coverage assembly | CLI helper tests serialize all detail kinds; handler tests cover content-only, definition-only, and combined flags in separate fixtures | Helper plus partial handler coverage | No single structured handler response proves independent file-list reachability; current contract lacks `indexedByFileList` and per-tier reasons | P0 coverage transparency | Failing three-tier handler contract test | #409, #395 for hidden-path reasons |
| Definition generation should be monotonic across watcher, edit, cache load, and background replacement | All definitions languages | `definition_generation`, `replace_live_definition_index`, watcher apply, edit reindex | Watcher lifecycle tests assert exact increments; replacement test covers only `current=7, replacement=11 -> 12`; edit test asserts only `index_epoch` | Partial integration | The reverse replacement case `current > replacement`, `xray_edit -> definition_generation`, older cache load, background build, and public response generation are not jointly asserted | P0 stale/regressed generation | Reverse replacement, edit, cache-load, and background-build monotonic tests | #410 |

## C# and SQL Parity Baseline

This baseline must be rerun before changing shared status, storage, walker, generation, or typed-edge APIs.

| Area | C# evidence | SQL evidence | Level | Remaining gap / risk |
|---|---|---|---|---|
| Direct, static, and instance calls | Real C# parser/handler tests find ordinary calls, but the persisted call kind does not distinguish all three forms | N/A | Handler behavior | Add one neutral real-file fixture with direct/static/instance controls; P1 |
| Constructors | `test_d20_object_creation_does_not_resolve_to_same_named_method` prevents false method binding | Constructor is not a SQL concept | Real parser/handler negative control | No positive constructor chain in UP and DOWN; P1 |
| Interface and DI | `test_xray_callers_down_respects_resolve_interfaces_policy`, DI roots, and same-name receiver tests assert policy fan-out and isolation | N/A | Full handler integration | Field-injected receiver is not isolated in one test; P2 |
| Overloads | UP/DOWN non-collapse tests plus argument applicability tests | Routine names and schema resolution tested | Mixed: synthetic tree plus real parser | Real-file bilateral overload symmetry is not one assertion; P2 |
| Partial methods | No dedicated parser or handler test | N/A | Missing | Desired C# behavior is not characterized; P1 |
| `cs:v1` | Storage roundtrip validates ID shape, symbol mapping, call shape, and corruption rejection | N/A | Persistence integration | No handler `targets[].symbolId` roundtrip after reload; P1 |
| UP/DOWN symmetry | Each direction has broad tests | SP caller and callee builders are tested separately | Behavior, not one bilateral invariant | Add C# and SQL exact-edge bilateral tests before typed-edge migration; #411 |
| Routine calls | N/A | EXEC dependencies and parser-emitted edges exclude comments/string-only references | Builder plus parser integration | Cross-database unresolved target needs a typed status test; P2 |
| Relations | N/A | `SqlRelation` is rejected by resolver and excluded by callee builder | Helper/builder only | Public handler exclusion test is absent; P2 |
| Scalar functions | N/A | Parser-emitted scalar function edges and XML `.value()` false-positive controls | Parser/builder integration | Nested scalar-function depth > 1 is absent; P2 |
| Unresolved/status | Clean C# graph shape and empty reasons are covered, but safety flags are untrusted | A synthetic SQL no-edge index is partial with SQL/generic reasons; all-routine blanket policy remains unverified | Partial handler characterization | Real-file controls and independent safety computation remain absent; #410 |
| Mixed extension filters | C# versus text extension isolation is tested | Pure SQL filtering is tested | Synthetic/helper | No same-name `.cs` + `.sql` mixed index control; P1 |
| Save/reload | C# semantic IDs and call shapes roundtrip | Generic call-site persistence is exercised indirectly | Persistence, not handler graph | No before/after handler graph equality for C# or SQL; P1 |
| Incremental update | C# semantic removal/update internals are tested | Parser rebuild tests exist | Helper/builder | No C# or SQL handler graph before/after incremental update; P1 |
| Disabled language builds | C# feature is independently checked | SQL parser is not a separate feature | CI compile only | Runtime query smoke tests remain absent; P1 |

### Baselines That Should Not Be Duplicated

- SQL EXEC edges from real parser output, including comment/string false-positive controls.
- SQL relation exclusion at resolver and callee-builder levels.
- C# interface policy fan-out and same-name receiver isolation.
- C# overload applicability and non-collapse in both directions.
- `cs:v1` storage roundtrip plus malformed hash/ordinal rejection.
- Initial `INDEX_BUILDING` envelopes for grep, definitions, and callers.
- The broad TypeScript handler test already asserts four reason counts: dynamic parameter, local, imported, and unknown-global. Add `ast_depth_limit` separately rather than duplicating those four.
- Angular delete/recreate at the same external template path.
- `replace_live_definition_index` covers only the fixed case `current=7, replacement=11 -> 12`; the reverse ordering remains a required gap.

## Canonical Issue Disposition

| Issue | Audit disposition |
|---|---|
| #394 | Direct P0 gap: unknown/untyped TypeScript member targets are omitted without making caller status non-exhaustive. Failing regression and production fix belong in `fix/truthful-caller-status`. |
| #395 | Direct P2 transparency gap: hidden paths collapse into generic exclusion reasons; the E2E audit also proves hidden `.github` cannot be discovered through the default index. No production change in this slice. |
| #403 | Foundational TypeScript module-identity work. Current same-file callback isolation is covered, while exact imported/re-exported cross-file resolution remains desired behavior requiring its own design and persistence tests. |
| #404 | Stable `ts:v1` IDs are not implemented and are blocked by #403/#406. Future tests must cover handler roundtrip, save/load, incremental update, rename stability, and old-version rejection; no passing characterization is possible now. |
| #405 | Direct platform/nightly gap: symlink, junction, cycle, UNC, hardlink, case alias, and workspace-escape behavior is not covered by the current E2E harness. |
| #406 | TypeScript callable signatures and overload groups remain a parser/identity design gap. Acceptance needs parser, definitions-handler, callers, persistence, and incremental tests; it does not alter the current characterization slice. |
| #407 | Static JSX component edges are not implemented and depend on #403. The current Angular selector graph is not evidence for JSX behavior; dedicated TSX process and persistence tests are required later. |
| #408 | Additive auto-summary cursor ergonomics is outside the requested correctness invariants. No audit-slice test is added; its acceptance should extend existing pagination/token tests without changing current record results. |
| #409 | Direct P0 gap: `xray_info` cannot independently prove content, definitions, and file-list reachability in one response. A failing three-tier contract test belongs with the production change. |
| #410 | Direct P0 gap and main follow-up slice: `safeForExactSemantics` is hard-coded `false`; `safeForExhaustiveClaims` aliases structural `complete` and misses C# resolution uncertainty; SQL incompleteness is keyed by routine kind; Angular depth truncation is not reflected. Typed reasons, exact `indexGeneration`, `index_update_in_progress`, monotonic apply, and no cross-language leakage are also missing. The audit records these weaknesses without asserting them as correct. |
| #411 | Direct architectural dependency: one persisted typed-edge store must preserve exact UP/DOWN inversion, old-version rejection, save/load, and incremental behavior. This slice characterizes Angular bidirectional navigation and template-source persistence. |
| #412 | Optional TypeScript member metadata is a bounded parser/persistence issue. Required tests are parser plus definitions handler, save/load, and old-version rejection; no current production contract is claimed here. |
| #413 | Static template-expression edges are not implemented. Existing Angular selector/template ownership tests are only a compatibility baseline; expression parsing needs external/inline parity, unresolved evidence, and bidirectional typed-edge tests. |
| #414 | The separate HTTP contract graph/tool is not implemented and must not be inferred from ordinary callers. Future coverage belongs to its independent contract index, process API, persistence, and lifecycle matrix. |
| #415 | Merged declaration views and typed method references are desired additive behavior blocked by #404/#406/#411. Current direct-call tests must remain negative controls so references never inflate caller counts. |


## Prioritized Gaps

### P0

| Gap | Exact production/test surface | Required regression | Issue |
|---|---|---|---|
| Unknown TypeScript member omission can still claim exhaustive coverage | `CallSiteKind::unresolved_reason`, TypeScript member resolution, `attach_unresolved_call_status`; add handler test in [handlers_tests_typescript.rs](../src/mcp/handlers/handlers_tests_typescript.rs) | `test_ts_unknown_member_receiver_is_non_exhaustive` with exact and known-mismatch controls | #394 |
| C# callers status cannot distinguish structurally complete from semantically exhaustive results | `build_callers_result_status` aliases `safeForExhaustiveClaims` to `complete`, hard-codes exact semantics false, and C# uncertainty has no unresolved-reason downgrade path | Failing exact, ambiguous overload/interface, and unknown-receiver controls followed by independent safety computation | #410 |
| No exact definition snapshot generation in callers/definitions responses | Status/page assembly in [utils.rs](../src/mcp/handlers/utils.rs), callers and definitions handlers | Compatibility tests for exact `indexGeneration` and monotonic apply | #410 |
| No update-in-progress status while a previous snapshot remains queryable | Watcher preparation/apply and handler gate | Deterministic barrier-based query/edit race test | #410 |
| Depth-limited Angular template traversal can report `complete=true` | Template-navigation status passes `truncated=false` and `per_level_dropped=0` independently of requested depth | Failing deeper-than-depth fixture that must downgrade exhaustive status or report bounded scope | #410, #411 |
| `xray_info` cannot independently prove all three index tiers | Info handler file metadata | Failing `xray_info_reports_independent_content_definition_file_list_coverage` | #409 |
| E2E is not run by CI | [.github/workflows/ci.yml](../.github/workflows/ci.yml), [e2e-test.ps1](../e2e-test.ps1) | Fast real-process Windows gate plus a portable Linux subset | No new issue created; delivery slice `test/e2e-refresh` |

### P1

| Gap | Exact production/test surface | Required regression | Issue |
|---|---|---|---|
| SQL semantic incompleteness is selected by routine kind rather than observed graph evidence | `is_sql_routine_query` returns true for every stored procedure and SQL function | Populated real-parser graph, empty routine, unresolved target, and dynamic SQL controls | #410 |
| `TypeScriptAnalysisIncomplete` / `ast_depth_limit` is absent from the existing four-reason handler fixture | `CallSiteKind::unresolved_reason` supports it, but the broad local-arrow test exercises only four other reasons | Dedicated handler assertion that depth-limited evidence downgrades exhaustive status | #410 |
| C#/SQL graph parity across save/load and incremental update | Definition storage, incremental update, public callers handler | Before/after equality and new-edge visibility for neutral `.cs` and `.sql` fixtures | #410/#411 shared contracts |
| Stale index rejection is not proven to rebuild a healthy process | Startup cross-load plus stale version | Real process restart with an intentionally old cache | #411 |
| Mixed `.cs`/`.sql` ext filters are not tested in one index | Callers ext filtering | Same-name cross-language isolation test | #410 |
| Runtime handlers with `def_index=None` are not covered | Definitions/callers dispatch | Graceful response/no-panic tests under disabled definitions | None |
| C# constructor chain and partial methods lack positive behavior tests | C# parser/call graph | Neutral UP/DOWN constructor fixture; characterize partial method behavior | None |
| Angular watcher/restart process scenarios are absent | Existing watcher integration tests, process harness | External and inline create/edit/delete/rename through MCP process | #411 |
| Platform alias safety is absent | Walkers follow links | Windows symlink/junction and workspace escape; Linux symlink/case cases | #405 |

### P2

- Public handler SQL relation exclusion and nested scalar-function traversal.
- Handler `cs:v1` target query after persistence reload.
- C# field-injected DI receiver characterization.
- Future-version cache rejection naming.
- Hidden path diagnostics and consistency across info/fast/grep (#395).
- Documentation ID cleanup and replacement of substring-only JSON assertions.

## Additional Weak-Test Findings

These existing tests are outside the current delivery diff. They are recorded as gaps rather than treated as correctness evidence:

| Priority | Existing test or surface | Why current evidence is weak | Required correction | Issue |
|---|---|---|---|---|
| P0 | `test_callers_template_navigation_ignores_inactive_root_posting` | The inactive duplicate root has no `template_children`, so DOWN traversal ignores it even without clearing its `file_index` posting; the test is vacuous for the stated inactive-posting behavior | Give both roots children, then prove clearing only the inactive posting removes ambiguity | #411 |
| P1 | `test_callers_template_navigation_reports_ambiguous_class_root`, `test_callers_template_navigation_up_reports_ambiguous_class_root`, and `test_callers_template_navigation_down_selector_reports_bounded_ambiguity` | They assert `rootResolution` but do not assert `resultStatus=partial`, `complete=false`, and non-exhaustive safety | Add public-status assertions to every ambiguous-root direction | #410, #411 |
| P1 | `test_callers_template_navigation_filters_child_candidates_before_ambiguity` | It proves one resolved child but not that the fully resolved response is complete within its declared scope | Assert result status and scope accounting after candidate filtering | #410, #411 |
| P1 | `transient_definition_read_race_preserves_graph_and_requeues_dirty_path` | It checks graph preservation and requeueing but not that the previous `input_fingerprints` entry survives the transient read failure | Assert fingerprint preservation so a transient error cannot silently force perpetual reparsing | #410, #411 |
| P1 | `test_xray_callers_typescript_root_ambiguity_candidates_are_truncated` | It asserts `status=partial` but not `complete=false` or `safeForExhaustiveClaims=false` | Assert the full ambiguity safety contract | #410 |
| P1 | Template navigation with `includeBody=true` | `callers_evidence_level` reports `full_body` when the body is complete, hiding `template_index` provenance; no test specifies the intended combined evidence contract | Add a handler test and define additive or composite provenance instead of silently replacing it | #410, #411 |
| P2 | `test_callers_template_navigation_reports_ambiguous_class_root` evidence-level assertion | `evidenceLevel="template_index"` is a direct function of the already asserted reason and adds little independent discrimination | Prefer a provenance contract test that contrasts template and AST paths | #410 |


## Current CI Evidence

The current tracked workflow is [.github/workflows/ci.yml](../.github/workflows/ci.yml).

| Job | OS | Command | E2E |
|---|---|---|---|
| `classify` | Ubuntu, PowerShell 7 | Policy regression tests and conservative PR change classification | Synthetic Git repositories |
| `setup` | Windows, PowerShell 5.1 and 7 | Local MCP setup, legacy migration, extension detection, and setup dependencies | Setup fixtures |
| `clippy` | Windows, Ubuntu | `cargo clippy --workspace --all-targets --locked -- -D warnings` | No |
| `test-windows`, `test-ubuntu` | Windows, Ubuntu | `cargo test --workspace --locked` | No |
| `setup-integration` | Windows, PowerShell 5.1 and 7 | `test-mcp-setup.ps1 -UserMcpOnly -XrayBinary ...` and `test-setup-xray-repos.ps1 -XrayBinary ...` | Real-process setup scenarios |
| `feature-matrix` | Ubuntu | default, all features, no default, and isolated C#/TypeScript/Rust/XML checks | No |
| `audit` | Ubuntu | `cargo audit --deny warnings --ignore RUSTSEC-2025-0141` | No |
| `deny` | Ubuntu | `cargo deny check` | No |
| `gate` | Ubuntu, PowerShell 7 | Validate every expected job result for the selected mode | No |

Pull requests skip heavy jobs only when every changed path is in the exact
allowlist in [ci-policy.ps1](../scripts/ci-policy.ps1). The list contains existing
documentation, license files, and selected Markdown agent/skill customizations.
New files outside the list, executable scripts, workflow changes, source code,
and mixed changes require full CI. Renames classify both old and new paths.
The diff covers the merge base through the PR head, not only its latest commit.
Invalid SHAs, missing or ambiguous history, or Git errors fail classification.

Pushes to `main` and manual `workflow_dispatch` runs always use full CI.
Use Actions **Run workflow** to select a ref for a manual Full once this workflow
is available on the default branch. No scheduled run is added.

New pushes cancel older runs of the same pull request. Every non-PR run uses
its own concurrency group keyed by run ID, so neither running nor pending main
and manual runs are displaced by another run.

The `CI gate` check runs even when dependencies fail or are skipped. It requires
successful classification and every heavy job to succeed in full mode or be
skipped in documentation mode. Missing results, cancellation, or an unexpected
skip fail the gate. A documentation-mode success explicitly reports that heavy
checks were not required; it does not claim they passed. Repository rules must
be configured separately if this check is to be required for merging.

Run the routing and gate regression tests locally with:

```powershell
powershell -NoProfile -File .\scripts\test-ci-policy.ps1
pwsh -NoProfile -File .\scripts\test-ci-policy.ps1
```

These tests use isolated Git repositories and synthetic job results. They do
not replace verification of job scheduling and `if`/`needs` behavior in Actions.

To verify documentation-only routing after a workflow change, open a PR that
changes only an already allowlisted document. Expect `CI policy` and `CI gate`
to succeed and every heavy job to be skipped. Check that the gate summary says
heavy checks were not required, rather than claiming they passed. A manual run
always selects Full and cannot verify this documentation-only path.

The Windows Rust job uploads its debug executable as a one-day artifact.
Both `setup-integration` jobs download that binary from the same workflow run
and run the full user and bulk setup suites in parallel. They depend only on
`test-windows`, not on the Ubuntu job. Rust test jobs have a 30-minute timeout;
local and integration setup jobs have a 20-minute timeout.

Re-running the Windows job replaces its artifact. Once the one-day retention
expires, use "Re-run all jobs" rather than "Re-run failed jobs" so the Windows
job builds and uploads the executable again.

These setup scenarios do not replace the broader public-tool E2E suite.
No workflow invokes [e2e-test.ps1](../e2e-test.ps1).

## E2E Documentation to Execution Map

`docs/e2e-test-plan.md` is only a redirect to [docs/e2e/README.md](e2e/README.md). The executable product harness is [e2e-test.ps1](../e2e-test.ps1). `scripts/mcp-filter/test-*.ps1` test installer/filter behavior and are a separate suite.

| Scenario ID | Documented | Implemented | Actually runs | CI | Platform | Status | Gap |
|---|---|---|---|---|---|---|---|
| T06-T20, T24, T49, T61-T64 | Yes | Sequential CLI block | Yes, local full harness | No | Windows in current script | Local-only, many exit-code assertions | Parse and assert JSON/content, make binary/cache paths portable |
| T21, T22, T65-fast, T76, T80 | Yes | Sequential error block | Yes, local | No | Windows in current script | Local-only | ID collisions and semantic drift; structured error assertions needed |
| T25 initialize | Yes | No dedicated block | No | No | Any process | Manual-only | Assert protocol version, capabilities, server info, and instructions |
| T26 tools/list | Yes | No JSON-RPC block; help text checks four names | No | No | Any process | Missing | Invoke `tools/list` and assert the full current tool set and schemas |
| T27/T27a/T27b grep MCP | Yes | Unit coverage only | No process E2E | No | Any | Unit-only | Basic JSON-RPC request and grouped `showLines` contract |
| T28-T28g definitions | Yes | Unit coverage only | No process E2E | No | Any | Unit-only | Body, range, budget, and `containsLine` through real process |
| T29a-T29f callers body budgets | Yes | Unit coverage only | No process E2E | No | Any | Unit-only | Real process body/truncation assertions |
| T31/T32 callers field and multi-ext | Yes | No matching E2E block | No | No | Any | Manual-only | Add neutral fixtures and structured response checks |
| T33-T37e grep modes | Yes | Mostly unit tests; CLI equivalents partly run | Partial | No | Windows current harness | Mixed | Docs must distinguish unit, CLI, and process coverage |
| T39-T40/T42c instructions/help | Yes | Partial string checks | Yes for partial blocks | No | Windows current harness | Shallow | Structured initialize/help response assertions |
| T44-T51 TypeScript definitions/watcher | Yes | T49 CLI only; watcher scenarios incomplete | Partial | No | Windows current harness | Manual/unit-only | Process create/edit/delete/rename and grammar assertions |
| T52 response truncation | Yes | Unit only | No | No | Any | Unit-only | Process response cap contract |
| T53-T58 TypeScript callers | Yes | Stale manual note; T65-69 overlap some behavior | Partial | No | Windows current harness | Duplicated/drifted IDs | Reconcile IDs and retain only missing process scenarios |
| T59 ambiguity warning | Yes | Unit only | No | No | Any | Unit-only | Process ambiguity envelope |
| T65-69 TypeScript caller blocks | Partly, with ID collision | Parallel jobs | Yes, local | No | Windows current harness | Local-only | Rename IDs; add generation/status assertions |
| T-FIX3-* C# parser/caller controls | Yes | Parallel jobs | Yes, local | No | Windows current harness | Local-only | Add status and persistence parity |
| T-OVERLOAD-DEDUP-UP | Yes | Parallel job | Yes, local | No | Windows current harness | Local-only | DOWN remains unit-only |
| T-SAME-NAME-IFACE | Yes | Parallel job | Yes, local | No | Windows current harness | Local-only | Keep as fast C# baseline |
| T-SQL | Yes | Definitions-only parallel job | Yes, local | No | Windows current harness | Incomplete | T-SQL-05/05b/06 callers and mixed-language isolation absent |
| T-ANGULAR | Yes | Parallel job | Yes, local | No | Windows (`LOCALAPPDATA`) | Shallow/local-only | Structured selector/template arrays; lifecycle and rename missing |
| T-SEARCH-INFO-MCP | Yes | Parallel job | Yes, local | No | Windows current harness | Shallow | Does not prove independent tiers (#409) |
| T-EDIT/T-EDIT-MULTI/T-EDIT-CREATE | Yes | Parallel jobs | Yes, local | No | Mostly portable after binary fix | Local-only | Couple edits to exact generation assertions |
| T-SYNC-GREP/DEFS/MULTI/FAST | Yes | Parallel jobs | Yes, local | No | Mostly portable after binary fix | Good local process coverage | Add callers and exact generation assertions |
| T-SYNC-RECONCILE-PRESERVED | Yes | Parallel watcher job | Yes, local | No | Timing-sensitive | Local-only | Move to platform/nightly suite |
| T-RECONCILE/T-BATCH-WATCHER | Yes | Parallel jobs | Yes, local | No | Timing-sensitive | Local-only | Move to nightly and replace fixed waits with bounded readiness polling |
| T-SHUTDOWN | Yes | Sequential process test | Yes, local | No | Windows cache path | Local-only | Portable cache root and deterministic shutdown evidence |
| T-CHECKPOINT-AFTER-RECONCILE | Yes | Parallel force-kill/restart job | Yes, local | No | Windows/timing-sensitive | Partial durability E2E | Expand to explicit force-kill durability matrix |
| T-FORMAT-VERSION | Yes | Sequential stale-cache test | Yes, local | No | Windows binary/cache assumptions | Local-only | Keep in fast gate after portability and structured rebuild assertion |
| T-GREP-STALE | Yes | Sequential stale index test | Yes, local | No | Windows binary assumption | Local-only | Structured generation/rebuild assertion |
| T-GIT-* and T-BRANCH-* | Yes | Several parallel jobs | Yes, local | No | Git-dependent | Mixed assertion quality | Replace substring checks with parsed JSON; many documented cases remain unit-only |
| T79/T79a-d/T81 fast MCP | Yes | Unit coverage plus selected fast jobs | Partial | No | Windows current harness | Mixed | `dirsOnly`, `fileCount`, and `maxDepth` process contracts absent |
| T83/T84 callers edge cases | Yes | No matching process blocks | No | No | Any | Missing | Reconcile with current unit tests or automate |
| T87-T102 TypeScript handlers | Yes | Unit tests | No process E2E | No | Any | Unit-only | Docs should label these as unit scenarios |
| T-LZ4 | Yes | Unit only | No process E2E | No | Any | Unit-only | Persist/restart process roundtrip and magic/version evidence |
| T-ASYNC-01..09 | Yes | Unit only | No process E2E | No | Any | Unit-only | Real startup/update race scenarios in fast/nightly split |
| T-CTRLC | Yes | Manual only | No automation | No | Platform-specific | Manual-only | Platform suite with signal-specific implementation |
| T-MEMORY-ESTIMATE/T-MI-* | Yes | Unit or manual checks | No process budget gate | No | Platform-specific | Missing | Nightly corpus, RSS, and latency budgets |
| `scripts/mcp-filter/test-*.ps1` | Separate script docs/comments | Nine scripts | Only when invoked directly | No | Windows-centric | Orphan suite | Add Windows nightly installer/filter job |
| Windows symlink/junction/UNC/escape | Required by audit | No complete suite | No | No | Windows | Missing | Platform suite; #405 |
| Linux case/wrong-case import | Required by audit | No | No | No | Linux | Missing | Platform suite; tie exact import behavior to #403 |
| Concurrent clients/edit-query races | Required by audit | No | No | No | Cross-platform | Missing | Nightly deterministic synchronization, not sleeps |
| Large corpus/watcher burst/memory/latency | Partly documented | No enforceable budget suite | No | No | Cross-platform | Missing | Nightly corpus and thresholds |

### E2E Drift and Dead IDs

- `T65` is both sequential fast-invalid-regex and a TypeScript callers block.
- `T30` means grep subdirectory in one document and callers DOWN in another.
- `T80` documents fast regex but executes grep nonexistent-directory.
- The final harness note that T25-T52 and T53-T58 are manual is stale; several overlapping parallel jobs now execute.
- `T-MULTI-METHOD` is preceded by a detached `T-EDIT-MULTI` comment.
- Many sequential `Run-Test` cases assert only exit code; several parallel cases match raw JSON substrings instead of deserializing the envelope.
- Current binary resolution hardcodes `xray.exe`; `LOCALAPPDATA` is assumed in index checks. Running the existing script under Linux `pwsh` is not currently valid.

## Runner Options

| Option | Advantages | Cost and risk | Decision |
|---|---|---|---|
| Existing PowerShell through `pwsh` on Windows and Linux | Reuses 80+ scenarios, JSON tooling and process control already exist | Needs portable binary/cache paths, suite selectors, structured assertion helpers, and removal of Windows-only assumptions | Recommended for `test/e2e-refresh`; estimated fast 30-45 s Windows and 45-75 s Linux after fixes |
| Rust integration runner | Best type safety, deterministic in-process tests, already part of `cargo test` | Rewriting the whole harness duplicates large scenario inventory; process/signal/platform tests remain verbose | Use only for a small protocol smoke runner if PowerShell cannot provide deterministic synchronization; full port is not justified now |
| Separate shell script | Native on Linux | Creates a second harness and doubles maintenance | Rejected; do not create automatically |

## Proposed Suite Split

### Fast PR Gate

Target: 45-60 seconds on Windows; portable subset 60-90 seconds on Ubuntu.

- Spawn the real MCP process; validate `initialize`, `tools/list`, and one structured JSON-RPC request.
- `xray_edit` followed immediately by grep, definitions, and callers with exact generation assertions.
- One external watcher edit and one batch edit using bounded readiness polling.
- Restart/load persisted content, definitions, and file-list indexes.
- Reject an old definition-index version, rebuild, and answer a query.
- Truthful callers status for TypeScript, C#, and SQL.
- Angular external and inline create/edit/delete/rename.
- Mixed-language filter isolation and `xray_info` three-tier coverage.
- C#/SQL parity smoke matrix.

### Platform Suite

Target: 2-4 minutes per supported OS, scheduled and manually dispatchable.

- Windows: symlink, junction, UNC, case-only aliases, and workspace escape.
- Linux: case sensitivity, wrong-case imports, symlink cycles, and workspace escape.
- Platform signal handling (`Ctrl+C`/SIGINT) and cache root behavior.
- Installer and MCP filter scripts on Windows.

### Nightly Stress and Durability

Target: 10-20 minutes with explicit budgets.

- Concurrent clients and deterministic edit/query races.
- Force-kill/restart durability and partial-write recovery.
- Large corpus cold/warm load, watcher bursts, memory ceilings, and p50/p95 latency.
- Repeated persistence/restart cycles across all enabled language parsers.

## Changes in the Audit Slice

No production files are changed.

| Test | File | Contract |
|---|---|---|
| `test_xray_callers_clean_csharp_result_has_expected_graph_without_foreign_reasons` | [handlers_tests_csharp_callers.rs](../src/mcp/handlers/handlers_tests_csharp_callers.rs) | Exact single-file C# graph shape and empty reasons; no safety flags are accepted as correct |
| `test_sql_routine_without_call_edges_is_non_exhaustive_with_sql_only_reasons` | [callers_tests.rs](../src/mcp/handlers/callers_tests.rs) | Synthetic no-edge SQL status and reason isolation; not an all-routine completeness invariant |
| `test_angular_exact_edge_is_navigable_both_directions_through_public_handler` | [handlers_tests_typescript.rs](../src/mcp/handlers/handlers_tests_typescript.rs) | Same-file static single edge and clean status through distinct DOWN/UP handler paths |
| `sync_reindex_html_rename_waits_for_ts_metadata_before_rebinding` | [watcher_tests.rs](../src/mcp/watcher_tests.rs) | Two-phase unavailable state then metadata-driven cleanup/rebind; one generation per phase |
| `test_angular_template_source_variants_survive_storage_roundtrip` | [storage_tests.rs](../src/definitions/storage_tests.rs) | Coherent manually constructed state preserves all five discriminants and derived maps through binary load/head reconstruction |

## Validation Record

Targeted tests completed before the broad gates:

- `test_xray_callers_clean_csharp_result_has_expected_graph_without_foreign_reasons`: pass.
- `test_sql_routine_without_call_edges_is_non_exhaustive_with_sql_only_reasons`: pass.
- `test_angular_exact_edge_is_navigable_both_directions_through_public_handler`: pass.
- `sync_reindex_html_rename_waits_for_ts_metadata_before_rebinding`: pass.
- `test_angular_template_source_variants_survive_storage_roundtrip`: pass.

Cargo and process commands use `CARGO_INCREMENTAL=0` and an isolated global Git configuration. Broad gates were rerun after the final Rust edits; final documentation is covered by the scoped policy check, `git diff --check`, and the temporary-index patch fingerprint reported in the handoff.

| Gate | Result |
|---|---|
| C# handler parity | 69 passed |
| SQL-focused parity | 87 passed |
| `cargo test --workspace --locked` | 3245 passed, 4 ignored; doc-tests 1 passed |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Pass |
| Six additional feature checks | Pass: all features, no default features, and isolated C#/TypeScript/Rust/XML; default features are covered by workspace test/clippy |
| Fast Windows real-process E2E | Pass: initialize, exactly 15 tools from `tools/list`, parsed `xray_grep` result |
| Scoped product-name policy | Pass for `docs/test-coverage-audit.md` via temporary Git index; repository-wide script still has unrelated pre-existing findings in `docs/di-support.md` |
| `git diff --check` | Pass before final review |

The diagnostic run of the unsplit legacy [e2e-test.ps1](../e2e-test.ps1) is not green on the base commit. It fails before the parallel suite because `T-EXT-CHECK` reads `.Count` from a scalar under StrictMode, `T-RESPECT-GIT-EXCLUDE` and `T-SHUTDOWN` dereference `.Source` from unresolved hard-coded `xray.exe`, and cleanup then reads an uninitialized `$t59proc`. These are E2E infrastructure findings for the separate `test/e2e-refresh` slice, not failures introduced by this test-only diff. Linux E2E was not run because this slice adds no cross-platform harness support.

Final diff SHA-256 and strict review verdict are reported in the delivery handoff after final validation.
