# Changelog

All notable changes to **search-index** are documented here.

Changes are grouped by date and organized into categories: **Features**, **Bug Fixes**, **Performance**, and **Internal**.

---

## 2026-02-28

### Features

- **`includeBody` in `search_callers`** — Each node in the call tree can now include the method's source code inline via `includeBody=true` parameter. Supports `maxBodyLines` (default: 30) and `maxTotalBodyLines` (default: 300) for budget control. Works for both `direction=up` (callers) and `direction=down` (callees). Reuses the existing `inject_body_into_obj()` function from `search_definitions`. When the total body budget is exceeded, remaining nodes get `bodyOmitted` instead of `body`. Eliminates the need for a separate `search_definitions` call to read caller source code after getting the call tree — one call instead of two. 8 new unit tests.

- **Global 64KB response budget for `includeBody=true`** — When any tool (`search_callers` or `search_definitions`) is called with `includeBody=true`, the response byte budget is automatically increased from 16KB to 64KB. This prevents premature truncation of body-rich responses by the progressive truncation mechanism. Tools without `includeBody` continue to use the default 16KB budget. Also retroactively fixes `search_definitions` with `includeBody=true` which could be truncated on large result sets.

- **`search_help` response budget increased to 32KB** — The `search_help` tool response budget was increased from 20KB to 32KB to accommodate the growing parameter examples and tips. Previously, adding new tool parameters could cause best practices tips to be truncated.

### Bug Fixes

- **`search_grep` non-UTF-8 files now return `lineContent`** — Replaced 4 instances of `std::fs::read_to_string()` with `read_file_lossy()` in grep handler. Previously, Windows-1252, Shift-JIS, and UTF-16LE files silently returned no `lineContent` in search results. Now uses the same lossy reading that the watcher and content indexer already use.

- **Reindex via MCP used hardcoded `min_token_len: 2`** — `handle_search_reindex` in `handlers/mod.rs` used a literal `2` instead of `DEFAULT_MIN_TOKEN_LEN`. Fixed to use the constant to prevent index divergence.

- **`eprintln!` diagnostic traces in grep handler** — 11 `eprintln!("[substring-trace] ...")` calls fired on every grep request in production, polluting stderr. Replaced with `tracing::debug!()` so they only appear when `RUST_LOG=debug` is set.

- **Unbounded stdin `read_line` could OOM** — `server.rs` read an entire line into memory before checking size. A malicious/buggy client sending gigabytes without a newline could cause OOM. Fixed with `.take(MAX_REQUEST_SIZE + 1)` to cap reading, plus bounded drain loop to discard remaining bytes.

- **Watcher thread infinite loop on poisoned `RwLock`** — If a panic poisoned the content or definition index RwLock, the watcher thread would loop forever logging errors and discarding all file changes. Now `process_batch()` returns `false` on poisoned lock, causing the watcher thread to exit gracefully. 3 new regression tests.

---

## 2026-02-27

### Bug Fixes

- **`search_grep` substring mode silently returned 0 for terms with punctuation** — When `search_grep` received terms containing non-token characters (punctuation, brackets, etc.) in substring mode (e.g., `#[cfg(test)]`, `<summary>`, `@Attribute`, `System.IO`), it silently returned 0 results. Root cause: the inverted index tokenizer splits on all non-alphanumeric, non-underscore characters, so no indexed token contains `#`, `[`, `(`, `)`, `]`, `.`, `<`, `>`, `@`, etc. The existing auto-switch for space-containing terms (`auto_switch_to_phrase_if_spaces`) only handled spaces. Fix: extended auto-switch to detect ANY non-token character via new `has_non_token_chars()` helper. When detected, automatically routes to phrase mode (which does raw substring matching on file content for punctuation-containing phrases). Renamed `auto_switch_to_phrase_if_spaces` → `auto_switch_to_phrase_if_needed`. Response includes `searchModeNote` explaining the auto-switch. 11 new unit tests (6 for `has_non_token_chars`, 5 for auto-switch scenarios including punctuation, angle brackets, underscore-only no-switch).

### Features

- **SQL stored procedure call graph in `search_callers`** — `search_callers` now supports SQL stored procedure and function call chains via EXEC statements. `direction=up` finds which SPs call a given SP; `direction=down` shows what SPs/functions a given SP calls via EXEC. Tables and views are deliberately excluded from the call graph (data artifacts, not callable code). The `class` parameter maps to SQL schema name (e.g., `class="dbo"`, `class="Sales"`) for disambiguation. Also set `parent` field on SP/SqlFunction definitions to the schema name in the SQL parser, enabling `resolve_call_site` to match EXEC calls across schemas. 8 new unit tests. Cross-language callers (C# → SQL SP via ADO.NET) remain a known limitation.

### Internal
- **Refactored `build_definition_index()` in `src/definitions/mod.rs`** — Decomposed the 388-line monolith (cognitive complexity 102, cyclomatic 56) into 3 focused helper functions: `collect_source_files()` (parallel file walking), `index_file_defs()` (shared index population), `enrich_angular_templates()` (Angular template enrichment). `index_file_defs()` eliminates ~50 lines of duplicated code between `build_definition_index()` and `update_file_definitions()` in `incremental.rs` — both now call the same shared function. Also added `ChunkResult` type alias for readability and removed dead `file_count` AtomicUsize. 14 new unit tests for extracted functions. No behavioral changes — all 1085 unit tests + 62 E2E tests pass.

- **Refactored `handle_search_grep()` / `handle_substring_search()` in `src/mcp/handlers/grep.rs`** — Extracted 4 shared helper functions to eliminate duplicated code across grep/substring/phrase search modes: `passes_file_filters()` (4→1 occurrences), `finalize_grep_results()` (2→1), `build_grep_base_summary()` (8→1 readErrors/lossyUtf8Files/branchWarning blocks), `ensure_trigram_index()`. 19 new unit tests for extracted functions. No behavioral changes — all 1071 unit tests + 62 E2E tests pass.

- **Refactored `build_caller_tree()` / `build_callee_tree()` in `src/mcp/handlers/callers.rs`** — Introduced `CallerTreeContext` struct reducing parameter count from 13→6 (`build_caller_tree`) and 11→6 (`build_callee_tree`). Extracted `resolve_parent_file_ids()` (parent class file pre-filtering) and `expand_interface_callers()` (90-line deeply-nested interface resolution block → 4-line call). No behavioral changes — all 1071 unit tests + 62 E2E tests pass.

- **Refactored `IndexMeta` — typed `IndexDetails` enum** — Replaced flat `IndexMeta` struct (15 fields, 10 `Option<T>`) with typed `IndexDetails` enum discriminated by `#[serde(tag = "type")]`. Four variants: `Content`, `Definition`, `FileList`, `GitHistory` — each carries only its relevant fields, eliminating `None`-padding anti-pattern. Updated 4 constructors, `meta_to_json()`, `cmd_info` display, and all tests. Added `test_meta_serde_roundtrip_all_variants` covering JSON round-trip for all 4 variants. Note: old `.meta` sidecar files are incompatible — they will be auto-recreated on next `search-index info`.

### Performance
- **`Arc<[String]>` for extensions in `build_content_index()`** — Replaced `Vec<String>.clone()` per parallel walker thread with `Arc::clone()` (O(1) vs O(n)). Minimal real-world impact (small vector, few threads), but eliminates an unnecessary allocation anti-pattern.

- **Cognitive complexity reduction for 3 highest-complexity functions** — Reduced cognitive complexity of the 3 remaining functions above the ≤50 threshold via pure extraction refactoring (no behavioral changes):
  - `build_caller_tree()` (callers.rs): **83→45** cognitive complexity. Extracted 4 helpers: `find_target_line()` (overload disambiguation lookup), `collect_definition_locations()` (definition-site exclusion set), `passes_caller_file_filters()` (ext/dir/file filter check), `build_caller_node()` (JSON node construction). Also reused `find_target_line()` and `passes_caller_file_filters()` in `build_callee_tree()`, eliminating duplicated code.
  - `handle_substring_search()` (grep.rs): **80→10** cognitive complexity. Extracted 4 helpers: `auto_switch_to_phrase_if_spaces()` (space detection + phrase delegation), `find_matching_tokens_for_term()` (trigram intersection + verification), `score_token_postings()` (TF-IDF scoring per token), `build_substring_response()` (JSON building with warnings/matchedTokens).
  - `handle_search_grep()` (grep.rs): **53→18** cognitive complexity. Extracted 4 helpers: `parse_grep_args()` → `ParsedGrepArgs` struct (parameter parsing + validation), `expand_regex_terms()` (regex pattern expansion), `score_normal_token_search()` (TF-IDF scoring for exact tokens), `build_grep_response()` (JSON building with count_only support).
  - 54 new unit tests for all 12 extracted functions. All 1146 unit tests + 62 E2E tests pass.


- **Refactored `cmd_grep()` in `src/cli/mod.rs`** — Decomposed from 320-line monolith (cognitive complexity 294, cyclomatic 100) into thin orchestrator (~45 lines) calling 10 focused sub-functions: `parse_grep_args()`, `load_grep_index()`, `resolve_grep_dir()`, `dispatch_grep_search()`, `run_exact_token_search()`, `run_substring_search()`, `run_phrase_search()`, `run_regex_search()`, `format_grep_results()`, `print_grep_summary()`. Added 38 new unit tests for extracted functions. No behavioral changes — all 1052 unit tests + 62 E2E tests pass.

- **Test file split: handlers_tests.rs (3,364 lines → 6 files) and handlers_tests_csharp.rs (2,977 lines → 2 files)** — Split two oversized test files into focused modules to improve LLM context efficiency, incremental compilation, and merge conflict risk. Zero behavior change — all 1012 tests pass identically. Bytewise verification confirmed every test line matches the original. Test function name diff against git HEAD confirmed perfect match (94/94 + 51/51).
  - `handlers_tests.rs` (core): 29 tests — tool definitions, dispatch, context, readiness gates
  - `handlers_tests_grep.rs` (NEW): 63 tests — grep, substring, phrase, truncation, unicode
  - `handlers_tests_fast.rs` (NEW): 14 tests — search_fast
  - `handlers_tests_find.rs` (NEW): 2 tests — search_find
  - `handlers_tests_git.rs` (NEW): 10 tests — git cache, noCache
  - `handlers_tests_misc.rs` (NEW): 24 tests — metrics, security, ranking, validation
  - `handlers_tests_csharp.rs` (definitions): 36 tests — definitions, includeBody, containsLine, audit, reindex
  - `handlers_tests_csharp_callers.rs` (NEW): 23 tests — callers up/down, DI, cycles, overloads
  - `handlers_test_utils.rs` extended with shared `make_empty_ctx()` helper

### Bug Fixes

- **TypeScript `enumMember` extraction broken for enums with explicit values** — Enums with string or numeric values (e.g., `enum Status { Active = "active", Inactive = 0 }`) produced 0 `enumMember` definitions. Root cause: tree-sitter-typescript emits `enum_assignment` nodes for valued members, but the parser only matched `enum_member` and `property_identifier` patterns. Fix: added `"enum_assignment"` to the match arm in `walk_typescript_node_collecting()`. Simple enums without values (`enum Foo { A, B, C }`) were not affected (they use `property_identifier` nodes). 3 new regression tests (string values, numeric values, mixed members). Discovered via large-scale TypeScript E2E testing (449K definitions, 0 `enumMember` results for `parent:"FilteringState"`).

### Features

- **Hint when `kind:"property"` returns 0 results for TypeScript** — When `search_definitions` with `kind:"property"` returns 0 results and the index contains `field` definitions, the response now includes a `hint` in the summary: "In TypeScript, class members are indexed as kind='field', while only interface property signatures use kind='property'. Try kind='field' instead." Also updated the `kind` parameter documentation in `search_help` to clarify the property vs field distinction. 2 new unit tests.

- **Rust parser included in default build** — `lang-rust` feature is now part of the default feature set (`lang-csharp`, `lang-typescript`, `lang-sql`, `lang-rust`). All 4 language parsers are compiled and available out of the box. No more `--features lang-rust` needed for Rust support.

### Performance

- **Lazy-compiled regex in SQL parser** — Converted 19 `Regex::new()` calls in `parser_sql.rs` from per-call compilation to `std::sync::LazyLock` module-level statics, compiled once on first use. Eliminates ~9,000 redundant regex compilations when indexing 500 SQL files with ~3 stored procedures each. 1 dynamic regex (using `format!` for `PROC|FUNCTION` keyword) kept as-is. Estimated 5–15% speedup for SQL-heavy codebases.

- **Pre-lowercased exclude lists in `search_callers`** — `excludeDir` and `excludeFile` parameters are now lowercased once at parse time instead of re-lowercasing on every file comparison inside the recursive `build_caller_tree`/`build_callee_tree` functions. In a depth-3 tree with 10 callers per level × 5 exclude entries, this eliminates ~150 unnecessary `to_lowercase()` string allocations per query.

- **`handle_search_definitions` refactored into composable functions** — Split the 634-line monolithic `handle_search_definitions` function (cognitive complexity 269, cyclomatic 136) into 10 focused functions: `parse_definition_args()` → `DefinitionSearchArgs` struct, `handle_audit_mode()`, `handle_contains_line_mode()`, `collect_candidates()`, `apply_entry_filters()`, `apply_stats_filters()`, `compute_term_breakdown()`, `sort_results()`, `format_definition_entry()`, `build_search_summary()`. The orchestrator is now ~60 lines. Each function is independently testable. 36 new unit tests covering: argument parsing (15 tests), candidate collection (9 tests), entry filtering (8 tests), stats filtering (7 tests), term breakdown (5 tests), sort logic (6 tests), format/summary (13 tests), get_sort_value (4 tests). Total tests: 952 → 988.

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

- **`search_grep` multi-phrase OR search — comma-separated phrases silently returned 0 results** — When `search_grep` received comma-separated phrases with spaces (e.g., `terms="fn handle_foo,fn build_bar"` or `terms="class UserService,class OrderService"`), the auto-switch to phrase mode passed the ENTIRE comma-separated string as a single phrase to `handle_phrase_search()`, which then searched for the literal string `"fn handle_foo,fn build_bar"` — a string that doesn't exist in any file → 0 results. The same bug affected the explicit `phrase: true` path. Fix: both paths now split by commas and search each phrase independently via new `handle_multi_phrase_search()` function, merging results with OR or AND semantics. Single phrases retain existing behavior. `searchMode` reports `"phrase-or"` or `"phrase-and"` for multi-phrase queries. 8 new unit tests + 1 E2E test.

- **`search_callers` DI resolution for nested class `Owner.m_field` pattern** — When a nested (inner) class accessed DI-injected fields of its outer (parent) class via `Owner.m_field` (ControllerBlock pattern), the receiver type was resolved to the outer class name (e.g., `"OrderControllerBlock"`) instead of the field's interface type (e.g., `"IQueryManager"`). Root cause: the `field_types` map passed to call site extraction only contained fields from the inner class — outer class fields were invisible. Fix: when building `field_types` for methods in a nested class, the parser now merges the outer class's field types into the map (inner class fields take precedence via `or_insert`). This enables `resolve_receiver_type` to find `m_field` in the merged map and resolve it to the correct DI interface type. 2 new unit tests (basic resolution + inner-class-takes-precedence edge case).

- **`search_callers` DI resolution gap for constructor field assignments** — When a class used DI fields with non-standard naming conventions (e.g., `m_field`, `fld_field`, `this.field`) WITHOUT explicit field declarations, the C# parser couldn't resolve the receiver type. Root cause: the parser only generated `_paramName` and bare `paramName` mappings from constructor parameters, missing any other naming convention. Fix: the parser now parses the constructor body AST for `field = param` assignments (e.g., `m_orderService = orderService`, `this.myRepo = repository`), mapping the assigned field to the parameter's type. This handles ANY naming convention automatically without hardcoding prefixes. Also fixed a secondary bug in `extract_constructor_param_types` where constructor initializers (`: base(logger)`, `: this(x)`) caused incorrect parameter extraction because `rfind(')')` matched the initializer's closing paren instead of the constructor's. Replaced with depth-tracking paren matching. 4 new unit tests.

- **`baseTypeTransitive` BFS cascade bug** — `collect_transitive_base_type_indices()` used substring matching (`key.contains(&current_type)`) at ALL BFS levels, causing a cascade when a descendant class had a short/common name (e.g., `"Service"` matched `"iservice"`, `"webservice"`, `"serviceprovider"`, etc.). This produced ~42,508 results instead of ~828 and took ~29 seconds. Fix: substring matching is now used only at level 0 (seed) for generic type support (`IAccessTable` → `iaccesstable<model>`); levels 1+ use exact HashMap lookup (O(1)). 3 new unit tests.

### Features

- **`termBreakdown` in `search_definitions` summary for multi-term name queries** — When `name` contains comma-separated terms (e.g., `name="AccessSource,AccessContracts,IAccessTable"`), the summary now includes a `termBreakdown` object showing how many results each term contributed (computed from the full result set before `maxResults` truncation). This helps LLM agents understand result distribution and decide whether to refine their query with `kind` filters or split into separate queries. Only appears for 2+ terms in non-regex mode. 6 new unit tests.

- **Hint when `search_callers` returns 0 results with class filter** — When `search_callers` returns an empty call tree and `class` parameter is set, the response now includes a `hint` field suggesting: try without `class` parameter, or use the interface name. Helps LLM agents diagnose why no callers were found. 3 new unit tests.

- **Hint for large `baseTypeTransitive` hierarchies** — When `baseTypeTransitive=true` and `totalResults > 5000`, the `search_definitions` response includes a `hint` suggesting `kind` or `file` filters to narrow results. 1 new unit test.

### Internal

- **Complete `..Default::default()` boilerplate cleanup** — Replaced ~60 explicit field enumerations in test code with `..Default::default()` for both `HandlerContext` (33 sites) and `DefinitionIndex` (27 sites) across 8 test files. Also ran `cargo fix` to remove 12 unused imports from 8 files. Final pass: replaced 6 remaining cosmetic sites (4 `ContentIndex` + 2 `DefinitionIndex`) in `search_benchmarks.rs`, `handlers_tests_find.rs`, `handlers_tests_misc.rs`, `handlers_tests_grep.rs`, `handlers_tests_typescript.rs`, and `callers_tests.rs`. Removed 2 unused `TrigramIndex` imports. No behavioral changes — purely mechanical cleanup. All 1214 unit tests + 62 E2E tests pass.

---

## 2026-02-26

### Internal

- **`impl Default for ContentIndex` + test boilerplate reduction** — Added `impl Default for ContentIndex` in `src/lib.rs` with compile-time guard test (`test_content_index_field_count_guard`) and default values test (`test_content_index_default_values`). Replaced ~88 test-only `ContentIndex` struct constructions across 13 files with `..Default::default()`, keeping only test-relevant fields explicit. Also replaced ~30 `DefinitionIndex` test constructions (Default already existed). Production code (`build_content_index()`, `serve.rs` empty index) retains explicit fields — compiler enforces conscious field assignment. Adding a new field now requires 3 changes (Default + guard test + production) instead of ~88. 2 new tests, 852 total pass.

### Features

- **`baseTypeTransitive` parameter for `search_definitions`** — New boolean parameter enables BFS traversal of the inheritance hierarchy. `baseType="BaseService" baseTypeTransitive=true` finds not just direct inheritors (MiddleService) but also grandchildren (ConcreteService) and deeper descendants, up to depth 10. Uses runtime BFS with visited set for cycle safety. Known limitation: name-only matching (no namespace resolution). 4 new unit tests.

- **`baseType` filter now uses substring matching** — `baseType="IAccessTable"` now matches `IAccessTable<Model>`, `IAccessTable<Report>`, `IAccessTable<Dashboard>`, etc. Previously, exact match required the full generic type name including type parameters. This makes generic interface discovery practical — a single query finds all 15+ implementations instead of requiring per-type-parameter queries. Both direct and transitive modes use substring matching.

- **Cross-index diagnostics (`crossValidate` parameter)** — New `crossValidate=true` parameter in `search_definitions audit=true` mode. Loads the file-list index from disk and compares with the definition index to identify coverage gaps: files in the file-list but missing from definitions (filtered by definition extensions), and files in definitions but missing from the file-list. Graceful handling when file-list index not found on disk. Results capped at 50 samples. 3 new unit tests.

- **Dynamic MCP instructions adapt to server `--ext` configuration** — The `initialize` → `instructions` field now dynamically generates the "NEVER READ" rule based on the intersection of `DEFINITION_EXTENSIONS` (extensions with parser support: cs, ts, tsx, sql) and the server's `--ext` CLI argument. Previously, the instruction was hardcoded to `.cs/.ts/.tsx`, which was misleading when the server was started with `--ext sql` (SQL was missing from the rule) or `--ext xml` (no parser-supported extensions, but the rule was still present). New behavior: `--ext cs,sql` → `"NEVER READ .cs/.sql FILES DIRECTLY"`, `--ext xml` → fallback note that `search_definitions` is unavailable. Added `DEFINITION_EXTENSIONS` constant in `definitions/mod.rs` as single source of truth. Empty extensions guard prevents malformed instructions. 3 new unit tests.

### Bug Fixes

- **Content index silently swallowed `read_file_lossy` errors** — `build_content_index()` had `Err(_) => {}` in the file walk loop, silently discarding IO errors with no logging, no counters, and no way for users to know files were skipped. Fix: added `read_errors: usize` and `lossy_file_count: usize` fields to `ContentIndex` (with `#[serde(default)]` for backward-compatible deserialization). During build, `AtomicUsize` counters track errors and lossy conversions, with `eprintln!` warnings per file. The `search_grep` summary now reports `readErrors` and `lossyUtf8Files` (matching the pattern already used by `search_definitions`). The `.meta` sidecar file also captures these counters. 5 new unit tests.

### Bug Fixes

- **Watcher bulk reindex after `git pull` — minutes delay reduced to seconds** — Two bugs fixed:
  - **Bug 1: `bulk_threshold=100` triggered full reindex on `git pull`** — When `git pull` changed 100+ files, the watcher's bulk threshold (`--bulk-threshold`, default 100) triggered a full `build_content_index()` that re-scanned all ~48K files, taking **minutes**. The bulk path has been **removed entirely** — the watcher now always uses incremental updates regardless of batch size. Additionally, a new `batch_purge_files()` function purges all stale postings in **O(total_postings)** (single pass) instead of **O(N × total_postings)** (N sequential scans), making even 10K-file `git checkout` operations fast (~10s instead of minutes).
  - **Bug 2: Definition index silently skipped in bulk path** — When the bulk threshold was exceeded, the `continue` statement at the end of the bulk path skipped the definition index update (lines 189-203), leaving `search_definitions` and `search_callers` with stale data after `git pull`. New methods were invisible until the next small file change triggered incremental update. Fixed by removing the bulk path — the incremental path always updates both content and definition indexes.
  - **Breaking change**: `--bulk-threshold` CLI parameter removed. If present in configuration, the server will fail to start with "unexpected argument" error.
  - Performance: `git pull` (300 files) ~4s, `git checkout` (10K files) ~25s (previously: minutes for both)
  - 3 new unit tests for `batch_purge_files` (multi-file, empty set, equivalence with single purge)


- **Tombstone growth in definition index during `--watch` mode** — When the file watcher incrementally updated definitions, old `DefinitionEntry` objects were never removed from the `definitions` Vec — they became tombstones occupying memory and inflating `totalDefinitions` count. After 100 file updates, the Vec could grow to 3× its active size. Four fixes applied:
  - **Fix 1 (UX):** `totalDefinitions` in `search_definitions`, `search_info`, `search_reindex_definitions`, CLI `info`, and memory estimates now shows active definition count (from `file_index`) instead of `definitions.len()` which included tombstones
  - **Fix 2 (Correctness):** When no `name`/`kind`/`attribute`/`baseType` filter is set, candidate generation now uses `file_index` (active definitions only) instead of `0..definitions.len()` which included tombstones and could produce duplicate results
  - **Fix 3 (Angular):** `remove_file_definitions()` now cleans `selector_index` and `template_children` (previously stale entries accumulated after incremental updates)
  - **Fix 4 (Memory):** Auto-compaction triggers when tombstone ratio exceeds 3× (67% waste). In-place `compact_definitions()` rebuilds the Vec with only active entries and remaps all 9 secondary indexes. ~100ms for 846K definitions, <1MB additional memory
  - 5 new unit tests for compaction (basic, no-op, method_calls/code_stats remapping, auto-trigger, selector/template cleanup)

- **Watcher startup gap: stale cache files invisible to `--watch` mode** — When the MCP server started with `--watch` and loaded a stale index from disk, files added/modified/deleted while the server was offline were permanently invisible. The `notify` file watcher only fires events for changes AFTER it starts — pre-existing files produce no events. Fix: added **reconciliation scan** at watcher startup that walks the filesystem and compares with the cached index using path diff (added/deleted) + mtime comparison (modified files). Runs once before the event loop, inside the watcher thread — filesystem events during reconciliation are buffered in the mpsc channel. Performance: ~15ms for 130 files, ~3.5s for 30K files (one-time startup cost, zero runtime impact). Also added cache age logging at startup. 4 new unit tests.

- **`search_definitions` `file` and `parent` parameters now support comma-separated OR** — Previously, `file` and `parent` parameters only accepted a single value (substring match), while `name` supported comma-separated OR. This inconsistency caused LLMs to waste queries when trying to search across multiple files or classes at once (e.g., `file: "UserService.cs,OrderService.cs"` returned 0 results). Both parameters now split on commas and match ANY term (OR logic), consistent with `name`. The relevance ranking for `parent` also supports comma-separated terms. 7 new unit tests.

---

## 2026-02-25

### Bug Fixes

- **Stale index cache loaded when extensions change** — When the MCP server was restarted with different `--ext` parameters (e.g., adding `sql` to a previously `cs`-only setup), the fallback index loader (`find_definition_index_for_dir` / `find_content_index_for_dir`) would find and return an old cached index file that was built with the previous extensions. This caused `search_definitions` to return 0 results for SQL stored procedures even though `sql` was in `--ext` and the SQL parser was fully functional. Root cause: the fallback functions only checked the root directory match but did NOT validate that the cached index's extensions were a superset of the requested extensions. Fix: both functions now accept `expected_exts` parameter and validate that the cached index contains ALL expected extensions (superset check). Stale caches are skipped with a log message, triggering a full rebuild. Same fix applied to content index fallback. 8 new unit tests.

---

## 2026-02-24

### Bug Fixes

- **MCP Server**: Replace all `unwrap()` calls on response serialization with proper error handling — server now returns JSON-RPC `-32603` internal error instead of panicking (audit finding F-07)
- **Call Tree Builder**: Preserve class filter during recursive caller tree building — prevents false positives for common method names like `Process`, `Execute`, `Handle` at recursion depth > 0 (audit finding F-10)

- **SQL parser: files with comment headers before CREATE not parsed** — SQL files starting with comment banners (dashes, copyright notices) before the `CREATE PROCEDURE/TABLE/VIEW/...` statement produced 0 definitions. Root cause: the `CREATE` regex used `^\s*CREATE` with `^` anchored to the start of the batch text (single-line mode). On files without `GO` delimiters, the entire file is one batch, and `^` only matched the first character. Fix: added `(?m)` multiline flag so `^` matches at the start of every line. 1 new unit test.

- **SQL tool description incorrectly stated "SQL parser retained but disabled"** — The `search_definitions` MCP tool description and `def-index --help` text both incorrectly claimed SQL parsing was disabled. The regex-based SQL parser has been fully active since 2026-02-23. Fixed both descriptions. The LLM was reading these descriptions and incorrectly telling users SQL wasn't supported.

### Features

- **`--debug-log` now logs full response body** — The debug log (`--debug-log` flag) now writes the complete MCP tool response JSON after each `RESP` line, enabling diagnosis of what the server actually returned. Previously only logged the response size (e.g., `0.3KB`).

- **`--debug-log` replaces `--memory-log` (US-17)** — Renamed `--memory-log` CLI flag to `--debug-log` and extended it into a full debug log for the MCP server. The debug log now captures MCP request/response traces (tool name, arguments, elapsed time, response size, Working Set) in addition to the existing memory diagnostics. Log format: ISO 8601 timestamp + type tag (REQ/RESP/MEM) per line. File extension changed from `.memory.log` to `.debug.log`. Breaking change: `--memory-log` flag removed (use `--debug-log` instead). New helper functions: `log_request()`, `log_response()`, `format_utc_timestamp()`. 4 new tests, 4 renamed tests.

### Documentation

- **Per-server memory.log with semantic prefix** — `--memory-log` now writes per-server log files using the same naming convention as index files (e.g., `repos_shared_00343f32.memory.log` instead of `memory.log`). Multiple MCP servers running simultaneously no longer overwrite each other's memory logs. 3 new tests.

### Documentation

- **3 tips.rs improvements based on LLM agent UX session analysis** — Added new tip "search_callers 0 results? Try the interface name" warning that DI calls use interface types (IUserService), not concrete classes (UserService), and suggesting `resolveInterfaces=true`. Added NOTE to `search_definitions` parameter examples clarifying that `name` searches AST definition names only — NOT string literals or values (use `search_grep` for string content). Enhanced substring search tip with guidance on short-token noise: `exclude=['pattern']` for filtering, with `dsp_` + ODSP example.

### Bug Fixes

- **search_grep substring auto-switches to phrase for spaced terms (US-16)** — When `search_grep` receives terms containing spaces in substring mode (e.g., `"CREATE PROCEDURE"`, `"public class"`), it now auto-switches to phrase search instead of silently returning 0 results. The response includes `searchModeNote` explaining the switch. Previously, spaced terms always returned 0 because the tokenizer splits on spaces and no individual token contains spaces. 4 new tests.

### Documentation

- **Bug report: search_grep substring mode silently returns 0 for terms with spaces** — Documented P1 UX trap where `terms: "CREATE PROCEDURE"` returns 0 results because the tokenizer splits on spaces, so no individual token contains `"create procedure"`. Fixed via Option A (auto-switch to phrase mode). See `docs/bug-reports/substring-space-in-terms-silent-failure.md`.

---

## 2026-02-23

### Bug Fixes

- **SQL excluded from definition index in MCP server** — The `supported_def_langs` array in `serve.rs` was hardcoded to `["cs", "ts", "tsx"]`, blocking SQL from the definition index even though the regex parser was fully implemented. When starting the server with `--ext cs,sql`, SQL files were silently filtered out and only C# definitions were built. Fixed by adding `"sql"` to the supported languages array. No new tests needed — existing SQL parser tests and E2E tests cover the functionality.

### Features

- **SQL parser** — `search_definitions` now parses `.sql` files, extracting stored procedures, tables, views, functions, user-defined types, indexes, columns, FK constraints, and call sites (EXEC/FROM/JOIN/INSERT/UPDATE/DELETE from stored procedure bodies). Uses a regex-based parser (no tree-sitter dependency for SQL). Definition kinds: `storedProcedure`, `sqlFunction`, `table`, `view`, `userDefinedType`, `sqlIndex`, `column`. GO-separated batches produce correct per-object line ranges. 29 unit tests.

- **Parent relevance ranking in `search_definitions`** — When `parent` filter is set, results are now sorted by parent match quality: exact parent match (tier 0) ranks before prefix match (tier 1), which ranks before substring/contains match (tier 2). Previously, all parent substring matches were treated equally, so searching with `parent=UserService` could return members of `UserServiceMock` before members of `UserService` itself. The fix activates relevance sorting when `parent_filter` is set (in addition to the existing `name_filter` path), using `best_match_tier()` as the primary sort key for parent, with name match tier as secondary. 5 new unit tests.

### Documentation

- **Method group/delegate limitation documented in `search_callers`** — Added a new tip and parameter example documenting that `search_callers` only detects direct method invocations (`obj.Method(args)`), NOT method group references or delegate passes (e.g., `list.Where(IsValid)`, `Func<bool> f = svc.Check`). Workaround: use `search_grep` to find all textual references. This is a known parser-level limitation requiring AST changes to fix.

---

## 2026-02-22

### Documentation

- **LLM instructions: "NEVER READ .cs/.ts/.tsx FILES DIRECTLY" rule** — Substantially strengthened the MCP `instructions` field to prevent LLMs from defaulting to `read_file` for C#/TS files when `search_definitions includeBody=true` is faster. The previous "BEFORE reading, try X first" wording was too soft and lost to Roo's built-in "read all related files together" guidance. New approach uses 4 reinforcement mechanisms: (1) **Absolute prohibition**: `"NEVER READ .cs/.ts/.tsx FILES DIRECTLY"` in ALL CAPS. (2) **Decision trigger**: `"before ANY file read, check each file's extension"` — forces the LLM to evaluate extensions before choosing the tool. (3) **Batch split rule**: `"if you need both .cs and .md files, make TWO calls"` — explicitly resolves the conflict with Roo's "batch all files in one read" advice. (4) **Single exception**: only for editing (need exact line numbers for diffs). Also added anti-pattern in Architecture Exploration strategy and expanded the "Read method source" tip. Root cause: LLMs have a "convenience bias" toward built-in tools (`read_file`) over MCP tools (`search_definitions`), especially when batch reading mixes indexed and non-indexed file types.

### Features

- **Angular Template Metadata** — Enriched Angular `@Component` definitions with template metadata. `search_definitions` now returns `selector` and `templateChildren` for Angular components. `search_callers` supports component tree navigation — `direction='down'` shows child components from HTML templates (recursive), `direction='up'` with a selector finds parent components. Custom elements (tags with hyphens) are extracted from external `.html` templates.

### Breaking Changes

- **Removed `search_git_pickaxe` MCP tool** — The `search_git_pickaxe` tool has been removed. Its use cases (finding when code was introduced) are better served by the `search_grep` → `search_git_blame` workflow, which is 780x faster (~200ms vs 156 seconds) and handles file renames correctly. The only unique pickaxe capability (finding deleted code) was rare and can be done via `git log -S` directly if needed. Tool count: 16 → 15. Also removed: `next_day_public()`, `run_git_public()`, `FIELD_SEP_STR/CHAR`, `RECORD_SEP_STR/CHAR` (all were pickaxe-only). Updated "Code History Investigation" strategy recipe to use grep+blame workflow. Removed 14 pickaxe unit tests + 3 helper tests.

### Bug Fixes

- **Angular template upward recursion stopped at level 1** — `search_callers` with `direction='up'` for Angular selectors only returned direct parent components, ignoring the `depth` parameter. For example, searching up from `operation-button` with `depth=3` found `OperationsBarComponent` (level 1) but NOT its 4 grandparent components (level 2). Root cause: `find_template_parents()` was a flat, non-recursive function — it found direct parents but never recursed to find their parents. Fix: rewrote `find_template_parents()` to accept `max_depth`, `current_depth`, and `visited` set (mirroring the working `build_template_callee_tree()` for downward direction). Grandparents are nested in a `"parents"` field on each parent node. Cycle detection via visited set prevents infinite loops. 4 new unit tests (recursive depth, max_depth respect, cyclic components).

- **`totalCommits` in cache showed truncated count instead of actual total** — When `search_git_history` used the in-memory cache with `maxResults` limiting output, `totalCommits` in the response equaled the returned (truncated) count instead of the actual total. For example, a file with 18 commits queried with `maxResults: 2` showed `totalCommits: 2` instead of `totalCommits: 18`. This misled LLMs into thinking the file had fewer commits than it actually did. Root cause: `query_file_history()` returned only `Vec<CommitInfo>` (after truncation), and the handler used `.len()` for the total count. Fix: `query_file_history()` now returns `(Vec<CommitInfo>, usize)` where the usize is the count BEFORE truncation. The `hint` field now correctly shows "More commits available..." when `totalCommits > returned`. CLI fallback path was already correct. 4 new unit tests. 1 new E2E regression test (T-GIT-TOTALCOMMITS).

- **E2E structural bug: 2 tests hidden inside catch block** — `T-OVERLOAD-DEDUP-UP` and `T-SAME-NAME-IFACE` E2E tests were nested inside the `catch` block of `T-FIX3-LAMBDA`, meaning they only executed when `T-FIX3-LAMBDA` threw an exception. Since `T-FIX3-LAMBDA` passes normally, both tests were silently skipped on every run. Fixed by moving them to top-level `try/catch` blocks. Also strengthened `T-SAME-NAME-IFACE` assertion: now explicitly asserts `totalNodes=0` instead of only checking for specific caller names, catching any unexpected caller in the tree.

- **`search_info` memory spike fix — 1.8 GB temporary allocation eliminated** — `search_info` MCP handler was calling `cmd_info_json()` which fully deserialized ALL index files from `%LOCALAPPDATA%/search-index/` (including indexes for repos the server doesn't serve). For a multi-repo setup with multiple indexed directories, this loaded ~1.8 GB into memory temporarily. Since the main thread never exits, mimalloc never decommitted these freed segments back to the OS, causing Working Set to stay at ~4.4 GB instead of ~2.5 GB. **Fix:** Rewrote `handle_search_info()` to read all statistics from already-loaded in-memory structures (`ctx.index`, `ctx.def_index`, `ctx.git_cache`) via read locks — zero additional allocations. Disk file sizes obtained via `fs::metadata()` only. Removed the `cmd_info_json()` call entirely from the MCP path. Also removed the temporary `force_mimalloc_collect()` workaround from `dispatch_tool()` that was added during diagnosis. Memory log (`--memory-log`) confirms: `search_info` Δ WS went from +1,799 MB to ~0 MB.

- **CLI `search-index info` sidecar `.meta` optimization** — CLI `search-index info` previously deserialized entire index files from disk (~1.8 GB for multi-repo setups) just to extract metadata (root, files count, tokens, age). Added sidecar `.meta` JSON files (~200 bytes each) that are written alongside every index file on save. `cmd_info()` and `cmd_info_json()` now read `.meta` files first (instant, zero deserialization), falling back to full deserialization only for old indexes without `.meta`. Affected save functions: `save_content_index()`, `save_index()`, `save_definition_index()`, `GitHistoryCache::save_to_disk()`. Cleanup functions (`cleanup_orphaned_indexes`, `cleanup_indexes_for_dir`) also remove `.meta` sidecars. 4 new unit tests.

### Performance

- **E2E test parallelization (~50% speedup)** — Parallelized 15 independent MCP tests (9 callers + 5 git + 1 help) using PowerShell `Start-Job`. Sequential CLI tests (shared index state) run first, then the parallel batch runs concurrently. Each parallel test uses isolated temp directories or read-only git queries, ensuring no race conditions. Parallel batch completes in ~6s instead of ~52s sequential. Total E2E time reduced from ~2 min to ~1 min. Compatible with PowerShell 5.1+ (uses `Start-Job`, not PS7-only `ForEach-Object -Parallel`).

### Internal

- **Test parallelism race conditions fixed (US-9)** — Migrated 14 unit tests from hardcoded `std::env::temp_dir().join("fixed_name")` to `tempfile::tempdir()` across 4 test files (`definitions_tests.rs`, `definitions_tests_csharp.rs`, `definitions_tests_typescript.rs`, `handlers_tests.rs`). The hardcoded temp directory names caused race conditions when `cargo test` ran tests in parallel (default behavior, 24 threads on this machine), as two tests could simultaneously write/delete the same directory. `tempfile::tempdir()` generates unique OS-guaranteed paths with automatic cleanup on drop. No test logic changed — only the temp directory creation mechanism. All 822 tests pass with 0 failures under full parallelism.

- **6 new E2E tests for previously untested MCP features** — Added `T-SERVE-HELP-TOOLS` (verifies `serve --help` lists key tools), `T-BRANCH-STATUS` (smoke test for `search_branch_status` MCP tool), `T-GIT-FILE-NOT-FOUND` (nonexistent file returns warning, not error), `T-GIT-NOCACHE` (`noCache` parameter returns valid result), `T-GIT-TOTALCOMMITS` (totalCommits > returned regression test for BUG-2 fix). Total E2E tests: 48 → 55.

- **`definition_index_path_for()` made public** — Renamed `def_index_path_for()` → `definition_index_path_for()` and made it `pub` in `src/definitions/storage.rs` for use by `handle_search_info()` disk size lookup.
- **`read_root_from_index_file_pub()` added** — Public wrapper for header-only index file reading in `src/index.rs`, used by `handle_search_info()` to get file-list root directory without full deserialization.

---

## 2026-02-21

### Features

- **Memory diagnostics (`--memory-log`)** — New `--memory-log` CLI flag for `search-index serve` writes Working Set / Peak WS / Commit metrics to `memory.log` in the index directory (`%LOCALAPPDATA%/search-index/`) at every key pipeline stage. Metrics are captured at: server startup, content/definition index build start/finish, drop/reload cycles, trigram builds, git cache init/ready. When disabled (default), `log_memory()` is a single `AtomicBool` check — zero overhead. Windows-only (uses `K32GetProcessMemoryInfo`); no-op on other platforms. 7 new unit tests.

- **Memory estimates in `search_info`** — `search_info` MCP response and CLI `search-index info` now include a `memoryEstimate` section with calculated per-component memory estimates: inverted index, trigram tokens/map, files, definitions, call sites, git cache, and process memory (Working Set / Peak / Commit). Estimates use sampling (first 1000 keys) for efficiency. Available on all platforms; process memory info is Windows-only.

### Performance

- **`mi_collect(true)` fix for cold-start memory spike** — After `drop(build_index)` and before `load_from_disk()`, the server now calls mimalloc's `mi_collect(true)` to force decommit of freed segments from abandoned thread heaps. This prevents the build+drop+reload pattern from inflating Working Set by ~1.5 GB. Applied in 3 locations: content index build thread, definition index build thread, and watcher bulk reindex path.

### Bug Fixes

- **Chained method calls missing from call-site index (C# and TypeScript)** — Inner calls in method chains like `service.SearchAsync<T>(...).ConfigureAwait(false)` and `builder.Where(...).OrderBy(...).ToList()` were not extracted. Only the outermost call (e.g., `ConfigureAwait`, `ToList`) was found; all inner calls were silently dropped. Root cause: `walk_for_invocations()` (C#) and `walk_ts_for_invocations()` (TypeScript) only recursed into `argument_list` children of `invocation_expression`/`call_expression` nodes, skipping the `member_access_expression` child where nested invocations live in the AST. The fix recurses into ALL children, capturing every call in the chain. This affects `search_callers` results for any code using `.ConfigureAwait(false)`, fluent APIs, LINQ chains, or promise chains. 2 new regression tests, 1 existing test strengthened.

- **Generic method call-site indexing in C# parser** — Call sites for generic method invocations like `client.SearchAsync<T>(args)` were stored with `method_name = "SearchAsync<T>"` (including type arguments) instead of `"SearchAsync"`. This caused `verify_call_site_target()` to fail matching when `class` filter was used in `search_callers`, producing 0 callers for any generic method. The fix adds `extract_method_name_from_name_node()` that strips type arguments from `generic_name` AST nodes in both `extract_member_access_call()` and `extract_conditional_access_call()`. Also fixes `direction=down` callee resolution for generic methods. TypeScript parser was NOT affected (different AST structure). 6 new unit tests.

### Internal

- **Independent audit test suite for code stats and call chains** — Added `src/definitions/audit_tests.rs` with 22 golden fixture tests that independently verify the accuracy of tree-sitter-based code complexity metrics and call chain analysis. Each fixture is hand-crafted code where every metric (cyclomatic complexity, cognitive complexity, nesting depth, param count, return count, call count, lambda count) is manually computed line-by-line. The audit covers: C# code stats (7 tests), TypeScript code stats (5 tests), call site accuracy with receiver type verification (2 tests), multi-class call graph completeness (2 tests), edge cases (4 tests), and statistical consistency checks including axiomatic invariants and cross-language parity (3 tests). Documents known tree-sitter grammar differences between C# and TypeScript (else-if handling, try nesting).

### Bug Fixes

- **UTF-16 BOM detection in `read_file_lossy()`** — Files encoded in UTF-16LE or UTF-16BE (with BOM) were previously read as lossy UTF-8, producing garbled content (`��/ / - - - -`). Tree-sitter received garbage instead of valid source code, resulting in 0 definitions for affected files. The fix adds BOM detection to `read_file_lossy()`: UTF-16LE BOM (`FF FE`) → decode as UTF-16LE, UTF-16BE BOM (`FE FF`) → decode as UTF-16BE, UTF-8 BOM (`EF BB BF`) → strip BOM. All three indexes (content, definitions, callers) benefit from this single-function fix. Affects ~44 files previously reported as `lossyUtf8Files` in audit. 15 new unit tests.

### Performance

- **Optimized MCP tool descriptions for LLM token budget** — Shortened parameter descriptions across all 14 MCP tools (~100 parameters total), reducing the system prompt token footprint by ~30% (~2,000 tokens). Concrete examples moved from inline parameter descriptions to a new `parameterExamples` section in `search_help` (on-demand via 1 extra call). Critical usage hints preserved (e.g., `class` in `search_callers`). Tool-level descriptions unchanged. Semantic purpose of each parameter preserved (8-15 words). Added `test_tool_definitions_token_budget` test to prevent description bloat from re-accumulating. Added `test_render_json_has_parameter_examples` test to verify examples are accessible via `search_help`.

### Documentation

- **Fixed inaccurate Copilot MCP claim in docs** — `README.md` and `docs/mcp-guide.md` incorrectly listed "Copilot" as an MCP-compatible client. GitHub Copilot does not read `.vscode/mcp.json`, does not launch local stdio servers, and is not an MCP client. Changed "(VS Code Roo, Copilot, Claude)" → "(Roo Code, Cline, or any MCP-compatible client)" in both files.

- **CLI help, LLM instructions, and documentation updated for new features** — 6 documentation changes across the codebase:
  1. `src/cli/args.rs` — Added 5 missing tools to AVAILABLE TOOLS list (`search_git_blame`, `search_branch_status`, `search_git_pickaxe`, `search_help`, `search_reindex_definitions`), bringing the list from 11 to 16 tools
  2. `src/tips.rs` — Added 3 new tips (branch status check, pickaxe usage, noCache parameter), 1 new "Code History Investigation" strategy recipe, git tools brief mention in `render_instructions()`, and `search_branch_status` in tool priority list
  3. `docs/mcp-guide.md` — Added "File Not Found Warning" section documenting the `warning` field in git tool responses when a file doesn't exist in git
  4. `docs/cli-reference.md` — Added `[GIT]` example output line to `search-index info` section
  5. `README.md` — Added "Branch awareness" feature mention for `branchWarning`
  6. `docs/use-cases.md` — Added "When Was This Error Introduced?" use case showing `search_branch_status` → `search_git_pickaxe` → `search_git_authors` → `search_git_diff` workflow

### Features

- **Type inference improvements for `search_callers` (7 user stories)** — Improved recall for `verify_call_site_target()` by adding 6 new type inference paths for local variables in C#:
  1. **Return type inference (US-1)**: `var stream = GetDataStream()` now resolves to the return type of same-class methods via signature parsing. Uses `parse_return_type_from_signature()` with angle-bracket-aware tokenization for generic types.
  2. **Cast expression (US-2)**: `var reader = (PackageReader)obj` → `reader : PackageReader`
  3. **`as` expression (US-3)**: `var reader = obj as PackageReader` → `reader : PackageReader`
  4. **`await` + Task unwrap (US-5)**: `var stream = await GetStreamAsync()` where return type is `Task<Stream>` → unwraps to `stream : Stream`. Handles `Task<T>` and `ValueTask<T>`.
  5. **Extension method detection (US-6)**: Builds extension method index during definition parsing (static classes with `this` parameter methods). `verify_call_site_target()` accepts extension method calls regardless of receiver type.
  6. **Pattern matching (US-7)**: `if (obj is PackageReader reader)` and `case StreamReader reader:` → extracts type from `declaration_pattern` AST node.

  US-4 (`using var`) was verified to already work — tree-sitter C# parses it as `local_declaration_statement`. 23 new unit tests.

- **`search_git_pickaxe` MCP tool** — New tool that finds commits where specific text was added or removed using git pickaxe (`git log -S`/`-G`). Unlike `search_git_history` which shows all commits for a file, pickaxe finds exactly the commits where a given string or regex first appeared or was deleted. Supports exact text (`-S`) and regex (`-G`) modes, optional file filter, date range filters, and `maxResults` limit. Patch output truncated to 2000 chars per commit. Tool count: 16. 14 new unit tests.

- **`search_branch_status` MCP tool** — New tool that shows the current git branch status before investigating production bugs. Returns: current branch name, whether it's main/master, how far behind/ahead of remote main, uncommitted (dirty) files list, last fetch timestamp with human-readable age, and a warning if the index is built on a non-main branch or is behind remote. Fetch age warnings use thresholds: < 1 hour (none), 1–24 hours (info), 1–7 days (outdated), > 7 days (recommend fetch). Tool count: 15. 14 new unit tests (6 handler tests + 8 helper function tests).

- **`branchWarning` in index-based tool responses** — When the MCP server is started on a branch other than `main` or `master`, all index-based tool responses (`search_grep`, `search_definitions`, `search_callers`, `search_fast`) now include a `branchWarning` field in the `summary` object: `"Index is built on branch '<name>', not on main/master. Results may differ from production."` The branch is detected at server startup via `git rev-parse --abbrev-ref HEAD`. Warning is absent on `main`/`master`, when not in a git repo, or when git is unavailable. Git tools are not affected (they query git directly). 7 new unit tests.

- **Empty results validation in `search_git_history`** — When `search_git_history` returns 0 commits, the tool now checks whether the queried file is tracked by git. If the file doesn't exist in git, the response includes a `"warning"` field: `"File not found in git: <path>. Check the path."`. This helps users distinguish between "no commits in the date range" and "wrong file path". Works in both cache and CLI fallback paths. New `file_exists_in_git()` helper function. 5 new unit tests, 2 new E2E test scenarios (T70, T70b).

- **`noCache` parameter for git tools** — Added `noCache` boolean parameter to `search_git_history`, `search_git_authors`, and `search_git_activity`. When `true`, bypasses the in-memory git history cache and queries git CLI directly. Useful when cache may be stale after recent commits. Default is `false` (use cache when available). 5 new unit tests.

### Performance

- **Trigram pre-warming on server start** — Added `ContentIndex::warm_up()` method that forces all trigram index pages into resident memory after deserialization. Previously, the first 1-2 substring queries took ~3.4 seconds due to OS page faults on freshly deserialized memory. Pre-warming touches all trigram posting lists, token strings, and inverted index HashMap buckets in a background thread at server startup, eliminating the cold-start penalty without delaying server readiness. Runs after both the disk-load fast path and the background-build path. Stderr logging: `[warmup] Starting trigram pre-warm...` / `[warmup] Trigram pre-warm completed in X.Xms (N trigrams, M tokens)`. 4 new unit tests.

### Internal

- **Substring search timing instrumentation** — Added `[substring-trace]` `eprintln!` timing traces to `handle_substring_search()` in `grep.rs` for diagnosing slow cold-start substring queries (~3.4s on first 1-2 queries). Traces cover 8 stages: terms parsing, trigram dirty check + rebuild, trigram intersection (per term), token verification (`.contains()`), main index lookups, file filter checks, response JSON building, and total elapsed time. Always-on via stderr (no feature flag), does not interfere with MCP protocol on stdout. Also instruments the trigram rebuild path in `handle_search_grep()`. E2E test plan updated with T-SUBSTRING-TRACE scenario.

### Features

- **Git history cache in `search-index info` / `search_info`** — The `info` CLI command and MCP `search_info` tool now display `.git-history` cache files alongside existing index types (`.file-list`, `.word-search`, `.code-structure`). CLI output shows `[GIT]` entries with branch, commit count, file count, author count, HEAD hash (first 8 chars), size, and age. MCP JSON output includes `type: "git-history"` entries with full metadata. Previously, `.git-history` cache files existed on disk but were silently skipped by the info command. 4 new unit tests.

### Bug Fixes

- **File-not-found warning in `search_git_authors` and `search_git_activity`** — When these tools return 0 results and a `path`/`file` parameter was provided, they now check whether the path exists in git. If not found, the response includes `"warning": "File not found in git: <path>. Check the path."` — matching the existing behavior of `search_git_history`. Works in both cache and CLI fallback paths. 4 new unit tests.

- **7 bugs found and fixed via code review** — Comprehensive code review of `callers.rs`, `grep.rs`, and `utils.rs` found 7 bugs (2 major, 4 minor, 1 cosmetic). All fixed with tests:
  - **`is_implementation_of` dead code in production (BUG-CR-2, MAJOR)** — `verify_call_site_target()` lowercased both arguments before calling `is_implementation_of()`, which checks for uppercase `'I'` prefix — always returned false. Fuzzy DI matching (e.g., `IDataModelService` → `DataModelWebService`) never worked in the call verification path. Unit tests passed because they called the function with original-case inputs directly. **Fix:** pass original-case values from `verify_call_site_target()`. 2 new regression tests.
  - **`search_grep` ext filter single-string comparison (BUG-CR-1)** — `search_grep` compared the ext filter as a whole string (e.g., `"cs" == "cs,sql"` → false), while `search_callers` correctly split by comma. Extracted shared `matches_ext_filter()` helper. Also fixed misleading doc: schema said "(default: server's --ext)" but actual default was None. 5 new unit tests.
  - **`inject_body_into_obj` uses `read_to_string` (BUG-CR-6)** — Files with non-UTF-8 content (Windows-1252) failed body reads while the definition index was built with `read_file_lossy`. Now uses `read_file_lossy` for consistency. ~44 lossy files no longer show `bodyError`.
  - **Normal grep mode missing empty terms check (BUG-CR-7)** — `terms: ",,,"` silently returned empty results in normal mode but gave an explicit error in substring mode. Added consistent empty terms check.
  - **`maxTotalNodes: 0` returns empty tree (BUG-CR-3)** — `0 >= 0` was always true, causing immediate return. Now treats 0 as unlimited (`usize::MAX`).
  - **`direction` parameter accepts any value as "down" (BUG-CR-4)** — `"UP"`, `"sideways"`, etc. silently ran as "down". Added validation with case-insensitive comparison.
  - **Warnings array shows only first warning (BUG-CR-5, cosmetic)** — Changed from `summary["warning"]` (singular string) to `summary["warnings"]` (array) for future-proofing. **Breaking change** for consumers reading `warning` key.

- **`search_grep` substring `matchedTokens` data leak (BUG-7)** — `matchedTokens` in substring search responses was populated from the global trigram index before applying `dir`/`ext`/`exclude` filters, showing tokens from files outside the requested scope. Now `matchedTokens` only includes tokens that have at least one file passing all filters. Affects `countOnly` and full response modes.

- **Input validation hardening (6 bugs fixed)** — Systematic input validation improvements across MCP tools, found via manual fuzzing:
  - `search_definitions`: `name: ""` now treated as "no filter" instead of returning 0 results (BUG-1)
  - `search_definitions`: `containsLine: -1` now returns error instead of silently returning ALL definitions (BUG-2, most critical)
  - `search_callers`: `depth: 0` now returns error instead of empty tree (BUG-3)
  - `search_git_history`/`search_git_diff`/`search_git_activity`: reversed date range (`from > to`) now returns descriptive error instead of silently returning 0 results (BUG-4)
  - `search_fast`: `pattern: ""` now returns error instead of scanning 97K files for 0 results (BUG-5)
  - `search_grep`: `contextLines > 0` now auto-enables `showLines: true` instead of silently ignoring context (BUG-6)

- **Panic-safety in background threads** — `.write().unwrap()` on `RwLock` in `serve.rs` (4 places) replaced with `.write().unwrap_or_else(|e| e.into_inner())` to handle poisoned locks gracefully (MAJOR-1). `.join().unwrap()` on thread handles in `index.rs` and `definitions/mod.rs` replaced with `unwrap_or_else` + warning log to survive individual worker thread panics during index building (MAJOR-2).

- **Mutex `into_inner().unwrap()` → graceful recovery** — Added `recover_mutex<T>()` helper in `src/index.rs` that handles poisoned mutex with a warning log instead of panicking. Applied to 3 locations: file index build (`src/index.rs`), content index build (`src/index.rs`), and definition index build (`src/definitions/mod.rs`). Consistent with the `.lock().unwrap_or_else(|e| e.into_inner())` pattern already used for mutex lock operations throughout the codebase.

- **`format_blame_date` timezone offset not applied** — `format_blame_date()` in `src/git/mod.rs` now applies the timezone offset string (e.g., `+0300`, `-0500`, `+0545`) to the Unix timestamp before civil date calculation. Previously, the timezone string was displayed but not used in the date math, causing all blame dates to show UTC time regardless of the author's timezone. Added `parse_tz_offset()` helper. 5 new tests for timezone formatting and 9 assertions for offset parsing.

- **`next_day()` broken fallback** — The `next_day()` function in `src/git/mod.rs` previously appended `T23:59:59` to unparseable date strings, producing invalid git date arguments. Now logs a warning and returns the original date string unchanged. This path is unreachable in practice (`validate_date()` is always called first), but the fix prevents silent corruption if the code path is ever reached. 1 new test for malformed date fallback.

---

## 2026-02-20

### Features

- **Git filter by author** — Added `author` parameter to `search_git_history`, `search_git_diff`, and `search_git_activity`. Case-insensitive substring match against author name or email. Works with both cache and CLI fallback paths. Example: `"author": "alice"` returns only commits by Alice.

- **Git filter by commit message** — Added `message` parameter to `search_git_history`, `search_git_diff`, `search_git_activity`, and `search_git_authors`. Case-insensitive substring match against commit subject. Combinable with `author` and date filters. Example: `"message": "fix bug"` returns only commits with "fix bug" in the message.

- **Directory ownership in `search_git_authors`** — `search_git_authors` now accepts a `path` parameter (file or directory path, or omit for entire repo). `file` remains as a backward-compatible alias. Directory paths return aggregated authors across all files under that directory with proper commit deduplication. Omitting `path` entirely returns authors for the entire repository.

- **`search_git_blame` tool** — New MCP tool for line-level attribution via `git blame --porcelain`. Parameters: `repo` (required), `file` (required), `startLine` (optional, 1-based), `endLine` (optional). Returns commit hash (8-char short), author name, email, date (with timezone), and line content for each blamed line. Always uses CLI. Total tool count: 14.

### Internal

- **Git feature unit tests** — Added 30 new unit tests across 4 feature areas: (1) Author/message filtering for `query_file_history`, `query_authors`, `query_activity` — 18 tests covering case-insensitive author/email matching, message substring filter, combined filters, and date+author combinations; (2) Directory ownership — 1 test for whole-repo `query_authors`; (3) Git blame — 5 tests for `blame_lines()` (success, single line, nonexistent file, bad repo, content verification); (4) Blame porcelain parser — 4 tests for `parse_blame_porcelain()` (basic, repeated hash reuse, empty input) and `format_blame_date()`. Also made `parse_blame_porcelain` and `format_blame_date` `pub(crate)` for test access, fixed pre-existing tool count assertion (13→14), and updated all existing test call sites to match new 6-arg `query_file_history`, 5-arg `query_authors`, 5-arg `query_activity`, 7-arg `file_history`, 5-arg `top_authors`, 4-arg `repo_activity` signatures.

- **Git cache test coverage** — Closed 5 test coverage gaps in the git history cache module (`src/git/cache_tests.rs`): (1) integration test for `build()` with a real temp git repo (`#[ignore]`), (2) bad timestamp parsing — verifies commits with non-numeric timestamps are skipped, (3) author pool overflow boundary — verifies error at 65536 unique authors and success at 65535, (4) `cache_path_for()` different directories produce different paths, (5) E2E test in `e2e-test.ps1` for `search_git_history` cache routing. Total: 5 new unit tests + 1 E2E test.

### Bug Fixes

- **Git CLI date filtering timezone fix** — The `add_date_args()` function in `src/git/mod.rs` now appends `T00:00:00Z` to `--after`/`--before` date parameters, forcing UTC interpretation. Previously, bare `YYYY-MM-DD` dates were interpreted in the local timezone by git, causing a ±N hour mismatch with the cache path (which always uses UTC timestamps). This could cause `search_git_history` CLI fallback to miss commits at day boundaries on non-UTC systems. Affects `search_git_history`, `search_git_diff`, `search_git_authors`, and `search_git_activity` CLI paths. 23 new diagnostic unit tests added for date conversion, timestamp formatting, and cache query boundary conditions.

- **Git cache progress logging** — The git cache background thread now emits `[git-cache]` progress messages during startup and build, preventing the appearance of a "stuck" server when building the cache for large repos (3+ minutes). Messages include: initialization, branch detection, disk cache validation, build progress every 10K commits, and completion summary.

- **`search_git_authors` missing `firstChange` on cached path** — The cached code path for `search_git_authors` now correctly returns the `firstChange` timestamp instead of an empty string. Added `first_commit_timestamp` field to `AuthorSummary` in the cache module.

### Features

- **Git history cache background build + disk persistence (PR 2c)** — The git history cache is now built automatically in a background thread on server startup, saved to disk (`.git-history` file, bincode + LZ4 compressed), and loaded from disk on subsequent restarts (~100 ms vs ~59 sec full rebuild). HEAD validation detects stale caches: if HEAD matches → use disk cache; if HEAD changed (fast-forward) → rebuild; if HEAD changed (force push/rebase) → rebuild; if repo re-cloned → rebuild. Commit-graph hint emitted at startup if `.git/objects/info/commit-graph` is missing. Key changes:
  - Background thread in `serve.rs` following existing content/definition index pattern (copy-paste, no refactor)
  - `save_to_disk()` / `load_from_disk()` methods using atomic write (temp file + rename) and shared `save_compressed()`/`load_compressed()`
  - `cache_path_for()` constructs `.git-history` file path matching existing `.word-search`/`.code-structure` naming convention
  - `is_ancestor()` / `object_exists()` helpers for HEAD validation
  - `run_server()` now accepts `git_cache` and `git_cache_ready` Arc handles from `serve.rs`
  - 12 new unit tests for disk persistence, atomic write, corrupt file handling, format version validation

- **Git history cache handler integration (PR 2b)** — Integrated the git history cache into the MCP handler layer with cache-or-fallback routing. When the cache is ready (populated by background thread in PR 2c), `search_git_history`, `search_git_authors`, and `search_git_activity` use sub-millisecond cache lookups instead of 2-6 sec CLI calls. When cache is not ready, handlers transparently fall back to existing CLI code (zero regression). `search_git_diff` always uses CLI (cache has no patch data). Cache responses include `"(from cache)"` hint in the summary field. Key changes:
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
  - `search_git_history` — commit history for a file (hash, date, author, message)
  - `search_git_diff` — commit history with full diff/patch (truncated to ~200 lines per commit)
  - `search_git_authors` — top authors for a file ranked by commit count
  - `search_git_activity` — repo-wide activity (all changed files) for a date range

  All tools support `from`/`to`/`date` filters and `maxResults` (default: 50). Performance: ~2 sec for single file, ~8 sec for full year in a 13K-commit repo. Response truncation via existing `truncate_large_response` mechanism.

- **Code complexity metrics (`includeCodeStats`)** — `search_definitions` now computes and returns 7 code complexity metrics for methods/functions during AST indexing: cyclomatic complexity, cognitive complexity (SonarSource), max nesting depth, parameter count, return/throw count, call count (fan-out), and lambda count. Always computed when `--definitions` is used (~2-5% CPU overhead, ~7 MB RAM). Query with `includeCodeStats=true` to see metrics, or use `sortBy` (e.g., `sortBy='cognitiveComplexity'`) and `min*` filters (e.g., `minComplexity=10`, `minParams=5`) to find complex methods. Supports C# and TypeScript/TSX.

### Internal

- **Lowercase index filenames** — `sanitize_for_filename()` now lowercases all characters, producing consistent lowercase index filenames (e.g., `repos_myproject_a1b2c3d4.word-search` instead of `Repos_MyProject_a1b2c3d4.word-search`). Follows industry best practices (Cargo, npm, Docker all use lowercase). Prevents duplicate index files when the same path is referenced with different casing on case-insensitive filesystems. Old index files with uppercase names will be re-created automatically.

---

## 2026-02-18

### Features

- **Async MCP server startup** — server responds to `initialize` immediately; indexes are built in background threads. Tools that don't need indexes (`search_help`, `search_info`, `search_find`) work instantly. Index-dependent tools return a "building, please retry" message until ready. ([PR #17](https://github.com/pustynsky/search-index/pull/17))

- **Save indexes on graceful shutdown** — when the MCP server receives stdin close (VS Code stop), both content and definition indexes are saved to disk, preserving all incremental watcher updates across restarts. ([PR #18](https://github.com/pustynsky/search-index/pull/18))

- **Phrase search with punctuation** — `search_grep` with `phrase: true` now uses raw substring matching when the phrase contains non-alphanumeric characters (e.g., `</Property>`, `ILogger<string>`), eliminating false positives from tokenization stripping XML/code punctuation. Alphanumeric-only phrases continue to use the existing tokenized regex path. ([PR #19](https://github.com/pustynsky/search-index/pull/19))

- **TypeScript call-site extraction for `search_callers`** — `search_callers` now works for TypeScript/TSX files. Supports method calls (`this.service.getUser()`), constructor calls (`new UserService()`), static calls, `super` calls, optional chaining (`?.`), and DI constructor parameter properties. Direction `"up"` and `"down"` both supported. ([PR #11](https://github.com/pustynsky/search-index/pull/11))

- **TypeScript AST parsing** — added tree-sitter-based TypeScript/TSX definition parsing for `search_definitions`. Extracts classes, interfaces, methods, properties, fields, enums, constructors, functions, type aliases, and variables. ([PR #9](https://github.com/pustynsky/search-index/pull/9))

- **`includeBody` for `search_definitions`** — returns actual source code inline in definition results, eliminating the need for follow-up `read_file` calls. Controlled via `maxBodyLines` and `maxTotalBodyLines` parameters. ([PR #2](https://github.com/pustynsky/search-index/pull/2))

- **Substring search** — `search_grep` now supports substring matching (enabled by default). Search term `"service"` matches tokens like `userservice`, `servicehelper`, etc. Powered by trigram index for fast lookup. ([PR #3](https://github.com/pustynsky/search-index/pull/3))

- **`--metrics` CLI flag** — displays index build metrics (file count, token count, definition count, build time) when building indexes. ([PR #4](https://github.com/pustynsky/search-index/pull/4))

- **Benchmarks** — added `benches/search_benchmarks.rs` with criterion-based benchmarks for index operations. ([PR #5](https://github.com/pustynsky/search-index/pull/5))

- **LZ4 compression for index files** — all index files (`.idx`, `.cidx`, `.didx`) are now LZ4-compressed on disk, reducing total size by ~42% (566 MB → 327 MB). Backward compatible: legacy uncompressed files are auto-detected on load. ([PR #15](https://github.com/pustynsky/search-index/pull/15))

- **`search_callers` caps** — added `maxCallersPerLevel` and `maxTotalNodes` parameters to prevent output explosion for heavily-used methods. ([PR #12](https://github.com/pustynsky/search-index/pull/12))

### Bug Fixes

- **Substring AND-mode false positives** — fixed a bug where AND-mode search (`mode: "and"`) returned false positives when a single search term matched multiple tokens via the trigram index. Now tracks distinct matched term indices per file. ([PR #16](https://github.com/pustynsky/search-index/pull/16))

- **Lossy UTF-8 file reading** — files with non-UTF8 bytes (e.g., Windows-1252 `0x92` smart quotes) were silently skipped during indexing. Now uses `String::from_utf8_lossy()` with a warning log, preserving all valid content. ([PR #13](https://github.com/pustynsky/search-index/pull/13))

- **Modifier bug** — fixed definition parsing issue with C# access modifiers. ([PR #6](https://github.com/pustynsky/search-index/pull/6))

- **Code review fixes** — bounds checking, security validation for path traversal, stable hash for index file paths, underflow protection with `saturating_sub`, and monitoring improvements. ([PR #8](https://github.com/pustynsky/search-index/pull/8))

- **Version desync** — MCP protocol version now derives from `Cargo.toml` via `env!("CARGO_PKG_VERSION")` instead of a hardcoded string. ([PR #16](https://github.com/pustynsky/search-index/pull/16))

### Performance

- **Memory optimization** — eliminated forward index (~1.5 GB savings in steady-state) and added drop+reload pattern after build (~1.5 GB savings during build). Steady-state memory: ~3.7 GB → ~2.1 GB. ([PR #20](https://github.com/pustynsky/search-index/pull/20))

- **Lazy parsers + parallel tokenization** — TypeScript grammars loaded lazily (only when `.ts`/`.tsx` files are encountered); content tokenization parallelized across threads. Index build time: ~150s → ~42s (3.6× faster). ([PR #14](https://github.com/pustynsky/search-index/pull/14))

- **Eliminated ~100 MB allocation** — `reindex_definitions` response was serializing the entire index just to get its byte size. Replaced with `bincode::serialized_size()`. ([PR #16](https://github.com/pustynsky/search-index/pull/16))

### Internal

- **Module decomposition** — extracted `cli/`, `mcp/handlers/`, and other modules from monolithic `main.rs`. ([PR #7](https://github.com/pustynsky/search-index/pull/7))

- **Refactor: type safety and error handling** — introduced `SearchError` enum, eliminated duplicate type definitions, extracted `index.rs` and `error.rs` modules, fixed `total_tokens` drift in incremental updates, reduced binary size from 20.4 MB to 9.8 MB by removing incompatible SQL grammar, added 11 unit tests. ([PR #1](https://github.com/pustynsky/search-index/pull/1))

- **Tips updated** — updated MCP server system prompt instructions (`src/tips.rs`). ([PR #10](https://github.com/pustynsky/search-index/pull/10))

- **Documentation fixes** — various doc corrections and updates. ([PR #21](https://github.com/pustynsky/search-index/pull/21))

- **Git history cache documentation and cleanup (PR 2d)** — Updated all documentation (README, architecture, MCP guide, storage model, E2E test plan, changelog) to reflect the git history cache feature. Added git cache to architecture overview table, module structure, and storage format descriptions. Verified no TODO/FIXME comments in cache module. No Rust code changes.

---

## Summary

| Metric                  | Value                       |
| ----------------------- | --------------------------- |
| Total PRs               | 28                          |
| Features                | 20                          |
| Bug Fixes               | 10                          |
| Performance             | 3                           |
| Internal                | 5                           |
| Unit tests (latest)     | 903 (with lang-rust)        |
| E2E tests (latest)      | 59                          |
| Binary size reduction   | 20.4 MB → 9.8 MB (−52%)     |
| Index size reduction    | 566 MB → 327 MB (−42%, LZ4) |
| Memory reduction        | 3.7 GB → 2.1 GB (−43%)      |
| Build speed improvement | 150s → 42s (3.6×)           |
