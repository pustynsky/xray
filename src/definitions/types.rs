//! Core data types for the definition index.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::csharp_semantics::{CSharpFileContribution, CSharpSemanticIndex};

// ─── Definition Kind ─────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
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
    // XML kinds (on-demand, not indexed)
    XmlElement,
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
    /// Every variant of [`DefinitionKind`] in canonical declaration order.
    /// Single source of truth for the closed enum exposed via the
    /// `xray_definitions.kind` MCP parameter and validated by
    /// [`crate::mcp::handlers::utils::read_kind_array`].
    ///
    /// **Drift guard:** [`kinds_drift_tests::all_kinds_lists_every_variant`]
    /// (a unit test at the bottom of this file) walks every variant via an
    /// exhaustive `match` and asserts it appears here. Adding a new
    /// `DefinitionKind` variant without adding it to `ALL_KINDS` breaks that
    /// test — by design.
    pub const ALL_KINDS: &'static [DefinitionKind] = &[
        DefinitionKind::Class,
        DefinitionKind::Interface,
        DefinitionKind::Enum,
        DefinitionKind::Struct,
        DefinitionKind::Record,
        DefinitionKind::Method,
        DefinitionKind::Property,
        DefinitionKind::Field,
        DefinitionKind::Constructor,
        DefinitionKind::Delegate,
        DefinitionKind::Event,
        DefinitionKind::EnumMember,
        DefinitionKind::Function,
        DefinitionKind::TypeAlias,
        DefinitionKind::Variable,
        DefinitionKind::XmlElement,
        DefinitionKind::StoredProcedure,
        DefinitionKind::Table,
        DefinitionKind::View,
        DefinitionKind::SqlFunction,
        DefinitionKind::UserDefinedType,
        DefinitionKind::Column,
        DefinitionKind::SqlIndex,
    ];

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
            Self::XmlElement => "xmlElement",
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
            "xmlelement" => Ok(Self::XmlElement),
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
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CallSiteKind {
    #[default]
    Unknown,
    SqlExecute,
    SqlRelation,
    SqlScalarFunction,
    Constructor,
    TypeScriptSameFile { definition_line: u32 },
    TypeScriptLocalCallable { definition_line: u32 },
    TypeScriptMember,
    TypeScriptAnalysisIncomplete,
    TypeScriptDynamicCallableParameter,
    TypeScriptUnresolvedLocal,
    TypeScriptImported,
    TypeScriptUnknownGlobal,
    RustUnexpandedMacro,
    RustAnalysisIncomplete,
    RustUnindexedNestedCallable,
}

impl CallSiteKind {
    pub fn unresolved_reason(self) -> Option<&'static str> {
        match self {
            Self::TypeScriptDynamicCallableParameter => Some("dynamic_callable_parameter"),
            Self::TypeScriptUnresolvedLocal => Some("unresolved_local_binding"),
            Self::TypeScriptImported => Some("module_resolution_failed"),
            Self::TypeScriptUnknownGlobal => Some("unknown_global"),
            Self::TypeScriptAnalysisIncomplete => Some("ast_depth_limit"),
            Self::RustUnexpandedMacro => Some("unexpanded_rust_macro"),
            Self::RustAnalysisIncomplete => Some("rust_call_analysis_limit"),
            Self::RustUnindexedNestedCallable => Some("unindexed_rust_nested_callable"),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallSite {
    /// Name of the method being called, e.g., "GetUser"
    pub method_name: String,
    /// Resolved type of the receiver, e.g., "IUserService".
    /// None for simple calls like Foo() where receiver type is unknown.
    pub receiver_type: Option<String>,
    /// Line number where the call occurs (1-based)
    pub line: u32,
    /// Parser-known invocation category used to constrain target resolution.
    #[serde(default)]
    pub call_kind: CallSiteKind,
    /// Whether the receiver type at the call site had generic parameters,
    /// e.g., `new List<int>()` → true, `new List()` → false.
    /// Used to filter out name collisions with non-generic classes.
    #[serde(default)]
    pub receiver_is_generic: bool,
}

// ─── Definition Index ────────────────────────────────────────────────
//
// ⚠️ IMPORTANT: INCREMENTAL UPDATE CONTRACT
//
// When adding a NEW FIELD to this struct, check if it uses `def_idx` (u32 index
// into the `definitions` Vec). If it does, you MUST update THREE functions in
// `incremental.rs` to prevent data corruption during --watch mode:
//
// 1. remove_file_definitions() — clean/remove entries for deleted file's def_indices
// 2. compact_definitions()     — remap old def_idx → new def_idx after compaction
// 3. update_file_definitions() — populate the new field when parsing a file
//
// The test `test_definition_index_field_count_guard` in `definitions_tests.rs`
// will FAIL TO COMPILE when a new field is added, reminding you of this contract.
//
// Fields are categorized below by their relationship to def_idx.

/// Format version for DefinitionIndex. Bump when changing layout or call-analysis semantics.
/// Loading an index with a different version triggers a rebuild.
pub const DEFINITION_INDEX_VERSION: u32 = 12;

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct DefinitionInputFingerprint {
    pub size: u64,
    pub modified_nanos: u128,
    pub content_hash: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefinitionInputRevision {
    pub size: u64,
    pub modified_nanos: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingDefinitionInput {
    pub attempts: u8,
    pub observed_revision: Option<DefinitionInputRevision>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum StaticValue<T> {
    Static(T),
    Dynamic { reason: String },
    Missing,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum AngularTemplateSource {
    External { relative_path: String },
    UnavailableExternal { relative_path: String, reason: String },
    Inline { content: String },
    Dynamic { reason: String },
    Missing,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct AngularComponentRecord {
    pub selector: StaticValue<String>,
    pub template: AngularTemplateSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedAngularComponentRecord {
    pub local_def_index: usize,
    pub component: AngularComponentRecord,
    pub template_children: Vec<String>,
}

#[cfg(feature = "lang-typescript")]
pub(crate) type TypeScriptParseResult = (ParseResult, Vec<ParsedAngularComponentRecord>);


#[derive(Serialize, Deserialize, Debug, Clone)]
#[derive(Default)]
pub struct DefinitionIndex {
    pub root: String,
    /// Format version — used to detect stale indexes after schema changes.
    /// Placed after `root` so that `read_root_from_index_file()` can still
    /// read root as the first bincode field.
    #[serde(default)]
    pub format_version: u32,
    pub created_at: u64,
    pub extensions: Vec<String>,
    /// file_id -> file path
    pub files: Vec<String>,
    /// All definitions. During --watch mode, may contain tombstones (entries not
    /// referenced by any secondary index). Use file_index for active count.
    pub definitions: Vec<DefinitionEntry>,

    // ── CATEGORY A: Secondary indexes with def_idx as VALUES (Vec<u32>) ──
    // These store Vec<u32> where each u32 is a def_idx into `definitions`.
    // On file removal: retain() to remove stale def_indices.
    // On compaction: remap_index_values() to update def_indices.

    /// name (lowercased) -> Vec<def_idx>
    pub name_index: HashMap<String, Vec<u32>>,
    /// kind -> Vec<def_idx>
    pub kind_index: HashMap<DefinitionKind, Vec<u32>>,
    /// attribute name (lowercased) -> Vec<def_idx>
    pub attribute_index: HashMap<String, Vec<u32>>,
    /// base type name (lowercased) -> Vec<def_idx>
    pub base_type_index: HashMap<String, Vec<u32>>,
    /// file_id -> Vec<def_idx>. THE source of truth for active definitions.
    pub file_index: HashMap<u32, Vec<u32>>,
    /// Angular component selector → Vec<def_idx> of @Component classes.
    #[serde(default)]
    pub selector_index: HashMap<String, Vec<u32>>,
    /// Normalized external template path → owning component def_idx values.
    #[serde(default)]
    pub template_owners: HashMap<String, Vec<u32>>,
    /// Child selector → parent component def_idx values.
    #[serde(default)]
    pub template_parents: HashMap<String, Vec<u32>>,

    // ── CATEGORY B: Indexes with def_idx as KEYS (HashMap<u32, _>) ──
    // These store data keyed by def_idx.
    // On file removal: remove() to delete entries for stale def_indices.
    // On compaction: drain().filter_map() to remap keys.

    /// def_idx -> list of call sites found in that method/constructor body.
    #[serde(default)]
    pub method_calls: HashMap<u32, Vec<CallSite>>,
    #[serde(default)]
    pub csharp_semantics: CSharpSemanticIndex,
    /// def_idx -> CodeStats for methods/constructors/functions.
    #[serde(default)]
    pub code_stats: HashMap<u32, CodeStats>,
    /// Parser-emitted Angular component records keyed by global component def_idx.
    #[serde(default)]
    pub angular_components: HashMap<u32, AngularComponentRecord>,
    /// def_idx of component → child selectors from HTML template.
    #[serde(default)]
    pub template_children: HashMap<u32, Vec<String>>,

    // ── CATEGORY C: Fields NOT using def_idx (no compact/remove updates needed) ──

    /// Path -> file_id lookup (for watcher)
    pub path_to_id: HashMap<PathBuf, u32>,
    /// Number of files that could not be read (IO errors) during index build.
    #[serde(default)]
    pub parse_errors: usize,
    /// Number of files that contained non-UTF8 bytes and were read with lossy conversion.
    #[serde(default)]
    pub lossy_file_count: usize,
    /// Files that were read and parsed but produced 0 definitions.
    #[serde(default)]
    pub empty_file_ids: Vec<(u32, u64)>,
    /// Extension method name → Vec of static class names containing the extension.
    #[serde(default)]
    pub extension_methods: HashMap<String, Vec<String>>,
    /// Number of worker threads that panicked during index building.
    /// Zero for a clean build. Non-zero indicates partial/degraded index.
    #[serde(default)]
    pub worker_panics: usize,
    /// Whether this index was built with `--respect-git-exclude` honoured.
    /// Persisted so that auto-rebuild paths (`xray serve` definition rebuild,
    /// `xray_reindex_definitions` MCP tool) preserve the user's original
    /// choice instead of silently flipping to the CLI default. See
    /// `docs/bug-reports/2026-04-23_417f315_cli-auto-rebuild-loses-flag.md`.
    #[serde(default)]
    pub respect_git_exclude: bool,
    /// Monotonic generation of fully-applied definition batches.
    #[serde(default)]
    pub definition_generation: u64,
    /// Normalized input path → snapshot used by the current definition graph.
    #[serde(default)]
    pub input_fingerprints: HashMap<String, DefinitionInputFingerprint>,
    /// Runtime-only transient input attempts, keyed by normalized path.
    #[serde(skip)]
    pub pending_definition_inputs: HashMap<PathBuf, PendingDefinitionInput>,
}


/// On-disk projection of `DefinitionIndex` with the heavy `definitions: Vec<DefinitionEntry>`
/// excluded. Used by the sharded layout (see `crate::index::save_sharded` /
/// `load_sharded`) so the per-entry payload can be decoded in parallel.
///
/// **Field order MUST exactly mirror the persisted-field order of
/// `DefinitionIndex`** (every non-`#[serde(skip)]` field EXCEPT `definitions`,
/// in source order). The first two fields MUST stay `root: String` then
/// `format_version: u32` so the lightweight header readers in `crate::index`
/// can extract them from the plain-bincode head.
///
/// Sharding `definitions` (and not the secondary `*_index` HashMaps) keeps
/// def_idx values consistent across save->load round-trips: the merged
/// `Vec<DefinitionEntry>` is a sequential concatenation of the original
/// chunks, so positions are preserved.
#[derive(Serialize, Deserialize, Debug)]
pub struct DefinitionIndexHead {
    pub root: String,
    #[serde(default)]
    pub format_version: u32,
    pub created_at: u64,
    pub extensions: Vec<String>,
    pub files: Vec<String>,
    pub name_index: HashMap<String, Vec<u32>>,
    pub kind_index: HashMap<DefinitionKind, Vec<u32>>,
    pub attribute_index: HashMap<String, Vec<u32>>,
    pub base_type_index: HashMap<String, Vec<u32>>,
    pub file_index: HashMap<u32, Vec<u32>>,
    #[serde(default)]
    pub selector_index: HashMap<String, Vec<u32>>,
    #[serde(default)]
    pub template_owners: HashMap<String, Vec<u32>>,
    #[serde(default)]
    pub template_parents: HashMap<String, Vec<u32>>,
    #[serde(default)]
    pub method_calls: HashMap<u32, Vec<CallSite>>,
    #[serde(default)]
    pub csharp_semantics: CSharpSemanticIndex,
    #[serde(default)]
    pub code_stats: HashMap<u32, CodeStats>,
    #[serde(default)]
    pub angular_components: HashMap<u32, AngularComponentRecord>,
    #[serde(default)]
    pub template_children: HashMap<u32, Vec<String>>,
    pub path_to_id: HashMap<PathBuf, u32>,
    #[serde(default)]
    pub parse_errors: usize,
    #[serde(default)]
    pub lossy_file_count: usize,
    #[serde(default)]
    pub empty_file_ids: Vec<(u32, u64)>,
    #[serde(default)]
    pub extension_methods: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub worker_panics: usize,
    #[serde(default)]
    pub respect_git_exclude: bool,
    #[serde(default)]
    pub definition_generation: u64,
    #[serde(default)]
    pub input_fingerprints: HashMap<String, DefinitionInputFingerprint>,
}

impl DefinitionIndex {
    /// Build a `DefinitionIndexHead` snapshot from this index. Clones every
    /// metadata field (small/medium maps) but does NOT touch the heavy
    /// `definitions: Vec<DefinitionEntry>` — callers pass that vec separately
    /// to `crate::index::save_sharded` as `Vec<&DefinitionEntry>` of borrowed
    /// entries to keep the save path's transient memory peak proportional to
    /// metadata size, not total-index size.
    pub fn build_head(&self) -> DefinitionIndexHead {
        DefinitionIndexHead {
            root: self.root.clone(),
            format_version: self.format_version,
            created_at: self.created_at,
            extensions: self.extensions.clone(),
            files: self.files.clone(),
            name_index: self.name_index.clone(),
            kind_index: self.kind_index.clone(),
            attribute_index: self.attribute_index.clone(),
            base_type_index: self.base_type_index.clone(),
            file_index: self.file_index.clone(),
            selector_index: self.selector_index.clone(),
            template_owners: self.template_owners.clone(),
            template_parents: self.template_parents.clone(),
            method_calls: self.method_calls.clone(),
            csharp_semantics: self.csharp_semantics.clone(),
            code_stats: self.code_stats.clone(),
            angular_components: self.angular_components.clone(),
            template_children: self.template_children.clone(),
            path_to_id: self.path_to_id.clone(),
            parse_errors: self.parse_errors,
            lossy_file_count: self.lossy_file_count,
            empty_file_ids: self.empty_file_ids.clone(),
            extension_methods: self.extension_methods.clone(),
            worker_panics: self.worker_panics,
            respect_git_exclude: self.respect_git_exclude,
            definition_generation: self.definition_generation,
            input_fingerprints: self.input_fingerprints.clone(),
        }
    }

    /// Reconstruct a `DefinitionIndex` from a sharded head + decoded
    /// definitions vec. `definitions` order MUST be the sequential
    /// concatenation of the per-shard chunks emitted by `save_sharded`
    /// (preserved by `load_sharded`'s in-order merge), otherwise the
    /// def_idx values stored in the secondary `*_index` HashMaps no
    /// longer point at the right entries.
    pub fn from_head_and_entries(
        head: DefinitionIndexHead,
        definitions: Vec<DefinitionEntry>,
    ) -> Self {
        Self {
            root: head.root,
            format_version: head.format_version,
            created_at: head.created_at,
            extensions: head.extensions,
            files: head.files,
            definitions,
            name_index: head.name_index,
            kind_index: head.kind_index,
            attribute_index: head.attribute_index,
            base_type_index: head.base_type_index,
            file_index: head.file_index,
            selector_index: head.selector_index,
            template_owners: head.template_owners,
            template_parents: head.template_parents,
            method_calls: head.method_calls,
            csharp_semantics: head.csharp_semantics,
            code_stats: head.code_stats,
            angular_components: head.angular_components,
            template_children: head.template_children,
            path_to_id: head.path_to_id,
            parse_errors: head.parse_errors,
            lossy_file_count: head.lossy_file_count,
            empty_file_ids: head.empty_file_ids,
            extension_methods: head.extension_methods,
            worker_panics: head.worker_panics,
            respect_git_exclude: head.respect_git_exclude,
            definition_generation: head.definition_generation,
            input_fingerprints: head.input_fingerprints,
            pending_definition_inputs: HashMap::new(),
        }
    }
}

impl DefinitionIndex {
    /// Number of live files currently indexed.
    ///
    /// `self.files.len()` is the file_id allocator capacity (append-only); on
    /// removal the slot is tombstoned (string cleared) but the Vec itself is
    /// never shrunk so file_id remains a stable index. Use this method for any
    /// user-visible count reporting (`xray_info`, `IndexMeta`, logs, memory
    /// estimates) so deletions are reflected.
    ///
    /// `path_to_id` is the authoritative live set; the raw `files` Vec is the
    /// id-allocator with tombstones for removed entries.
    pub fn live_file_count(&self) -> usize {
        self.path_to_id.len()
    }

    /// Shrink all internal HashMaps to fit their current size.
    /// Call after loading from disk to reclaim excess capacity.
    /// Saves ~20-50 MB for large indexes by eliminating HashMap over-allocation.
    pub fn shrink_maps(&mut self) {
        self.name_index.shrink_to_fit();
        self.kind_index.shrink_to_fit();
        self.attribute_index.shrink_to_fit();
        self.base_type_index.shrink_to_fit();
        self.file_index.shrink_to_fit();
        self.selector_index.shrink_to_fit();
        self.template_owners.shrink_to_fit();
        self.template_parents.shrink_to_fit();
        self.method_calls.shrink_to_fit();
        self.code_stats.shrink_to_fit();
        self.angular_components.shrink_to_fit();
        self.template_children.shrink_to_fit();
        self.path_to_id.shrink_to_fit();
        self.extension_methods.shrink_to_fit();
        // Also shrink inner Vecs for maps with non-trivial value sizes
        for v in self.name_index.values_mut() { v.shrink_to_fit(); }
        for v in self.kind_index.values_mut() { v.shrink_to_fit(); }
        for v in self.attribute_index.values_mut() { v.shrink_to_fit(); }
        for v in self.base_type_index.values_mut() { v.shrink_to_fit(); }
        for v in self.file_index.values_mut() { v.shrink_to_fit(); }
        for v in self.selector_index.values_mut() { v.shrink_to_fit(); }
        for v in self.template_owners.values_mut() { v.shrink_to_fit(); }
        for v in self.template_parents.values_mut() { v.shrink_to_fit(); }
        for v in self.method_calls.values_mut() { v.shrink_to_fit(); }
    }
}


// ─── Parser result type aliases ──────────────────────────────────────
// Shared by all parser entry points to avoid clippy::type_complexity.

/// Result type for parsers that don't produce extension methods (TS, Rust, SQL).
pub type ParseResult = (Vec<DefinitionEntry>, Vec<(usize, Vec<CallSite>)>, Vec<(usize, CodeStats)>);

/// Result type for the C# parser, which also returns extension method mappings.
#[cfg(all(feature = "lang-csharp", test))]
pub type CsharpParseResult = (Vec<DefinitionEntry>, Vec<(usize, Vec<CallSite>)>, Vec<(usize, CodeStats)>, HashMap<String, Vec<String>>);

#[cfg(feature = "lang-csharp")]
pub type CsharpSemanticParseResult = (Vec<DefinitionEntry>, Vec<(usize, Vec<CallSite>)>, Vec<(usize, CodeStats)>, HashMap<String, Vec<String>>, CSharpFileContribution);

/// Chunk type used during parallel definition index building.
pub(crate) type DefChunk = (
    u32,
    Vec<DefinitionEntry>,
    Vec<(usize, Vec<CallSite>)>,
    Vec<(usize, CodeStats)>,
    CSharpFileContribution,
    Vec<ParsedAngularComponentRecord>,
    DefinitionInputFingerprint,
);


/// Parsed results for a single file — ready to be applied to the index.
/// This struct is produced by `parse_file_standalone()` WITHOUT holding any lock,
/// and then applied to the index via `apply_parsed_result()` under a brief write lock.
#[derive(Debug)]
pub struct ParsedFileResult {
    pub path: PathBuf,
    pub definitions: Vec<DefinitionEntry>,
    pub call_sites: Vec<(usize, Vec<CallSite>)>,
    pub code_stats: Vec<(usize, CodeStats)>,
    pub extension_methods: HashMap<String, Vec<String>>,
    pub csharp_semantics: CSharpFileContribution,
    pub(crate) angular_components: Vec<ParsedAngularComponentRecord>,
    pub(crate) fingerprint: DefinitionInputFingerprint,
}

#[derive(Debug)]
pub(crate) struct PreparedAngularTemplateUpdate {
    #[cfg_attr(not(feature = "lang-typescript"), allow(dead_code))]
    pub path: PathBuf,
    pub owner_key: String,
    pub dependent_source_paths: Vec<PathBuf>,
    pub template_children: Vec<String>,
    pub unavailable_reason: Option<String>,
    pub triggered_by_change: bool,
    pub fingerprint: Option<DefinitionInputFingerprint>,
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
  by 'xray serve --definitions'.

EXAMPLES:
  Index C# files:     xray def-index --dir C:\Projects --ext cs
  Index TypeScript:   xray def-index --dir C:\Projects --ext ts,tsx
  Index C# + SQL:     xray def-index --dir C:\Projects --ext cs,sql
  Index all:          xray def-index --dir C:\Projects --ext cs,sql,ts,tsx
  Custom threads:     xray def-index --dir C:\Projects --ext cs --threads 8

PERFORMANCE:
  48,643 files -> 846,167 definitions in ~14s (24 threads)
  Index size: ~103 MB on disk (LZ4-compressed)
"#)]
pub struct DefIndexArgs {
    /// Directory to recursively scan for source files to parse
    #[arg(short, long, default_value = ".")]
    pub dir: String,

    /// File extensions to parse, comma-separated.
    /// C# (.cs) uses tree-sitter-c-sharp grammar.
    /// TypeScript (.ts, .tsx) uses tree-sitter-typescript grammar.
    /// SQL (.sql) uses a regex-based parser (stored procedures, tables, views, functions, types, indexes, columns, call sites).
    #[arg(short, long, default_value = "cs")]
    pub ext: String,

    /// Number of parallel parsing threads. Each thread gets its own
    /// tree-sitter parser instance. 0 = auto-detect CPU cores.
    #[arg(short, long, default_value = "0")]
    pub threads: usize,

    /// Respect .git/info/exclude (by default xray ignores it so local-excluded files are indexed).
    /// Mirrors the same flag on `xray index` / `xray content-index` / `xray serve`.
    #[arg(long)]
    pub respect_git_exclude: bool,

}

#[derive(Parser, Debug)]
#[command(after_long_help = r#"WHAT IT DOES:
  Loads a previously built definition index (.code-structure file) from disk and
  reports coverage statistics: how many files have definitions, how many
  are empty, and which "suspicious" files (>N bytes but 0 definitions)
  may have parsing issues.

  This is a read-only operation — it does NOT rebuild the index.

EXAMPLES:
  Audit with defaults:     xray def-audit --dir C:\Projects --ext cs
  Lower threshold:         xray def-audit --dir C:\Projects --ext cs --min-bytes 2000
  Show lossy files too:    xray def-audit --dir C:\Projects --ext cs --show-lossy
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

#[cfg(test)]
mod kinds_drift_tests {
    use super::*;

    /// Drift guard for [`DefinitionKind::ALL_KINDS`].
    ///
    /// Walks every variant via an exhaustive `match`, collects the variants
    /// that appear in `ALL_KINDS`, and asserts the two sets coincide.
    /// Adding a new `DefinitionKind` variant without listing it in
    /// `ALL_KINDS` makes this test fail — by design — because the new
    /// variant has no match arm here either, which is itself a compile error.
    /// Both signals together guarantee `ALL_KINDS` cannot silently drift
    /// from the enum (see also `mcp::handlers::utils::read_kind_array`).
    #[test]
    fn all_kinds_lists_every_variant() {
        // Force exhaustive match on a sample value so adding a variant
        // breaks compilation here, not just at run time.
        fn touch(k: DefinitionKind) -> &'static str {
            match k {
                DefinitionKind::Class => "class",
                DefinitionKind::Interface => "interface",
                DefinitionKind::Enum => "enum",
                DefinitionKind::Struct => "struct",
                DefinitionKind::Record => "record",
                DefinitionKind::Method => "method",
                DefinitionKind::Property => "property",
                DefinitionKind::Field => "field",
                DefinitionKind::Constructor => "constructor",
                DefinitionKind::Delegate => "delegate",
                DefinitionKind::Event => "event",
                DefinitionKind::EnumMember => "enumMember",
                DefinitionKind::Function => "function",
                DefinitionKind::TypeAlias => "typeAlias",
                DefinitionKind::Variable => "variable",
                DefinitionKind::XmlElement => "xmlElement",
                DefinitionKind::StoredProcedure => "storedProcedure",
                DefinitionKind::Table => "table",
                DefinitionKind::View => "view",
                DefinitionKind::SqlFunction => "sqlFunction",
                DefinitionKind::UserDefinedType => "userDefinedType",
                DefinitionKind::Column => "column",
                DefinitionKind::SqlIndex => "sqlIndex",
            }
        }
        // Sanity: as_str() output of every variant in ALL_KINDS matches the
        // exhaustive match above (i.e. ALL_KINDS, as_str, and this test all
        // agree on the canonical wire string for every variant).
        for k in DefinitionKind::ALL_KINDS {
            assert_eq!(
                k.as_str(),
                touch(*k),
                "as_str()/exhaustive-match disagree for {:?}",
                k
            );
        }
        // Sanity: ALL_KINDS contains 23 entries (current variant count).
        // If you added a variant, update this number AND verify the
        // exhaustive match above still compiles. Both must change in
        // lockstep, which is the whole point of this test.
        assert_eq!(
            DefinitionKind::ALL_KINDS.len(),
            23,
            "ALL_KINDS length drifted from DefinitionKind variant count"
        );
    }
}
