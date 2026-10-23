#![allow(clippy::field_reassign_with_default)] // tests prefer mutate-after-default for readability
use super::*;
use crate::definitions::DefinitionIndex;

fn cache_467_definition_fixture(root: &str, generation: char, count: u32) -> DefinitionIndex {
    use crate::definitions::{DefinitionEntry, DefinitionKind, DEFINITION_INDEX_VERSION};
    let mut index = DefinitionIndex {
        root: root.to_string(),
        format_version: DEFINITION_INDEX_VERSION,
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };
    for i in 0..count {
        let name = format!("{generation}{i:03}");
        let path = format!("{generation}{i:03}.cs");
        index.files.push(path.clone());
        index.path_to_id.insert(std::path::PathBuf::from(path), i);
        index.definitions.push(DefinitionEntry {
            file_id: i, name: name.clone(), kind: DefinitionKind::Class,
            line_start: i + 1, line_end: i + 2, parent: None, signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        });
        index.name_index.insert(name, vec![i]);
        index.kind_index.entry(DefinitionKind::Class).or_default().push(i);
        index.file_index.insert(i, vec![i]);
    }
    index
}

fn cache_467_assert_definitions(index: &DefinitionIndex, generation: char, count: u32) {
    assert_eq!(index.format_version, crate::definitions::DEFINITION_INDEX_VERSION);
    assert_eq!(index.files.len(), count as usize);
    assert_eq!(index.definitions.len(), count as usize);
    for i in 0..count {
        let name = format!("{generation}{i:03}");
        let entry = &index.definitions[i as usize];
        assert_eq!(entry.name, name);
        assert_eq!(entry.file_id, i);
        assert_eq!(entry.line_start, i + 1);
        assert_eq!(index.files[entry.file_id as usize], format!("{generation}{i:03}.cs"));
        assert_eq!(index.name_index.get(&name), Some(&vec![i]));
        assert_eq!(index.file_index.get(&i), Some(&vec![i]));
        assert_eq!(index.path_to_id.get(&std::path::PathBuf::from(&index.files[i as usize])), Some(&i));
    }
    if count > 0 {
        assert_eq!(index.kind_index[&crate::definitions::DefinitionKind::Class], (0..count).collect::<Vec<_>>());
    }
}

fn cache_467_definition_replacement(stage: crate::index::cache_load_test_hooks::Stage) {
    use crate::index::cache_load_test_hooks as hooks;
    let tmp = tempfile::tempdir().unwrap();
    let replacement_dir = tempfile::tempdir().unwrap();
    let root = tmp.path().to_str().unwrap();
    let a = cache_467_definition_fixture(root, 'a', 12);
    let mut b = cache_467_definition_fixture(root, 'b', 12);
    if stage == hooks::Stage::Version {
        b.format_version += 1;
    }
    save_definition_index(&a, tmp.path()).unwrap();
    save_definition_index(&b, replacement_dir.path()).unwrap();
    let path = definition_index_path_for(root, "cs", tmp.path());
    let replacement = definition_index_path_for(root, "cs", replacement_dir.path());
    assert_eq!(std::fs::metadata(&path).unwrap().len(), std::fs::metadata(&replacement).unwrap().len());
    let target = path.clone();
    let swapped = std::rc::Rc::new(std::cell::Cell::new(false));
    let observed = swapped.clone();
    let _guard = hooks::on(&path, stage, move || {
        hooks::replace(&target, &replacement).unwrap();
        observed.set(true);
    });
    let loaded = load_definition_index(root, "cs", tmp.path()).unwrap();
    assert!(swapped.get(), "replacement hook did not run");
    cache_467_assert_definitions(&loaded, 'a', 12);
    let fresh = load_definition_index(root, "cs", tmp.path());
    if stage == hooks::Stage::Version {
        assert!(matches!(fresh, Err(crate::SearchError::IndexLoad { message, .. }) if message.contains("format version mismatch")));
    } else {
        cache_467_assert_definitions(&fresh.unwrap(), 'b', 12);
    }
}

#[test]
fn cache_467_definition_header_replacement() {
    cache_467_definition_replacement(crate::index::cache_load_test_hooks::Stage::Header);
}

#[test]
fn cache_467_definition_version_replacement() {
    cache_467_definition_replacement(crate::index::cache_load_test_hooks::Stage::Version);
}

#[test]
fn cache_467_definition_magic_replacement() {
    cache_467_definition_replacement(crate::index::cache_load_test_hooks::Stage::Magic);
}

#[test]
fn cache_467_definition_unchanged_multishard() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_str().unwrap();
    for count in [0, 1, 3, 12, 4096] {
        let index = cache_467_definition_fixture(root, 'a', count);
        save_definition_index(&index, tmp.path()).unwrap();
        cache_467_assert_definitions(&load_definition_index(root, "cs", tmp.path()).unwrap(), 'a', count);
    }
}

#[test]
fn cache_467_definition_legacy_replacement_and_controls() {
    use crate::index::{cache_load_test_hooks as hooks, save_compressed};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_str().unwrap();
    let path = definition_index_path_for(root, "cs", tmp.path());
    for compressed in [false, true] {
        for stage in [hooks::Stage::Version, hooks::Stage::Magic] {
            let a = cache_467_definition_fixture(root, 'a', 12);
            let b = cache_467_definition_fixture(root, 'b', 12);
            if compressed {
                save_compressed(&path, &a, "definition-index").unwrap();
            } else {
                std::fs::write(&path, bincode::serialize(&a).unwrap()).unwrap();
            }
            cache_467_assert_definitions(&load_definition_index(root, "cs", tmp.path()).unwrap(), 'a', 12);
            let replacement = tmp.path().join("replacement");
            crate::index::save_sharded(&replacement, &b.build_head(), b.definitions.iter().collect(), "definition-index").unwrap();
            let target = path.clone();
            let swapped = std::rc::Rc::new(std::cell::Cell::new(false));
            let observed = swapped.clone();
            let _guard = hooks::on(&path, stage, move || {
                hooks::replace(&target, &replacement).unwrap();
                observed.set(true);
            });
            cache_467_assert_definitions(&load_definition_index(root, "cs", tmp.path()).unwrap(), 'a', 12);
            assert!(swapped.get());
            cache_467_assert_definitions(&load_definition_index(root, "cs", tmp.path()).unwrap(), 'b', 12);
        }
    }
}

#[test]
fn cache_467_definition_corruption_is_explicit() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_str().unwrap();
    let path = definition_index_path_for(root, "cs", tmp.path());
    save_definition_index(&cache_467_definition_fixture(root, 'a', 12), tmp.path()).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    for (case, corrupt) in crate::index::cache_load_test_hooks::corrupt_sharded(&bytes) {
        std::fs::write(&path, corrupt).unwrap();
        let result = load_definition_index(root, "cs", tmp.path());
        assert!(matches!(result, Err(crate::SearchError::IndexLoad { .. })), "{case}: {result:?}");
    }
}

/// Create a minimal DefinitionIndex with given root and extensions, save to disk.
fn save_test_def_index(root: &str, exts: &[&str], index_base: &std::path::Path) {
    let idx = DefinitionIndex {
        root: root.to_string(),
        format_version: crate::definitions::DEFINITION_INDEX_VERSION,
        extensions: exts.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    save_definition_index(&idx, index_base).unwrap();
}

#[test]
fn test_find_def_index_skips_stale_extensions() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    // Create a real directory for the root (canonicalize needs it)
    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = clean_path(&root_dir.to_string_lossy());

    // Save an index with only "cs" extension
    save_test_def_index(&root_str, &["cs"], index_base);

    // Request "cs,sql" — should NOT find the old cs-only index
    let expected = vec!["cs".to_string(), "sql".to_string()];
    let result = find_definition_index_for_dir(&root_str, index_base, &expected);
    assert!(result.is_none(),
        "Should not find cs-only index when cs,sql is expected");
}

#[test]
fn test_find_def_index_accepts_superset() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = clean_path(&root_dir.to_string_lossy());

    // Save an index with "cs,sql,ts" extensions
    save_test_def_index(&root_str, &["cs", "sql", "ts"], index_base);

    // Request "cs,sql" — should find the superset index
    let expected = vec!["cs".to_string(), "sql".to_string()];
    let result = find_definition_index_for_dir(&root_str, index_base, &expected);
    assert!(result.is_some(),
        "Should find cs,sql,ts index when cs,sql is expected (superset)");
}

#[test]
fn test_find_def_index_accepts_exact_match() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = clean_path(&root_dir.to_string_lossy());

    // Save an index with "cs,sql" extensions
    save_test_def_index(&root_str, &["cs", "sql"], index_base);

    // Request "cs,sql" — should find the exact match
    let expected = vec!["cs".to_string(), "sql".to_string()];
    let result = find_definition_index_for_dir(&root_str, index_base, &expected);
    assert!(result.is_some(),
        "Should find cs,sql index when cs,sql is expected (exact match)");
}

#[test]
fn test_find_def_index_empty_expected_accepts_any() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = clean_path(&root_dir.to_string_lossy());

    // Save an index with "cs" extension
    save_test_def_index(&root_str, &["cs"], index_base);

    // Request with empty expected — should accept any
    let result = find_definition_index_for_dir(&root_str, index_base, &[]);
    assert!(result.is_some(),
        "Empty expected_exts should accept any cached index (backward compatible)");
}

#[test]
fn test_find_def_index_case_insensitive_ext_match() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = clean_path(&root_dir.to_string_lossy());

    // Save an index with uppercase extensions
    save_test_def_index(&root_str, &["CS", "SQL"], index_base);

    // Request lowercase — should still match
    let expected = vec!["cs".to_string(), "sql".to_string()];
    let result = find_definition_index_for_dir(&root_str, index_base, &expected);
    assert!(result.is_some(),
        "Extension matching should be case-insensitive");
}

// ─── find_definition_index_for_dir meta-based optimization tests ─────

/// Verify that find_definition_index_for_dir skips non-matching indexes
/// without loading the full index when .meta sidecar files are present.
#[test]
fn test_find_def_index_uses_meta_to_skip_non_matching_root() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let dir_a = tmp.path().join("project_a");
    let dir_b = tmp.path().join("project_b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();

    let root_a = clean_path(&dir_a.to_string_lossy());
    let root_b = clean_path(&dir_b.to_string_lossy());

    // Save def index for project_a
    save_test_def_index(&root_a, &["rs"], index_base);

    // Searching for project_b should NOT find project_a's index
    let result = find_definition_index_for_dir(&root_b, index_base, &[]);
    assert!(result.is_none(),
        "Should not find project_a's index when searching for project_b");

    // Searching for project_a SHOULD find it
    let result = find_definition_index_for_dir(&root_a, index_base, &[]);
    assert!(result.is_some(),
        "Should find project_a's index when searching for project_a");
}

/// Verify that find_definition_index_for_dir works when .meta file is missing.
#[test]
fn test_find_def_index_works_without_meta_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = clean_path(&root_dir.to_string_lossy());

    // Save def index (creates both .code-structure and .code-structure.meta)
    save_test_def_index(&root_str, &["rs", "md"], index_base);

    // Delete the .meta sidecar file
    for entry in std::fs::read_dir(index_base).unwrap().flatten() {
        let path = entry.path();
        if path.to_string_lossy().ends_with(".meta") {
            std::fs::remove_file(&path).unwrap();
        }
    }

    // Should still find the index via fallback
    let expected = vec!["rs".to_string()];
    let result = find_definition_index_for_dir(&root_str, index_base, &expected);
    assert!(result.is_some(),
        "Should find index even without .meta sidecar (fallback path)");
}

/// Verify that meta-based filtering correctly rejects extension mismatches.
#[test]
fn test_find_def_index_meta_rejects_extension_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = clean_path(&root_dir.to_string_lossy());

    // Save def index with only "rs" extension
    save_test_def_index(&root_str, &["rs"], index_base);

    // Request "rs,sql" — meta should reject because "sql" is not in cached extensions
    let expected = vec!["rs".to_string(), "sql".to_string()];
    let result = find_definition_index_for_dir(&root_str, index_base, &expected);
    assert!(result.is_none(),
        "Meta-based filtering should reject when cached extensions don't include all expected");
}

// ─── Definition index format_version tests ──────────────────────────

#[test]
fn test_definition_index_version_is_pinned() {
    assert_eq!(crate::definitions::DEFINITION_INDEX_VERSION, 11);
}

#[test]
fn test_def_index_format_version_correct_loads_ok() {
    use crate::definitions::DEFINITION_INDEX_VERSION;
    let tmp = tempfile::tempdir().unwrap();

    let root = tmp.path().to_string_lossy().to_string();
    let mut idx = crate::definitions::DefinitionIndex::default();
    idx.format_version = DEFINITION_INDEX_VERSION;
    idx.root = root.clone();
    idx.extensions = vec!["rs".to_string()];

    let path = definition_index_path_for(&root, "rs", tmp.path());
    crate::index::save_compressed(&path, &idx, "test").unwrap();

    let result = load_definition_index(&root, "rs", tmp.path());
    assert!(result.is_ok(), "Loading definition index with correct version should succeed");
    assert_eq!(result.unwrap().format_version, DEFINITION_INDEX_VERSION);
}

#[test]
fn test_typescript_lexical_call_kinds_survive_storage_roundtrip() {
    use crate::definitions::{CallSite, CallSiteKind, DEFINITION_INDEX_VERSION};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_string_lossy().to_string();
    let mut index = DefinitionIndex::default();
    index.format_version = DEFINITION_INDEX_VERSION;
    index.root = root.clone();
    index.extensions = vec!["ts".to_string()];
    index.method_calls.insert(
        0,
        vec![
            CallSite {
                method_name: "local".to_string(),
                receiver_type: None,
                line: 7,
                call_kind: CallSiteKind::TypeScriptLocalCallable { definition_line: 3 },
                receiver_is_generic: false,
            },
            CallSite {
                method_name: "same_file".to_string(),
                receiver_type: None,
                line: 8,
                call_kind: CallSiteKind::TypeScriptSameFile { definition_line: 2 },
                receiver_is_generic: false,
            },
            CallSite {
                method_name: "member".to_string(),
                receiver_type: Some("Owner".to_string()),
                line: 9,
                call_kind: CallSiteKind::TypeScriptMember,
                receiver_is_generic: false,
            },
            CallSite {
                method_name: "analysis_incomplete".to_string(),
                receiver_type: None,
                line: 10,
                call_kind: CallSiteKind::TypeScriptAnalysisIncomplete,
                receiver_is_generic: false,
            },
            CallSite {
                method_name: "callback".to_string(),
                receiver_type: None,
                line: 11,
                call_kind: CallSiteKind::TypeScriptDynamicCallableParameter,
                receiver_is_generic: false,
            },
            CallSite {
                method_name: "reassigned".to_string(),
                receiver_type: None,
                line: 9,
                call_kind: CallSiteKind::TypeScriptUnresolvedLocal,
                receiver_is_generic: false,
            },
            CallSite {
                method_name: "imported".to_string(),
                receiver_type: None,
                line: 10,
                call_kind: CallSiteKind::TypeScriptImported,
                receiver_is_generic: false,
            },
            CallSite {
                method_name: "unknown".to_string(),
                receiver_type: None,
                line: 11,
                call_kind: CallSiteKind::TypeScriptUnknownGlobal,
                receiver_is_generic: false,
            },
        ],
    );

    let path = definition_index_path_for(&root, "ts", tmp.path());
    crate::index::save_compressed(&path, &index, "test").unwrap();
    let loaded = load_definition_index(&root, "ts", tmp.path()).unwrap();
    let calls = &loaded.method_calls[&0];
    assert_eq!(
        calls[0].call_kind,
        CallSiteKind::TypeScriptLocalCallable { definition_line: 3 }
    );
    assert_eq!(
        calls[1].call_kind,
        CallSiteKind::TypeScriptSameFile { definition_line: 2 }
    );
    assert_eq!(calls[2].call_kind, CallSiteKind::TypeScriptMember);
    assert_eq!(
        calls[3].call_kind,
        CallSiteKind::TypeScriptAnalysisIncomplete
    );
    assert_eq!(
        calls[4].call_kind,
        CallSiteKind::TypeScriptDynamicCallableParameter
    );
    assert_eq!(calls[5].call_kind, CallSiteKind::TypeScriptUnresolvedLocal);
    assert_eq!(calls[6].call_kind, CallSiteKind::TypeScriptImported);
    assert_eq!(calls[7].call_kind, CallSiteKind::TypeScriptUnknownGlobal);
}

#[test]
fn test_angular_template_source_variants_survive_storage_roundtrip() {
    use crate::definitions::{
        AngularComponentRecord, AngularTemplateSource, DefinitionInputFingerprint,
        StaticValue, DEFINITION_INDEX_VERSION,
    };

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_string_lossy().to_string();
    let mut index = DefinitionIndex::default();
    index.format_version = DEFINITION_INDEX_VERSION;
    index.root = root.clone();
    index.extensions = vec!["ts".to_string()];
    index.angular_components.extend([
        (
            7,
            AngularComponentRecord {
                selector: StaticValue::Static("app-a".to_string()),
                template: AngularTemplateSource::UnavailableExternal {
                    relative_path: "./template-a.html".to_string(),
                    reason: "external template could not be read".to_string(),
                },
            },
        ),
        (
            8,
            AngularComponentRecord {
                selector: StaticValue::Static("app-b".to_string()),
                template: AngularTemplateSource::External {
                    relative_path: "./template-b.html".to_string(),
                },
            },
        ),
        (
            9,
            AngularComponentRecord {
                selector: StaticValue::Static("app-c".to_string()),
                template: AngularTemplateSource::Inline {
                    content: "<app-a></app-a>".to_string(),
                },
            },
        ),
        (
            10,
            AngularComponentRecord {
                selector: StaticValue::Static("app-d".to_string()),
                template: AngularTemplateSource::Dynamic {
                    reason: "template expression is not static".to_string(),
                },
            },
        ),
        (
            11,
            AngularComponentRecord {
                selector: StaticValue::Static("app-e".to_string()),
                template: AngularTemplateSource::Missing,
            },
        ),
    ]);
    index.selector_index.extend([
        ("app-a".to_string(), vec![7]),
        ("app-b".to_string(), vec![8]),
        ("app-c".to_string(), vec![9]),
        ("app-d".to_string(), vec![10]),
        ("app-e".to_string(), vec![11]),
    ]);
    index.template_children.insert(8, vec!["app-c".to_string()]);
    index.template_children.insert(9, vec!["app-a".to_string()]);
    index
        .template_owners
        .insert("q:/repo/template-a.html".to_string(), vec![7]);
    index
        .template_owners
        .insert("q:/repo/template-b.html".to_string(), vec![8]);
    index
        .template_parents
        .insert("app-a".to_string(), vec![9]);
    index
        .template_parents
        .insert("app-c".to_string(), vec![8]);
    index.definition_generation = 7;
    index.input_fingerprints.insert(
        "q:/repo/template-b.html".to_string(),
        DefinitionInputFingerprint {
            size: 42,
            modified_nanos: 123,
            content_hash: [9; 32],
        },
    );
    index.pending_definition_inputs.insert(
        std::path::PathBuf::from("q:/repo/busy.ts"),
        crate::definitions::PendingDefinitionInput {
            attempts: 2,
            observed_revision: None,
        },
    );

    let path = definition_index_path_for(&root, "ts", temp.path());
    crate::index::save_compressed(&path, &index, "test").unwrap();
    let loaded = load_definition_index(&root, "ts", temp.path()).unwrap();

    assert_eq!(loaded.angular_components, index.angular_components);
    assert_eq!(loaded.selector_index, index.selector_index);
    assert_eq!(loaded.template_children, index.template_children);
    assert_eq!(loaded.template_owners, index.template_owners);
    assert_eq!(loaded.template_parents, index.template_parents);
    assert_eq!(loaded.definition_generation, index.definition_generation);
    assert_eq!(loaded.input_fingerprints, index.input_fingerprints);
    assert!(loaded.pending_definition_inputs.is_empty());

    let rebuilt = DefinitionIndex::from_head_and_entries(index.build_head(), Vec::new());
    assert_eq!(rebuilt.angular_components, index.angular_components);
    assert_eq!(rebuilt.selector_index, index.selector_index);
    assert_eq!(rebuilt.template_children, index.template_children);
    assert_eq!(rebuilt.template_owners, index.template_owners);
    assert_eq!(rebuilt.template_parents, index.template_parents);
    assert_eq!(rebuilt.definition_generation, index.definition_generation);
    assert_eq!(rebuilt.input_fingerprints, index.input_fingerprints);
    assert!(rebuilt.pending_definition_inputs.is_empty());
}


#[test]
fn test_def_index_format_version_mismatch_returns_err() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_string_lossy().to_string();
    let mut idx = crate::definitions::DefinitionIndex::default();
    idx.format_version = crate::definitions::DEFINITION_INDEX_VERSION - 1;
    idx.root = root.clone();
    idx.extensions = vec!["rs".to_string()];

    let path = definition_index_path_for(&root, "rs", tmp.path());
    crate::index::save_compressed(&path, &idx, "test").unwrap();

    let result = load_definition_index(&root, "rs", tmp.path());
    assert!(result.is_err(), "Loading definition index with wrong version should fail");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("format version mismatch"), "Error should mention version mismatch, got: {}", err);
}

#[test]
fn test_def_index_format_version_legacy_zero_returns_err() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_string_lossy().to_string();
    let mut idx = crate::definitions::DefinitionIndex::default();
    idx.format_version = 0; // legacy
    idx.root = root.clone();
    idx.extensions = vec!["rs".to_string()];

    let path = definition_index_path_for(&root, "rs", tmp.path());
    crate::index::save_compressed(&path, &idx, "test").unwrap();

    let result = load_definition_index(&root, "rs", tmp.path());
    assert!(result.is_err(), "Loading legacy definition index (version 0) should fail");
}

// ─── DEF-S-001 / DEF-S-002 hardening regressions (2026-04-22) ──────────────────

#[test]
fn test_save_compressed_uses_unique_temp_filename() {
    // DEF-S-001 regression: temp filename must include pid + nanos + counter so
    // two concurrent saves cannot collide on a fixed `<file>.tmp` path. We can't
    // race two saves in a unit test, but we can verify the legacy deterministic
    // name is gone and saving twice in a row leaves no staging artifacts.
    let tmp = tempfile::tempdir().unwrap();
    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = clean_path(&root_dir.to_string_lossy());

    save_test_def_index(&root_str, &["cs"], tmp.path());
    save_test_def_index(&root_str, &["cs"], tmp.path());

    // No leftover legacy `<file>.tmp` literal — pre-fix the temp name was
    // `<path>.tmp`. The new pattern is `.<file>.xray_tmp.<pid>.<nanos>.<counter>`.
    let entries: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    let stale_legacy: Vec<_> = entries.iter()
        .filter(|n| n.ends_with(".code-structure.tmp"))
        .collect();
    assert!(
        stale_legacy.is_empty(),
        "legacy `.tmp` filename pattern leaked: {stale_legacy:?}"
    );
    let stale_xray_tmp: Vec<_> = entries.iter()
        .filter(|n| n.contains(".xray_tmp."))
        .collect();
    assert!(
        stale_xray_tmp.is_empty(),
        "unexpected xray_tmp staging files left behind: {stale_xray_tmp:?}"
    );
}

#[test]
fn test_remove_file_definitions_clears_empty_file_ids() {
    // DEF-S-002 regression: a file that was once empty but later acquires
    // definitions must not retain its (file_id, size) tuple in `empty_file_ids`.
    // Pre-fix this entry was kept forever, inflating audit reports and the
    // on-disk index size.
    use crate::definitions::incremental::remove_file_definitions;

    let mut idx = DefinitionIndex {
        root: "/test".to_string(),
        format_version: crate::definitions::DEFINITION_INDEX_VERSION,
        ..Default::default()
    };
    idx.files.push("/test/foo.cs".to_string());
    idx.files.push("/test/bar.cs".to_string());
    idx.empty_file_ids.push((0, 1234));
    idx.empty_file_ids.push((1, 5678));

    // Removing file_id=0 must drop ONLY its empty_file_ids entry, leaving file_id=1.
    remove_file_definitions(&mut idx, 0);
    assert_eq!(
        idx.empty_file_ids.len(), 1,
        "empty_file_ids should drop the removed file's entry, got: {:?}",
        idx.empty_file_ids
    );
    assert_eq!(idx.empty_file_ids[0].0, 1, "unrelated file_id must remain");
}

#[cfg(feature = "lang-csharp")]
#[test]
fn test_csharp_semantics_round_trip_and_file_removal() {
    use crate::definitions::{CSharpTypeEvidence, DEFINITION_INDEX_VERSION};

    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let source_file = root.join("D20Storage.cs");
    std::fs::write(
        &source_file,
        r#"namespace Demo {
public sealed class Router {
    public void Route(int value) { }
    public void CallInt() => Route(1);
}
}
"#,
    )
    .unwrap();

    let root_text = root.to_string_lossy().to_string();
    let mut index = DefinitionIndex {
        root: root_text.clone(),
        format_version: DEFINITION_INDEX_VERSION,
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };
    crate::definitions::update_file_definitions(&mut index, &source_file);

    let route = index.name_index["route"][0];
    let call_int = index.name_index["callint"][0];
    let index_base = root.join("index");
    save_definition_index(&index, &index_base).unwrap();

    let mut loaded = load_definition_index(&root_text, "cs", &index_base).unwrap();
    let callable = loaded.csharp_semantics.callable_for_definition(route).unwrap();
    let symbol_id = callable.symbol_id.as_public_id();
    assert_eq!(symbol_id.len(), 70);
    assert_eq!(
        crate::definitions::CSharpSymbolId::parse(&symbol_id),
        Some(callable.symbol_id)
    );
    assert_eq!(loaded.csharp_semantics.definitions_for_symbol(callable.symbol_id), &[route]);
    assert_eq!(
        loaded.csharp_semantics.strings.get(callable.qualified_parent),
        Some("Demo.Router")
    );
    assert_eq!(callable.parameters.len(), 1);
    assert_eq!(
        loaded.csharp_semantics.strings.get(callable.parameters[0].ty),
        Some("System.Int32")
    );

    let call_shape = loaded.csharp_semantics.call_shape(call_int, 0).unwrap();
    assert!(call_shape.self_receiver);
    let CSharpTypeEvidence::Exact(argument_type) = call_shape.arguments[0].ty else {
        panic!("expected exact argument type");
    };
    assert_eq!(
        loaded.csharp_semantics.strings.get(argument_type),
        Some("System.Int32")
    );

    let file_id = *loaded.path_to_id.get(&crate::path_identity_key(&source_file)).unwrap();
    crate::definitions::remove_file_definitions(&mut loaded, file_id);
    assert!(loaded.csharp_semantics.call_shapes_by_owner.is_empty());
    assert!(loaded.csharp_semantics.def_to_callable.iter().all(|&value| value == 0));
}


#[cfg(feature = "lang-csharp")]
fn csharp_semantic_storage_fixture(
    root: &std::path::Path,
) -> (DefinitionIndex, std::path::PathBuf, u32) {
    let source_file = root.join("D20Corrupt.cs");
    std::fs::write(
        &source_file,
        r#"namespace Demo {
public sealed class Router {
    public void Route(int value) { }
    public void CallInt() => Route(1);
}
}
"#,
    )
    .unwrap();
    let mut index = DefinitionIndex {
        root: root.to_string_lossy().to_string(),
        format_version: crate::definitions::DEFINITION_INDEX_VERSION,
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };
    crate::definitions::update_file_definitions(&mut index, &source_file);
    let call_int = index.name_index["callint"][0];
    (index, source_file, call_int)
}

#[cfg(feature = "lang-csharp")]
#[test]
fn test_csharp_semantics_rejects_call_shape_ordinal_mismatch() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let (mut index, _, call_int) = csharp_semantic_storage_fixture(&root);
    index.csharp_semantics.call_shapes_by_owner.get_mut(&call_int).unwrap().clear();

    let index_base = root.join("index");
    save_definition_index(&index, &index_base).unwrap();
    let result = load_definition_index(&index.root, "cs", &index_base);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("semantic index inconsistent"));
}

#[cfg(feature = "lang-csharp")]
#[test]
fn test_csharp_semantics_rejects_symbol_hash_mismatch() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let (mut index, _, _) = csharp_semantic_storage_fixture(&root);
    index.csharp_semantics.callables[0].symbol_id = crate::definitions::CSharpSymbolId([0; 32]);

    let index_base = root.join("index");
    save_definition_index(&index, &index_base).unwrap();
    let result = load_definition_index(&index.root, "cs", &index_base);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("semantic index inconsistent"));
}
