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
            why: "search_grep defaults to substring=true so compound identifiers (IUserService, m_userService) are always found. Use substring=false for exact-token-only matching. Auto-disabled when regex or phrase is used. Short tokens (<4 chars) may produce broad results -- use exclude=['pattern'] to filter noise from file paths.",
            example: "Default: terms='UserService' finds IUserService, m_userService. Noisy short token: terms='dsp_' exclude=['ODSP','test']. Exact only: terms='UserService', substring=false",
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
            why: "search_grep / content-index use a language-agnostic tokenizer -- works with any text file (C#, Rust, Python, JS, XML, etc.). search_definitions / def-index supports C# and TypeScript/TSX (tree-sitter) and SQL (regex-based). search_callers uses call-graph analysis -- supports C# and TypeScript/TSX (DI-aware, inject() support, interface resolution). SQL call sites (EXEC, FROM, JOIN, INSERT, UPDATE, DELETE) are extracted from stored procedure bodies.",
            example: "search-index grep works on -e rs,py,js,xml,json | search_definitions supports .cs, .ts, .tsx, .sql | search_callers supports .cs, .ts, .tsx, .sql (SP call sites)",
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
                "Step 1 - Get call tree (1 call): search_callers method='MethodName' class='ClassName' depth=3 direction='up' -> full caller hierarchy",
                "Step 2 (optional) - Read caller source (1 call): search_definitions name='<top callers from step 1>' includeBody=true -> see what callers actually do",
            ],
            anti_patterns: &[
                "Don't omit the class parameter -- without it, results mix callers from ALL classes with the same method name",
                "Don't use search_grep to manually find callers -- search_callers does it in sub-millisecond with DI/interface resolution",
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
        ToolPriority { rank: 1, tool: "search_callers", description: "call trees up/down (<1ms, C# and TypeScript/TSX)" },
        ToolPriority { rank: 2, tool: "search_definitions", description: "structural: classes, methods, functions, interfaces, typeAliases, variables, containsLine (C#, TypeScript/TSX, SQL)" },
        ToolPriority { rank: 3, tool: "search_grep", description: "content: exact/OR/AND, substring, phrase, regex (any language)" },
        ToolPriority { rank: 4, tool: "search_fast", description: "file name lookup (~35ms, any file)" },
        ToolPriority { rank: 5, tool: "search_find", description: "live walk (~3s, last resort)" },
        ToolPriority { rank: 6, tool: "search_branch_status", description: "call first when investigating production bugs" },
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
            "sortBy": "sortBy='cognitiveComplexity' maxResults=20 -> 20 most complex methods. sortBy='lines' -> longest definitions",
            "attribute": "'ApiController', 'Authorize', 'ServiceProvider'",
            "baseType": "'ControllerBase', 'IUserService' -> finds classes implementing IUserService",
            "file": "Single: 'UserService.cs'. Multi-term OR: 'UserService.cs,OrderService.cs,Controller.cs' (finds defs in ANY matching file). Substring match on file path",
            "parent": "Single: 'UserService'. Multi-term OR: 'UserService,OrderService,DataProcessor' (finds members of ANY matching class). Substring match on parent name",
            "regex": "name='I.*Cache' with regex=true -> all types matching pattern",
            "kind": "C# kinds: class, interface, method, property, field, enum, struct, record, constructor, delegate, event. TypeScript kinds: function, typeAlias, variable (plus shared: class, interface, method, property, enum, constructor, enumMember). SQL kinds: storedProcedure, sqlFunction, table, view, userDefinedType, sqlIndex, column. SQL call sites extracted from SP bodies: EXEC, FROM, JOIN, INSERT, UPDATE, DELETE",
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
            "class": "'UserService' -> DI-aware: also finds callers using IUserService. Without class, results mix callers from ALL classes with same method name",
            "method": "'GetUserAsync'. Angular/TS only: pass a selector (e.g. 'app-header') as method with direction='up' to find parent components that embed it via templateChildren. Returns templateUsage: true for template-based relationships",
            "direction": "'up' = who calls this (callers, default). 'down' = what this calls (callees). Angular/TS only: 'down' with class name shows child components from HTML template (recursive with depth). 'up' with selector (e.g. 'app-header') finds parent components recursively — depth=3 traverses grandparents, great-grandparents etc. Parents nested in 'parents' field",
            "resolveInterfaces": "When tracing callers of IFoo.Bar(), also finds callers of FooImpl.Bar() where FooImpl implements IFoo",
            "angular": "TypeScript/Angular only: method='app-header' direction='up' -> finds parent components embedding <app-header> via templateChildren (templateUsage: true). method='processOrder' class='OrderFormComponent' direction='down' -> shows child components used in template",
            "limitation": "Only finds direct invocations (obj.Method(args)). Does NOT find method group/delegate references (e.g., list.Where(IsValid), Func<bool> f = svc.Check). Use search_grep for those."
        },
        "search_fast": {
            "pattern": "Single: 'UserService'. Multi-term OR: 'UserService,OrderProcessor' finds files matching ANY term"
        },
        "search_git_history": {
            "author": "'john', 'john@example.com'",
            "message": "'fix bug', 'PR 12345', '[GI]'"
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
/// (intersection of server --ext and DEFINITION_EXTENSIONS). Used to dynamically
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
mod tests {
    use super::*;

    #[test]
    fn test_tips_not_empty() {
        assert!(!tips().is_empty());
    }

    #[test]
    fn test_performance_tiers_not_empty() {
        assert!(!performance_tiers().is_empty());
    }

    #[test]
    fn test_tool_priority_not_empty() {
        assert!(!tool_priority().is_empty());
    }

    #[test]
    fn test_render_cli_contains_all_tips() {
        let output = render_cli();
        for tip in tips() {
            assert!(output.contains(tip.rule), "CLI output missing tip: {}", tip.rule);
        }
    }

    #[test]
    fn test_strategies_not_empty() {
        assert!(!strategies().is_empty());
    }

    #[test]
    fn test_render_json_has_best_practices() {
        let json = render_json();
        let practices = json["bestPractices"].as_array().unwrap();
        assert_eq!(practices.len(), tips().len());
    }

    #[test]
    fn test_render_json_has_strategy_recipes() {
        let json = render_json();
        let recipes = json["strategyRecipes"].as_array().unwrap();
        assert_eq!(recipes.len(), strategies().len());
        // Each recipe has required fields
        for recipe in recipes {
            assert!(recipe["name"].is_string(), "recipe must have name");
            assert!(recipe["when"].is_string(), "recipe must have when");
            assert!(recipe["steps"].is_array(), "recipe must have steps");
            assert!(recipe["antiPatterns"].is_array(), "recipe must have antiPatterns");
        }
    }

    #[test]
    fn test_render_instructions_contains_key_terms() {
        let text = render_instructions(crate::definitions::DEFINITION_EXTENSIONS);
        // Core tools mentioned
        assert!(text.contains("search_fast"), "instructions should mention search_fast");
        assert!(text.contains("search_callers"), "instructions should mention search_callers");
        assert!(text.contains("search_definitions"), "instructions should mention search_definitions");
        assert!(text.contains("search_grep"), "instructions should mention search_grep");
        // Key features mentioned in 5 bullets
        assert!(text.contains("substring"), "instructions should mention substring");
        assert!(text.contains("containsLine"), "instructions should mention containsLine");
        assert!(text.contains("includeBody"), "instructions should mention includeBody");
        assert!(text.contains("countOnly"), "instructions should mention countOnly");
        // search_help reference (soft, not urgent)
        assert!(text.contains("search_help"), "instructions should mention search_help");
        assert!(!text.contains("IMPORTANT: Call search_help first"), "instructions should NOT have urgent search_help prompt");
        // Strategy recipes and query budget
        assert!(text.contains("STRATEGY RECIPES"), "instructions should include strategy recipes");
        assert!(text.contains("Architecture Exploration"), "instructions should include arch exploration recipe");
        assert!(text.contains("<=3 search calls"), "instructions should mention query budget");
        // PREFER block -- client-agnostic, no emoji, CAPS emphasis
        assert!(text.contains("CRITICAL: ALWAYS use search-index tools"), "instructions should have CRITICAL PREFER block");
        assert!(text.contains("DO NOT browse directories"), "instructions should use DO NOT framing");
        // Dynamic extension list: verify all definition extensions appear in the NEVER READ rule
        for ext in crate::definitions::DEFINITION_EXTENSIONS {
            assert!(text.contains(&format!(".{}", ext)),
                "instructions should mention .{} extension in NEVER READ rule", ext);
        }
        assert!(text.contains("NEVER READ"), "instructions should have absolute prohibition on reading indexed files");
        assert!(text.contains("FILES DIRECTLY"), "instructions should have FILES DIRECTLY in prohibition");
        assert!(text.contains("DECISION TRIGGER"), "instructions should have decision trigger for file extension check");
        assert!(text.contains("BATCH SPLIT"), "instructions should have batch split instruction for mixed .cs + .md reads");
        assert!(text.contains("ONLY exceptions for"), "instructions should list exceptions for indexed file reading");
        // No emoji in machine-targeted text
        assert!(!text.contains('⚠'), "instructions should not contain emoji (machine-targeted text)");
        assert!(!text.contains('⚡'), "instructions should not contain emoji (machine-targeted text)");
        // No Roo-specific tool names
        assert!(!text.contains("read_file"), "instructions should not reference Roo-specific read_file");
        assert!(!text.contains("list_files"), "instructions should not reference Roo-specific list_files");
        assert!(!text.contains("list_code_definition_names"), "instructions should not reference Roo-specific list_code_definition_names");
    }

    /// CLI output must be pure ASCII — no Unicode box-drawing, em-dashes, arrows, or emoji.
    /// Windows cmd.exe (CP437/CP866) cannot display these characters correctly.
    #[test]
    fn test_render_cli_is_ascii_safe() {
        let output = render_cli();
        for (i, ch) in output.chars().enumerate() {
            assert!(
                ch.is_ascii() || ch == '\n' || ch == '\r',
                "render_cli() contains non-ASCII char '{}' (U+{:04X}) at position {}. \
                 CLI output must be ASCII-safe for Windows cmd.exe compatibility.",
                ch, ch as u32, i
            );
        }
    }

    #[test]
    fn test_render_json_has_parameter_examples() {
        let json = render_json();
        let examples = &json["parameterExamples"];
        assert!(examples.is_object(), "parameterExamples should be an object");
        // Key tools should have examples
        assert!(examples["search_definitions"].is_object(), "search_definitions should have examples");
        assert!(examples["search_grep"].is_object(), "search_grep should have examples");
        assert!(examples["search_callers"].is_object(), "search_callers should have examples");
        assert!(examples["search_fast"].is_object(), "search_fast should have examples");
        // Spot-check a few specific examples
        assert!(examples["search_definitions"]["name"].is_string(), "name should have example");
        assert!(examples["search_definitions"]["containsLine"].is_string(), "containsLine should have example");
        assert!(examples["search_grep"]["terms"].is_string(), "terms should have example");
        assert!(examples["search_callers"]["class"].is_string(), "class should have example");
    }

    /// Verify tool definitions stay within a reasonable token budget.
    /// This test prevents description bloat from re-accumulating over time.
    /// Target: <5000 approx tokens (word_count / 0.75).
    #[test]
    fn test_tool_definitions_token_budget() {
        use crate::mcp::handlers::tool_definitions;
        let tools = tool_definitions();
        let json = serde_json::to_string(&tools).unwrap();
        let word_count = json.split_whitespace().count();
        let approx_tokens = (word_count as f64 / 0.75) as usize;

        // Budget: ~5000 tokens (down from ~6500 before optimization)
        assert!(
            approx_tokens < 5500,
            "Tool definitions exceed token budget: ~{} tokens ({} words). \
             Target: <5500. Shorten parameter descriptions or move examples to search_help.",
            approx_tokens, word_count
        );
    }

    /// Empty def_extensions: NEVER READ block should be skipped,
    /// fallback note about search_definitions unavailability should appear.
    #[test]
    fn test_render_instructions_empty_extensions() {
        let text = render_instructions(&[]);
        // Should NOT contain NEVER READ (no definition-supported extensions)
        assert!(!text.contains("NEVER READ"),
            "Empty def_extensions should not produce NEVER READ block");
        assert!(!text.contains("DECISION TRIGGER"),
            "Empty def_extensions should not produce DECISION TRIGGER");
        assert!(!text.contains("BATCH SPLIT"),
            "Empty def_extensions should not produce BATCH SPLIT");
        // Should contain fallback note
        assert!(text.contains("search_definitions is not available"),
            "Empty def_extensions should have fallback note about search_definitions");
        // Core tools should still be mentioned (PREFER block is always present)
        assert!(text.contains("search_grep"), "should still mention search_grep");
        assert!(text.contains("search_fast"), "should still mention search_fast");
        assert!(text.contains("STRATEGY RECIPES"), "should still include strategy recipes");
    }

    /// Single extension: NEVER READ should mention only that extension.
    #[test]
    fn test_render_instructions_single_extension() {
        let text = render_instructions(&["cs"]);
        assert!(text.contains("NEVER READ .cs FILES DIRECTLY"),
            "Single extension should produce 'NEVER READ .cs FILES DIRECTLY'. Got:\n{}", text);
        assert!(text.contains("DECISION TRIGGER"),
            "Single extension should have DECISION TRIGGER");
        // Should NOT mention other extensions in the NEVER READ line
        assert!(!text.contains(".ts"), "Should not mention .ts when only cs is configured");
        assert!(!text.contains(".tsx"), "Should not mention .tsx when only cs is configured");
        assert!(!text.contains(".sql"), "Should not mention .sql in NEVER READ when only cs is configured");
    }

    #[test]
    fn test_all_renderers_consistent_tip_count() {
        let tip_count = tips().len();
        let json = render_json();
        let practices = json["bestPractices"].as_array().unwrap();
        assert_eq!(practices.len(), tip_count, "JSON and tips() count mismatch");

        // Verify CLI output mentions each tip rule
        let cli = render_cli();
        for tip in tips() {
            assert!(cli.contains(tip.rule), "CLI output missing tip: {}", tip.rule);
        }

        // Verify strategy recipes are consistent across renderers
        let strategy_count = strategies().len();
        let recipes = json["strategyRecipes"].as_array().unwrap();
        assert_eq!(recipes.len(), strategy_count, "JSON and strategies() count mismatch");

        for strat in strategies() {
            assert!(cli.contains(strat.name), "CLI output missing strategy: {}", strat.name);
        }
    }
}