//! MCP tool handlers — dispatches tool calls to specialized handler modules.

mod advisory_hints;
mod callers;
mod arg_validation;
mod definitions;
mod edit;
mod fast;
mod git;
mod grep;
pub(crate) mod utils;
#[cfg(feature = "lang-xml")]
mod xml_on_demand;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use serde_json::{json, Value};
use tracing::{info, warn};

use crate::mcp::protocol::{ToolCallResult, ToolDefinition};
use crate::{
    build_content_index, clean_path, load_content_index, find_content_index_for_dir,
    save_content_index, ContentIndex, ContentIndexArgs, FileIndex,
    DEFAULT_MIN_TOKEN_LEN,
};
use crate::definitions::DefinitionIndex;
use crate::git::cache::GitHistoryCache;

// Re-export for use by tests (crate-internal only)
#[cfg(test)]
pub(crate) use self::callers::find_containing_method;
#[cfg(test)]
pub(crate) use self::callers::resolve_call_site;

/// Return all tool definitions for tools/list.
/// `def_extensions` — file extensions with definition parser support (e.g., ["cs", "rs"]).
/// Used to dynamically generate language lists in xray_definitions and xray_callers descriptions.
pub fn tool_definitions(def_extensions: &[String]) -> Vec<ToolDefinition> {
    let lang_list = crate::tips::format_supported_languages(def_extensions);
    let mut tools = vec![
        ToolDefinition {
            name: "xray_grep".to_string(),
            description: "Preferred for content/pattern search across indexed files. Use before built-in text/regex search for indexed file types. Search file contents using an inverted index with TF-IDF ranking. LANGUAGE-AGNOSTIC: works with any text file (C#, Rust, Python, JS/TS, XML, JSON, config, etc.). Supports exact tokens, multi-term OR/AND, regex, phrase search, substring search, and exclusion filters. Results ranked by relevance. Index stays in memory for instant subsequent queries (~0.001s). Substring search is ON by default. Large results are auto-truncated to ~16KB (~4K tokens). Use countOnly=true or narrow with dir/ext/excludeDir for focused results. Comma-separated phrases with spaces are searched independently (OR/AND).".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "terms": {
                        "type": "string",
                        "description": "Search terms. Comma-separated for multi-term OR/AND."
                    },
                    "dir": {
                        "type": "string",
                        "description": "Directory to search (default: server's --dir). Supports both absolute and relative paths — relative paths are resolved against server_dir. If a FILE path is passed, it is auto-converted to its parent directory + file= basename filter; the response includes a `dirAutoConverted` note explaining the conversion. To narrow to a specific file, prefer `file='<name>'` directly."
                    },
                    "file": {
                        "type": "string",
                        "description": "Restrict results to files whose path or basename contains this substring (case-insensitive). Comma-separated for multi-term OR (e.g., 'Service,Client'). Combines with `dir`/`ext`/`excludeDir` via AND. Use this to search in a specific file or a small set of files without passing a file path in `dir`."
                    },
                    "ext": {
                        "type": "string",
                        "description": "File extension filter, comma-separated (default: all indexed)"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["or", "and"],
                        "description": "Multi-term mode: 'or' = ANY term (default), 'and' = ALL terms."
                    },
                    "regex": {
                        "type": "boolean",
                        "description": "Treat as regex pattern (default: false)"
                    },
                    "phrase": {
                        "type": "boolean",
                        "description": "Exact phrase match on raw file content (default: false). Matches literal strings including XML tags, angle brackets, slashes — no escaping needed. Example: terms='<MaxRetries>3</MaxRetries>', phrase=true finds the exact XML tag. Comma-separated phrases are searched independently with OR/AND semantics."
                    },
                    "lineRegex": {
                        "type": "boolean",
                        "description": "Line-anchored regex search (default: false). Auto-enables regex=true and disables substring. Unlike default regex (which matches against tokenized index entries — alphanumeric+underscore only), lineRegex applies the pattern to each line of file content with `multi_line=true`, so `^` and `$` anchor to line boundaries and patterns may contain spaces, punctuation, brackets, etc. Required for: markdown headings (`^## `), C# attributes (`^\\s*\\[Test\\]`), function signatures (`^pub fn`), end-of-line braces (`\\}$`). Comma-separated patterns supported (OR/AND via mode). For patterns containing literal `,` (CSV-shape, log prefixes), use `linePatterns` instead. Whitespace inside patterns is significant — patterns are NOT trimmed. File scope MUST be narrowed via ext/dir/file filters; otherwise every indexed file is read from disk (slower than token regex). Mutually exclusive with phrase=true."
                    },
                    "linePatterns": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Explicit array of line-regex patterns for `lineRegex=true` mode. Each entry is one pattern, taken verbatim — `,` inside a pattern is preserved (e.g. CSV regex `^[^,]+,[^,]+$`, log prefix `^ERROR,WARN:`). Use this instead of `terms` when any pattern contains a literal comma. Mutually exclusive with `terms`; requires lineRegex=true."
                    },
                    "showLines": {
                        "type": "boolean",
                        "description": "Include matching source lines in results (default: false)"
                    },
                    "contextLines": {
                        "type": "integer",
                        "description": "Context lines before/after each match, requires showLines (default: 0)"
                    },
                    "maxResults": {
                        "type": "integer",
                        "description": "Max results (0=unlimited, default: 50)"
                    },
                    "excludeDir": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Directory names to exclude"
                    },
                    "exclude": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "File path substrings to exclude"
                    },
                    "countOnly": {
                        "type": "boolean",
                        "description": "Return counts only (default: false)"
                    },
                    "substring": {
                        "type": "boolean",
                        "description": "Match within tokens (default: true). Auto-disabled for regex/phrase."
                    },
                    "autoBalance": {
                        "type": "boolean",
                        "description": "Auto-balance multi-term substring-OR results (default: true). When ONE term has >10× more occurrences than the rarest matched term, dominant-only files are trimmed beyond an auto-derived cap (`min(100, max(20, 2*second_max))`) so rare-term matches stay visible. Files matching ≥2 terms are always kept. Set false to opt out (return raw TF-IDF order). No effect on AND mode, regex, phrase, or single-term queries."
                    },
                    "maxOccurrencesPerTerm": {
                        "type": "integer",
                        "description": "Explicit cap (in dominant-only files) for auto-balance. Default 0 = derived from `2 * second_max` clamped to [20, 100]. Only consulted when autoBalance triggers."
                    }
                },
                "required": ["terms"]
            }),
        },

        ToolDefinition {
            name: "xray_fast".to_string(),
            description: "PREFERRED file lookup tool — searches pre-built file name index. Instant results (~35ms for 100K files). Auto-builds index if not present. Supports comma-separated patterns for multi-file lookup (OR logic). Example: pattern='UserService,OrderProcessor' finds files whose name contains ANY of the terms. Supports pattern='*' or empty pattern with dir to list ALL files/directories (wildcard listing). Use with dirsOnly=true to list subdirectories. ALWAYS use this instead of built-in list_files or list_directory. When dirsOnly=true with wildcard, returns directories sorted by fileCount (largest modules first) and includes fileCount field.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "File name substring or glob pattern. Comma-separated for multi-term OR. Glob characters (* and ?) are auto-detected and converted to regex (e.g., 'Order*' finds files starting with Order, 'Use?Service' matches single char). Without glob chars, uses substring matching. Use '*' to list all entries. Empty string with dir also lists all." },
                    "dir": { "type": "string", "description": "Directory to search. Supports both absolute and relative paths. Relative paths are resolved against server_dir (e.g., 'src/services' resolves to server_dir/src/services)." },
                    "ext": { "type": "string", "description": "Filter by extension" },
                    "regex": { "type": "boolean", "description": "Treat as regex" },
                    "ignoreCase": { "type": "boolean", "description": "Case-insensitive" },
                    "dirsOnly": { "type": "boolean", "description": "Show only directories. Returns directories sorted by fileCount descending (largest first). Useful for identifying important modules. Works with both wildcard (pattern='*') and filtered patterns (e.g., 'Storage,Redis'). ext filter is ignored (directories have no extension)" },
                    "filesOnly": { "type": "boolean", "description": "Show only files" },
                    "countOnly": { "type": "boolean", "description": "Count only" },
                    "maxDepth": { "type": "integer", "description": "Max directory depth for dirsOnly results (1=immediate children only). Default: unlimited" },
                    "maxResults": { "type": "integer", "description": "Max results to return (0=unlimited, default: 0). Use to limit response size for large directories." }
                },
                "required": ["pattern"]
            }),
        },
        ToolDefinition {
            name: "xray_info".to_string(),
            description: "Show all existing indexes with their status, sizes, and age.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: "xray_reindex".to_string(),
            description: "Force rebuild the content index and reload it into the server's in-memory cache. Also rebinds workspace when dir parameter switches to a different directory (updates workspace binding, loads cached index first for speed, falls back to full rebuild). Response includes workspaceChanged, previousServerDir, indexAction fields. When workspace is UNRESOLVED (wrong CWD), call this with dir=<project_path> to fix it.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "dir": { "type": "string", "description": "Directory to reindex" },
                    "ext": {
                        "type": "string",
                        "description": "File extensions (comma-separated)"
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "xray_reindex_definitions".to_string(),
            description: if def_extensions.is_empty() {
                "Definition index not available. Start server with --definitions flag.".to_string()
            } else {
                format!(
                    "Force rebuild the definition index and reload it into the server's in-memory cache. \
                     Supports {}. Returns build metrics: files parsed, definitions extracted, call sites, \
                     codeStatsEntries (methods with complexity metrics), parse errors, build time, and index size. \
                     After rebuild, code stats are available for includeCodeStats/sortBy/min* queries. \
                     Requires server started with --definitions flag.",
                    lang_list
                )
            },
            input_schema: json!({
                "type": "object",
                "properties": {
                    "dir": { "type": "string", "description": "Directory to reindex (default: server's --dir)" },
                    "ext": {
                        "type": "string",
                        "description": "File extensions to parse, comma-separated (default: server's --ext)"
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "xray_definitions".to_string(),
            description: if def_extensions.is_empty() {
                "Definition index not available for current file extensions. Use xray_grep for content search.".to_string()
            } else {
                format!(
                    "PREFERRED for code exploration AND module structure discovery. \
                     REPLACES directory listing for understanding code — use file='<dirname>' to get ALL classes, methods, \
                     interfaces in ONE call (more informative than directory tree which only shows file names). \
                     REPLACES read_file for indexed source files — use includeBody=true maxBodyLines=0 to get full file content. \
                     Search code definitions — classes, interfaces, methods, properties, enums. \
                     Uses pre-built AST index for instant results (~0.001s). \
                     LANGUAGE-SPECIFIC: Supports {}. Only these extensions are indexed — for other file types (JSON, config, MD) use xray_grep. XML on-demand: for .xml, .config, .csproj, .manifestxml, .props, .targets, .resx files, use file='<path>' with containsLine=<N> or name='<element>' to parse XML on-the-fly and get structural context with parent promotion (leaf elements are auto-promoted to parent block). The name filter searches both element names AND text content of leaf elements (e.g., name='PremiumStorage' finds <ServiceType>PremiumStorage</ServiceType> and returns the parent block with matchedBy='textContent', matchedChild='ServiceType'). Text content search requires term >= 3 chars. Multiple leaf matches in same parent are de-duplicated into one result with matchedChildren array. Name matches take priority over textContent matches. Passing a directory path returns a clear error with guidance to use xray_fast. \
                     Requires server started with --definitions flag. \
                     Supports 'containsLine' to find which method/class contains a given line number. \
                     Supports 'includeBody' to return actual source code inline. \
                     When results exceed maxResults and no name filter is set, automatically returns a \
                     directory-grouped summary (autoSummary) with definition counts per subdirectory and \
                     top-3 largest classes/interfaces per group, instead of truncated entries. \
                     Add a name filter or narrow the file scope to get individual definitions. \
                     ADVISORY HINTS: Property/Field results may include 'valueSourceHint' (string) when the \
                     symbol carries an attribute with a string-literal argument — the hint surfaces those \
                     literals as ready-to-grep keys for external config files (manifest, appsettings, env, \
                     secrets). Shape-based: it does NOT classify the attribute as a binder, only frames \
                     'if any attribute binds to external configuration, search here'.",
                    lang_list
                )
            },
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name to search (substring). Comma-separated for multi-term OR."
                    },
                    "kind": {
                        "type": "string",
                        "description": "Filter by definition kind. Comma-separated for multi-kind OR (e.g., 'class,interface,enum'). Valid values: class, interface, method, property, field, enum, struct, record, constructor, delegate, event, enumMember, function, typeAlias, variable, storedProcedure, table, view, sqlFunction, userDefinedType, column, sqlIndex."
                    },
                    "attribute": {
                        "type": "string",
                        "description": "Filter by C# attribute name."
                    },
                    "baseType": {
                        "type": "string",
                        "description": "Filter by base type or implemented interface (substring match — 'IAccessTable' finds IAccessTable<Model>, IAccessTable<Report>, etc.)."
                    },
                    "baseTypeTransitive": {
                        "type": "boolean",
                        "description": "When true with baseType, traverses inheritance chain transitively (BFS, max depth 10). Finds classes that inherit from classes that inherit from the specified baseType. Known limitation: name-only matching (no namespace resolution). (default: false)"
                    },
                    "file": {
                        "type": "string",
                        "description": "Filter by file path substring. Comma-separated for multi-term OR. Use file='<dirname>' to explore an entire module — returns all definitions in files matching this directory path."
                    },
                    "parent": {
                        "type": "string",
                        "description": "Filter by parent/containing class name. Comma-separated for multi-term OR."
                    },
                    "containsLine": {
                        "type": "integer",
                        "description": "Find definition(s) containing this line number. Returns innermost method + parent class. Requires 'file' parameter. With includeBody=true, body is emitted ONLY for the innermost (most specific) definition; parent definitions get 'bodyOmitted' hint instead — this maximizes body budget for the target method."
                    },
                    "regex": {
                        "type": "boolean",
                        "description": "Treat name as regex pattern (default: false)."
                    },
                    "maxResults": {
                        "type": "integer",
                        "description": "Max results (default: 100, 0=unlimited)"
                    },
                    "excludeDir": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Directory names to exclude"
                    },
                    "includeBody": {
                        "type": "boolean",
                        "description": "Include source code body in results. Use maxBodyLines to control size. (default: false)"
                    },
                    "includeDocComments": {
                        "type": "boolean",
                        "description": "Expand body upward to include doc-comments (/// in C#/Rust, /** */ JSDoc in TypeScript). Implies includeBody=true. Adds 'docCommentLines' field showing how many lines are comments. Budget-aware: respects maxBodyLines. (default: false)"
                    },
                    "maxBodyLines": {
                        "type": "integer",
                        "description": "Max source lines per definition when includeBody=true (default: 100, 0=unlimited)"
                    },
                    "maxTotalBodyLines": {
                        "type": "integer",
                        "description": "Max total body lines across all results (default: 500, 0=unlimited)"
                    },
                    "bodyLineStart": {
                        "type": "integer",
                        "description": "Filter body to start at this absolute file line number (1-based, inclusive). Use with bodyLineEnd to extract a precise slice from a large method body, avoiding response truncation. Example: containsLine=1335, bodyLineStart=1330, bodyLineEnd=1345 returns only 15 lines of the body."
                    },
                    "bodyLineEnd": {
                        "type": "integer",
                        "description": "Filter body to end at this absolute file line number (1-based, inclusive). Use with bodyLineStart for precise body extraction."
                    },
                    "audit": {
                        "type": "boolean",
                        "description": "Return index coverage report instead of search results. (default: false)"
                    },
                    "auditMinBytes": {
                        "type": "integer",
                        "description": "Min file size to flag as suspicious in audit (default: 500)"
                    },
                    "crossValidate": {
                        "type": "boolean",
                        "description": "When used with audit=true, compares definition index files against file-list index to find coverage gaps. Loads file-list index from disk. (default: false)"
                    },
                    "includeCodeStats": {
                        "type": "boolean",
                        "description": "Include complexity metrics (cyclomatic, cognitive, nesting, params, returns, calls, lambdas). Auto-enabled by sortBy/min*. (default: false)"
                    },
                    "sortBy": {
                        "type": "string",
                        "enum": ["cyclomaticComplexity", "cognitiveComplexity", "maxNestingDepth", "paramCount", "returnCount", "callCount", "lambdaCount", "lines"],
                        "description": "Sort by metric descending (worst first). Auto-enables includeCodeStats."
                    },
                    "minComplexity": {
                        "type": "integer",
                        "description": "Min cyclomatic complexity. Auto-enables includeCodeStats. Multiple min* combine with AND."
                    },
                    "minCognitive": {
                        "type": "integer",
                        "description": "Min cognitive complexity. Auto-enables includeCodeStats."
                    },
                    "minNesting": {
                        "type": "integer",
                        "description": "Min nesting depth. Auto-enables includeCodeStats."
                    },
                    "minParams": {
                        "type": "integer",
                        "description": "Min parameter count. Auto-enables includeCodeStats."
                    },
                    "minReturns": {
                        "type": "integer",
                        "description": "Min return/throw count. Auto-enables includeCodeStats."
                    },
                    "minCalls": {
                        "type": "integer",
                        "description": "Min call count (fan-out). Auto-enables includeCodeStats."
                    },
                    "includeUsageCount": {
                        "type": "boolean",
                        "description": "Add usageCount to each definition — number of files containing this name in content index (not call count). Useful for dead code detection (usageCount=0 or 1). Counts ALL text occurrences including comments and strings. (default: false)"
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "xray_callers".to_string(),
            description: if def_extensions.is_empty() {
                "Call chain analysis not available — definition index not configured. Start server with --definitions flag.".to_string()
            } else {
                format!(
                    "RECOMMENDED for call chain analysis -- find all callers of a method and build a call tree \
                     (up or down) in a SINGLE sub-millisecond request. Supports {}. DI-aware. Returns a hierarchical \
                     call tree with method signatures, file paths, and line numbers. Always specify the 'class' parameter \
                     to avoid mixing callers from unrelated classes. Requires server started with --definitions flag. \
                     Limitation: calls through local variables (e.g., `var x = service.GetFoo(); x.Bar()`) may not be \
                     detected because the tool uses AST parsing without type inference. DI-injected fields, `this`/`base` \
                     calls, and direct receiver calls are fully supported. \
                     ADVISORY HINTS (direction='up' only): the response may include an 'advisories' field (array of \
                     strings) flagging known AST blind spots: (a) when class= is passed and the class implements an \
                     interface, suggests re-running with class=IFoo to include interface-receiver call sites; \
                     (b) when 1–3 callers are returned (and the result was NOT truncated), suggests an \
                     'xray_grep terms=METHOD countOnly=true' cross-check to verify completeness against AST blind spots \
                     (DI/dynamic dispatch). Suppressed when 'truncated' or 'perLevelTruncated' is true.",
                    lang_list
                )
            },
            input_schema: json!({
                "type": "object",
                "properties": {
                    "method": {
                        "type": "string",
                        "description": "Method name to find callers/callees for. Comma-separated for multi-method batch (e.g., 'Foo,Bar,Baz'). Each method gets an independent call tree. Single method returns {callTree: [...]}, multiple methods return {results: [{method, callTree}, ...]}."
                    },
                    "class": {
                        "type": "string",
                        "description": "STRONGLY RECOMMENDED: Parent class name to scope the search. Without this, callers of ALL methods with this name across the entire codebase are found, which may mix results from unrelated classes and produce misleading call trees. Always specify when you know the containing class. DI-aware: automatically includes callers that use the interface (e.g., class='UserService' also finds callers using IUserService)."
                    },
                    "depth": {
                        "type": "integer",
                        "description": "Max recursion depth (default: 3, max: 10)"
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["up", "down"],
                        "description": "'up' = callers (default), 'down' = callees."
                    },
                    "ext": {
                        "type": "string",
                        "description": "File extension filter (default: server's --ext)"
                    },
                    "excludeDir": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Directory names to exclude"
                    },
                    "excludeFile": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "File path substrings to exclude"
                    },
                    "maxCallersPerLevel": {
                        "type": "integer",
                        "description": "Max callers per tree node (default: 10)"
                    },
                    "maxTotalNodes": {
                        "type": "integer",
                        "description": "Max total nodes in call tree (default: 200)"
                    },
                    "resolveInterfaces": {
                        "type": "boolean",
                        "description": "Auto-resolve interface methods to implementations (default: true)"
                    },
                    "includeBody": {
                        "type": "boolean",
                        "description": "Include source code body of each method in the call tree, plus a 'rootMethod' object with the searched method's own body. Eliminates the need for a separate xray_definitions call. (default: false)"
                    },
                    "includeDocComments": {
                        "type": "boolean",
                        "description": "Expand body upward to include doc-comments (/// in C#/Rust, /** */ JSDoc in TypeScript). Implies includeBody=true. Adds 'docCommentLines' field. (default: false)"
                    },
                    "maxBodyLines": {
                        "type": "integer",
                        "description": "Max source lines per method when includeBody=true (default: 30, 0=unlimited)"
                    },
                    "maxTotalBodyLines": {
                        "type": "integer",
                        "description": "Max total body lines across all methods in the tree (default: 300, 0=unlimited)"
                    },
                    "bodyLineStart": {
                        "type": "integer",
                        "description": "Filter rootMethod body to start at this absolute file line number (1-based). Only affects rootMethod body, not caller bodies."
                    },
                    "bodyLineEnd": {
                        "type": "integer",
                        "description": "Filter rootMethod body to end at this absolute file line number (1-based). Use with bodyLineStart for precise extraction."
                    },
                    "impactAnalysis": {
                        "type": "boolean",
                        "description": "When true with direction='up', identifies test methods in the caller chain. Test methods (detected via [Test]/[Fact]/[Theory]/[TestMethod]/#[test] attributes or *.spec.ts/*.test.ts file patterns) are marked with isTest=true and collected in a 'testsCovering' array with full file path, depth (distance from target), and callChain (array of method names from target to test). Recursion stops at test methods. (default: false)"
                    },
                    "includeGrepReferences": {
                        "type": "boolean",
                        "description": "Add grepReferences[] — files containing the method name as text but NOT in the call tree. Catches delegate usage, method groups, reflection. Skipped for method names shorter than 4 characters to avoid noise. (default: false)"
                    }
                },
                "required": ["method"]
            }),
        },
        ToolDefinition {
            name: "xray_edit".to_string(),
            description: "ALWAYS USE THIS instead of apply_diff, search_and_replace, or insert_content for ANY file edit. Edit a file by line-range operations or text-match replacements. Auto-creates new files when they don't exist (treats as empty — use Mode A: operations [{startLine:1, endLine:0, content:'...'}] for new file content). Mode A (operations): Replace/insert/delete lines by line number. Applied bottom-up to avoid offset cascade. Mode B (edits): Find and replace text or regex patterns, or insert content after/before anchor text. Applied sequentially. Returns unified diff. Use dryRun=true to preview without writing — pure preview, no parent directories or temp files are created on disk. Works on any text file (not limited to --dir). Accepts absolute or relative paths. UTF-8 only: files with invalid UTF-8 sequences (Windows-1251, Shift-JIS, GB2312, Latin-1) are rejected with an error rather than silently corrupted via lossy decode. Supports multi-file editing via 'paths' parameter (best-effort transactional: Phase 3a stages all temps + verifies I/O before touching originals; Phase 3b commits renames sequentially — a mid-batch rename failure cannot be rolled back and is surfaced as an error with `committed` / `pending` counts). PREFERRED over apply_diff for all file edits — atomic write per file (temp + fsync + rename), per-call unique temp names so concurrent edits do not collide, no whitespace issues, minimal token cost. FLEX-WHITESPACE FALLBACK: Mode B search/replace and insertAfter/insertBefore try exact match first, then strip-trailing-WS, then trim-blank-lines; the 4th step (regex-based whitespace-collapsing match) is opt-in and only runs when the edit carries an `expectedContext` — without `expectedContext` a failed match returns `Text not found` with a hint (no silent cross-block misfires). IDEMPOTENCY: insertAfter/insertBefore detect if the would-be-inserted content already exists adjacent to the anchor and skip the edit (response: skippedDetails[].reason = \"alreadyApplied: ...\"). Safe to retry after partial/unknown success. APPLIED semantics: the `applied` field excludes edits that were skipped via skipIfNotFound or idempotency — it reports only edits that actually mutated the file. LINE ENDINGS: response includes lineEnding (\"LF\" | \"CRLF\"). The returned diff is always LF-based; on CRLF files the on-disk bytes are CRLF, so `git diff` of a CRLF file will not match tool diff character-for-character — use lineEnding to reconcile. POST-WRITE VERIFICATION: after every real write (not dryRun) the file is re-read and compared byte-for-byte to the computed post-state; any mismatch returns an error instead of a misleading success. SYNC REINDEX: after a successful real write (NOT dryRun), the inverted-index and definition-index are refreshed in-process before the response returns — a follow-up xray_grep / xray_definitions / xray_callers / xray_fast call sees the new content with zero latency (no 500ms FS-watcher debounce wait). Response includes new fields (real writes only): contentIndexUpdated (bool), defIndexUpdated (bool — true only when server has --definitions and parse succeeded), fileListInvalidated (bool — true when a new file is created → xray_fast cache will rebuild on next call), reindexElapsedMs (string, e.g. \"0.42\"), skippedReason (string — set to \"outsideServerDir\" / \"extensionNotIndexed\" / \"insideGitDir\" when the file is written but reindex is skipped because the file is out of the server's indexing scope; the file is still committed to disk). dryRun: true OMITS all reindex fields. Multi-file responses report per-file reindex outcome and a single summary.reindexElapsedMs. If an index lock is poisoned during reindex, the response includes reindexWarning explaining that the FS watcher will reconcile within 500ms — the write itself always succeeds.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path — absolute or relative to server --dir. Mutually exclusive with 'paths'."
                    },
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Array of file paths for multi-file editing. Same edits/operations applied to ALL files. Best-effort transactional: Phase 3a stages all temps + verifies I/O before touching originals; Phase 3b commits the renames sequentially. If a rename fails mid-batch, already-committed files cannot be rolled back — the error message reports `committed` / `pending` counts so the caller can recover. Max 20 files. Mutually exclusive with 'path'."
                    },
                    "operations": {
                        "type": "array",
                        "description": "Line-range edits (Mode A). Mutually exclusive with 'edits'. Applied bottom-up.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "startLine": {
                                    "type": "integer",
                                    "description": "1-based start line (inclusive)"
                                },
                                "endLine": {
                                    "type": "integer",
                                    "description": "1-based end line (inclusive). Set endLine = startLine-1 to INSERT before startLine without deleting."
                                },
                                "content": {
                                    "type": "string",
                                    "description": "Replacement content. Empty string deletes the line range. Newlines create multiple output lines."
                                }
                            },
                            "required": ["startLine", "endLine", "content"]
                        }
                    },
                    "edits": {
                        "type": "array",
                        "description": "Text-match edits (Mode B). Mutually exclusive with 'operations'. Applied sequentially. Each edit is either search/replace OR insertAfter/insertBefore.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "search": {
                                    "type": "string",
                                    "description": "Text to find (literal or regex). Mutually exclusive with insertAfter/insertBefore."
                                },
                                "replace": {
                                    "type": "string",
                                    "description": "Replacement text. Supports $1, $2 capture groups when regex=true."
                                },
                                "occurrence": {
                                    "type": "integer",
                                    "description": "1-based occurrence to target. 0 or omitted = ALL occurrences (search/replace) or first occurrence (insertAfter/insertBefore)."
                                },
                                "insertAfter": {
                                    "type": "string",
                                    "description": "Anchor text — insert content on next line after this text. Mutually exclusive with search/replace and insertBefore. Idempotent: if `content` is already present immediately after the anchor line, the edit is skipped (skippedDetails[].reason = 'alreadyApplied: ...')."
                                },
                                "insertBefore": {
                                    "type": "string",
                                    "description": "Anchor text — insert content on line before this text. Mutually exclusive with search/replace and insertAfter. Idempotent: if `content` is already present immediately before the anchor line, the edit is skipped (skippedDetails[].reason = 'alreadyApplied: ...')."
                                },
                                "content": {
                                    "type": "string",
                                    "description": "Content to insert (required with insertAfter/insertBefore)."
                                },
                                "expectedContext": {
                                    "type": "string",
                                    "description": "Safety check: verify this text exists within ±5 lines of the match. Aborts if not found."
                                },
                                "skipIfNotFound": {
                                    "type": "boolean",
                                    "description": "If true, silently skip this edit when search/anchor text is not found (default: false). Useful with multi-file 'paths' where not all files contain the target text."
                                }
                            }
                        }
                    },
                    "regex": {
                        "type": "boolean",
                        "description": "Treat edit search strings as regex (default: false). Only for Mode B search/replace."
                    },
                    "dryRun": {
                        "type": "boolean",
                        "description": "Preview diff without writing (default: false)"
                    },
                    "expectedLineCount": {
                        "type": "integer",
                        "description": "Safety check: if file has a different line count, abort. Counts the same way editors and xray_definitions/xray_grep do (1-based, trailing newline = terminator, NOT an extra empty line). Reusable from a previous response's `newLineCount`. Honored in both Mode A and Mode B."
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "xray_help".to_string(),
            description: "Show best practices and usage tips for xray tools. Call this when unsure which tool to use or how to optimize queries. Returns a concise guide with tool selection priorities, performance tiers, and common pitfalls.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
    ];

    // Git history tools (always available)
    tools.extend(git::git_tool_definitions());

    tools
}

/// How the workspace directory was determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceBindingMode {
    /// `--dir /explicit/path` — nothing overrides this.
    PinnedCli,
    /// `roots/list` response from MCP client — authoritative.
    ClientRoots,
    /// `xray_reindex dir=X` — explicit LLM/user action.
    ManualOverride,
    /// `--dir .` in a CWD that has source files — temporary until roots.
    DotBootstrap,
    /// Workspace not determined (--dir . in wrong CWD, no roots yet).
    Unresolved,
}

impl std::fmt::Display for WorkspaceBindingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PinnedCli => write!(f, "pinned_cli"),
            Self::ClientRoots => write!(f, "client_roots"),
            Self::ManualOverride => write!(f, "manual_override"),
            Self::DotBootstrap => write!(f, "dot_bootstrap"),
            Self::Unresolved => write!(f, "unresolved"),
        }
    }
}

/// Current workspace status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceStatus {
    /// Workspace determined, indexes ready.
    Resolved,
    /// Workspace switching / indexes building.
    Reindexing,
    /// Workspace not determined, workspace-dependent tools blocked.
    Unresolved,
}

impl std::fmt::Display for WorkspaceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resolved => write!(f, "resolved"),
            Self::Reindexing => write!(f, "reindexing"),
            Self::Unresolved => write!(f, "unresolved"),
        }
    }
}

/// Workspace binding state — tracks how the server's working directory was determined
/// and whether it's ready for use.
pub struct WorkspaceBinding {
    /// Current workspace path.
    pub dir: String,
    /// Cached canonicalized form of `dir` (computed once on set, avoids ~1-5ms syscall per request).
    pub canonical_dir: String,
    /// How the workspace was determined.
    pub mode: WorkspaceBindingMode,
    /// Current status.
    pub status: WorkspaceStatus,
    /// Incremented on every workspace directory change (for generation-safe commits).
    pub generation: u64,
}

impl WorkspaceBinding {
    /// Create a new resolved workspace binding (PinnedCli mode).
    pub fn pinned(dir: String) -> Self {
        let canonical_dir = Self::compute_canonical(&dir);
        Self {
            dir,
            canonical_dir,
            mode: WorkspaceBindingMode::PinnedCli,
            status: WorkspaceStatus::Resolved,
            generation: 1,
        }
    }

    /// Create a new DotBootstrap workspace binding.
    pub fn dot_bootstrap(dir: String) -> Self {
        let canonical_dir = Self::compute_canonical(&dir);
        Self {
            dir,
            canonical_dir,
            mode: WorkspaceBindingMode::DotBootstrap,
            status: WorkspaceStatus::Resolved,
            generation: 1,
        }
    }

    /// Create an unresolved workspace binding.
    pub fn unresolved(dir: String) -> Self {
        let canonical_dir = Self::compute_canonical(&dir);
        Self {
            dir,
            canonical_dir,
            mode: WorkspaceBindingMode::Unresolved,
            status: WorkspaceStatus::Unresolved,
            generation: 0,
        }
    }

    /// Compute canonical form of a path (once, at bind time).
    fn compute_canonical(dir: &str) -> String {
        std::fs::canonicalize(dir)
            .map(|p| crate::clean_path(&p.to_string_lossy()))
            .unwrap_or_else(|_| dir.replace('\\', "/"))
    }

    /// Update dir and recompute canonical form. Use instead of `ws.dir = ...` directly.
    pub fn set_dir(&mut self, dir: String) {
        self.canonical_dir = Self::compute_canonical(&dir);
        self.dir = dir;
    }
}

// Context for tool handlers -- shared state
/// Quick check: does a directory contain any files with the given extensions?
/// Uses a shallow walk (max_depth levels) with early exit on first match.
/// Returns `true` if at least one matching file is found.
pub fn has_source_files(dir: &str, extensions: &[String], max_depth: usize, respect_git_exclude: bool) -> bool {
    use ignore::WalkBuilder;
    let walker = WalkBuilder::new(dir)
        .follow_links(true)
        .git_exclude(respect_git_exclude)
        .max_depth(Some(max_depth))
        .hidden(false)
        .build();
    for entry in walker {
        if let Ok(entry) = entry
            && entry.file_type().is_some_and(|ft| ft.is_file())
                && let Some(ext) = entry.path().extension().and_then(|e| e.to_str())
                    && extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
                        return true;
                    }
    }
    false
}

/// Determine the initial WorkspaceBindingMode based on --dir argument.
/// - Explicit non-dot path → PinnedCli
/// - Dot path with source files → DotBootstrap
/// - Dot path without source files → Unresolved
pub fn determine_initial_binding(dir: &str, extensions: &[String], respect_git_exclude: bool) -> WorkspaceBinding {
    let is_dot = dir == "." || dir == "./" || dir == ".\\";
    if !is_dot {
        // Explicit path — PinnedCli, always resolved
        WorkspaceBinding::pinned(dir.to_string())
    } else {
        // Dot path — check if CWD has source files
        let canonical = std::fs::canonicalize(dir)
            .map(|p| crate::clean_path(&p.to_string_lossy()))
            .unwrap_or_else(|_| dir.to_string());
        if has_source_files(&canonical, extensions, 3, respect_git_exclude) {
            WorkspaceBinding::dot_bootstrap(canonical)
        } else {
            WorkspaceBinding::unresolved(canonical)
        }
    }
}
pub struct HandlerContext {
    pub index: Arc<RwLock<ContentIndex>>,
    pub def_index: Option<Arc<RwLock<DefinitionIndex>>>,
    /// Workspace binding state: directory, mode, status, generation.
    pub workspace: Arc<RwLock<WorkspaceBinding>>,
    pub server_ext: String,
    pub metrics: bool,
    /// Base directory for index file storage.
    /// Production: `index_dir()` (`%LOCALAPPDATA%/xray`).
    /// Tests: test-local temp directory (prevents orphan files).
    pub index_base: PathBuf,
    /// Maximum response size in bytes before truncation kicks in. 0 = no limit.
    pub max_response_bytes: usize,
    /// Whether the content index has been fully built/loaded.
    /// Tools return a "building" message when false.
    pub content_ready: Arc<AtomicBool>,
    /// Whether the definition index has been fully built/loaded.
    /// Tools return a "building" message when false.
    pub def_ready: Arc<AtomicBool>,
    /// Git history cache — populated by background thread (PR 2c).
    /// `None` until cache is built; queries fall back to CLI.
    pub git_cache: Arc<RwLock<Option<GitHistoryCache>>>,
    /// Fast readiness check for git cache (avoids RwLock contention).
    pub git_cache_ready: Arc<AtomicBool>,
    /// Current checked-out branch name (detected at server startup).
    /// Used to inject branchWarning into index-based tool responses.
    pub current_branch: Option<String>,
    /// File extensions with definition parser support (e.g., ["cs", "ts", "tsx", "rs"]).
    /// Computed as intersection of --ext and definition_extensions() at startup.
    /// Used to dynamically generate tool descriptions with correct language lists.
    pub def_extensions: Vec<String>,
    /// In-memory cache for file-list index (used by xray_fast).
    /// `None` means not yet built (lazy initialization on first xray_fast call).
    pub file_index: Arc<RwLock<Option<FileIndex>>>,
    /// Dirty flag for file-list index. Set by watcher on any FS create/delete/rename.
    /// When true, xray_fast rebuilds the file index before serving results.
    ///
    /// **Atomic ordering note (MINOR-6):** all reads/writes of this flag use
    /// `Ordering::Relaxed`. This is intentional and correct because the flag
    /// is a pure *signal* — it tells a reader "something changed, rebuild on
    /// your next call" but is **not** used as a happens-before edge for any
    /// other data. The rebuild itself walks the filesystem from scratch and
    /// does not rely on memory synchronization with the writer. Contrast with
    /// `content_ready` / `def_ready` / `bg_ready` (see [`serve::cmd_serve`])
    /// which publish newly-built index data and therefore require
    /// `Release`/`Acquire` pairs.
    pub file_index_dirty: Arc<AtomicBool>,
    /// Whether a content index build is currently in progress (background thread or reindex).
    /// Used to prevent concurrent builds. Different from `content_ready` which means
    /// "index is available for queries".
    pub content_building: Arc<AtomicBool>,
    /// Whether a definition index build is currently in progress.
    /// Used to prevent concurrent builds. Different from `def_ready`.
    pub def_building: Arc<AtomicBool>,
    /// Generation counter for the file watcher. Each watcher thread receives its
    /// generation at start, and exits when the counter changes (workspace switch).
    /// Supports unlimited sequential workspace switches (unlike a boolean stop flag).
    pub watcher_generation: Arc<AtomicU64>,
    /// Whether --watch was requested at startup. Needed to know whether to
    /// restart the watcher on workspace switch.
    pub watch_enabled: bool,
    /// Debounce milliseconds for the file watcher.
    pub watch_debounce_ms: u64,
    /// Whether to respect .git/info/exclude when rebuilding content / file-list
    /// indexes via MCP (xray_reindex, workspace switch, file-list auto-rebuild).
    /// Initialized from `ServeArgs.respect_git_exclude` at server startup.
    pub respect_git_exclude: bool,
    /// Lock-free counters describing what the file watcher has observed
    /// since startup. Exposed via `xray_info` for diagnosing missed events
    /// (see `docs/bug-reports/bug-2026-04-21-watcher-misses-new-files-both-indexes.md`).
    pub watcher_stats: Arc<crate::mcp::watcher::WatcherStats>,
    /// Whether the periodic-rescan fail-safe is enabled. Needed so
    /// `restart_watcher_for_workspace` can respawn the rescan thread
    /// after a workspace switch — the thread self-exits on generation
    /// change, and the original spawn site in `cmd_serve` only runs once.
    pub periodic_rescan_enabled: bool,
    /// Interval in seconds between periodic rescans. Clamped to
    /// `MIN_RESCAN_INTERVAL_SEC` by `start_periodic_rescan`.
    pub rescan_interval_sec: u64,
}

impl HandlerContext {
    /// Convenience accessor: read the current workspace directory.
    /// This replaces direct access to the old `server_dir` field.
    pub fn server_dir(&self) -> String {
        self.workspace.read().unwrap_or_else(|e| e.into_inner()).dir.clone()
    }

    /// Cached canonical form of server_dir (computed once at workspace bind time).
    /// Avoids ~1-5ms canonicalize() syscall per request on Windows.
    pub fn canonical_server_dir(&self) -> String {
        self.workspace.read().unwrap_or_else(|e| e.into_inner()).canonical_dir.clone()
    }
}

impl Default for HandlerContext {
    fn default() -> Self {
        HandlerContext {
            index: Arc::new(RwLock::new(ContentIndex::default())),
            def_index: None,
            workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(".".to_string()))),
            server_ext: "cs".to_string(),
            metrics: false,
            index_base: PathBuf::from("."),
            max_response_bytes: utils::DEFAULT_MAX_RESPONSE_BYTES,
            content_ready: Arc::new(AtomicBool::new(true)),
            def_ready: Arc::new(AtomicBool::new(true)),
            git_cache: Arc::new(RwLock::new(None)),
            git_cache_ready: Arc::new(AtomicBool::new(false)),
            current_branch: None,
            def_extensions: Vec::new(),
            file_index: Arc::new(RwLock::new(None)),
            file_index_dirty: Arc::new(AtomicBool::new(true)),
            content_building: Arc::new(AtomicBool::new(false)),
            def_building: Arc::new(AtomicBool::new(false)),
            watcher_generation: Arc::new(AtomicU64::new(0)),
            watch_enabled: false,
            watch_debounce_ms: 500,
            respect_git_exclude: false,
            watcher_stats: Arc::new(crate::mcp::watcher::WatcherStats::new()),
            periodic_rescan_enabled: false,
            rescan_interval_sec: 300,
        }
    }
}

/// Message returned when the content index is still building in background.
const INDEX_BUILDING_MSG: &str =
    "Content index is currently being built in the background. Please retry in a few seconds.";

/// Message returned when the definition index is still building in background.
const DEF_INDEX_BUILDING_MSG: &str =
    "Definition index is currently being built in the background. Please retry in a few seconds.";

/// Message returned when xray_reindex is called while a background build is in progress.
const ALREADY_BUILDING_MSG: &str =
    "Index is already being built in the background. Please wait for it to finish.";

/// Minimum response budget for xray_help (32KB).
/// xray_help returns reference content (best practices, strategies, parameter examples)
/// that exceeds the default 16KB search-result budget (~20KB as of 23 tips + parameter examples).
/// 48KB gives comfortable headroom for adding more tips and parameter examples.
const XRAY_HELP_MIN_RESPONSE_BYTES: usize = 49_152;

/// Minimum response budget for tools called with includeBody=true (64KB).
/// When includeBody is true, responses contain source code of methods which
/// can easily exceed the default 16KB budget. 300 body lines × ~80 chars ≈ 24KB
/// plus metadata ≈ 30-35KB. 64KB gives comfortable headroom.
/// Applies globally to any tool with includeBody (currently xray_definitions
/// and xray_callers). Users can increase further via --max-response-kb CLI flag.
const INCLUDE_BODY_MIN_RESPONSE_BYTES: usize = 65_536;

/// Per-method response budget scaling for multi-method batch callers (32KB per method).
/// E.g., 3 methods → max(base, 32KB × 3) = 96KB, capped at 128KB.
const MULTI_METHOD_RESPONSE_BYTES_PER: usize = 32_768;

/// Maximum response budget cap for multi-method batch (128KB).
const MULTI_METHOD_RESPONSE_MAX: usize = 131_072;

/// Returns true when a tool requires the content index to be ready.
fn requires_content_index(tool_name: &str) -> bool {
    // Note: xray_fast uses its own file-list index, not the content index.
    // xray_reindex is NOT included here — it needs to run even when content_ready=false
    // (e.g., after workspace switch). Concurrent build protection is handled by
    // content_building flag inside handle_xray_reindex.
    matches!(tool_name, "xray_grep")
}

/// Returns true when a tool requires the definition index to be ready.
fn requires_def_index(tool_name: &str) -> bool {
    // xray_reindex_definitions is NOT included — it needs to run even when def_ready=false.
    // Concurrent build protection is handled by def_building flag inside the handler.
    matches!(tool_name, "xray_definitions" | "xray_callers")
}

/// Dispatch a tool call to the right handler.
/// When `ctx.metrics` is true, injects performance metrics into the response summary.
pub fn dispatch_tool(
    ctx: &HandlerContext,
    tool_name: &str,
    arguments: &Value,
) -> ToolCallResult {
    let dispatch_start = Instant::now();

    // Workspace gate: always-available tools bypass the workspace check
    let is_workspace_independent = matches!(tool_name,
        "xray_info" | "xray_help" | "xray_reindex" | "xray_reindex_definitions"
    );
    if !is_workspace_independent {
        let ws = ctx.workspace.read().unwrap_or_else(|e| e.into_inner());
        match ws.status {
            WorkspaceStatus::Unresolved => {
                let error_msg = serde_json::json!({
                    "error": "WORKSPACE_UNRESOLVED",
                    "message": format!("Server started with --dir . which resolved to '{}' — no source files found.", ws.dir),
                    "hint": "Call xray_reindex with dir parameter pointing to your project: xray_reindex dir=C:/Projects/MyApp",
                    "serverDir": ws.dir,
                    "workspaceStatus": "unresolved"
                });
                return ToolCallResult::error(error_msg.to_string());
            }
            WorkspaceStatus::Reindexing => {
                let error_msg = serde_json::json!({
                    "error": "WORKSPACE_REINDEXING",
                    "message": format!("Workspace is switching to '{}'. Indexes are not ready yet.", ws.dir),
                    "hint": "Call xray_reindex to complete the workspace switch and build indexes.",
                    "serverDir": ws.dir,
                    "workspaceStatus": "reindexing"
                });
                return ToolCallResult::error(error_msg.to_string());
            }
            WorkspaceStatus::Resolved => { /* proceed */ }
        }
    }

    // Check readiness: if the required index is still building, return early.
    // Note: xray_reindex and xray_reindex_definitions are NOT in these guards —
    // they need to run even when ready=false (e.g., after workspace switch).
    // Their concurrent build protection uses content_building/def_building flags.
    if requires_content_index(tool_name) && !ctx.content_ready.load(Ordering::Acquire) {
        return ToolCallResult::error(INDEX_BUILDING_MSG.to_string());
    }
    if requires_def_index(tool_name) && !ctx.def_ready.load(Ordering::Acquire) {
        return ToolCallResult::error(DEF_INDEX_BUILDING_MSG.to_string());
    }

    // Strict args validation: detect unknown/aliased keys before dispatch.
    // - Default: silently collect into `unknown_args_report`, inject as
    //   `summary.unknownArgsWarning` after the handler runs.
    // - With `XRAY_STRICT_ARGS=1`: short-circuit with a hard error so the
    //   caller (CI, scripted agent) cannot proceed against an ignored arg.
    let unknown_args_report = arg_validation::check_unknown_args(tool_name, arguments);
    if let Some(ref rep) = unknown_args_report
        && arg_validation::strict_args_enabled()
    {
        return arg_validation::strict_error_response(tool_name, rep);
    }

    let result = match tool_name {
        "xray_grep" => grep::handle_xray_grep(ctx, arguments),
        "xray_fast" => fast::handle_xray_fast(ctx, arguments),
        "xray_info" => handle_xray_info(ctx),
        "xray_reindex" => handle_xray_reindex(ctx, arguments),
        "xray_reindex_definitions" => handle_xray_reindex_definitions(ctx, arguments),
        "xray_definitions" => definitions::handle_xray_definitions(ctx, arguments),
        "xray_callers" => callers::handle_xray_callers(ctx, arguments),
        "xray_edit" => edit::handle_xray_edit(ctx, arguments),
        "xray_help" => handle_xray_help(ctx),
        // Git history tools
        "xray_git_history" | "xray_git_diff" | "xray_git_authors" | "xray_git_activity" | "xray_git_blame" | "xray_branch_status" => {
            git::dispatch_git_tool(ctx, tool_name, arguments)
        }
        _ => return ToolCallResult::error(format!("Unknown tool: {}", tool_name)),
    };

    // A5 fix: Apply guidance injection to ALL responses (success AND error).
    // Previously, error responses skipped inject_response_guidance, losing
    // policyReminder, nextStepHint, and workspace metadata — which could cause
    // LLMs to fall back to built-in tools instead of xray tools.
    let was_error = result.is_error;
    let result = utils::inject_response_guidance(result, tool_name, &ctx.server_ext, ctx);
    // Preserve is_error flag (inject_response_guidance returns ToolCallResult::success)
    let result = if was_error {
        ToolCallResult { is_error: true, ..result }
    } else {
        result
    };

    // Inject unknown-args warning into summary (after guidance so summary exists).
    let result = match unknown_args_report {
        Some(ref rep) => arg_validation::inject_warning(result, rep),
        None => result,
    };

    if result.is_error {
        return result;
    }

    // Determine effective response budget:
    // - xray_help: 32KB (static reference content)
    // - Multi-method callers: 32KB × N, capped at 128KB
    // - Any tool with includeBody=true: 64KB (source code is large)
    // - Everything else: default (16KB)
    let has_include_body = arguments
        .get("includeBody")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || arguments
            .get("includeDocComments")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

    // Count methods for multi-method budget scaling
    let method_count = if tool_name == "xray_callers" {
        arguments.get("method").and_then(|v| v.as_str())
            .map(|m| m.split(',').filter(|s| !s.trim().is_empty()).count())
            .unwrap_or(1)
    } else {
        1
    };

    let effective_max = if tool_name == "xray_help" {
        ctx.max_response_bytes.max(XRAY_HELP_MIN_RESPONSE_BYTES)
    } else if tool_name == "xray_callers" && method_count > 1 {
        // Multi-method batch: scale budget proportionally, cap at 128KB
        let scaled = MULTI_METHOD_RESPONSE_BYTES_PER * method_count;
        ctx.max_response_bytes.max(scaled.min(MULTI_METHOD_RESPONSE_MAX))
    } else if has_include_body {
        ctx.max_response_bytes.max(INCLUDE_BODY_MIN_RESPONSE_BYTES)
    } else {
        ctx.max_response_bytes
    };

    if ctx.metrics {
        if tool_name == "xray_help" {
            // xray_help is static content, no need for metrics injection
            utils::truncate_response_if_needed(result, effective_max)
        } else {
            utils::inject_metrics(result, ctx, dispatch_start, effective_max)
        }
    } else {
        // Even without metrics, apply response size truncation
        utils::truncate_response_if_needed(result, effective_max)
    }
}

// ─── Small inline handlers ──────────────────────────────────────────

fn handle_xray_help(ctx: &HandlerContext) -> ToolCallResult {
    let help = crate::tips::render_json(&ctx.def_extensions);
    ToolCallResult::success(utils::json_to_string(&help))
}

/// Build xray_info response from in-memory indexes only.
/// Previous implementation called `cmd_info_json()` which deserialized ALL index
/// files from disk (~1.8 GB for multi-repo setups), causing a massive memory spike.
/// This version reads stats directly from the already-loaded in-memory structures
/// via read locks — zero additional allocations.
fn handle_xray_info(ctx: &HandlerContext) -> ToolCallResult {
    let mut indexes = Vec::new();
    let mut memory_estimate = json!({});

    // ── Content index (in-memory) ──
    if ctx.content_ready.load(Ordering::Acquire) {
        if let Ok(idx) = ctx.index.read() {
            if !idx.files.is_empty() {
                // Get disk file size without loading
                let exts_str = idx.extensions.join(",");
                let disk_path = crate::index::content_index_path_for(&idx.root, &exts_str, &ctx.index_base);
                let size_mb = std::fs::metadata(&disk_path)
                    .map(|m| (m.len() as f64 / 1_048_576.0 * 10.0).round() / 10.0)
                    .unwrap_or(0.0);

                let age_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or(std::time::Duration::ZERO)
                    .as_secs()
                    .saturating_sub(idx.created_at);

                let mut content_info = json!({
                    "type": "content",
                    "root": idx.root,
                    "files": idx.files.len(),
                    "uniqueTokens": idx.index.len(),
                    "totalTokens": idx.total_tokens,
                    "extensions": idx.extensions,
                    "sizeMb": size_mb,
                    "ageHours": (age_secs as f64 / 3600.0 * 10.0).round() / 10.0,
                    "inMemory": true,
                });
                if idx.worker_panics > 0 {
                    content_info["workerPanics"] = json!(idx.worker_panics);
                    content_info["degraded"] = json!(true);
                }
                indexes.push(content_info);
            }
            memory_estimate["contentIndex"] = crate::index::estimate_content_index_memory(&idx);
        }
    } else {
        indexes.push(json!({
            "type": "content",
            "status": "building",
        }));
    }

    // ── Definition index (in-memory) ──
    if let Some(ref def_arc) = ctx.def_index {
        if ctx.def_ready.load(Ordering::Acquire) {
            if let Ok(idx) = def_arc.read() {
                if !idx.files.is_empty() {
                    let disk_path = crate::definitions::definition_index_path_for(
                        &idx.root, &idx.extensions.join(","), &ctx.index_base,
                    );
                    let size_mb = std::fs::metadata(&disk_path)
                        .map(|m| (m.len() as f64 / 1_048_576.0 * 10.0).round() / 10.0)
                        .unwrap_or(0.0);

                    let age_secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or(std::time::Duration::ZERO)
                        .as_secs()
                        .saturating_sub(idx.created_at);

                    let call_sites: usize = idx.method_calls.values().map(|v| v.len()).sum();
                    let active_defs: usize = idx.file_index.values().map(|v| v.len()).sum();
                    let mut def_info = json!({
                        "type": "definition",
                        "root": idx.root,
                        "files": idx.files.len(),
                        "definitions": active_defs,
                        "callSites": call_sites,
                        "extensions": idx.extensions,
                        "sizeMb": size_mb,
                        "ageHours": (age_secs as f64 / 3600.0 * 10.0).round() / 10.0,
                        "inMemory": true,
                    });
                    if idx.parse_errors > 0 {
                        def_info["readErrors"] = json!(idx.parse_errors);
                    }
                    if idx.lossy_file_count > 0 {
                        def_info["lossyUtf8Files"] = json!(idx.lossy_file_count);
                    }
                    if idx.worker_panics > 0 {
                        def_info["workerPanics"] = json!(idx.worker_panics);
                        def_info["degraded"] = json!(true);
                    }
                    indexes.push(def_info);
                }
                memory_estimate["definitionIndex"] = crate::index::estimate_definition_index_memory(&idx);
            }
        } else {
            indexes.push(json!({
                "type": "definition",
                "status": "building",
            }));
        }
    }

    // ── File list index (disk metadata only — small file, no full deserialization) ──
    {
        let file_index_path = crate::index::index_path_for(&ctx.server_dir(), &ctx.index_base);
        if file_index_path.exists()
            && let Some(root) = crate::index::read_root_from_index_file_pub(&file_index_path) {
                let size_mb = std::fs::metadata(&file_index_path)
                    .map(|m| (m.len() as f64 / 1_048_576.0 * 10.0).round() / 10.0)
                    .unwrap_or(0.0);
                indexes.push(json!({
                    "type": "file-list",
                    "root": root,
                    "sizeMb": size_mb,
                }));
            }
    }

    // ── Git cache (in-memory) ──
    if ctx.git_cache_ready.load(Ordering::Acquire)
        && let Ok(guard) = ctx.git_cache.read()
            && let Some(ref cache) = *guard {
                let cache_path = crate::git::cache::GitHistoryCache::cache_path_for(&ctx.server_dir(), &ctx.index_base);
                let size_mb = std::fs::metadata(&cache_path)
                    .map(|m| (m.len() as f64 / 1_048_576.0 * 10.0).round() / 10.0)
                    .unwrap_or(0.0);

                indexes.push(json!({
                    "type": "git-history",
                    "commits": cache.commits.len(),
                    "files": cache.file_commits.len(),
                    "authors": cache.authors.len(),
                    "branch": cache.branch,
                    "headHash": cache.head_hash,
                    "sizeMb": size_mb,
                    "inMemory": true,
                }));
                memory_estimate["gitCache"] = crate::index::estimate_git_cache_memory(cache);
            }

    // ── Process memory info (Windows only) ──
    let process_memory = crate::index::get_process_memory_info();
    if !process_memory.as_object().is_none_or(|m| m.is_empty()) {
        memory_estimate["process"] = process_memory;
    }

    // Workspace state
    let workspace_state = {
        let ws = ctx.workspace.read().unwrap_or_else(|e| e.into_inner());
        json!({
            "dir": ws.dir,
            "mode": ws.mode.to_string(),
            "status": ws.status.to_string(),
            "generation": ws.generation,
        })
    };

    let mut info = json!({
        "directory": ctx.index_base.display().to_string(),
        "workspace": workspace_state,
        "indexes": indexes,
    });

    // ── Watcher observability (Phase 0 of periodic-rescan rollout) ──
    // Only emitted when --watch is enabled. Counters help diagnose missed
    // events: a non-zero `eventsEmptyPaths` is a strong signal that the
    // notify backend dropped path metadata; index drift between filesystem
    // and indexes can occur silently in such cases.
    if ctx.watch_enabled {
        let effective_rescan = if ctx.periodic_rescan_enabled {
            Some(ctx.rescan_interval_sec.max(crate::mcp::watcher::MIN_RESCAN_INTERVAL_SEC))
        } else {
            None
        };
        info["watcher"] = json!({
            "eventsTotal": ctx.watcher_stats.events_total.load(Ordering::Relaxed),
            "eventsEmptyPaths": ctx.watcher_stats.events_empty_paths.load(Ordering::Relaxed),
            "eventsErrors": ctx.watcher_stats.events_errors.load(Ordering::Relaxed),
            "periodicRescanTotal": ctx.watcher_stats.periodic_rescan_total.load(Ordering::Relaxed),
            "periodicRescanDriftEvents": ctx.watcher_stats.periodic_rescan_drift_events.load(Ordering::Relaxed),
            "periodicRescanEnabled": ctx.periodic_rescan_enabled,
            // Effective value after clamp to MIN_RESCAN_INTERVAL_SEC; null when rescan is disabled.
            "effectiveRescanIntervalSec": effective_rescan,
        });
    }

    if !memory_estimate.as_object().is_none_or(|m| m.is_empty()) {
        info["memoryEstimate"] = memory_estimate;
    }

    ToolCallResult::success(utils::json_to_string(&info))
}

fn handle_xray_reindex(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    // Concurrent build protection: prevent two reindex calls from running simultaneously.
    // Uses compare_exchange for atomic "test-and-set" — only one caller wins.
    if ctx.content_building.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return ToolCallResult::error(ALREADY_BUILDING_MSG.to_string());
    }

    // SAFETY: from this point, content_building=true. We MUST reset it on ALL exit paths.
    let result = handle_xray_reindex_inner(ctx, args);
    ctx.content_building.store(false, Ordering::Release);
    result
}

/// Rolls back workspace state to pre-switch values after a failed reindex.
/// Call this in every error branch when `workspace_changed` is true.
fn rollback_workspace_state(
    ctx: &HandlerContext,
    previous_dir: &str,
    old_mode: WorkspaceBindingMode,
    old_generation: u64,
) {
    let mut ws = ctx.workspace.write().unwrap_or_else(|e| e.into_inner());
    ws.set_dir(previous_dir.to_string());
    ws.mode = old_mode;
    ws.generation = old_generation;
    ws.status = WorkspaceStatus::Resolved;
}

/// Build or load a content index for the given directory.
/// Returns (index, action_str) on success, or error message on failure.
fn build_or_load_content_index(
    dir: &str,
    ext: &str,
    index_base: &std::path::Path,
    respect_git_exclude: bool,
) -> Result<(ContentIndex, &'static str), String> {
    let extensions: Vec<String> = ext.split(',').map(|s| s.trim().to_string()).collect();
    let loaded = load_content_index(dir, ext, index_base)
        .ok()
        .or_else(|| find_content_index_for_dir(dir, index_base, &extensions));

    if let Some(idx) = loaded {
        return Ok((idx, "loaded_cache"));
    }

    // Build from scratch
    let idx = build_content_index(&ContentIndexArgs {
        dir: dir.to_string(),
        ext: ext.to_string(),
        max_age_hours: 24,
        hidden: false,
        no_ignore: false, respect_git_exclude,
        threads: 0,
        min_token_len: DEFAULT_MIN_TOKEN_LEN,
    }).map_err(|e| format!("Failed to build content index: {}", e))?;

    // Save to disk
    if let Err(e) = save_content_index(&idx, index_base) {
        warn!(error = %e, "Failed to save reindexed content to disk");
    }

    // Drop build result and reload from disk for compact memory layout
    drop(idx);
    crate::index::force_mimalloc_collect();
    crate::index::log_memory("reindex: after drop+mi_collect (content)");

    let reloaded = match load_content_index(dir, ext, index_base) {
        Ok(idx) => idx,
        Err(e) => {
            warn!(error = %e, "Failed to reload content index from disk after reindex, rebuilding");
            build_content_index(&ContentIndexArgs {
                dir: dir.to_string(), ext: ext.to_string(),
                max_age_hours: 24, hidden: false, no_ignore: false, respect_git_exclude,
                threads: 0, min_token_len: DEFAULT_MIN_TOKEN_LEN,
            }).map_err(|e2| format!("Failed to rebuild content index: {}", e2))?
        }
    };
    Ok((reloaded, "rebuilt"))
}

/// Cross-load definition index on workspace switch.
/// Returns the action taken: "loaded_cache", "background_build", or None.
fn cross_load_definition_index(ctx: &HandlerContext, dir: &str) -> Option<&'static str> {
    let def_arc = ctx.def_index.as_ref()?;
    let def_ext_str = ctx.def_extensions.join(",");
    let def_ext_vec: Vec<String> = ctx.def_extensions.clone();

    // Try cache load
    let def_loaded = crate::definitions::load_definition_index(dir, &def_ext_str, &ctx.index_base).ok()
        .or_else(|| crate::definitions::find_definition_index_for_dir(dir, &ctx.index_base, &def_ext_vec));

    if let Some(mut idx) = def_loaded {
        idx.shrink_maps();
        *def_arc.write().unwrap_or_else(|e| e.into_inner()) = idx;
        ctx.def_ready.store(true, Ordering::Release);
        info!(dir = %dir, "Definition index cross-loaded from cache on workspace switch");
        return Some("loaded_cache");
    }

    // No cache — start background build
    ctx.def_ready.store(false, Ordering::Release);
    let bg_def = Arc::clone(def_arc);
    let bg_dir = dir.to_string();
    let bg_ext = def_ext_str;
    let bg_idx_base = ctx.index_base.clone();
    let bg_ready = Arc::clone(&ctx.def_ready);
    let bg_building = Arc::clone(&ctx.def_building);
    let bg_respect = ctx.respect_git_exclude;
    std::thread::spawn(move || {
        if bg_building.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return; // another build already running
        }
        info!(dir = %bg_dir, "Building definition index in background (workspace switch)");
        let new_idx = crate::definitions::build_definition_index(&crate::definitions::DefIndexArgs {
            dir: bg_dir.clone(), ext: bg_ext.clone(), threads: 0,
            respect_git_exclude: bg_respect,
        });
        if let Err(e) = crate::definitions::save_definition_index(&new_idx, &bg_idx_base) {
            warn!(error = %e, "Failed to save definition index to disk");
        }
        // Drop + reload pattern for compact memory
        drop(new_idx);
        crate::index::force_mimalloc_collect();
        let new_idx = crate::definitions::load_definition_index(&bg_dir, &bg_ext, &bg_idx_base)
            .unwrap_or_else(|e| {
                warn!(error = %e, "Failed to reload def index, rebuilding");
                crate::definitions::build_definition_index(&crate::definitions::DefIndexArgs {
                    dir: bg_dir, ext: bg_ext, threads: 0,
                    respect_git_exclude: bg_respect,
                })
            });
        *bg_def.write().unwrap_or_else(|e| e.into_inner()) = new_idx;
        bg_building.store(false, Ordering::Release);
        bg_ready.store(true, Ordering::Release);
        crate::index::log_memory("reindex: def cross-build complete");
    });
    info!(dir = %dir, "Definition index background build started (no cache)");
    Some("background_build")
}

/// Cross-load content index on workspace switch from `handle_xray_reindex_definitions_inner`.
/// Mirrors `cross_load_definition_index` for symmetry.
/// Tries cache first, then schedules a background build if no cache exists.
/// Returns `Some("loaded_cache")` or `Some("background_build")` describing the action taken.
fn cross_load_content_index(ctx: &HandlerContext, dir: &str) -> Option<&'static str> {
    let content_ext = ctx.server_ext.clone();
    let content_loaded = load_content_index(dir, &content_ext, &ctx.index_base).ok()
        .or_else(|| {
            let ext_vec: Vec<String> = content_ext.split(',').map(|s| s.to_string()).collect();
            find_content_index_for_dir(dir, &ctx.index_base, &ext_vec)
        });

    if let Some(idx) = content_loaded {
        let had_watch = ctx.index.read()
            .map(|i| i.path_to_id.is_some())
            .unwrap_or(false);
        let idx = if had_watch {
            crate::mcp::watcher::build_watch_index_from(idx)
        } else {
            idx
        };
        match ctx.index.write() {
            Ok(mut current) => {
                *current = idx;
                ctx.content_ready.store(true, Ordering::Release);
                info!(dir = %dir, "Content index cross-loaded from cache on workspace switch");
            }
            Err(e) => {
                warn!(error = %e, "Failed to update content index on workspace switch");
            }
        }
        // Invalidate file-list index
        if let Ok(mut fi) = ctx.file_index.write() { *fi = None; }
        ctx.file_index_dirty.store(true, Ordering::Relaxed);
        return Some("loaded_cache");
    }

    // No cache — start background build
    ctx.content_ready.store(false, Ordering::Release);
    let bg_index = Arc::clone(&ctx.index);
    let bg_dir = dir.to_string();
    let bg_ext = content_ext;
    let bg_idx_base = ctx.index_base.clone();
    let bg_ready = Arc::clone(&ctx.content_ready);
    let bg_building = Arc::clone(&ctx.content_building);
    let bg_file_dirty = Arc::clone(&ctx.file_index_dirty);
    let bg_respect_git_exclude = ctx.respect_git_exclude;
    std::thread::spawn(move || {
        if bg_building.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return;
        }
        info!(dir = %bg_dir, "Building content index in background (workspace switch)");
        match build_content_index(&ContentIndexArgs {
            dir: bg_dir.clone(), ext: bg_ext.clone(),
            max_age_hours: 24, hidden: false, no_ignore: false, respect_git_exclude: bg_respect_git_exclude,
            threads: 0, min_token_len: DEFAULT_MIN_TOKEN_LEN,
        }) {
            Ok(idx) => {
                if let Err(e) = save_content_index(&idx, &bg_idx_base) {
                    warn!(error = %e, "Failed to save content index");
                }
                *bg_index.write().unwrap_or_else(|e| e.into_inner()) = idx;
                bg_file_dirty.store(true, Ordering::Relaxed);
            }
            Err(e) => {
                warn!(error = %e, "Failed to build content index");
            }
        }
        bg_building.store(false, Ordering::Release);
        bg_ready.store(true, Ordering::Release);
        crate::index::log_memory("reindex_def: content cross-build complete");
    });
    info!(dir = %dir, "Content index background build started (no cache)");
    Some("background_build")
}


/// Restart file watcher for a new workspace directory.
fn restart_watcher_for_workspace(ctx: &HandlerContext, dir: &str) {
    // Increment watcher generation — old watcher will detect the mismatch and exit.
    // This supports unlimited sequential workspace switches (generation counter, not boolean).
    let new_gen = ctx.watcher_generation.fetch_add(1, Ordering::Release) + 1;
    // Start new watcher for new directory.
    let watch_dir = std::fs::canonicalize(dir)
        .unwrap_or_else(|_| std::path::PathBuf::from(dir));
    let ext_vec: Vec<String> = ctx.server_ext.split(',').map(|s| s.trim().to_string()).collect();
    if let Err(e) = crate::mcp::watcher::start_watcher(
        Arc::clone(&ctx.index),
        ctx.def_index.as_ref().map(Arc::clone),
        watch_dir,
        ext_vec,
        ctx.watch_debounce_ms,
        ctx.index_base.clone(),
        Arc::clone(&ctx.content_ready),
        Arc::clone(&ctx.def_ready),
        Arc::clone(&ctx.file_index_dirty),
        Arc::clone(&ctx.watcher_generation),
        new_gen,
        Arc::clone(&ctx.watcher_stats),
        ctx.respect_git_exclude,
    ) {
        warn!(error = %e, "Failed to restart file watcher for new workspace");
    } else {
        info!(dir = %dir, generation = new_gen, "File watcher restarted for new workspace");
    }

    // ── Respawn the periodic-rescan fail-safe for the new workspace ──
    // The previous rescan thread exits on generation change (see
    // `start_periodic_rescan`), so without this call the Phase-3 fail-safe
    // would be permanently disabled after any workspace switch.
    if ctx.periodic_rescan_enabled {
        let watch_dir_rescan = std::fs::canonicalize(dir)
            .unwrap_or_else(|_| std::path::PathBuf::from(dir));
        let ext_vec_rescan: Vec<String> = ctx.server_ext.split(',').map(|s| s.trim().to_string()).collect();
        crate::mcp::watcher::start_periodic_rescan(
            Arc::clone(&ctx.index),
            ctx.def_index.as_ref().map(Arc::clone),
            Arc::clone(&ctx.file_index),
            Arc::clone(&ctx.file_index_dirty),
            watch_dir_rescan,
            ext_vec_rescan,
            ctx.rescan_interval_sec,
            Arc::clone(&ctx.watcher_generation),
            new_gen,
            Arc::clone(&ctx.watcher_stats),
            ctx.respect_git_exclude,
        );
    }
}

/// Rebuild git history cache for a new workspace directory (background thread).
fn rebuild_git_cache_for_workspace(ctx: &HandlerContext, dir: &str) {
    // Clear stale cache for old workspace
    if let Ok(mut cache) = ctx.git_cache.write() {
        *cache = None;
    }
    ctx.git_cache_ready.store(false, Ordering::Release);

    // Start background rebuild
    let bg_git_cache = Arc::clone(&ctx.git_cache);
    let bg_git_ready = Arc::clone(&ctx.git_cache_ready);
    let bg_dir = dir.to_string();
    let bg_idx_base = ctx.index_base.clone();
    std::thread::spawn(move || {
        let repo_path = std::path::PathBuf::from(&bg_dir);
        let git_dir = repo_path.join(".git");
        if !git_dir.exists() {
            info!(dir = %bg_dir, "No .git directory in new workspace, skipping git cache");
            bg_git_ready.store(true, Ordering::Release);
            return;
        }
        let branch = match GitHistoryCache::detect_default_branch(&repo_path) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "Failed to detect default branch for new workspace");
                bg_git_ready.store(true, Ordering::Release);
                return;
            }
        };
        let cache_path = GitHistoryCache::cache_path_for(&bg_dir, &bg_idx_base);
        // Try load from disk first
        let cache = if cache_path.exists() {
            GitHistoryCache::load_from_disk(&cache_path).ok()
        } else {
            None
        };
        let cache = match cache {
            Some(c) => c,
            None => {
                match GitHistoryCache::build(&repo_path, &branch) {
                    Ok(c) => {
                        let _ = c.save_to_disk(&cache_path);
                        c
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to build git cache for new workspace");
                        bg_git_ready.store(true, Ordering::Release);
                        return;
                    }
                }
            }
        };
        info!(dir = %bg_dir, commits = cache.commits.len(), "Git cache rebuilt for new workspace");
        *bg_git_cache.write().unwrap_or_else(|e| e.into_inner()) = Some(cache);
        bg_git_ready.store(true, Ordering::Release);
    });
}

fn handle_xray_reindex_inner(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    let current_dir = ctx.server_dir();
    let dir = args.get("dir").and_then(|v| v.as_str())
        .map(|d| {
            std::fs::canonicalize(d)
                .map(|p| clean_path(&p.to_string_lossy()))
                .unwrap_or_else(|_| d.to_string())
        })
        .unwrap_or_else(|| current_dir.clone());
    let ext = args.get("ext").and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| ctx.server_ext.clone());

    // Determine if workspace is changing
    let previous_dir = current_dir.clone();
    let workspace_changed = !code_xray::path_eq(&dir, &previous_dir);

    // Check if workspace switch is allowed (only blocked in PinnedCli mode)
    if workspace_changed {
        let ws = ctx.workspace.read().unwrap_or_else(|e| e.into_inner());
        if ws.mode == WorkspaceBindingMode::PinnedCli {
            return ToolCallResult::error(format!(
                "Server started with --dir {} (pinned). Cannot switch workspace. \
                 Start another server instance or use CLI.",
                previous_dir
            ));
        }
    }

    // Save old state for rollback on error
    let (old_mode, old_generation) = {
        let ws = ctx.workspace.read().unwrap_or_else(|e| e.into_inner());
        (ws.mode, ws.generation)
    };

    // Update workspace binding if dir changed
    if workspace_changed {
        let mut ws = ctx.workspace.write().unwrap_or_else(|e| e.into_inner());
        ws.set_dir(dir.clone());
        ws.mode = WorkspaceBindingMode::ManualOverride;
        ws.generation += 1;
        ws.status = WorkspaceStatus::Reindexing;
        info!(dir = %dir, previous = %previous_dir, generation = ws.generation, "Workspace switched via xray_reindex");
    }

    info!(dir = %dir, ext = %ext, "Rebuilding content index");
    let start = Instant::now();

    // Phase 1: Build or load content index
    let (new_index, index_action) = match build_or_load_content_index(
        &dir, &ext, &ctx.index_base, ctx.respect_git_exclude,
    ) {
        Ok(result) => result,
        Err(e) => {
            if workspace_changed {
                rollback_workspace_state(ctx, &previous_dir, old_mode, old_generation);
            }
            return ToolCallResult::error(e);
        }
    };

    let file_count = new_index.files.len();
    let token_count = new_index.index.len();

    // Rebuild path_to_id if watcher is active (old index had path_to_id).
    // Without this, watcher becomes no-op after reindex (path_to_id = None).
    let had_watch = ctx.index.read()
        .map(|idx| idx.path_to_id.is_some())
        .unwrap_or(false);
    let new_index = if had_watch {
        crate::mcp::watcher::build_watch_index_from(new_index)
    } else {
        new_index
    };

    // Update in-memory cache
    match ctx.index.write() {
        Ok(mut idx) => {
            *idx = new_index;
        }
        Err(e) => {
            // Rollback workspace state to avoid getting stuck in Reindexing
            if workspace_changed {
                rollback_workspace_state(ctx, &previous_dir, old_mode, old_generation);
            } else {
                let mut ws = ctx.workspace.write().unwrap_or_else(|e| e.into_inner());
                ws.status = WorkspaceStatus::Resolved;
            }
            return ToolCallResult::error(format!("Failed to update in-memory index: {}", e));
        }
    }

    // Mark workspace as resolved and content index as ready.
    // CRITICAL: content_ready must be set to true here because Fix B
    // (handle_pending_response) may have reset it to false during workspace switch.
    {
        let mut ws = ctx.workspace.write().unwrap_or_else(|e| e.into_inner());
        ws.status = WorkspaceStatus::Resolved;
    }
    ctx.content_ready.store(true, Ordering::Release);

    // Force mimalloc to return freed pages (old index) to OS
    crate::index::force_mimalloc_collect();
    crate::index::log_memory("reindex: after replace+mi_collect (content)");

    // Phase 2: Cross-load definition index on workspace switch
    let def_index_action = if workspace_changed {
        cross_load_definition_index(ctx, &dir)
    } else {
        None
    };

    // Phase 3: Restart watcher for new workspace
    if workspace_changed && ctx.watch_enabled {
        restart_watcher_for_workspace(ctx, &dir);
    }

    // Phase 4: Rebuild git cache for new workspace
    if workspace_changed {
        rebuild_git_cache_for_workspace(ctx, &dir);
    }

    let elapsed = start.elapsed();

    let mut output = json!({
        "status": "ok",
        "files": file_count,
        "uniqueTokens": token_count,
        "rebuildTimeMs": elapsed.as_secs_f64() * 1000.0,
        "indexAction": index_action,
    });
    if workspace_changed {
        output["workspaceChanged"] = json!(true);
        output["previousServerDir"] = json!(previous_dir);
    }
    if let Some(action) = def_index_action {
        output["defIndexAction"] = json!(action);
    }
    output["serverDir"] = json!(dir);

    // Invalidate file-list index so xray_fast rebuilds it on next call
    if let Ok(mut fi) = ctx.file_index.write() {
        *fi = None;
    }
    ctx.file_index_dirty.store(true, Ordering::Relaxed);

    ToolCallResult::success(utils::json_to_string(&output))
}

fn handle_xray_reindex_definitions(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    // Concurrent build protection (same pattern as handle_xray_reindex).
    if ctx.def_building.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return ToolCallResult::error(ALREADY_BUILDING_MSG.to_string());
    }

    let result = handle_xray_reindex_definitions_inner(ctx, args);
    ctx.def_building.store(false, Ordering::Release);
    result
}

fn handle_xray_reindex_definitions_inner(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    let def_index_arc = match &ctx.def_index {
        Some(di) => Arc::clone(di),
        None => return ToolCallResult::error(
            "Definition index not available. Start server with --definitions flag.".to_string()
        ),
    };

    let current_dir = ctx.server_dir();
    let dir = args.get("dir").and_then(|v| v.as_str())
        .map(|d| {
            std::fs::canonicalize(d)
                .map(|p| clean_path(&p.to_string_lossy()))
                .unwrap_or_else(|_| d.to_string())
        })
        .unwrap_or_else(|| current_dir.clone());
    let ext = args.get("ext").and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| ctx.server_ext.clone());

    // Determine if workspace is changing
    let previous_dir = current_dir.clone();
    let workspace_changed = !code_xray::path_eq(&dir, &previous_dir);

    // Check if workspace switch is allowed (only blocked in PinnedCli mode)
    if workspace_changed {
        let ws = ctx.workspace.read().unwrap_or_else(|e| e.into_inner());
        if ws.mode == WorkspaceBindingMode::PinnedCli {
            return ToolCallResult::error(format!(
                "Server started with --dir {} (pinned). Cannot switch workspace. \
                 Start another server instance or use CLI.",
                previous_dir
            ));
        }
    }

    // Save old state for rollback on error
    let (old_mode, old_generation) = {
        let ws = ctx.workspace.read().unwrap_or_else(|e| e.into_inner());
        (ws.mode, ws.generation)
    };

    // Update workspace binding if dir changed
    if workspace_changed {
        let mut ws = ctx.workspace.write().unwrap_or_else(|e| e.into_inner());
        ws.set_dir(dir.clone());
        ws.mode = WorkspaceBindingMode::ManualOverride;
        ws.generation += 1;
        ws.status = WorkspaceStatus::Reindexing;
        info!(dir = %dir, previous = %previous_dir, generation = ws.generation, "Workspace switched via xray_reindex_definitions");
    }

    info!(dir = %dir, ext = %ext, "Rebuilding definition index");
    let start = Instant::now();

    let new_index = crate::definitions::build_definition_index(&crate::definitions::DefIndexArgs {
        dir: dir.to_string(),
        ext: ext.clone(),
        threads: 0,
        respect_git_exclude: ctx.respect_git_exclude,
    });

    // Save to disk
    if let Err(e) = crate::definitions::save_definition_index(&new_index, &ctx.index_base) {
        warn!(error = %e, "Failed to save definition index to disk");
    }

    let file_count = new_index.files.len();
    let def_count = new_index.definitions.len();
    let call_site_count: usize = new_index.method_calls.values().map(|v| v.len()).sum();
    let code_stats_count = new_index.code_stats.len();

    // Compute index size without allocating (uses bincode::serialized_size)
    let size_mb = bincode::serialized_size(&new_index)
        .map(|size| size as f64 / 1_048_576.0)
        .unwrap_or(0.0);

    // Drop build result and reload from disk for compact memory layout
    // (same pattern as serve.rs startup — eliminates allocator fragmentation)
    drop(new_index);
    crate::index::force_mimalloc_collect();
    crate::index::log_memory("reindex: after drop+mi_collect (def)");

    let new_index = match crate::definitions::load_definition_index(&dir, &ext, &ctx.index_base) {
        Ok(idx) => idx,
        Err(e) => {
            warn!(error = %e, "Failed to reload def index from disk after reindex, rebuilding");
            crate::definitions::build_definition_index(&crate::definitions::DefIndexArgs {
                dir: dir.to_string(), ext: ext.clone(), threads: 0,
                respect_git_exclude: ctx.respect_git_exclude,
            })
        }
    };

    // Update in-memory cache
    match def_index_arc.write() {
        Ok(mut idx) => {
            *idx = new_index;
        }
        Err(e) => {
            // Rollback workspace state to avoid getting stuck in Reindexing
            if workspace_changed {
                rollback_workspace_state(ctx, &previous_dir, old_mode, old_generation);
            } else {
                let mut ws = ctx.workspace.write().unwrap_or_else(|e| e.into_inner());
                ws.status = WorkspaceStatus::Resolved;
            }
            return ToolCallResult::error(format!("Failed to update in-memory definition index: {}", e));
        }
    }

    // Mark workspace as resolved and def index as ready.
    // CRITICAL: def_ready must be set to true here because Fix B
    // (handle_pending_response) may have reset it to false during workspace switch.
    {
        let mut ws = ctx.workspace.write().unwrap_or_else(|e| e.into_inner());
        ws.status = WorkspaceStatus::Resolved;
    }
    ctx.def_ready.store(true, Ordering::Release);

    // Force mimalloc to return freed pages (old index) to OS
    crate::index::force_mimalloc_collect();
    crate::index::log_memory("reindex: after replace+mi_collect (def)");

    // ─── Cross-load content index on workspace switch ───
    let content_index_action = if workspace_changed {
        cross_load_content_index(ctx, &dir)
    } else {
        None
    };

    let elapsed = start.elapsed();

    let mut output = json!({
        "status": "ok",
        "files": file_count,
        "definitions": def_count,
        "callSites": call_site_count,
        "codeStatsEntries": code_stats_count,
        "sizeMb": (size_mb * 10.0).round() / 10.0,
        "rebuildTimeMs": elapsed.as_secs_f64() * 1000.0,
    });
    if workspace_changed {
        output["workspaceChanged"] = json!(true);
        output["previousServerDir"] = json!(previous_dir);
    }
    if let Some(action) = content_index_action {
        output["contentIndexAction"] = json!(action);
    }
    output["serverDir"] = json!(dir);

    ToolCallResult::success(utils::json_to_string(&output))
}

// ─── Tests ──────────────────────────────────────────────────────────
// Tests remain in the original handlers_tests.rs file to avoid
// duplicating ~3000 lines. They use `use super::*` to access
// all re-exported symbols.

#[cfg(test)]
mod handlers_test_utils;

#[cfg(test)]
#[path = "handlers_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "handlers_tests_grep.rs"]
mod tests_grep;

#[cfg(test)]
#[path = "handlers_tests_line_regex.rs"]
mod tests_line_regex;

#[cfg(test)]
#[path = "handlers_tests_fast.rs"]
mod tests_fast;


#[cfg(test)]
#[path = "handlers_tests_git.rs"]
mod tests_git;

#[cfg(test)]
#[path = "handlers_tests_misc.rs"]
mod tests_misc;

#[cfg(test)]
#[cfg(feature = "lang-csharp")]
#[path = "handlers_tests_csharp.rs"]
mod tests_csharp;

#[cfg(test)]
#[cfg(feature = "lang-csharp")]
#[path = "handlers_tests_csharp_callers.rs"]
mod tests_csharp_callers;

#[cfg(test)]
#[cfg(all(feature = "lang-csharp", feature = "lang-typescript"))]
#[path = "handlers_tests_typescript.rs"]
mod tests_typescript;

#[cfg(test)]
#[cfg(feature = "lang-rust")]
#[path = "handlers_tests_rust.rs"]
mod tests_rust;

#[cfg(test)]
#[path = "handlers_tests_workspace.rs"]
mod tests_workspace;