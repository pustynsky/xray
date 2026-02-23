//! Core data types for the definition index.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ─── Definition Kind ─────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefinitionKind {
    // C# kinds
    Class,
    Interface,
    Enum,
    Struct,
    Record,
    Method,
    Property,
    Field,
    Constructor,
    Delegate,
    Event,
    EnumMember,
    // TypeScript kinds
    Function,
    TypeAlias,
    Variable,
    // SQL kinds
    StoredProcedure,
    Table,
    View,
    SqlFunction,
    UserDefinedType,
    Column,
    SqlIndex,
}

impl DefinitionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Enum => "enum",
            Self::Struct => "struct",
            Self::Record => "record",
            Self::Method => "method",
            Self::Property => "property",
            Self::Field => "field",
            Self::Constructor => "constructor",
            Self::Delegate => "delegate",
            Self::Event => "event",
            Self::EnumMember => "enumMember",
            Self::Function => "function",
            Self::TypeAlias => "typeAlias",
            Self::Variable => "variable",
            Self::StoredProcedure => "storedProcedure",
            Self::Table => "table",
            Self::View => "view",
            Self::SqlFunction => "sqlFunction",
            Self::UserDefinedType => "userDefinedType",
            Self::Column => "column",
            Self::SqlIndex => "sqlIndex",
        }
    }
}

impl std::fmt::Display for DefinitionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for DefinitionKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "class" => Ok(Self::Class),
            "interface" => Ok(Self::Interface),
            "enum" => Ok(Self::Enum),
            "struct" => Ok(Self::Struct),
            "record" => Ok(Self::Record),
            "method" => Ok(Self::Method),
            "property" => Ok(Self::Property),
            "field" => Ok(Self::Field),
            "constructor" => Ok(Self::Constructor),
            "delegate" => Ok(Self::Delegate),
            "event" => Ok(Self::Event),
            "enummember" => Ok(Self::EnumMember),
            "function" => Ok(Self::Function),
            "typealias" => Ok(Self::TypeAlias),
            "variable" => Ok(Self::Variable),
            "storedprocedure" => Ok(Self::StoredProcedure),
            "table" => Ok(Self::Table),
            "view" => Ok(Self::View),
            "sqlfunction" => Ok(Self::SqlFunction),
            "userdefinedtype" => Ok(Self::UserDefinedType),
            "column" => Ok(Self::Column),
            "sqlindex" => Ok(Self::SqlIndex),
            other => Err(format!("Unknown definition kind: '{}'", other)),
        }
    }
}

// ─── Definition Entry ────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DefinitionEntry {
    pub file_id: u32,
    pub name: String,
    pub kind: DefinitionKind,
    pub line_start: u32,
    pub line_end: u32,
    pub parent: Option<String>,
    pub signature: Option<String>,
    pub modifiers: Vec<String>,
    pub attributes: Vec<String>,
    pub base_types: Vec<String>,
}

// ─── Code Stats ──────────────────────────────────────────────────────

/// Code complexity metrics computed during AST walk.
/// Only populated for Method, Constructor, Function, Property (expression body).
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CodeStats {
    /// Cyclomatic complexity: number of linearly independent execution paths.
    /// Base = 1, each branching node adds +1.
    pub cyclomatic_complexity: u16,
    /// SonarSource Cognitive Complexity: penalizes nesting depth.
    /// Measures code readability, not just structural complexity.
    pub cognitive_complexity: u16,
    /// Maximum nesting depth of control flow structures.
    pub max_nesting_depth: u8,
    /// Number of parameters in the method/function signature.
    pub param_count: u8,
    /// Number of return + throw statements (exit points).
    pub return_count: u8,
    /// Number of method/function calls in the body (fan-out).
    pub call_count: u16,
    /// Number of lambda/arrow function expressions in the body.
    pub lambda_count: u8,
}

/// A call site found in a method/constructor body via AST analysis.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CallSite {
    /// Name of the method being called, e.g., "GetUser"
    pub method_name: String,
    /// Resolved type of the receiver, e.g., "IUserService".
    /// None for simple calls like Foo() where receiver type is unknown.
    pub receiver_type: Option<String>,
    /// Line number where the call occurs (1-based)
    pub line: u32,
    /// Whether the receiver type at the call site had generic parameters,
    /// e.g., `new List<int>()` → true, `new List()` → false.
    /// Used to filter out name collisions with non-generic classes.
    #[serde(default)]
    pub receiver_is_generic: bool,
}

// ─── Definition Index ────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug)]
pub struct DefinitionIndex {
    pub root: String,
    pub created_at: u64,
    pub extensions: Vec<String>,
    /// file_id -> file path
    pub files: Vec<String>,
    /// All definitions
    pub definitions: Vec<DefinitionEntry>,
    /// name (lowercased) -> Vec<index into definitions>
    pub name_index: HashMap<String, Vec<u32>>,
    /// kind -> Vec<index into definitions>
    pub kind_index: HashMap<DefinitionKind, Vec<u32>>,
    /// attribute name (lowercased) -> Vec<index into definitions>
    pub attribute_index: HashMap<String, Vec<u32>>,
    /// base type name (lowercased) -> Vec<index into definitions>
    pub base_type_index: HashMap<String, Vec<u32>>,
    /// file_id -> Vec<index into definitions>
    pub file_index: HashMap<u32, Vec<u32>>,
    /// Path -> file_id lookup (for watcher)
    pub path_to_id: HashMap<PathBuf, u32>,
    /// def_idx -> list of call sites found in that method/constructor body.
    /// Only populated for Method and Constructor kinds.
    #[serde(default)]
    pub method_calls: HashMap<u32, Vec<CallSite>>,
    /// Number of files that could not be read (IO errors) during index build.
    #[serde(default)]
    pub parse_errors: usize,
    /// Number of files that contained non-UTF8 bytes and were read with lossy conversion.
    #[serde(default)]
    pub lossy_file_count: usize,
    /// Files that were read and parsed but produced 0 definitions.
    /// Each entry is (file_id, byte_size). Files >500 bytes with 0 defs are suspicious.
    #[serde(default)]
    pub empty_file_ids: Vec<(u32, u64)>,
    /// def_idx -> CodeStats for methods/constructors/functions.
    /// Always populated when --definitions is used.
    #[serde(default)]
    pub code_stats: HashMap<u32, CodeStats>,
    /// Extension method name → Vec of static class names containing the extension.
    /// Populated during C# parsing by detecting static classes with `this` parameter methods.
    #[serde(default)]
    pub extension_methods: HashMap<String, Vec<String>>,
    /// Angular component selector → def_idx of the @Component class.
    /// Example: "datahub-compact-view" → [idx of DatahubCompactViewComponent]
    #[serde(default)]
    pub selector_index: HashMap<String, Vec<u32>>,
    /// def_idx of component → child selectors from HTML template.
    /// Example: idx of DatahubEmbedComponent → ["datahub-compact-view", "pbi-spinner"]
    #[serde(default)]
    pub template_children: HashMap<u32, Vec<String>>,
}

impl Default for DefinitionIndex {
    fn default() -> Self {
        Self {
            root: String::new(),
            created_at: 0,
            extensions: Vec::new(),
            files: Vec::new(),
            definitions: Vec::new(),
            name_index: HashMap::new(),
            kind_index: HashMap::new(),
            attribute_index: HashMap::new(),
            base_type_index: HashMap::new(),
            file_index: HashMap::new(),
            path_to_id: HashMap::new(),
            method_calls: HashMap::new(),
            code_stats: HashMap::new(),
            parse_errors: 0,
            lossy_file_count: 0,
            empty_file_ids: Vec::new(),
            extension_methods: HashMap::new(),
            selector_index: HashMap::new(),
            template_children: HashMap::new(),
        }
    }
}

// ─── CLI Args ────────────────────────────────────────────────────────

use clap::Parser;

#[derive(Parser, Debug)]
#[command(after_long_help = r#"WHAT IT DOES:
  Parses C#, TypeScript, and SQL files using tree-sitter to extract code structure:
    - C#: classes, interfaces, structs, enums, records, methods, constructors,
      properties, fields, delegates, events, enum members
    - TypeScript/TSX: functions, type aliases, variables, classes, interfaces, enums, methods
    - SQL: stored procedures, tables, views, functions, user-defined types
      (requires compatible tree-sitter-sql grammar)

  Each definition includes: name, kind, file path, line range, signature,
  modifiers, attributes (e.g. [ServiceProvider]), and base types.

  The index is saved to disk as a .code-structure file and can be loaded instantly
  by 'search-index serve --definitions'.

EXAMPLES:
  Index C# files:     search-index def-index --dir C:\Projects --ext cs
  Index TypeScript:   search-index def-index --dir C:\Projects --ext ts,tsx
  Index C# + SQL:     search-index def-index --dir C:\Projects --ext cs,sql
  Index all:          search-index def-index --dir C:\Projects --ext cs,sql,ts,tsx
  Custom threads:     search-index def-index --dir C:\Projects --ext cs --threads 8

PERFORMANCE:
  48,643 files -> 846,167 definitions in ~14s (24 threads)
  Index size: ~230 MB on disk
"#)]
pub struct DefIndexArgs {
    /// Directory to recursively scan for source files to parse
    #[arg(short, long, default_value = ".")]
    pub dir: String,

    /// File extensions to parse, comma-separated.
    /// C# (.cs) uses tree-sitter-c-sharp grammar.
    /// TypeScript (.ts, .tsx) uses tree-sitter-typescript grammar.
    /// SQL (.sql) parsing is currently disabled (no compatible T-SQL grammar for tree-sitter 0.24).
    #[arg(short, long, default_value = "cs")]
    pub ext: String,

    /// Number of parallel parsing threads. Each thread gets its own
    /// tree-sitter parser instance. 0 = auto-detect CPU cores.
    #[arg(short, long, default_value = "0")]
    pub threads: usize,
}

#[derive(Parser, Debug)]
#[command(after_long_help = r#"WHAT IT DOES:
  Loads a previously built definition index (.code-structure file) from disk and
  reports coverage statistics: how many files have definitions, how many
  are empty, and which "suspicious" files (>N bytes but 0 definitions)
  may have parsing issues.

  This is a read-only operation — it does NOT rebuild the index.

EXAMPLES:
  Audit with defaults:     search def-audit --dir C:\Projects --ext cs
  Lower threshold:         search def-audit --dir C:\Projects --ext cs --min-bytes 2000
  Show lossy files too:    search def-audit --dir C:\Projects --ext cs --show-lossy
"#)]
pub struct DefAuditArgs {
    /// Directory that was indexed (must match the --dir used during def-index)
    #[arg(short, long, default_value = ".")]
    pub dir: String,

    /// File extensions that were indexed (must match the --ext used during def-index)
    #[arg(short, long, default_value = "cs")]
    pub ext: String,

    /// Minimum file size in bytes to flag as suspicious.
    /// Files with 0 definitions but more than this many bytes are reported.
    #[arg(long, default_value = "500")]
    pub min_bytes: u64,

    /// Also show files that required lossy UTF-8 conversion.
    #[arg(long)]
    pub show_lossy: bool,
}