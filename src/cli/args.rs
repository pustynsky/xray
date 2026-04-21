//! CLI argument structs for all subcommands.
//!
//! Every *Args struct with user-facing CLI flags provides an `impl Default`
//! that mirrors clap `default_value` attributes. This enables
//! `..Default::default()` in tests (reduces boilerplate when adding new
//! fields). A drift-test in `args_defaults_tests` ensures every manual
//! `impl Default` stays in sync with clap defaults — if you change a
//! `default_value` attribute, update `impl Default` or the drift test will
//! fail.
//!
//! Note: `..Default::default()` is only used in **tests**. Production code
//! should continue to list all fields explicitly for readability.

use clap::Parser;


#[derive(Parser, Debug, PartialEq)]
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

    /// Respect .git/info/exclude (by default xray ignores it so local-excluded files are indexed)
    #[arg(long)]
    pub respect_git_exclude: bool,

    /// Number of parallel threads (0 = auto)
    #[arg(short, long, default_value = "0")]
    pub threads: usize,
}

impl Default for IndexArgs {
    fn default() -> Self {
        Self {
            dir: ".".to_string(),
            max_age_hours: 24,
            hidden: false,
            no_ignore: false,
            respect_git_exclude: false,
            threads: 0,
        }
    }
}

#[derive(Parser, Debug, PartialEq)]
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

    /// Respect .git/info/exclude when auto-rebuilding the file-list index
    /// (by default xray ignores it so local-excluded files are indexed)
    #[arg(long)]
    pub respect_git_exclude: bool,
}

impl Default for FastArgs {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            dir: ".".to_string(),
            regex: false,
            ignore_case: false,
            count: false,
            ext: None,
            auto_reindex: true,
            dirs_only: false,
            files_only: false,
            min_size: None,
            max_size: None,
            respect_git_exclude: false,
        }
    }
}

#[derive(Parser, Debug, PartialEq)]
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

    /// Respect .git/info/exclude (by default xray ignores it so local-excluded files are indexed)
    #[arg(long)]
    pub respect_git_exclude: bool,

    /// Number of parallel threads (0 = auto)
    #[arg(short, long, default_value = "0")]
    pub threads: usize,

    /// Minimum token length to index (default: 2)
    #[arg(long, default_value = "2")]
    pub min_token_len: usize,
}

impl Default for ContentIndexArgs {
    fn default() -> Self {
        Self {
            dir: ".".to_string(),
            ext: "cs".to_string(),
            max_age_hours: 24,
            hidden: false,
            no_ignore: false,
            respect_git_exclude: false,
            threads: 0,
            min_token_len: 2,
        }
    }
}

#[derive(Parser, Debug)]
pub struct CleanupArgs {
    /// Remove indexes only for this directory (instead of removing orphaned indexes)
    #[arg(short, long)]
    pub dir: Option<String>,
}

/// Hidden test helper: create a content index with a specific format_version (for E2E testing).
#[derive(Parser, Debug)]
pub struct TestCreateStaleIndexArgs {
    /// Directory to index
    #[arg(short, long)]
    pub dir: String,

    /// File extensions to index (comma-separated)
    #[arg(short, long, default_value = "cs")]
    pub ext: String,

    /// Format version to write (0 = simulate legacy/stale index)
    #[arg(long, default_value = "0")]
    pub version: u32,
}

#[derive(Parser, Debug)]
#[command(after_long_help = r#"WHAT IS MCP:
  Model Context Protocol (MCP) is a JSON-RPC 2.0 protocol over stdio that
  allows AI agents (VS Code Copilot, Roo/Cline, Claude) to call tools natively.
  The server reads JSON requests from stdin and writes responses to stdout.

EXAMPLES:
  Basic:          xray serve --dir C:\Projects\MyApp --ext cs
  Multi-ext:      xray serve --dir C:\Projects --ext cs,csproj,sql
  C# + TypeScript: xray serve --dir C:\Projects --ext cs,ts,tsx
  With watcher:   xray serve --dir C:\Projects --ext cs --watch
  With defs:      xray serve --dir C:\Projects --ext cs --watch --definitions
  TS defs:        xray serve --dir C:\Projects --ext ts,tsx --watch --definitions
  Custom debounce: xray serve --dir . --ext rs --watch --debounce-ms 1000

VS CODE CONFIGURATION (.vscode/mcp.json):
  {
    "servers": {
      "xray": {
        "command": "xray",
        "args": ["serve", "--dir", "C:\\Projects\\MyApp", "--ext", "cs", "--watch", "--definitions"]
      }
    }
  }

AVAILABLE TOOLS (exposed via MCP):
  xray_grep        -- Search content index (TF-IDF ranked, regex, phrase, multi-term)
  xray_definitions -- Search code definitions: classes, methods, interfaces, enums, SPs,
                        functions, type aliases, variables. Supports C#, TypeScript/TSX,
                        and SQL (.sql files: stored procedures, tables, views, functions,
                        types, indexes, columns via regex parser; call sites from SP bodies).
                        Supports containsLine to find which method/class/function contains
                        a line. (requires --definitions flag)
  xray_callers     -- Find all callers of a method and build a call tree (up/down).
                       Combines grep index + AST index. Replaces 7+ manual queries with 1.
                       Supports C#, TypeScript/TSX (DI-aware, inject() support), and SQL
                       (call sites from EXEC/FROM/JOIN/INSERT/UPDATE/DELETE in SP bodies).
                       Note: calls through local variables (var x = ...; x.Method())
                       may not be detected (AST parsing without type inference).
                       (requires --definitions flag)
  xray_fast        -- Search file name index (instant)
  xray_info        -- Show all indexes
  xray_reindex     -- Force rebuild + reload index
  xray_git_history -- Commit history for a file (cached or git CLI)
  xray_git_diff    -- Commit history with full diff/patch for a file
  xray_git_authors -- Top authors for a file ranked by commit count
  xray_git_activity-- Activity (changed files) for a date range, optionally filtered by path
  xray_git_blame   -- Line-by-line git blame for a file or line range
  xray_branch_status-- Show current git branch status, behind/ahead counts, dirty files
  xray_help        -- Show tips and best practices for effective search tool usage
  xray_reindex_definitions -- Re-index code definitions (AST parser). Requires --definitions

HOW IT WORKS:
  1. On startup: loads (or builds) content index into RAM (~0.7-1.6s one-time)
  2. With --definitions: loads cached definition index from disk (~1.5s),
     or builds it using AST parsers on first use (~16-32s for 48K files)
  3. Starts JSON-RPC event loop on stdin/stdout
  4. All search queries use in-memory index (~0.6-4ms per query)
  5. With --watch: file changes update both indexes incrementally (<1s/file)
  6. Logging goes to stderr (never pollutes JSON-RPC on stdout)
"#)]
#[derive(PartialEq)]
pub struct ServeArgs {
    /// Directory to index and serve.
    #[arg(short, long, default_value = ".")]
    pub dir: String,

    /// File extensions to index.
    /// Accepts comma-separated ("rs,md") or space-separated ("rs" "md") values.
    /// In mcp.json args array, each extension can be a separate element:
    ///   ["--ext", "rs", "md"]  or  ["--ext", "rs,md"]
    #[arg(short, long, default_value = "cs", num_args = 1..)]
    pub ext: Vec<String>,

    /// Watch for file changes and update index incrementally.
    #[arg(long)]
    pub watch: bool,

    /// Debounce delay in ms for file watcher.
    #[arg(long, default_value = "500")]
    pub debounce_ms: u64,

    /// Log level for stderr output (error, warn, info, debug)
    #[arg(long, default_value = "info")]
    pub log_level: String,

    /// Also load (or build) a code definition index using AST parsers.
    #[arg(long)]
    pub definitions: bool,

    /// Include performance metrics in every tool response summary.
    #[arg(long)]
    pub metrics: bool,

    /// Maximum response size in KB before truncation (0 = no limit, default: 16).
    /// Prevents large search results from filling the LLM context window.
    #[arg(long, default_value = "16")]
    pub max_response_kb: usize,

    /// Enable debug logging: write MCP request/response traces and memory diagnostics
    /// to a per-server .debug.log file in the index directory. Useful for diagnosing
    /// performance issues and MCP traffic.
    #[arg(long)]
    pub debug_log: bool,

    /// Respect .git/info/exclude (by default xray ignores it so local-excluded files are indexed)
    #[arg(long)]
    pub respect_git_exclude: bool,

    /// Disable the periodic re-scan fail-safe that catches filesystem
    /// events the OS-level `notify` watcher dropped (best-effort on every
    /// platform; see `docs/bug-reports/bug-2026-04-21-watcher-misses-new-files-both-indexes.md`).
    /// Only relevant together with `--watch`.
    #[arg(long)]
    pub no_periodic_rescan: bool,

    /// Interval in seconds between periodic re-scans (default: 300).
    /// Minimum 10 seconds — values below are silently raised to avoid
    /// self-DoS on large workspaces. Only relevant together with
    /// `--watch` and when `--no-periodic-rescan` is NOT set.
    #[arg(long, default_value = "300")]
    pub rescan_interval_sec: u64,
}

impl Default for ServeArgs {
    fn default() -> Self {
        Self {
            dir: ".".to_string(),
            ext: vec!["cs".to_string()],
            watch: false,
            debounce_ms: 500,
            log_level: "info".to_string(),
            definitions: false,
            metrics: false,
            max_response_kb: 16,
            debug_log: false,
            respect_git_exclude: false,
            no_periodic_rescan: false,
            rescan_interval_sec: 300,
        }
    }
}

#[derive(Parser, Debug, PartialEq)]
#[command(after_long_help = r#"EXAMPLES:
  Single term:     xray grep "HttpClient" -d C:\Projects -e cs
  Multi-term OR:   xray grep "HttpClient,ILogger,Task" -d C:\Projects -e cs
  Multi-term AND:  xray grep "HttpClient,ILogger" -d C:\Projects -e cs --all
  Regex:           xray grep "i.*cache" -d C:\Projects -e cs --regex
  Regex + lines:   xray grep ".*factory" -d C:\Projects -e cs --regex --show-lines
  Top 10 results:  xray grep "HttpClient" -d C:\Projects --max-results 10
  Exclude dirs:    xray grep "HttpClient" -d . -e cs --exclude-dir test --exclude-dir E2E
  Exclude files:   xray grep "HttpClient" -d . -e cs --exclude Mock
  Context lines:   xray grep "HttpClient" -d . -e cs --show-lines -C 3
  Before/after:    xray grep "HttpClient" -d . -e cs --show-lines -B 2 -A 5
  Exact tokens:    xray grep "UserService" -d C:\Projects -e cs --exact

NOTES:
  - Requires a content index. Build one first:
      xray content-index -d C:\Projects -e cs,rs,py
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

    /// Maximum results to display (0 = all, default: 50 — same as MCP).
    #[arg(long, default_value = "50")]
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

    /// Respect .git/info/exclude when auto-rebuilding the content index
    /// (by default xray ignores it so local-excluded files are indexed)
    #[arg(long)]
    pub respect_git_exclude: bool,
}

impl Default for GrepArgs {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            dir: ".".to_string(),
            count: false,
            show_lines: false,
            auto_reindex: true,
            ext: None,
            max_results: 50,
            all: false,
            regex: false,
            exclude_dir: Vec::new(),
            exclude: Vec::new(),
            context: 0,
            before: 0,
            after: 0,
            phrase: false,
            exact: false,
            respect_git_exclude: false,
        }
    }
}

#[cfg(test)]
mod args_defaults_tests {
    //! Drift-tests: ensure manual `impl Default` stays in sync with clap
    //! `default_value` attributes. If you change a clap default, update
    //! the matching `impl Default` or these tests will fail.
    use super::*;
    use clap::Parser;

    #[test]
    fn index_args_default_matches_clap() {
        let parsed = IndexArgs::parse_from(["xray"]);
        assert_eq!(IndexArgs::default(), parsed);
    }

    #[test]
    fn content_index_args_default_matches_clap() {
        let parsed = ContentIndexArgs::parse_from(["xray"]);
        assert_eq!(ContentIndexArgs::default(), parsed);
    }

    #[test]
    fn serve_args_default_matches_clap() {
        let parsed = ServeArgs::parse_from(["xray"]);
        assert_eq!(ServeArgs::default(), parsed);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn fast_args_default_matches_clap() {
        // FastArgs.pattern is a required positional — pass dummy and
        // override pattern in Default for comparison.
        let parsed = FastArgs::parse_from(["xray", "dummy"]);
        let mut expected = FastArgs::default();
        expected.pattern = "dummy".to_string();
        assert_eq!(expected, parsed);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn grep_args_default_matches_clap() {
        // GrepArgs.pattern is a required positional — pass dummy and
        // override pattern in Default for comparison.
        let parsed = GrepArgs::parse_from(["xray", "dummy"]);
        let mut expected = GrepArgs::default();
        expected.pattern = "dummy".to_string();
        assert_eq!(expected, parsed);
    }
}