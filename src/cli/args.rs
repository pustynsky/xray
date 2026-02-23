//! CLI argument structs for all subcommands.

use clap::Parser;

#[derive(Parser, Debug)]
pub struct FindArgs {
    /// Search pattern (substring or regex with --regex)
    pub pattern: String,

    /// Root directory to search in
    #[arg(short, long, default_value = ".")]
    pub dir: String,

    /// Treat pattern as a regular expression
    #[arg(short, long)]
    pub regex: bool,

    /// Search file contents instead of file names
    #[arg(long)]
    pub contents: bool,

    /// Show hidden files
    #[arg(long)]
    pub hidden: bool,

    /// Maximum search depth (0 = unlimited)
    #[arg(long, default_value = "0")]
    pub max_depth: usize,

    /// Number of parallel threads (0 = auto)
    #[arg(short, long, default_value = "0")]
    pub threads: usize,

    /// Case-insensitive search
    #[arg(short = 'i', long)]
    pub ignore_case: bool,

    /// Also search .gitignore'd files
    #[arg(long)]
    pub no_ignore: bool,

    /// Show only the count of matches
    #[arg(short = 'c', long)]
    pub count: bool,

    /// File extension filter
    #[arg(short, long)]
    pub ext: Option<String>,
}

#[derive(Parser, Debug)]
pub struct IndexArgs {
    /// Directory to index
    #[arg(short, long, default_value = ".")]
    pub dir: String,

    /// Max index age in hours before auto-reindex (default: 24)
    #[arg(long, default_value = "24")]
    pub max_age_hours: u64,

    /// Include hidden files in index
    #[arg(long)]
    pub hidden: bool,

    /// Include .gitignore'd files
    #[arg(long)]
    pub no_ignore: bool,

    /// Number of parallel threads (0 = auto)
    #[arg(short, long, default_value = "0")]
    pub threads: usize,
}

#[derive(Parser, Debug)]
pub struct FastArgs {
    /// Search pattern (substring or regex with --regex)
    pub pattern: String,

    /// Directory whose index to search (must be indexed first)
    #[arg(short, long, default_value = ".")]
    pub dir: String,

    /// Treat pattern as a regular expression
    #[arg(short, long)]
    pub regex: bool,

    /// Case-insensitive search
    #[arg(short = 'i', long)]
    pub ignore_case: bool,

    /// Show only the count of matches
    #[arg(short = 'c', long)]
    pub count: bool,

    /// File extension filter
    #[arg(short, long)]
    pub ext: Option<String>,

    /// Auto-reindex if index is stale
    #[arg(long, default_value = "true")]
    pub auto_reindex: bool,

    /// Search only directories
    #[arg(long)]
    pub dirs_only: bool,

    /// Search only files
    #[arg(long)]
    pub files_only: bool,

    /// Minimum file size in bytes
    #[arg(long)]
    pub min_size: Option<u64>,

    /// Maximum file size in bytes
    #[arg(long)]
    pub max_size: Option<u64>,
}

#[derive(Parser, Debug)]
pub struct ContentIndexArgs {
    /// Directory to index
    #[arg(short, long, default_value = ".")]
    pub dir: String,

    /// File extensions to index (comma-separated, e.g. "cs,rs,py,js")
    #[arg(short, long, default_value = "cs")]
    pub ext: String,

    /// Max index age in hours before auto-reindex (default: 24)
    #[arg(long, default_value = "24")]
    pub max_age_hours: u64,

    /// Include hidden files
    #[arg(long)]
    pub hidden: bool,

    /// Include .gitignore'd files
    #[arg(long)]
    pub no_ignore: bool,

    /// Number of parallel threads (0 = auto)
    #[arg(short, long, default_value = "0")]
    pub threads: usize,

    /// Minimum token length to index (default: 2)
    #[arg(long, default_value = "2")]
    pub min_token_len: usize,
}

#[derive(Parser, Debug)]
pub struct CleanupArgs {
    /// Remove indexes only for this directory (instead of removing orphaned indexes)
    #[arg(short, long)]
    pub dir: Option<String>,
}

#[derive(Parser, Debug)]
#[command(after_long_help = r#"WHAT IS MCP:
  Model Context Protocol (MCP) is a JSON-RPC 2.0 protocol over stdio that
  allows AI agents (VS Code Copilot, Roo/Cline, Claude) to call tools natively.
  The server reads JSON requests from stdin and writes responses to stdout.

EXAMPLES:
  Basic:          search-index serve --dir C:\Projects\MyApp --ext cs
  Multi-ext:      search-index serve --dir C:\Projects --ext cs,csproj,sql
  C# + TypeScript: search-index serve --dir C:\Projects --ext cs,ts,tsx
  With watcher:   search-index serve --dir C:\Projects --ext cs --watch
  With defs:      search-index serve --dir C:\Projects --ext cs --watch --definitions
  TS defs:        search-index serve --dir C:\Projects --ext ts,tsx --watch --definitions
  Custom debounce: search-index serve --dir . --ext rs --watch --debounce-ms 1000

VS CODE CONFIGURATION (.vscode/mcp.json):
  {
    "servers": {
      "search-index": {
        "command": "search-index",
        "args": ["serve", "--dir", "C:\\Projects\\MyApp", "--ext", "cs", "--watch", "--definitions"]
      }
    }
  }

AVAILABLE TOOLS (exposed via MCP):
  search_grep        -- Search content index (TF-IDF ranked, regex, phrase, multi-term)
  search_definitions -- Search code definitions: classes, methods, interfaces, enums, SPs,
                        functions, type aliases, variables. Supports C#, TypeScript/TSX,
                        and SQL (.sql files: stored procedures, tables, views, functions,
                        types, indexes, columns via regex parser; call sites from SP bodies).
                        Supports containsLine to find which method/class/function contains
                        a line. (requires --definitions flag)
  search_callers     -- Find all callers of a method and build a call tree (up/down).
                       Combines grep index + AST index. Replaces 7+ manual queries with 1.
                       Supports C#, TypeScript/TSX (DI-aware, inject() support), and SQL
                       (call sites from EXEC/FROM/JOIN/INSERT/UPDATE/DELETE in SP bodies).
                       Note: calls through local variables (var x = ...; x.Method())
                       may not be detected (AST parsing without type inference).
                       (requires --definitions flag)
  search_find        -- Live filesystem search (no index, slow for large dirs)
  search_fast        -- Search file name index (instant)
  search_info        -- Show all indexes
  search_reindex     -- Force rebuild + reload index
  search_git_history -- Commit history for a file (cached or git CLI)
  search_git_diff    -- Commit history with full diff/patch for a file
  search_git_authors -- Top authors for a file ranked by commit count
  search_git_activity-- Repo-wide activity (all changed files) for a date range
  search_git_blame   -- Line-by-line git blame for a file or line range
  search_branch_status-- Show current git branch status, behind/ahead counts, dirty files
  search_help        -- Show tips and best practices for effective search tool usage
  search_reindex_definitions -- Re-index code definitions (tree-sitter). Requires --definitions

HOW IT WORKS:
  1. On startup: loads (or builds) content index into RAM (~0.7-1.6s one-time)
  2. With --definitions: loads cached definition index from disk (~1.5s),
     or builds it using tree-sitter on first use (~16-32s for 48K files)
  3. Starts JSON-RPC event loop on stdin/stdout
  4. All search queries use in-memory index (~0.6-4ms per query)
  5. With --watch: file changes update both indexes incrementally (<1s/file)
  6. Logging goes to stderr (never pollutes JSON-RPC on stdout)
"#)]
pub struct ServeArgs {
    /// Directory to index and serve.
    #[arg(short, long, default_value = ".")]
    pub dir: String,

    /// File extensions to index, comma-separated.
    #[arg(short, long, default_value = "cs")]
    pub ext: String,

    /// Watch for file changes and update index incrementally.
    #[arg(long)]
    pub watch: bool,

    /// Debounce delay in ms for file watcher.
    #[arg(long, default_value = "500")]
    pub debounce_ms: u64,

    /// Log level for stderr output (error, warn, info, debug)
    #[arg(long, default_value = "info")]
    pub log_level: String,

    /// If more than N files change in one debounce window, do a full reindex.
    #[arg(long, default_value = "100")]
    pub bulk_threshold: usize,

    /// Also load (or build) a code definition index using tree-sitter.
    #[arg(long)]
    pub definitions: bool,

    /// Include performance metrics in every tool response summary.
    #[arg(long)]
    pub metrics: bool,

    /// Maximum response size in KB before truncation (0 = no limit, default: 16).
    /// Prevents large search results from filling the LLM context window.
    #[arg(long, default_value = "16")]
    pub max_response_kb: usize,

    /// Enable memory diagnostics: write Working Set / Commit / Peak to a log file
    /// in the index directory (memory.log). Useful for diagnosing memory spikes.
    #[arg(long)]
    pub memory_log: bool,
}

#[derive(Parser, Debug)]
#[command(after_long_help = r#"EXAMPLES:
  Single term:     search-index grep "HttpClient" -d C:\Projects -e cs
  Multi-term OR:   search-index grep "HttpClient,ILogger,Task" -d C:\Projects -e cs
  Multi-term AND:  search-index grep "HttpClient,ILogger" -d C:\Projects -e cs --all
  Regex:           search-index grep "i.*cache" -d C:\Projects -e cs --regex
  Regex + lines:   search-index grep ".*factory" -d C:\Projects -e cs --regex --show-lines
  Top 10 results:  search-index grep "HttpClient" -d C:\Projects --max-results 10
  Exclude dirs:    search-index grep "HttpClient" -d . -e cs --exclude-dir test --exclude-dir E2E
  Exclude files:   search-index grep "HttpClient" -d . -e cs --exclude Mock
  Context lines:   search-index grep "HttpClient" -d . -e cs --show-lines -C 3
  Before/after:    search-index grep "HttpClient" -d . -e cs --show-lines -B 2 -A 5
  Exact tokens:    search-index grep "UserService" -d C:\Projects -e cs --exact

NOTES:
  - Requires a content index. Build one first:
      search-index content-index -d C:\Projects -e cs,rs,py
  - Results sorted by TF-IDF relevance (most relevant files first)
  - Multi-term: comma-separated, OR by default, AND with --all
  - Regex: pattern matched against all indexed tokens (e.g. 754K unique tokens)
  - Default: substring search via trigram index (finds IUserService, m_userService)
  - Use --exact to search for exact tokens only (disables substring matching)
  - Use --show-lines to see actual source code lines from matching files
  - --exclude-dir and --exclude filter results by path substring (case-insensitive)
  - Context lines (-C/-B/-A) show surrounding code, like grep -C
"#)]
pub struct GrepArgs {
    /// Search term(s). Comma-separated for multi-term.
    pub pattern: String,

    /// Directory whose content index to search.
    #[arg(short, long, default_value = ".")]
    pub dir: String,

    /// Show only the count of matching files.
    #[arg(short = 'c', long)]
    pub count: bool,

    /// Show actual source code lines.
    #[arg(long)]
    pub show_lines: bool,

    /// Automatically rebuild index if stale.
    #[arg(long, default_value = "true")]
    pub auto_reindex: bool,

    /// Filter by file extension.
    #[arg(short, long)]
    pub ext: Option<String>,

    /// Maximum results to display (0 = all).
    #[arg(long, default_value = "0")]
    pub max_results: usize,

    /// AND mode: file must contain ALL terms.
    #[arg(long)]
    pub all: bool,

    /// Treat pattern as regex.
    #[arg(short, long)]
    pub regex: bool,

    /// Exclude directories by substring.
    #[arg(long, action = clap::ArgAction::Append)]
    pub exclude_dir: Vec<String>,

    /// Exclude files by path substring.
    #[arg(long, action = clap::ArgAction::Append)]
    pub exclude: Vec<String>,

    /// Context lines around each match (like grep -C).
    #[arg(short = 'C', long, default_value = "0")]
    pub context: usize,

    /// Lines before each match (like grep -B).
    #[arg(short = 'B', long, default_value = "0")]
    pub before: usize,

    /// Lines after each match (like grep -A).
    #[arg(short = 'A', long, default_value = "0")]
    pub after: usize,

    /// Phrase search: find exact phrase.
    #[arg(long)]
    pub phrase: bool,

    /// Exact token matching only (disables default substring search).
    /// By default, grep uses trigram-based substring matching that finds
    /// compound identifiers (e.g. "UserService" matches IUserService, m_userService).
    /// Use --exact to search for exact tokens only.
    #[arg(long)]
    pub exact: bool,
}