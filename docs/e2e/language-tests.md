# Language-Specific Tests

Tests for SQL, TypeScript/TSX, Angular, and Rust parser-specific behavior.

---

## SQL Support

### T-SQL-01: `def-index` — Build SQL definition index

**Expected:**

- Definitions include: `storedProcedure`, `table`, `view`, `sqlFunction`, `userDefinedType`, `sqlIndex`, `column`
- Regex-based SQL parsing

---

### T-SQL-02: `xray_definitions` finds SQL stored procedures

**Expected:**

- `kind: "storedProcedure"` results with `name`, `file`, `lines`, `signature`

---

### T-SQL-03: `xray_definitions` finds SQL tables

**Expected:**

- `kind: "table"` results

---

### T-SQL-04: `def-index` — SQL file with GO-separated objects

**Expected:**

- 3+ definitions extracted (table, procedure, view)
- Correct non-overlapping line ranges

---

### T-SQL-05: `xray_callers` direction=up finds callers of SQL stored procedure

**Expected:**

- Call sites matched via EXEC statements in SP bodies
- `class` = SQL schema name (e.g., `"dbo"`)

---

### T-SQL-05b: `xray_callers` direction=down shows SP EXEC dependencies

**Expected:**

- `callTree` includes EXEC-called stored procedures (SP→SP)
- Does NOT include tables (tables are data, not code)
- SQL functions included

---

### T-SQL-06: `def-index` — Mixed C#/TypeScript/SQL definition index

**Expected:**

- C# (tree-sitter), TypeScript (tree-sitter), and SQL (regex) definitions all in same `.code-structure` index

---

## TypeScript Support

### T44: `def-index` — Build TypeScript definition index

**Expected:**

- Definitions include: `function`, `class`, `interface`, `enum`, `typeAlias`, `variable`
- Tree-sitter TypeScript parsing

---

### T45: `def-index` — Build TypeScript + TSX definition index

**Expected:**

- Both `.ts` and `.tsx` files parsed
- TSX uses TSX grammar

---

### T46: `xray_definitions` finds TypeScript functions

**Expected:**

- `kind: "function"` with `name`, `file`, `lines`, `signature`

---

### T47: `xray_definitions` finds TypeScript class by name

---

### T48: `xray_definitions` finds decorated TypeScript classes

**Expected:**

- Decorator names stored as attributes (lowercased, without `@` prefix)

---

### T49: `def-index` — Mixed C# + TypeScript definition index

**Expected:**

- Both C# and TypeScript definitions coexist in the same index

---

### T50: `serve` — Incremental TypeScript definition update via watcher

**Note:** Manual test requiring a running server.

---

### T50b: `serve` — Incremental content index update without forward index

**Validates:** Brute-force inverted index purge (memory optimization ~1.5 GB savings).

**Note:** Manual test. Also covered by unit tests: `test_purge_file_from_inverted_index_*`, `test_update_existing_file_without_forward_index`.

---

### T51: `serve` — TypeScript-specific definition kinds (typeAlias, variable)

**Expected:**

- `kind: "typeAlias"` returns type declarations
- `kind: "variable"` returns exported `const`/`let`/`var` declarations

---

## TypeScript Handler-Level Unit Tests

### T87–T96: TypeScript definition kinds

| Test | Kind | Unit Test |
|------|------|-----------|
| T87 | class | `test_ts_xray_definitions_finds_class` |
| T88 | interface | `test_ts_xray_definitions_finds_interface` |
| T89 | method | `test_ts_xray_definitions_finds_method` |
| T90 | function | `test_ts_xray_definitions_finds_function` |
| T91 | enum | `test_ts_xray_definitions_finds_enum` |
| T92 | enumMember | `test_ts_xray_definitions_finds_enum_member` |
| T93 | typeAlias | `test_ts_xray_definitions_finds_type_alias` |
| T94 | variable | `test_ts_xray_definitions_finds_variable` |
| T95 | field | `test_ts_xray_definitions_finds_field` |
| T96 | constructor | `test_ts_xray_definitions_finds_constructor` |

---

### T97–T102: TypeScript filters

| Test | Filter | Unit Test |
|------|--------|-----------|
| T97 | baseType (implements) | `test_ts_xray_definitions_base_type_implements` |
| T98 | baseType (abstract/extends) | `test_ts_xray_definitions_base_type_abstract` |
| T99 | containsLine | `test_ts_contains_line_finds_method` |
| T100 | includeBody | `test_ts_xray_definitions_include_body` |
| T101 | combined name+parent+kind | `test_ts_xray_definitions_combined_name_parent_kind` |
| T102 | regex name | `test_ts_xray_definitions_name_regex` |

---

## TypeScript Callers

### T53: `xray_callers` finds TypeScript class method callers

**Expected:**

- `callTree` includes callers from `this.service.method()` pattern
- Receiver type resolved through field type map

---

### T54: `xray_callers` finds TypeScript standalone function calls

**Expected:**

- Standalone function calls have no receiver type
- `DefinitionKind::Function` supported in `find_containing_method`

---

### T55: `xray_callers` with `ext` parameter filters by language

**Expected:**

- `ext: "ts"` → only `.ts` files in results

**Unit test:** `test_mixed_cs_ts_callers_ext_filter`

---

### T56: `xray_callers` finds callers in TypeScript arrow function class properties

**Expected:**

- Arrow function properties treated as methods for call-site extraction

---

### T57: `xray_callers` tracks TypeScript `new` expression constructor calls

**Expected:**

- `new ClassName(...)` tracked as a call to `ClassName`

---

### T58: `xray_callers` resolves Angular `inject()` field types

**Expected:**

- `inject(ClassName)` resolved in both field initializers and constructor assignments
- Generic type arguments stripped

---

### T103–T105: TypeScript callers handler tests

| Test | Scenario | Unit Test |
|------|----------|-----------|
| T103 | callers up | `test_ts_xray_callers_up_finds_caller` |
| T104 | callees down | `test_ts_xray_callers_down_finds_callees` |
| T105 | nonexistent method | `test_ts_xray_callers_nonexistent_method` |

---

## Angular Template Metadata

### T-ANGULAR-01: `def-index` — Angular component selector and template metadata indexed

**Expected:**

- `selector_index` entries mapping selectors to component definitions
- `template_children` listing custom elements from HTML templates

---

### T-ANGULAR-02: `xray_definitions` returns Angular template metadata

**Expected:**

- Component definitions include `selector` and `templateChildren` fields

---

### T-ANGULAR-03: `xray_callers` direction=down shows template children

**Expected:**

- `callTree` includes child components from HTML with `templateUsage: true`

---

### T-ANGULAR-04: `xray_callers` direction=up shows parent components

**Expected:**

- `callTree` includes parent components that use the selector with `templateUsage: true`

---

### T-ANGULAR-04b: `xray_callers` direction=up recursive depth shows grandparents

**Expected:**

- `depth` parameter controls recursion depth for component hierarchy
- Cycle detection prevents infinite loops

---

### T-ANGULAR-05: `def-index` — Graceful handling of missing HTML template

**Expected:**

- No crash on missing `templateUrl`
- Component indexed without `templateChildren`

---

## Parser-Level Unit Tests

### T-PARSER-CONST-ENUM: TypeScript `const enum` parsing

**Unit test:** `test_parse_ts_const_enum`

---

### T-PARSER-INJECTION-TOKEN: TypeScript `InjectionToken` variable parsing

**Unit test:** `test_parse_ts_injection_token_variable`

---

### T-SPEC-AUDIT: `.spec.ts` files — 0 definitions expected

**Expected:**

- `describe()` and `it()` are call expressions, not definitions
- `.spec.ts` files correctly reported with 0 definitions in audit

**Note:** By-design behavior, not a bug.

---

## Encoding

### T-UTF16: UTF-16 BOM detection in `read_file_lossy()`

**Expected:**

- UTF-16LE/BE files decoded via BOM detection
- `0 lossy-utf8` (UTF-16 files no longer lossy)

**Unit tests:** `test_read_file_lossy_utf16le_bom`, `test_read_file_lossy_utf16be_bom`, `test_decode_utf16le_basic`

---

### T-LOSSY: Non-UTF8 file indexing (lossy UTF-8 conversion)

**Expected:**

- Windows-1252 bytes converted with replacement characters
- stderr summary mentions a `lossy-utf8` count (definition-audit logs use the form `"{n} lossy-UTF8 files"`; content-index summary lines use `"{n} lossy-utf8"`)
- Definitions still extracted

---

### T-CR-06: `inject_body_into_obj` — Non-UTF-8 files handled via `read_file_lossy`

**Expected:**

- Non-UTF-8 files return `body` with lossy-converted content
- No `bodyError`

---

## Code Stats — Audit Tests

### T-AUDIT: Independent audit tests for code stats and call chains

**25 unit tests in `src/definitions/audit_tests.rs` organised into 6 parts:**

1. **C# Code Stats** — method/while/do/try-catch/switch/lambda counting against golden fixtures
2. **TypeScript Code Stats** — arrow counting, else-if chain, switch/case
3. **Call Site Accuracy** — all C# and TS call patterns with correct receiver types
4. **Multi-class Call Graphs** — cross-class completeness for both languages
5. **Edge Cases** — nested lambdas, constructor stats, non-method definitions
6. **Statistical Consistency** — invariant checks and cross-language consistency
