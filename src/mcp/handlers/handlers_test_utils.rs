//! Shared test helpers for MCP handler tests.
//! Contains functions used by multiple test modules (handlers_tests, handlers_tests_csharp, etc.)

use super::*;
use crate::definitions::*;
use crate::Posting;
use crate::TrigramIndex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Remove a temporary directory used in tests.
pub(crate) fn cleanup_tmp(tmp_dir: &std::path::Path) {
    let _ = std::fs::remove_dir_all(tmp_dir);
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
        server_dir: ".".to_string(),
        server_ext: "cs".to_string(),
        metrics: false,
        index_base: PathBuf::from("."),
        max_response_bytes: crate::mcp::handlers::utils::DEFAULT_MAX_RESPONSE_BYTES,
        content_ready: Arc::new(AtomicBool::new(true)),
        def_ready: Arc::new(AtomicBool::new(true)),
        git_cache: Arc::new(RwLock::new(None)),
        git_cache_ready: Arc::new(AtomicBool::new(false)),
        current_branch: None,
    }
}