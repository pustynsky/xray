# MCP Server Guide

Complete guide for the `xray serve` MCP server — setup, tools API, and examples.

## Overview

The MCP server starts its event loop **immediately** and responds to `initialize` / `tools/list` without waiting for indexes to build. If a pre-built index exists on disk, it is loaded synchronously (< 3s). Otherwise, indexes are built in a background thread — search tools return a friendly "Index is being built, please retry" message until ready. This eliminates startup timeouts when Roo/VS Code launches the server for the first time.

## Setup in VS Code

1. **Install search** (if not already):

   ```bash
   cargo install --path .
   # Or copy xray.exe to a folder in your PATH
   ```

2. **Build a content index** for your project:

   ```bash
   xray content-index -d C:\Projects\MyApp -e cs,sql,csproj
   ```

3. **Create `.vscode/mcp.json`** in your workspace root:

   ```json
   {
     "servers": {
       "xray": {
         "command": "C:\\Users\\you\\.cargo\\bin\\xray.exe",
         "args": [
           "serve",
           "--dir",
           "C:\\Projects\\MyApp",
           "--ext",
           "cs,csproj,xml,config",
           "--watch"
         ]
       }
     }
   }
   ```

   > **Tip:** Include non-code file extensions like `csproj`, `xml`, `config`, `manifestxml` in `--ext` to search NuGet dependencies, project settings, connection strings, and other configuration files alongside your code.

4. **Restart VS Code** — the MCP server starts automatically. Your MCP-compatible AI agent (Roo Code, Cline, etc.) now has access to all MCP tools. The server also sends an `instructions` field during MCP initialization with best practices for tool selection. The instructions include:
   - **TASK ROUTING table** — maps user tasks to recommended tools (auto-generated, context-aware based on indexed file extensions)
   - **DECISION TRIGGERs** — hard prohibitions for file reading and editing that redirect LLM to xray tools
   - **Fallback rule** — guidance for uncertain file types
   - **Strategy Recipes** — multi-step workflow patterns for common tasks
   - **Named policy anchor** — the instructions are wrapped in `=== XRAY_POLICY ===` / `================================` so the agent sees a stable, reusable policy name during MCP initialization

5. **Verify** — ask the AI: _"Use xray_grep to find all files containing HttpClient"_

## Exposed Tools

| Tool                         | Description                                                                                                                             |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `xray_grep`                | Search content index with TF-IDF ranking, regex, phrase, AND/OR                                                                         |
| `xray_definitions`         | Search code definitions (classes, methods, interfaces, etc.). Supports C#, TypeScript/TSX, Rust (tree-sitter) and SQL (regex). `containsLine`, `includeBody`, `audit`. Relevance-ranked when name filter is active. Requires `--definitions` |
| `xray_callers`             | Find callers / callees and build recursive call tree. Supports C#, TypeScript/TSX, and SQL (EXEC call chains). Requires `--definitions`  |
| `xray_find`                | Live filesystem walk (⚠️ slow for large dirs)                                                                                           |
| `xray_fast`                | Search pre-built file name index (instant). Supports comma-separated OR patterns. Results ranked: exact stem → prefix → contains        |
| `xray_info`                | Show all indexes with status, sizes, age                                                                                                |
| `xray_reindex`             | Force rebuild + reload content index                                                                                                    |
| `xray_reindex_definitions` | Force rebuild + reload definition index. Requires `--definitions`                                                                       |
| `xray_edit`                | Edit files by line-range operations or text-match replacements. Auto-creates new files. Supports multi-file (`paths`), insert after/before, expectedContext. Atomic, returns unified diff |
| `xray_help`                | Best practices guide, strategy recipes, performance tiers                                                                               |
| `xray_git_history`         | Commit history for a file. Uses in-memory cache when available (sub-millisecond), falls back to CLI (~2–6 sec)                          |
| `xray_git_diff`            | Commit history with full diff/patch. Always uses CLI (cache has no patch data)                                                          |
| `xray_git_authors`         | Top authors for a file ranked by commit count. Uses in-memory cache when available (sub-millisecond), falls back to CLI                  |
| `xray_git_activity`        | Activity for a date range. `groupBy=file` (default): files with commit counts, authors, subjects. `groupBy=commit`: commits with changed files — ideal for "what was developed?" Uses cache (<1ms) or CLI |
| `xray_git_blame`           | Line-level attribution (`git blame`) for a file or line range. Returns commit hash, author, date, and content per line                   |
| `xray_branch_status`       | Shows current git branch status: branch name, main/master check, behind/ahead counts, dirty files, fetch age. Call before investigating production bugs |

## What the AI Agent Sees

When the AI connects, it discovers tools with full JSON schemas. Each tool has a detailed description with required/optional parameters and examples.

Example interaction:

```
AI:  "Let me search for HttpClient in your codebase..."
     → calls xray_grep { terms: "HttpClient", maxResults: 10 }
     ← receives JSON with file paths, scores, line numbers
AI:  "Found 1,082 files. The most relevant is CustomHttpClient.cs (score: 0.49)..."
```

---

## Response Guidance Fields

Successful **JSON** MCP tool responses may include guidance fields inside `summary`:

| Field | When present | Description |
|---|---|---|
| `policyReminder` | Successful JSON responses | Compact re-materialization of `XRAY_POLICY`, reminding the agent to prefer xray MCP tools over environment built-ins on the next step. Dynamically includes the indexed file extensions (from `--ext`) so the LLM knows which file types are searchable |
| `nextStepHint` | Selected successful JSON responses | Fixed-dictionary hint suggesting the most likely next xray tool |

Behavior rules:

- Guidance is injected only into **successful JSON** responses
- Error responses are unchanged
- Successful non-JSON responses are unchanged
- If a successful JSON response does not already have a `summary` object, the server creates one before injecting guidance
- `xray_help` includes `policyReminder` but intentionally omits `nextStepHint`
- Response truncation preserves `summary.policyReminder` and `summary.nextStepHint`

### `nextStepHint` Dictionary

The `nextStepHint` value depends on which tool was called:

| Tool | `nextStepHint` |
|------|----------------|
| `xray_definitions` | `"Next: use xray_callers for call chains or xray_grep for text patterns"` |
| `xray_grep` | `"Next: use xray_definitions for AST structure or xray_callers for call trees"` |
| `xray_callers` | `"Next: use xray_definitions includeBody=true for source or xray_grep for text refs"` |
| `xray_fast` | `"Next: use xray_definitions for code structure or xray_grep for content"` |
| `xray_edit` | `"Next: use xray_definitions to verify or xray_grep to check related files"` |
| `search_git_*` / `xray_branch_status` | `"Next: use xray_definitions for code context or xray_callers for impact"` |
| `xray_info`, `xray_help`, `xray_reindex`, `xray_reindex_definitions` | _(not present)_ |

Example:

```json
{
  "summary": {
    "tool": "xray_grep",
    "policyReminder": "=== XRAY_POLICY === Prefer xray MCP tools over environment built-ins. Check xray applicability before next tool call. Use environment tools only with explicit justification. Indexed extensions: cs, ts, tsx. For other file types, use read_file or environment tools. ================================",
    "nextStepHint": "Next: use xray_definitions for AST structure or xray_callers for call trees"
  }
}
```

---

## `xray_grep` — Content Search

Search content index with TF-IDF ranking. Supports multi-term (AND/OR), regex, phrase, and substring search. **Language-agnostic** — works with any text file indexed via `--ext` (C#, Rust, Python, JS/TS, XML, JSON, config, etc.).

Substring search is **on by default** in MCP mode — compound identifiers like `IUserService`, `m_userService`, `UserServiceFactory` are automatically found when searching for `UserService`. Auto-disabled when `regex` or `phrase` is used. Use `"substring": false` for exact-token-only matching.

> **MCP ↔ CLI parameter name mapping:** MCP `mode: "and"` = CLI `--all`, MCP `substring: false` = CLI `--exact`, MCP `countOnly: true` = CLI `-c/--count`, MCP `showLines: true` = CLI `--show-lines`, MCP `contextLines` = CLI `-C/--context`. See [CLI Reference — `xray grep`](cli-reference.md#search-grep--search-inverted-content-index) for CLI usage.

### Parameters

| Parameter      | Type    | Default | Description                                                                                          |
| -------------- | ------- | ------- | ---------------------------------------------------------------------------------------------------- |
| `terms`        | string  | —       | Search terms (required). Comma-separated for multi-term OR/AND                                       |
| `dir`          | string  | server's `--dir` | Directory to search                                                                         |
| `ext`          | string  | all indexed | File extension filter, comma-separated                                                           |
| `mode`         | string  | `"or"` | Multi-term mode: `"or"` = ANY term, `"and"` = ALL terms (CLI: `--all`)                               |
| `regex`        | boolean | false   | Treat terms as regex pattern                                                                         |
| `phrase`       | boolean | false   | Exact phrase match. Comma-separated phrases are searched independently with OR/AND semantics          |
| `substring`    | boolean | true    | Match within tokens (finds `IUserService` when searching `UserService`). Auto-disabled for regex/phrase. (CLI: `--exact` to disable) |
| `showLines`    | boolean | false   | Include matching source lines in results (CLI: `--show-lines`)                                       |
| `contextLines` | integer | 0       | Context lines before/after each match, requires `showLines` (CLI: `-C`)                              |
| `maxResults`   | integer | 50      | Max results (0 = unlimited)                                                                          |
| `excludeDir`   | array   | —       | Directory names to exclude                                                                           |
| `exclude`      | array   | —       | File path substrings to exclude                                                                      |
| `countOnly`    | boolean | false   | Return counts only — no file list (CLI: `-c/--count`)                                                |

### Response Fields

```json
// Request
{ "terms": "HttpClient", "maxResults": 3, "ext": "cs" }

// Response
{
  "files": [
    {
      "file": "Services/CustomHttpClient.cs",
      "score": 0.49,
      "matchingTokens": ["httpclient"],
      "termCounts": { "httpclient": 12 }
    },
    {
      "file": "Controllers/ApiController.cs",
      "score": 0.31,
      "matchingTokens": ["httpclient"],
      "termCounts": { "httpclient": 3 }
    }
  ],
  "summary": {
    "tool": "xray_grep",
    "totalFiles": 1082,
    "returned": 3,
    "searchTimeMs": 0.6,
    "responseTruncated": false
  }
}
```

When `showLines: true`:

```json
{
  "files": [
    {
      "file": "Services/CustomHttpClient.cs",
      "score": 0.49,
      "lineGroups": [
        {
          "startLine": 15,
          "lines": [
            "    private readonly HttpClient _client;",
            "    ",
            "    public CustomHttpClient(HttpClient client)"
          ],
          "matchIndices": [0, 2]
        }
      ]
    }
  ]
}
```

When `countOnly: true`, returns only summary with file/token counts (~46 tokens vs 265+ for full results).

When `responseTruncated: true` appears in the summary, narrow your query with `ext`, `dir`, `excludeDir`, or use `countOnly: true`.

---

## `xray_callers` — Call Tree

Traces who calls a method (or what a method calls) and builds a hierarchical call tree. Combines the content index (grep) with the definition index (AST) to determine which method/class contains each call site. Replaces 7+ sequential `xray_grep` + `read_file` calls with a single request. Supports C#, TypeScript/TSX, and SQL (call sites from stored procedure bodies: EXEC, FROM, JOIN, INSERT, UPDATE, DELETE). For SQL, the `class` parameter maps to schema name (e.g., `class="dbo"`).

```json
// Find all callers of ExecuteQueryAsync, 5 levels deep, excluding tests
{
  "method": "ExecuteQueryAsync",
  "direction": "up",
  "depth": 5,
  "excludeDir": ["\\test\\", "\\Mock\\"]
}

// Result: hierarchical call tree
{
  "callTree": [
    {
      "method": "RunQueryAsync",
      "class": "QueryService",
      "file": "QueryService.cs",
      "line": 386,
      "callers": [
        {
          "method": "HandleRequestAsync",
          "class": "QueryController",
          "line": 154,
          "callers": [
            { "method": "ProcessBatchAsync", "class": "BatchProcessor", "line": 275 }
          ]
        }
      ]
    },
    { "method": "ExecuteQueryAsync", "class": "QueryProxy", "file": "QueryProxy.cs", "line": 74 }
  ],
  "summary": { "totalNodes": 19, "searchTimeMs": 0.13, "truncated": false }
}
```

### Parameters

| Parameter            | Description                                                                                                                                         |
| -------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| `method` (required)  | Method name to trace. Comma-separated for multi-method batch (e.g., `"Foo,Bar,Baz"`). Each method gets an independent call tree. Single method returns `{callTree}`, multiple returns `{results: [{method, callTree, nodesInTree}, ...]}` |
| `class`              | Scope to a specific class. DI-aware: `class: "UserService"` also finds callers using `IUserService`. Works for both `"up"` and `"down"` directions. |
| `direction`          | `"up"` = find callers (default), `"down"` = find callees                                                                                            |
| `depth`              | Max recursion depth (default: 3, max: 10)                                                                                                           |
| `maxCallersPerLevel` | Max callers per node (default: 10). Prevents explosion.                                                                                             |
| `maxTotalNodes`      | Max total nodes in tree (default: 200). Caps output size.                                                                                           |
| `excludeDir`         | Directory substrings to exclude, e.g. `["\\test\\", "\\Mock\\"]`                                                                                    |
| `excludeFile`        | File path substrings to exclude                                                                                                                     |
| `resolveInterfaces`  | Auto-resolve interface → implementation (default: true)                                                                                             |
| `ext`                | File extension filter (default: server's `--ext`)                                                                                                   |
| `includeBody`        | Include source code body of each method in the call tree (default: false). Also adds `rootMethod` with the target method's body                     |
| `includeDocComments` | Expand body upward to include doc-comments above definitions. Implies `includeBody=true`. Adds `docCommentLines` field (default: false)             |
| `maxBodyLines`       | Max source lines per method when `includeBody=true` (default: 30, 0=unlimited)                                                                      |
| `maxTotalBodyLines`  | Max total body lines across all methods in the tree (default: 300, 0=unlimited)                                                                      |
| `impactAnalysis`     | When `true` with `direction=up`, identifies test methods covering the target. Returns `testsCovering` array with full file path, `depth`, and `callChain`. Test nodes marked `isTest: true`. Recursion stops at tests. Tests detected via C# `[Test]`/`[Fact]`/`[Theory]`/`[TestMethod]`, Rust `#[test]`, TS `*.spec.ts`/`*.test.ts` files. (default: false) |

### Impact Analysis

Find which tests cover a method — one call replaces manual multi-step investigation.

```json
// Request
{
  "method": "SaveOrder",
  "class": "OrderService",
  "direction": "up",
  "depth": 5,
  "impactAnalysis": true
}

// Response
{
  "callTree": [
    {
      "method": "ProcessCheckout",
      "class": "CheckoutController",
      "callers": [
        {
          "method": "TestCheckout_SavesOrder",
          "class": "CheckoutTests",
          "file": "CheckoutTests.cs",
          "isTest": true
        }
      ]
    }
  ],
  "testsCovering": [
    {
      "method": "TestCheckout_SavesOrder",
      "class": "CheckoutTests",
      "file": "test/CheckoutTests.cs",
      "depth": 2,
      "callChain": ["SaveOrder", "ProcessCheckout", "TestCheckout_SavesOrder"]
    }
  ],
  "summary": { "totalNodes": 3, "testsFound": 1, "searchTimeMs": 0.15 }
}
```

`callChain` shows the method-by-method path from target to test. Short chain (depth 1-2) = direct test. Long chain (depth 4+) = transitive via helpers — may be less relevant.

### Response Fields with `includeBody`

When `includeBody: true`, each node in the call tree includes source code:

```json
{
  "callTree": [
    {
      "method": "RunQueryAsync",
      "class": "QueryService",
      "file": "QueryService.cs",
      "line": 386,
      "body": [
        "public async Task<Result> RunQueryAsync(string sql)",
        "{",
        "    return await ExecuteQueryAsync(sql);",
        "}"
      ],
      "bodyStartLine": 386,
      "bodyTruncated": false,
      "callers": []
    }
  ],
  "rootMethod": {
    "name": "ExecuteQueryAsync",
    "class": "QueryService",
    "file": "QueryService.cs",
    "lines": "400-420",
    "body": ["public async Task<Result> ExecuteQueryAsync(string sql)", "{", "    ..."],
    "bodyStartLine": 400,
    "bodyTruncated": false
  },
  "summary": { "totalNodes": 1, "searchTimeMs": 0.2 }
}
```

| Response field    | When present                                    | Description                                          |
| ----------------- | ----------------------------------------------- | ---------------------------------------------------- |
| `body`            | `includeBody=true` and body budget not exceeded  | Array of source lines for the method                 |
| `bodyStartLine`   | `includeBody=true` and body budget not exceeded  | 1-based line number of the first body line            |
| `bodyTruncated`   | Body exceeds `maxBodyLines`                      | `true` when body was cut short                       |
| `bodyOmitted`     | Global `maxTotalBodyLines` budget exceeded        | `true` — body skipped entirely for this node         |
| `bodyWarning`     | Body omitted                                     | Human-readable reason for omission                   |
| `docCommentLines` | `includeDocComments=true` and doc-comment found   | Number of lines that are doc-comments (before method declaration) |
| `rootMethod`      | `includeBody=true`                               | Top-level object with the searched method's own body |
| `callSite`        | Always (caller nodes)                            | Line number of the first call site                   |
| `callSites`       | >1 call sites in same caller                     | Array of all call site line numbers (e.g., `[273, 475, 486]`). Only present when a method is called multiple times within the same caller method. `callSite` is always the first element. |

### Multi-Method Batch

Query multiple methods in a single call to reduce MCP round trips. Each method gets its own independent call tree with its own `maxTotalNodes` budget. `maxTotalBodyLines` is shared across all methods.

```json
// Request: trace callers of 3 methods at once
{
  "method": "GetUser,SaveOrder,ValidateInput",
  "class": "OrderService",
  "direction": "up",
  "depth": 2
}

// Response: results array with per-method trees
{
  "results": [
    {
      "method": "GetUser",
      "callTree": [...],
      "nodesInTree": 5
    },
    {
      "method": "SaveOrder",
      "callTree": [...],
      "nodesInTree": 12
    },
    {
      "method": "ValidateInput",
      "callTree": [...],
      "nodesInTree": 3
    }
  ],
  "query": {
    "methods": ["GetUser", "SaveOrder", "ValidateInput"],
    "class": "OrderService",
    "direction": "up",
    "depth": 2
  },
  "summary": {
    "totalMethods": 3,
    "totalNodes": 20,
    "searchTimeMs": 0.45
  }
}
```

**Budget behavior:**
- `maxTotalNodes` — per-method (each gets full budget independently)
- `maxTotalBodyLines` — shared across all methods
- Response size auto-scales: `max(base, 32KB × N methods)`, capped at 128KB

**Backward compatibility:** Single-method calls (no comma) return the existing format with `callTree` at the top level. Multi-method calls return `results` array.

### Limitations

- **Interface vs concrete class name** — when searching for callers, always use the **interface name** (e.g., `class: "IUserService"`) rather than the concrete class name (e.g., `class: "UserService"`). Calls through DI use the interface type as the receiver. Searching with the concrete class returns 0 callers if all call sites use the interface. Alternatively, set `resolveInterfaces: true` to auto-resolve implementations.

- **Local variable calls not tracked** — calls through local variables (e.g., `var x = service.GetFoo(); x.Bar()`) may not be detected because the tool uses AST parsing without type inference. DI-injected fields, `this`/`base` calls, and direct receiver calls are fully supported.

---

## `xray_definitions` — Code Definitions

Search code definitions: classes, methods, interfaces, enums, functions, type aliases, stored procedures. Supports C#, TypeScript/TSX, and Rust via tree-sitter grammars; SQL via regex parser. Requires `--definitions`.

Results are **relevance-ranked** when a `name` filter is active (non-regex): exact matches first, then prefix matches, then substring matches. Within the same match tier, type-level definitions (classes, interfaces, enums) sort before members (methods, properties), and shorter names before longer. See [Architecture — Relevance Ranking](architecture.md#relevance-ranking) for details.

### Parameters

| Parameter           | Type    | Default | Description                                                                              |
| ------------------- | ------- | ------- | ---------------------------------------------------------------------------------------- |
| `name`              | string  | —       | Substring filter — use short fragments (e.g., `name='Scan'` finds `PostScanOrdersAsAdmin`). Comma-separated for multi-term OR. Use `regex=true` for patterns |
| `detail`            | string  | —       | Detail level: omit for auto (compact format when >20 results without name filter), `'full'` to force all fields |
| `kind`              | string  | —       | Filter by definition kind. Comma-separated for multi-kind OR (e.g., `class,interface,enum`). Valid: class, interface, method, property, field, enum, struct, record, constructor, delegate, event, enumMember, function, typeAlias, variable, storedProcedure, table, view, sqlFunction, userDefinedType, column, sqlIndex |
| `attribute`         | string  | —       | Filter by C# attribute or TypeScript decorator                                           |
| `baseType`          | string  | —       | Filter by base type/interface (substring match — `IAccessTable` finds `IAccessTable<Model>`, etc.) |
| `baseTypeTransitive`| boolean | false   | With `baseType`, traverses inheritance chain transitively (BFS, max depth 10). Finds classes that inherit from classes that inherit from the specified baseType |
| `file`              | string  | —       | Filter by file path substring. Comma-separated for multi-term OR                         |
| `parent`            | string  | —       | Filter by parent class name                                                              |
| `containsLine`      | integer | —       | Find definition containing a line number (requires `file`). With `includeBody=true`, body is emitted only for innermost definition; parents get `bodyOmitted` |
| `regex`             | boolean | false   | Treat `name` as regex                                                                    |
| `maxResults`        | integer | 100     | Max results returned                                                                     |
| `excludeDir`        | array   | —       | Exclude directories                                                                      |
| `includeBody`       | boolean | false   | Include source code body inline                                                          |
| `includeDocComments`| boolean | false   | Expand body upward to include `///` (C#/Rust) or `/** */` (TypeScript) doc-comments. Implies `includeBody=true`. Adds `docCommentLines` field |
| `maxBodyLines`      | integer | 100     | Max lines per definition body (0 = unlimited)                                            |
| `maxTotalBodyLines` | integer | 500     | Max total body lines across all results (0 = unlimited)                                  |
| `audit`             | boolean | false   | Return index coverage report instead of search results                                   |
| `auditMinBytes`     | integer | 500     | Min file size to flag as suspicious in audit mode                                        |
| `crossValidate`     | boolean | false   | With `audit=true`, compares definition index files against file-list index to find coverage gaps |
| `includeCodeStats`  | boolean | false   | Include complexity metrics (`codeStats` object) for methods/functions/constructors        |
| `sortBy`            | string  | —       | Sort by metric descending. Values: `cyclomaticComplexity`, `cognitiveComplexity`, `maxNestingDepth`, `paramCount`, `returnCount`, `callCount`, `lambdaCount`, `lines`. Auto-enables `includeCodeStats` |
| `minComplexity`     | integer | —       | Filter: min cyclomatic complexity. Auto-enables `includeCodeStats`                       |
| `minCognitive`      | integer | —       | Filter: min cognitive complexity. Auto-enables `includeCodeStats`                        |
| `minNesting`        | integer | —       | Filter: min nesting depth. Auto-enables `includeCodeStats`                               |
| `minParams`         | integer | —       | Filter: min parameter count. Auto-enables `includeCodeStats`                             |
| `minReturns`        | integer | —       | Filter: min return/throw count. Auto-enables `includeCodeStats`                          |
| `minCalls`          | integer | —       | Filter: min call count (fan-out). Auto-enables `includeCodeStats`                        |

### `containsLine` — Find Containing Method

Find which method/class contains a given line number. No more `read_file` just to figure out "what method is on line 812".

With `includeBody=true`, body is emitted **only for the innermost (most specific) definition**. Parent definitions receive `bodyOmitted` with a hint instead — this maximizes the body budget for the target method.

```json
// Request
{ "file": "QueryService.cs", "containsLine": 812 }

// Response: definitions containing that line, sorted by specificity (innermost first)
{
  "containingDefinitions": [
    { "name": "ExecuteQueryAsync", "kind": "method", "lines": "766-830", "parent": "QueryService" },
    { "name": "QueryService", "kind": "class", "lines": "1-900" }
  ]
}
```

With `includeBody=true`:

```json
// Request
{ "file": "QueryService.cs", "containsLine": 812, "includeBody": true }

// Response: innermost gets body, parent gets bodyOmitted
{
  "containingDefinitions": [
    {
      "name": "ExecuteQueryAsync",
      "kind": "method",
      "lines": "766-830",
      "parent": "QueryService",
      "body": ["public async Task<Result> ExecuteQueryAsync(...)", "{", "    ..."]
    },
    {
      "name": "QueryService",
      "kind": "class",
      "lines": "1-900",
      "bodyOmitted": "parent definition - use includeBody with name filter to get full body"
    }
  ]
}
```

### `includeBody` — Return Source Code Inline

Retrieve the actual source code of definitions without a separate `read_file` call. Three-level protection prevents response explosion:

- **`maxBodyLines`** — caps lines per individual definition (default: 100, 0 = unlimited)
- **`maxTotalBodyLines`** — caps total body lines across all results (default: 500, 0 = unlimited)
- **`maxResults`** — caps the number of definitions returned (default: 100)

When a definition's body exceeds `maxBodyLines`, the `body` array is truncated and `bodyTruncated: true` is set. When the global `maxTotalBodyLines` budget is exhausted, remaining definitions receive `bodyOmitted: true` with a `bodyWarning` message. If the source file cannot be read, `bodyError` is returned instead.

When body is truncated, the summary includes `totalBodyLinesAvailable` — the total body lines that would have been returned without truncation. Use this value to calibrate `maxTotalBodyLines` for a retry (e.g., if `totalBodyLinesReturned: 500` and `totalBodyLinesAvailable: 2300`, set `maxTotalBodyLines: 2300`). The field is absent when all bodies fit within the budget.

```json
// Request
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "xray_definitions",
    "arguments": {
      "name": "GetProductEntriesAsync",
      "includeBody": true,
      "maxBodyLines": 10
    }
  }
}

// Response
{
  "definitions": [
    {
      "name": "GetProductEntriesAsync",
      "kind": "method",
      "file": "ProductService.cs",
      "lines": "142-189",
      "parent": "ProductService",
      "bodyStartLine": 142,
      "body": [
        "public async Task<List<ProductEntry>> GetProductEntriesAsync(int tenantId)",
        "{",
        "    var entries = await _repository.GetEntriesAsync(tenantId);",
        "    if (entries == null)",
        "    {",
        "        _logger.LogWarning(\"No entries found for tenant {TenantId}\", tenantId);",
        "        return new List<ProductEntry>();",
        "    }",
        "    return entries.Where(e => e.IsActive).ToList();",
        "}"
      ],
      "bodyTruncated": false
    }
  ],
  "summary": {
    "total": 1,
    "searchTimeMs": 0.4,
    "totalBodyLines": 10,
    "totalBodyLinesReturned": 10
  }
}
```

### `includeCodeStats` — Code Complexity Metrics

Get complexity metrics for methods, functions, and constructors. Metrics are always computed during indexing — this parameter just controls output visibility.

```json
// Request: find 20 most complex methods
{ "sortBy": "cognitiveComplexity", "maxResults": 20 }

// Response
{
  "definitions": [
    {
      "name": "ProcessOrder",
      "kind": "method",
      "parent": "OrderService",
      "file": "Services/OrderService.cs",
      "lines": "45-89",
      "codeStats": {
        "lines": 45,
        "cyclomaticComplexity": 12,
        "cognitiveComplexity": 18,
        "maxNestingDepth": 4,
        "paramCount": 3,
        "returnCount": 4,
        "callCount": 8,
        "lambdaCount": 1
      }
    }
  ],
  "summary": { "totalResults": 1247, "returned": 20, "sortedBy": "cognitiveComplexity" }
}
```

**Combine filters for "God Method" detection:**

```json
// Find methods with high complexity AND many params AND high fan-out
{ "minComplexity": 20, "minParams": 5, "minCalls": 15, "sortBy": "cyclomaticComplexity" }
```

**Note:** Classes, fields, and enum members do not have `codeStats`. Old indexes (before this feature) return results normally with `summary.codeStatsAvailable: false` — run `xray_reindex_definitions` to compute metrics.

### `audit` — Index Coverage Report

Check if all files in the repository are properly indexed. Files >500 bytes with 0 definitions are flagged as suspicious (possible parse failures).

```json
// Request
{ "audit": true }

// Response
{
  "audit": {
    "totalFiles": 48730,
    "filesWithDefinitions": 48177,
    "filesWithoutDefinitions": 553,
    "readErrors": 0,
    "lossyUtf8Files": 44,
    "suspiciousFiles": 390,
    "suspiciousThresholdBytes": 500
  },
  "suspiciousFiles": [
    { "file": "Tools\\CodeGenerator\\GlobalSuppressions.cs", "bytes": 2312 },
    { "file": "Tests\\Common\\AssemblyInfo.cs", "bytes": 2122 }
  ]
}
```

> **Note:** Most "suspicious" files are legitimate — `AssemblyInfo.cs` and `GlobalSuppressions.cs` contain assembly-level attributes that the parser doesn't extract as definitions. Use `auditMinBytes` to raise the threshold if needed.

### Zero-Result Hints

When `xray_definitions` returns 0 results, the response `summary` may include a `hint` field with a contextual suggestion to help correct the query. This is particularly useful for LLM agents that may use wrong `kind` values across languages or confuse `xray_definitions` with `xray_grep`.

Five types of hints are generated (first matching wins):

| Hint | When | Example |
|------|------|---------|
| **Unsupported extension** | `file` filter has extension not in `def_extensions` (checked first — highest priority) | If in content index: `"Extension '.xml' is not in the definition index. However, .xml files ARE indexed in the content index. Use xray_grep."` If not in any index: `"Extension '.xyz' is not supported by any index. Use read_file."` |
| **Wrong kind** | `kind` filter set + `name` or `file` filter set, but definitions exist with different kinds | `"0 results with kind='method'. Without kind filter: 8 defs found (5 function, 2 struct). Did you mean kind='function'?"` |
| **File has definitions** | `file` filter matches files with definitions, but other filters (name/kind/parent) are too narrow | If name exists in other files: `"File 'tips.rs' has 8 definitions (...). Found 'X' in other.rs — consider removing file filter."` If name doesn't exist anywhere: `"File 'tips.rs' has 8 definitions (...). Use xray_grep for content search."` |
| **Nearest name** | `name` filter set (non-regex), closest name in index has ≥80% Jaro-Winkler similarity | `"0 results for name='getusr'. Nearest match: 'getuser' (1 definition, similarity 96%)"` |
| **Name in content** | `name` not found as AST definition but exists in content index as text | `"'inputSchema' not found as an AST definition name, but appears in 3 files. The files contain 2 classes and 15 methods total. Tip: use parent='<ClassName>' kind='method'. Use xray_grep."` |
| **CamelCase fragments** | Long `name` (>15 chars) returns 0 results and no other hint fired | `"No definitions match name='PostUpdateOrdersAsAdmin'. Try shorter fragments: name='Post' or name='Update' or name='Orders' or name='Admin'. Or use comma-separated: name='Post,Update,Orders,Admin' for multi-term OR."` |

Hints are **not generated** when results are found (zero overhead for successful queries). The existing `kind='property'` → `kind='field'` TypeScript hint is preserved and takes priority.

### Auto-Correction

Before generating hints, `xray_definitions` attempts to **automatically correct** the query and return results in a single round-trip (no second LLM call needed). Two correction types:

| Correction | When | What happens |
|---|---|---|
| **Kind mismatch** | `kind` filter set + `name` or `file` set, 0 results | Removes kind, finds correct kind, re-runs. E.g., `kind='method'` on Rust code → auto-corrects to `kind='function'` |
| **Nearest name** | `name` set (non-regex), nearest match ≥85% Jaro-Winkler | Re-runs with corrected name. E.g., `name='hndl_search'` → `name='handle_xray_find'` |

When auto-correction produces results, the response includes an `autoCorrection` object in `summary`:

```json
{
  "definitions": [...],
  "summary": {
    "autoCorrection": {
      "type": "kindCorrected",
      "original": { "kind": "method" },
      "corrected": { "kind": "function" },
      "reason": "kind='method' returned 0 results, auto-corrected to kind='function'"
    }
  }
}
```

For name corrections, the object also includes `"similarity": "95%"`.

If auto-correction produces 0 results, it falls through to the regular hint system described above.

### Missing Terms Detection

When a multi-name query with a `kind` filter returns results but some terms are silently dropped due to kind mismatch, the response `summary` includes a `missingTerms` array:

```json
// Request: name="UserService,GetUser" kind="class"
// UserService is a class (found), GetUser is a method (filtered out by kind)
{
  "definitions": [
    { "name": "UserService", "kind": "class", "file": "UserService.cs" }
  ],
  "summary": {
    "totalResults": 1,
    "termBreakdown": { "userservice": 1, "getuser": 0 },
    "missingTerms": [
      { "term": "getuser", "reason": "kind mismatch: found as method, not class" }
    ]
  }
}
```

`missingTerms` is only generated when:
- Multi-name query (2+ comma-separated terms)
- `kind` filter is active
- At least one term has results (total > 0)
- At least one term is missing from results

Possible `reason` values:
- `"kind mismatch: found as <actual_kind>, not <requested_kind>"` — the term exists but with a different kind
- `"not found in index"` — the term doesn't exist in the definition index at all

### Auto-Compact Mode

When `xray_definitions` returns more than 20 results **without a `name` filter** and `includeBody` is false, it automatically switches to **compact format** — each definition contains only `name`, `kind`, `file`, `lines`, and `parent`. Fields like `signature`, `modifiers`, `attributes`, `baseTypes`, `templateChildren`, and `selector` are omitted to reduce response size (7-12x smaller).

The summary includes `compactMode: true` and `compactReason` with instructions on how to get full details. Use `detail='full'` to override and force full format with all fields.

### Auto-Summary for Broad Queries

When `xray_definitions` finds more results than `maxResults` and **no `name` filter** is set (and `includeBody` is false, and `sortBy` is not set), it automatically returns a **directory-grouped summary** instead of truncated entries. This eliminates the need for preliminary `xray_fast dirsOnly=true` calls when exploring unfamiliar code modules. Each group includes `memberNames` — an alphabetically sorted list of `Parent.Method` names (deduped, globally capped at 50) for quick method discovery.

```json
// Request: explore a large service directory
{ "file": "Services/" }

// Response: directory-grouped summary (instead of truncated entries)
{
  "autoSummary": {
    "groups": [
      {
        "directory": "Orders",
        "total": 180,
        "counts": { "class": 12, "interface": 5, "method": 120, "field": 43 },
        "topDefinitions": ["OrderService", "OrderProcessor", "OrderValidator"]
      },
      {
        "directory": "Users",
        "total": 250,
        "counts": { "class": 15, "interface": 8, "method": 170, "field": 57 },
        "topDefinitions": ["UserService", "UserRepository", "AuthenticationManager"]
      }
    ],
    "totalDefinitions": 3222,
    "groupCount": 12,
    "hint": "Use file='Orders' to explore the largest group, or name='OrderService' for a specific class"
  },
  "summary": {
    "totalResults": 3222,
    "returned": 0,
    "autoSummaryMode": true,
    "searchTimeMs": 0.8
  }
}
```

**Activation conditions** (all must be true):
- `totalResults > maxResults` — results exceed the limit
- No `name` filter — broad exploration query
- `includeBody` is false — not requesting source code
- `sortBy` is not set — when sorting is requested, individual ranked results are returned instead

**To get individual definitions instead**, add a `name` filter or narrow the `file` scope.

| Response field | Description |
|---|---|
| `autoSummary.groups[]` | Array of directory groups, sorted by `total` descending |
| `groups[].directory` | Subdirectory name (1 level below `file` filter) |
| `groups[].total` | Total definitions in this directory |
| `groups[].counts` | Definition counts by kind (`class`, `method`, etc.) |
| `groups[].topDefinitions` | Top-3 largest classes/interfaces/structs/enums by line count |
| `groups[].memberNames` | Alphabetically sorted list of `Parent.Method` names (deduped, globally capped at 50). Only includes method/function/constructor kinds |
| `autoSummary.totalDefinitions` | Grand total of all matching definitions |
| `autoSummary.hint` | Context-aware suggestion for next query |
| `summary.autoSummaryMode` | `true` when auto-summary is active |

---

## `xray_fast` — File Name Search

Search pre-built file name index for instant file lookup (~35ms vs ~3s for live filesystem walk). Auto-builds index if not present. Supports comma-separated patterns for multi-file lookup (OR logic). Supports `pattern='*'` or empty pattern with `dir` for wildcard listing (all files/directories). Results are relevance-ranked: exact stem match → prefix match → contains match (ranking skipped for wildcard).

### Parameters

| Parameter   | Type    | Default          | Description                                                  |
| ----------- | ------- | ---------------- | ------------------------------------------------------------ |
| `pattern`   | string  | —                | File name pattern (required). Comma-separated for multi-term OR. Use `'*'` to list all entries. Empty string with `dir` also lists all |
| `dir`       | string  | server's `--dir` | Directory to search                                          |
| `ext`       | string  | —                | Filter by extension                                          |
| `regex`     | boolean | false            | Treat as regex                                               |
| `ignoreCase`| boolean | false            | Case-insensitive                                             |
| `dirsOnly`  | boolean | false            | Show only directories. When true, `ext` filter is ignored (directories have no extension); response includes a hint |
| `filesOnly` | boolean | false            | Show only files                                              |
| `countOnly` | boolean | false            | Count only                                                   |

### Response

```json
// Request
{ "pattern": "UserService", "ext": "cs" }

// Response
{
  "files": [
    "src/Services/UserService.cs",
    "src/Services/IUserService.cs",
    "test/UserServiceTests.cs"
  ],
  "summary": { "tool": "xray_fast", "totalMatches": 3, "searchTimeMs": 35 }
}
```

---

## `xray_info` — Index Information

Shows all existing indexes with their status, sizes, age, and memory usage. No parameters.

### Response

```json
{
  "directory": "C:\\Users\\you\\AppData\\Local\\xray",
  "indexes": [
    {
      "type": "content",
      "root": "C:\\Projects\\MyApp",
      "files": 48986,
      "uniqueTokens": 754000,
      "totalTokens": 33229888,
      "extensions": ["cs", "sql"],
      "sizeMb": 242.7,
      "ageHours": 0.5,
      "inMemory": true
    },
    {
      "type": "definition",
      "root": "C:\\Projects\\MyApp",
      "files": 48730,
      "definitions": 846000,
      "callSites": 2400000,
      "extensions": ["cs"],
      "sizeMb": 324.0,
      "ageHours": 0.5,
      "inMemory": true
    },
    {
      "type": "file-list",
      "root": "C:\\Projects\\MyApp",
      "sizeMb": 47.8
    },
    {
      "type": "git-history",
      "commits": 12345,
      "files": 2500,
      "authors": 42,
      "branch": "main",
      "headHash": "abc123de",
      "sizeMb": 1.2,
      "inMemory": true
    }
  ],
  "memoryEstimate": {
    "contentIndex": "...",
    "definitionIndex": "...",
    "gitCache": "...",
    "process": { "workingSetMb": 512 }
  }
}
```

---

## `xray_reindex` — Rebuild Content Index

Force rebuild the content index and reload it into the server's in-memory cache. Useful after many file changes or when `--watch` is not enabled.

### Parameters

| Parameter | Type   | Default          | Description                       |
| --------- | ------ | ---------------- | --------------------------------- |
| `dir`     | string | server's `--dir` | Directory to reindex              |
| `ext`     | string | server's `--ext` | File extensions (comma-separated) |

### Response

```json
{
  "status": "ok",
  "files": 48986,
  "uniqueTokens": 754000,
  "rebuildTimeMs": 1200.5
}
```

---

## `xray_reindex_definitions` — Rebuild Definition Index

Force rebuild the AST definition index (tree-sitter) and reload it into the server's in-memory cache. Requires server started with `--definitions` flag.

### Parameters

| Parameter | Type   | Default          | Description                              |
| --------- | ------ | ---------------- | ---------------------------------------- |
| `dir`     | string | server's `--dir` | Directory to reindex                     |
| `ext`     | string | server's `--ext` | File extensions to parse, comma-separated |

### Response

```json
{
  "status": "ok",
  "files": 48730,
  "definitions": 846000,
  "callSites": 2400000,
  "codeStatsEntries": 320000,
  "sizeMb": 324.0,
  "rebuildTimeMs": 16500.0
}
```

---

## `xray_edit` — File Editing

Edit files by line-range operations or text-match replacements. Works on any text file (not limited to `--dir`). Supports multi-file editing, insert after/before, safety checks, and returns unified diff.

### Response Fields

| Response field     | When present                              | Description                                              |
| ------------------ | ----------------------------------------- | -------------------------------------------------------- |
| `applied`          | Always                                    | Number of edits processed                                |
| `fileCreated`      | File didn't exist before edit             | `true` when file was auto-created (non-existent → treated as empty, insert operations succeed) |
| `diff`             | Always                                    | Unified diff or `"(no changes)"`                         |
| `linesAdded`       | Always                                    | Lines added (net)                                        |
| `linesRemoved`     | Always                                    | Lines removed (net)                                      |
| `newLineCount`     | Always                                    | Total lines after editing                                |
| `totalReplacements`| Mode B with matches                       | Number of text replacements made                         |
| `dryRun`           | Always (single-file)                      | Whether file was actually written                        |
| `skippedEdits`     | `skipIfNotFound=true` with skipped edits  | Count of edits that were skipped                         |
| `skippedDetails`   | `skipIfNotFound=true` with skipped edits  | Array of `{editIndex, search, reason}` per skipped edit  |
| `results`          | Multi-file mode (`paths`)                 | Array of per-file results                                |
| `summary`          | Multi-file mode (`paths`)                 | `{filesEdited, totalApplied, dryRun}`                    |

### Error Diagnostics — Nearest Match Hint

When search text, regex pattern, or anchor text is not found, the error message includes a **nearest match hint** showing the most similar line in the file:

```
Text not found: "Девять "израильтян"". Nearest match at line 2 (similarity 92%): "Девять «израильтян»"
```

**Behavior:**
- Uses char-level LCS ratio (`similar::TextDiff`) for similarity scoring
- Multi-line search text: sliding window of N lines for comparison
- Suppressed for files > 500KB (performance protection)
- Suppressed when best similarity < 40% (unhelpful)
- Applied to all three error types: `"Text not found"`, `"Pattern not found"`, `"Anchor text not found"`

### skipIfNotFound — Skipped Edit Details

When `skipIfNotFound=true` is used and edits are skipped, the response includes detailed information about each skipped edit:

```json
{
  "skippedEdits": 2,
  "skippedDetails": [
    { "editIndex": 0, "search": "SemaphoreSlim(10)", "reason": "text not found" },
    { "editIndex": 3, "search": "missing_anchor", "reason": "anchor text not found" }
  ]
}
```

Possible `reason` values: `"text not found"`, `"regex pattern not found"`, `"anchor text not found"`.

For full parameter documentation, see `xray_help` → `parameterExamples` → `xray_edit`.

---

## Git History Tools

Six MCP tools for querying git history. Always available — no flags needed. When the in-memory git history cache is ready (built automatically in the background on server startup), `xray_git_history`, `xray_git_authors`, and `xray_git_activity` use sub-millisecond cache lookups. When the cache is not ready (first ~60 sec on cold start), these tools transparently fall back to CLI `git log` commands (~2–6 sec). `xray_git_diff` and `xray_git_blame` always use CLI.

Cache responses include a `"(from cache)"` hint in the `summary` field so the AI agent knows the data source.

### Parameters (shared across git tools)

| Parameter    | Type   | Required | Description |
|---|---|---|---|
| `repo`       | string | ✅ | Path to local git repository |
| `file`       | string | ✅* | File path relative to repo root (*required for `xray_git_history`, `xray_git_diff`, `xray_git_blame`) |
| `path`       | string | — | File or directory path relative to repo root. `xray_git_authors` and `xray_git_activity` accept `path` (file, directory, or omit for entire repo). `file` is a backward-compatible alias for `path` in `xray_git_authors` |
| `from`       | string | — | Start date (YYYY-MM-DD, inclusive) |
| `to`         | string | — | End date (YYYY-MM-DD, inclusive) |
| `date`       | string | — | Exact date (YYYY-MM-DD), overrides from/to |
| `maxResults` | number | — | Maximum results to return (default: 50) |
| `top`        | number | — | Maximum authors to return (default: 10, `xray_git_authors` only) |
| `author`     | string | — | Filter by author name or email (case-insensitive substring match). Available on `xray_git_history`, `xray_git_diff`, `xray_git_activity` |
| `message`    | string | — | Filter by commit message (case-insensitive substring match). Available on `xray_git_history`, `xray_git_diff`, `xray_git_activity`, `xray_git_authors` |
| `noCache`    | boolean | — | If true, bypass the in-memory git history cache and query git CLI directly. Useful when cache may be stale. Available on `xray_git_history`, `xray_git_authors`, `xray_git_activity` |

### Cache behavior

| Scenario | Behavior |
|---|---|
| Server just started, no `.git-history` on disk | Cache builds in background (~59 sec). Tools use CLI fallback during build. |
| Server restart, `.git-history` exists on disk | Cache loads from disk (~100 ms). Tools use cache almost immediately. |
| HEAD changed since cache was built | Cache rebuilds in background. Old cache (if loaded from disk) serves queries during rebuild. |
| `xray_git_diff` | Always uses CLI — diff data is too large and variable to cache. |
| No `.git` directory in `--dir` | Git tools return errors. No cache is built. |

### xray_git_history

Get commit history for a specific file. Returns commit hash, date, author, email, and message. Uses in-memory cache when available (sub-millisecond), falls back to `git log` CLI (~2–6 sec).

```json
// Request
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"xray_git_history","arguments":{"repo":".","file":"src/main.rs","maxResults":3}}}

// Response (abbreviated)
{
  "commits": [
    {"hash":"abc123...","date":"2025-01-15 10:30:00 +0000","author":"Alice","email":"alice@example.com","message":"Fix null check in main"}
  ],
  "summary": {"totalCommits":1,"returned":1,"file":"src/main.rs","elapsedMs":0.15,"hint":"(from cache)","tool":"xray_git_history"}
}
```

### xray_git_diff

Get commit history with full diff/patch for a specific file. Same as `xray_git_history` but includes added/removed lines for each commit. Patches are truncated to ~200 lines per commit to manage output size.

> **Note:** Always uses CLI (`git log -p`) — never uses the in-memory cache, because diff data is too large and variable to cache.

```json
// Request
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"xray_git_diff","arguments":{"repo":".","file":"src/main.rs","maxResults":2}}}

// Response (abbreviated)
{
  "commits": [
    {
      "hash":"abc123...","date":"2025-01-15 10:30:00 +0000","author":"Alice","email":"alice@example.com","message":"Fix null check in main",
      "patch":"--- a/src/main.rs\n+++ b/src/main.rs\n@@ -10,3 +10,4 @@\n+    if value.is_none() { return; }\n"
    }
  ],
  "summary": {"totalCommits":1,"returned":1,"file":"src/main.rs","elapsedMs":1250.5,"tool":"xray_git_diff"}
}
```

### xray_git_authors

Get top authors for a file, directory, or entire repository ranked by number of commits. Shows who changed the code the most, with commit count and date range of their changes.

The `path` parameter (or its backward-compatible alias `file`) accepts:
- **File path** — authors for a single file (e.g., `"path": "src/main.rs"`)
- **Directory path** — authors across all files in a directory (e.g., `"path": "src/controllers"`)
- **Omitted** — authors across the entire repository

```json
// Request — single file
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"xray_git_authors","arguments":{"repo":".","path":"src/main.rs","top":3}}}

// Request — directory
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"xray_git_authors","arguments":{"repo":".","path":"src/controllers","top":5}}}

// Request — entire repo
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"xray_git_authors","arguments":{"repo":".","top":10}}}

// Response (abbreviated)
{
  "authors": [
    {"rank":1,"name":"Alice","email":"alice@example.com","commits":42,"firstChange":"2024-03-01","lastChange":"2025-01-15"},
    {"rank":2,"name":"Bob","email":"bob@example.com","commits":17,"firstChange":"2024-06-10","lastChange":"2024-12-20"},
    {"rank":3,"name":"Carol","email":"carol@example.com","commits":5,"firstChange":"2024-09-05","lastChange":"2024-11-30"}
  ],
  "summary": {"totalCommits":64,"totalAuthors":3,"returned":3,"path":"src/main.rs","elapsedMs":0.08,"hint":"(from cache)","tool":"xray_git_authors"}
}
```

### xray_git_activity

Get activity across files in a repository for a date range. Two grouping modes:

- **`groupBy=file`** (default): Returns files with commit counts, authors, and commit subjects. Best for "which files changed?"
- **`groupBy=commit`**: Returns commits with their changed files. Best for "what was developed this week?" — ideal for sprint summaries, release notes, team activity reports

The optional `path` parameter filters activity to a specific file or directory:
- **File path** — activity for a single file (e.g., `"path": "src/main.rs"`)
- **Directory path** — activity across all files in a directory (e.g., `"path": "src/controllers"`)
- **Omitted** — activity across the entire repository

#### Additional Parameters

| Parameter | Type | Default | Description |
|---|---|---|---|
| `groupBy` | string | `"file"` | Grouping mode: `"file"` = file→commits with subjects, `"commit"` = commit→files |
| `maxResults` | integer | — | For `groupBy=commit`: max commits (default: 50). For `groupBy=file`: unlimited |
| `maxFilesPerCommit` | integer | 20 | Max files per commit when `groupBy=commit`. Excess truncated with `truncatedFiles` count |

#### groupBy=file (default)

```json
// Request — whole repo, file grouping
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"xray_git_activity","arguments":{"repo":".","from":"2025-01-01","to":"2025-01-31"}}}

// Response (abbreviated — from cache)
{
  "activity": [
    {"path":"src/main.rs","commitCount":12,"lastModified":"2025-01-31 18:45:00 +0000","authors":["Alice <alice@example.com>","Bob <bob@example.com>"],"subjects":["Add caching layer","Fix retry logic","Refactor auth"]},
    {"path":"src/lib.rs","commitCount":8,"lastModified":"2025-01-28 10:20:00 +0000","authors":["Alice <alice@example.com>"],"subjects":["Add caching layer","Extract utils"]},
    {"path":"Cargo.toml","commitCount":3,"lastModified":"2025-01-15 09:00:00 +0000","authors":["Carol <carol@example.com>"],"subjects":["Bump dependencies"]}
  ],
  "summary": {"filesChanged":3,"totalEntries":23,"commitsProcessed":150,"elapsedMs":0.12,"hint":"(from cache)","tool":"xray_git_activity"}
}
```

#### groupBy=commit

```json
// Request — commit grouping ("what was developed this week?")
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"xray_git_activity","arguments":{"repo":".","groupBy":"commit","from":"2025-01-20","to":"2025-01-26"}}}

// Response (abbreviated — from cache)
{
  "commits": [
    {"hash":"abc123def456","subject":"Add caching layer for user queries","author":"Alice <alice@example.com>","date":"2025-01-26 14:30:00 +0000","filesChanged":5,"files":["src/cache.rs","src/lib.rs","src/main.rs","src/services/user.rs","tests/cache_tests.rs"]},
    {"hash":"def789abc012","subject":"Fix retry logic in HTTP client","author":"Bob <bob@example.com>","date":"2025-01-25 10:15:00 +0000","filesChanged":2,"files":["src/http.rs","tests/http_tests.rs"]}
  ],
  "summary": {"groupBy":"commit","totalCommits":2,"returned":2,"elapsedMs":0.08,"hint":"(from cache, grouped by commit)","tool":"xray_git_activity"}
}
```

When a commit touches more than `maxFilesPerCommit` files, the `files` array is truncated and a `truncatedFiles` field shows how many files were omitted.

### xray_git_blame

Get line-level attribution for a file or line range via `git blame`. Returns the commit hash, author, date, and source content for each line. Always uses CLI (`git blame --porcelain`).

#### Parameters

| Parameter   | Type    | Required | Description |
|---|---|---|---|
| `repo`      | string  | ✅ | Path to local git repository |
| `file`      | string  | ✅ | File path relative to repo root |
| `startLine` | integer | ✅ | First line to blame (1-based, inclusive) |
| `endLine`   | integer | — | Last line to blame (1-based, inclusive). If omitted, only `startLine` |

```json
// Request — blame lines 10-15 of UserService.cs
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"xray_git_blame","arguments":{"repo":".","file":"src/UserService.cs","startLine":10,"endLine":15}}}

// Response (abbreviated)
{
  "blame": [
    {"line":10,"hash":"a1b2c3d4","author":"Alice","email":"alice@example.com","date":"2025-01-10 14:30:00 +0000","content":"    public async Task<User> GetUserAsync(int id)"},
    {"line":11,"hash":"a1b2c3d4","author":"Alice","email":"alice@example.com","date":"2025-01-10 14:30:00 +0000","content":"    {"},
    {"line":12,"hash":"d4e5f6a7","author":"Bob","email":"bob@example.com","date":"2025-01-12 09:15:00 +0000","content":"        var user = await _repository.FindAsync(id);"},
    {"line":13,"hash":"d4e5f6a7","author":"Bob","email":"bob@example.com","date":"2025-01-12 09:15:00 +0000","content":"        if (user == null) throw new NotFoundException(id);"},
    {"line":14,"hash":"a1b2c3d4","author":"Alice","email":"alice@example.com","date":"2025-01-10 14:30:00 +0000","content":"        return user;"},
    {"line":15,"hash":"a1b2c3d4","author":"Alice","email":"alice@example.com","date":"2025-01-10 14:30:00 +0000","content":"    }"}
  ],
  "summary": {"tool":"xray_git_blame","file":"src/UserService.cs","lineRange":"10-15","uniqueAuthors":2,"uniqueCommits":2,"oldestLine":"2025-01-10","newestLine":"2025-01-12","elapsedMs":45.3}
}
```

---

## `xray_branch_status` — Branch Status

Shows whether you're on the right branch before investigating production bugs. Reports branch name, behind/ahead of remote main, uncommitted changes, and how fresh the last fetch is.

```json
// Request
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"xray_branch_status","arguments":{"repo":"."}}}

// Response
{
  "currentBranch": "users/dev/my-feature",
  "isMainBranch": false,
  "mainBranch": "master",
  "behindMain": 47,
  "aheadOfMain": 3,
  "dirtyFiles": ["src/SomeFile.cs", "src/Other.cs"],
  "dirtyFileCount": 2,
  "lastFetchTime": "2025-06-15 10:30:00 +0000",
  "fetchAge": "3 hours ago",
  "fetchWarning": null,
  "warning": "Index is built on 'users/dev/my-feature', not on master. Local branch is 47 commits behind remote master.",
  "summary": { "tool": "xray_branch_status", "elapsedMs": 45.2 }
}
```

### Fetch age warning thresholds

| Time since last fetch | Warning |
|---|---|
| < 1 hour | `null` (no warning) |
| 1–24 hours | `"Last fetch: 6 hours ago"` |
| 1–7 days | `"Last fetch: 3 days ago. Remote data may be outdated."` |
| > 7 days | `"Last fetch: 12 days ago! Recommend: git fetch origin"` |

---

## File Not Found Warning

When `xray_git_history`, `xray_git_authors`, or `xray_git_activity` return 0 results and the specified file doesn't exist in git, the response includes a `"warning"` field:

```json
{
  "commits": [],
  "summary": { "totalCommits": 0, "tool": "xray_git_history" },
  "warning": "File not found in git: path/to/file.cs. Check the path."
}
```

This helps distinguish between "no commits in the date range" and "wrong file path". The warning works in both cache and CLI fallback paths. When the file exists but simply has no matching commits, no warning is added.

---

## Branch Warning

When the MCP server is started on a branch other than `main` or `master`, all index-based tool responses (`xray_grep`, `xray_definitions`, `xray_callers`, `xray_fast`) include a `branchWarning` field in the `summary` object:

```json
{
  "summary": {
    "totalFiles": 42,
    "branchWarning": "Index is built on branch 'users/dev/my-feature', not on main/master. Results may differ from production."
  }
}
```

This warning is **absent** when:
- The current branch is `main` or `master`
- The indexed directory is not a git repository
- The `git rev-parse` command fails (e.g., git not installed)

The branch is detected **once at server startup** via `git rev-parse --abbrev-ref HEAD`. Git tools (`xray_git_history`, `xray_git_diff`, etc.) do **not** include this warning because they query the git repository directly and are not affected by which branch the index was built on.

---

## Manual Testing (without AI)

```bash
xray serve --dir . --ext rs --definitions
# Then paste JSON-RPC messages to stdin:
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"xray_grep","arguments":{"terms":"tokenize"}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"xray_callers","arguments":{"method":"ExecuteQueryAsync","depth":3}}}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_definitions","arguments":{"file":"QueryService.cs","containsLine":812}}}
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"xray_definitions","arguments":{"name":"GetProductEntriesAsync","includeBody":true,"maxBodyLines":10}}}
{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"xray_git_history","arguments":{"repo":".","file":"Cargo.toml","maxResults":5}}}
```
