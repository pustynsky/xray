//! MCP tool handlers — dispatches tool calls to specialized handler modules.

mod callers;
mod definitions;
mod edit;
mod fast;
mod git;
mod grep;
pub(crate) mod utils;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use serde_json::{json, Value};
use tracing::{info, warn};

use crate::mcp::protocol::{ToolCallResult, ToolDefinition};
use crate::{
    build_content_index, clean_path, load_content_index, find_content_index_for_dir,
    save_content_index, ContentIndex, ContentIndexArgs,
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
                        "description": "Directory to search (default: server's --dir). Accepts directories only — if you pass a file path, an error with a helpful hint is returned."
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
                        "description": "Exact phrase match (default: false). Comma-separated phrases are searched independently with OR/AND semantics."
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
                    "pattern": { "type": "string", "description": "File name pattern. Comma-separated for multi-term OR. Use '*' to list all entries. Empty string with dir also lists all." },
                    "dir": { "type": "string", "description": "Directory to search" },
                    "ext": { "type": "string", "description": "Filter by extension" },
                    "regex": { "type": "boolean", "description": "Treat as regex" },
                    "ignoreCase": { "type": "boolean", "description": "Case-insensitive" },
                    "dirsOnly": { "type": "boolean", "description": "Show only directories. When true with wildcard pattern, returns directories sorted by fileCount descending (largest first). Useful for identifying important modules. ext filter is ignored (directories have no extension)" },
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
                     LANGUAGE-SPECIFIC: Supports {}. Only these extensions are indexed — for other file types (XML, JSON, config, MD) use xray_grep. \
                     Requires server started with --definitions flag. \
                     Supports 'containsLine' to find which method/class contains a given line number. \
                     Supports 'includeBody' to return actual source code inline. \
                     When results exceed maxResults and no name filter is set, automatically returns a \
                     directory-grouped summary (autoSummary) with definition counts per subdirectory and \
                     top-3 largest classes/interfaces per group, instead of truncated entries. \
                     Add a name filter or narrow the file scope to get individual definitions.",
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
                     calls, and direct receiver calls are fully supported.",
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
            description: "ALWAYS USE THIS instead of apply_diff, search_and_replace, or insert_content for ANY file edit. Edit a file by line-range operations or text-match replacements. Auto-creates new files when they don't exist (treats as empty — use Mode A: operations [{startLine:1, endLine:0, content:'...'}] for new file content). Mode A (operations): Replace/insert/delete lines by line number. Applied bottom-up to avoid offset cascade. Mode B (edits): Find and replace text or regex patterns, or insert content after/before anchor text. Applied sequentially. Returns unified diff. Use dryRun=true to preview without writing. Works on any text file (not limited to --dir). Accepts absolute or relative paths. Supports multi-file editing via 'paths' parameter (transactional: all-or-nothing). PREFERRED over apply_diff for all file edits — atomic, no whitespace issues, minimal token cost.".to_string(),
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
                        "description": "Array of file paths for multi-file editing. Same edits/operations applied to ALL files. Transactional: if any file fails, none are written. Max 20 files. Mutually exclusive with 'path'."
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
                                    "description": "Anchor text — insert content on next line after this text. Mutually exclusive with search/replace and insertBefore."
                                },
                                "insertBefore": {
                                    "type": "string",
                                    "description": "Anchor text — insert content on line before this text. Mutually exclusive with search/replace and insertAfter."
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
                        "description": "Safety check: if file has different line count, abort. Prevents stale line numbers."
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
        Self {
            dir,
            mode: WorkspaceBindingMode::PinnedCli,
            status: WorkspaceStatus::Resolved,
            generation: 1,
        }
    }

    /// Create a new DotBootstrap workspace binding.
    pub fn dot_bootstrap(dir: String) -> Self {
        Self {
            dir,
            mode: WorkspaceBindingMode::DotBootstrap,
            status: WorkspaceStatus::Resolved,
            generation: 1,
        }
    }

    /// Create an unresolved workspace binding.
    pub fn unresolved(dir: String) -> Self {
        Self {
            dir,
            mode: WorkspaceBindingMode::Unresolved,
            status: WorkspaceStatus::Unresolved,
            generation: 0,
        }
    }
}

/// Context for tool handlers -- shared state

/// Quick check: does a directory contain any files with the given extensions?
/// Uses a shallow walk (max_depth levels) with early exit on first match.
/// Returns `true` if at least one matching file is found.
pub fn has_source_files(dir: &str, extensions: &[String], max_depth: usize) -> bool {
    use ignore::WalkBuilder;
    let walker = WalkBuilder::new(dir)
        .max_depth(Some(max_depth))
        .hidden(false)
        .build();
    for entry in walker {
        if let Ok(entry) = entry {
            if entry.file_type().is_some_and(|ft| ft.is_file()) {
                if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                    if extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Determine the initial WorkspaceBindingMode based on --dir argument.
/// - Explicit non-dot path → PinnedCli
/// - Dot path with source files → DotBootstrap  
/// - Dot path without source files → Unresolved
pub fn determine_initial_binding(dir: &str, extensions: &[String]) -> WorkspaceBinding {
    let is_dot = dir == "." || dir == "./" || dir == ".\\";
    if !is_dot {
        // Explicit path — PinnedCli, always resolved
        WorkspaceBinding::pinned(dir.to_string())
    } else {
        // Dot path — check if CWD has source files
        let canonical = std::fs::canonicalize(dir)
            .map(|p| crate::clean_path(&p.to_string_lossy()))
            .unwrap_or_else(|_| dir.to_string());
        if has_source_files(&canonical, extensions, 3) {
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
}

impl HandlerContext {
    /// Convenience accessor: read the current workspace directory.
    /// This replaces direct access to the old `server_dir` field.
    pub fn server_dir(&self) -> String {
        self.workspace.read().unwrap_or_else(|e| e.into_inner()).dir.clone()
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
/// 32KB gives comfortable headroom for adding more tips and parameter examples.
const XRAY_HELP_MIN_RESPONSE_BYTES: usize = 32_768;

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
    // Note: xray_fast uses its own file-list index, not the content index
    matches!(tool_name, "xray_grep" | "xray_reindex")
}

/// Returns true when a tool requires the definition index to be ready.
fn requires_def_index(tool_name: &str) -> bool {
    matches!(tool_name, "xray_definitions" | "xray_callers" | "xray_reindex_definitions")
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

    // Check readiness: if the required index is still building, return early
    if requires_content_index(tool_name) && !ctx.content_ready.load(Ordering::Acquire) {
        if tool_name == "xray_reindex" {
            return ToolCallResult::error(ALREADY_BUILDING_MSG.to_string());
        }
        return ToolCallResult::error(INDEX_BUILDING_MSG.to_string());
    }
    if requires_def_index(tool_name) && !ctx.def_ready.load(Ordering::Acquire) {
        if tool_name == "xray_reindex_definitions" {
            return ToolCallResult::error(ALREADY_BUILDING_MSG.to_string());
        }
        return ToolCallResult::error(DEF_INDEX_BUILDING_MSG.to_string());
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

    if result.is_error {
        return result;
    }

    let result = utils::inject_response_guidance(result, tool_name, &ctx.server_ext, ctx);

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

                indexes.push(json!({
                    "type": "content",
                    "root": idx.root,
                    "files": idx.files.len(),
                    "uniqueTokens": idx.index.len(),
                    "totalTokens": idx.total_tokens,
                    "extensions": idx.extensions,
                    "sizeMb": size_mb,
                    "ageHours": (age_secs as f64 / 3600.0 * 10.0).round() / 10.0,
                    "inMemory": true,
                }));
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

    if !memory_estimate.as_object().is_none_or(|m| m.is_empty()) {
        info["memoryEstimate"] = memory_estimate;
    }

    ToolCallResult::success(utils::json_to_string(&info))
}

fn handle_xray_reindex(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
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
    let workspace_changed = !dir.eq_ignore_ascii_case(&previous_dir);

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
        ws.dir = dir.clone();
        ws.mode = WorkspaceBindingMode::ManualOverride;
        ws.generation += 1;
        ws.status = WorkspaceStatus::Reindexing;
        info!(dir = %dir, previous = %previous_dir, generation = ws.generation, "Workspace switched via xray_reindex");
    }

    info!(dir = %dir, ext = %ext, "Rebuilding content index");
    let start = Instant::now();

    // Load-first: try cached index from disk (~1-2s), fall back to build (~30s)
    let extensions: Vec<String> = ext.split(',').map(|s| s.trim().to_string()).collect();
    let loaded = load_content_index(&dir, &ext, &ctx.index_base)
        .ok()
        .or_else(|| find_content_index_for_dir(&dir, &ctx.index_base, &extensions));

    let (new_index, index_action) = if let Some(idx) = loaded {
        (idx, "loaded_cache")
    } else {
        // Build from scratch
        let idx = match build_content_index(&ContentIndexArgs {
            dir: dir.to_string(),
            ext: ext.clone(),
            max_age_hours: 24,
            hidden: false,
            no_ignore: false,
            threads: 0,
            min_token_len: DEFAULT_MIN_TOKEN_LEN,
        }) {
            Ok(idx) => idx,
            Err(e) => {
                // Full rollback on failure
                if workspace_changed {
                    let mut ws = ctx.workspace.write().unwrap_or_else(|e| e.into_inner());
                    ws.dir = previous_dir.clone();
                    ws.mode = old_mode;
                    ws.generation = old_generation;
                    ws.status = WorkspaceStatus::Resolved;
                }
                return ToolCallResult::error(format!("Failed to build content index: {}", e));
            }
        };

        // Save to disk
        if let Err(e) = save_content_index(&idx, &ctx.index_base) {
            warn!(error = %e, "Failed to save reindexed content to disk");
        }

        let file_count = idx.files.len();
        let token_count = idx.index.len();

        // Drop build result and reload from disk for compact memory layout
        drop(idx);
        crate::index::force_mimalloc_collect();
        crate::index::log_memory("reindex: after drop+mi_collect (content)");

        let reloaded = match load_content_index(&dir, &ext, &ctx.index_base) {
            Ok(idx) => idx,
            Err(e) => {
                warn!(error = %e, "Failed to reload content index from disk after reindex, rebuilding");
                match build_content_index(&ContentIndexArgs {
                    dir: dir.to_string(), ext: ext.clone(),
                    max_age_hours: 24, hidden: false, no_ignore: false,
                    threads: 0, min_token_len: DEFAULT_MIN_TOKEN_LEN,
                }) {
                    Ok(idx) => idx,
                    Err(e2) => return ToolCallResult::error(format!("Failed to rebuild content index: {}", e2)),
                }
            }
        };
        let _ = (file_count, token_count); // suppress unused warnings
        (reloaded, "rebuilt")
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
        Err(e) => return ToolCallResult::error(format!("Failed to update in-memory index: {}", e)),
    }

    // Mark workspace as resolved
    {
        let mut ws = ctx.workspace.write().unwrap_or_else(|e| e.into_inner());
        ws.status = WorkspaceStatus::Resolved;
    }

    // Force mimalloc to return freed pages (old index) to OS
    crate::index::force_mimalloc_collect();
    crate::index::log_memory("reindex: after replace+mi_collect (content)");

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
    output["serverDir"] = json!(dir);

    ToolCallResult::success(utils::json_to_string(&output))
}

fn handle_xray_reindex_definitions(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
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
    let workspace_changed = !dir.eq_ignore_ascii_case(&previous_dir);

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

    // Update workspace binding if dir changed
    if workspace_changed {
        let mut ws = ctx.workspace.write().unwrap_or_else(|e| e.into_inner());
        ws.dir = dir.clone();
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
            })
        }
    };

    // Update in-memory cache
    match def_index_arc.write() {
        Ok(mut idx) => {
            *idx = new_index;
        }
        Err(e) => {
            // Rollback workspace status to avoid getting stuck in Reindexing
            let mut ws = ctx.workspace.write().unwrap_or_else(|e| e.into_inner());
            ws.status = WorkspaceStatus::Resolved;
            return ToolCallResult::error(format!("Failed to update in-memory definition index: {}", e));
        }
    }

    // Mark workspace as resolved
    {
        let mut ws = ctx.workspace.write().unwrap_or_else(|e| e.into_inner());
        ws.status = WorkspaceStatus::Resolved;
    }

    // Force mimalloc to return freed pages (old index) to OS
    crate::index::force_mimalloc_collect();
    crate::index::log_memory("reindex: after replace+mi_collect (def)");

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