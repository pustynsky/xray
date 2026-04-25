//! Shared test helpers for MCP handler tests.
//!
//! **Convention:** new tests in this module hierarchy SHOULD use
//! [`HandlerContextBuilder`] for constructing [`HandlerContext`] and
//! [`make_params_default`] for constructing [`GrepSearchParams`].
//! Old tests are migrated opportunistically (Boy Scout rule).
//!
//! Canonical demos:
//! - [`HandlerContextBuilder`] usage: see `test_response_truncation_triggers_on_large_result`
//!   in `handlers_tests_grep.rs`.
//! - [`make_params_default`] usage: see tests in `grep_tests_additional.rs`.

use super::*;
use super::grep::GrepSearchParams;
use crate::definitions::*;
use crate::Posting;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Instant;

/// Remove a temporary directory used in tests.
pub(crate) fn cleanup_tmp(tmp_dir: &std::path::Path) {
    let _ = std::fs::remove_dir_all(tmp_dir);
}

/// Create a HandlerContext with empty/default indexes.
pub(crate) fn make_empty_ctx() -> HandlerContext {
    HandlerContext::default()
}

/// Helper: create a context with both content + definition indexes (C# classes/methods).
/// Used by general handler tests and C#-specific tests.
pub(crate) fn make_ctx_with_defs() -> HandlerContext {
    // Content index: tokens -> files+lines
    let mut content_idx = HashMap::new();
    content_idx.insert("executequeryasync".to_string(), vec![
        Posting { file_id: 0, lines: vec![242] },
        Posting { file_id: 1, lines: vec![88] },
        Posting { file_id: 2, lines: vec![391] },
    ]);
    content_idx.insert("queryinternalasync".to_string(), vec![
        Posting { file_id: 2, lines: vec![766] },
        Posting { file_id: 2, lines: vec![462] },
    ]);
    content_idx.insert("proxyclient".to_string(), vec![
        Posting { file_id: 1, lines: vec![1, 88] },
    ]);
    content_idx.insert("resilientclient".to_string(), vec![
        Posting { file_id: 0, lines: vec![1, 242] },
    ]);
    content_idx.insert("queryservice".to_string(), vec![
        Posting { file_id: 2, lines: vec![1, 391, 462, 766] },
    ]);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "C:\\src\\ResilientClient.cs".to_string(),
            "C:\\src\\ProxyClient.cs".to_string(),
            "C:\\src\\QueryService.cs".to_string(),
        ],
        index: content_idx,
        total_tokens: 500,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![100, 50, 200],
        ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "ResilientClient".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 300,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "ExecuteQueryAsync".to_string(),
            kind: DefinitionKind::Method, line_start: 240, line_end: 260,
            parent: Some("ResilientClient".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "ProxyClient".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 100,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "ExecuteQueryAsync".to_string(),
            kind: DefinitionKind::Method, line_start: 85, line_end: 95,
            parent: Some("ProxyClient".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 2, name: "QueryService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 900,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 2, name: "RunQueryBatchAsync".to_string(),
            kind: DefinitionKind::Method, line_start: 386, line_end: 395,
            parent: Some("QueryService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 2, name: "QueryImplAsync".to_string(),
            kind: DefinitionKind::Method, line_start: 450, line_end: 470,
            parent: Some("QueryService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 2, name: "QueryInternalAsync".to_string(),
            kind: DefinitionKind::Method, line_start: 760, line_end: 830,
            parent: Some("QueryService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();

    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }

    path_to_id.insert(PathBuf::from("C:\\src\\ResilientClient.cs"), 0);
    path_to_id.insert(PathBuf::from("C:\\src\\ProxyClient.cs"), 1);
    path_to_id.insert(PathBuf::from("C:\\src\\QueryService.cs"), 2);

    let def_index = DefinitionIndex {
        root: ".".to_string(),
        created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec![
            "C:\\src\\ResilientClient.cs".to_string(),
            "C:\\src\\ProxyClient.cs".to_string(),
            "C:\\src\\QueryService.cs".to_string(),
        ],
        definitions,
        name_index,
        kind_index,
        attribute_index: HashMap::new(),
        base_type_index: HashMap::new(),
        file_index,
        path_to_id,
        ..Default::default()
    };

    HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    }
}

/// Builder for `HandlerContext` — eliminates repetitive
/// `Arc::new(RwLock::new(...))` wrapping in tests.
///
/// # Usage
/// ```ignore
/// let ctx = HandlerContextBuilder::new()
///     .with_content_index(my_index)
///     .with_metrics(true)
///     .build();
/// ```
#[allow(dead_code)] // Methods used incrementally as tests are Boy-Scout-migrated.
pub(crate) struct HandlerContextBuilder {
    ctx: HandlerContext,
}

#[allow(dead_code)] // Methods used incrementally as tests are Boy-Scout-migrated.
impl HandlerContextBuilder {
    pub fn new() -> Self {
        Self { ctx: HandlerContext::default() }
    }

    pub fn with_content_index(mut self, index: ContentIndex) -> Self {
        self.ctx.index = Arc::new(RwLock::new(index));
        self
    }

    pub fn with_def_index(mut self, index: DefinitionIndex) -> Self {
        self.ctx.def_index = Some(Arc::new(RwLock::new(index)));
        self
    }

    pub fn with_workspace(mut self, binding: WorkspaceBinding) -> Self {
        self.ctx.workspace = Arc::new(RwLock::new(binding));
        self
    }

    pub fn with_server_dir(mut self, dir: impl Into<String>) -> Self {
        self.ctx.workspace = Arc::new(RwLock::new(
            WorkspaceBinding::pinned(dir.into()),
        ));
        self
    }

    pub fn with_server_ext(mut self, ext: impl Into<String>) -> Self {
        self.ctx.server_ext = ext.into();
        self
    }

    pub fn with_metrics(mut self, enabled: bool) -> Self {
        self.ctx.metrics = enabled;
        self
    }

    pub fn with_index_base(mut self, path: PathBuf) -> Self {
        self.ctx.index_base = path;
        self
    }

    pub fn with_max_response_bytes(mut self, bytes: usize) -> Self {
        self.ctx.max_response_bytes = bytes;
        self
    }

    pub fn with_current_branch(mut self, branch: impl Into<String>) -> Self {
        self.ctx.current_branch = Some(branch.into());
        self
    }

    pub fn build(self) -> HandlerContext {
        self.ctx
    }
}

/// Default `GrepSearchParams` for tests. Override individual fields via
/// `GrepSearchParams { field: value, ..make_params_default() }`.
pub(crate) fn make_params_default<'a>() -> GrepSearchParams<'a> {
    GrepSearchParams {
        ext_filter: &[],
        show_lines: false,
        context_lines: 0,
        max_results: 50,
        mode_and: false,
        count_only: false,
        search_start: Instant::now(),
        dir_filter: &None,
        file_filter: &[],
        exclude_patterns: super::utils::ExcludePatterns::from_dirs(&[]),
        exclude_lower: vec![],
        dir_auto_converted_note: None,
        exact_file_path: &None,
        exact_file_path_canonical: &None,
        auto_balance: true,
        max_occurrences_per_term: None,
    }
}
