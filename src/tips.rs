//! Single source of truth for best practices and tips.
//! Used by: CLI `xray tips`, MCP `xray_help` tool, MCP `instructions` field.

use std::borrow::Cow;
use serde_json::{json, Value};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LanguageProfile {
    pub content_extensions: Vec<String>,
    pub definition_extensions: Vec<String>,
    pub has_csharp_defs: bool,
    pub has_typescript_defs: bool,
    pub has_rust_defs: bool,
    pub has_sql_defs: bool,
    pub has_html_content: bool,
    pub has_scss_content: bool,
    pub has_xml_content: bool,
    pub has_xml_on_demand: bool,
}

impl LanguageProfile {
    #[cfg(test)]
    pub fn new(content_extensions: &[&str], definition_extensions: &[&str]) -> Self {
        let content_extensions = normalize_extensions(content_extensions.iter().copied());
        let definition_extensions = normalize_extensions(definition_extensions.iter().copied());
        let has_xml_on_demand = content_extensions
            .iter()
            .any(|ext| xml_on_demand_extension_supported(ext));
        Self::from_normalized(content_extensions, definition_extensions, has_xml_on_demand)
    }

    pub fn new_with_xml_on_demand(
        content_extensions: &[&str],
        definition_extensions: &[&str],
        xml_on_demand_available: bool,
    ) -> Self {
        let content_extensions = normalize_extensions(content_extensions.iter().copied());
        let definition_extensions = normalize_extensions(definition_extensions.iter().copied());
        Self::from_normalized(
            content_extensions,
            definition_extensions,
            xml_on_demand_enabled(xml_on_demand_available),
        )
    }

    fn from_normalized(
        content_extensions: Vec<String>,
        definition_extensions: Vec<String>,
        has_xml_on_demand: bool,
    ) -> Self {
        let has_csharp_defs = has_ext(&definition_extensions, "cs");
        let has_typescript_defs = has_ext(&definition_extensions, "ts") || has_ext(&definition_extensions, "tsx");
        let has_rust_defs = has_ext(&definition_extensions, "rs");
        let has_sql_defs = has_ext(&definition_extensions, "sql");
        let has_html_content = has_ext(&content_extensions, "html");
        let has_scss_content = has_ext(&content_extensions, "scss") || has_ext(&content_extensions, "css");
        let has_xml_content = content_extensions
            .iter()
            .any(|ext| xml_on_demand_extension_supported(ext));

        Self {
            content_extensions,
            definition_extensions,
            has_csharp_defs,
            has_typescript_defs,
            has_rust_defs,
            has_sql_defs,
            has_html_content,
            has_scss_content,
            has_xml_content,
            has_xml_on_demand,
        }
    }

    fn definition_extension_refs(&self) -> Vec<&str> {
        self.definition_extensions.iter().map(String::as_str).collect()
    }
}

fn normalize_extensions<'a>(extensions: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut normalized = Vec::new();
    for ext in extensions {
        let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
        if !ext.is_empty() && !normalized.iter().any(|existing| existing == &ext) {
            normalized.push(ext);
        }
    }
    normalized
}

fn has_ext(extensions: &[String], wanted: &str) -> bool {
    extensions.iter().any(|ext| ext == wanted)
}

fn xml_on_demand_extension_supported(extension: &str) -> bool {
    #[cfg(feature = "lang-xml")]
    {
        crate::definitions::parser_xml::is_xml_extension(extension)
    }
    #[cfg(not(feature = "lang-xml"))]
    {
        let _ = extension;
        false
    }
}

fn xml_on_demand_enabled(available: bool) -> bool {
    #[cfg(feature = "lang-xml")]
    {
        available
    }
    #[cfg(not(feature = "lang-xml"))]
    {
        let _ = available;
        false
    }
}

/// Canonical Mode A example (line-range splice) for `xray_edit`. Single source
/// of truth shared by:
///   - `xray_edit` rejection-error hints (when caller sends an invented
///     top-level wrapper like `files` / `batch` / `targets`),
///   - the per-tool `xray_help tool="xray_edit"` payload.
///
/// Keeping both consumers on this constant prevents the help text and the
/// error hint from drifting apart.
pub const CANONICAL_MODE_A_EXAMPLE: &str =
    "{\"path\": \"src/foo.rs\", \"operations\": [{\"startLine\": 5, \"endLine\": 5, \"content\": \"new line\\n\"}]}";

/// Canonical Mode B example (text find/replace). Sibling of
/// `CANONICAL_MODE_A_EXAMPLE`; same single-source-of-truth contract.
pub const CANONICAL_MODE_B_EXAMPLE: &str =
    "{\"path\": \"src/foo.rs\", \"edits\": [{\"search\": \"old\", \"replace\": \"new\"}]}";

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
// TaskRouting struct and task_routings() function were removed as part of
// the 2026-04-17 prompt slimming (Part 4 audit). TASK ROUTING was a 1:1 duplicate
// of INTENT -> TOOL MAPPING in render_instructions — merged into the mapping.
pub fn tips(def_extensions: &[String]) -> Vec<Tip> {
    let lang_list = format_supported_languages(def_extensions);
    let has_sql = def_extensions.iter().any(|ext| ext == "sql");
    let mut all = vec![
        Tip {
            rule: "File lookup: use xray_fast, not built-in list_files".into(),
            why: "xray_fast uses a pre-built index (~35ms). Built-in list_files does a live filesystem walk (~3s). 90x+ faster.".into(),
            example: "xray_fast with pattern=[\"UserService\"] instead of built-in list_files".into(),
        },
        Tip {
            rule: "Multi-term OR: find all variants in ONE query".into(),
            why: "Comma-separated terms with mode='or' finds files containing ANY term. Much faster than separate queries.".into(),
            example: "xray grep \"UserService,IUserService,UserServiceFactory\" -e cs  |  MCP: terms=[\"UserService\",\"IUserService\",\"UserServiceFactory\"], mode='or'".into(),
        },
        Tip {
            rule: "AND mode: find files containing ALL terms".into(),
            why: "mode='and' finds files where ALL comma-separated terms co-occur. Useful for finding DI registrations.".into(),
            example: "xray grep \"ServiceProvider,IUserService\" -e cs --all  |  MCP: terms=[\"ServiceProvider\",\"IUserService\"], mode='and'".into(),
        },
        Tip {
            rule: "Substring search is ON by default".into(),
            why: "xray_grep defaults to substring=true so compound identifiers (IUserService, m_userService) are always found. Use substring=false for exact-token-only matching. Auto-disabled when regex or phrase is used. Short tokens (<4 chars) may produce broad results -- use exclude=['pattern'] to filter noise from file paths. Comma-separated phrases with spaces are searched independently as OR (or AND with mode='and').".into(),
            example: "Default: terms=[\"UserService\"] finds IUserService, m_userService. Multi-phrase OR: terms=[\"fn handle_foo\",\"fn build_bar\"]. Exact only: terms=[\"UserService\"], substring=false".into(),
        },
        Tip {
            rule: "Phrase search: literal match on raw file content (XML, config, any punctuation)".into(),
            why: "phrase=true matches the exact string in raw file content -- including XML tags, angle brackets, slashes, dots. No escaping needed. Ideal for XML/config search. Slower (~80ms) but precise.".into(),
            example: "xray grep \"<MaxRetries>3</MaxRetries>\" --phrase  |  MCP: terms=[\"<MaxRetries>3</MaxRetries>\"], phrase=true. Also: terms=[\"new HttpClient\"], phrase=true".into(),
        },
        Tip {
            rule: "Regex pattern search".into(),
            why: "Full regex for pattern matching. Also works in xray_definitions name parameter.".into(),
            example: "xray grep \"I[A-Z]\\w+Cache\" -e cs --regex  |  MCP: terms=[\"I[A-Z]\\w+Cache\"], regex=true".into(),
        },
        Tip {
            rule: "lineRegex is more expensive than substring (trigram prefilter is best-effort) -- use terms=[...] when possible".into(),
            why: "lineRegex compiles a regex per pattern and (when the AC-4 prefilter cannot help) reads every file that survives file/ext/dir filtering; for each, a whole-content precheck runs first, then a per-line scan on the files that pass. Since 2026-04-26 a literal-trigram prefilter automatically narrows the candidate file set when the regex contains a fixed-substring literal of >=3 chars (see the next tip + `summary.literalPrefilter`), but unprefilterable patterns (`\\d+`, `.*`, `\\w+`, OR with any unprefilterable branch) and >50% candidate-ratio cases still fall back to the full per-file scan and can take tens of seconds on large repos. Substring search via terms=[...] uses the trigram index directly and answers in <10ms -- typically ~1000x faster. lineRegex is only the right tool when anchors (^/$), char classes, or lookarounds are essential to the match. If you can express the search as one or more fixed substrings, drop lineRegex.".into(),
            example: "BAD: terms=[\"App.*=.*\\\\d\"] lineRegex=true (full-scan, ~50s on 60k files). GOOD: terms=[\"App\"] (trigram-prefiltered, <5ms; client-side regex over the few result files). When anchors are essential: ALWAYS narrow with file=/dir=/ext=. Response includes a `perfHint` when a slow lineRegex scan is detected (helper gates on elapsed >=2s and indexed-corpus size >=1k files; actual scan set is the post-filter subset).".into(),
        },

        Tip {
            rule: "lineRegex auto-prefilters by extracted literals when possible (AC-4)".into(),
            why: "Since 2026-04-26, lineRegex regex patterns whose AST exposes a fixed-substring literal (e.g. `App\\s*=\\s*\\d+` -> literal `App`; `OrgAppTypeId|AppType\\d+` -> literals `OrgAppTypeId`/`AppType`) are first run through the trigram index to shrink the candidate file set before the per-line scan. Pure substrings still beat this prefilter (no regex compilation, no AST walk), but for unavoidable regex/anchor queries the prefilter can eliminate the bulk of the corpus when a discriminating literal exists (measured speedups belong in docs/measurements/ -- not promised here). Skipped when the regex has no extractable literal (`\\d+`, `.*`, `\\w+`), when OR composition mixes a prefilterable and unprefilterable pattern, or when the candidate set still covers >50% of the index (short-circuit). Inspect the per-call decision via `summary.literalPrefilter`: `used` (bool), `candidateFiles`/`totalFiles`, `extractedFragments` (lowercased word fragments fed to the trigram lookup), `shortCircuited` (only when present), `reason` (only when not used).".into(),
            example: "FAST (prefiltered): terms=[\"App\\\\s*=\\\\s*\\\\d+\"] lineRegex=true regex=true -> literalPrefilter={used:true, candidateFiles<<totalFiles, extractedFragments:['app']} (fragments are lowercased). SLOW (unprefilterable): terms=[\"\\\\d+\"] lineRegex=true regex=true -> literalPrefilter={used:false, extractedFragments:[], reason:'no pattern has extractable literals'}. SHORT-CIRCUIT: terms=[\"App\"] lineRegex=true regex=true (every file has 'app') -> literalPrefilter={used:false, shortCircuited:true, reason:'candidate set covers N/M files (>50% threshold)'}.".into(),
        },
        Tip {
            rule: "Exclude test/mock dirs for production-only results".into(),
            why: "Half the results are often test files. Use excludeDir to filter them out.".into(),
            example: "--exclude-dir test --exclude-dir Mock  |  MCP: excludeDir=['test','Mock','UnitTests']".into(),
        },
        Tip {
            rule: "Call chain tracing: xray_callers (up and down)".into(),
            why: "Single sub-millisecond request replaces 7+ sequential grep + read_file calls. direction='up' (callers) or 'down' (callees). For Angular: direction='down' with class shows template children, direction='up' with selector finds parent components.".into(),
            example: "MCP: xray_callers method=[\"GetUserAsync\"], class='UserService', depth=2, direction='up'".into(),
        },
        Tip {
            rule: "Always specify class in xray_callers".into(),
            why: "Without class, results mix callers from ALL classes with same method name. Misleading call trees.".into(),
            example: "MCP: xray_callers method=[\"ExecuteAsync\"], class='OrderProcessor'".into(),
        },
        Tip {
            rule: "xray_callers 0 results? Try the interface name".into(),
            why: "Calls through DI use the interface (IUserService), not the implementation (UserService). xray_callers matches the receiver type recorded at call sites. If you search class='UserService' but callers use IUserService, you get 0 results.".into(),
            example: "0 callers with class='UserService' -> retry with class='IUserService'. Also: resolveInterfaces=true auto-resolves implementations".into(),
        },
        Tip {
            rule: "Stack trace analysis: containsLine".into(),
            why: "Given file + line number, returns innermost method AND parent class. No manual read_file needed.".into(),
            example: "MCP: xray_definitions file=[\"UserService.cs\"], containsLine=42".into(),
        },
        Tip {
            rule: "Read method source: use includeBody=true instead of reading files".into(),
            why: "xray_definitions with includeBody=true returns method body inline, eliminating read_file round-trips. BEFORE reading any indexed source file, try xray_definitions with includeBody=true first. Use maxBodyLines/maxTotalBodyLines for budget. Only read files directly for content that xray cannot parse/search, or when you need exact line numbers for editing.".into(),
            example: "MCP: xray_definitions parent=[\"UserService\"], includeBody=true, maxBodyLines=20. Also: xray_definitions name=[\"Program\",\"Startup\",\"OrderService\"] includeBody=true -> reads multiple classes at once, faster than multiple read_file calls".into(),
        },
        Tip {
            rule: "Known-symbol shortcut: read the body with xray_definitions".into(),
            why: "Once the class/method/procedure/function name is known, use xray_definitions includeBody=true instead of a raw file read. Add parent=[\"Y\"] or file=[\"path\"] for common names, overloads, traits/impls, and constructors.".into(),
            example: "MCP: xray_definitions name=[\"ProcessOrder\"] parent=[\"OrderService\"] includeBody=true maxBodyLines=0. Or: file=[\"src/order.rs\"] name=[\"parse\"] includeBody=true maxBodyLines=0".into(),
        },
        Tip {
            rule: "Body budgets: manage with maxBodyLines and maxTotalBodyLines".into(),
            why: "Default limits: 100 lines/def, 500 total. Strategies: (1) Wide overview: includeBody=true maxBodyLines=30 -- gets first 30 lines of many methods. (2) Targeted read: name=[\"SpecificMethod\"] includeBody=true maxBodyLines=0 -- full body of one method. (3) Unlimited: maxBodyLines=0 maxTotalBodyLines=0 -- removes all limits (large response). If response contains definitions with 'bodyOmitted', narrow with name=[\"<specific names>\"] to read them individually.".into(),
            example: "Overview: includeBody=true maxBodyLines=30. Targeted: name=[\"ProcessOrder\"] includeBody=true maxBodyLines=0. Unlimited: maxBodyLines=0, maxTotalBodyLines=0. After bodyOmitted: name=[\"OmittedMethod1\",\"OmittedMethod2\"] includeBody=true".into(),
        },
        Tip {
            rule: "Reconnaissance: use countOnly=true".into(),
            why: "xray_grep with countOnly=true returns ~46 tokens (counts only) vs 265+ for full results. Perfect for 'how many files use X?'.".into(),
            example: "xray grep \"HttpClient\" -e cs --count-only  |  MCP: terms=[\"HttpClient\"], countOnly=true".into(),
        },
        Tip {
            rule: "Search ANY indexed file type: XML, csproj, config, etc.".into(),
            why: "xray_grep works with all file extensions passed to --ext. Use ext=[\"csproj\"] to find NuGet dependencies, ext=[\"xml\",\"config\",\"manifestxml\"] for configuration values.".into(),
            example: "xray grep \"Newtonsoft.Json\" -e csproj  |  MCP: terms=[\"Newtonsoft.Json\"], ext=[\"csproj\"]".into(),
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
            example: "xray_definitions name=[\"UserService\",\"IUserService\",\"UserController\"] -> finds ALL matching definitions in one call".into(),
        },
        Tip {
            rule: "Query budget: aim for 3 or fewer search calls per exploration task".into(),
            why: "Each search call adds latency and LLM context. Use multi-term queries, includeBody, and combined filters to minimize round-trips. Most architecture questions can be answered in 1-3 calls.".into(),
            example: "Step 1: xray_definitions name=[\"OrderService\",\"IOrderService\"] includeBody=true (map + read). Step 2: xray_callers method=[\"ProcessOrder\"] class='OrderService' (call chain). Done in 2 calls.".into(),
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
            example: "xray_callers misses delegate usage -> use xray_grep terms=[\"IsValid\"] ext=[\"cs\"] to find all references including method group passes".into(),
        },
        Tip {
            rule: "Impact analysis: find which tests cover a method".into(),
            why: "impactAnalysis=true in xray_callers traces callers upward and identifies test methods (via [Test]/[Fact]/[Theory]/[TestMethod]/#[test] attributes or *.spec.ts/*.test.ts files). Returns a 'testsCovering' array with all tests in the call chain. Use depth=5-7 for deep call chains. One call replaces manual multi-step investigation.".into(),
            example: "MCP: xray_callers method=[\"SaveOrder\"] class='OrderService' direction='up' depth=5 impactAnalysis=true -> testsCovering: [{method: 'TestSaveOrder', class: 'OrderTests', file: 'src/OrderTests.cs', depth: 1, callChain: ['SaveOrder', 'TestSaveOrder']}]".into(),
        },
        Tip {
            rule: "using static: search by defining class, not consuming class".into(),
            why: "xray_definitions searches AST definition names. Methods imported via C# 'using static' are defined in their original class. Searching parent=[\"ConsumingClass\"] will return 0 results. Search without parent filter or with parent=[\"DefiningClass\"] instead.".into(),
            example: "Percentile() imported via 'using static PercentileHelper' -> xray_definitions name=[\"Percentile\"] (without parent) or parent=[\"PercentileHelper\"]".into(),
        },
        Tip {
            rule: "Deleted files: xray_git_* tools cover them fully".into(),
            why: "xray_git_history auto-falls back from --follow when a file is deleted; xray_git_activity supports includeDeleted=true. Empty results carry warning ('never existed') vs info ('deleted') -- both are definitive. Raw `git log --all --diff-filter=D` is a policy violation.".into(),
            example: "xray_git_history file='src/old/Legacy.cs' returns full history incl. deletion commit; xray_git_activity from='2024-01-01' includeDeleted=true lists removals.".into(),
        },
        Tip {
            rule: "Trivial task != trivial policy check -- ALWAYS pause before built-in".into(),
            why: "The most common policy violation is on tasks that FEEL trivial (quick validation, simple fact-check, one-line read). LLM skips MANDATORY PRE-FLIGHT CHECK because 'it's just a quick search'. But quick searches are EXACTLY where xray_grep shines (countOnly=true, <1ms). Habit-driven tool selection based on tool name matching intent keyword is the #1 policy break.".into(),
            example: "User asks 'validate that code has no X'. Reaching for search_files because intent contains 'search' -- VIOLATION. Correct: xray_grep terms=[\"X\"] countOnly=true.".into(),
        },
    ];

    if has_sql {
        all.push(Tip {
            rule: "SQL signature is a summary, not the source of truth".into(),
            why: "For stored procedures, the structured signature field is a convenience summary. Exact parameter types, defaults, OUTPUT markers, and formatting must be verified from the procedure body header.".into(),
            example: "MCP: xray_definitions kind=[\"storedProcedure\"] name=[\"usp_Foo\"] includeBody=true bodyLineStart=<line_start> bodyLineEnd=<line_start+40>".into(),
        });
        all.push(Tip {
            rule: "Stale def-index after parser fixes: reindex definitions".into(),
            why: "Definition metadata is cached. After shipping or testing a parser/signature fix, old structured metadata remains until the definition index is rebuilt.".into(),
            example: "MCP: xray_reindex_definitions ext=[\"sql\"] before validating stored procedure signatures again.".into(),
        });
    }

    all
}

pub fn strategies() -> Vec<Strategy> {
    vec![
        Strategy {
            name: "Architecture Exploration",
            when: "User asks 'how is X structured', 'explain module X', or 'show me the architecture of X'",
            steps: &[
                "Step 1 - Map the landscape (1 call): xray_definitions file=[\"<dirname>\"] excludeDir=['test','Test','Mock'] -> if many results, returns autoSummary with directory grouping and top classes per subdirectory. If few results, returns full definition list. Use file=[\"<subdir>\"] to drill into specific groups",
                "Step 2 - Read key implementations (1 call): xray_definitions name=[\"<top 3-5 key classes from step 1>\"] includeBody=true maxBodyLines=30 -> returns source code of the most important files",
                "Step 3 (optional) - Scope and dependencies (1 call): xray_grep terms=[\"X\"] countOnly=true -> scale (how many files, occurrences); or xray_fast pattern=[\"X\"] dirsOnly=true -> directory structure",
            ],
            anti_patterns: &[
                "Don't use list_files + read_file to explore architecture -- xray_definitions returns classes, methods, file paths, and source code in ONE call",
                "Don't read indexed source files to see source code -- xray_definitions includeBody=true returns it directly. read_file is ONLY for non-indexed files (markdown, JSON, XML, config) or for editing (need exact line numbers)",
                "Don't search one kind at a time (class, then interface, then enum) -- use kind=[\"class\",\"interface\",\"enum\"] for multi-kind OR, or omit kind filter to get everything at once",
                "Don't use countOnly first then re-query with body -- go straight to includeBody=true with maxBodyLines",
                "Don't browse directories (list_files, list_directory, xray_fast with empty pattern) to understand code -- xray_definitions file=[\"<dir>\"] returns ALL definitions with file paths in ONE call. For large modules, autoSummary kicks in with directory grouping",
                "Don't search for file names separately if xray_definitions already found them (results include file paths)",
                "Don't make separate queries for ClassName and IClassName -- use comma-separated: name=[\"ClassName\",\"IClassName\"]",
            ],
        },
        Strategy {
            name: "Call Chain Investigation",
            when: "User asks 'who calls X', 'trace how X is invoked', or 'show the call chain for X'",
            steps: &[
                "Step 1 - Get call tree with source (1 call): xray_callers method=[\"MethodName\"] class='ClassName' depth=3 direction='up' includeBody=true -> full caller hierarchy WITH method source code inline",
                "Step 2 (optional, only if more context needed): xray_definitions name=[\"<specific callers from step 1>\"] includeBody=true maxBodyLines=0 -> full source of specific methods",
            ],
            anti_patterns: &[
                "Don't omit the class parameter -- without it, results mix callers from ALL classes with the same method name",
                "Don't use xray_grep to manually find callers -- xray_callers does it in sub-millisecond with DI/interface resolution",
                "Don't call xray_callers then xray_definitions separately to get source -- use includeBody=true in xray_callers to get both in ONE call",
            ],
        },
        Strategy {
            name: "Code Flow Investigation (end-to-end)",
            when: "User asks 'explain this flow', 'trace X end-to-end', or 'investigate how X works'",
            steps: &[
                "Step 1 - Locate candidates: xray_fast pattern=[\"<name>\"] and xray_grep terms=[\"<term>\"] -> candidate files / call sites",
                "Step 2 - List structure before bodies: xray_definitions file=[\"<dir-or-file>\"] -> classes/methods/functions/procedures in scope",
                "Step 3 - Read the known target body: xray_definitions name=[\"<symbol>\"] parent=[\"<owner>\"] includeBody=true maxBodyLines=0, or file=[\"<known-path>\"] name=[\"<symbol>\"] includeBody=true maxBodyLines=0",
                "Step 4 - Trace edges: xray_callers method=[\"<X>\"] class='<Y>' direction='up'|'down' includeBody=true",
                "Step 5 - Wider file context only when needed: imports/usings, fields, neighbouring methods, generated regions, partial-class layout, non-indexed files, or exact edit line ranges",
            ],
            anti_patterns: &[
                "Don't use read_file at step 3 -- the symbol name is known, so xray_definitions includeBody=true is the body read",
                "Don't skip step 2 and jump straight to a common name like new/parse/Execute/Handle -- you risk reading the wrong overload or impl",
            ],
        },
        Strategy {
            name: "SQL Stored Procedure Investigation",
            when: "User asks what a stored procedure does, how EXEC chains flow, or whether parameters/types are correct",
            steps: &[
                "Step 1 - Find the procedure file: xray_fast pattern=[\"usp_Foo\"] ext=[\"sql\"]",
                "Step 2 - Read the procedure body: xray_definitions kind=[\"storedProcedure\"] name=[\"usp_Foo\"] includeBody=true maxBodyLines=0",
                "Step 3 - Trace SP-to-SP calls: xray_callers method=[\"usp_Foo\"] class='dbo' direction='down' includeBody=true",
                "Step 4 - If signature disagrees with the body header, run xray_reindex_definitions ext=[\"sql\"] and repeat the validation",
            ],
            anti_patterns: &[
                "Don't trust the structured signature field for exact SQL parameter types/defaults -- verify the body header",
                "Don't omit class for SQL call chains -- class is the schema name, for example 'dbo'",
            ],
        },
        Strategy {
            name: "Rust Code Flow Investigation",
            when: "User asks to trace Rust methods/functions, impl flows, or test coverage",
            steps: &[
                "Step 1 - Read the exact body: xray_definitions name=[\"foo\"] parent=[\"TypeName\"] includeBody=true maxBodyLines=0; for free functions omit parent or add file=[\"path.rs\"]",
                "Step 2 - Trace callers/callees: xray_callers method=[\"foo\"] class='TypeName' direction='up'|'down' includeBody=true",
                "Step 3 - Find covering tests: xray_callers method=[\"foo\"] class='TypeName' direction='up' impactAnalysis=true depth=5",
            ],
            anti_patterns: &[
                "Don't read common Rust method names without parent or file -- new/from/fmt/default/parse/handle are usually ambiguous",
                "Don't use grep as the first caller-chain tool -- xray_callers has the Rust call graph and can include bodies",
            ],
        },
        Strategy {
            name: "Stack Trace / Bug Investigation",
            when: "User provides a stack trace, error at file:line, or asks 'what method is at line N'",
            steps: &[
                "Step 1 - Identify method (1 call): xray_definitions file=[\"FileName.cs\"] containsLine=42 includeBody=true -> returns the method body (innermost) + parent class metadata (body omitted to save budget)",
                "Step 2 (optional) - Trace callers (1 call): xray_callers method=[\"<method from step 1>\"] class='<class from step 1>' depth=2 -> who triggered this code path",
            ],
            anti_patterns: &[
                "Don't use read_file to manually scan for the method -- containsLine finds it instantly with proper class context",
                "Don't guess the method name from the stack trace -- use containsLine for precise AST-based lookup",
            ],
        },
        Strategy {
            name: "Deleted File Archaeology",
            when: "User asks 'when was this file deleted', 'who removed it', 'show history of a file that no longer exists', or 'what files were deleted in range X'",
            steps: &[
                "Step 1 - History of a deleted path: xray_git_history repo='.' file='<path>' (auto --follow fallback returns the deletion commit). warning='never existed' = wrong path, NOT a limitation",
                "Step 2 - List deletions in a range: xray_git_activity repo='.' from='<date>' includeDeleted=true (one git ls-files spawn, no per-file overhead)",
                "Step 3 (optional) - Owners: xray_git_authors repo='.' path='<path>'",
            ],
            anti_patterns: &[
                "Don't run raw `git log --all --diff-filter=D` -- xray_git_history + includeDeleted already cover this. Empty results with warning/info are definitive answers.",
            ],
        },
        Strategy {
            name: "Code History Investigation",
            when: "User asks 'when was this bug introduced', 'who changed this file', or 'trace the origin of this code'",
            steps: &[
                "Step 1 - Verify branch (1 call): xray_branch_status repo='.' -> confirm you're on main and data is fresh",
                "Step 2 - Find where code lives (1 call): xray_grep terms=[\"<error text>\"] ext=[\"cs\"] -> find file and line number",
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
                "Step 1 (1 call): xray_callers method=[\"<selector>\"] direction='up' depth=3 -> finds parent AND grandparent components recursively via templateChildren (<1ms). Parents nested in 'parents' field.",
                "Step 2 (optional): xray_callers method=[\"<class-name>\"] direction='down' depth=2 -> shows child components used in template (recursive)",
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
                "Step 2 (optional) - Narrow to module (1 call): xray_definitions file=[\"Services\"] minComplexity=10 sortBy='cyclomaticComplexity' -> complex methods in specific directory",
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
                 xray_definitions file=[\"<files mentioned in story/PR>\"] includeBody=false \
                 -> list all definitions to understand current architecture",
                "Step 2 - Validate specific code (1 call): \
                 xray_definitions name=[\"<specific functions referenced>\"] includeBody=true maxBodyLines=0 \
                 -> read actual code to validate claims and code sketches",
                "Step 3 (optional) - Verify scale (1 call): \
                 xray_grep terms=[\"<patterns discussed>\"] countOnly=true \
                 -> confirm how widespread a pattern is",
            ],
            anti_patterns: &[
                "Don't read entire files with read_file to understand architecture \
                 -- xray_definitions file=[\"...\"] gives a structured view of all definitions",
                "Don't read files just to count occurrences \
                 -- xray_grep countOnly=true is instant",
                "Don't read files to check if a function exists \
                 -- xray_definitions name=[\"...\"] answers in <1ms",
            ],
        },
    ]
}

pub fn strategies_for_extensions(def_extensions: &[String]) -> Vec<Strategy> {
    let has_sql = def_extensions.iter().any(|ext| ext == "sql");
    let has_rust = def_extensions.iter().any(|ext| ext == "rs");
    let has_typescript = def_extensions
        .iter()
        .any(|ext| ext == "ts" || ext == "tsx");

    strategies()
        .into_iter()
        .filter(|strategy| match strategy.name {
            "SQL Stored Procedure Investigation" => has_sql,
            "Rust Code Flow Investigation" => has_rust,
            "Angular Component Hierarchy (TypeScript only)" => has_typescript,
            _ => true,
        })
        .collect()
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
        ToolPriority { rank: 6, tool: "xray_edit", description: "reliable file editing -- line-range or text-match, atomic, no whitespace issues. Supports multi-file (paths), insert after/before, expectedContext. Sync reindex after write -- xray_grep / xray_definitions / xray_callers / xray_fast see new content immediately, no 500ms wait".into() },
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
            "name": "Single: [\"UserService\"]. Multi-term OR: [\"UserService\",\"IUserService\",\"UserController\"] (finds ALL in one query). Naming variants: [\"Order\",\"IOrder\",\"OrderFactory\"]. NOTE: name searches AST definition names (classes, methods, properties) -- NOT string literals or values inside code. Use xray_grep for string content. For methods via 'using static', search without parent filter",
            "containsLine": "file=[\"UserService.cs\"], containsLine=42 -> returns GetUserAsync (lines 35-50), parent: UserService. With includeBody=true, body is emitted ONLY for innermost definition; parent gets 'bodyOmitted' hint (saves body budget for target method)",
            "bodyLineStart/End": "containsLine=1335, bodyLineStart=1330, bodyLineEnd=1345 -> returns only 15 lines of a 363-line method body, avoiding response truncation. Use when you know which lines you need from a large method.",
            "includeBody": "parent=[\"UserService\"], includeBody=true, maxBodyLines=20 -> returns method bodies inline. When body is truncated, summary includes totalBodyLinesAvailable — use it to calibrate maxTotalBodyLines for retry",
            "includeDocComments": "includeDocComments=true -> expands body upward to capture /// XML doc-comments (C#/Rust) or /** */ JSDoc (TypeScript) above the definition. Implies includeBody=true. Response includes 'docCommentLines' count. Budget-aware: counts against maxBodyLines",
            "sortBy": "sortBy='cognitiveComplexity' maxResults=20 -> 20 most complex methods. sortBy='lines' -> longest definitions",
            "attribute": "'ApiController', 'Authorize', 'ServiceProvider'",
            "baseType": "'ControllerBase', 'IUserService' -> finds classes implementing IUserService",
            "file": "Single: [\"UserService.cs\"]. Multi-term OR: [\"UserService.cs\",\"OrderService.cs\",\"Controller.cs\"] (finds defs in ANY matching file). Substring match on file path",
            "parent": "Single: [\"UserService\"]. Multi-term OR: [\"UserService\",\"OrderService\",\"DataProcessor\"] (finds members of ANY matching class). Substring match on parent name",
            "regex": "name=[\"I.*Cache\"] with regex=true -> all types matching pattern",
            "kind": "Comma-separated for multi-kind OR: kind=[\"class\",\"interface\",\"enum\"]. C# kinds: class, interface, method, property, field, enum, struct, record, constructor, delegate, event, enumMember. TypeScript kinds: function, typeAlias, variable (plus shared: class, interface, method, field, enum, constructor, enumMember). NOTE: In TypeScript, class members (e.g. 'private name: string') are kind=[\"field\"], while interface signatures (e.g. 'readonly id: string' in an interface) are kind=[\"property\"]. If kind=[\"property\"] returns 0 results for a TS class, try kind=[\"field\"]. SQL kinds: storedProcedure, sqlFunction, table, view, userDefinedType, sqlIndex, column. SQL call sites extracted from SP bodies: EXEC, FROM, JOIN, INSERT, UPDATE, DELETE. Response: when multi-name + kind causes some terms to silently drop, summary includes missingTerms array with {term, reason} for each dropped term",
            "includeCodeStats": "Each method gets: lines, cyclomaticComplexity, cognitiveComplexity, maxNestingDepth, paramCount, returnCount, callCount, lambdaCount",
            "audit": "Shows: total files, files with/without definitions, read errors, lossy UTF-8, suspicious files (large files with 0 definitions)",
            "angular": "Angular @Component classes include 'selector' and 'templateChildren' in output, showing which child components are used in the template",
            "includeUsageCount": "includeUsageCount=true -> each definition gets usageCount (number of files containing this name in content index). usageCount=0 or 1 = potential dead code. Counts ALL text occurrences including comments/strings. Exact token match only",
            "autoSummary": "Triggered automatically when results > maxResults and no name filter. Returns directory-grouped overview with counts and top-3 definitions per subdirectory. To get individual definitions instead, add name filter or narrow file scope",
            "xmlTextContent": "XML on-demand: name filter searches both element names AND text content. Example: name=[\"PremiumStorage\"] finds <ServiceType>PremiumStorage</ServiceType> and returns parent block via auto-promotion. Response includes matchedBy ('name' or 'textContent'), matchedChild (single leaf) or matchedChildren (multiple leaves). Min 3-char term for text content search. Name matches appear first, then textContent-promoted results. Multiple leaf matches in same parent are de-duplicated"
        },
        "xray_grep": {
            "terms": "Token: [\"HttpClient\"]. Multi-term OR: [\"HttpClient\",\"ILogger\",\"Task\"]. Multi-term AND (mode='and'): [\"ServiceProvider\",\"IUserService\"]. Phrase (phrase=true): [\"new HttpClient\"]. Regex (regex=true): [\"I.*Cache\"]",
            "contextLines": "contextLines=5 shows 5 lines before and 5 lines after each match (like grep -C)",
            "showLines": "Returns groups of consecutive lines with startLine, lines array, and matchIndices",
            "ext": "[\"cs\"], [\"cs\",\"sql\"], [\"xml\",\"config\"] (each entry is one extension)",
            "substring": "Default: terms=[\"UserService\"] finds IUserService, m_userService. Set substring=false for exact-token-only",
            "dir": "Directory to search (default: server's --dir). Example: dir='src/services'. If a FILE path is passed (e.g., dir='src/main.rs'), it is auto-converted to dir='src' + file=[\"main.rs\"]; the response summary includes a `dirAutoConverted` note. Prefer file= directly to avoid the conversion",
            "file": "Restrict to files whose path or basename contains this substring (case-insensitive). Single: [\"CHANGELOG.md\"]. Multi-term OR: [\"Service\",\"Client\"] finds files matching either. Combines with dir/ext/excludeDir via AND. Use this instead of passing a file path in `dir`",
            "lineRegex": "Line-anchored regex (auto-enables regex=true, disables substring). Use when token regex (default) returns 0 results because the pattern contains `^`/`$`, spaces, or punctuation. Examples: terms=[\"^##\"] ext=[\"md\"] file=[\"README.md\"] -> level-2 markdown headings. terms=[\"^pub fn\"] ext=[\"rs\"] -> Rust public functions. terms=[\"^\\s*\\[Test\\]\"] ext=[\"cs\"] -> NUnit test attributes. terms=[\"\\}$\"] -> lines ending with closing brace. Patterns are NOT trimmed (whitespace is significant: '^## ' != '^##'). Anchors: `^` matches at logical line start, `$` at logical line end. Line endings: both `\\n` (Unix) and `\\r\\n` (Windows/CRLF) are recognized — `$` matches before the `\\r` of `\\r\\n`, so `terms=[\"\\}$\"]` works on Windows-edited files. Case sensitivity: case-SENSITIVE by default; use inline flag `(?i)` (e.g. terms=[\"(?i)^todo:\"]) to opt into case-insensitive. Multiple patterns combine via OR (default) or AND (mode='and'). ALWAYS narrow scope via ext/dir/file — without filters, every indexed file is read from disk. Mutually exclusive with phrase=true."
        },
        "xray_callers": {
            "class": "'UserService' -> DI-aware: also finds callers using IUserService. SQL: class = schema name (e.g., class='dbo', class='Sales'). Without class, results mix callers from ALL classes/schemas with same method name",
            "method": "[\"GetUserAsync\"] (single) or [\"GetUserAsync\",\"SaveChangesAsync\",\"ValidateInput\"] (multi-method batch). Single method returns {callTree: [...]}, multiple returns {results: [{method, callTree, nodesInTree}, ...]} with shared body budget. Multi-method batch saves N-1 MCP round trips. Angular/TS only: pass a selector (e.g. [\"app-header\"]) as method with direction='up' to find parent components that embed it via templateChildren. Returns templateUsage: true for template-based relationships",
            "direction": "'up' = who calls this (callers, default). 'down' = what this calls (callees). Angular/TS only: 'down' with class name shows child components from HTML template (recursive with depth). 'up' with selector (e.g. 'app-header') finds parent components recursively — depth=3 traverses grandparents, great-grandparents etc. Parents nested in 'parents' field",
            "resolveInterfaces": "When tracing callers of IFoo.Bar(), also finds callers of FooImpl.Bar() where FooImpl implements IFoo",
            "includeBody": "includeBody=true -> each node in call tree includes 'body' (source lines) and 'bodyStartLine'. Also adds a top-level 'rootMethod' object with the searched method's own body. Eliminates the need for a separate xray_definitions call. Default: false",
            "includeDocComments": "includeDocComments=true -> expands each method body to include doc-comments above it. Implies includeBody=true. Adds 'docCommentLines' field. Default: false",
            "maxBodyLines": "Max source lines per method (default: 30, 0=unlimited). Controls per-method body size when includeBody=true",
            "maxTotalBodyLines": "Max total body lines across all methods in tree (default: 300, 0=unlimited). When exceeded, remaining methods get 'bodyOmitted' instead of body",
            "angular": "TypeScript/Angular only: method=[\"app-header\"] direction='up' -> finds parent components embedding <app-header> via templateChildren (templateUsage: true). method=[\"processOrder\"] class='OrderFormComponent' direction='down' -> shows child components used in template",
            "impactAnalysis": "impactAnalysis=true with direction='up' -> identifies test methods covering the target. Response includes 'testsCovering' array (with full file path, depth, and callChain for each test) and isTest=true on test nodes. callChain shows the method-by-method path from target to test — use it to assess relevance (short chain = direct test, long chain = transitive via helpers). Tests detected via: C# [Test]/[Fact]/[Theory]/[TestMethod], Rust #[test], TS *.spec.ts/*.test.ts files. Use depth=5-7 for deep chains.",
            "limitation": "Only finds direct invocations (obj.Method(args)). Does NOT find method group/delegate references (e.g., list.Where(IsValid), Func<bool> f = svc.Check). Use xray_grep for those.",
            "includeGrepReferences": "includeGrepReferences=true -> adds grepReferences[] with files containing the method name as text but NOT in the call tree. Catches delegate usage, method groups, reflection. Skipped for method names < 4 chars. Each entry has file + tokenCount. For line-level detail, use xray_grep with showLines=true"
        },
        "xray_fast": {
            "pattern": "Substring or glob. Single: [\"UserService\"]. Multi-term OR: [\"UserService\",\"OrderProcessor\"]. Glob: [\"Order*\"], [\"*Service.cs\"], [\"Use?Service\"] — auto-converts to regex. No glob chars → substring. [\"*\"] or [] with dir → list all. dirsOnly=true for subdirectory listing",
            "dirsOnly": "Returns directories with fileCount, sorted descending (largest first). Works with wildcard and filtered patterns (e.g., [\"Storage\",\"Redis\"])",
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
            "noCache": "true -> bypass in-memory cache, query git CLI directly",
            "deletedFiles": "Deleted files are FULLY supported -- auto --follow fallback returns full history. info='deleted from HEAD' = history available; warning='never existed' = wrong path. Do NOT run raw `git log --all --diff-filter=D`."
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
            "message": "'refactor' — filter by commit message",
            "includeDeleted": "true -> filter to files deleted in the range (single `git ls-files` spawn + set diff). Use instead of raw `git log --all --diff-filter=D`."
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
            "paths": "Array of file paths for multi-file editing. Same edits/operations applied to ALL files. Transactional on input validation, content edits, and per-file write (temp+rename with backup-swap on Windows). In the rare event a rename fails mid-batch, earlier files in the batch may remain committed and are listed in the error response (use `git restore` to roll back). Max 20 files. Response has 'results' array + 'summary'. Example: paths=['file1.cs', 'file2.cs', 'file3.cs']",
            "operations": "Mode A (line-range): [{startLine: 5, endLine: 5, content: 'new line'}] — replace line 5. [{startLine: 3, endLine: 2, content: 'inserted'}] — insert before line 3 (endLine < startLine). [{startLine: 2, endLine: 4, content: ''}] — delete lines 2-4. Append to end: [{startLine: N+1, endLine: N, content: 'appended'}] where N = line count. Multiple ops applied bottom-up (no offset cascade). EMPTY FILE (incl. auto-created / nonexistent) = 0 lines: REPLACE/DELETE error with hint; only INSERT (startLine: 1, endLine: 0) is valid",
            "edits": "Mode B (text-match): [{search: 'old', replace: 'new'}] — replace all. [{search: 'old', replace: 'new', occurrence: 2}] — 2nd only. Insert after: [{insertAfter: 'using X;', content: 'using Y;'}]. Insert before: [{insertBefore: 'class Foo', content: '// comment'}]",
            "_responseFields_syncReindex": "After a real write (NOT dryRun), response includes: writeStatus ('committed' — the bytes always landed on disk by this point), reindexStatus ('completed' | 'skipped' — explicit index-update outcome), reindexSkipReason (string: 'outsideServerDir' | 'extensionNotIndexed' | 'insideGitDir' — only present when reindexStatus='skipped'; file is still written, only index update skipped), contentIndexUpdated (bool), defIndexUpdated (bool), fileListInvalidated (bool — true on file creation), reindexElapsedMs (string, e.g. '0.42'), reindexWarning (only set on lock poisoning — FS watcher reconciles within 500ms), fileCreated: true (on new files), skippedReason (DEPRECATED alias of reindexSkipReason — use reindexStatus + reindexSkipReason instead). dryRun: true OMITS all reindex fields. Multi-file: per-file reindex outcome + summary.reindexElapsedMs (one batched call).",
            "insertAfter_insertBefore": "Insert content on the line after/before an anchor text. Mutually exclusive with search/replace. Requires 'content' field. Use 'occurrence' to target Nth match (default: first). Example: {insertAfter: 'using System.IO;', content: 'using System.Linq;'}",
            "expectedContext": "Per-edit safety check: verify this text exists within ±5 lines of the match before applying. Example: {search: 'SemaphoreSlim(10)', replace: 'SemaphoreSlim(30)', expectedContext: 'var semaphore = new'}",
            "skipIfNotFound": "Per-edit flag: if true, silently skip when search/anchor text is not found (default: false). Essential for multi-file 'paths' where not all files contain the target text. Without it, one missing file aborts the entire batch. Response includes 'skippedDetails' array with editIndex, search text, and reason for each skipped edit",
            "regex": "true -> treat edit search strings as regex with $1, $2 capture groups (Mode B search/replace only)",
            "dryRun": "true -> preview unified diff without writing file. Works with both single and multi-file",
            "expectedLineCount": "Safety check: abort if file has different line count (prevents stale line numbers). Uses human/editor line count (matches xray_definitions/xray_grep numbers); reusable from previous response's newLineCount. Honored in both Mode A and Mode B.",
            "errorDiagnostics": "When text/anchor/pattern is not found, the error includes a nearest-match hint showing the most similar line in the file with line number and similarity percentage (e.g., 'Nearest match at line 5 (similarity 92%): ...'). Helps diagnose Unicode quote mismatches, case differences, and whitespace issues"
        },
        "xray_help": {
            "tool": "Optional string: return help for a single tool (e.g. 'xray_edit', 'xray_grep'). Omit to get the full reference catalog."
        },
        "xray_info": {
            "file": "Array of file paths. Returns lineCount, byteSize, lineEnding, extension, indexed status for each file."
        },
        "xray_reindex": {
            "dir": "Project root directory to index. Binds the workspace and builds content + definition indexes.",
            "ext": "Array of file extensions to index (e.g. ['rs', 'ts', 'cs']). Overrides defaults."
        },
        "xray_reindex_definitions": {
            "ext": "Array of file extensions to reindex definitions for. Rebuilds only the definition (AST) index."
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
    for strat in strategies_for_extensions(def_extensions) {
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

/// Known tool names recognised by `tool_help` — mirrors the dispatch list in
/// `src/mcp/handlers/mod.rs::dispatch_tool_call`. A drift-guard test in
/// `tips_tests.rs` keeps this in sync with the actual tool registry.
pub const KNOWN_TOOL_NAMES: &[&str] = &[
    "xray_definitions",
    "xray_grep",
    "xray_callers",
    "xray_fast",
    "xray_edit",
    "xray_info",
    "xray_reindex",
    "xray_reindex_definitions",
    "xray_help",
    "xray_git_history",
    "xray_git_diff",
    "xray_git_authors",
    "xray_git_activity",
    "xray_git_blame",
    "xray_branch_status",
];

/// Per-tool reference card returned by `xray_help { tool: "<name>" }`.
///
/// Goal: a focused, ~1KB payload an LLM agent can request when it needs to
/// recover from a first-attempt schema error — without consuming the budget
/// of the full `render_json` reference catalog. Keeps the `xray_edit` entry
/// in particular short by reusing `CANONICAL_MODE_A_EXAMPLE` /
/// `CANONICAL_MODE_B_EXAMPLE` (so a schema change updates one place).
///
/// Returns `Err` with the list of known tool names when `tool_name` is not
/// recognised, so the caller can self-correct in one round-trip.
pub fn tool_help(tool_name: &str, def_extensions: &[String]) -> Result<Value, String> {
    if !KNOWN_TOOL_NAMES.contains(&tool_name) {
        return Err(format!(
            "Unknown tool '{}'. Known tools: {}.",
            tool_name,
            KNOWN_TOOL_NAMES.join(", ")
        ));
    }

    let parameters = parameter_examples(def_extensions)
        .get(tool_name)
        .cloned()
        .unwrap_or(json!({}));

    let mut payload = json!({
        "tool": tool_name,
        "parameters": parameters,
    });

    // xray_edit is the only tool with two valid mode shapes — surface both
    // canonical examples up front so the caller does not need to reconstruct
    // them from the parameter description.
    if tool_name == "xray_edit" {
        payload["canonicalExamples"] = json!({
            "modeA_lineRange": CANONICAL_MODE_A_EXAMPLE,
            "modeB_textMatch": CANONICAL_MODE_B_EXAMPLE,
        });
        payload["decisionCard"] = json!({
            "title": "When to use xray_edit vs built-in edit tools",
            "ruleOfThumb": "Existing text file -> xray_edit. New large file or full rewrite >200 lines -> built-in whole-file write is acceptable.",
            "useXrayEditFor": [
                "Editing any existing text file, regardless of extension or index coverage",
                "Small or surgical line-range/text-match changes",
                "Multi-file edits, dryRun previews, post-write verification, and sync reindex"
            ],
            "builtInOkFor": [
                "Creating a brand-new large file (xray_edit Mode A also works for small new files)",
                "Full-file rewrites over about 200 lines",
                "Binary files or byte-exact preservation cases"
            ]
        });
    }

    Ok(payload)
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

    let strategy_recipes: Vec<Value> = strategies_for_extensions(def_extensions).iter().map(|s| {
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

fn render_language_instruction_blocks(out: &mut String, profile: &LanguageProfile) {
    if profile.has_sql_defs {
        out.push_str("SQL INVESTIGATION (SQL parser is active):\n");
        out.push_str("  - Read a stored procedure body: xray_definitions kind=[\"storedProcedure\"] name=[\"usp_Foo\"] includeBody=true maxBodyLines=0\n");
        out.push_str("  - Read JUST the parameter header: first get line_start, then xray_definitions name=[\"usp_Foo\"] includeBody=true bodyLineStart=<line_start> bodyLineEnd=<line_start+40>. bodyLineStart/bodyLineEnd are absolute file lines.\n");
        out.push_str("  - SP-to-SP EXEC chains: xray_callers method=[\"usp_Foo\"] class='dbo' direction='down'\n");
        out.push_str("  - The structured signature field is a summary; verify exact parameter types/defaults against the body header.\n");
        out.push_str("  - After a SQL parser/signature fix, run xray_reindex_definitions ext=[\"sql\"] before validating cached metadata.\n");
        out.push_str("  - Tables/views/columns/indexes are searchable via kind, but call graph targets only callable code.\n\n");
    }

    if profile.has_typescript_defs {
        out.push_str("TYPESCRIPT / ANGULAR INVESTIGATION (TS parser is active):\n");
        out.push_str("  - Find parents of a component: xray_callers method=[\"app-foo\"] direction='up' depth=3 (selector string, not class name).\n");
        out.push_str("  - Find children of a component: xray_callers method=[\"FooComponent\"] direction='down' depth=2 (templateChildren from templateUrl files).\n");
        if profile.has_html_content {
            out.push_str("  - Search HTML templates directly: xray_grep terms=[\"<app-foo\"] phrase=true ext=[\"html\"].\n");
            out.push_str("  - Search Angular bindings directly: xray_grep terms=[\"[(ngModel)]\"] phrase=true ext=[\"html\"].\n");
        } else {
            out.push_str("  - HTML is not content-indexed; add html (and optionally scss/css) to --ext for xray_grep template searches. Selector call graph can still work from templateUrl enrichment.\n");
        }
        if profile.has_scss_content {
            out.push_str("  - Styles are content-indexed; use xray_grep ext=[\"scss\"] or ext=[\"css\"] for style references.\n");
        }
        out.push_str("  - TS class fields are kind=[\"field\"]; interface signatures are kind=[\"property\"]. If property returns 0 in a class, retry field.\n\n");
    }

    if profile.has_csharp_defs {
        out.push_str("C# INVESTIGATION (C# parser is active):\n");
        out.push_str("  - DI-aware callers: xray_callers class='UserService' also matches callers using IUserService. If 0 callers, retry class='IUserService'.\n");
        out.push_str("  - using static methods are defined on the helper class: xray_definitions name=[\"Bar\"] without parent, or parent=[\"FooHelper\"].\n");
        out.push_str("  - Attribute string literals can surface as valueSourceHint on properties/fields; grep that string in appsettings/manifest/env/secrets.\n\n");
    }

    if profile.has_xml_on_demand {
        // XML on-demand is independent of the server's --ext flag. The whitelist
        // (see parser_xml::is_xml_extension) is what determines which file
        // extensions the on-demand path will parse — agents have historically
        // confused "xray_grep ext=[\"xml\"] returned 0" with "xray cannot read
        // this file at all", and fall back to PowerShell. Spell out the gap.
        out.push_str("XML / CSPROJ ON-DEMAND PARSING (parses one file on demand; works regardless of --ext):\n");
        out.push_str("  - Whitelist: .xml .config .csproj .vbproj .fsproj .vcxproj .props .targets .nuspec .vsixmanifest .manifestxml .appxmanifest .resx\n");
        out.push_str("  - Find which element contains a line: xray_definitions file=[\"app.csproj\"] containsLine=42\n");
        out.push_str("  - Search by element name or leaf text: xray_definitions file=[\"service.config\"] name=[\"PremiumStorage\"] (auto-promotes leaf matches to parent block).\n");
        out.push_str("  - Deployment / manifest configs: xray_definitions file=[\"cluster.parameters.manifestxml\"] name=[\"PlatformSearchIndexConfiguration\"] includeBody=true\n");
        out.push_str("  - Compare config across N files: xray_fast pattern=[\"*.search.xml\"] to enumerate, then loop xray_definitions per matched path. Do NOT fall back to PowerShell Get-ChildItem + [xml] / Select-String.\n");
        out.push_str("  - xray_grep ext=[\"xml\"] does NOT match .manifestxml/.csproj/.props (different ext tokens). For grep coverage, list each ext explicitly OR use xray_definitions on-demand which does not depend on --ext at all.\n");
        if profile.has_xml_content {
            out.push_str("  - Raw XML phrase search: xray_grep terms=[\"<MaxRetries>3</MaxRetries>\"] phrase=true ext=[\"config\",\"xml\"]\n");
        }
        out.push('\n');
    }

    if profile.has_rust_defs {
        out.push_str("RUST INVESTIGATION (Rust parser is active):\n");
        out.push_str("  - Read a known function or impl method body: xray_definitions name=[\"foo\"] parent=[\"TypeName\"] includeBody=true maxBodyLines=0. For free functions, omit parent or add file=[\"path.rs\"].\n");
        out.push_str("  - For common names (new/from/fmt/default/parse/handle), always add parent or file.\n");
        out.push_str("  - Trace callers/callees: xray_callers method=[\"foo\"] class='TypeName' direction='up'|'down' includeBody=true.\n");
        out.push_str("  - Find tests covering a method: xray_callers method=[\"foo\"] class='TypeName' direction='up' impactAnalysis=true depth=5.\n\n");
    }
}

fn render_read_examples(profile: &LanguageProfile) -> String {
    let mut examples = Vec::new();
    if profile.has_csharp_defs {
        examples.push("  - C#: xray_definitions file=[\"UserService.cs\"] includeBody=true maxBodyLines=0".to_string());
    }
    if profile.has_typescript_defs {
        examples.push("  - TS: xray_definitions file=[\"user.service.ts\"] includeBody=true maxBodyLines=0".to_string());
    }
    if profile.has_sql_defs {
        examples.push("  - SQL: xray_definitions file=[\"usp_GetUser.sql\"] kind=[\"storedProcedure\"] includeBody=true maxBodyLines=0".to_string());
    }
    if profile.has_rust_defs {
        examples.push("  - Rust: xray_definitions file=[\"src/lib.rs\"] name=[\"parse\"] includeBody=true maxBodyLines=0".to_string());
    }

    for ext in &profile.definition_extensions {
        let already_covered = matches!(ext.as_str(), "cs" | "ts" | "tsx" | "sql" | "rs");
        if !already_covered {
            examples.push(format!(
                "  - .{}: xray_definitions file=[\"handler.{}\"] includeBody=true maxBodyLines=0",
                ext, ext
            ));
        }
    }

    examples.join("\n")
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
#[cfg(test)]
pub fn render_instructions(def_extensions: &[&str]) -> String {
    let profile = LanguageProfile::new(def_extensions, def_extensions);
    render_instructions_for_profile(&profile)
}

pub fn render_instructions_for_profile(profile: &LanguageProfile) -> String {
    let def_extensions = profile.definition_extension_refs();
    let mut out = String::new();

    out.push_str("=== XRAY_POLICY ===\n");

    // --- TERMS (condensed — built-in tool names differ across LLM hosts; think in operations, not names) ---
    out.push_str("=== TERMS ===\n");
    out.push_str("  \"xray tools\"       = MCP tools from this server, names prefixed xray_*.\n");
    out.push_str("  \"built-in tools\"   = your LLM host's native file-manipulation tools (read/search/edit/list), names vary per host — map by OPERATION TYPE, not by name.\n");
    out.push_str("=============\n");

    // --- CRITICAL OVERRIDE (must be first — highest priority) ---
    out.push_str("CRITICALLY IMPORTANT — THIS OVERRIDES ALL OTHER TOOL PREFERENCES:\n");
    out.push_str("  When this MCP server is connected, ALWAYS prefer xray tools over built-in equivalents:\n");
    if !def_extensions.is_empty() {
        out.push_str("  - Explore code/modules/directories -> xray_definitions file=[\"<dir>\"] (NOT directory_tree, NOT list_directory)\n");
        out.push_str("  - Read source code -> xray_definitions includeBody=true (NOT file reading tools)\n");
        out.push_str("  - Find callers/callees -> xray_callers (NOT grep-based manual search)\n");
    }
    out.push_str("  - Search file contents -> xray_grep (NOT built-in text/regex search)\n");
    out.push_str("  - Find files by name -> xray_fast (NOT built-in file search)\n");
    out.push_str("  - Edit files -> xray_edit (NOT built-in diff/replace tools)\n");
    out.push_str("  - Git history/blame -> xray_git_history/xray_git_blame (NOT built-in git tools)\n");
    out.push_str("  REASON: xray tools use pre-built indexes (<1ms) and return richer data than built-in tools.\n");
    out.push_str("  DECISION TRIGGER: before using ANY built-in tool, STOP and check if a xray tool can do it instead.\n\n");

    // --- INTENT -> TOOL MAPPING (positive triggers, intent-first) ---
    // Rationale: intent-first models (Claude 4.7+) select tools by matching the user's
    // underlying intent, not by scanning NEVER-lists. This compact map shortcuts the
    // selection to xray before built-in habits kick in.
    out.push_str("INTENT -> TOOL MAPPING (consult BEFORE choosing any tool):\n");
    if !def_extensions.is_empty() {
        out.push_str("  \"read the source code of a method/class\"         -> xray_definitions name=[\"X\"] includeBody=true maxBodyLines=0\n");
        out.push_str("  \"I already know the class/method/procedure/function name and want its body\" -> xray_definitions name=[\"X\"] parent=[\"Y\"] includeBody=true maxBodyLines=0 OR file=[\"known/path.ext\"] name=[\"X\"] includeBody=true maxBodyLines=0\n");
        out.push_str("  \"I just got the file path from xray_grep / xray_fast and want to list its symbols\" -> xray_definitions file=[\"<path>\"] (no includeBody first)\n");
        out.push_str("  \"I just got the file path and truly need every body in it\" -> xray_definitions file=[\"<path>\"] includeBody=true maxBodyLines=0 maxTotalBodyLines=0 (use sparingly)\n");
        out.push_str("  \"find which method is at file:line N\"            -> xray_definitions file=[\"X\"] containsLine=N includeBody=true\n");
        out.push_str("  \"I just got file:line from a stack trace\"       -> xray_definitions file=[\"<path>\"] containsLine=N includeBody=true\n");
        out.push_str("  \"find who calls/implements method X\"             -> xray_callers method=[\"X\"] class='Y' direction='up'\n");
        out.push_str("  \"verify upstream reachability of helper X\"       -> xray_callers method=[\"X\"] direction='up' (add class='Y' for methods; do NOT grep+read_file)\n");
    }
    out.push_str("  \"see a few lines of context around a match\"      -> xray_grep showLines=true contextLines=N\n");
    out.push_str("  \"search text across codebase\"                    -> xray_grep terms=[\"...\"]\n");
    out.push_str("  \"validate/fact-check whether a term exists in code\"  -> xray_grep terms=[\"...\"] countOnly=true\n");
    out.push_str("  \"quick yes/no: does X appear anywhere\"           -> xray_grep countOnly=true\n");
    out.push_str("  \"confirm absence of pattern before editing\"      -> xray_grep terms=[\"...\"] countOnly=true\n");
    out.push_str("  \"replace similar patterns in one or more files\"  -> xray_edit with multiple edits (atomic, batch)\n");
    out.push_str("  \"rewrite entire file\"                            -> xray_edit Mode A [{startLine:1, endLine:<total>, content:<new>}]\n");
    out.push_str("  \"INSERT lines (no replace)\"                      -> xray_edit Mode A empty range: startLine:N,endLine:N-1 | startLine:lineCount+1,endLine:lineCount\n");
    out.push_str("  \"create a new file\"                              -> xray_edit (auto-creates — Mode A with endLine:0)\n");
    out.push_str("  \"list files or subdirectories\"                   -> xray_fast pattern=[\"*\"] dir='<path>' dirsOnly=true\n");
    out.push_str("  \"find a file by name\"                            -> xray_fast pattern=[\"<name>\"]\n");

    out.push_str("  \"git blame / history / authors\"                  -> xray_git_blame / xray_git_history / xray_git_authors (works for BOTH existing AND deleted files)\n");
    out.push_str("  \"history of a file that was DELETED/removed\"     -> xray_git_history repo='.' file='<path>' (auto --follow)\n");
    out.push_str("  \"show activity including deleted files\"         -> xray_git_activity repo='.' from='YYYY-MM-DD' includeDeleted=true\n\n");

    // (TASK ROUTING removed — 100% duplicate of INTENT -> TOOL MAPPING above.
    //  All entries from TASK ROUTING are now implicit in the INTENT mapping.
    //  Fallback guidance kept inline:)
    out.push_str("If uncertain about file type/lines/encoding, call xray_info first (file=[\"X\"] -> lineCount/byteSize/lineEnding; not Get-Content/wc -l). Do not default to raw file reading.\n\n");

    if !def_extensions.is_empty() {
        out.push_str("INVESTIGATION DECISION TREE (apply when user asks explain/trace/investigate a code flow):\n");
        out.push_str("  1. LOCATE - xray_fast (file names) + xray_grep (content). Output: candidate files / call sites.\n");
        out.push_str("  2. STRUCTURE - xray_definitions file=[\"<dir-or-file>\"] (no includeBody). Output: classes/methods/SPs/tables in scope.\n");
        out.push_str("  3. READ BODY - xray_definitions name=[\"<symbol>\"] parent=[\"<owner>\"] includeBody=true maxBodyLines=0 OR file=[\"<known-path>\"] name=[\"<symbol>\"] includeBody=true maxBodyLines=0. Once a symbol is known, this replaces read_file.\n");
        out.push_str("  4. TRACE - xray_callers method=[\"<X>\"] class='<Y>' direction='up'|'down' includeBody=true.\n");
        out.push_str("  5. WIDER FILE CONTEXT - read_file ONLY for imports/usings, fields, neighbouring methods, generated regions, partial-class layout, non-indexed files, or exact edit line ranges.\n");
        out.push_str("  ANTI-PATTERN: doing step 3 with read_file when xray_definitions is available.\n\n");
    }

    if !def_extensions.is_empty() || profile.has_xml_on_demand {
        render_language_instruction_blocks(&mut out, profile);
    }
    // --- MANDATORY PRE-FLIGHT CHECK (procedural friction before built-in) ---
    // Rationale: even with INTENT mapping, habits can bypass the map. A 3-question
    // pre-flight in <thinking> forces a conscious justification before a built-in call.
    out.push_str("MANDATORY PRE-FLIGHT CHECK — REQUIRED for EVERY built-in tool call (read_file, search_files, apply_diff, search_and_replace, insert_content, write_to_file, list_files, list_directory, directory_tree, and any equivalent built-in tool your LLM host provides):\n");
    out.push_str("  Before ANY call to a built-in tool, you MUST write a <thinking> block answering ALL three questions:\n");
    out.push_str("  Q1 (operation type): What am I trying to do? Match to INTENT -> TOOL MAPPING above.\n");
    out.push_str("       - If my intent has a mapped xray tool -> I MUST use the xray tool. No exceptions for 'familiarity' or 'habit'.\n");
    out.push_str("  Q2 (file scope): Is the target file(s) in scope for xray? READ -> xray_definitions if extension indexed, else built-in OK. SEARCH -> xray_grep (ALL indexed contents) or xray_fast (ALL file names) — almost always xray. EDIT -> xray_edit on ANY text file (extension IRRELEVANT; exceptions: create new, rewrite >200 lines, binary).\n");
    out.push_str("  Q3 (justification): If Q1+Q2 point me to a built-in tool, WHY?\n");
    out.push_str("       Valid reasons: (a) file type not indexed AND content not text-searchable, (b) binary file, (c) byte-exact preservation impossible with xray (rare), (d) creating a brand-new file (built-in whole-file-write OK), (e) rewriting >200 lines completely (avoids noisy diff).\n");
    out.push_str("       Invalid reasons: 'habit', 'familiarity', 'it is just a small edit', 'this file extension is not indexed so xray probably does not apply to editing' — these are all UNJUSTIFIED. Just habit / familiarity is NEVER a valid reason.\n");
    out.push_str("       If you cannot cite a valid reason from (a)-(e) -> the built-in call is a VIOLATION. Use xray instead.\n");
    out.push_str("  Without a clear valid reason, the built-in call is NOT allowed.\n");
    out.push_str("  ENFORCEMENT: omitting the <thinking> block before a built-in call is itself a violation — the policy requires a visible justification trace.\n");
    out.push_str("  SELF-AUDIT HOOK: if you just called a built-in tool and realize (during subsequent reasoning) that an xray tool would have worked — STOP, acknowledge the misstep, and switch to xray for the next call. Do NOT continue the built-in-tool chain.\n");
    out.push_str("  PRE-CALL SELF-AUDIT: before FORMULATING any built-in tool call, ask:\n");
    out.push_str("       (a) \"What is my actual intent?\" (1-word answer)\n");
    out.push_str("       (b) \"Is there an xray tool mapped to that intent in INTENT -> TOOL MAPPING?\"\n");
    out.push_str("       (c) If yes -> STOP built-in call, switch to xray.\n");
    out.push_str("       This takes 1 second per call. It prevents the most common violation class — 'mental shortcuts on seemingly-trivial tasks'.\n\n");

    // --- COST REALITY (single rule-of-thumb; details live in xray_help) ---
    out.push_str("COST REALITY: xray tools are 3-24x cheaper in tokens and round-trips than built-in equivalents (see xray_help for measured ratios). Rule of thumb: 2 built-in calls in a row on the same file = you should have used xray.\n\n");

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
        out.push_str("   EXAMPLES:\n");
        out.push_str(&render_read_examples(profile));
        out.push_str("\n\n");
    } else if profile.has_xml_on_demand {
        out.push_str("NOTE: no ordinary source-code definition parser extensions are active, but XML on-demand parsing is available through xray_definitions for XML/config/csproj-style files. Use xray_grep for content search.\n\n");
    } else {
        out.push_str("NOTE: xray_definitions is not available for the configured file extensions. Use xray_grep for content search.\n\n");
    }

    // --- FILE EDITING DECISION TRIGGER (tool-name-agnostic, operation-based) ---
    // v3: severity aligned with NEVER READ rule for indexed files. Extension-
    // agnostic — xray_edit works on ANY text file, not only indexed ones.
    out.push_str("NEVER USE your built-in edit tools for EDITING existing text files. ALWAYS use xray_edit — regardless of file extension. xray_edit works on ALL text files, NOT only on indexed extensions. The indexed-extensions list (from --ext CLI) governs what xray_definitions / xray_grep can PARSE — it does NOT limit xray_edit. xray_edit operates on BYTES, not on AST.\n");
    out.push_str("   MISCONCEPTION ALERT: 'this file is not indexed (not in --ext) — I'll use my built-in edit tool' — WRONG. xray_edit has NO extension filter. It edits ANY text file.\n");
    out.push_str("   DECISION TRIGGER: before calling ANY built-in edit tool (apply_diff, search_and_replace, insert_content, patch/diff tools, line-insert tools, or write_to_file for edits) for an EXISTING file — STOP. Use xray_edit instead.\n");
    out.push_str("   xray_edit advantages: atomic (all-or-nothing rollback), no whitespace fragility, multi-file transactional batch, dryRun preview, works on any file extension.\n");
    out.push_str("   xray_edit auto-creates new files (treats as empty — use Mode A: operations [{startLine:1, endLine:0, content:'...'}]). For small new files this avoids switching tools.\n");
    out.push_str("   EXCEPTION — CREATING new files: your built-in whole-file-write tool is acceptable for creating new files (xray_edit Mode A also works but may be verbose for large new files). For EDITING existing files — always xray_edit.\n");
    out.push_str("   EXCEPTION — FULL FILE REWRITE >200 lines: your built-in whole-file-write tool is acceptable to avoid noisy diff output. For rewrites <=200 lines — xray_edit Mode A.\n");
    out.push_str("   EXCEPTION — BINARY files or byte-exact preservation: built-in tool with explicit justification (rare).\n\n");

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
    out.push_str("   5. Only use built-in tools if the file type is NOT indexed by xray\n");
    out.push_str("   6. GIT: warning ('never existed') / info ('deleted') ARE the answer — do NOT fall back to raw `git log --all --diff-filter=D`. xray_git_* covers deleted files via auto --follow.\n\n");

    // --- Top anti-patterns (extracted from strategy recipes) ---
    out.push_str("ANTI-PATTERNS (most common mistakes — each one wastes 3-5 extra tool calls):\n");
    out.push_str("  - NEVER list or browse directories to explore code — xray_definitions file=[\"<dir>\"] returns ALL classes/methods/interfaces in ONE call\n");
    out.push_str("  - NEVER search one kind at a time (class, then interface, then enum) — use kind=[\"class\",\"interface\",\"enum\"] for multi-kind OR, or omit kind filter to get everything at once\n");
    out.push_str("  - ALWAYS use excludeDir=['test','Test','Mock'] to skip test files from results\n");
    out.push_str("  - NEVER call xray_fast dirsOnly=true to explore code modules — xray_definitions file=[\"<dir>\"] auto-generates directory-grouped summary (autoSummary) when results are too many to list individually\n");
    out.push_str("  EXAMPLE VIOLATION — the most common policy break:\n");
    out.push_str("  - User intent: \"validate/check/verify whether code has/doesn't have X\"\n");
    out.push_str("  - WRONG reasoning: \"I need to search for X -> search_files\"\n");
    out.push_str("  - RIGHT: xray_grep terms=[\"X\"] countOnly=true (1 call, <1ms, clean yes/no)\n");
    out.push_str("  - ROOT CAUSE: search_files is literally named the same as the intent \"search files\". This linguistic coincidence causes habit-driven selection, bypassing MANDATORY PRE-FLIGHT CHECK.\n");
    out.push_str("  - PREVENTION: always pause 1 second before ANY built-in call: \"could xray_grep do this?\" Answer is YES ~100% of the time for indexed text searches.\n");
    if !def_extensions.is_empty() {
        let ext_dotted: Vec<String> = def_extensions.iter().map(|e| format!(".{}", e)).collect();
        out.push_str(&format!(
            "  - NEVER use xray_definitions for non-{} files (JSON, YAML, MD) — \
             it only supports AST parsing for {}. Use xray_grep instead\n",
            ext_dotted.join("/"), ext_dotted.join("/")
        ));
    }
    out.push('\n');

    // (STRATEGY RECIPES removed from system-prompt rendering — available on-demand
    //  via xray_help. Inline reference kept:)
    out.push_str("STRATEGY RECIPES: aim for <=3 search calls per task. Call xray_help for the full catalog of recipes (architecture exploration, call-chain investigation, stack-trace-to-method, etc.).\n");

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
