//! Single source of truth for best practices and tips.
//! Used by: CLI `search tips`, MCP `xray_help` tool, MCP `instructions` field.

use std::borrow::Cow;
use serde_json::{json, Value};

/// Map file extensions to human-readable language names for tool descriptions.
/// Separates tree-sitter languages from regex-based (SQL).
/// Deduplicates (ts+tsx → one "TypeScript/TSX").
///
/// Examples:
/// - `["rs"]` → `"Rust"`
/// - `["cs", "ts", "tsx"]` → `"C# and TypeScript/TSX"`
/// - `["cs", "rs", "ts", "sql"]` → `"C#, Rust, and TypeScript/TSX. SQL supported via regex parser"
pub fn format_supported_languages(def_extensions: &[String]) -> String {
    let mut tree_sitter_langs: Vec<&str> = Vec::new();
    let mut regex_langs: Vec<&str> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let has_ts = def_extensions.iter().any(|e| e == "ts");
    let has_tsx = def_extensions.iter().any(|e| e == "tsx");
    let ts_label = match (has_ts, has_tsx) {
        (true, true) => "TypeScript/TSX",
        (true, false) => "TypeScript",
        (false, true) => "TSX",
        (false, false) => "", // neither present, won't be used
    };

    for ext in def_extensions {
        let (lang, is_tree_sitter) = match ext.as_str() {
            "cs" => ("C#", true),
            "ts" | "tsx" => (ts_label, true),
            "rs" => ("Rust", true),
            "sql" => ("SQL", false),
            _ => continue,
        };
        if seen.insert(lang) {
            if is_tree_sitter {
                tree_sitter_langs.push(lang);
            } else {
                regex_langs.push(lang);
            }
        }
    }

    let ts_part = format_lang_list(&tree_sitter_langs);
    let regex_part = format_lang_list(&regex_langs);

    match (ts_part.is_empty(), regex_part.is_empty()) {
        (true, true) => String::new(),
        (false, true) => ts_part,
        (true, false) => format!("{} (regex-based parser)", regex_part),
        (false, false) => format!("{}. {} supported via regex parser", ts_part, regex_part),
    }
}

fn format_lang_list(langs: &[&str]) -> String {
    match langs.len() {
        0 => String::new(),
        1 => langs[0].to_string(),
        2 => format!("{} and {}", langs[0], langs[1]),
        _ => {
            let (last, rest) = langs.split_last().unwrap();
            format!("{}, and {}", rest.join(", "), last)
        }
    }
}

/// A single best practice tip.
pub struct Tip {
    pub rule: Cow<'static, str>,
    pub why: Cow<'static, str>,
    pub example: Cow<'static, str>,
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
    pub description: Cow<'static, str>,
}

/// A strategy recipe for a common task pattern.
pub struct Strategy {
    pub name: &'static str,
    pub when: &'static str,
    pub steps: &'static [&'static str],
    pub anti_patterns: &'static [&'static str],
}

/// Task routing record for auto-generated TASK ROUTING table in MCP instructions.
/// Maps a user task to the recommended tool, with scope filtering by def_extensions.
pub struct TaskRouting {
    /// Task description (task-first framing, e.g., "Read/explore source code")
    pub task: &'static str,
    /// Tool name (no arguments — table answers "WHICH tool", not "HOW to call")
    pub tool: &'static str,
    /// Whether this routing requires definition index (filtered when def_extensions is empty)
    pub requires_definitions: bool,
}

/// Returns all task routing records. Filtered by `def_extensions` at render time.
pub fn task_routings() -> Vec<TaskRouting> {
    vec![
        // --- Require definition index ---
        TaskRouting {
            task: "Read/explore source code",
            tool: "xray_definitions",
            requires_definitions: true,
        },
        TaskRouting {
            task: "Find method/class at a known line number",
            tool: "xray_definitions",
            requires_definitions: true,
        },
        TaskRouting {
            task: "Find callers or callees of a method",
            tool: "xray_callers",
            requires_definitions: true,
        },
        TaskRouting {
            task: "Code complexity / health scan",
            tool: "xray_definitions",
            requires_definitions: true,
        },
        TaskRouting {
            task: "Explore a module / understand directory structure",
            tool: "xray_definitions",
            requires_definitions: true,
        },
        // --- Always available ---
        TaskRouting {
            task: "Read/search non-code files (XML, JSON, config, YAML, MD, txt)",
            tool: "xray_grep",
            requires_definitions: false,
        },
        TaskRouting {
            task: "Search file contents (text, patterns)",
            tool: "xray_grep",
            requires_definitions: false,
        },
        TaskRouting {
            task: "Find a file by name",
            tool: "xray_fast",
            requires_definitions: false,
        },
        TaskRouting {
            task: "List files or subdirectories in a folder",
            tool: "xray_fast",
            requires_definitions: false,
        },
        TaskRouting {
            task: "Edit a file (any modification)",
            tool: "xray_edit",
            requires_definitions: false,
        },
        TaskRouting {
            task: "Git blame / history / authorship",
            tool: "xray_git_blame / xray_git_history / xray_git_authors",
            requires_definitions: false,
        },
    ]
}

// ─── Single source of truth ─────────────────────────────────────────

pub fn tips(def_extensions: &[String]) -> Vec<Tip> {
    let lang_list = format_supported_languages(def_extensions);
    vec![
        Tip {
            rule: "File lookup: use xray_fast, not built-in list_files".into(),
            why: "xray_fast uses a pre-built index (~35ms). Built-in list_files does a live filesystem walk (~3s). 90x+ faster.".into(),
            example: "xray_fast with pattern='UserService' instead of built-in list_files".into(),
        },
        Tip {
            rule: "Multi-term OR: find all variants in ONE query".into(),
            why: "Comma-separated terms with mode='or' finds files containing ANY term. Much faster than separate queries.".into(),
            example: "xray grep \"UserService,IUserService,UserServiceFactory\" -e cs  |  MCP: terms='...', mode='or'".into(),
        },
        Tip {
            rule: "AND mode: find files containing ALL terms".into(),
            why: "mode='and' finds files where ALL comma-separated terms co-occur. Useful for finding DI registrations.".into(),
            example: "xray grep \"ServiceProvider,IUserService\" -e cs --all  |  MCP: terms='...', mode='and'".into(),
        },
        Tip {
            rule: "Substring search is ON by default".into(),
            why: "xray_grep defaults to substring=true so compound identifiers (IUserService, m_userService) are always found. Use substring=false for exact-token-only matching. Auto-disabled when regex or phrase is used. Short tokens (<4 chars) may produce broad results -- use exclude=['pattern'] to filter noise from file paths. Comma-separated phrases with spaces are searched independently as OR (or AND with mode='and').".into(),
            example: "Default: terms='UserService' finds IUserService, m_userService. Multi-phrase OR: terms='fn handle_foo,fn build_bar'. Exact only: terms='UserService', substring=false".into(),
        },
        Tip {
            rule: "Phrase search: exact multi-word match".into(),
            why: "phrase=true finds exact adjacent word sequences. Slower (~80ms) but precise.".into(),
            example: "xray grep \"new HttpClient\" -e cs --phrase  |  MCP: terms='new HttpClient', phrase=true".into(),
        },
        Tip {
            rule: "Regex pattern search".into(),
            why: "Full regex for pattern matching. Also works in xray_definitions name parameter.".into(),
            example: "xray grep \"I[A-Z]\\w+Cache\" -e cs --regex  |  MCP: terms='I[A-Z]\\w+Cache', regex=true".into(),
        },
        Tip {
            rule: "Exclude test/mock dirs for production-only results".into(),
            why: "Half the results are often test files. Use excludeDir to filter them out.".into(),
            example: "--exclude-dir test --exclude-dir Mock  |  MCP: excludeDir=['test','Mock','UnitTests']".into(),
        },
        Tip {
            rule: "Call chain tracing: xray_callers (up and down)".into(),
            why: "Single sub-millisecond request replaces 7+ sequential grep + read_file calls. direction='up' (callers) or 'down' (callees). For Angular: direction='down' with class shows template children, direction='up' with selector finds parent components.".into(),
            example: "MCP: xray_callers method='GetUserAsync', class='UserService', depth=2, direction='up'".into(),
        },
        Tip {
            rule: "Always specify class in xray_callers".into(),
            why: "Without class, results mix callers from ALL classes with same method name. Misleading call trees.".into(),
            example: "MCP: xray_callers method='ExecuteAsync', class='OrderProcessor'".into(),
        },
        Tip {
            rule: "xray_callers 0 results? Try the interface name".into(),
            why: "Calls through DI use the interface (IUserService), not the implementation (UserService). xray_callers matches the receiver type recorded at call sites. If you search class='UserService' but callers use IUserService, you get 0 results.".into(),
            example: "0 callers with class='UserService' -> retry with class='IUserService'. Also: resolveInterfaces=true auto-resolves implementations".into(),
        },
        Tip {
            rule: "Stack trace analysis: containsLine".into(),
            why: "Given file + line number, returns innermost method AND parent class. No manual read_file needed.".into(),
            example: "MCP: xray_definitions file='UserService.cs', containsLine=42".into(),
        },
        Tip {
            rule: "Read method source: use includeBody=true instead of reading files".into(),
            why: "xray_definitions with includeBody=true returns method body inline, eliminating read_file round-trips. BEFORE reading any indexed source file, try xray_definitions with includeBody=true first. Use maxBodyLines/maxTotalBodyLines for budget. Only read files directly for non-indexed content (markdown, JSON, XML, config) or when you need exact line numbers for editing.".into(),
            example: "MCP: xray_definitions parent='UserService', includeBody=true, maxBodyLines=20. Also: xray_definitions name='Program,Startup,OrderService' includeBody=true -> reads multiple classes at once, faster than multiple read_file calls".into(),
        },
        Tip {
            rule: "Body budgets: manage with maxBodyLines and maxTotalBodyLines".into(),
            why: "Default limits: 100 lines/def, 500 total. Strategies: (1) Wide overview: includeBody=true maxBodyLines=30 -- gets first 30 lines of many methods. (2) Targeted read: name='SpecificMethod' includeBody=true maxBodyLines=0 -- full body of one method. (3) Unlimited: maxBodyLines=0 maxTotalBodyLines=0 -- removes all limits (large response). If response contains definitions with 'bodyOmitted', narrow with name='<specific names>' to read them individually.".into(),
            example: "Overview: includeBody=true maxBodyLines=30. Targeted: name='ProcessOrder' includeBody=true maxBodyLines=0. Unlimited: maxBodyLines=0, maxTotalBodyLines=0. After bodyOmitted: name='OmittedMethod1,OmittedMethod2' includeBody=true".into(),
        },
        Tip {
            rule: "Reconnaissance: use countOnly=true".into(),
            why: "xray_grep with countOnly=true returns ~46 tokens (counts only) vs 265+ for full results. Perfect for 'how many files use X?'.".into(),
            example: "xray grep \"HttpClient\" -e cs --count-only  |  MCP: terms='HttpClient', countOnly=true".into(),
        },
        Tip {
            rule: "Search ANY indexed file type: XML, csproj, config, etc.".into(),
            why: "xray_grep works with all file extensions passed to --ext. Use ext='csproj' to find NuGet dependencies, ext='xml,config,manifestxml' for configuration values.".into(),
            example: "xray grep \"Newtonsoft.Json\" -e csproj  |  MCP: terms='Newtonsoft.Json', ext='csproj'".into(),
        },
        Tip {
            rule: if lang_list.is_empty() {
                Cow::Borrowed("Language scope: content search = any language, AST = no definition parsers active")
            } else {
                Cow::Owned(format!("Language scope: content search = any language, AST = {}", lang_list))
            },
            why: "xray_grep / content-index use a language-agnostic tokenizer -- works with any text file (C#, Rust, Python, JS, XML, etc.). xray_definitions / def-index uses AST parsing for languages configured with --definitions (see xray_definitions tool description for current list). xray_callers uses call-graph analysis for the same languages (DI-aware, inject() support, interface resolution). SQL uses regex-based parser with SP-to-SP EXEC call chains (class parameter = schema name). Tables/views excluded from call graph (data, not code).".into(),
            example: "xray grep works on any -e extension | xray_definitions/xray_callers work only on extensions with definition parser support (check tool descriptions for current list)".into(),
        },
        Tip {
            rule: "Response truncation: large results are auto-capped at ~16KB".into(),
            why: "Broad queries (short substring, common tokens) can return thousands of files. The server auto-truncates responses to ~16KB (~4K tokens) to avoid filling LLM context. summary.totalFiles always shows the FULL count. Use countOnly=true or narrow with dir/ext/exclude to get focused results.".into(),
            example: "If responseTruncated=true appears, narrow your query: add ext, dir, excludeDir, or use countOnly=true. Server flag --max-response-kb adjusts the limit (0=unlimited).".into(),
        },
        Tip {
            rule: "Code health: find complex methods with includeCodeStats/sortBy/min*".into(),
            why: "Instant code quality scan across entire codebase. sortBy='cognitiveComplexity' ranks worst methods first. Combine min* filters (AND logic) to find God Methods. Only methods/functions/constructors have stats.".into(),
            example: "xray_definitions sortBy='cognitiveComplexity' maxResults=20  |  xray_definitions minComplexity=10 minParams=5 sortBy='cyclomaticComplexity'".into(),
        },
        Tip {
            rule: "Multi-term name in xray_definitions: find ALL types in ONE call".into(),
            why: "The name parameter accepts comma-separated terms (OR logic). Find a class + its interface + related types in a single query instead of 3 separate calls.".into(),
            example: "xray_definitions name='UserService,IUserService,UserController' -> finds ALL matching definitions in one call".into(),
        },
        Tip {
            rule: "Query budget: aim for 3 or fewer search calls per exploration task".into(),
            why: "Each search call adds latency and LLM context. Use multi-term queries, includeBody, and combined filters to minimize round-trips. Most architecture questions can be answered in 1-3 calls.".into(),
            example: "Step 1: xray_definitions name='OrderService,IOrderService' includeBody=true (map + read). Step 2: xray_callers method='ProcessOrder' class='OrderService' (call chain). Done in 2 calls.".into(),
        },
        Tip {
            rule: "Check branch status before investigating production bugs".into(),
            why: "Call xray_branch_status first to verify you're on the right branch and your data is up-to-date. Avoids wasted investigation on stale or wrong-branch data.".into(),
            example: "MCP: xray_branch_status repo='.' -> shows branch, behind/ahead counts, fetch age, dirty files".into(),
        },
        Tip {
            rule: "Use noCache=true when git results seem stale".into(),
            why: "xray_git_history/authors/activity use an in-memory cache for speed. If results seem outdated after recent commits, use noCache=true to bypass cache and query git CLI directly.".into(),
            example: "MCP: xray_git_history repo='.', file='src/main.rs', noCache=true".into(),
        },
        Tip {
            rule: "Method group/delegate references: use xray_grep, not xray_callers".into(),
            why: "xray_callers only finds direct method invocations (obj.Method(args)). It does NOT detect methods passed as delegates or method groups (e.g., list.Where(IsValid), Func<bool> f = service.Check). Use xray_grep to find all textual references including delegate usage.".into(),
            example: "xray_callers misses delegate usage -> use xray_grep terms='IsValid' ext='cs' to find all references including method group passes".into(),
        },
        Tip {
            rule: "Impact analysis: find which tests cover a method".into(),
            why: "impactAnalysis=true in xray_callers traces callers upward and identifies test methods (via [Test]/[Fact]/[Theory]/[TestMethod]/#[test] attributes or *.spec.ts/*.test.ts files). Returns a 'testsCovering' array with all tests in the call chain. Use depth=5-7 for deep call chains. One call replaces manual multi-step investigation.".into(),
            example: "MCP: xray_callers method='SaveOrder' class='OrderService' direction='up' depth=5 impactAnalysis=true -> testsCovering: [{method: 'TestSaveOrder', class: 'OrderTests', file: 'src/OrderTests.cs', depth: 1, callChain: ['SaveOrder', 'TestSaveOrder']}]".into(),
        },
        Tip {
            rule: "using static: search by defining class, not consuming class".into(),
            why: "xray_definitions searches AST definition names. Methods imported via C# 'using static' are defined in their original class. Searching parent='ConsumingClass' will return 0 results. Search without parent filter or with parent='DefiningClass' instead.".into(),
            example: "Percentile() imported via 'using static PercentileHelper' -> xray_definitions name='Percentile' (without parent) or parent='PercentileHelper'".into(),
        },
    ]
}

pub fn strategies() -> Vec<Strategy> {
    vec![
        Strategy {
            name: "Architecture Exploration",
            when: "User asks 'how is X structured', 'explain module X', or 'show me the architecture of X'",
            steps: &[
                "Step 1 - Map the landscape (1 call): xray_definitions file='<dirname>' excludeDir=['test','Test','Mock'] -> if many results, returns autoSummary with directory grouping and top classes per subdirectory. If few results, returns full definition list. Use file='<subdir>' to drill into specific groups",
                "Step 2 - Read key implementations (1 call): xray_definitions name='<top 3-5 key classes from step 1>' includeBody=true maxBodyLines=30 -> returns source code of the most important files",
                "Step 3 (optional) - Scope and dependencies (1 call): xray_grep terms='X' countOnly=true -> scale (how many files, occurrences); or xray_fast pattern='X' dirsOnly=true -> directory structure",
            ],
            anti_patterns: &[
                "Don't use list_files + read_file to explore architecture -- xray_definitions returns classes, methods, file paths, and source code in ONE call",
                "Don't read indexed source files to see source code -- xray_definitions includeBody=true returns it directly. read_file is ONLY for non-indexed files (markdown, JSON, XML, config) or for editing (need exact line numbers)",
                "Don't search one kind at a time (class, then interface, then enum) -- use kind='class,interface,enum' for multi-kind OR, or omit kind filter to get everything at once",
                "Don't use countOnly first then re-query with body -- go straight to includeBody=true with maxBodyLines",
                "Don't browse directories (list_files, list_directory, xray_fast with empty pattern) to understand code -- xray_definitions file='<dir>' returns ALL definitions with file paths in ONE call. For large modules, autoSummary kicks in with directory grouping",
                "Don't search for file names separately if xray_definitions already found them (results include file paths)",
                "Don't make separate queries for ClassName and IClassName -- use comma-separated: name='ClassName,IClassName'",
            ],
        },
        Strategy {
            name: "Call Chain Investigation",
            when: "User asks 'who calls X', 'trace how X is invoked', or 'show the call chain for X'",
            steps: &[
                "Step 1 - Get call tree with source (1 call): xray_callers method='MethodName' class='ClassName' depth=3 direction='up' includeBody=true -> full caller hierarchy WITH method source code inline",
                "Step 2 (optional, only if more context needed): xray_definitions name='<specific callers from step 1>' includeBody=true maxBodyLines=0 -> full source of specific methods",
            ],
            anti_patterns: &[
                "Don't omit the class parameter -- without it, results mix callers from ALL classes with the same method name",
                "Don't use xray_grep to manually find callers -- xray_callers does it in sub-millisecond with DI/interface resolution",
                "Don't call xray_callers then xray_definitions separately to get source -- use includeBody=true in xray_callers to get both in ONE call",
            ],
        },
        Strategy {
            name: "Stack Trace / Bug Investigation",
            when: "User provides a stack trace, error at file:line, or asks 'what method is at line N'",
            steps: &[
                "Step 1 - Identify method (1 call): xray_definitions file='FileName.cs' containsLine=42 includeBody=true -> returns the method body (innermost) + parent class metadata (body omitted to save budget)",
                "Step 2 (optional) - Trace callers (1 call): xray_callers method='<method from step 1>' class='<class from step 1>' depth=2 -> who triggered this code path",
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
                "Step 1 - Verify branch (1 call): xray_branch_status repo='.' -> confirm you're on main and data is fresh",
                "Step 2 - Find where code lives (1 call): xray_grep terms='<error text>' ext='cs' -> find file and line number",
                "Step 3 - Find who introduced it (1 call): xray_git_blame repo='.', file='<file from step 2>', startLine=<line> -> exact commit, author, date",
                "Step 4 (optional) - Full history (1 call): xray_git_history repo='.', file='<file from step 2>' -> all commits for context",
                "Step 5 (optional) - File ownership (1 call): xray_git_authors repo='.', path='<file>' -> who maintains this file",
            ],
            anti_patterns: &[
                "Don't skip xray_branch_status -- investigating on the wrong branch wastes time",
                "Don't use xray_git_history to find WHEN a specific string appeared -- use xray_grep + xray_git_blame instead (faster and more precise)",
            ],
        },
        Strategy {
            name: "Angular Component Hierarchy (TypeScript only)",
            when: "User asks 'who uses <component>', 'where is it embedded', or 'show parent components' (TypeScript/Angular projects only)",
            steps: &[
                "Step 1 (1 call): xray_callers method='<selector>' direction='up' depth=3 -> finds parent AND grandparent components recursively via templateChildren (<1ms). Parents nested in 'parents' field.",
                "Step 2 (optional): xray_callers method='<class-name>' direction='down' depth=2 -> shows child components used in template (recursive)",
            ],
            anti_patterns: &[
                "Don't use xray_grep to find component selectors in HTML -- xray_callers resolves template relationships from the AST index in <1ms",
                "Don't forget to use the component selector (e.g. 'app-header'), not the class name, when searching direction='up'",
            ],
        },
        Strategy {
            name: "Code Health Scan",
            when: "User asks 'find complex methods', 'code quality', 'refactoring candidates', or 'technical debt'",
            steps: &[
                "Step 1 - Top offenders (1 call): xray_definitions sortBy='cognitiveComplexity' maxResults=20 -> worst 20 methods by cognitive complexity",
                "Step 2 (optional) - Narrow to module (1 call): xray_definitions file='Services' minComplexity=10 sortBy='cyclomaticComplexity' -> complex methods in specific directory",
                "Step 3 (optional) - God Method detection (1 call): xray_definitions minComplexity=20 minParams=5 minCalls=15 -> methods that are too large, have too many params, and high fan-out",
            ],
            anti_patterns: &[
                "Don't read every file to count if/else -- sortBy computes metrics from the AST index in <1ms",
                "Don't forget includeCodeStats is auto-enabled by sortBy and min* -- no need to pass it explicitly",
                "Don't use sortBy on old indexes without code stats -- run xray_reindex_definitions first",
            ],
        },
        Strategy {
            name: "Code Review / Story Evaluation",
            when: "User asks 'review this PR', 'evaluate this story', \
                   'is this change feasible?', or 'assess this code change'",
            steps: &[
                "Step 1 - Understand current structure (1 call): \
                 xray_definitions file='<files mentioned in story/PR>' includeBody=false \
                 -> list all definitions to understand current architecture",
                "Step 2 - Validate specific code (1 call): \
                 xray_definitions name='<specific functions referenced>' includeBody=true maxBodyLines=0 \
                 -> read actual code to validate claims and code sketches",
                "Step 3 (optional) - Verify scale (1 call): \
                 xray_grep terms='<patterns discussed>' countOnly=true \
                 -> confirm how widespread a pattern is",
            ],
            anti_patterns: &[
                "Don't read entire files with read_file to understand architecture \
                 -- xray_definitions file='...' gives a structured view of all definitions",
                "Don't read files just to count occurrences \
                 -- xray_grep countOnly=true is instant",
                "Don't read files to check if a function exists \
                 -- xray_definitions name='...' answers in <1ms",
            ],
        },
    ]
}

pub fn performance_tiers() -> Vec<PerfTier> {
    vec![
        PerfTier {
            name: "Instant",
            range: "<1ms",
            operations: &["xray_grep (substring default)", "xray_callers", "xray_definitions baseType/attribute"],
        },
        PerfTier {
            name: "Fast",
            range: "1-10ms",
            operations: &["xray_grep showLines", "xray_definitions containsLine"],
        },
        PerfTier {
            name: "Quick",
            range: "10-100ms",
            operations: &["xray_fast", "xray_definitions name/parent/includeBody", "xray_grep regex/phrase"],
        },

    ]
}

pub fn tool_priority(def_extensions: &[String]) -> Vec<ToolPriority> {
    let lang_desc = if def_extensions.is_empty() {
        "no definition parsers active".to_string()
    } else {
        format_supported_languages(def_extensions)
    };
    vec![
        ToolPriority { rank: 1, tool: "xray_callers", description: Cow::Owned(format!("call trees up/down (<1ms, {})", lang_desc)) },
        ToolPriority { rank: 2, tool: "xray_definitions", description: Cow::Owned(format!("structural: classes, methods, functions, interfaces, typeAliases, variables, containsLine ({})", lang_desc)) },
        ToolPriority { rank: 3, tool: "xray_grep", description: "content: exact/OR/AND, substring, phrase, regex (any language)".into() },
        ToolPriority { rank: 4, tool: "xray_fast", description: "file name lookup (~35ms, any file)".into() },
        ToolPriority { rank: 5, tool: "xray_branch_status", description: "call first when investigating production bugs".into() },
        ToolPriority { rank: 6, tool: "xray_edit", description: "reliable file editing -- line-range or text-match, atomic, no whitespace issues. Supports multi-file (paths), insert after/before, expectedContext".into() },
    ]
}

// ─── Parameter examples (moved from inline tool descriptions) ───────

/// Examples for tool parameters, organized by tool.
/// Displayed via xray_help so LLMs can look up usage patterns on demand,
/// without consuming tokens on every turn in the system prompt.
pub fn parameter_examples(def_extensions: &[String]) -> Value {
    let has_rs = def_extensions.iter().any(|e| e == "rs");
    let has_cs = def_extensions.iter().any(|e| e == "cs");
    let has_ts = def_extensions.iter().any(|e| e == "ts" || e == "tsx");
    let _ = (has_rs, has_cs, has_ts); // used below in conditional examples
    json!({
        "xray_definitions": {
            "name": "Single: 'UserService'. Multi-term OR: 'UserService,IUserService,UserController' (finds ALL in one query). Naming variants: 'Order,IOrder,OrderFactory'. NOTE: name searches AST definition names (classes, methods, properties) -- NOT string literals or values inside code. Use xray_grep for string content. For methods via 'using static', search without parent filter",
            "containsLine": "file='UserService.cs', containsLine=42 -> returns GetUserAsync (lines 35-50), parent: UserService. With includeBody=true, body is emitted ONLY for innermost definition; parent gets 'bodyOmitted' hint (saves body budget for target method)",
            "bodyLineStart/End": "containsLine=1335, bodyLineStart=1330, bodyLineEnd=1345 -> returns only 15 lines of a 363-line method body, avoiding response truncation. Use when you know which lines you need from a large method.",
            "includeBody": "parent='UserService', includeBody=true, maxBodyLines=20 -> returns method bodies inline. When body is truncated, summary includes totalBodyLinesAvailable — use it to calibrate maxTotalBodyLines for retry",
            "includeDocComments": "includeDocComments=true -> expands body upward to capture /// XML doc-comments (C#/Rust) or /** */ JSDoc (TypeScript) above the definition. Implies includeBody=true. Response includes 'docCommentLines' count. Budget-aware: counts against maxBodyLines",
            "sortBy": "sortBy='cognitiveComplexity' maxResults=20 -> 20 most complex methods. sortBy='lines' -> longest definitions",
            "attribute": "'ApiController', 'Authorize', 'ServiceProvider'",
            "baseType": "'ControllerBase', 'IUserService' -> finds classes implementing IUserService",
            "file": "Single: 'UserService.cs'. Multi-term OR: 'UserService.cs,OrderService.cs,Controller.cs' (finds defs in ANY matching file). Substring match on file path",
            "parent": "Single: 'UserService'. Multi-term OR: 'UserService,OrderService,DataProcessor' (finds members of ANY matching class). Substring match on parent name",
            "regex": "name='I.*Cache' with regex=true -> all types matching pattern",
            "kind": "Comma-separated for multi-kind OR: kind='class,interface,enum'. C# kinds: class, interface, method, property, field, enum, struct, record, constructor, delegate, event, enumMember. TypeScript kinds: function, typeAlias, variable (plus shared: class, interface, method, field, enum, constructor, enumMember). NOTE: In TypeScript, class members (e.g. 'private name: string') are kind='field', while interface signatures (e.g. 'readonly id: string' in an interface) are kind='property'. If kind='property' returns 0 results for a TS class, try kind='field'. SQL kinds: storedProcedure, sqlFunction, table, view, userDefinedType, sqlIndex, column. SQL call sites extracted from SP bodies: EXEC, FROM, JOIN, INSERT, UPDATE, DELETE. Response: when multi-name + kind causes some terms to silently drop, summary includes missingTerms array with {term, reason} for each dropped term",
            "includeCodeStats": "Each method gets: lines, cyclomaticComplexity, cognitiveComplexity, maxNestingDepth, paramCount, returnCount, callCount, lambdaCount",
            "audit": "Shows: total files, files with/without definitions, read errors, lossy UTF-8, suspicious files (large files with 0 definitions)",
            "angular": "Angular @Component classes include 'selector' and 'templateChildren' in output, showing which child components are used in the template",
            "includeUsageCount": "includeUsageCount=true -> each definition gets usageCount (number of files containing this name in content index). usageCount=0 or 1 = potential dead code. Counts ALL text occurrences including comments/strings. Exact token match only",
            "autoSummary": "Triggered automatically when results > maxResults and no name filter. Returns directory-grouped overview with counts and top-3 definitions per subdirectory. To get individual definitions instead, add name filter or narrow file scope",
            "xmlTextContent": "XML on-demand: name filter searches both element names AND text content. Example: name='PremiumStorage' finds <ServiceType>PremiumStorage</ServiceType> and returns parent block via auto-promotion. Response includes matchedBy ('name' or 'textContent'), matchedChild (single leaf) or matchedChildren (multiple leaves). Min 3-char term for text content search. Name matches appear first, then textContent-promoted results. Multiple leaf matches in same parent are de-duplicated"
        },
        "xray_grep": {
            "terms": "Token: 'HttpClient'. Multi-term OR: 'HttpClient,ILogger,Task'. Multi-term AND (mode='and'): 'ServiceProvider,IUserService'. Phrase (phrase=true): 'new HttpClient'. Regex (regex=true): 'I.*Cache'",
            "contextLines": "contextLines=5 shows 5 lines before and 5 lines after each match (like grep -C)",
            "showLines": "Returns groups of consecutive lines with startLine, lines array, and matchIndices",
            "ext": "'cs', 'cs,sql', 'xml,config' (comma-separated for multiple)",
            "substring": "Default: terms='UserService' finds IUserService, m_userService. Set substring=false for exact-token-only",
            "dir": "Directory path only (not file path). dir='src/services' searches files under that directory. If you pass a file path (e.g., dir='src/main.rs'), xray_grep returns an error with a hint to use the parent directory or xray_definitions instead"
        },
        "xray_callers": {
            "class": "'UserService' -> DI-aware: also finds callers using IUserService. SQL: class = schema name (e.g., class='dbo', class='Sales'). Without class, results mix callers from ALL classes/schemas with same method name",
            "method": "'GetUserAsync'. Multi-method batch: 'GetUserAsync,SaveChangesAsync,ValidateInput' -> returns independent call trees for each method in a single call (saves N-1 MCP round trips). Single method returns {callTree: [...]}, multiple returns {results: [{method, callTree, nodesInTree}, ...]} with shared body budget. Angular/TS only: pass a selector (e.g. 'app-header') as method with direction='up' to find parent components that embed it via templateChildren. Returns templateUsage: true for template-based relationships",
            "direction": "'up' = who calls this (callers, default). 'down' = what this calls (callees). Angular/TS only: 'down' with class name shows child components from HTML template (recursive with depth). 'up' with selector (e.g. 'app-header') finds parent components recursively — depth=3 traverses grandparents, great-grandparents etc. Parents nested in 'parents' field",
            "resolveInterfaces": "When tracing callers of IFoo.Bar(), also finds callers of FooImpl.Bar() where FooImpl implements IFoo",
            "includeBody": "includeBody=true -> each node in call tree includes 'body' (source lines) and 'bodyStartLine'. Also adds a top-level 'rootMethod' object with the searched method's own body. Eliminates the need for a separate xray_definitions call. Default: false",
            "includeDocComments": "includeDocComments=true -> expands each method body to include doc-comments above it. Implies includeBody=true. Adds 'docCommentLines' field. Default: false",
            "maxBodyLines": "Max source lines per method (default: 30, 0=unlimited). Controls per-method body size when includeBody=true",
            "maxTotalBodyLines": "Max total body lines across all methods in tree (default: 300, 0=unlimited). When exceeded, remaining methods get 'bodyOmitted' instead of body",
            "angular": "TypeScript/Angular only: method='app-header' direction='up' -> finds parent components embedding <app-header> via templateChildren (templateUsage: true). method='processOrder' class='OrderFormComponent' direction='down' -> shows child components used in template",
            "impactAnalysis": "impactAnalysis=true with direction='up' -> identifies test methods covering the target. Response includes 'testsCovering' array (with full file path, depth, and callChain for each test) and isTest=true on test nodes. callChain shows the method-by-method path from target to test — use it to assess relevance (short chain = direct test, long chain = transitive via helpers). Tests detected via: C# [Test]/[Fact]/[Theory]/[TestMethod], Rust #[test], TS *.spec.ts/*.test.ts files. Use depth=5-7 for deep chains.",
            "limitation": "Only finds direct invocations (obj.Method(args)). Does NOT find method group/delegate references (e.g., list.Where(IsValid), Func<bool> f = svc.Check). Use xray_grep for those.",
            "includeGrepReferences": "includeGrepReferences=true -> adds grepReferences[] with files containing the method name as text but NOT in the call tree. Catches delegate usage, method groups, reflection. Skipped for method names < 4 chars. Each entry has file + tokenCount. For line-level detail, use xray_grep with showLines=true"
        },
        "xray_fast": {
            "pattern": "Single: 'UserService'. Multi-term OR: 'UserService,OrderProcessor' finds files matching ANY term. Wildcard: '*' or '' (empty with dir) lists ALL entries. Use with dirsOnly=true for subdirectory listing: pattern='*', dir='src/services', dirsOnly=true",
            "dirsOnly": "When true with wildcard pattern, returns directories sorted by fileCount descending (largest modules first). Each directory entry includes 'fileCount' — total files recursively. Useful for identifying important modules in large repos",
            "maxDepth": "Limit directory depth for dirsOnly results (1=immediate children only, 2=two levels). Default: unlimited. Use maxDepth=1 to avoid truncation on large repos"
        },
        "xray_git_history": {
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
        "xray_git_diff": {
            "note": "Same params as xray_git_history (except noCache — always uses CLI). Includes 'patch' field with diff lines per commit"
        },
        "xray_git_authors": {
            "path": "'src/main.rs' (file), 'src/controllers' (directory), or omit for entire repo. 'file' is backward-compatible alias",
            "top": "10 (default). Max authors to return",
            "from": "'2025-01-01' — narrow to date range",
            "message": "'feature' — filter commits by message substring",
            "noCache": "true -> bypass cache"
        },
        "xray_git_activity": {
            "path": "'src/controllers' — filter by directory. Aggregates across all files within. Omit for whole repo",
            "from": "'2025-01-01' — RECOMMENDED to narrow results. Without date filter, returns ALL repo activity",
            "author": "'alice' — filter by author",
            "message": "'refactor' — filter by commit message"
        },
        "xray_git_blame": {
            "file": "'src/UserService.cs' — file path relative to repo root",
            "startLine": "10 (1-based, required). Start of line range",
            "endLine": "20 (1-based, optional). If omitted, only startLine is blamed"
        },
        "xray_branch_status": {
            "repo": "'.' — shows branch name, behind/ahead counts, dirty files, fetch age. Call FIRST when investigating production bugs"
        },
        "xray_edit": {
            "path": "File path — absolute or relative to server --dir. Works on any text file, not limited to indexed extensions. Mutually exclusive with 'paths'",
            "paths": "Array of file paths for multi-file editing. Same edits/operations applied to ALL files. Transactional: if any file fails, none are written. Max 20 files. Response has 'results' array + 'summary'. Example: paths=['file1.cs', 'file2.cs', 'file3.cs']",
            "operations": "Mode A (line-range): [{startLine: 5, endLine: 5, content: 'new line'}] — replace line 5. [{startLine: 3, endLine: 2, content: 'inserted'}] — insert before line 3 (endLine < startLine). [{startLine: 2, endLine: 4, content: ''}] — delete lines 2-4. Append to end: [{startLine: N+1, endLine: N, content: 'appended'}] where N = line count. Multiple ops applied bottom-up (no offset cascade)",
            "edits": "Mode B (text-match): [{search: 'old', replace: 'new'}] — replace all. [{search: 'old', replace: 'new', occurrence: 2}] — 2nd only. Insert after: [{insertAfter: 'using X;', content: 'using Y;'}]. Insert before: [{insertBefore: 'class Foo', content: '// comment'}]",
            "insertAfter_insertBefore": "Insert content on the line after/before an anchor text. Mutually exclusive with search/replace. Requires 'content' field. Use 'occurrence' to target Nth match (default: first). Example: {insertAfter: 'using System.IO;', content: 'using System.Linq;'}",
            "expectedContext": "Per-edit safety check: verify this text exists within ±5 lines of the match before applying. Example: {search: 'SemaphoreSlim(10)', replace: 'SemaphoreSlim(30)', expectedContext: 'var semaphore = new'}",
            "skipIfNotFound": "Per-edit flag: if true, silently skip when search/anchor text is not found (default: false). Essential for multi-file 'paths' where not all files contain the target text. Without it, one missing file aborts the entire batch. Response includes 'skippedDetails' array with editIndex, search text, and reason for each skipped edit",
            "regex": "true -> treat edit search strings as regex with $1, $2 capture groups (Mode B search/replace only)",
            "dryRun": "true -> preview unified diff without writing file. Works with both single and multi-file",
            "expectedLineCount": "Safety check: abort if file has different line count (prevents stale line numbers)",
            "errorDiagnostics": "When text/anchor/pattern is not found, the error includes a nearest-match hint showing the most similar line in the file with line number and similarity percentage (e.g., 'Nearest match at line 5 (similarity 92%): ...'). Helps diagnose Unicode quote mismatches, case differences, and whitespace issues"
        }
    })
}

// ─── Renderers ──────────────────────────────────────────────────────

/// Render tips as human-readable CLI output.
pub fn render_cli(def_extensions: &[String]) -> String {
    let mut out = String::new();
    out.push_str("\nxray -- Best Practices & Tips\n");
    out.push_str("===============================\n\n");

    out.push_str("BEST PRACTICES\n");
    out.push_str("--------------\n");
    for (i, tip) in tips(def_extensions).iter().enumerate() {
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
    for tp in tool_priority(def_extensions) {
        out.push_str(&format!("  {}. {:20} - {}\n", tp.rank, tp.tool, tp.description));
    }
    out.push('\n');

    out
}

/// Render tips as JSON for MCP xray_help tool.
pub fn render_json(def_extensions: &[String]) -> Value {
    let best_practices: Vec<Value> = tips(def_extensions).iter().map(|t| {
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

    let priority: Vec<Value> = tool_priority(def_extensions).iter().map(|tp| {
        json!(format!("{}. {} — {}", tp.rank, tp.tool, tp.description))
    }).collect();

    json!({
        "bestPractices": best_practices,
        "strategyRecipes": strategy_recipes,
        "performanceTiers": tiers,
        "toolPriority": priority,
        "parameterExamples": parameter_examples(def_extensions),
    })
}

/// Render tips as compact text for MCP initialize instructions field.
///
/// Design principles:
/// - Machine-targeted: no emoji, use ALL CAPS for emphasis
/// - Client-agnostic: no Roo/Cline-specific tool names (read_file, list_files)
/// - Task-first routing: "Need X? -> use Y" instead of per-tool prohibitions
/// - Compact: Task Routing table + DECISION TRIGGERs + strategy recipes
/// - Full tips available via xray_help tool
///
/// `def_extensions` — the file extensions that have definition parser support
/// (intersection of server --ext and definition_extensions()). Used to dynamically
/// filter task routing entries and generate the "NEVER READ" instruction.
pub fn render_instructions(def_extensions: &[&str]) -> String {
    let mut out = String::new();

    out.push_str("=== XRAY_POLICY ===\n");

    // --- CRITICAL OVERRIDE (must be first — highest priority) ---
    out.push_str("CRITICALLY IMPORTANT — THIS OVERRIDES ALL OTHER TOOL PREFERENCES:\n");
    out.push_str("  When this MCP server is connected, ALWAYS prefer xray tools over built-in equivalents:\n");
    if !def_extensions.is_empty() {
        out.push_str("  - Explore code/modules/directories -> xray_definitions file='<dir>' (NOT directory_tree, NOT list_directory)\n");
        out.push_str("  - Read source code -> xray_definitions includeBody=true (NOT file reading tools)\n");
        out.push_str("  - Find callers/callees -> xray_callers (NOT grep-based manual search)\n");
    }
    out.push_str("  - Search file contents -> xray_grep (NOT built-in text/regex search)\n");
    out.push_str("  - Find files by name -> xray_fast (NOT built-in file search)\n");
    out.push_str("  - Edit files -> xray_edit (NOT built-in diff/replace tools)\n");
    out.push_str("  - Git history/blame -> xray_git_history/xray_git_blame (NOT built-in git tools)\n");
    out.push_str("  REASON: xray tools use pre-built indexes (<1ms) and return richer data than built-in tools.\n");
    out.push_str("  DECISION TRIGGER: before using ANY built-in tool, STOP and check if a xray tool can do it instead.\n\n");

    // --- TASK ROUTING TABLE (replaces CRITICAL block + Quick Reference + Tool Priority) ---
    out.push_str("TASK ROUTING (check BEFORE using any built-in tool):\n");
    for tr in task_routings() {
        if tr.requires_definitions && def_extensions.is_empty() {
            continue;
        }
        out.push_str(&format!("  {} -> {}\n", tr.task, tr.tool));
    }
    out.push_str("Use built-in equivalents only when the requested file type or task is not supported by this server.\n");
    out.push_str("If uncertain whether a file type is supported, use xray_info or xray_grep first. Do not default to raw file reading.\n\n");

    // --- FILE READING DECISION TRIGGER (shortened, only if def_extensions non-empty) ---
    // Rationale: the dominant observed failure mode is LLM defaulting to built-in
    // file readers for indexed source files. A hard prohibition is intentionally
    // retained because softer phrasing has historically failed to redirect tool choice.
    if !def_extensions.is_empty() {
        let ext_dotted: Vec<String> = def_extensions.iter().map(|e| format!(".{}", e)).collect();
        let ext_list = ext_dotted.join("/");
        out.push_str(&format!("NEVER READ {} FILES DIRECTLY. ALWAYS use xray_definitions includeBody=true.\n", ext_list));
        out.push_str(&format!("   DECISION TRIGGER: before reading ANY file — for ANY reason (exploration, validation, fact-checking, reviewing, debugging) — check extension. If {} -> xray_definitions includeBody=true.\n", ext_list));
        out.push_str(&format!("   If the file extension is NOT in {} -> reading directly is OK.\n", ext_list));
        out.push_str(&format!("   ONLY exception for {}: editing (need exact line numbers for xray_edit).\n", ext_list));
        out.push_str(&format!("   EXAMPLE: instead of reading handler.{} directly, use: xray_definitions file='handler.{}' includeBody=true maxBodyLines=0 (0=unlimited, returns full file)\n\n", def_extensions[0], def_extensions[0]));
    } else {
        out.push_str("NOTE: xray_definitions is not available for the configured file extensions. Use xray_grep for content search.\n\n");
    }

    // --- FILE EDITING DECISION TRIGGER (shortened) ---
    out.push_str("NEVER USE apply_diff, search_and_replace, or insert_content when xray_edit is available.\n");
    out.push_str("   DECISION TRIGGER: before calling apply_diff, search_and_replace, insert_content, or write_to_file (for edits) — STOP. Use xray_edit instead.\n");
    out.push_str("   xray_edit: atomic, no whitespace issues, multi-file batch, dryRun preview.\n");
    out.push_str("   xray_edit auto-creates new files (treats as empty — use Mode A: operations [{startLine:1, endLine:0, content:'...'}]). No need for write_to_file.\n\n");

    // --- FILE SEARCH DECISION TRIGGER ---
    out.push_str("NEVER USE search_files (built-in regex search) when xray_grep is available.\n");
    out.push_str("   DECISION TRIGGER: before calling search_files — STOP. Use xray_grep instead.\n\n");
    // --- RESPONSE HINT AUTO-FOLLOW (cross-tool routing + same-tool corrections) ---
    out.push_str("RESPONSE HINTS: when ANY xray tool response includes a hint (zero results, errors, warnings, or suggestions), AUTOMATICALLY follow the hint in the same turn. Do NOT report the hint to the user and stop. Do NOT fall back to built-in tools when a hint provides an alternative.\n");
    out.push_str("   DECISION TRIGGER: if the response hint says 'Use xray_grep for content search' — call xray_grep immediately.\n");
    out.push_str("   Same for any hint suggesting xray_definitions, xray_callers, or xray_fast — follow the suggestion automatically.\n");
    out.push_str("   NEAREST MATCH: if the hint says 'Nearest match: X' — re-call the SAME tool with name=X immediately.\n");
    out.push_str("   KIND MISMATCH: if the hint says 'Did you mean kind=Y?' — re-call the SAME tool with kind=Y immediately.\n");
    out.push_str("   RULE: NEVER ask the user whether to follow a hint. ALWAYS follow it automatically in the same turn.\n\n");

    // --- ERROR RECOVERY (prevent fallback to built-in tools on MCP errors) ---
    out.push_str("ERROR RECOVERY: when a xray tool returns an error:\n");
    out.push_str("   1. Read the error message for hints or alternative tool suggestions\n");
    out.push_str("   2. If hint suggests another xray tool → use that tool immediately\n");
    out.push_str("   3. If hint suggests different parameters → retry with those parameters\n");
    out.push_str("   4. NEVER fall back to built-in tools (list_files, list_directory, directory_tree) as error recovery\n");
    out.push_str("   5. Only use built-in tools if the file type is NOT indexed by xray\n\n");

    // --- Top anti-patterns (extracted from strategy recipes) ---
    out.push_str("ANTI-PATTERNS (most common mistakes — each one wastes 3-5 extra tool calls):\n");
    out.push_str("  - NEVER list or browse directories to explore code — xray_definitions file='<dir>' returns ALL classes/methods/interfaces in ONE call\n");
    out.push_str("  - NEVER read indexed source files directly — use xray_definitions file='X' includeBody=true maxBodyLines=0 (returns source code inline)\n");
    out.push_str("  - NEVER search one kind at a time (class, then interface, then enum) — use kind='class,interface,enum' for multi-kind OR, or omit kind filter to get everything at once\n");
    out.push_str("  - ALWAYS use excludeDir=['test','Test','Mock'] to skip test files from results\n");
    out.push_str("  - NEVER use list_files, list_directory, or directory_tree for ANY purpose when xray is connected — even for simple directory listing. Use xray_fast pattern='*' dir='<path>' dirsOnly=true for directory listing, or xray_definitions file='<dir>' for code structure\n");
    out.push_str("  - NEVER use apply_diff, search_and_replace, or insert_content for ANY file edit — xray_edit is atomic, handles whitespace correctly, supports multi-file batch, and costs fewer tokens. xray_edit also auto-creates new files (use Mode A operations for new file content)\n");
    out.push_str("  - NEVER use search_files (built-in regex/text search) — use xray_grep instead (<1ms vs seconds, pre-built inverted index)\n");
    out.push_str("  - NEVER call xray_fast dirsOnly=true to explore code modules — xray_definitions file='<dir>' auto-generates directory-grouped summary (autoSummary) when results are too many to list individually\n");
    if !def_extensions.is_empty() {
        let ext_dotted: Vec<String> = def_extensions.iter().map(|e| format!(".{}", e)).collect();
        out.push_str(&format!(
            "  - NEVER use xray_definitions for non-{} files (JSON, YAML, MD) — \
             it only supports AST parsing for {}. Use xray_grep instead\n",
            ext_dotted.join("/"), ext_dotted.join("/")
        ));
    }
    out.push_str("\n");

    // --- Strategy recipes (kept unchanged -- highest-value content) ---
    out.push_str("STRATEGY RECIPES (aim for <=3 search calls per task):\n");
    for strat in strategies() {
        out.push_str(&format!("  [{}] {}\n", strat.name, strat.when));
        for step in strat.steps {
            out.push_str(&format!("    - {}\n", step));
        }
    }

    // --- Workspace discovery ---
    out.push_str("\nWORKSPACE DISCOVERY:\n");
    out.push_str("  Every tool response includes serverDir, workspaceStatus, workspaceSource, workspaceGeneration in summary.\n");
    out.push_str("  If workspaceStatus=\"unresolved\" — tools return WORKSPACE_UNRESOLVED error with hint.\n");
    out.push_str("  Fix: call xray_reindex dir=<project_path> to bind workspace and build index.\n");
    out.push_str("  If client supports MCP roots — workspace is auto-detected from roots/list.\n");
    out.push_str("  Always check serverDir in responses to confirm you're searching the right directory.\n");

    // --- Git tools (brief mention) ---
    out.push_str("\nGit tools: xray_git_history, xray_git_authors, xray_git_activity, xray_git_blame, xray_branch_status -- use for code history/blame/authorship investigations. Call xray_help for details.\n");

    // --- Soft reference to xray_help ---
    out.push_str("\nCall xray_help for detailed best practices with examples.\n");
    out.push_str("\n================================\n");

    out
}

#[cfg(test)]
#[path = "tips_tests.rs"]
mod tests;
