# MCP Server Guide

Complete guide for the `search-index serve` MCP server — setup, tools API, and examples.

## Overview

The MCP server starts its event loop **immediately** and responds to `initialize` / `tools/list` without waiting for indexes to build. If a pre-built index exists on disk, it is loaded synchronously (< 3s). Otherwise, indexes are built in a background thread — search tools return a friendly "Index is being built, please retry" message until ready. This eliminates startup timeouts when Roo/VS Code launches the server for the first time.

## Setup in VS Code

1. **Install search** (if not already):

   ```bash
   cargo install --path .
   # Or copy search-index.exe to a folder in your PATH
   ```

2. **Build a content index** for your project:

   ```bash
   search-index content-index -d C:\Projects\MyApp -e cs,sql,csproj
   ```

3. **Create `.vscode/mcp.json`** in your workspace root:

   ```json
   {
     "servers": {
       "search-index": {
         "command": "C:\\Users\\you\\.cargo\\bin\\search-index.exe",
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

4. **Restart VS Code** — the MCP server starts automatically. Your MCP-compatible AI agent (Roo Code, Cline, etc.) now has access to all MCP tools. The server also sends an `instructions` field during MCP initialization with best practices for tool selection.

5. **Verify** — ask the AI: _"Use search_grep to find all files containing HttpClient"_

## Exposed Tools

| Tool                         | Description                                                                                                                             |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `search_grep`                | Search content index with TF-IDF ranking, regex, phrase, AND/OR                                                                         |
| `search_definitions`         | Search code definitions (classes, methods, interfaces, etc.). Supports `containsLine`, `includeBody`, `audit`. Relevance-ranked when name filter is active. Requires `--definitions` |
| `search_callers`             | Find callers / callees and build recursive call tree. Requires `--definitions`                                                          |
| `search_find`                | Live filesystem walk (⚠️ slow for large dirs)                                                                                           |
| `search_fast`                | Search pre-built file name index (instant). Supports comma-separated OR patterns. Results ranked: exact stem → prefix → contains        |
| `search_info`                | Show all indexes with status, sizes, age                                                                                                |
| `search_reindex`             | Force rebuild + reload content index                                                                                                    |
| `search_reindex_definitions` | Force rebuild + reload definition index. Requires `--definitions`                                                                       |
| `search_help`                | Best practices guide, strategy recipes, performance tiers                                                                               |
| `search_git_history`         | Commit history for a file. Uses in-memory cache when available (sub-millisecond), falls back to CLI (~2–6 sec)                          |
| `search_git_diff`            | Commit history with full diff/patch. Always uses CLI (cache has no patch data)                                                          |
| `search_git_authors`         | Top authors for a file ranked by commit count. Uses in-memory cache when available (sub-millisecond), falls back to CLI                  |
| `search_git_activity`        | Repo-wide activity (all changed files) for a date range. Uses in-memory cache when available (sub-millisecond), falls back to CLI        |
| `search_git_blame`           | Line-level attribution (`git blame`) for a file or line range. Returns commit hash, author, date, and content per line                   |
| `search_branch_status`       | Shows current git branch status: branch name, main/master check, behind/ahead counts, dirty files, fetch age. Call before investigating production bugs |

## What the AI Agent Sees

When the AI connects, it discovers tools with full JSON schemas. Each tool has a detailed description with required/optional parameters and examples.

Example interaction:

```
AI:  "Let me search for HttpClient in your codebase..."
     → calls search_grep { terms: "HttpClient", maxResults: 10 }
     ← receives JSON with file paths, scores, line numbers
AI:  "Found 1,082 files. The most relevant is CustomHttpClient.cs (score: 0.49)..."
```

---

## `search_grep` — Content Search

Search content index with TF-IDF ranking. Supports multi-term (AND/OR), regex, phrase, and substring search.

Substring search is **on by default** in MCP mode — compound identifiers like `IUserService`, `m_userService`, `UserServiceFactory` are automatically found when searching for `UserService`. Auto-disabled when `regex` or `phrase` is used. Use `"substring": false` for exact-token-only matching.

See [CLI Reference — `search-index grep`](cli-reference.md#search-grep--search-inverted-content-index) for full parameter details.

---

## `search_callers` — Call Tree

Traces who calls a method (or what a method calls) and builds a hierarchical call tree. Combines the content index (grep) with the definition index (AST) to determine which method/class contains each call site. Replaces 7+ sequential `search_grep` + `read_file` calls with a single request. Supports C#, TypeScript/TSX, and SQL (call sites from stored procedure bodies: EXEC, FROM, JOIN, INSERT, UPDATE, DELETE).

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
| `method` (required)  | Method name to trace                                                                                                                                |
| `class`              | Scope to a specific class. DI-aware: `class: "UserService"` also finds callers using `IUserService`. Works for both `"up"` and `"down"` directions. |
| `direction`          | `"up"` = find callers (default), `"down"` = find callees                                                                                            |
| `depth`              | Max recursion depth (default: 3, max: 10)                                                                                                           |
| `maxCallersPerLevel` | Max callers per node (default: 10). Prevents explosion.                                                                                             |
| `maxTotalNodes`      | Max total nodes in tree (default: 200). Caps output size.                                                                                           |
| `excludeDir`         | Directory substrings to exclude, e.g. `["\\test\\", "\\Mock\\"]`                                                                                    |
| `excludeFile`        | File path substrings to exclude                                                                                                                     |
| `resolveInterfaces`  | Auto-resolve interface → implementation (default: true)                                                                                             |
| `ext`                | File extension filter (default: server's `--ext`)                                                                                                   |

### Limitations

- **Interface vs concrete class name** — when searching for callers, always use the **interface name** (e.g., `class: "IUserService"`) rather than the concrete class name (e.g., `class: "UserService"`). Calls through DI use the interface type as the receiver. Searching with the concrete class returns 0 callers if all call sites use the interface. Alternatively, set `resolveInterfaces: true` to auto-resolve implementations.

- **Local variable calls not tracked** — calls through local variables (e.g., `var x = service.GetFoo(); x.Bar()`) may not be detected because the tool uses AST parsing without type inference. DI-injected fields, `this`/`base` calls, and direct receiver calls are fully supported.

---

## `search_definitions` — Code Definitions

Search code definitions: classes, methods, interfaces, enums, functions, type aliases, stored procedures. Requires `--definitions`.

Results are **relevance-ranked** when a `name` filter is active (non-regex): exact matches first, then prefix matches, then substring matches. Within the same match tier, type-level definitions (classes, interfaces, enums) sort before members (methods, properties), and shorter names before longer. See [Architecture — Relevance Ranking](architecture.md#relevance-ranking) for details.

### Parameters

| Parameter           | Type    | Default | Description                                                                              |
| ------------------- | ------- | ------- | ---------------------------------------------------------------------------------------- |
| `name`              | string  | —       | Substring or comma-separated OR search                                                   |
| `kind`              | string  | —       | Filter by definition kind (class, method, property, function, typeAlias, variable, etc.) |
| `attribute`         | string  | —       | Filter by C# attribute or TypeScript decorator                                           |
| `baseType`          | string  | —       | Filter by base type/interface                                                            |
| `file`              | string  | —       | Filter by file path substring                                                            |
| `parent`            | string  | —       | Filter by parent class name                                                              |
| `containsLine`      | integer | —       | Find definition containing a line number (requires `file`)                               |
| `regex`             | boolean | false   | Treat `name` as regex                                                                    |
| `maxResults`        | integer | 100     | Max results returned                                                                     |
| `excludeDir`        | array   | —       | Exclude directories                                                                      |
| `includeBody`       | boolean | false   | Include source code body inline                                                          |
| `maxBodyLines`      | integer | 100     | Max lines per definition body (0 = unlimited)                                            |
| `maxTotalBodyLines` | integer | 500     | Max total body lines across all results (0 = unlimited)                                  |
| `audit`             | boolean | false   | Return index coverage report instead of search results                                   |
| `auditMinBytes`     | integer | 500     | Min file size to flag as suspicious in audit mode                                        |
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

### `includeBody` — Return Source Code Inline

Retrieve the actual source code of definitions without a separate `read_file` call. Three-level protection prevents response explosion:

- **`maxBodyLines`** — caps lines per individual definition (default: 100, 0 = unlimited)
- **`maxTotalBodyLines`** — caps total body lines across all results (default: 500, 0 = unlimited)
- **`maxResults`** — caps the number of definitions returned (default: 100)

When a definition's body exceeds `maxBodyLines`, the `body` array is truncated and `bodyTruncated: true` is set. When the global `maxTotalBodyLines` budget is exhausted, remaining definitions receive `bodyOmitted: true` with a `bodyWarning` message. If the source file cannot be read, `bodyError` is returned instead.

```json
// Request
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "search_definitions",
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

**Note:** Classes, fields, and enum members do not have `codeStats`. Old indexes (before this feature) return results normally with `summary.codeStatsAvailable: false` — run `search_reindex_definitions` to compute metrics.

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

---

---

## Git History Tools

Six MCP tools for querying git history. Always available — no flags needed. When the in-memory git history cache is ready (built automatically in the background on server startup), `search_git_history`, `search_git_authors`, and `search_git_activity` use sub-millisecond cache lookups. When the cache is not ready (first ~60 sec on cold start), these tools transparently fall back to CLI `git log` commands (~2–6 sec). `search_git_diff` and `search_git_blame` always use CLI.

Cache responses include a `"(from cache)"` hint in the `summary` field so the AI agent knows the data source.

### Parameters (shared across git tools)

| Parameter    | Type   | Required | Description |
|---|---|---|---|
| `repo`       | string | ✅ | Path to local git repository |
| `file`       | string | ✅* | File path relative to repo root (*required for `search_git_history`, `search_git_diff`, `search_git_blame`) |
| `path`       | string | — | File or directory path relative to repo root. `search_git_authors` accepts `path` (file, directory, or omit for entire repo). `file` is a backward-compatible alias for `path` |
| `from`       | string | — | Start date (YYYY-MM-DD, inclusive) |
| `to`         | string | — | End date (YYYY-MM-DD, inclusive) |
| `date`       | string | — | Exact date (YYYY-MM-DD), overrides from/to |
| `maxResults` | number | — | Maximum results to return (default: 50) |
| `top`        | number | — | Maximum authors to return (default: 10, `search_git_authors` only) |
| `author`     | string | — | Filter by author name or email (case-insensitive substring match). Available on `search_git_history`, `search_git_diff`, `search_git_activity` |
| `message`    | string | — | Filter by commit message (case-insensitive substring match). Available on `search_git_history`, `search_git_diff`, `search_git_activity`, `search_git_authors` |
| `noCache`    | boolean | — | If true, bypass the in-memory git history cache and query git CLI directly. Useful when cache may be stale. Available on `search_git_history`, `search_git_authors`, `search_git_activity` |

### Cache behavior

| Scenario | Behavior |
|---|---|
| Server just started, no `.git-history` on disk | Cache builds in background (~59 sec). Tools use CLI fallback during build. |
| Server restart, `.git-history` exists on disk | Cache loads from disk (~100 ms). Tools use cache almost immediately. |
| HEAD changed since cache was built | Cache rebuilds in background. Old cache (if loaded from disk) serves queries during rebuild. |
| `search_git_diff` | Always uses CLI — diff data is too large and variable to cache. |
| No `.git` directory in `--dir` | Git tools return errors. No cache is built. |

### search_git_history

Get commit history for a specific file. Returns commit hash, date, author, email, and message. Uses in-memory cache when available (sub-millisecond), falls back to `git log` CLI (~2–6 sec).

```json
// Request
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_git_history","arguments":{"repo":".","file":"src/main.rs","maxResults":3}}}

// Response (abbreviated)
{
  "commits": [
    {"hash":"abc123...","date":"2025-01-15 10:30:00 +0000","author":"Alice","email":"alice@example.com","message":"Fix null check in main"}
  ],
  "summary": {"totalCommits":1,"returned":1,"tool":"search_git_history"}
}
```

### search_git_diff

Get commit history with full diff/patch for a specific file. Same as `search_git_history` but includes added/removed lines for each commit. Patches are truncated to ~200 lines per commit to manage output size.

> **Note:** Always uses CLI (`git log -p`) — never uses the in-memory cache, because diff data is too large and variable to cache.

```json
// Request
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_git_diff","arguments":{"repo":".","file":"src/main.rs","maxResults":2}}}

// Response (abbreviated)
{
  "commits": [
    {
      "hash":"abc123...","date":"2025-01-15 10:30:00 +0000","author":"Alice","email":"alice@example.com","message":"Fix null check in main",
      "patch":"--- a/src/main.rs\n+++ b/src/main.rs\n@@ -10,3 +10,4 @@\n+    if value.is_none() { return; }\n"
    }
  ],
  "summary": {"totalCommits":1,"returned":1,"tool":"search_git_diff"}
}
```

### search_git_authors

Get top authors for a file, directory, or entire repository ranked by number of commits. Shows who changed the code the most, with commit count and date range of their changes.

The `path` parameter (or its backward-compatible alias `file`) accepts:
- **File path** — authors for a single file (e.g., `"path": "src/main.rs"`)
- **Directory path** — authors across all files in a directory (e.g., `"path": "src/controllers"`)
- **Omitted** — authors across the entire repository

```json
// Request — single file
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_git_authors","arguments":{"repo":".","path":"src/main.rs","top":3}}}

// Request — directory
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_git_authors","arguments":{"repo":".","path":"src/controllers","top":5}}}

// Request — entire repo
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_git_authors","arguments":{"repo":".","top":10}}}

// Response (abbreviated)
{
  "authors": [
    {"rank":1,"name":"Alice","email":"alice@example.com","commits":42,"firstChange":"2024-03-01","lastChange":"2025-01-15"},
    {"rank":2,"name":"Bob","email":"bob@example.com","commits":17,"firstChange":"2024-06-10","lastChange":"2024-12-20"},
    {"rank":3,"name":"Carol","email":"carol@example.com","commits":5,"firstChange":"2024-09-05","lastChange":"2024-11-30"}
  ],
  "summary": {"totalAuthors":3,"returned":3,"tool":"search_git_authors"}
}
```

### search_git_activity

Get activity across all files in a repository for a date range. Returns a list of changed files with their commit counts. Useful for answering "what changed this week?" Date filters are recommended to keep results manageable.

```json
// Request
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_git_activity","arguments":{"repo":".","from":"2025-01-01","to":"2025-01-31"}}}

// Response (abbreviated)
{
  "activity": [
    {"path":"src/main.rs","commitCount":12},
    {"path":"src/lib.rs","commitCount":8},
    {"path":"Cargo.toml","commitCount":3}
  ],
  "summary": {"totalFiles":3,"totalCommits":23,"tool":"search_git_activity"}
}
```

### search_git_blame

Get line-level attribution for a file or line range via `git blame`. Returns the commit hash, author, date, and source content for each line. Always uses CLI (`git blame --porcelain`).

#### Parameters

| Parameter   | Type    | Required | Description |
|---|---|---|---|
| `repo`      | string  | ✅ | Path to local git repository |
| `file`      | string  | ✅ | File path relative to repo root |
| `startLine` | integer | — | First line to blame (1-based, default: 1) |
| `endLine`   | integer | — | Last line to blame (1-based, default: end of file) |

```json
// Request — blame lines 10-15 of UserService.cs
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_git_blame","arguments":{"repo":".","file":"src/UserService.cs","startLine":10,"endLine":15}}}

// Response (abbreviated)
{
  "lines": [
    {"line":10,"hash":"a1b2c3d4","author":"Alice","email":"alice@example.com","date":"2025-01-10 14:30:00 +0000","content":"    public async Task<User> GetUserAsync(int id)"},
    {"line":11,"hash":"a1b2c3d4","author":"Alice","email":"alice@example.com","date":"2025-01-10 14:30:00 +0000","content":"    {"},
    {"line":12,"hash":"d4e5f6a7","author":"Bob","email":"bob@example.com","date":"2025-01-12 09:15:00 +0000","content":"        var user = await _repository.FindAsync(id);"},
    {"line":13,"hash":"d4e5f6a7","author":"Bob","email":"bob@example.com","date":"2025-01-12 09:15:00 +0000","content":"        if (user == null) throw new NotFoundException(id);"},
    {"line":14,"hash":"a1b2c3d4","author":"Alice","email":"alice@example.com","date":"2025-01-10 14:30:00 +0000","content":"        return user;"},
    {"line":15,"hash":"a1b2c3d4","author":"Alice","email":"alice@example.com","date":"2025-01-10 14:30:00 +0000","content":"    }"}
  ],
  "summary": {"totalLines":6,"file":"src/UserService.cs","startLine":10,"endLine":15,"tool":"search_git_blame"}
}
```

---

## `search_branch_status` — Branch Status

Shows whether you're on the right branch before investigating production bugs. Reports branch name, behind/ahead of remote main, uncommitted changes, and how fresh the last fetch is.

```json
// Request
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_branch_status","arguments":{"repo":"."}}}

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
  "summary": { "tool": "search_branch_status", "elapsedMs": 45.2 }
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

When `search_git_history`, `search_git_authors`, or `search_git_activity` return 0 results and the specified file doesn't exist in git, the response includes a `"warning"` field:

```json
{
  "commits": [],
  "summary": { "totalCommits": 0, "tool": "search_git_history" },
  "warning": "File not found in git: path/to/file.cs. Check the path."
}
```

This helps distinguish between "no commits in the date range" and "wrong file path". The warning works in both cache and CLI fallback paths. When the file exists but simply has no matching commits, no warning is added.

---

## Branch Warning

When the MCP server is started on a branch other than `main` or `master`, all index-based tool responses (`search_grep`, `search_definitions`, `search_callers`, `search_fast`) include a `branchWarning` field in the `summary` object:

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

The branch is detected **once at server startup** via `git rev-parse --abbrev-ref HEAD`. Git tools (`search_git_history`, `search_git_diff`, etc.) do **not** include this warning because they query the git repository directly and are not affected by which branch the index was built on.

---

## Manual Testing (without AI)

```bash
search-index serve --dir . --ext rs --definitions
# Then paste JSON-RPC messages to stdin:
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"tokenize"}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_callers","arguments":{"method":"ExecuteQueryAsync","depth":3}}}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_definitions","arguments":{"file":"QueryService.cs","containsLine":812}}}
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"search_definitions","arguments":{"name":"GetProductEntriesAsync","includeBody":true,"maxBodyLines":10}}}
{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"search_git_history","arguments":{"repo":".","file":"Cargo.toml","maxResults":5}}}
```
