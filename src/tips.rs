//! Single source of truth for best practices and tips.
//! Used by: CLI `search tips`, MCP `search_help` tool, MCP `instructions` field.

use serde_json::{json, Value};

/// A single best practice tip.
pub struct Tip {
    pub rule: &'static str,
    pub why: &'static str,
    pub example: &'static str,
}

/// Performance tier description.
pub struct PerfTier {
    pub name: &'static str,
    pub range: &'static str,
    pub operations: &'static [&'static str],
}

/// Tool priority entry.
pub struct ToolPriority {
    pub rank: u8,
    pub tool: &'static str,
    pub description: &'static str,
}

/// A strategy recipe for a common task pattern.
pub struct Strategy {
    pub name: &'static str,
    pub when: &'static str,
    pub steps: &'static [&'static str],
    pub anti_patterns: &'static [&'static str],
}

// ─── Single source of truth ─────────────────────────────────────────

pub fn tips() -> Vec<Tip> {
    vec![
        Tip {
            rule: "File lookup: use search_fast, not search_find",
            why: "search_fast uses a pre-built index (~35ms). search_find does a live filesystem walk (~3s). 90x+ faster.",
            example: "search_fast with pattern='UserService' instead of search_find",
        },
        Tip {
            rule: "Multi-term OR: find all variants in ONE query",
            why: "Comma-separated terms with mode='or' finds files containing ANY term. Much faster than separate queries.",
            example: "search-index grep \"UserService,IUserService,UserServiceFactory\" -e cs  |  MCP: terms='...', mode='or'",
        },
        Tip {
            rule: "AND mode: find files containing ALL terms",
            why: "mode='and' finds files where ALL comma-separated terms co-occur. Useful for finding DI registrations.",
            example: "search-index grep \"ServiceProvider,IUserService\" -e cs --all  |  MCP: terms='...', mode='and'",
        },
        Tip {
            rule: "Substring search is ON by default",
            why: "search_grep defaults to substring=true so compound identifiers (IUserService, m_userService) are always found. Use substring=false for exact-token-only matching. Auto-disabled when regex or phrase is used. Short tokens (<4 chars) may produce broad results -- use exclude=['pattern'] to filter noise from file paths. Comma-separated phrases with spaces are searched independently as OR (or AND with mode='and').",
            example: "Default: terms='UserService' finds IUserService, m_userService. Multi-phrase OR: terms='fn handle_foo,fn build_bar'. Exact only: terms='UserService', substring=false",
        },
        Tip {
            rule: "Phrase search: exact multi-word match",
            why: "phrase=true finds exact adjacent word sequences. Slower (~80ms) but precise.",
            example: "search-index grep \"new HttpClient\" -e cs --phrase  |  MCP: terms='new HttpClient', phrase=true",
        },
        Tip {
            rule: "Regex pattern search",
            why: "Full regex for pattern matching. Also works in search_definitions name parameter.",
            example: "search-index grep \"I[A-Z]\\w+Cache\" -e cs --regex  |  MCP: terms='I[A-Z]\\w+Cache', regex=true",
        },
        Tip {
            rule: "Exclude test/mock dirs for production-only results",
            why: "Half the results are often test files. Use excludeDir to filter them out.",
            example: "--exclude-dir test --exclude-dir Mock  |  MCP: excludeDir=['test','Mock','UnitTests']",
        },
        Tip {
            rule: "Call chain tracing: search_callers (up and down)",
            why: "Single sub-millisecond request replaces 7+ sequential grep + read_file calls. direction='up' (callers) or 'down' (callees). For Angular: direction='down' with class shows template children, direction='up' with selector finds parent components.",
            example: "MCP: search_callers method='GetUserAsync', class='UserService', depth=2, direction='up'",
        },
        Tip {
            rule: "Always specify class in search_callers",
            why: "Without class, results mix callers from ALL classes with same method name. Misleading call trees.",
            example: "MCP: search_callers method='ExecuteAsync', class='OrderProcessor'",
        },
        Tip {
            rule: "search_callers 0 results? Try the interface name",
            why: "Calls through DI use the interface (IUserService), not the implementation (UserService). search_callers matches the receiver type recorded at call sites. If you search class='UserService' but callers use IUserService, you get 0 results.",
            example: "0 callers with class='UserService' -> retry with class='IUserService'. Also: resolveInterfaces=true auto-resolves implementations",
        },
        Tip {
            rule: "Stack trace analysis: containsLine",
            why: "Given file + line number, returns innermost method AND parent class. No manual read_file needed.",
            example: "MCP: search_definitions file='UserService.cs', containsLine=42",
        },
        Tip {
            rule: "Read method source: use includeBody=true instead of reading files",
            why: "search_definitions with includeBody=true returns method body inline, eliminating read_file round-trips. BEFORE reading any .cs/.ts file, try search_definitions with includeBody=true first. Use maxBodyLines/maxTotalBodyLines for budget. Only read files directly for non-C#/TS content (markdown, JSON, XML) or when you need exact line numbers for editing.",
            example: "MCP: search_definitions parent='UserService', includeBody=true, maxBodyLines=20. Also: search_definitions name='Program,Startup,OrderService' includeBody=true -> reads multiple classes at once, faster than multiple read_file calls",
        },
        Tip {
            rule: "Body budgets: 0 means unlimited",
            why: "Default limits: 100 lines/def, 500 total. Set maxBodyLines=0, maxTotalBodyLines=0 for unlimited output.",
            example: "MCP: search_definitions parent='UserService', includeBody=true, maxBodyLines=0, maxTotalBodyLines=0",
        },
        Tip {
            rule: "Reconnaissance: use countOnly=true",
            why: "search_grep with countOnly=true returns ~46 tokens (counts only) vs 265+ for full results. Perfect for 'how many files use X?'.",
            example: "search-index grep \"HttpClient\" -e cs --count-only  |  MCP: terms='HttpClient', countOnly=true",
        },
        Tip {
            rule: "Search ANY indexed file type: XML, csproj, config, etc.",
            why: "search_grep works with all file extensions passed to --ext. Use ext='csproj' to find NuGet dependencies, ext='xml,config,manifestxml' for configuration values.",
            example: "search-index grep \"Newtonsoft.Json\" -e csproj  |  MCP: terms='Newtonsoft.Json', ext='csproj'",
        },
        Tip {
            rule: "Language scope: content search = any language, AST = C#, TypeScript/TSX, and SQL",
            why: "search_grep / content-index use a language-agnostic tokenizer -- works with any text file (C#, Rust, Python, JS, XML, etc.). search_definitions / def-index supports C# and TypeScript/TSX (tree-sitter) and SQL (regex-based). search_callers uses call-graph analysis -- supports C#, TypeScript/TSX (DI-aware, inject() support, interface resolution), and SQL (SP-to-SP EXEC call chains). SQL class parameter = schema name (dbo, Sales). Tables/views excluded from call graph (data, not code).",
            example: "search-index grep works on -e rs,py,js,xml,json | search_definitions supports .cs, .ts, .tsx, .sql | search_callers supports .cs, .ts, .tsx, .sql (SP EXEC call chains, class=schema)",
        },
        Tip {
            rule: "Response truncation: large results are auto-capped at ~16KB",
            why: "Broad queries (short substring, common tokens) can return thousands of files. The server auto-truncates responses to ~16KB (~4K tokens) to avoid filling LLM context. summary.totalFiles always shows the FULL count. Use countOnly=true or narrow with dir/ext/exclude to get focused results.",
            example: "If responseTruncated=true appears, narrow your query: add ext, dir, excludeDir, or use countOnly=true. Server flag --max-response-kb adjusts the limit (0=unlimited).",
        },
        Tip {
            rule: "Code health: find complex methods with includeCodeStats/sortBy/min*",
            why: "Instant code quality scan across entire codebase. sortBy='cognitiveComplexity' ranks worst methods first. Combine min* filters (AND logic) to find God Methods. Only methods/functions/constructors have stats.",
            example: "search_definitions sortBy='cognitiveComplexity' maxResults=20  |  search_definitions minComplexity=10 minParams=5 sortBy='cyclomaticComplexity'",
        },
        Tip {
            rule: "Multi-term name in search_definitions: find ALL types in ONE call",
            why: "The name parameter accepts comma-separated terms (OR logic). Find a class + its interface + related types in a single query instead of 3 separate calls.",
            example: "search_definitions name='UserService,IUserService,UserController' -> finds ALL matching definitions in one call",
        },
        Tip {
            rule: "Query budget: aim for 3 or fewer search calls per exploration task",
            why: "Each search call adds latency and LLM context. Use multi-term queries, includeBody, and combined filters to minimize round-trips. Most architecture questions can be answered in 1-3 calls.",
            example: "Step 1: search_definitions name='OrderService,IOrderService' includeBody=true (map + read). Step 2: search_callers method='ProcessOrder' class='OrderService' (call chain). Done in 2 calls.",
        },
        Tip {
            rule: "Check branch status before investigating production bugs",
            why: "Call search_branch_status first to verify you're on the right branch and your data is up-to-date. Avoids wasted investigation on stale or wrong-branch data.",
            example: "MCP: search_branch_status repo='.' -> shows branch, behind/ahead counts, fetch age, dirty files",
        },
        Tip {
            rule: "Use noCache=true when git results seem stale",
            why: "search_git_history/authors/activity use an in-memory cache for speed. If results seem outdated after recent commits, use noCache=true to bypass cache and query git CLI directly.",
            example: "MCP: search_git_history repo='.', file='src/main.rs', noCache=true",
        },
        Tip {
            rule: "Method group/delegate references: use search_grep, not search_callers",
            why: "search_callers only finds direct method invocations (obj.Method(args)). It does NOT detect methods passed as delegates or method groups (e.g., list.Where(IsValid), Func<bool> f = service.Check). Use search_grep to find all textual references including delegate usage.",
            example: "search_callers misses delegate usage -> use search_grep terms='IsValid' ext='cs' to find all references including method group passes",
        },
        Tip {
            rule: "Impact analysis: find which tests cover a method",
            why: "impactAnalysis=true in search_callers traces callers upward and identifies test methods (via [Test]/[Fact]/[Theory]/[TestMethod]/#[test] attributes or *.spec.ts/*.test.ts files). Returns a 'testsCovering' array with all tests in the call chain. Use depth=5-7 for deep call chains. One call replaces manual multi-step investigation.",
            example: "MCP: search_callers method='SaveOrder' class='OrderService' direction='up' depth=5 impactAnalysis=true -> testsCovering: [{method: 'TestSaveOrder', class: 'OrderTests', file: 'src/OrderTests.cs', depth: 1, callChain: ['SaveOrder', 'TestSaveOrder']}]",
        },
    ]
}

pub fn strategies() -> Vec<Strategy> {
    vec![
        Strategy {
            name: "Architecture Exploration",
            when: "User asks 'how is X structured', 'explain module X', or 'show me the architecture of X'",
            steps: &[
                "Step 1 - Map the landscape (1 call): search_definitions name='X' maxResults=50 includeBody=false -> lists ALL classes, interfaces, enums, methods in one shot",
                "Step 2 - Read key implementations (1 call): search_definitions name='<top 3-5 key classes from step 1>' includeBody=true maxBodyLines=30 -> returns source code of the most important files",
                "Step 3 (optional) - Scope and dependencies (1 call): search_grep terms='X' countOnly=true -> scale (how many files, occurrences); or search_fast pattern='X' dirsOnly=true -> directory structure",
            ],
            anti_patterns: &[
                "Don't use list_files + read_file to explore architecture -- search_definitions returns classes, methods, file paths, and source code in ONE call",
                "Don't read .cs/.ts files to see source code -- search_definitions includeBody=true returns it directly. read_file is ONLY for non-indexed files (markdown, JSON, XML, config) or for editing (need exact line numbers)",
                "Don't search one kind at a time (class, then interface, then enum) -- omit kind filter to get everything at once",
                "Don't use countOnly first then re-query with body -- go straight to includeBody=true with maxBodyLines",
                "Don't search for file names separately if search_definitions already found them (results include file paths)",
                "Don't make separate queries for ClassName and IClassName -- use comma-separated: name='ClassName,IClassName'",
            ],
        },
        Strategy {
            name: "Call Chain Investigation",
            when: "User asks 'who calls X', 'trace how X is invoked', or 'show the call chain for X'",
            steps: &[
                "Step 1 - Get call tree with source (1 call): search_callers method='MethodName' class='ClassName' depth=3 direction='up' includeBody=true -> full caller hierarchy WITH method source code inline",
                "Step 2 (optional, only if more context needed): search_definitions name='<specific callers from step 1>' includeBody=true maxBodyLines=0 -> full source of specific methods",
            ],
            anti_patterns: &[
                "Don't omit the class parameter -- without it, results mix callers from ALL classes with the same method name",
                "Don't use search_grep to manually find callers -- search_callers does it in sub-millisecond with DI/interface resolution",
                "Don't call search_callers then search_definitions separately to get source -- use includeBody=true in search_callers to get both in ONE call",
            ],
        },
        Strategy {
            name: "Stack Trace / Bug Investigation",
            when: "User provides a stack trace, error at file:line, or asks 'what method is at line N'",
            steps: &[
                "Step 1 - Identify method (1 call): search_definitions file='FileName.cs' containsLine=42 includeBody=true -> returns the method + its parent class with source code",
                "Step 2 (optional) - Trace callers (1 call): search_callers method='<method from step 1>' class='<class from step 1>' depth=2 -> who triggered this code path",
            ],
            anti_patterns: &[
                "Don't use read_file to manually scan for the method -- containsLine finds it instantly with proper class context",
                "Don't guess the method name from the stack trace -- use containsLine for precise AST-based lookup",
            ],
        },
        Strategy {
            name: "Code History Investigation",
            when: "User asks 'when was this bug introduced', 'who changed this file', or 'trace the origin of this code'",
            steps: &[
                "Step 1 - Verify branch (1 call): search_branch_status repo='.' -> confirm you're on main and data is fresh",
                "Step 2 - Find where code lives (1 call): search_grep terms='<error text>' ext='cs' -> find file and line number",
                "Step 3 - Find who introduced it (1 call): search_git_blame repo='.', file='<file from step 2>', startLine=<line> -> exact commit, author, date",
                "Step 4 (optional) - Full history (1 call): search_git_history repo='.', file='<file from step 2>' -> all commits for context",
                "Step 5 (optional) - File ownership (1 call): search_git_authors repo='.', path='<file>' -> who maintains this file",
            ],
            anti_patterns: &[
                "Don't skip search_branch_status -- investigating on the wrong branch wastes time",
                "Don't use search_git_history to find WHEN a specific string appeared -- use search_grep + search_git_blame instead (faster and more precise)",
            ],
        },
        Strategy {
            name: "Angular Component Hierarchy (TypeScript only)",
            when: "User asks 'who uses <component>', 'where is it embedded', or 'show parent components' (TypeScript/Angular projects only)",
            steps: &[
                "Step 1 (1 call): search_callers method='<selector>' direction='up' depth=3 -> finds parent AND grandparent components recursively via templateChildren (<1ms). Parents nested in 'parents' field.",
                "Step 2 (optional): search_callers method='<class-name>' direction='down' depth=2 -> shows child components used in template (recursive)",
            ],
            anti_patterns: &[
                "Don't use search_grep to find component selectors in HTML -- search_callers resolves template relationships from the AST index in <1ms",
                "Don't forget to use the component selector (e.g. 'app-header'), not the class name, when searching direction='up'",
            ],
        },
        Strategy {
            name: "Code Health Scan",
            when: "User asks 'find complex methods', 'code quality', 'refactoring candidates', or 'technical debt'",
            steps: &[
                "Step 1 - Top offenders (1 call): search_definitions sortBy='cognitiveComplexity' maxResults=20 -> worst 20 methods by cognitive complexity",
                "Step 2 (optional) - Narrow to module (1 call): search_definitions file='Services' minComplexity=10 sortBy='cyclomaticComplexity' -> complex methods in specific directory",
                "Step 3 (optional) - God Method detection (1 call): search_definitions minComplexity=20 minParams=5 minCalls=15 -> methods that are too large, have too many params, and high fan-out",
            ],
            anti_patterns: &[
                "Don't read every file to count if/else -- sortBy computes metrics from the AST index in <1ms",
                "Don't forget includeCodeStats is auto-enabled by sortBy and min* -- no need to pass it explicitly",
                "Don't use sortBy on old indexes without code stats -- run search_reindex_definitions first",
            ],
        },
    ]
}

pub fn performance_tiers() -> Vec<PerfTier> {
    vec![
        PerfTier {
            name: "Instant",
            range: "<1ms",
            operations: &["search_grep (substring default)", "search_callers", "search_definitions baseType/attribute"],
        },
        PerfTier {
            name: "Fast",
            range: "1-10ms",
            operations: &["search_grep showLines", "search_definitions containsLine"],
        },
        PerfTier {
            name: "Quick",
            range: "10-100ms",
            operations: &["search_fast", "search_definitions name/parent/includeBody", "search_grep regex/phrase"],
        },
        PerfTier {
            name: "Slow",
            range: ">1s",
            operations: &["search_find (live filesystem walk - avoid!)"],
        },
    ]
}

pub fn tool_priority() -> Vec<ToolPriority> {
    vec![
        ToolPriority { rank: 1, tool: "search_callers", description: "call trees up/down (<1ms, C#, TypeScript/TSX, and SQL)" },
        ToolPriority { rank: 2, tool: "search_definitions", description: "structural: classes, methods, functions, interfaces, typeAliases, variables, containsLine (C#, TypeScript/TSX, SQL)" },
        ToolPriority { rank: 3, tool: "search_grep", description: "content: exact/OR/AND, substring, phrase, regex (any language)" },
        ToolPriority { rank: 4, tool: "search_fast", description: "file name lookup (~35ms, any file)" },
        ToolPriority { rank: 5, tool: "search_find", description: "live walk (~3s, last resort)" },
        ToolPriority { rank: 6, tool: "search_branch_status", description: "call first when investigating production bugs" },
        ToolPriority { rank: 7, tool: "search_edit", description: "reliable file editing -- line-range or text-match, atomic, no whitespace issues. Supports multi-file (paths), insert after/before, expectedContext" },
    ]
}

// ─── Parameter examples (moved from inline tool descriptions) ───────

/// Examples for tool parameters, organized by tool.
/// Displayed via search_help so LLMs can look up usage patterns on demand,
/// without consuming tokens on every turn in the system prompt.
pub fn parameter_examples() -> Value {
    json!({
        "search_definitions": {
            "name": "Single: 'UserService'. Multi-term OR: 'UserService,IUserService,UserController' (finds ALL in one query). Naming variants: 'Order,IOrder,OrderFactory'. NOTE: name searches AST definition names (classes, methods, properties) -- NOT string literals or values inside code. Use search_grep for string content",
            "containsLine": "file='UserService.cs', containsLine=42 -> returns GetUserAsync (lines 35-50), parent: UserService",
            "includeBody": "parent='UserService', includeBody=true, maxBodyLines=20 -> returns method bodies inline",
            "includeDocComments": "includeDocComments=true -> expands body upward to capture /// XML doc-comments (C#/Rust) or /** */ JSDoc (TypeScript) above the definition. Implies includeBody=true. Response includes 'docCommentLines' count. Budget-aware: counts against maxBodyLines",
            "sortBy": "sortBy='cognitiveComplexity' maxResults=20 -> 20 most complex methods. sortBy='lines' -> longest definitions",
            "attribute": "'ApiController', 'Authorize', 'ServiceProvider'",
            "baseType": "'ControllerBase', 'IUserService' -> finds classes implementing IUserService",
            "file": "Single: 'UserService.cs'. Multi-term OR: 'UserService.cs,OrderService.cs,Controller.cs' (finds defs in ANY matching file). Substring match on file path",
            "parent": "Single: 'UserService'. Multi-term OR: 'UserService,OrderService,DataProcessor' (finds members of ANY matching class). Substring match on parent name",
            "regex": "name='I.*Cache' with regex=true -> all types matching pattern",
            "kind": "C# kinds: class, interface, method, property, field, enum, struct, record, constructor, delegate, event, enumMember. TypeScript kinds: function, typeAlias, variable (plus shared: class, interface, method, field, enum, constructor, enumMember). NOTE: In TypeScript, class members (e.g. 'private name: string') are kind='field', while interface signatures (e.g. 'readonly id: string' in an interface) are kind='property'. If kind='property' returns 0 results for a TS class, try kind='field'. SQL kinds: storedProcedure, sqlFunction, table, view, userDefinedType, sqlIndex, column. SQL call sites extracted from SP bodies: EXEC, FROM, JOIN, INSERT, UPDATE, DELETE",
            "includeCodeStats": "Each method gets: lines, cyclomaticComplexity, cognitiveComplexity, maxNestingDepth, paramCount, returnCount, callCount, lambdaCount",
            "audit": "Shows: total files, files with/without definitions, read errors, lossy UTF-8, suspicious files (large files with 0 definitions)",
            "angular": "Angular @Component classes include 'selector' and 'templateChildren' in output, showing which child components are used in the template"
        },
        "search_grep": {
            "terms": "Token: 'HttpClient'. Multi-term OR: 'HttpClient,ILogger,Task'. Multi-term AND (mode='and'): 'ServiceProvider,IUserService'. Phrase (phrase=true): 'new HttpClient'. Regex (regex=true): 'I.*Cache'",
            "contextLines": "contextLines=5 shows 5 lines before and 5 lines after each match (like grep -C)",
            "showLines": "Returns groups of consecutive lines with startLine, lines array, and matchIndices",
            "ext": "'cs', 'cs,sql', 'xml,config' (comma-separated for multiple)",
            "substring": "Default: terms='UserService' finds IUserService, m_userService. Set substring=false for exact-token-only"
        },
        "search_callers": {
            "class": "'UserService' -> DI-aware: also finds callers using IUserService. SQL: class = schema name (e.g., class='dbo', class='Sales'). Without class, results mix callers from ALL classes/schemas with same method name",
            "method": "'GetUserAsync'. Angular/TS only: pass a selector (e.g. 'app-header') as method with direction='up' to find parent components that embed it via templateChildren. Returns templateUsage: true for template-based relationships",
            "direction": "'up' = who calls this (callers, default). 'down' = what this calls (callees). Angular/TS only: 'down' with class name shows child components from HTML template (recursive with depth). 'up' with selector (e.g. 'app-header') finds parent components recursively — depth=3 traverses grandparents, great-grandparents etc. Parents nested in 'parents' field",
            "resolveInterfaces": "When tracing callers of IFoo.Bar(), also finds callers of FooImpl.Bar() where FooImpl implements IFoo",
            "includeBody": "includeBody=true -> each node in call tree includes 'body' (source lines) and 'bodyStartLine'. Also adds a top-level 'rootMethod' object with the searched method's own body. Eliminates the need for a separate search_definitions call. Default: false",
            "includeDocComments": "includeDocComments=true -> expands each method body to include doc-comments above it. Implies includeBody=true. Adds 'docCommentLines' field. Default: false",
            "maxBodyLines": "Max source lines per method (default: 30, 0=unlimited). Controls per-method body size when includeBody=true",
            "maxTotalBodyLines": "Max total body lines across all methods in tree (default: 300, 0=unlimited). When exceeded, remaining methods get 'bodyOmitted' instead of body",
            "angular": "TypeScript/Angular only: method='app-header' direction='up' -> finds parent components embedding <app-header> via templateChildren (templateUsage: true). method='processOrder' class='OrderFormComponent' direction='down' -> shows child components used in template",
            "impactAnalysis": "impactAnalysis=true with direction='up' -> identifies test methods covering the target. Response includes 'testsCovering' array (with full file path, depth, and callChain for each test) and isTest=true on test nodes. callChain shows the method-by-method path from target to test — use it to assess relevance (short chain = direct test, long chain = transitive via helpers). Tests detected via: C# [Test]/[Fact]/[Theory]/[TestMethod], Rust #[test], TS *.spec.ts/*.test.ts files. Use depth=5-7 for deep chains.",
            "limitation": "Only finds direct invocations (obj.Method(args)). Does NOT find method group/delegate references (e.g., list.Where(IsValid), Func<bool> f = svc.Check). Use search_grep for those."
        },
        "search_fast": {
            "pattern": "Single: 'UserService'. Multi-term OR: 'UserService,OrderProcessor' finds files matching ANY term"
        },
        "search_git_history": {
            "repo": "'.' (current directory) or absolute path to git repo",
            "file": "File path relative to repo root: 'src/main.rs', 'Services/UserService.cs'",
            "from": "'2025-01-01' (YYYY-MM-DD, inclusive start date)",
            "to": "'2025-01-31' (YYYY-MM-DD, inclusive end date)",
            "date": "'2025-01-15' — overrides from/to for single-day filter",
            "maxResults": "50 (default). 0 = unlimited. Use with date filters for large repos",
            "author": "'john', 'john@example.com' (case-insensitive substring)",
            "message": "'fix bug', 'PR 12345', '[GI]' (case-insensitive substring)",
            "noCache": "true -> bypass in-memory cache, query git CLI directly"
        },
        "search_git_diff": {
            "note": "Same params as search_git_history (except noCache — always uses CLI). Includes 'patch' field with diff lines per commit"
        },
        "search_git_authors": {
            "path": "'src/main.rs' (file), 'src/controllers' (directory), or omit for entire repo. 'file' is backward-compatible alias",
            "top": "10 (default). Max authors to return",
            "from": "'2025-01-01' — narrow to date range",
            "message": "'feature' — filter commits by message substring",
            "noCache": "true -> bypass cache"
        },
        "search_git_activity": {
            "from": "'2025-01-01' — RECOMMENDED to narrow results. Without date filter, returns ALL repo activity",
            "author": "'alice' — filter by author",
            "message": "'refactor' — filter by commit message"
        },
        "search_git_blame": {
            "file": "'src/UserService.cs' — file path relative to repo root",
            "startLine": "10 (1-based, required). Start of line range",
            "endLine": "20 (1-based, optional). If omitted, only startLine is blamed"
        },
        "search_branch_status": {
            "repo": "'.' — shows branch name, behind/ahead counts, dirty files, fetch age. Call FIRST when investigating production bugs"
        },
        "search_edit": {
            "path": "File path — absolute or relative to server --dir. Works on any text file, not limited to indexed extensions. Mutually exclusive with 'paths'",
            "paths": "Array of file paths for multi-file editing. Same edits/operations applied to ALL files. Transactional: if any file fails, none are written. Max 20 files. Response has 'results' array + 'summary'. Example: paths=['file1.cs', 'file2.cs', 'file3.cs']",
            "operations": "Mode A (line-range): [{startLine: 5, endLine: 5, content: 'new line'}] — replace line 5. [{startLine: 3, endLine: 2, content: 'inserted'}] — insert before line 3 (endLine < startLine). [{startLine: 2, endLine: 4, content: ''}] — delete lines 2-4. Append to end: [{startLine: N+1, endLine: N, content: 'appended'}] where N = line count. Multiple ops applied bottom-up (no offset cascade)",
            "edits": "Mode B (text-match): [{search: 'old', replace: 'new'}] — replace all. [{search: 'old', replace: 'new', occurrence: 2}] — 2nd only. Insert after: [{insertAfter: 'using X;', content: 'using Y;'}]. Insert before: [{insertBefore: 'class Foo', content: '// comment'}]",
            "insertAfter_insertBefore": "Insert content on the line after/before an anchor text. Mutually exclusive with search/replace. Requires 'content' field. Use 'occurrence' to target Nth match (default: first). Example: {insertAfter: 'using System.IO;', content: 'using System.Linq;'}",
            "expectedContext": "Per-edit safety check: verify this text exists within ±5 lines of the match before applying. Example: {search: 'SemaphoreSlim(10)', replace: 'SemaphoreSlim(30)', expectedContext: 'var semaphore = new'}",
            "skipIfNotFound": "Per-edit flag: if true, silently skip when search/anchor text is not found (default: false). Essential for multi-file 'paths' where not all files contain the target text. Without it, one missing file aborts the entire batch",
            "regex": "true -> treat edit search strings as regex with $1, $2 capture groups (Mode B search/replace only)",
            "dryRun": "true -> preview unified diff without writing file. Works with both single and multi-file",
            "expectedLineCount": "Safety check: abort if file has different line count (prevents stale line numbers)"
        }
    })
}

// ─── Renderers ──────────────────────────────────────────────────────

/// Render tips as human-readable CLI output.
pub fn render_cli() -> String {
    let mut out = String::new();
    out.push_str("\nsearch-index -- Best Practices & Tips\n");
    out.push_str("===============================\n\n");

    out.push_str("BEST PRACTICES\n");
    out.push_str("--------------\n");
    for (i, tip) in tips().iter().enumerate() {
        out.push_str(&format!("{:2}. {}\n", i + 1, tip.rule));
        out.push_str(&format!("    Why: {}\n", tip.why));
        out.push_str(&format!("    Example: {}\n\n", tip.example));
    }

    out.push_str("STRATEGY RECIPES\n");
    out.push_str("----------------\n");
    for strat in strategies() {
        out.push_str(&format!("  [{}]\n", strat.name));
        out.push_str(&format!("  When: {}\n", strat.when));
        for step in strat.steps {
            out.push_str(&format!("    - {}\n", step));
        }
        out.push_str("  Anti-patterns:\n");
        for ap in strat.anti_patterns {
            out.push_str(&format!("    X {}\n", ap));
        }
        out.push('\n');
    }

    out.push_str("PERFORMANCE TIERS\n");
    out.push_str("-----------------\n");
    for tier in performance_tiers() {
        out.push_str(&format!("  {:>6}  {}\n", tier.range, tier.operations.join(", ")));
    }
    out.push('\n');

    out.push_str("TOOL PRIORITY (MCP)\n");
    out.push_str("-------------------\n");
    for tp in tool_priority() {
        out.push_str(&format!("  {}. {:20} - {}\n", tp.rank, tp.tool, tp.description));
    }
    out.push('\n');

    out
}

/// Render tips as JSON for MCP search_help tool.
pub fn render_json() -> Value {
    let best_practices: Vec<Value> = tips().iter().map(|t| {
        json!({
            "rule": t.rule,
            "why": t.why,
            "example": t.example,
        })
    }).collect();

    let strategy_recipes: Vec<Value> = strategies().iter().map(|s| {
        json!({
            "name": s.name,
            "when": s.when,
            "steps": s.steps,
            "antiPatterns": s.anti_patterns,
        })
    }).collect();

    let mut tiers = serde_json::Map::new();
    for tier in performance_tiers() {
        let key = format!("{}_{}", tier.name.to_lowercase(), tier.range.replace(['<', '>', ' '], ""));
        tiers.insert(key, json!(tier.operations));
    }

    let priority: Vec<Value> = tool_priority().iter().map(|tp| {
        json!(format!("{}. {} — {}", tp.rank, tp.tool, tp.description))
    }).collect();

    json!({
        "bestPractices": best_practices,
        "strategyRecipes": strategy_recipes,
        "performanceTiers": tiers,
        "toolPriority": priority,
        "parameterExamples": parameter_examples(),
    })
}

/// Render tips as compact text for MCP initialize instructions field.
///
/// Design principles (see docs/TODO-hint-optimization.md):
/// - Machine-targeted: no emoji, use ALL CAPS for emphasis
/// - Client-agnostic: no Roo/Cline-specific tool names (read_file, list_files)
/// - Compact: 5 key bullets + strategy recipes + tool priority (not all 18 tips)
/// - Full tips available via search_help tool
///
/// `def_extensions` — the file extensions that have definition parser support
/// (intersection of server --ext and definition_extensions()). Used to dynamically
/// generate the "NEVER READ" instruction so it covers exactly the right file types.
pub fn render_instructions(def_extensions: &[&str]) -> String {
    let mut out = String::new();

    // --- PREFER block (Phase 0: client-agnostic, no emoji, CAPS emphasis) ---
    out.push_str("CRITICAL: ALWAYS use search-index tools for code exploration. They are 90-1000x faster than file browsing.\n");
    out.push_str("   search_definitions -- classes, methods, source code, file paths in ONE call. DO NOT browse directories then read files.\n");
    out.push_str("   search_callers -- full call trees in <1ms. DO NOT search for callers manually.\n");
    out.push_str("   search_grep -- content search across 100K+ files instantly. DO NOT use regex-based file search.\n");
    out.push_str("   search_fast -- file name lookup in ~35ms. DO NOT walk the filesystem.\n\n");

    // --- FILE READING RULE (strongest possible: NEVER + decision trigger + batch split) ---
    // Dynamically generate from the actually-indexed definition extensions.
    // When no definition-supported extensions are configured (e.g., --ext xml),
    // skip the NEVER READ block entirely to avoid misleading instructions.
    if !def_extensions.is_empty() {
        let ext_dotted: Vec<String> = def_extensions.iter().map(|e| format!(".{}", e)).collect();
        let ext_list = ext_dotted.join("/");
        out.push_str(&format!("NEVER READ {} FILES DIRECTLY. ALWAYS use search_definitions includeBody=true instead.\n", ext_list));
        out.push_str("   DECISION TRIGGER: before ANY file read, check each file's extension.\n");
        out.push_str(&format!("   If the file is {} -> use search_definitions name='ClassName' includeBody=true maxBodyLines=30 (or file='path' containsLine=N includeBody=true). Use maxBodyLines to control output size for large classes.\n", ext_list));
        out.push_str("   If the file is .md, .json, .xml, .config, .csproj, or other non-definition-indexed -> reading directly is OK.\n");
        out.push_str(&format!("   BATCH SPLIT: if you need both {} and .md files, make TWO calls: search_definitions for indexed files, direct read for .md files. Do NOT batch them into one direct read.\n", ext_list));
        out.push_str(&format!("   ONLY exceptions for {}: (1) editing (need exact line numbers for diffs), (2) search_definitions returned an error or is unavailable.\n\n", ext_list));
    } else {
        out.push_str("NOTE: search_definitions is not available for the configured file extensions. Use search_grep for content search.\n\n");
    }

    // --- Quick reference (Phase 1: 5 bullets instead of 18 full tips) ---
    out.push_str("search-index MCP server -- Quick Reference\n\n");
    out.push_str("1. USE search_definitions for code exploration -- returns classes, methods, bodies, file paths in ONE call. Supports containsLine for stack traces, includeBody for source code.\n");
    out.push_str("2. USE search_callers for call chains -- sub-millisecond full call tree. ALWAYS specify class parameter.\n");
    out.push_str("3. USE search_grep for content search -- substring ON by default, multi-term OR with commas, countOnly for reconnaissance.\n");
    out.push_str("4. USE search_fast for file lookup -- 90x faster than search_find.\n");
    out.push_str("5. AIM for <=3 search calls per task. Call search_help for full guide with examples.\n");
    out.push_str("6. USE sortBy/min* in search_definitions for code health scans -- sortBy='cognitiveComplexity' ranks worst methods first.\n");
    out.push_str("7. USE search_edit for file editing -- atomic line-range or text-match edits, no whitespace issues. PREFERRED over apply_diff.\n");

    // --- Strategy recipes (kept unchanged -- highest-value content) ---
    out.push_str("\nSTRATEGY RECIPES (aim for <=3 search calls per task):\n");
    for strat in strategies() {
        out.push_str(&format!("  [{}] {}\n", strat.name, strat.when));
        for step in strat.steps {
            out.push_str(&format!("    - {}\n", step));
        }
    }

    // --- Tool priority (kept unchanged) ---
    out.push_str("\nTOOL PRIORITY:\n");
    for tp in tool_priority() {
        // Use ASCII -- for CLI compatibility instead of em-dash
        out.push_str(&format!("  {}. {} -- {}\n", tp.rank, tp.tool, tp.description));
    }

    // --- Git tools (brief mention) ---
    out.push_str("\nGit tools: search_git_history, search_git_authors, search_git_activity, search_git_blame, search_branch_status -- use for code history/blame/authorship investigations. Call search_help for details.\n");

    // --- Soft reference to search_help (Phase 4: no urgency) ---
    out.push_str("\nCall search_help for detailed best practices with examples.\n");

    out
}

#[cfg(test)]
#[path = "tips_tests.rs"]
mod tests;
