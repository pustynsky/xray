# CLI Reference

Complete reference for all `search` CLI commands.

## `search-index find` — Live Filesystem Search

Walks the filesystem in real-time. No index needed.

```bash
# Search for files by name
search-index find "config" -d C:\Projects

# Search with extension filter
search-index find "main" -e rs

# Search file contents
search-index find "TODO" -d C:\Projects --contents -e cs

# Regex search in file contents
search-index find "fn\s+\w+" --contents --regex -e rs

# Case-insensitive search
search-index find "readme" -i -d C:\

# Count matches only
search-index find ".exe" -d C:\Windows -c

# Limit search depth
search-index find "node_modules" -d C:\Projects --max-depth 3

# Include hidden and gitignored files
search-index find "secret" --hidden --no-ignore
```

**Options:**

| Flag                | Description                           |
| ------------------- | ------------------------------------- |
| `-d, --dir <DIR>`   | Root directory (default: `.`)         |
| `-r, --regex`       | Treat pattern as regex                |
| `--contents`        | Search file contents instead of names |
| `--hidden`          | Include hidden files                  |
| `--max-depth <N>`   | Max directory depth (0 = unlimited)   |
| `-t, --threads <N>` | Thread count (0 = auto)               |
| `-i, --ignore-case` | Case-insensitive search               |
| `--no-ignore`       | Include `.gitignore`d files           |
| `-c, --count`       | Show match count only                 |
| `-e, --ext <EXT>`   | Filter by file extension              |

---

## `search-index index` — Build File Name Index

Pre-builds an index of all file paths for instant lookups.

```bash
# Index a directory
search-index index -d C:\Projects

# Index with custom max age (hours)
search-index index -d C:\ --max-age-hours 48

# Include hidden and gitignored files
search-index index -d C:\Projects --hidden --no-ignore
```

**Options:**

| Flag                  | Description                                          |
| --------------------- | ---------------------------------------------------- |
| `-d, --dir <DIR>`     | Directory to index (default: `.`)                    |
| `--max-age-hours <N>` | Hours before index is considered stale (default: 24) |
| `--hidden`            | Include hidden files                                 |
| `--no-ignore`         | Include `.gitignore`d files                          |
| `-t, --threads <N>`   | Thread count (0 = auto)                              |

---

## `search-index fast` — Search File Name Index

Searches a pre-built file name index. Instant results. Supports comma-separated patterns for multi-file lookup (OR logic).

```bash
# Search by file name (substring match)
search-index fast "notepad" -d C:\Windows

# With extension filter
search-index fast "notepad" -d C:\Windows -e exe --files-only

# Comma-separated multi-term search (OR logic) — find multiple files at once
search-index fast "UserService,OrderProcessor,PaymentHandler" -d C:\Projects -e cs

# Regex search
search-index fast "config\.\w+" -d C:\Projects --regex

# Find large files (> 100MB)
search-index fast "" -d C:\ --min-size 104857600

# Find directories only
search-index fast "node_modules" -d C:\Projects --dirs-only

# Count only
search-index fast ".dll" -d C:\Windows -c
```

If no index exists for the directory, it will be built automatically on first use.

**Options:**

| Flag                 | Description                                    |
| -------------------- | ---------------------------------------------- |
| `-d, --dir <DIR>`    | Directory whose index to search (default: `.`) |
| `-r, --regex`        | Treat pattern as regex                         |
| `-i, --ignore-case`  | Case-insensitive search                        |
| `-c, --count`        | Show match count only                          |
| `-e, --ext <EXT>`    | Filter by extension                            |
| `--auto-reindex`     | Auto-rebuild if stale (default: true)          |
| `--dirs-only`        | Show only directories                          |
| `--files-only`       | Show only files                                |
| `--min-size <BYTES>` | Minimum file size filter                       |
| `--max-size <BYTES>` | Maximum file size filter                       |

---

## `search-index content-index` — Build Inverted Content Index

Reads file contents, tokenizes them, and builds an inverted index mapping tokens to file locations. **The tokenizer is language-agnostic** — it works with any text file (C#, Rust, Python, JavaScript, TypeScript, XML, JSON, Markdown, config files, etc.). Specify the extensions you want to index with `-e`.

```bash
# Index C# files
search-index content-index -d C:\Projects -e cs

# Index multiple file types (any text files work)
search-index content-index -d C:\Projects -e cs,rs,py,js,ts

# Custom token minimum length
search-index content-index -d C:\Projects -e cs --min-token-len 3

# Include everything
search-index content-index -d C:\Projects -e cs --hidden --no-ignore
```

**Tokenization rules:**

- Text is split on non-alphanumeric characters (except `_`)
- All tokens are lowercased
- Tokens shorter than `--min-token-len` (default: 2) are discarded
- Example: `private readonly HttpClient _client;` → `["private", "readonly", "httpclient", "_client"]`

**Options:**

| Flag                  | Description                                      |
| --------------------- | ------------------------------------------------ |
| `-d, --dir <DIR>`     | Directory to index (default: `.`)                |
| `-e, --ext <EXTS>`    | File extensions, comma-separated (default: `cs`) |
| `--max-age-hours <N>` | Hours before stale (default: 24)                 |
| `--hidden`            | Include hidden files                             |
| `--no-ignore`         | Include `.gitignore`d files                      |
| `-t, --threads <N>`   | Thread count (0 = auto)                          |
| `--min-token-len <N>` | Minimum token length (default: 2)                |

---

## `search-index grep` — Search Inverted Content Index

Searches the inverted index for tokens. Results are ranked by TF-IDF score. Supports multi-term search (AND/OR) and regex pattern matching against indexed tokens.

```bash
# Search for a single term (results ranked by relevance)
search-index grep "HttpClient" -d C:\Projects

# Multi-term OR search (files containing ANY of the terms)
search-index grep "HttpClient,ILogger,Task" -d C:\Projects -e cs

# Multi-term AND search (files containing ALL terms)
search-index grep "HttpClient,ILogger" -d C:\Projects -e cs --all

# Regex: find all cache interfaces
search-index grep "i.*cache" -d C:\Projects -e cs --regex

# Regex: find all factory classes
search-index grep ".*factory" -d C:\Projects -e cs --regex --max-results 20

# Regex: find all async methods
search-index grep ".*async" -d C:\Projects -e cs --regex -c

# Show actual matching lines from files
search-index grep "HttpClient" -d C:\Projects --show-lines

# Top 10 results only
search-index grep "HttpClient" -d C:\Projects --max-results 10

# Count matches
search-index grep "HttpClient" -d C:\Projects -c

# Filter by extension
search-index grep "HttpClient" -d C:\Projects -e cs
```

### Multi-term search

- Separate terms with commas: `"term1,term2,term3"`
- **OR mode** (default): file matches if it contains **any** of the terms
- **AND mode** (`--all`): file matches only if it contains **all** terms
- TF-IDF scores are summed across matching terms — files matching more terms rank higher
- Output shows `X/N terms` indicating how many of the search terms were found in each file

### Substring search (default in both CLI and MCP)

- **Default in both CLI and MCP** — compound C# identifiers like `IUserService`, `m_userService`, `UserServiceFactory` are automatically found when searching for `UserService`. Auto-disabled when `--regex`, `--phrase`, or `--exact` is used.
- Uses a trigram index for fast matching (~1ms) — much faster than regex scanning (~12–44ms)
- Solves the compound-identifier problem: searching `DatabaseConnection` finds the token `databaseconnectionfactory` even though it's stored as a single token in the inverted index
- Results sorted by TF-IDF: exact matches rank highest, compound matches lower
- For queries shorter than 4 characters, a warning is included in the response (trigram matching is less selective for very short queries)
- Use `--exact` to disable substring matching and search for exact tokens only
- CLI example: `search-index grep "DatabaseConn" -d C:\Projects -e cs` (substring by default)
- CLI exact: `search-index grep "DatabaseConn" -d C:\Projects -e cs --exact` (exact tokens only)
- MCP example: `{ "terms": "DatabaseConn" }` (substring by default; use `"substring": false` for exact-token-only)

### Regex search (`-r, --regex`)

- Pattern is matched against all indexed tokens using Rust regex syntax
- Anchored with `^...$` automatically — matches full tokens
- Example: `"i.*cache"` → matches `itenantcache`, `iusercache`, `isessioncache`, etc.
- Multiple regex patterns via commas: `"i.*cache,.*factory"`
- Can combine with `--all` for AND across regex patterns
- Performance: scans 754K tokens in ~12ms, then instant posting lookups

### Options

| Flag                | Description                                                                                                                                                                                                                |
| ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `-d, --dir <DIR>`   | Directory whose content index to search (default: `.`)                                                                                                                                                                     |
| `-c, --count`       | Show match count only                                                                                                                                                                                                      |
| `--show-lines`      | Display actual line content from files                                                                                                                                                                                     |
| `--auto-reindex`    | Auto-rebuild if stale (default: true)                                                                                                                                                                                      |
| `-e, --ext <EXT>`   | Filter results by extension                                                                                                                                                                                                |
| `--max-results <N>` | Limit number of results (0 = unlimited)                                                                                                                                                                                    |
| `--all`             | AND mode: file must contain ALL terms (default: OR)                                                                                                                                                                        |
| `-r, --regex`       | Treat pattern as regex, match against indexed tokens                                                                                                                                                                       |
| `--exclude-dir <S>` | Exclude files with this substring in path (repeatable)                                                                                                                                                                     |
| `--exclude <S>`     | Exclude files matching this pattern in path (repeatable)                                                                                                                                                                   |
| `-C, --context <N>` | Show N context lines around matches (with --show-lines)                                                                                                                                                                    |
| `-B, --before <N>`  | Show N lines before each match (with --show-lines)                                                                                                                                                                         |
| `-A, --after <N>`   | Show N lines after each match (with --show-lines)                                                                                                                                                                          |
| `--phrase`          | Phrase search: find exact phrase via index + verification. When the phrase contains punctuation (e.g., `</Property>`), a post-filter verifies matching lines against the raw untokenized text to eliminate false positives |
| `--exact`           | Exact token matching only (disables default substring search)                                                                                                                                                              |

---

## `search-index info` — Index Information

Shows all existing indexes with their status.

```bash
search-index info
```

Example output:

```
Index directory: C:\Users\you\AppData\Local\search-index

  [FILE] C:\Windows — 333875 entries, 47.8 MB, 0.1h ago
  [CONTENT] C:\Projects — 48986 files, 33229888 tokens, exts: [cs, rs], 242.7 MB, 0.5h ago
  [GIT]  branch=main  commits=12345  files=2500  authors=42  HEAD=abc123de  1.2 MB  0.5 hours
```

---

## `search-index cleanup` — Remove Orphaned or Directory-Specific Indexes

Without `--dir`: scans the index directory and removes `.file-list`, `.word-search`, `.code-structure` files whose root directories no longer exist on disk.

With `--dir`: removes all index files whose root matches the specified directory (case-insensitive). Indexes for other directories are preserved.

```bash
# Remove orphaned indexes (root dirs that no longer exist)
search-index cleanup

# Remove all indexes for a specific directory
search-index cleanup --dir C:\Projects\MyApp

# Remove indexes for current directory (useful after E2E tests)
search-index cleanup --dir .
```

| Flag       | Description                                              |
| ---------- | -------------------------------------------------------- |
| `--dir`    | Remove indexes only for this directory (instead of orphaned cleanup) |

Example output (orphaned):

```
Scanning for orphaned indexes in C:\Users\you\AppData\Local\search-index...
  Removed orphaned index: Deleted_OldProject_abc12345.file-list (root: C:\Deleted\OldProject)
  Removed orphaned index: Temp_test_dir_12345_def45678.word-search (root: C:\Temp\test_dir_12345)
Removed 2 orphaned index file(s).
```

Example output (`--dir`):

```
Removing indexes for directory '.' from C:\Users\you\AppData\Local\search-index...
  Removed index for dir '.': Repos_MyApp_abc12345.file-list (file-list)
  Removed index for dir '.': Repos_MyApp_def45678.word-search (word-search)
  Removed index for dir '.': Repos_MyApp_ghi78901.code-structure (code-structure)
Removed 3 index file(s) for '.'.
```

---

## `search-index def-index` — Build Code Definition Index

Parses source files using tree-sitter (C#, TypeScript/TSX) or regex (SQL) to extract structural code definitions (classes, methods, interfaces, enums, stored procedures, tables, views, etc.). **Unlike the content index, this is language-specific** — supports C#, TypeScript/TSX, and SQL. See [Supported Languages](architecture.md#supported-languages) for details.

```bash
# Index C# files
search-index def-index --dir C:\Projects --ext cs

# Index TypeScript files
search-index def-index --dir C:\Projects --ext ts

# Index TypeScript + TSX files
search-index def-index --dir C:\Projects --ext ts,tsx

# Index C# + TypeScript together (mixed-language project)
search-index def-index --dir C:\Projects --ext cs,ts,tsx

# Custom thread count
search-index def-index --dir C:\Projects --ext cs --threads 8
```

**What it extracts:**

| Language               | Definition Types                                                                                                                |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| C# (.cs)               | classes, interfaces, structs, enums, records, methods, constructors, properties, fields, delegates, events, enum members        |
| TypeScript (.ts, .tsx) | classes, interfaces, enums, functions, type aliases, variables (const/let/var), methods, constructors, properties, enum members |
| SQL (.sql)             | stored procedures, tables, views, functions, user-defined types, indexes, columns, FK constraints (regex-based parser)          |

Each definition includes: name, kind, file path, line range, full signature, modifiers (public/static/async/etc.), attributes/decorators (`[ServiceProvider]`, `@Injectable()`, etc.), base types/interfaces, and parent class.

**TypeScript field type resolution** for call-site analysis supports three patterns:

- Constructor DI parameters: `constructor(private service: UserService)`
- Typed class fields: `private cache: CacheService;`
- Angular `inject()` function: `private store = inject(Store)` and `this.router = inject(Router)` (generic type params like `Store<AppState>` are stripped to base name)

**Performance:**

| Metric                | Value                           |
| --------------------- | ------------------------------- |
| ~48,600 files         | ~16-32s (varies by CPU/threads) |
| Definitions extracted | ~846,000                        |
| Call sites extracted  | ~2.4M                           |
| Index size            | ~324 MB                         |

**Options:**

| Flag                | Description                                     |
| ------------------- | ----------------------------------------------- |
| `-d, --dir <DIR>`   | Directory to scan recursively (default: `.`)    |
| `-e, --ext <EXTS>`  | Extensions to parse (default: `cs,sql`)         |
| `-t, --threads <N>` | Parallel parsing threads, 0 = auto (default: 0) |

---

## `search def-audit` — Audit Definition Index Coverage

Loads a previously built `.code-structure` file from disk (instant, no rebuild) and reports which files have 0 definitions. Files >500 bytes with 0 definitions are flagged as "suspicious" — possible parse failures.

```bash
# Show all suspicious files (>500B, 0 definitions)
search def-audit --dir C:\Projects --ext cs

# Only flag files >2KB as suspicious
search def-audit --dir C:\Projects --ext cs --min-bytes 2000
```

**Example output:**

```
[def-audit] Index: 48730 total files, 48177 with definitions, 553 without definitions
[def-audit] 854865 definitions, 0 read errors, 44 lossy-UTF8 files
[def-audit] 390 suspicious files (>500B with 0 definitions):
  C:\...\GlobalSuppressions.cs (2312 bytes)
  C:\...\AssemblyInfo.cs (2122 bytes)
  ...
```

> **Note:** Most "suspicious" files are legitimate — `AssemblyInfo.cs` and `GlobalSuppressions.cs` contain assembly-level attributes that the parser doesn't extract. Use `--min-bytes` to raise the threshold.

---

## `search-index serve` — Start MCP Server

Starts a Model Context Protocol (MCP) server over stdio. See [MCP Server Guide](mcp-guide.md) for full documentation on setup, tools API, and examples.

```bash
# Start MCP server for C# files
search-index serve --dir C:\Projects --ext cs

# With file watching and code definitions
search-index serve --dir C:\Projects --ext cs --watch --definitions

# Mixed C# + TypeScript project
search-index serve --dir C:\Projects --ext cs,ts,tsx --watch --definitions

# Mixed C# + SQL project
search-index serve --dir C:\Projects --ext cs,sql --watch --definitions
```

**Options:**

| Flag                   | Description                                                          |
| ---------------------- | -------------------------------------------------------------------- |
| `-d, --dir <DIR>`      | Directory to index and serve (default: `.`)                          |
| `-e, --ext <EXTS>`     | File extensions, comma-separated (default: `cs`)                     |
| `--watch`              | Watch for file changes and update indexes incrementally              |
| `--definitions`        | Load (or build on first use) code definition index (tree-sitter AST) |
| `--metrics`            | Add `responseBytes` and `estimatedTokens` to every tool response     |
| `--debounce-ms <MS>`   | Debounce delay for file watcher (default: 500)                       |
| `--bulk-threshold <N>` | File changes triggering full reindex (default: 100)                  |
| `--log-level <LEVEL>`  | Log level: error, warn, info, debug (default: info)                  |
| `--max-response-kb <N>`| Max response size in KB before truncation, 0 = unlimited (default: 16)|
| `--debug-log`          | Write MCP request/response traces and memory diagnostics to `.debug.log` in index dir|

---

## `search-index tips` — Best Practices Guide

Prints the same best practices and strategy recipes available via the `search_help` MCP tool. Includes step-by-step patterns for common tasks (architecture exploration, call chain investigation, stack trace analysis) with a target of ≤3 search calls per task.
