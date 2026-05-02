//! MCP tool handlers — dispatches tool calls to specialized handler modules.

mod advisory_hints;
mod callers;
mod arg_validation;
mod definitions;
mod edit;
mod fast;
mod git;
mod grep;
pub(crate) use grep::{start_warm_trigram_index, warm_trigram_index};
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

#[cfg(test)]
pub(crate) static PROCESS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Number of tools advertised by `tools/list`.
/// Keep this in sync with `tool_definitions_with_runtime`; server protocol trace
/// uses it to avoid rebuilding full tool descriptions during initialize logging.
pub const TOOL_DEFINITION_COUNT: usize = 15;

/// Return all tool definitions for tools/list.
/// `def_extensions` — file extensions with definition parser support (e.g., ["cs", "rs"]).
/// Used to dynamically generate language lists in xray_definitions and xray_callers descriptions.
pub fn tool_definitions(def_extensions: &[String]) -> Vec<ToolDefinition> {
    tool_definitions_with_runtime(def_extensions, false)
}

pub fn tool_definitions_with_runtime(def_extensions: &[String], xml_on_demand_available: bool) -> Vec<ToolDefinition> {
    let lang_list = crate::tips::format_supported_languages(def_extensions);
    let xml_definition_note = if xml_on_demand_available {
        " XML on-demand: for .xml, .config, .csproj, .vbproj, .fsproj, .vcxproj, .nuspec, .vsixmanifest, .manifestxml, .appxmanifest, .props, .targets, .resx files, use file='<path>' with containsLine=<N> or name='<element>' to parse XML on-the-fly and get structural context with parent promotion (leaf elements are auto-promoted to parent block). The name filter searches both element names AND text content of leaf elements (e.g., name='PremiumStorage' finds <ServiceType>PremiumStorage</ServiceType> and returns the parent block with matchedBy='textContent', matchedChild='ServiceType'). Text content search requires term >= 3 chars. Multiple leaf matches in same parent are de-duplicated into one result with matchedChildren array. Name matches take priority over textContent matches. Passing a directory path returns a clear error with guidance to use xray_fast."
    } else {
        ""
    };
    // Single source of truth for the closed `kind` enum exposed via
    // `xray_definitions.kind`. Derived from `DefinitionKind::ALL_KINDS` so the
    // schema and the runtime validator (`utils::read_kind_array`) cannot drift.
    let kind_enum: Vec<&'static str> = crate::definitions::DefinitionKind::ALL_KINDS
        .iter()
        .map(|k| k.as_str())
        .collect();
    let mut tools = vec![
        ToolDefinition {
            name: "xray_grep".to_string(),
            description: "Preferred for content/pattern search across indexed files. Use before built-in text/regex search for indexed file types. Search file contents using an inverted index with TF-IDF ranking. LANGUAGE-AGNOSTIC: works with any text file (C#, Rust, Python, JS/TS, XML, JSON, config, etc.). Supports exact tokens, multi-term OR/AND, regex, phrase search, substring search, and exclusion filters. Results ranked by relevance. Index stays in memory for instant subsequent queries (~0.001s). Substring search is ON by default. Large results are auto-truncated to ~16KB (~4K tokens). Use countOnly=true or narrow with dir/ext/excludeDir for focused results. Multi-term OR/AND via array `terms` — each entry is one term; literal commas inside an entry are preserved.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "terms": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Search terms. Each array entry is one term. Multi-term OR/AND via `mode`. Literal commas inside an entry are preserved (e.g. regex `^[^,]+,[^,]+$`)."
                    },
                    "dir": {
                        "type": "string",
                        "description": "Directory to search (default: server's --dir). Supports both absolute and relative paths — relative paths are resolved against server_dir. If a FILE path is passed, it is auto-converted to its parent directory + file= basename filter; the response includes a `dirAutoConverted` note explaining the conversion. To narrow to a specific file, prefer `file='<name>'` directly."
                    },
                    "file": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Restrict results to files whose path or basename contains any of these substrings (case-insensitive, OR semantics — e.g., [\"Service\",\"Client\"]). Combines with `dir`/`ext`/`excludeDir` via AND."
                    },
                    "ext": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "File extension filter. Each array entry is one extension (without dot); multiple entries = OR — e.g., [\"rs\",\"toml\"]. Default: all indexed."
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
                        "description": "Exact phrase match on raw file content (default: false). Matches literal strings including XML tags, angle brackets, slashes — no escaping needed. Example: terms=['<MaxRetries>3</MaxRetries>'], phrase=true finds the exact XML tag. Multi-phrase OR/AND via the `terms` array."
                    },
                    "lineRegex": {
                        "type": "boolean",
                        "description": "Line-anchored regex search (default: false). Auto-enables regex=true and disables substring. Unlike default regex (which matches against tokenized index entries — alphanumeric+underscore only), lineRegex applies the pattern to each line of file content with `multi_line=true` and `crlf=true`, so `^` and `$` anchor to logical line boundaries on BOTH `\\n` (Unix) and `\\r\\n` (Windows/CRLF) input — `terms=[\"\\}$\"]` matches lines ending in `}` even on Windows-edited files. Patterns may contain spaces, punctuation, brackets, etc. Required for: markdown headings (`^## `), C# attributes (`^\\s*\\[Test\\]`), function signatures (`^pub fn`), end-of-line braces (`\\}$`). Each `terms` array entry is one regex pattern, taken verbatim — literal `,` inside a pattern is preserved (CSV-shape, log prefixes). Whitespace inside patterns is significant — patterns are NOT trimmed. Case-SENSITIVE by default; use inline flag `(?i)` for case-insensitive (e.g. `terms=[\"(?i)^todo:\"]`). File scope MUST be narrowed via ext/dir/file filters; otherwise every indexed file is read from disk (slower than token regex). Mutually exclusive with phrase=true."
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
            description: "PREFERRED file lookup tool — searches pre-built file name index. Instant results (~35ms for 100K files). Auto-builds index if not present. Pass multiple patterns as an array (`pattern: [\"UserService\",\"OrderProcessor\"]`) for multi-file lookup with OR semantics. Supports `pattern=['*']` or an empty array with `dir` to list ALL files/directories (wildcard listing). Use with dirsOnly=true to list subdirectories. ALWAYS use this instead of built-in list_files or list_directory. When dirsOnly=true with wildcard, returns directories sorted by fileCount (largest modules first) and includes fileCount field.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "array", "items": { "type": "string" }, "description": "File name substring or glob pattern(s). Each array entry is one pattern; multiple entries = OR. Glob characters (* and ?) are auto-detected and converted to regex (e.g., 'Order*' finds files starting with Order, 'Use?Service' matches single char). Without glob chars, uses substring matching. Use ['*'] to list all entries. Empty array with `dir` also lists all." },
                    "dir": { "type": "string", "description": "Directory to search. Supports both absolute and relative paths. Relative paths are resolved against server_dir (e.g., 'src/services' resolves to server_dir/src/services)." },
                    "ext": { "type": "array", "items": { "type": "string" }, "description": "File extension filter. Each array entry is one extension (without dot); multiple entries = OR — e.g., [\"rs\",\"toml\"]." },
                    "regex": { "type": "boolean", "description": "Treat as regex" },
                    "ignoreCase": { "type": "boolean", "description": "Case-insensitive" },
                    "dirsOnly": { "type": "boolean", "description": "Show only directories. Returns directories sorted by fileCount descending (largest first). Useful for identifying important modules. Works with both wildcard (pattern=['*']) and filtered patterns (e.g., ['Storage','Redis']). ext filter is ignored (directories have no extension)" },
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
            description: "Show all existing indexes with their status, sizes, and age. With `file=[\"path\",...]`: returns per-file metadata (lineCount, byteSize, extension, indexed, lineEnding, definitionParserActive, xmlOnDemandActive, symbolReadableViaDefinitions, optional hint) WITHOUT loading file content into the response. Use this to discover the line count of a file before composing an `xray_edit` call (e.g. for the append-EOF idiom `startLine: lineCount+1, endLine: lineCount`) instead of falling back to `Get-Content | Measure-Object` or `wc -l`. lineCount uses the same semantics as `xray_edit`'s `newLineCount` / `originalLineCount` (trailing newline is a terminator, not a line) so the value can be fed directly into edit ranges.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of files to inspect. Each entry is one path (absolute, or workspace-relative). Returns lineCount/byteSize/extension/indexed/lineEnding plus parser/on-demand readability metadata per file. Without this argument, the existing index-level summary is returned."
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "xray_reindex".to_string(),
            description: "Force rebuild the content index and reload it into the server's in-memory cache. Also rebinds workspace when dir parameter switches to a different directory, unless the server was started with a pinned --dir. If the server is pinned and you want to rebuild that same workspace, omit dir; pass dir only when intentionally rebinding an unpinned/unresolved workspace. Response includes workspaceChanged, previousServerDir, indexAction fields. When workspace is UNRESOLVED (wrong CWD), call this with dir=<project_path> to fix it.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "dir": { "type": "string", "description": "Directory to reindex. Omit this when rebuilding the current pinned --dir workspace." },
                    "ext": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "File extensions, e.g., [\"rs\",\"toml\"]"
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
                     Supports {}. For servers started with a pinned --dir, omit dir when rebuilding the current workspace; \
                     pass dir only when intentionally rebinding an unpinned/unresolved workspace. \
                     Returns build metrics: files parsed, definitions extracted, call sites, codeStatsEntries \
                     (methods with complexity metrics), parse errors, build time, and index size. \
                     After rebuild, code stats are available for includeCodeStats/sortBy/min* queries. \
                     Requires server started with --definitions flag.",
                    lang_list
                )
            },
            input_schema: json!({
                "type": "object",
                "properties": {
                    "dir": { "type": "string", "description": "Directory to reindex. Omit this when rebuilding the current pinned --dir workspace." },
                    "ext": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "File extensions to parse, e.g., [\"rs\",\"toml\"] (default: server's --ext)"
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "xray_definitions".to_string(),
            description: if def_extensions.is_empty() {
                if xml_on_demand_available {
                    format!(
                        "Definition parsers are not active for ordinary source files, but XML on-demand parsing is available.{} Requires server started with --definitions flag. Use xray_grep for content search over indexed extensions.",
                        xml_definition_note
                    )
                } else {
                    "Definition index not available for current file extensions. Use xray_grep for content search.".to_string()
                }
            } else {
                format!(
                    "PREFERRED for code exploration AND module structure discovery. \
                     REPLACES directory listing for understanding code — use file='<dirname>' to get ALL classes, methods, \
                     interfaces in ONE call (more informative than directory tree which only shows file names). \
                     REPLACES read_file for indexed source files — use includeBody=true maxBodyLines=0 to get full file content. \
                     Search code definitions — classes, interfaces, methods, properties, enums. \
                     Uses pre-built AST index for instant results (~0.001s). \
                     LANGUAGE-SPECIFIC: Supports {}. Only these extensions are indexed — for other file types (JSON, config, MD) use xray_grep.{} \
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
                    lang_list,
                    xml_definition_note,
                )
            },
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Name(s) to search (substring). Each array entry is one term; multiple entries = OR."
                    },
                    "kind": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": kind_enum
                        },
                        "description": "Filter by definition kind(s). Each array entry is one kind. Empty/omitted = all kinds."
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
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Filter by file path substring(s). Each array entry is one term; multiple entries = OR. Use file=['<dirname>'] to explore an entire module — returns all definitions in files matching this directory path."
                    },
                    "parent": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Filter by parent/containing class name(s). Each array entry is one term; multiple entries = OR."
                    },
                    "containsLine": {
                        "type": "integer",
                        "description": "Find definition(s) containing this line number. Returns innermost method + parent class. Requires 'file' parameter. With includeBody=true, body is emitted ONLY for the innermost (most specific) definition; parent definitions get 'bodyOmitted' hint instead — this maximizes body budget for the target method."
                    },
                    "regex": {
                        "type": "boolean",
                        "description": "Treat name as regex pattern (default: false)."
                    },
                    "exactNameOnly": {
                        "type": "boolean",
                        "description": "When true with name, match definition names exactly instead of substring matching. Also disables name auto-correction for this request. (default: false)"
                    },
                    "autoCorrect": {
                        "type": "boolean",
                        "description": "Allow best-effort kind/name correction when the initial search returns no definitions. Set false to keep not_found exact. Ignored when exactNameOnly=true. (default: true)"
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
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Method name(s) to find callers/callees for. Each array entry is one method; multiple entries = batch (e.g., [\"Foo\",\"Bar\",\"Baz\"]). Each method gets an independent call tree. Single-element array returns {callTree: [...]}, multi-element returns {results: [{method, callTree}, ...]}."
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
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "File extension filter. Each array entry is one extension (without dot); multiple entries = OR — e.g., [\"cs\",\"sql\"]. Default: server's --ext."
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
                    "productionOnly": {
                        "type": "boolean",
                        "description": "When true, excludes test files and test methods from caller/callee trees and marks resultStatus.scope.productionOnly=true. (default: false)"
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
            description: "ALWAYS USE THIS instead of apply_diff, search_and_replace, or insert_content for ANY file edit. Edit a file by line-range operations or text-match replacements. Auto-creates new files when they don't exist (treats as empty — use Mode A: operations [{startLine:1, endLine:0, content:'...'}] for new file content). Mode A (operations): Replace/insert/delete lines by line number. Applied bottom-up to avoid offset cascade. Mode B (edits): Find and replace text or regex patterns, or insert content after/before anchor text. Applied sequentially. Returns unified diff. Use dryRun=true to preview without writing — pure preview, no parent directories or temp files are created on disk. Works on any text file (not limited to --dir). Accepts absolute or relative paths. UTF-8 only: files with invalid UTF-8 sequences (Windows-1251, Shift-JIS, GB2312, Latin-1) are rejected with an error rather than silently corrupted via lossy decode. Supports multi-file editing via 'paths' parameter (best-effort transactional: Phase 3a stages all temps + verifies I/O before touching originals; Phase 3b commits renames sequentially — a mid-batch rename failure cannot be rolled back and is surfaced as an error with `committed` / `pending` counts). PREFERRED over apply_diff for all file edits — atomic write per file (temp + fsync + rename), per-call unique temp names so concurrent edits do not collide, no whitespace issues, minimal token cost. FLEX-WHITESPACE FALLBACK: Mode B search/replace and insertAfter/insertBefore try exact match first, then strip-trailing-WS, then trim-blank-lines; the 4th step (regex-based whitespace-collapsing match) is opt-in and only runs when the edit carries an `expectedContext` — without `expectedContext` a failed match returns `Text not found` with a hint (no silent cross-block misfires). FIELD-NAME ALIASES: edits[] items accept common cross-tool aliases (oldText/newText, oldString/newString, old_str/new_str, find/with, pattern/replacement, after/before/text → search/replace/insertAfter/insertBefore/content) and silently rewrite them to canonical names before validation; a conflict (both alias AND canonical present in the same item) is still rejected. IDEMPOTENCY: insertAfter/insertBefore detect if the would-be-inserted content already exists adjacent to the anchor and skip the edit (response: skippedDetails[].reason = \"alreadyApplied: ...\"). Safe to retry after partial/unknown success. APPLIED semantics: the `applied` field excludes edits that were skipped via skipIfNotFound or idempotency — it reports only edits that actually mutated the file. EDIT RESULTS: every successful response carries `editResults: [{idx, matchCount}]` — one entry per input edit in input order. matchCount reports actual replacements (Mode B) or 1-per-applied-op / 0-per-skipped-op (Mode A), letting callers detect over-matches without re-running the batch. LINE ENDINGS: response includes lineEnding (\"LF\" | \"CRLF\"). The returned diff is always LF-based; on CRLF files the on-disk bytes are CRLF, so `git diff` of a CRLF file will not match tool diff character-for-character — use lineEnding to reconcile. POST-WRITE VERIFICATION: after every real write (not dryRun) the file is re-read and compared byte-for-byte to the computed post-state; any mismatch returns an error instead of a misleading success. SYNC REINDEX: after a successful real write (NOT dryRun), the inverted-index and definition-index are refreshed in-process before the response returns — a follow-up xray_grep / xray_definitions / xray_callers / xray_fast call sees the new content with zero latency (no 500ms FS-watcher debounce wait). Response includes new fields (real writes only): contentIndexUpdated (bool), defIndexUpdated (bool — true only when server has --definitions and parse succeeded), fileListInvalidated (bool — true when a new file is created → xray_fast cache will rebuild on next call), reindexElapsedMs (string, e.g. \"0.42\"), skippedReason (string — set to \"outsideServerDir\" / \"extensionNotIndexed\" / \"insideGitDir\" when the file is written but reindex is skipped because the file is out of the server's indexing scope; the file is still committed to disk). dryRun: true OMITS all reindex fields. Multi-file responses report per-file reindex outcome and a single summary.reindexElapsedMs. If an index lock is poisoned during reindex, the response includes reindexWarning explaining that the FS watcher will reconcile within 500ms — the write itself always succeeds.".to_string(),
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
                                    "description": "1-based start line (inclusive). To APPEND after EOF: startLine = lineCount+1, endLine = lineCount (use xray_info file=[X] to fetch lineCount)."
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
                "properties": {
                    "tool": {
                        "type": "string",
                        "description": "Optional: return help for a single tool only (e.g. 'xray_edit', 'xray_grep'). Omit to get the full reference catalog."
                    }
                },
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
    /// PERF-02: per-repo cache of `detect_main_branch_name` result.
    ///
    /// Lookup key is the raw repo string the caller passed (no
    /// canonicalisation — canonicalisation would itself spawn a syscall on
    /// Windows). Value is `Some("main"|"master")` if the probe found one.
    ///
    /// **Negative results are NOT cached** (PERF-02 follow-up). Caching
    /// `Some(None)` permanently poisoned paths probed before their repo
    /// existed (e.g. probe runs against an empty workspace, then user runs
    /// `git init` + creates `main`) — they would forever return None until
    /// server restart. The map's `Option<String>` value type is kept for
    /// internal `clone()` ergonomics on cache hits; `None` simply never
    /// appears as a stored value.
    ///
    /// **Invalidation:** keyed by repo path, so workspace switches
    /// naturally miss-then-repopulate (the new repo is a different key).
    /// Known limitation: if a long-running session does
    /// `git branch -d main` after the cache populated, the next
    /// `xray_branch_status` will still report `mainBranch="main"` until
    /// the server restarts. Acceptable trade-off because the destructive
    /// rename is a manual one-shot action; the previous behaviour spawned
    /// up to 4 sequential `git rev-parse` per request indefinitely.
    pub branch_name_cache: Arc<RwLock<std::collections::HashMap<String, Option<String>>>>,
    /// PERF-08: single-flight gate for `xray_fast`'s lazy file-index rebuild.
    ///
    /// **Problem fixed:** the previous code did
    /// `if needs_rebuild { build_index(...); store(...); }` with no
    /// mutual exclusion. Under N concurrent `xray_fast` calls on a cold
    /// or dirty context, every thread saw `needs_rebuild=true` (because
    /// the others hadn't finished writing yet) and **each one ran a
    /// full filesystem walk + 8 MB allocation + on-disk save in
    /// parallel**. On a 100k-file repo with 4-8 concurrent LLM tool
    /// calls this multiplied cold-start cost by ~N.
    ///
    /// **Fix:** explicit single-flight via `Mutex<bool> + Condvar`.
    /// Exactly one thread runs `build_index`; the rest sleep on the
    /// condvar and wake up to read the freshly-built index. The mutex
    /// is held only across cheap state inspection — the actual build
    /// runs lock-free, so other unrelated `xray_fast` requests against
    /// an already-built index never touch this gate.
    ///
    /// **Reset semantics:** the gate is a transient guard — it does
    /// not own the index. After build completion the gate returns to
    /// `building=false` and the next dirty signal from the watcher
    /// (which sets `file_index_dirty=true`) re-triggers exactly one
    /// rebuild, giving the same single-flight guarantee on every
    /// invalidation cycle. (`tokio::sync::OnceCell` was rejected
    /// precisely because it lacks reset semantics.)
    ///
    /// **Panic safety:** the builder thread holds an RAII guard that
    /// resets `building=false` and `notify_all`s on drop, so a panic
    /// inside `build_index` cannot strand waiters forever — they wake,
    /// see `file_index` still `None`, and one of them retakes the
    /// build slot.
    pub file_index_build_gate: Arc<utils::FileIndexBuildGate>,
    /// Single-flight gate for trigram rebuilds after content-index mutations.
    pub trigram_build_gate: Arc<utils::TrigramRebuildGate>,
    /// Cross-thread dirty flag: set by `xray_edit` (handler thread) after
    /// `reindex_paths_sync` mutates the in-memory indexes. Read by the
    /// watcher thread to prevent clearing `have_unsaved` when the snapshot
    /// that was just saved is already stale. Without this, a force-kill
    /// after edit-then-autosave loses the edit's index mutations.
    ///
    /// **Ordering:** `Relaxed` — same rationale as `file_index_dirty`.
    /// Pure signal, not a happens-before edge for data.
    pub autosave_dirty: Arc<AtomicBool>,
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
            branch_name_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
            file_index_build_gate: Arc::new(utils::FileIndexBuildGate::new()),
            trigram_build_gate: Arc::new(utils::TrigramRebuildGate::new()),
            autosave_dirty: Arc::new(AtomicBool::new(false)),
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
/// Public entry point for tool dispatch. Delegates to [`dispatch_inner`] for
/// gate checks + handler invocation, then **always** runs the response pipeline
/// ([`finalize_response`]) — even when a gate rejects the request early.
///
/// Phase 2 fix: before this refactor, 6 dispatch-level early-return sites
/// (workspace gate, index gate, strict-args, unknown tool) bypassed
/// `inject_response_guidance` / `inject_metrics`, so their error responses
/// carried no `totalTimeMs`, `policyReminder`, or workspace metadata.
pub fn dispatch_tool(
    ctx: &HandlerContext,
    tool_name: &str,
    arguments: &Value,
) -> ToolCallResult {
    let dispatch_start = Instant::now();
    tracing::info!(tool = tool_name, "[dispatch-trace] dispatch entered");
    let (result, unknown_args_report) = dispatch_inner(ctx, tool_name, arguments);
    finalize_response(result, ctx, tool_name, arguments, dispatch_start, unknown_args_report)
}

/// All gate checks + handler dispatch. May `return` early from any gate —
/// the caller ([`dispatch_tool`]) guarantees the pipeline always runs.
fn dispatch_inner(
    ctx: &HandlerContext,
    tool_name: &str,
    arguments: &Value,
) -> (ToolCallResult, Option<arg_validation::UnknownArgsReport>) {
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
                return (ToolCallResult::error(error_msg.to_string()), None);
            }
            WorkspaceStatus::Reindexing => {
                let error_msg = serde_json::json!({
                    "error": "WORKSPACE_REINDEXING",
                    "message": format!("Workspace is switching to '{}'. Indexes are not ready yet.", ws.dir),
                    "hint": "Call xray_reindex to complete the workspace switch and build indexes.",
                    "serverDir": ws.dir,
                    "workspaceStatus": "reindexing"
                });
                return (ToolCallResult::error(error_msg.to_string()), None);
            }
            WorkspaceStatus::Resolved => { /* proceed */ }
        }
    }

    // Check readiness: if the required index is still building, return early.
    // Note: xray_reindex and xray_reindex_definitions are NOT in these guards —
    // they need to run even when ready=false (e.g., after workspace switch).
    // Their concurrent build protection uses content_building/def_building flags.
    if requires_content_index(tool_name) && !ctx.content_ready.load(Ordering::Acquire) {
        return (ToolCallResult::error(INDEX_BUILDING_MSG.to_string()), None);
    }
    if requires_def_index(tool_name) && !ctx.def_ready.load(Ordering::Acquire) {
        return (ToolCallResult::error(DEF_INDEX_BUILDING_MSG.to_string()), None);
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
        // Return None for unknown_args: strict_error_response already embeds
        // the full warning — inject_warning would duplicate it.
        return (arg_validation::strict_error_response(tool_name, rep), None);
    }

    let result = match tool_name {
        "xray_grep" => grep::handle_xray_grep(ctx, arguments),
        "xray_fast" => fast::handle_xray_fast(ctx, arguments),
        "xray_info" => handle_xray_info(ctx, arguments),
        "xray_reindex" => handle_xray_reindex(ctx, arguments),
        "xray_reindex_definitions" => handle_xray_reindex_definitions(ctx, arguments),
        "xray_definitions" => definitions::handle_xray_definitions(ctx, arguments),
        "xray_callers" => callers::handle_xray_callers(ctx, arguments),
        "xray_edit" => edit::handle_xray_edit(ctx, arguments),
        "xray_help" => handle_xray_help(ctx, arguments),
        // Git history tools
        "xray_git_history" | "xray_git_diff" | "xray_git_authors" | "xray_git_activity" | "xray_git_blame" | "xray_branch_status" => {
            git::dispatch_git_tool(ctx, tool_name, arguments)
        }
        _ => return (ToolCallResult::error(format!("Unknown tool: {}", tool_name)), None),
    };

    (result, unknown_args_report)
}

/// Response pipeline: guidance → unknown-args warning → metrics/truncation → optional guidance prefix.
/// Runs on **every** response (success, handler error, AND gate error).
fn finalize_response(
    result: ToolCallResult,
    ctx: &HandlerContext,
    tool_name: &str,
    arguments: &Value,
    dispatch_start: Instant,
    unknown_args_report: Option<arg_validation::UnknownArgsReport>,
) -> ToolCallResult {
    // Guidance injection: policyReminder, nextStepHint, workspace metadata.
    let was_error = result.is_error;
    let result = utils::inject_response_guidance_with_args(result, tool_name, &ctx.server_ext, ctx, Some(arguments));
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

    // Count methods for multi-method budget scaling.
    let method_count = if tool_name == "xray_callers" {
        arguments.get("method")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter()
                .filter_map(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .count())
            .filter(|n| *n > 0)
            .unwrap_or(1)
    } else {
        1
    };

    let effective_max = if tool_name == "xray_help" {
        ctx.max_response_bytes.max(XRAY_HELP_MIN_RESPONSE_BYTES)
    } else if tool_name == "xray_callers" && method_count > 1 {
        let scaled = MULTI_METHOD_RESPONSE_BYTES_PER * method_count;
        ctx.max_response_bytes.max(scaled.min(MULTI_METHOD_RESPONSE_MAX))
    } else if has_include_body {
        ctx.max_response_bytes.max(INCLUDE_BODY_MIN_RESPONSE_BYTES)
    } else {
        ctx.max_response_bytes
    };

    let result = if ctx.metrics {
        if tool_name == "xray_help" {
            utils::truncate_response_if_needed(result, effective_max)
        } else {
            utils::inject_metrics(result, ctx, dispatch_start, effective_max)
        }
    } else {
        utils::truncate_response_if_needed(result, effective_max)
    };

    utils::render_guidance_prefix_if_enabled(result, tool_name)
}

// ─── Small inline handlers ──────────────────────────────────────────

fn handle_xray_help(ctx: &HandlerContext, arguments: &Value) -> ToolCallResult {
    match arguments.get("tool") {
        Some(Value::String(tool_name)) => {
            match crate::tips::tool_help(tool_name, &ctx.def_extensions) {
                Ok(help) => ToolCallResult::success(utils::json_to_string(&help)),
                Err(msg) => ToolCallResult::error(msg),
            }
        }
        Some(_) => ToolCallResult::error(
            "Parameter 'tool' must be a string (e.g. 'xray_edit'). Omit it to get the full reference catalog.".to_string()
        ),
        None => {
            let help = crate::tips::render_json(&ctx.def_extensions);
            ToolCallResult::success(utils::json_to_string(&help))
        }
    }
}

/// Build xray_info response from in-memory indexes only.
/// Previous implementation called `cmd_info_json()` which deserialized ALL index
/// files from disk (~1.8 GB for multi-repo setups), causing a massive memory spike.
/// This version reads stats directly from the already-loaded in-memory structures
/// via read locks — zero additional allocations.
fn handle_xray_info(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    // Per-file metadata mode: when caller passes `file=["path", ...]` we skip
    // the index-summary aggregation and return cheap-to-compute metadata for
    // each requested path (lineCount/byteSize/extension/indexed/lineEnding).
    // Closes docs/user-stories/todo_2026-04-25_xray-edit-append-and-line-staleness.md §2.3
    // — without this, agents fall back to `Get-Content | Measure-Object` to
    // discover line count before composing an `xray_edit` call.
    let files: Vec<String> = match utils::read_string_array(args, "file") {
        Ok(v) => v,
        Err(e) => return ToolCallResult::error(e),
    };
    if !files.is_empty() {
        return handle_xray_info_files(ctx, &files);
    }
    let mut indexes = Vec::new();
    let mut memory_estimate = json!({});

    // ── Content index (in-memory) ──
    if ctx.content_ready.load(Ordering::Acquire) {
        if let Ok(idx) = ctx.index.read() {
            let live_files = idx.live_file_count();
            if live_files > 0 {
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
                    "files": live_files,
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
                let live_files = idx.live_file_count();
                if live_files > 0 {
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
                        "files": live_files,
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
// ─── Per-file metadata mode for `xray_info` ──────────────────────────
//
// Returns cheap-to-compute file metadata (lineCount/byteSize/extension/indexed/
// lineEnding) WITHOUT putting file content into the response. The intended
// caller is an LLM agent that needs to know the line count of a file before
// composing an `xray_edit` call (notably for the append-EOF idiom
// `startLine: lineCount+1, endLine: lineCount`).
//
// `lineCount` semantics MUST match `xray_edit`'s `count_lines` (see
// edit.rs::count_lines): split on '\n' and subtract 1 if the file ends with
// '\n'. This way `xray_info` -> `xray_edit` round-trips without off-by-one.
//
// Security: paths must resolve INSIDE the workspace root; out-of-workspace
// paths return a per-file error instead of metadata. Oversized files
// (>MAX_INDEX_FILE_BYTES) also return an error, mirroring `read_file_lossy`.
fn handle_xray_info_files(ctx: &HandlerContext, files: &[String]) -> ToolCallResult {
    let server_dir = ctx.server_dir();
    let canonical_server_dir = ctx.canonical_server_dir();
    let server_exts: Vec<String> = ctx
        .server_ext
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let def_exts = ctx.def_extensions.clone();
    let xml_on_demand_available = cfg!(feature = "lang-xml") && ctx.def_index.is_some();

    let mut entries: Vec<Value> = Vec::with_capacity(files.len());
    for raw_path in files {
        entries.push(file_metadata_entry(
            raw_path,
            &server_dir,
            &canonical_server_dir,
            &server_exts,
            &def_exts,
            xml_on_demand_available,
        ));
    }

    let summary = json!({
        "requested": files.len(),
        "returned": entries.len(),
    });
    let output = json!({ "files": entries, "summary": summary });
    ToolCallResult::success(utils::json_to_string(&output))
}


/// Compute metadata for a single file. Returns a per-file object that always
/// contains `path` (the input string, echoed back for batch correlation) and
/// either the metadata fields or an `error` describing why metadata could not
/// be produced.
fn file_metadata_entry(
    raw_path: &str,
    server_dir: &str,
    canonical_server_dir: &str,
    server_exts: &[String],
    def_exts: &[String],
    xml_on_demand_available: bool,
) -> Value {
    use std::path::{Path, PathBuf};

    let p = Path::new(raw_path);
    let resolved: PathBuf = if p.is_absolute() {
        p.to_path_buf()
    } else {
        Path::new(server_dir).join(p)
    };
    let resolved_str = crate::clean_path(&resolved.to_string_lossy());

    // Workspace boundary check. Use the same logical-path comparison the edit
    // handler uses so symlinked subdirectories within the workspace are
    // accepted, but symlink targets outside are rejected.
    if !crate::is_path_within(&resolved_str, server_dir)
        && !crate::is_path_within(&resolved_str, canonical_server_dir)
    {
        return json!({
            "path": raw_path,
            "resolvedPath": resolved_str,
            "error": "path is outside the workspace root; pass an absolute path inside the workspace or a workspace-relative path",
        });
    }

    // Stat first to surface size / not-found errors before reading bytes.
    let meta = match std::fs::metadata(&resolved) {
        Ok(m) => m,
        Err(e) => {
            return json!({
                "path": raw_path,
                "resolvedPath": resolved_str,
                "error": format!("cannot stat file: {}", e),
            });
        }
    };
    if !meta.is_file() {
        return json!({
            "path": raw_path,
            "resolvedPath": resolved_str,
            "error": "path is not a regular file (use xray_fast dir=<path> for directory listings)",
        });
    }
    let byte_size = meta.len();
    if byte_size > crate::MAX_INDEX_FILE_BYTES {
        return json!({
            "path": raw_path,
            "resolvedPath": resolved_str,
            "byteSize": byte_size,
            "error": format!(
                "file is {} bytes, exceeds MAX_INDEX_FILE_BYTES ({} bytes); xray refuses to read it",
                byte_size,
                crate::MAX_INDEX_FILE_BYTES,
            ),
        });
    }

    // Extension + indexed flag are derived from the resolved path so callers
    // can ask about files outside the indexed extension set and still get a
    // stable answer (`indexed: false`).
    let extension = resolved
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let indexed = !extension.is_empty() && server_exts.iter().any(|e| e == &extension);
    let definition_parser_active = !extension.is_empty()
        && def_exts.iter().any(|e| e.eq_ignore_ascii_case(&extension));
    let xml_on_demand_active = xml_on_demand_available && xml_on_demand_active_for_extension(&extension);
    let symbol_readable_via_definitions = definition_parser_active || xml_on_demand_active;

    // Read content via `read_file_lossy` so BOM detection / UTF-16 decoding
    // matches what `xray_grep`/`xray_edit` see. lineEnding detection runs on
    // the raw bytes (before BOM stripping) so CRLF status reflects the file
    // on disk, not the decoded representation.
    let raw = match std::fs::read(&resolved) {
        Ok(b) => b,
        Err(e) => {
            return json!({
                "path": raw_path,
                "resolvedPath": resolved_str,
                "byteSize": byte_size,
                "extension": extension,
                "indexed": indexed,
                "definitionParserActive": definition_parser_active,
                "xmlOnDemandActive": xml_on_demand_active,
                "symbolReadableViaDefinitions": symbol_readable_via_definitions,
                "error": format!("cannot read file: {}", e),
            });
        }
    };
    let line_ending = detect_line_ending(&raw);

    let (content, was_lossy) = match crate::read_file_lossy(&resolved) {
        Ok(pair) => pair,
        Err(e) => {
            return json!({
                "path": raw_path,
                "resolvedPath": resolved_str,
                "byteSize": byte_size,
                "extension": extension,
                "indexed": indexed,
                "lineEnding": line_ending,
                "definitionParserActive": definition_parser_active,
                "xmlOnDemandActive": xml_on_demand_active,
                "symbolReadableViaDefinitions": symbol_readable_via_definitions,
                "error": format!("cannot decode file: {}", e),
            });
        }
    };
    let line_count = count_lines_for_info(&content);

    let mut entry = json!({
        "path": raw_path,
        "resolvedPath": resolved_str,
        "lineCount": line_count,
        "byteSize": byte_size,
        "extension": extension,
        "indexed": indexed,
        "definitionParserActive": definition_parser_active,
        "xmlOnDemandActive": xml_on_demand_active,
        "symbolReadableViaDefinitions": symbol_readable_via_definitions,
        "lineEnding": line_ending,
    });
    if was_lossy {
        entry["lossyUtf8"] = json!(true);
    }
    if definition_parser_active {
        entry["hint"] = json!(format!(
            "{} has an active definition parser. Prefer xray_definitions file=[\"{}\"] includeBody=true maxBodyLines=0 over read_file for symbol-level reads.",
            raw_path, raw_path
        ));
    } else if xml_on_demand_active {
        entry["hint"] = json!(format!(
            "{} is XML-on-demand parseable. Use xray_definitions file=[\"{}\"] containsLine=N or name=[\"ElementOrText\"] instead of raw read_file scans.",
            raw_path, raw_path
        ));
    }
    entry
}

fn xml_on_demand_active_for_extension(extension: &str) -> bool {
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

/// Mirror of `edit::count_lines` so `xray_info`'s `lineCount` matches
/// `xray_edit`'s `originalLineCount`/`newLineCount` exactly. Trailing newline
/// is treated as a terminator, not a line.
fn count_lines_for_info(s: &str) -> usize {
    if s.is_empty() {
        0
    } else {
        s.split('\n').count() - usize::from(s.ends_with('\n'))
    }
}

/// Inspect raw file bytes and report the dominant line-ending style.
/// Returns one of: `"NONE"` (no `\n` at all), `"LF"`, `"CRLF"`, or `"MIXED"`
/// when both LF and CRLF appear. `"MIXED"` is a useful diagnostic — agents can
/// avoid line-based edits on such files until normalised.
///
/// UTF-16 (LE/BE) files are recognised by their BOM and scanned at u16-code-unit
/// granularity so that on-disk CRLF (`00 0D 00 0A` BE / `0D 00 0A 00` LE) is
/// reported as `"CRLF"` instead of being misclassified as `"LF"` because the
/// 0x0A byte is not preceded by a 0x0D byte at byte granularity.
fn detect_line_ending(bytes: &[u8]) -> &'static str {
    // UTF-16 BOM detection — must precede the byte-oriented scan because
    // the byte preceding 0x0A in UTF-16 CRLF is the high half of a NUL
    // code unit, not 0x0D.
    if bytes.len() >= 2 {
        match (bytes[0], bytes[1]) {
            (0xFF, 0xFE) => return detect_line_ending_utf16(&bytes[2..], false),
            (0xFE, 0xFF) => return detect_line_ending_utf16(&bytes[2..], true),
            _ => {}
        }
    }
    let mut has_lf = false;
    let mut has_crlf = false;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            if i > 0 && bytes[i - 1] == b'\r' {
                has_crlf = true;
            } else {
                has_lf = true;
            }
        }
        i += 1;
    }
    match (has_lf, has_crlf) {
        (false, false) => "NONE",
        (true, false) => "LF",
        (false, true) => "CRLF",
        (true, true) => "MIXED",
    }
}

/// Scan UTF-16 code units (after the BOM has been stripped by the caller)
/// looking for `\n` (U+000A) optionally preceded by `\r` (U+000D).
/// `big_endian` selects the byte order for u16 reconstruction.
fn detect_line_ending_utf16(body: &[u8], big_endian: bool) -> &'static str {
    let mut has_lf = false;
    let mut has_crlf = false;
    let mut prev: Option<u16> = None;
    let mut i = 0;
    while i + 1 < body.len() {
        let cu = if big_endian {
            u16::from_be_bytes([body[i], body[i + 1]])
        } else {
            u16::from_le_bytes([body[i], body[i + 1]])
        };
        if cu == 0x000A {
            if prev == Some(0x000D) {
                has_crlf = true;
            } else {
                has_lf = true;
            }
        }
        prev = Some(cu);
        i += 2;
    }
    match (has_lf, has_crlf) {
        (false, false) => "NONE",
        (true, false) => "LF",
        (false, true) => "CRLF",
        (true, true) => "MIXED",
    }
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
/// Returns `Some("loaded_cache")`, `Some("background_build")`, or
/// `Some("build_in_progress")` describing the action taken.
fn cross_load_content_index(ctx: &HandlerContext, dir: &str) -> Option<&'static str> {
    let content_ext = ctx.server_ext.clone();
    let content_loaded = load_content_index(dir, &content_ext, &ctx.index_base).ok()
        .or_else(|| {
            let ext_vec: Vec<String> = content_ext.split(',').map(|s| s.to_string()).collect();
            find_content_index_for_dir(dir, &ctx.index_base, &ext_vec)
        });

    if let Some(idx) = content_loaded {
        let had_watch = ctx.index.read()
            .map(|i| i.file_tokens_authoritative)
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
        // Warm `file_tokens` in background so the first user edit avoids the
        // ~2.6 s rebuild under the watcher write lock. Lazy guard remains as
        // safety net. No-op when watch mode was not active.
        if had_watch {
            crate::mcp::watcher::schedule_rebuild_file_tokens(Arc::clone(&ctx.index));
        }
        // Invalidate file-list index
        if let Ok(mut fi) = ctx.file_index.write() { *fi = None; }
        ctx.file_index_dirty.store(true, Ordering::Relaxed);
        return Some("loaded_cache");
    }

    // No cache — start background build
    ctx.content_ready.store(false, Ordering::Release);
    let had_watch = ctx.index.read()
        .map(|i| i.file_tokens_authoritative)
        .unwrap_or(false);
    let (build_generation, build_workspace_dir) = {
        let ws = ctx.workspace.read().unwrap_or_else(|e| e.into_inner());
        (ws.generation, ws.dir.clone())
    };
    let bg_index = Arc::clone(&ctx.index);
    let bg_dir = dir.to_string();
    let bg_ext = content_ext;
    let bg_idx_base = ctx.index_base.clone();
    let bg_ready = Arc::clone(&ctx.content_ready);
    let bg_building = Arc::clone(&ctx.content_building);
    let bg_workspace = Arc::clone(&ctx.workspace);
    let bg_file_dirty = Arc::clone(&ctx.file_index_dirty);
    let bg_respect_git_exclude = ctx.respect_git_exclude;
    if ctx.content_building.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        info!(dir = %dir, "Content index background build already running (workspace switch)");
        return Some("build_in_progress");
    }
    std::thread::spawn(move || {
        let mut build_succeeded = false;
        info!(dir = %bg_dir, "Building content index in background (workspace switch)");
        match build_content_index(&ContentIndexArgs {
            dir: bg_dir.clone(), ext: bg_ext.clone(),
            max_age_hours: 24, hidden: false, no_ignore: false, respect_git_exclude: bg_respect_git_exclude,
            threads: 0, min_token_len: DEFAULT_MIN_TOKEN_LEN,
        }) {
            Ok(idx) => {
                let workspace_still_current = {
                    let ws = bg_workspace.read().unwrap_or_else(|e| e.into_inner());
                    ws.generation == build_generation
                        && code_xray::path_eq(&ws.dir, &build_workspace_dir)
                        && code_xray::path_eq(&ws.dir, &bg_dir)
                };
                if !workspace_still_current {
                    warn!(
                        dir = %bg_dir,
                        generation = build_generation,
                        "Discarding stale content index background build after workspace switch"
                    );
                } else {
                    if let Err(e) = save_content_index(&idx, &bg_idx_base) {
                        warn!(error = %e, "Failed to save content index");
                    }
                    let idx = if had_watch {
                        crate::mcp::watcher::build_watch_index_from(idx)
                    } else {
                        idx
                    };
                    *bg_index.write().unwrap_or_else(|e| e.into_inner()) = idx;
                    if had_watch {
                        crate::mcp::watcher::schedule_rebuild_file_tokens(Arc::clone(&bg_index));
                    }
                    bg_file_dirty.store(true, Ordering::Relaxed);
                    build_succeeded = true;
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to build content index");
            }
        }
        bg_building.store(false, Ordering::Release);
        if build_succeeded {
            bg_ready.store(true, Ordering::Release);
            crate::index::log_memory("reindex_def: content cross-build complete");
        }
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
    let def_ext_vec = ctx.def_index.as_ref()
        .and_then(|idx| idx.read().ok().map(|idx| idx.extensions.clone()))
        .unwrap_or_else(|| ctx.def_extensions.clone());
    if let Err(e) = crate::mcp::watcher::start_watcher(
        Arc::clone(&ctx.index),
        ctx.def_index.as_ref().map(Arc::clone),
        watch_dir,
        ext_vec,
        def_ext_vec.clone(),
        ctx.watch_debounce_ms,
        ctx.index_base.clone(),
        Arc::clone(&ctx.content_ready),
        Arc::clone(&ctx.def_ready),
        Arc::clone(&ctx.file_index_dirty),
        Arc::clone(&ctx.watcher_generation),
        new_gen,
        Arc::clone(&ctx.watcher_stats),
        ctx.respect_git_exclude,
        Arc::clone(&ctx.autosave_dirty),
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
            def_ext_vec,
            ctx.rescan_interval_sec,
            Arc::clone(&ctx.watcher_generation),
            new_gen,
            Arc::clone(&ctx.watcher_stats),
            ctx.respect_git_exclude,
            Arc::clone(&ctx.autosave_dirty),
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
    // 2026-04-25 list-params migration: `ext` is array<string>. Bridge into a
    // comma-joined String for downstream `build_or_load_content_index`.
    let ext = match utils::read_string_array(args, "ext") {
        Ok(v) if v.is_empty() => ctx.server_ext.clone(),
        Ok(v) => v.join(","),
        Err(e) => return ToolCallResult::error(e),
    };

    // Determine if workspace is changing
    let previous_dir = current_dir.clone();
    let workspace_changed = !code_xray::path_eq(&dir, &previous_dir);

    // Check if workspace switch is allowed (only blocked in PinnedCli mode)
    if workspace_changed {
        let ws = ctx.workspace.read().unwrap_or_else(|e| e.into_inner());
        if ws.mode == WorkspaceBindingMode::PinnedCli {
            return ToolCallResult::error(format!(
                "Server is pinned to --dir {} and cannot switch workspaces. \
                 To rebuild this pinned workspace, omit the `dir` argument; \
                 to index a different workspace, start another server instance or use CLI.",
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

    // Rebuild mutation lookups if watcher is active.
    // Without this, watcher becomes no-op after reindex (path_to_id = None).
    let had_watch = ctx.index.read()
        .map(|idx| idx.file_tokens_authoritative)
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

    // Warm `file_tokens` in background so the first user edit avoids the
    // ~2.6 s rebuild under the watcher write lock. Lazy guard remains as
    // safety net. No-op when watch mode was not active.
    if had_watch {
        crate::mcp::watcher::schedule_rebuild_file_tokens(Arc::clone(&ctx.index));
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
    // 2026-04-25 list-params migration: `ext` is array<string>. Bridge into a
    // comma-joined String for downstream `build_definition_index`.
    let ext = match utils::read_string_array(args, "ext") {
        Ok(v) if v.is_empty() => ctx.server_ext.clone(),
        Ok(v) => v.join(","),
        Err(e) => return ToolCallResult::error(e),
    };

    // Determine if workspace is changing
    let previous_dir = current_dir.clone();
    let workspace_changed = !code_xray::path_eq(&dir, &previous_dir);

    // Check if workspace switch is allowed (only blocked in PinnedCli mode)
    if workspace_changed {
        let ws = ctx.workspace.read().unwrap_or_else(|e| e.into_inner());
        if ws.mode == WorkspaceBindingMode::PinnedCli {
            return ToolCallResult::error(format!(
                "Server is pinned to --dir {} and cannot switch workspaces. \
                 To rebuild this pinned workspace, omit the `dir` argument; \
                 to index a different workspace, start another server instance or use CLI.",
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