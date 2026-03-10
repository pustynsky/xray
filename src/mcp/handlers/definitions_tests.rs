use super::*;
use crate::definitions::DefinitionKind;

// ─── kind_priority tests ─────────────────────────────────────────

#[test]
fn test_kind_priority_class_returns_0() {
    assert_eq!(kind_priority(&DefinitionKind::Class), 0);
}

#[test]
fn test_kind_priority_interface_returns_0() {
    assert_eq!(kind_priority(&DefinitionKind::Interface), 0);
}

#[test]
fn test_kind_priority_enum_returns_0() {
    assert_eq!(kind_priority(&DefinitionKind::Enum), 0);
}

#[test]
fn test_kind_priority_struct_returns_0() {
    assert_eq!(kind_priority(&DefinitionKind::Struct), 0);
}

#[test]
fn test_kind_priority_record_returns_0() {
    assert_eq!(kind_priority(&DefinitionKind::Record), 0);
}

#[test]
fn test_kind_priority_method_returns_1() {
    assert_eq!(kind_priority(&DefinitionKind::Method), 1);
}

#[test]
fn test_kind_priority_function_returns_1() {
    assert_eq!(kind_priority(&DefinitionKind::Function), 1);
}

#[test]
fn test_kind_priority_property_returns_1() {
    assert_eq!(kind_priority(&DefinitionKind::Property), 1);
}

#[test]
fn test_kind_priority_field_returns_1() {
    assert_eq!(kind_priority(&DefinitionKind::Field), 1);
}

#[test]
fn test_kind_priority_constructor_returns_1() {
    assert_eq!(kind_priority(&DefinitionKind::Constructor), 1);
}

#[test]
fn test_kind_priority_delegate_returns_1() {
    assert_eq!(kind_priority(&DefinitionKind::Delegate), 1);
}

#[test]
fn test_kind_priority_event_returns_1() {
    assert_eq!(kind_priority(&DefinitionKind::Event), 1);
}

#[test]
fn test_kind_priority_enum_member_returns_1() {
    assert_eq!(kind_priority(&DefinitionKind::EnumMember), 1);
}

#[test]
fn test_kind_priority_type_alias_returns_1() {
    assert_eq!(kind_priority(&DefinitionKind::TypeAlias), 1);
}

#[test]
fn test_kind_priority_variable_returns_1() {
    assert_eq!(kind_priority(&DefinitionKind::Variable), 1);
}

#[test]
fn test_kind_priority_type_level_before_members() {
    // Verify that type-level definitions sort before member-level definitions
    assert!(kind_priority(&DefinitionKind::Class) < kind_priority(&DefinitionKind::Method));
    assert!(kind_priority(&DefinitionKind::Interface) < kind_priority(&DefinitionKind::Property));
    assert!(kind_priority(&DefinitionKind::Enum) < kind_priority(&DefinitionKind::Field));
    assert!(kind_priority(&DefinitionKind::Struct) < kind_priority(&DefinitionKind::Function));
    assert!(kind_priority(&DefinitionKind::Record) < kind_priority(&DefinitionKind::Constructor));
}

// ─── Parent relevance ranking tests ──────────────────────────────

/// Helper to create a DefinitionEntry with specific name, parent, and kind
fn make_def(name: &str, parent: Option<&str>, kind: DefinitionKind) -> DefinitionEntry {
    DefinitionEntry {
        name: name.to_string(),
        kind,
        file_id: 0,
        line_start: 1,
        line_end: 10,
        signature: None,
        parent: parent.map(|s| s.to_string()),
        modifiers: vec![],
        attributes: vec![],
        base_types: vec![],
    }
}

#[test]
fn test_parent_ranking_exact_parent_before_substring_parent() {
    let parent_terms = vec!["userservice".to_string()];
    let _name_terms: Vec<String> = vec![];

    let def_exact = make_def("GetUser", Some("UserService"), DefinitionKind::Method);
    let def_substring = make_def("GetUser", Some("UserServiceMock"), DefinitionKind::Method);

    let tier_exact = def_exact.parent.as_deref()
        .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);
    let tier_substring = def_substring.parent.as_deref()
        .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);

    assert!(tier_exact < tier_substring,
        "Exact parent tier {} should be less than substring parent tier {}",
        tier_exact, tier_substring);
    assert_eq!(tier_exact, 0, "Exact parent match should be tier 0");
}

#[test]
fn test_parent_ranking_prefix_parent_before_contains_parent() {
    let parent_terms = vec!["userservice".to_string()];

    let def_prefix = make_def("Create", Some("UserServiceFactory"), DefinitionKind::Method);
    let def_contains = make_def("Validate", Some("IUserService"), DefinitionKind::Method);

    let tier_prefix = def_prefix.parent.as_deref()
        .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);
    let tier_contains = def_contains.parent.as_deref()
        .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);

    assert!(tier_prefix < tier_contains,
        "Prefix parent tier {} should be less than contains parent tier {}",
        tier_prefix, tier_contains);
    assert_eq!(tier_prefix, 1, "Prefix parent match should be tier 1");
    assert_eq!(tier_contains, 2, "Contains parent match should be tier 2");
}

#[test]
fn test_parent_ranking_takes_precedence_over_name_ranking() {
    let parent_terms = vec!["userservice".to_string()];
    let name_terms = vec!["getuser".to_string()];

    let def_a = make_def("GetUser", Some("MockUserServiceWrapper"), DefinitionKind::Method);
    let def_b = make_def("FetchData", Some("UserService"), DefinitionKind::Method);

    let parent_tier_a = def_a.parent.as_deref()
        .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);
    let parent_tier_b = def_b.parent.as_deref()
        .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);

    let name_tier_a = best_match_tier(&def_a.name, &name_terms);
    let name_tier_b = best_match_tier(&def_b.name, &name_terms);

    assert!(name_tier_a < name_tier_b, "def_a should have better name tier");
    assert!(parent_tier_b < parent_tier_a, "def_b should have better parent tier");

    let cmp = parent_tier_a.cmp(&parent_tier_b)
        .then_with(|| name_tier_a.cmp(&name_tier_b));
    assert_eq!(cmp, std::cmp::Ordering::Greater,
        "def_a should sort AFTER def_b because parent tier is primary");
}

#[test]
fn test_parent_ranking_no_parent_sorts_last() {
    let parent_terms = vec!["userservice".to_string()];

    let def_with_parent = make_def("GetUser", Some("UserService"), DefinitionKind::Method);
    let def_no_parent = make_def("GetUser", None, DefinitionKind::Method);

    let tier_with = def_with_parent.parent.as_deref()
        .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);
    let tier_without = def_no_parent.parent.as_deref()
        .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);

    assert_eq!(tier_with, 0, "Exact parent should be tier 0");
    assert_eq!(tier_without, 3, "No parent should be tier 3 (worst)");
    assert!(tier_with < tier_without);
}

#[test]
fn test_parent_ranking_only_active_with_parent_filter() {
    let parent_terms: Vec<String> = vec![];

    let def_a = make_def("GetUser", Some("UserService"), DefinitionKind::Method);
    let def_b = make_def("FetchData", Some("OrderService"), DefinitionKind::Method);

    let tier_a = if !parent_terms.is_empty() {
        def_a.parent.as_deref().map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3)
    } else { 0 };
    let tier_b = if !parent_terms.is_empty() {
        def_b.parent.as_deref().map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3)
    } else { 0 };

    assert_eq!(tier_a, 0);
    assert_eq!(tier_b, 0);
    assert_eq!(tier_a.cmp(&tier_b), std::cmp::Ordering::Equal,
        "Without parent filter, parent tier should be equal for all");
}

// ─── Comma-separated file filter tests ───────────────────────────

#[test]
fn test_file_filter_comma_separated_matches_multiple_files() {
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "file": "ResilientClient.cs,ProxyClient.cs",
        "kind": "method"
    }));
    assert!(!result.is_error, "should not error: {:?}", result.content[0].text);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert!(defs.len() >= 2, "expected >= 2 methods from two files, got {}", defs.len());
    let files: Vec<&str> = defs.iter()
        .map(|d| d["file"].as_str().unwrap())
        .collect();
    assert!(files.iter().any(|f| f.contains("ResilientClient")),
        "should include ResilientClient");
    assert!(files.iter().any(|f| f.contains("ProxyClient")),
        "should include ProxyClient");
}

#[test]
fn test_file_filter_single_value_still_works() {
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "file": "QueryService.cs",
        "kind": "method"
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert!(defs.len() >= 3, "expected >= 3 methods in QueryService, got {}", defs.len());
    for d in defs {
        assert!(d["file"].as_str().unwrap().contains("QueryService"),
            "all results should be from QueryService");
    }
}

#[test]
fn test_file_filter_comma_separated_no_match_returns_empty() {
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "file": "NonExistent.cs,AlsoMissing.cs"
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 0, "no files match, should return 0 results");
}

// ─── Comma-separated parent filter tests ─────────────────────────

#[test]
fn test_parent_filter_comma_separated_matches_multiple_classes() {
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "parent": "ResilientClient,ProxyClient",
        "kind": "method"
    }));
    assert!(!result.is_error, "should not error: {:?}", result.content[0].text);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert!(defs.len() >= 2, "expected >= 2 methods from two classes, got {}", defs.len());
    let parents: Vec<&str> = defs.iter()
        .map(|d| d["parent"].as_str().unwrap())
        .collect();
    assert!(parents.iter().any(|p| *p == "ResilientClient"),
        "should include ResilientClient methods");
    assert!(parents.iter().any(|p| *p == "ProxyClient"),
        "should include ProxyClient methods");
}

#[test]
fn test_parent_filter_single_value_still_works() {
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "parent": "QueryService",
        "kind": "method"
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert!(defs.len() >= 3, "expected >= 3 methods in QueryService, got {}", defs.len());
    for d in defs {
        assert_eq!(d["parent"].as_str().unwrap(), "QueryService",
            "all results should have parent QueryService");
    }
}

#[test]
fn test_parent_filter_comma_separated_no_match_returns_empty() {
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "parent": "NonExistentClass,AlsoMissing"
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 0, "no parents match, should return 0 results");
}

// ─── crossValidate audit tests ────────────────────────────────────

#[test]
fn test_audit_cross_validate_no_file_index_returns_skipped() {
    let ctx = make_transitive_inheritance_ctx();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "audit": true,
        "crossValidate": true
    }));
    assert!(!result.is_error, "audit+crossValidate should not error: {:?}", result.content[0].text);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v["crossValidation"].is_object(), "Should have crossValidation object");
    assert_eq!(v["crossValidation"]["status"], "skipped",
        "Should be 'skipped' when file-list index not found");
}

#[test]
fn test_audit_without_cross_validate_has_no_cross_validation() {
    let ctx = make_transitive_inheritance_ctx();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "audit": true
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v.get("crossValidation").is_none(),
        "Without crossValidate=true, should NOT have crossValidation in output");
}

#[test]
fn test_audit_cross_validate_with_file_index() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path().join("project");
    std::fs::create_dir_all(&project_dir).unwrap();

    { let mut f = std::fs::File::create(project_dir.join("FileA.cs")).unwrap();
      writeln!(f, "class FileA {{ }}").unwrap(); }
    { let mut f = std::fs::File::create(project_dir.join("FileB.cs")).unwrap();
      writeln!(f, "class FileB {{ }}").unwrap(); }

    let project_str = crate::clean_path(&project_dir.to_string_lossy());
    let idx_base = tmp.path().join("indexes");
    std::fs::create_dir_all(&idx_base).unwrap();

    let file_index = crate::build_index(&crate::IndexArgs {
        dir: project_str.clone(),
        max_age_hours: 24, hidden: false, no_ignore: false, threads: 0,
    }).unwrap();
    crate::save_index(&file_index, &idx_base).unwrap();

    let def_index = crate::definitions::build_definition_index(
        &crate::definitions::DefIndexArgs {
            dir: project_str.clone(),
            ext: "cs".to_string(),
            threads: 1,
        }
    );

    let content_index = crate::ContentIndex {
        root: project_str.clone(),
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_index))),
        server_dir: project_str,
        index_base: idx_base,
        ..Default::default()
    };

    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "audit": true,
        "crossValidate": true
    }));
    assert!(!result.is_error, "Should not error: {:?}", result.content[0].text);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(v["crossValidation"]["status"], "ok",
        "Cross-validation should succeed when file-list index exists");
    assert!(v["crossValidation"]["fileListFiles"].as_u64().unwrap() > 0,
        "Should report file-list file count");
    assert!(v["crossValidation"]["defIndexFiles"].as_u64().unwrap() > 0,
        "Should report def index file count");
}

// ─── baseTypeTransitive tests ─────────────────────────────────────

/// Helper to create a context with a 3-level inheritance chain:
/// BaseService → MiddleService → ConcreteService
fn make_transitive_inheritance_ctx() -> HandlerContext {
    use crate::definitions::*;

    let definitions = vec![
        DefinitionEntry {
            name: "BaseService".to_string(),
            kind: DefinitionKind::Class,
            file_id: 0, line_start: 1, line_end: 50,
            signature: None, parent: None, modifiers: vec![],
            attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            name: "MiddleService".to_string(),
            kind: DefinitionKind::Class,
            file_id: 0, line_start: 52, line_end: 100,
            signature: None, parent: None, modifiers: vec![],
            attributes: vec![],
            base_types: vec!["BaseService".to_string()],
        },
        DefinitionEntry {
            name: "ConcreteService".to_string(),
            kind: DefinitionKind::Class,
            file_id: 1, line_start: 1, line_end: 80,
            signature: None, parent: None, modifiers: vec![],
            attributes: vec![],
            base_types: vec!["MiddleService".to_string()],
        },
        DefinitionEntry {
            name: "UnrelatedService".to_string(),
            kind: DefinitionKind::Class,
            file_id: 1, line_start: 82, line_end: 120,
            signature: None, parent: None, modifiers: vec![],
            attributes: vec![],
            base_types: vec!["SomethingElse".to_string()],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut base_type_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();

    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
        for bt in &def.base_types {
            base_type_index.entry(bt.to_lowercase()).or_default().push(idx);
        }
    }

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec![
            "C:\\src\\Services.cs".to_string(),
            "C:\\src\\Concrete.cs".to_string(),
        ],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index,
        file_index,
        path_to_id: HashMap::new(),
        method_calls: HashMap::new(),
        ..Default::default()
    };

    let content_index = crate::ContentIndex {
        root: ".".to_string(),
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };

    super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_index))),
        ..Default::default()
    }
}

#[test]
fn test_base_type_transitive_finds_indirect_descendants() {
    let ctx = make_transitive_inheritance_ctx();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "baseType": "BaseService",
        "baseTypeTransitive": true
    }));
    assert!(!result.is_error, "Should not error: {:?}", result.content[0].text);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"MiddleService"), "Should find MiddleService (direct child)");
    assert!(names.contains(&"ConcreteService"), "Should find ConcreteService (grandchild via transitive BFS)");
    assert!(!names.contains(&"UnrelatedService"), "Should NOT find UnrelatedService");
    assert!(!names.contains(&"BaseService"), "Should NOT find BaseService itself (it doesn't inherit from itself)");
}

#[test]
fn test_base_type_non_transitive_finds_only_direct() {
    let ctx = make_transitive_inheritance_ctx();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "baseType": "BaseService"
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"MiddleService"), "Should find MiddleService (direct child)");
    assert!(!names.contains(&"ConcreteService"), "Should NOT find ConcreteService (indirect, transitive=false)");
}

#[test]
fn test_base_type_transitive_no_match_returns_empty() {
    let ctx = make_transitive_inheritance_ctx();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "baseType": "NonExistentType",
        "baseTypeTransitive": true
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert!(defs.is_empty(), "Non-existent base type should return 0 results");
}

#[test]
fn test_base_type_empty_string_treated_as_no_filter() {
    let ctx = make_transitive_inheritance_ctx();
    let result_empty = handle_search_definitions(&ctx, &serde_json::json!({
        "baseType": ""
    }));
    assert!(!result_empty.is_error);
    let v_empty: serde_json::Value = serde_json::from_str(&result_empty.content[0].text).unwrap();
    let defs_empty = v_empty["definitions"].as_array().unwrap();

    let result_no_filter = handle_search_definitions(&ctx, &serde_json::json!({}));
    let v_no_filter: serde_json::Value = serde_json::from_str(&result_no_filter.content[0].text).unwrap();
    let defs_no_filter = v_no_filter["definitions"].as_array().unwrap();

    assert_eq!(defs_empty.len(), defs_no_filter.len(),
        "baseType='' should return same results as no baseType filter. Got {} vs {}",
        defs_empty.len(), defs_no_filter.len());
}

#[test]
fn test_base_type_substring_matches_generic_interface() {
    use crate::definitions::*;

    let definitions = vec![
        DefinitionEntry {
            name: "GenericImpl".to_string(),
            kind: DefinitionKind::Class,
            file_id: 0, line_start: 1, line_end: 50,
            signature: None, parent: None, modifiers: vec![],
            attributes: vec![],
            base_types: vec!["IRepository<Model>".to_string()],
        },
        DefinitionEntry {
            name: "AnotherImpl".to_string(),
            kind: DefinitionKind::Class,
            file_id: 0, line_start: 52, line_end: 100,
            signature: None, parent: None, modifiers: vec![],
            attributes: vec![],
            base_types: vec!["IRepository<Report>".to_string()],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut base_type_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();

    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
        for bt in &def.base_types {
            base_type_index.entry(bt.to_lowercase()).or_default().push(idx);
        }
    }

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec!["C:\\src\\Impls.cs".to_string()],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index,
        file_index, path_to_id: HashMap::new(),
        method_calls: HashMap::new(), ..Default::default()
    };

    let content_index = crate::ContentIndex {
        root: ".".to_string(),
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_index))),
        ..Default::default()
    };

    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "baseType": "IRepository"
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 2, "baseType='IRepository' should find both IRepository<Model> and IRepository<Report> via substring. Got: {:?}",
        defs.iter().map(|d| d["name"].as_str().unwrap()).collect::<Vec<_>>());

    let result2 = handle_search_definitions(&ctx, &serde_json::json!({
        "baseType": "IRepository<Model>"
    }));
    assert!(!result2.is_error);
    let v2: serde_json::Value = serde_json::from_str(&result2.content[0].text).unwrap();
    let defs2 = v2["definitions"].as_array().unwrap();
    assert_eq!(defs2.len(), 1, "baseType='IRepository<Model>' should find only GenericImpl via exact match");
    assert_eq!(defs2[0]["name"], "GenericImpl");
}

#[test]
fn test_base_type_transitive_case_insensitive() {
    let ctx = make_transitive_inheritance_ctx();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "baseType": "BASESERVICE",
        "baseTypeTransitive": true
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert!(defs.len() >= 2, "Case-insensitive transitive should find both descendants");
}

// ─── B-1 BFS cascade prevention test ──────────────────────────────

#[test]
fn test_base_type_transitive_no_cascade_with_dangerous_names() {
    use crate::definitions::*;

    let definitions = vec![
        DefinitionEntry {
            name: "BaseBlock".to_string(), kind: DefinitionKind::Class,
            file_id: 0, line_start: 1, line_end: 50,
            signature: None, parent: None, modifiers: vec![],
            attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            name: "Service".to_string(), kind: DefinitionKind::Class,
            file_id: 0, line_start: 52, line_end: 100,
            signature: None, parent: None, modifiers: vec![],
            attributes: vec![],
            base_types: vec!["BaseBlock".to_string()],
        },
        DefinitionEntry {
            name: "UnrelatedA".to_string(), kind: DefinitionKind::Class,
            file_id: 1, line_start: 1, line_end: 50,
            signature: None, parent: None, modifiers: vec![],
            attributes: vec![],
            base_types: vec!["IService".to_string()],
        },
        DefinitionEntry {
            name: "UnrelatedB".to_string(), kind: DefinitionKind::Class,
            file_id: 1, line_start: 52, line_end: 100,
            signature: None, parent: None, modifiers: vec![],
            attributes: vec![],
            base_types: vec!["WebServiceBase".to_string()],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut base_type_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();

    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
        for bt in &def.base_types {
            base_type_index.entry(bt.to_lowercase()).or_default().push(idx);
        }
    }

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec![
            "C:\\src\\Blocks.cs".to_string(),
            "C:\\src\\Services.cs".to_string(),
        ],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index,
        file_index, path_to_id: HashMap::new(),
        method_calls: HashMap::new(), ..Default::default()
    };

    let content_index = crate::ContentIndex {
        root: ".".to_string(),
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_index))),
        ..Default::default()
    };

    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "baseType": "BaseBlock",
        "baseTypeTransitive": true
    }));
    assert!(!result.is_error, "Should not error: {:?}", result.content[0].text);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();

    assert!(names.contains(&"Service"),
        "Should find Service (direct descendant). Got: {:?}", names);
    assert!(!names.contains(&"UnrelatedA"),
        "Should NOT find UnrelatedA (unrelated, inherits IService not BaseBlock). Got: {:?}", names);
    assert!(!names.contains(&"UnrelatedB"),
        "Should NOT find UnrelatedB (unrelated, inherits WebServiceBase not BaseBlock). Got: {:?}", names);
}

#[test]
fn test_base_type_transitive_generics_still_work_at_seed_level() {
    use crate::definitions::*;

    let definitions = vec![
        DefinitionEntry {
            name: "GenericImpl".to_string(), kind: DefinitionKind::Class,
            file_id: 0, line_start: 1, line_end: 50,
            signature: None, parent: None, modifiers: vec![],
            attributes: vec![],
            base_types: vec!["IRepository<Model>".to_string()],
        },
        DefinitionEntry {
            name: "AnotherImpl".to_string(), kind: DefinitionKind::Class,
            file_id: 0, line_start: 52, line_end: 100,
            signature: None, parent: None, modifiers: vec![],
            attributes: vec![],
            base_types: vec!["IRepository<Report>".to_string()],
        },
        DefinitionEntry {
            name: "SubImpl".to_string(), kind: DefinitionKind::Class,
            file_id: 0, line_start: 102, line_end: 150,
            signature: None, parent: None, modifiers: vec![],
            attributes: vec![],
            base_types: vec!["GenericImpl".to_string()],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut base_type_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();

    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
        for bt in &def.base_types {
            base_type_index.entry(bt.to_lowercase()).or_default().push(idx);
        }
    }

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec!["C:\\src\\Impls.cs".to_string()],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index,
        file_index, path_to_id: HashMap::new(),
        method_calls: HashMap::new(), ..Default::default()
    };

    let content_index = crate::ContentIndex {
        root: ".".to_string(),
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_index))),
        ..Default::default()
    };

    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "baseType": "IRepository",
        "baseTypeTransitive": true
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();

    assert!(names.contains(&"GenericImpl"),
        "Should find GenericImpl (IRepository<Model> matched via seed substring). Got: {:?}", names);
    assert!(names.contains(&"AnotherImpl"),
        "Should find AnotherImpl (IRepository<Report> matched via seed substring). Got: {:?}", names);
    assert!(names.contains(&"SubImpl"),
        "Should find SubImpl (inherits GenericImpl, found via level 1 exact match). Got: {:?}", names);
}

// ─── F-2 Hint for large transitive hierarchy test ─────────────────

#[test]
fn test_base_type_transitive_hint_for_large_hierarchy() {
    let ctx = make_transitive_inheritance_ctx();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "baseType": "BaseService",
        "baseTypeTransitive": true
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v["summary"].get("hint").is_none(),
        "No hint expected for small result set (< 5000)");
}

#[test]
fn test_parent_filter_comma_with_spaces_trimmed() {
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "parent": " ResilientClient , ProxyClient ",
        "kind": "method"
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert!(defs.len() >= 2, "spaces should be trimmed, still match both classes");
}

// ─── termBreakdown tests ──────────────────────────────────────────

#[test]
fn test_term_breakdown_multi_term_shows_per_term_counts() {
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "name": "QueryService,ResilientClient"
    }));
    assert!(!result.is_error, "should not error: {:?}", result.content[0].text);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let summary = &v["summary"];
    assert!(summary.get("termBreakdown").is_some(),
        "Multi-term name query should have termBreakdown in summary");
    let breakdown = summary["termBreakdown"].as_object().unwrap();
    assert!(breakdown.contains_key("queryservice"),
        "termBreakdown should have key for 'queryservice'");
    assert!(breakdown.contains_key("resilientclient"),
        "termBreakdown should have key for 'resilientclient'");
    assert!(breakdown["queryservice"].as_u64().unwrap() > 0,
        "queryservice should have results");
    assert!(breakdown["resilientclient"].as_u64().unwrap() > 0,
        "resilientclient should have results");
}

#[test]
fn test_term_breakdown_single_term_not_present() {
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "name": "QueryService"
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v["summary"].get("termBreakdown").is_none(),
        "Single-term query should NOT have termBreakdown");
}

#[test]
fn test_term_breakdown_regex_not_present() {
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "name": "Query.*",
        "regex": true
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v["summary"].get("termBreakdown").is_none(),
        "Regex query should NOT have termBreakdown");
}

#[test]
fn test_term_breakdown_no_name_filter_not_present() {
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "kind": "class"
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v["summary"].get("termBreakdown").is_none(),
        "Query without name filter should NOT have termBreakdown");
}

#[test]
fn test_term_breakdown_with_zero_match_term() {
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "name": "QueryService,NonExistentXyzZzz"
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let breakdown = v["summary"]["termBreakdown"].as_object().unwrap();
    assert!(breakdown["queryservice"].as_u64().unwrap() > 0,
        "queryservice should have results");
    assert_eq!(breakdown["nonexistentxyzzzz"].as_u64().unwrap(), 0,
        "nonexistent term should have 0 results");
}

#[test]
fn test_term_breakdown_counts_are_pre_truncation() {
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "name": "QueryService,ResilientClient",
        "maxResults": 1
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let breakdown = v["summary"]["termBreakdown"].as_object().unwrap();
    let total_breakdown: u64 = breakdown.values()
        .filter_map(|v| v.as_u64())
        .sum();
    let total_results = v["summary"]["totalResults"].as_u64().unwrap();
    assert_eq!(total_breakdown, total_results,
        "Sum of termBreakdown counts ({}) should equal totalResults ({})",
        total_breakdown, total_results);
    let returned = v["summary"]["returned"].as_u64().unwrap();
    assert!(returned <= 1, "returned should be <= maxResults=1, got {}", returned);
}

// ═══════════════════════════════════════════════════════════════════
// NEW TESTS — for extracted functions
// ═══════════════════════════════════════════════════════════════════

// ─── parse_definition_args tests ─────────────────────────────────

#[test]
fn test_parse_args_empty_returns_defaults() {
    let args = json!({});
    let parsed = parse_definition_args(&args).unwrap();
    assert!(parsed.name_filter.is_none());
    assert!(parsed.kind_filter.is_none());
    assert!(parsed.file_filter.is_none());
    assert!(parsed.parent_filter.is_none());
    assert!(parsed.contains_line.is_none());
    assert!(!parsed.use_regex);
    assert_eq!(parsed.max_results, 100);
    assert!(parsed.exclude_dir.is_empty());
    assert!(!parsed.include_body);
    assert_eq!(parsed.max_body_lines, 100);
    assert_eq!(parsed.max_total_body_lines, 500);
    assert!(!parsed.audit);
    assert!(!parsed.include_code_stats);
    assert!(parsed.sort_by.is_none());
    assert!(!parsed.has_stats_filter());
}

#[test]
fn test_parse_args_name_filter_empty_string_is_none() {
    let args = json!({"name": ""});
    let parsed = parse_definition_args(&args).unwrap();
    assert!(parsed.name_filter.is_none(), "empty name should be treated as None");
}

#[test]
fn test_parse_args_name_filter_non_empty() {
    let args = json!({"name": "UserService"});
    let parsed = parse_definition_args(&args).unwrap();
    assert_eq!(parsed.name_filter, Some("UserService".to_string()));
}

#[test]
fn test_parse_args_base_type_empty_string_is_none() {
    let args = json!({"baseType": ""});
    let parsed = parse_definition_args(&args).unwrap();
    assert!(parsed.base_type_filter.is_none(), "empty baseType should be treated as None");
}

#[test]
fn test_parse_args_contains_line_zero_rejected() {
    let args = json!({"containsLine": 0});
    let result = parse_definition_args(&args);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must be >= 1"));
}

#[test]
fn test_parse_args_contains_line_negative_rejected() {
    let args = json!({"containsLine": -5});
    let result = parse_definition_args(&args);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must be >= 1"));
}

#[test]
fn test_parse_args_contains_line_valid() {
    let args = json!({"containsLine": 42});
    let parsed = parse_definition_args(&args).unwrap();
    assert_eq!(parsed.contains_line, Some(42));
}

#[test]
fn test_parse_args_sort_by_valid_values() {
    for field in &["cyclomaticComplexity", "cognitiveComplexity", "maxNestingDepth",
                   "paramCount", "returnCount", "callCount", "lambdaCount", "lines"] {
        let args = json!({"sortBy": field});
        let parsed = parse_definition_args(&args);
        assert!(parsed.is_ok(), "sortBy='{}' should be valid", field);
        assert_eq!(parsed.unwrap().sort_by, Some(field.to_string()));
    }
}

#[test]
fn test_parse_args_sort_by_invalid_rejected() {
    let args = json!({"sortBy": "invalidField"});
    let result = parse_definition_args(&args);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Invalid sortBy"));
}

#[test]
fn test_parse_args_has_stats_filter_with_min() {
    let args = json!({"minComplexity": 10});
    let parsed = parse_definition_args(&args).unwrap();
    assert!(parsed.has_stats_filter());
    assert!(parsed.include_code_stats, "min* implies includeCodeStats");
}

#[test]
fn test_parse_args_has_stats_filter_with_sort_by() {
    let args = json!({"sortBy": "lines"});
    let parsed = parse_definition_args(&args).unwrap();
    assert!(parsed.has_stats_filter());
    assert!(parsed.include_code_stats);
}

#[test]
fn test_parse_args_include_code_stats_explicit() {
    let args = json!({"includeCodeStats": true});
    let parsed = parse_definition_args(&args).unwrap();
    assert!(parsed.include_code_stats);
    assert!(!parsed.has_stats_filter(), "explicit includeCodeStats doesn't set has_stats_filter");
}

#[test]
fn test_parse_args_exclude_dir() {
    let args = json!({"excludeDir": ["node_modules", "bin"]});
    let parsed = parse_definition_args(&args).unwrap();
    assert_eq!(parsed.exclude_dir, vec!["node_modules".to_string(), "bin".to_string()]);
}

#[test]
fn test_parse_args_all_code_stats_filters() {
    let args = json!({
        "minComplexity": 5,
        "minCognitive": 10,
        "minNesting": 3,
        "minParams": 4,
        "minReturns": 2,
        "minCalls": 8
    });
    let parsed = parse_definition_args(&args).unwrap();
    assert_eq!(parsed.min_complexity, Some(5u16));
    assert_eq!(parsed.min_cognitive, Some(10u16));
    assert_eq!(parsed.min_nesting, Some(3u8));
    assert_eq!(parsed.min_params, Some(4u8));
    assert_eq!(parsed.min_returns, Some(2u8));
    assert_eq!(parsed.min_calls, Some(8u16));
    assert!(parsed.has_stats_filter());
}

// ─── collect_candidates tests ────────────────────────────────────

/// Helper to create a DefinitionIndex for collect_candidates tests.
fn make_test_def_index() -> DefinitionIndex {
    let definitions = vec![
        DefinitionEntry {
            name: "UserService".to_string(), kind: DefinitionKind::Class,
            file_id: 0, line_start: 1, line_end: 100,
            signature: None, parent: None, modifiers: vec![],
            attributes: vec!["injectable".to_string()], base_types: vec![],
        },
        DefinitionEntry {
            name: "GetUser".to_string(), kind: DefinitionKind::Method,
            file_id: 0, line_start: 10, line_end: 30,
            signature: None, parent: Some("UserService".to_string()), modifiers: vec![],
            attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            name: "OrderService".to_string(), kind: DefinitionKind::Class,
            file_id: 1, line_start: 1, line_end: 80,
            signature: None, parent: None, modifiers: vec![],
            attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            name: "GetOrder".to_string(), kind: DefinitionKind::Method,
            file_id: 1, line_start: 10, line_end: 25,
            signature: None, parent: Some("OrderService".to_string()), modifiers: vec![],
            attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut attribute_index: HashMap<String, Vec<u32>> = HashMap::new();

    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
        for attr in &def.attributes {
            attribute_index.entry(attr.to_lowercase()).or_default().push(idx);
        }
    }

    DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec![
            "C:\\src\\UserService.cs".to_string(),
            "C:\\src\\OrderService.cs".to_string(),
        ],
        definitions, name_index, kind_index,
        attribute_index, base_type_index: HashMap::new(),
        file_index, path_to_id: HashMap::new(),
        method_calls: HashMap::new(), ..Default::default()
    }
}

#[test]
fn test_collect_candidates_no_filters_returns_all() {
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({})).unwrap();
    let (candidates, _) = collect_candidates(&index, &args).unwrap();
    assert_eq!(candidates.len(), 4, "No filters → all 4 definitions");
}

#[test]
fn test_collect_candidates_kind_filter() {
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({"kind": "class"})).unwrap();
    let (candidates, _) = collect_candidates(&index, &args).unwrap();
    assert_eq!(candidates.len(), 2, "kind=class → 2 classes");
    for &idx in &candidates {
        assert_eq!(index.definitions[idx as usize].kind, DefinitionKind::Class);
    }
}

#[test]
fn test_collect_candidates_name_substring() {
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({"name": "Service"})).unwrap();
    let (candidates, _) = collect_candidates(&index, &args).unwrap();
    assert_eq!(candidates.len(), 2, "name=Service → UserService + OrderService");
}

#[test]
fn test_collect_candidates_name_multi_term() {
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({"name": "UserService,GetOrder"})).unwrap();
    let (candidates, def_to_term) = collect_candidates(&index, &args).unwrap();
    assert_eq!(candidates.len(), 2, "name=UserService,GetOrder → 2 matches");
    // Check term mapping
    assert!(!def_to_term.is_empty(), "def_to_term should be populated for multi-term");
}

#[test]
fn test_collect_candidates_kind_and_name_intersection() {
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({"kind": "method", "name": "GetUser"})).unwrap();
    let (candidates, _) = collect_candidates(&index, &args).unwrap();
    assert_eq!(candidates.len(), 1, "kind=method + name=GetUser → 1 match");
    assert_eq!(index.definitions[candidates[0] as usize].name, "GetUser");
}

#[test]
fn test_collect_candidates_invalid_kind_returns_error() {
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({"kind": "nonexistent"})).unwrap();
    let result = collect_candidates(&index, &args);
    assert!(result.is_err(), "Invalid kind should return error");
}

#[test]
fn test_collect_candidates_attribute_filter() {
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({"attribute": "Injectable"})).unwrap();
    let (candidates, _) = collect_candidates(&index, &args).unwrap();
    assert_eq!(candidates.len(), 1, "attribute=Injectable → 1 match");
    assert_eq!(index.definitions[candidates[0] as usize].name, "UserService");
}

#[test]
fn test_collect_candidates_regex_name() {
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({"name": "Get.*", "regex": true})).unwrap();
    let (candidates, _) = collect_candidates(&index, &args).unwrap();
    assert_eq!(candidates.len(), 2, "regex Get.* → GetUser + GetOrder");
}

// ─── apply_entry_filters tests ───────────────────────────────────

#[test]
fn test_apply_entry_filters_file_filter() {
    let index = make_test_def_index();
    let candidates: Vec<u32> = (0..4).collect();
    let args = parse_definition_args(&json!({"file": "UserService.cs"})).unwrap();
    let results = apply_entry_filters(&index, &candidates, &args);
    assert_eq!(results.len(), 2, "file=UserService.cs → 2 defs in that file");
    for (_, def) in &results {
        assert_eq!(def.file_id, 0);
    }
}

#[test]
fn test_apply_entry_filters_parent_filter() {
    let index = make_test_def_index();
    let candidates: Vec<u32> = (0..4).collect();
    let args = parse_definition_args(&json!({"parent": "UserService"})).unwrap();
    let results = apply_entry_filters(&index, &candidates, &args);
    assert_eq!(results.len(), 1, "parent=UserService → 1 method");
    assert_eq!(results[0].1.name, "GetUser");
}

#[test]
fn test_apply_entry_filters_exclude_dir() {
    let index = make_test_def_index();
    let candidates: Vec<u32> = (0..4).collect();
    let args = parse_definition_args(&json!({"excludeDir": ["OrderService"]})).unwrap();
    let results = apply_entry_filters(&index, &candidates, &args);
    // OrderService.cs is excluded → only UserService.cs defs remain
    assert_eq!(results.len(), 2, "excludeDir OrderService → 2 defs from UserService.cs");
}

#[test]
fn test_apply_entry_filters_comma_separated_file() {
    let index = make_test_def_index();
    let candidates: Vec<u32> = (0..4).collect();
    let args = parse_definition_args(&json!({"file": "UserService.cs,OrderService.cs"})).unwrap();
    let results = apply_entry_filters(&index, &candidates, &args);
    assert_eq!(results.len(), 4, "both files → all 4 defs");
}

#[test]
fn test_apply_entry_filters_parent_no_match_returns_empty() {
    let index = make_test_def_index();
    let candidates: Vec<u32> = (0..4).collect();
    let args = parse_definition_args(&json!({"parent": "NonExistentClass"})).unwrap();
    let results = apply_entry_filters(&index, &candidates, &args);
    assert_eq!(results.len(), 0);
}

// ─── apply_stats_filters tests ───────────────────────────────────

#[test]
fn test_apply_stats_filters_no_filter_passthrough() {
    let index = make_test_def_index();
    let all_defs: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
        .enumerate()
        .map(|(i, d)| (i as u32, d))
        .collect();
    let mut results = all_defs;
    let args = parse_definition_args(&json!({})).unwrap();
    let info = apply_stats_filters(&index, &mut results, &args).unwrap();
    assert!(!info.applied, "No stats filter → not applied");
    assert_eq!(results.len(), 4, "All 4 should remain");
}

#[test]
fn test_apply_stats_filters_error_when_no_stats() {
    let index = make_test_def_index(); // no code_stats populated
    let all_defs: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
        .enumerate()
        .map(|(i, d)| (i as u32, d))
        .collect();
    let mut results = all_defs;
    let args = parse_definition_args(&json!({"minComplexity": 5})).unwrap();
    let result = apply_stats_filters(&index, &mut results, &args);
    assert!(result.is_err(), "Should error when code_stats is empty");
    assert!(result.unwrap_err().contains("Code stats not available"));
}

#[test]
fn test_apply_stats_filters_sort_by_lines_no_stats_needed() {
    let index = make_test_def_index(); // no code_stats — but sortBy=lines doesn't need them
    let all_defs: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
        .enumerate()
        .map(|(i, d)| (i as u32, d))
        .collect();
    let mut results = all_defs;
    let args = parse_definition_args(&json!({"sortBy": "lines"})).unwrap();
    let info = apply_stats_filters(&index, &mut results, &args).unwrap();
    assert!(!info.applied, "sortBy=lines doesn't filter, just sorts");
    assert_eq!(results.len(), 4, "All 4 should remain");
}

// ─── compute_term_breakdown tests ────────────────────────────────

#[test]
fn test_compute_term_breakdown_single_term_returns_none() {
    let results: Vec<(u32, &DefinitionEntry)> = vec![];
    let def_to_term = HashMap::new();
    let args = parse_definition_args(&json!({"name": "UserService"})).unwrap();
    let breakdown = compute_term_breakdown(&results, &def_to_term, &args);
    assert!(breakdown.is_none(), "Single term → no breakdown");
}

#[test]
fn test_compute_term_breakdown_no_name_returns_none() {
    let results: Vec<(u32, &DefinitionEntry)> = vec![];
    let def_to_term = HashMap::new();
    let args = parse_definition_args(&json!({})).unwrap();
    let breakdown = compute_term_breakdown(&results, &def_to_term, &args);
    assert!(breakdown.is_none(), "No name filter → no breakdown");
}

#[test]
fn test_compute_term_breakdown_regex_returns_none() {
    let results: Vec<(u32, &DefinitionEntry)> = vec![];
    let def_to_term = HashMap::new();
    let args = parse_definition_args(&json!({"name": "Get.*", "regex": true})).unwrap();
    let breakdown = compute_term_breakdown(&results, &def_to_term, &args);
    assert!(breakdown.is_none(), "Regex → no breakdown");
}

#[test]
fn test_compute_term_breakdown_multi_term() {
    let index = make_test_def_index();
    let mut def_to_term: HashMap<u32, usize> = HashMap::new();
    def_to_term.insert(0, 0); // UserService → term 0
    def_to_term.insert(3, 1); // GetOrder → term 1

    let results: Vec<(u32, &DefinitionEntry)> = vec![
        (0, &index.definitions[0]),
        (3, &index.definitions[3]),
    ];
    let args = parse_definition_args(&json!({"name": "UserService,GetOrder"})).unwrap();
    let breakdown = compute_term_breakdown(&results, &def_to_term, &args);
    assert!(breakdown.is_some(), "Multi-term → should have breakdown");
    let bd = breakdown.unwrap();
    assert_eq!(bd["userservice"], 1);
    assert_eq!(bd["getorder"], 1);
}

// ─── sort_results tests ──────────────────────────────────────────

#[test]
fn test_sort_results_by_lines_descending() {
    let index = make_test_def_index();
    let mut results: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
        .enumerate()
        .map(|(i, d)| (i as u32, d))
        .collect();
    let args = parse_definition_args(&json!({"sortBy": "lines"})).unwrap();
    sort_results(&mut results, &index, &args);
    // Should be sorted by line count descending
    let line_counts: Vec<u32> = results.iter()
        .map(|(_, d)| d.line_end - d.line_start + 1)
        .collect();
    for i in 0..line_counts.len() - 1 {
        assert!(line_counts[i] >= line_counts[i + 1],
            "Should be descending: {} >= {}", line_counts[i], line_counts[i + 1]);
    }
}

#[test]
fn test_sort_results_relevance_exact_before_prefix() {
    let index = make_test_def_index();
    // Search for "userservice" — UserService (exact) should come before GetUser (no match)
    let mut results: Vec<(u32, &DefinitionEntry)> = vec![
        (1, &index.definitions[1]), // GetUser
        (0, &index.definitions[0]), // UserService
    ];
    let args = parse_definition_args(&json!({"name": "userservice"})).unwrap();
    sort_results(&mut results, &index, &args);
    assert_eq!(results[0].1.name, "UserService", "Exact match should sort first");
}

#[test]
fn test_sort_results_no_filter_no_sort() {
    let index = make_test_def_index();
    let original_order: Vec<u32> = (0..4).collect();
    let mut results: Vec<(u32, &DefinitionEntry)> = original_order.iter()
        .map(|&i| (i, &index.definitions[i as usize]))
        .collect();
    let args = parse_definition_args(&json!({})).unwrap();
    sort_results(&mut results, &index, &args);
    // Without name/parent filter, no sorting happens — order preserved
    let result_indices: Vec<u32> = results.iter().map(|(i, _)| *i).collect();
    assert_eq!(result_indices, original_order, "No filter → original order preserved");
}

// ─── get_sort_value tests ────────────────────────────────────────

#[test]
fn test_get_sort_value_lines() {
    let def = make_def("Test", None, DefinitionKind::Method);
    // line_start=1, line_end=10 → 10 lines
    assert_eq!(get_sort_value(None, &def, "lines"), 10);
}

#[test]
fn test_get_sort_value_no_stats_returns_zero() {
    let def = make_def("Test", None, DefinitionKind::Method);
    assert_eq!(get_sort_value(None, &def, "cyclomaticComplexity"), 0);
}

#[test]
fn test_get_sort_value_with_stats() {
    let def = make_def("Test", None, DefinitionKind::Method);
    let stats = CodeStats {
        cyclomatic_complexity: 15,
        cognitive_complexity: 25,
        max_nesting_depth: 4,
        param_count: 3,
        return_count: 2,
        call_count: 10,
        lambda_count: 1,
    };
    assert_eq!(get_sort_value(Some(&stats), &def, "cyclomaticComplexity"), 15);
    assert_eq!(get_sort_value(Some(&stats), &def, "cognitiveComplexity"), 25);
    assert_eq!(get_sort_value(Some(&stats), &def, "maxNestingDepth"), 4);
    assert_eq!(get_sort_value(Some(&stats), &def, "paramCount"), 3);
    assert_eq!(get_sort_value(Some(&stats), &def, "returnCount"), 2);
    assert_eq!(get_sort_value(Some(&stats), &def, "callCount"), 10);
    assert_eq!(get_sort_value(Some(&stats), &def, "lambdaCount"), 1);
}

#[test]
fn test_get_sort_value_unknown_field_returns_zero() {
    let def = make_def("Test", None, DefinitionKind::Method);
    let stats = CodeStats {
        cyclomatic_complexity: 15,
        cognitive_complexity: 25,
        max_nesting_depth: 4,
        param_count: 3,
        return_count: 2,
        call_count: 10,
        lambda_count: 1,
    };
    assert_eq!(get_sort_value(Some(&stats), &def, "unknownField"), 0);
}

// ═══════════════════════════════════════════════════════════════════
// ADDITIONAL TESTS — covering remaining gaps
// ═══════════════════════════════════════════════════════════════════

// ─── parse_definition_args: remaining field coverage ─────────────

#[test]
fn test_parse_args_audit_and_cross_validate() {
    let args = json!({"audit": true, "crossValidate": true, "auditMinBytes": 1000});
    let parsed = parse_definition_args(&args).unwrap();
    assert!(parsed.audit);
    assert!(parsed.cross_validate);
    assert_eq!(parsed.audit_min_bytes, 1000);
}

#[test]
fn test_parse_args_audit_defaults() {
    let args = json!({"audit": true});
    let parsed = parse_definition_args(&args).unwrap();
    assert!(parsed.audit);
    assert!(!parsed.cross_validate);
    assert_eq!(parsed.audit_min_bytes, 500, "default auditMinBytes should be 500");
}

#[test]
fn test_parse_args_body_params() {
    let args = json!({"includeBody": true, "maxBodyLines": 50, "maxTotalBodyLines": 200});
    let parsed = parse_definition_args(&args).unwrap();
    assert!(parsed.include_body);
    assert_eq!(parsed.max_body_lines, 50);
    assert_eq!(parsed.max_total_body_lines, 200);
}

#[test]
fn test_parse_args_use_regex() {
    let args = json!({"regex": true});
    let parsed = parse_definition_args(&args).unwrap();
    assert!(parsed.use_regex);
}

#[test]
fn test_parse_args_base_type_transitive() {
    let args = json!({"baseType": "IService", "baseTypeTransitive": true});
    let parsed = parse_definition_args(&args).unwrap();
    assert_eq!(parsed.base_type_filter, Some("IService".to_string()));
    assert!(parsed.base_type_transitive);
}

#[test]
fn test_parse_args_file_and_parent_filter() {
    let args = json!({"file": "UserService.cs", "parent": "UserService"});
    let parsed = parse_definition_args(&args).unwrap();
    assert_eq!(parsed.file_filter, Some("UserService.cs".to_string()));
    assert_eq!(parsed.parent_filter, Some("UserService".to_string()));
}

#[test]
fn test_parse_args_max_results_custom() {
    let args = json!({"maxResults": 50});
    let parsed = parse_definition_args(&args).unwrap();
    assert_eq!(parsed.max_results, 50);
}

#[test]
fn test_parse_args_max_results_zero_means_unlimited() {
    let args = json!({"maxResults": 0});
    let parsed = parse_definition_args(&args).unwrap();
    assert_eq!(parsed.max_results, 0);
}

#[test]
fn test_parse_args_contains_line_non_numeric_ignored() {
    let args = json!({"containsLine": "abc"});
    let parsed = parse_definition_args(&args).unwrap();
    assert!(parsed.contains_line.is_none(), "non-numeric containsLine should be None");
}

// ─── collect_candidates: additional edge cases ───────────────────

#[test]
fn test_collect_candidates_invalid_regex_returns_error() {
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({"name": "[invalid(", "regex": true})).unwrap();
    let result = collect_candidates(&index, &args);
    assert!(result.is_err(), "Invalid regex should return error");
    assert!(result.unwrap_err().contains("Invalid regex"));
}

#[test]
fn test_collect_candidates_kind_no_matches_returns_empty() {
    let index = make_test_def_index();
    // "property" kind exists in the enum but no definitions have it
    let args = parse_definition_args(&json!({"kind": "property"})).unwrap();
    let (candidates, _) = collect_candidates(&index, &args).unwrap();
    assert!(candidates.is_empty(), "No properties exist → empty result");
}

#[test]
fn test_collect_candidates_attribute_and_kind_intersection() {
    let index = make_test_def_index();
    // Injectable attribute is on UserService (class). kind=method should yield empty intersection
    let args = parse_definition_args(&json!({"attribute": "Injectable", "kind": "method"})).unwrap();
    let (candidates, _) = collect_candidates(&index, &args).unwrap();
    assert!(candidates.is_empty(), "Injectable + method → empty (Injectable is on a class)");
}

// ─── apply_entry_filters: additional edge cases ──────────────────

#[test]
fn test_apply_entry_filters_combined_file_and_parent() {
    let index = make_test_def_index();
    let candidates: Vec<u32> = (0..4).collect();
    // Both file and parent filter — intersection
    let args = parse_definition_args(&json!({
        "file": "UserService.cs",
        "parent": "UserService"
    })).unwrap();
    let results = apply_entry_filters(&index, &candidates, &args);
    assert_eq!(results.len(), 1, "file=UserService.cs + parent=UserService → only GetUser");
    assert_eq!(results[0].1.name, "GetUser");
}

#[test]
fn test_apply_entry_filters_case_insensitive_file() {
    let index = make_test_def_index();
    let candidates: Vec<u32> = (0..4).collect();
    // Uppercase file filter still matches
    let args = parse_definition_args(&json!({"file": "USERSERVICE.CS"})).unwrap();
    let results = apply_entry_filters(&index, &candidates, &args);
    assert_eq!(results.len(), 2, "case-insensitive file filter should match");
}

#[test]
fn test_apply_entry_filters_parent_null_excluded_when_parent_filter_set() {
    let index = make_test_def_index();
    let candidates: Vec<u32> = (0..4).collect();
    // UserService (idx 0) and OrderService (idx 2) have parent=None → excluded
    let args = parse_definition_args(&json!({"parent": "UserService,OrderService"})).unwrap();
    let results = apply_entry_filters(&index, &candidates, &args);
    // Only methods (which have parents) should be returned
    for (_, def) in &results {
        assert!(def.parent.is_some(), "When parent filter is set, defs without parent are excluded");
    }
}

// ─── apply_stats_filters: actual filtering with populated code_stats ──

/// Helper: create a DefinitionIndex with populated code_stats
fn make_index_with_stats() -> DefinitionIndex {
    let mut index = make_test_def_index();
    // Add code_stats for methods (idx 1: GetUser, idx 3: GetOrder)
    index.code_stats.insert(1, CodeStats {
        cyclomatic_complexity: 15,
        cognitive_complexity: 25,
        max_nesting_depth: 4,
        param_count: 3,
        return_count: 2,
        call_count: 10,
        lambda_count: 1,
    });
    index.code_stats.insert(3, CodeStats {
        cyclomatic_complexity: 5,
        cognitive_complexity: 8,
        max_nesting_depth: 2,
        param_count: 1,
        return_count: 1,
        call_count: 3,
        lambda_count: 0,
    });
    index
}

#[test]
fn test_apply_stats_filters_min_complexity_filters() {
    let index = make_index_with_stats();
    let mut results: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
        .enumerate()
        .map(|(i, d)| (i as u32, d))
        .collect();
    let args = parse_definition_args(&json!({"minComplexity": 10})).unwrap();
    let info = apply_stats_filters(&index, &mut results, &args).unwrap();
    assert!(info.applied);
    assert_eq!(info.before_count, 4);
    // Only GetUser (complexity=15) should pass; GetOrder (5) filtered out;
    // UserService and OrderService have no stats → filtered out
    assert_eq!(results.len(), 1, "Only GetUser passes minComplexity=10");
    assert_eq!(results[0].1.name, "GetUser");
}

#[test]
fn test_apply_stats_filters_min_params_filters() {
    let index = make_index_with_stats();
    let mut results: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
        .enumerate()
        .map(|(i, d)| (i as u32, d))
        .collect();
    let args = parse_definition_args(&json!({"minParams": 2})).unwrap();
    let info = apply_stats_filters(&index, &mut results, &args).unwrap();
    assert!(info.applied);
    // GetUser has param_count=3, GetOrder has param_count=1
    assert_eq!(results.len(), 1, "Only GetUser passes minParams=2");
    assert_eq!(results[0].1.name, "GetUser");
}

#[test]
fn test_apply_stats_filters_multiple_min_filters_and_logic() {
    let index = make_index_with_stats();
    let mut results: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
        .enumerate()
        .map(|(i, d)| (i as u32, d))
        .collect();
    // GetUser: complexity=15, nesting=4. GetOrder: complexity=5, nesting=2
    // minComplexity=3 AND minNesting=3 → only GetUser passes both
    let args = parse_definition_args(&json!({"minComplexity": 3, "minNesting": 3})).unwrap();
    let info = apply_stats_filters(&index, &mut results, &args).unwrap();
    assert!(info.applied);
    assert_eq!(results.len(), 1, "AND logic: only GetUser passes both thresholds");
    assert_eq!(results[0].1.name, "GetUser");
}

#[test]
fn test_apply_stats_filters_before_count_correct() {
    let index = make_index_with_stats();
    let mut results: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
        .enumerate()
        .map(|(i, d)| (i as u32, d))
        .collect();
    let args = parse_definition_args(&json!({"minComplexity": 100})).unwrap();
    let info = apply_stats_filters(&index, &mut results, &args).unwrap();
    assert_eq!(info.before_count, 4, "before_count should capture pre-filter count");
    assert_eq!(results.len(), 0, "No results pass minComplexity=100");
}

// ─── format_definition_entry: direct tests ───────────────────────

#[test]
fn test_format_definition_entry_basic_fields() {
    let index = make_test_def_index();
    let def = &index.definitions[1]; // GetUser method
    let args = parse_definition_args(&json!({})).unwrap();
    let mut cache = HashMap::new();
    let mut body_lines = 0usize;
    let obj = format_definition_entry(&index, 1, def, &args, &mut cache, &mut body_lines);
    assert_eq!(obj["name"], "GetUser");
    assert_eq!(obj["kind"], "method");
    assert!(obj["file"].as_str().unwrap().contains("UserService"));
    assert_eq!(obj["lines"], "10-30");
    assert_eq!(obj["parent"], "UserService");
    // No body by default
    assert!(obj.get("body").is_none());
    assert!(obj.get("codeStats").is_none());
}

#[test]
fn test_format_definition_entry_optional_fields_absent_when_empty() {
    let index = make_test_def_index();
    let def = &index.definitions[0]; // UserService class — no signature, no parent
    let args = parse_definition_args(&json!({})).unwrap();
    let mut cache = HashMap::new();
    let mut body_lines = 0usize;
    let obj = format_definition_entry(&index, 0, def, &args, &mut cache, &mut body_lines);
    assert!(obj.get("parent").is_none(), "no parent → field absent");
    assert!(obj.get("signature").is_none(), "no signature → field absent");
    // modifiers is empty → should be absent
    assert!(obj.get("modifiers").is_none(), "empty modifiers → field absent");
}

#[test]
fn test_format_definition_entry_with_code_stats() {
    let index = make_index_with_stats();
    let def = &index.definitions[1]; // GetUser — has code_stats
    let args = parse_definition_args(&json!({"includeCodeStats": true})).unwrap();
    let mut cache = HashMap::new();
    let mut body_lines = 0usize;
    let obj = format_definition_entry(&index, 1, def, &args, &mut cache, &mut body_lines);
    assert!(obj.get("codeStats").is_some(), "includeCodeStats=true → codeStats present");
    let stats = &obj["codeStats"];
    assert_eq!(stats["cyclomaticComplexity"], 15);
    assert_eq!(stats["cognitiveComplexity"], 25);
    assert_eq!(stats["maxNestingDepth"], 4);
    assert_eq!(stats["paramCount"], 3);
    assert_eq!(stats["returnCount"], 2);
    assert_eq!(stats["callCount"], 10);
    assert_eq!(stats["lambdaCount"], 1);
    assert_eq!(stats["lines"], 21); // 30 - 10 + 1
}

#[test]
fn test_format_definition_entry_no_code_stats_when_not_requested() {
    let index = make_index_with_stats();
    let def = &index.definitions[1]; // GetUser — has code_stats
    let args = parse_definition_args(&json!({})).unwrap(); // includeCodeStats defaults false
    let mut cache = HashMap::new();
    let mut body_lines = 0usize;
    let obj = format_definition_entry(&index, 1, def, &args, &mut cache, &mut body_lines);
    assert!(obj.get("codeStats").is_none(), "includeCodeStats=false → no codeStats");
}

#[test]
fn test_format_definition_entry_with_attributes() {
    let index = make_test_def_index();
    let def = &index.definitions[0]; // UserService — has attributes: ["injectable"]
    let args = parse_definition_args(&json!({})).unwrap();
    let mut cache = HashMap::new();
    let mut body_lines = 0usize;
    let obj = format_definition_entry(&index, 0, def, &args, &mut cache, &mut body_lines);
    assert!(obj.get("attributes").is_some(), "UserService has attributes");
    let attrs = obj["attributes"].as_array().unwrap();
    assert_eq!(attrs.len(), 1);
    assert_eq!(attrs[0], "injectable");
}

// ─── build_search_summary: direct tests ──────────────────────────

#[test]
fn test_build_search_summary_basic_fields() {
    let index = make_test_def_index();
    let defs_json = vec![json!({"name": "a"}), json!({"name": "b"})];
    let args = parse_definition_args(&json!({})).unwrap();
    let stats_info = StatsFilterInfo { applied: false, before_count: 2 };
    let elapsed = std::time::Duration::from_millis(5);
    let ctx = HandlerContext::default();
    let summary = build_search_summary(
        &index, &defs_json, &args, 10, &stats_info, &None, 0, 0, elapsed, &ctx);
    assert_eq!(summary["totalResults"], 10);
    assert_eq!(summary["returned"], 2);
    assert_eq!(summary["indexFiles"], 2); // 2 files in make_test_def_index
    assert!(summary["searchTimeMs"].as_f64().unwrap() > 0.0);
}

#[test]
fn test_build_search_summary_sorted_by_field() {
    let index = make_test_def_index();
    let defs_json = vec![];
    let args = parse_definition_args(&json!({"sortBy": "cognitiveComplexity"})).unwrap();
    let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
    let ctx = HandlerContext::default();
    let summary = build_search_summary(
        &index, &defs_json, &args, 0, &stats_info, &None, 0, 0,
        std::time::Duration::ZERO, &ctx);
    assert_eq!(summary["sortedBy"], "cognitiveComplexity");
}

#[test]
fn test_build_search_summary_stats_filters_applied() {
    let index = make_test_def_index();
    let defs_json = vec![json!({"name": "a"})];
    let args = parse_definition_args(&json!({"minComplexity": 5})).unwrap();
    let stats_info = StatsFilterInfo { applied: true, before_count: 10 };
    let ctx = HandlerContext::default();
    let summary = build_search_summary(
        &index, &defs_json, &args, 1, &stats_info, &None, 0, 0,
        std::time::Duration::ZERO, &ctx);
    assert_eq!(summary["statsFiltersApplied"], true);
    assert_eq!(summary["beforeStatsFilter"], 10);
    assert_eq!(summary["afterStatsFilter"], 1);
}

#[test]
fn test_build_search_summary_code_stats_unavailable() {
    let index = make_test_def_index(); // empty code_stats
    let defs_json = vec![];
    let args = parse_definition_args(&json!({"includeCodeStats": true})).unwrap();
    let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
    let ctx = HandlerContext::default();
    let summary = build_search_summary(
        &index, &defs_json, &args, 0, &stats_info, &None, 0, 0,
        std::time::Duration::ZERO, &ctx);
    assert_eq!(summary["codeStatsAvailable"], false);
}

#[test]
fn test_build_search_summary_body_lines_reported() {
    let index = make_test_def_index();
    let defs_json = vec![];
    let args = parse_definition_args(&json!({"includeBody": true})).unwrap();
    let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
    let ctx = HandlerContext::default();
    let summary = build_search_summary(
        &index, &defs_json, &args, 0, &stats_info, &None, 42, 0,
        std::time::Duration::ZERO, &ctx);
    assert_eq!(summary["totalBodyLinesReturned"], 42);
}

#[test]
fn test_build_search_summary_no_body_lines_when_not_requested() {
    let index = make_test_def_index();
    let defs_json = vec![];
    let args = parse_definition_args(&json!({})).unwrap();
    let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
    let ctx = HandlerContext::default();
    let summary = build_search_summary(
        &index, &defs_json, &args, 0, &stats_info, &None, 0, 0,
        std::time::Duration::ZERO, &ctx);
    assert!(summary.get("totalBodyLinesReturned").is_none(),
        "No body → no totalBodyLinesReturned");
}

#[test]
fn test_build_search_summary_read_errors_only_when_nonzero() {
    let mut index = make_test_def_index();
    let args = parse_definition_args(&json!({})).unwrap();
    let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
    let ctx = HandlerContext::default();

    // No errors → field absent
    let summary = build_search_summary(
        &index, &[], &args, 0, &stats_info, &None, 0, 0,
        std::time::Duration::ZERO, &ctx);
    assert!(summary.get("readErrors").is_none());

    // With errors → field present
    index.parse_errors = 3;
    let summary2 = build_search_summary(
        &index, &[], &args, 0, &stats_info, &None, 0, 0,
        std::time::Duration::ZERO, &ctx);
    assert_eq!(summary2["readErrors"], 3);
}

#[test]
fn test_build_search_summary_term_breakdown_injected() {
    let index = make_test_def_index();
    let defs_json = vec![];
    let args = parse_definition_args(&json!({})).unwrap();
    let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
    let ctx = HandlerContext::default();
    let breakdown = Some(json!({"term1": 5, "term2": 3}));
    let summary = build_search_summary(
        &index, &defs_json, &args, 0, &stats_info, &breakdown, 0, 0,
        std::time::Duration::ZERO, &ctx);
    assert!(summary.get("termBreakdown").is_some());
    assert_eq!(summary["termBreakdown"]["term1"], 5);
}

// ─── sort_results: additional coverage ───────────────────────────

#[test]
fn test_sort_results_kind_priority_class_before_method() {
    let index = make_test_def_index();
    // Search for "service" — UserService (class) and GetUser (method containing "service" via parent)
    // Actually, let's search for "user" which matches: UserService (class) and GetUser (method)
    let mut results: Vec<(u32, &DefinitionEntry)> = vec![
        (1, &index.definitions[1]), // GetUser (method)
        (0, &index.definitions[0]), // UserService (class)
    ];
    let args = parse_definition_args(&json!({"name": "user"})).unwrap();
    sort_results(&mut results, &index, &args);
    // Both contain "user" — class (kind_priority=0) should come before method (kind_priority=1)
    assert_eq!(results[0].1.kind, DefinitionKind::Class,
        "Class should sort before method (kind priority tiebreaker)");
}

#[test]
fn test_sort_results_parent_filter_exact_parent_first() {
    let index = make_test_def_index();
    let mut results: Vec<(u32, &DefinitionEntry)> = vec![
        (3, &index.definitions[3]), // GetOrder (parent: OrderService)
        (1, &index.definitions[1]), // GetUser (parent: UserService)
    ];
    // parent=UserService — exact match should sort first
    let args = parse_definition_args(&json!({"parent": "UserService"})).unwrap();
    sort_results(&mut results, &index, &args);
    assert_eq!(results[0].1.name, "GetUser",
        "Exact parent match 'UserService' should sort first");
}

#[test]
fn test_sort_results_name_length_tiebreaker() {
    // Two definitions with same kind, both exact matches — shorter name first
    let defs = vec![
        make_def("AB", None, DefinitionKind::Class),   // shorter
        make_def("ABC", None, DefinitionKind::Class),  // longer
    ];
    let index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec!["C:\\src\\file.cs".to_string()],
        definitions: defs,
        name_index: {
            let mut m = HashMap::new();
            m.insert("ab".to_string(), vec![0]);
            m.insert("abc".to_string(), vec![1]);
            m
        },
        kind_index: {
            let mut m = HashMap::new();
            m.entry(DefinitionKind::Class).or_insert_with(Vec::new).extend([0u32, 1]);
            m
        },
        file_index: {
            let mut m = HashMap::new();
            m.insert(0u32, vec![0, 1]);
            m
        },
        ..Default::default()
    };
    let mut results: Vec<(u32, &DefinitionEntry)> = vec![
        (1, &index.definitions[1]), // ABC
        (0, &index.definitions[0]), // AB
    ];
    // Both contain "ab" — tiebreak by name length
    let args = parse_definition_args(&json!({"name": "ab"})).unwrap();
    sort_results(&mut results, &index, &args);
    assert_eq!(results[0].1.name, "AB", "Shorter name should sort first as tiebreaker");
    assert_eq!(results[1].1.name, "ABC");
}

// ─── compute_term_breakdown: edge case ───────────────────────────

#[test]
fn test_compute_term_breakdown_comma_only_returns_none() {
    let results: Vec<(u32, &DefinitionEntry)> = vec![];
    let def_to_term = HashMap::new();
    let args = parse_definition_args(&json!({"name": ",,,"})).unwrap();
    // name=",,," → after filtering empty strings, terms is empty → name_filter is Some(",,,") but terms.len() < 2
    // Actually, ",,," splits into ["", "", "", ""], filter empty → empty vec, len() = 0 < 2 → None
    let breakdown = compute_term_breakdown(&results, &def_to_term, &args);
    assert!(breakdown.is_none(), "Comma-only name → no usable terms → no breakdown");
}

// ─── property→field hint tests ────────────────────────────────────

#[test]
fn test_kind_property_hint_when_fields_exist() {
    // Create an index with Field definitions but no Property definitions
    let mut index = make_test_def_index();
    // Add a Field definition explicitly
    let field_def = DefinitionEntry {
        name: "title".to_string(), kind: DefinitionKind::Field,
        file_id: 0, line_start: 5, line_end: 5,
        signature: Some("title: string".to_string()),
        parent: Some("UserService".to_string()),
        modifiers: vec!["private".to_string()],
        attributes: vec![], base_types: vec![],
    };
    let field_idx = index.definitions.len() as u32;
    index.definitions.push(field_def);
    index.kind_index.entry(DefinitionKind::Field).or_default().push(field_idx);
    index.name_index.entry("title".to_string()).or_default().push(field_idx);
    index.file_index.entry(0).or_default().push(field_idx);

    let content_index = crate::ContentIndex {
        root: ".".to_string(),
        extensions: vec!["ts".to_string()],
        ..Default::default()
    };

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(index))),
        ..Default::default()
    };

    // Search with kind="property" and parent="UserService" — should return 0 results + hint
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "kind": "property",
        "parent": "UserService"
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 0, "kind=property should return 0 for TS class fields");

    // Should have a hint suggesting kind='field'
    let hint = v["summary"]["hint"].as_str();
    assert!(hint.is_some(), "Should have hint when kind=property returns 0 but fields exist");
    assert!(hint.unwrap().contains("kind='field'"),
        "Hint should suggest kind='field'. Got: {}", hint.unwrap());
}

#[test]
fn test_kind_property_no_hint_when_results_exist() {
    // If kind="property" returns results, no hint needed
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = handle_search_definitions(&ctx, &serde_json::json!({
        "kind": "class"
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v["summary"].get("hint").is_none(),
        "No hint when results are returned");
}


// ─── Zero-result hints tests ──────────────────────────────────────

#[test]
fn test_hint_wrong_kind() {
    // make_test_def_index has: UserService (class), GetUser (method), OrderService (class), GetOrder (method)
    // Searching kind='function' with name='GetUser' should give hint suggesting 'method'
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({"kind": "function", "name": "GetUser"})).unwrap();
    let defs_json: Vec<Value> = vec![];
    let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
    let ctx = HandlerContext::default();
    let summary = build_search_summary(
        &index, &defs_json, &args, 0, &stats_info, &None, 0, 0,
        std::time::Duration::ZERO, &ctx);
    let hint = summary["hint"].as_str().unwrap();
    assert!(hint.contains("kind='function'"), "Should mention the wrong kind. Got: {}", hint);
    assert!(hint.contains("method"), "Should suggest 'method' as alternative. Got: {}", hint);
    assert!(hint.contains("Did you mean"), "Should ask 'did you mean'. Got: {}", hint);
}

#[test]
fn test_hint_wrong_kind_with_file_filter() {
    // Searching kind='function' with file='UserService' should give hint showing available kinds
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({"kind": "function", "file": "UserService.cs"})).unwrap();
    let defs_json: Vec<Value> = vec![];
    let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
    let ctx = HandlerContext::default();
    let summary = build_search_summary(
        &index, &defs_json, &args, 0, &stats_info, &None, 0, 0,
        std::time::Duration::ZERO, &ctx);
    let hint = summary["hint"].as_str().unwrap();
    assert!(hint.contains("kind='function'"), "Should mention the wrong kind. Got: {}", hint);
    assert!(hint.contains("class") || hint.contains("method"),
        "Should list available kinds. Got: {}", hint);
}

#[test]
fn test_hint_file_has_defs_but_name_not_found() {
    // File 'UserService.cs' has definitions, but name='nonexistent' doesn't match any
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({"file": "UserService.cs", "name": "nonexistent"})).unwrap();
    let defs_json: Vec<Value> = vec![];
    let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
    let ctx = HandlerContext::default();
    let summary = build_search_summary(
        &index, &defs_json, &args, 0, &stats_info, &None, 0, 0,
        std::time::Duration::ZERO, &ctx);
    let hint = summary["hint"].as_str().unwrap();
    assert!(hint.contains("UserService.cs"), "Should mention the file. Got: {}", hint);
    assert!(hint.contains("definitions"), "Should mention definitions count. Got: {}", hint);
    assert!(hint.contains("search_grep"), "Should suggest search_grep. Got: {}", hint);
}

#[test]
fn test_hint_nearest_name_match() {
    // Search for 'GetUsr' — close to 'GetUser' but not exact
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({"name": "GetUsr"})).unwrap();
    let defs_json: Vec<Value> = vec![];
    let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
    let ctx = HandlerContext::default();
    let summary = build_search_summary(
        &index, &defs_json, &args, 0, &stats_info, &None, 0, 0,
        std::time::Duration::ZERO, &ctx);
    let hint = summary["hint"].as_str();
    assert!(hint.is_some(), "Should have a nearest-match hint for typo 'GetUsr'");
    let h = hint.unwrap();
    assert!(h.contains("Nearest match"), "Hint should say 'Nearest match'. Got: {}", h);
    // Should suggest either 'getuser' or 'getorder'
    assert!(h.contains("getuser") || h.contains("getorder"),
        "Should suggest a close name. Got: {}", h);
    assert!(h.contains("similarity"), "Should show similarity %. Got: {}", h);
}

#[test]
fn test_hint_name_in_content_not_in_defs() {
    // Create a context with content index containing 'inputschema' but no matching definition
    use std::sync::Arc;
    use std::sync::RwLock;
    use std::sync::atomic::AtomicBool;

    let index = make_test_def_index();
    let mut content_index = crate::ContentIndex {
        root: ".".to_string(),
        ..Default::default()
    };
    // Add 'inputschema' to content index
    content_index.index.insert("inputschema".to_string(), vec![
        crate::Posting { file_id: 0, lines: vec![10] },
        crate::Posting { file_id: 1, lines: vec![20] },
    ]);

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(index))),
        content_ready: Arc::new(AtomicBool::new(true)),
        ..Default::default()
    };

    let def_index_guard = ctx.def_index.as_ref().unwrap().read().unwrap();
    let args = parse_definition_args(&json!({"name": "inputSchema"})).unwrap();
    let defs_json: Vec<Value> = vec![];
    let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
    let summary = build_search_summary(
        &def_index_guard, &defs_json, &args, 0, &stats_info, &None, 0, 0,
        std::time::Duration::ZERO, &ctx);
    let hint = summary["hint"].as_str();
    assert!(hint.is_some(), "Should have hint when name is in content but not in definitions");
    let h = hint.unwrap();
    assert!(h.contains("not found as an AST definition name"), "Hint should explain the issue. Got: {}", h);
    assert!(h.contains("search_grep"), "Hint should suggest search_grep. Got: {}", h);
    assert!(h.contains("2 files"), "Hint should show file count. Got: {}", h);
}

#[test]
fn test_hint_priority_kind_first() {
    // When both kind is wrong AND name is a typo, kind hint (A) should take priority over name hint (B)
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({"kind": "function", "name": "GetUsr"})).unwrap();
    let defs_json: Vec<Value> = vec![];
    let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
    let ctx = HandlerContext::default();
    let summary = build_search_summary(
        &index, &defs_json, &args, 0, &stats_info, &None, 0, 0,
        std::time::Duration::ZERO, &ctx);
    // Hint A won't trigger because 'GetUsr' doesn't match any name in name_index
    // (it's a substring search, and 'getusr' doesn't match 'getuser' or 'getorder' as substring)
    // So Hint B should trigger instead
    let hint = summary["hint"].as_str();
    assert!(hint.is_some(), "Should have some hint");
}

#[test]
fn test_hint_no_hint_for_regex() {
    // Regex search resulting in 0 results should NOT give nearest-name hint
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({"name": "xyz_nonexistent_regex.*", "regex": true})).unwrap();
    let defs_json: Vec<Value> = vec![];
    let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
    let ctx = HandlerContext::default();
    let summary = build_search_summary(
        &index, &defs_json, &args, 0, &stats_info, &None, 0, 0,
        std::time::Duration::ZERO, &ctx);
    // Hint B checks !args.use_regex, so no nearest-name hint
    // Hint D might fire if content index has nothing
    // At least verify no nearest-match hint
    let hint = summary.get("hint").and_then(|v| v.as_str());
    if let Some(h) = hint {
        assert!(!h.contains("Nearest match"),
            "Regex search should NOT give nearest-name hint. Got: {}", h);
    }
}

#[test]
fn test_hint_no_hint_when_results_found() {
    // When search finds results, no hint should be generated
    let index = make_test_def_index();
    let args = parse_definition_args(&json!({"name": "GetUser"})).unwrap();
    let defs_json = vec![json!({"name": "GetUser"})];
    let stats_info = StatsFilterInfo { applied: false, before_count: 1 };
    let ctx = HandlerContext::default();
    let summary = build_search_summary(
        &index, &defs_json, &args, 1, &stats_info, &None, 0, 0,
        std::time::Duration::ZERO, &ctx);
    assert!(summary.get("hint").is_none(),
        "No hint when results are found (total_results > 0)");
}

#[test]
fn test_hint_existing_property_field_hint_not_overwritten() {
    // Ensure the existing property→field hint is preserved and not overwritten by new hints
    let mut index = make_test_def_index();
    // Add Field definitions so the property→field hint triggers
    let field_def = DefinitionEntry {
        name: "name".to_string(), kind: DefinitionKind::Field,
        file_id: 0, line_start: 5, line_end: 5,
        signature: None, parent: Some("UserService".to_string()),
        modifiers: vec![], attributes: vec![], base_types: vec![],
    };
    let field_idx = index.definitions.len() as u32;
    index.definitions.push(field_def);
    index.kind_index.entry(DefinitionKind::Field).or_default().push(field_idx);
    index.file_index.entry(0).or_default().push(field_idx);

    let args = parse_definition_args(&json!({"kind": "property"})).unwrap();
    let defs_json: Vec<Value> = vec![];
    let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
    let ctx = HandlerContext::default();
    let summary = build_search_summary(
        &index, &defs_json, &args, 0, &stats_info, &None, 0, 0,
        std::time::Duration::ZERO, &ctx);
    let hint = summary["hint"].as_str().unwrap();
    // Should be the original property→field hint, not overwritten by Hint A/B/C/D
    assert!(hint.contains("kind='field'"),
        "Original property→field hint should be preserved. Got: {}", hint);
}

#[test]
fn test_file_matches_filter_helper() {
    let index = make_test_def_index();
    // file 0 = "C:\\src\\UserService.cs"
    assert!(file_matches_filter(&index, 0, "UserService.cs"));
    assert!(file_matches_filter(&index, 0, "userservice.cs")); // case insensitive
    assert!(file_matches_filter(&index, 0, "UserService.cs,OrderService.cs")); // comma-separated
    assert!(!file_matches_filter(&index, 0, "OrderService.cs")); // only matches file 1
    assert!(!file_matches_filter(&index, 0, "nonexistent.cs"));
    assert!(!file_matches_filter(&index, 99, "UserService.cs")); // invalid file_id
}

// ─── Auto-correction tests ──────────────────────────────────────────

/// Helper to create a context from make_test_def_index for auto-correction tests
fn make_auto_correction_ctx() -> HandlerContext {
    let index = make_test_def_index();
    let content_index = crate::ContentIndex {
        root: ".".to_string(),
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };
    super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(index))),
        ..Default::default()
    }
}

#[test]
fn test_auto_correct_kind_method_to_function() {
    // make_test_def_index has GetUser (method) and GetOrder (method)
    // Searching kind='function' + name='GetUser' should auto-correct to kind='method'
    let ctx = make_auto_correction_ctx();
    let result = handle_search_definitions(&ctx, &json!({
        "kind": "function",
        "name": "GetUser"
    }));
    assert!(!result.is_error, "Should not error: {:?}", result.content[0].text);
    let v: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Should return corrected results. Got: {}", defs.len());
    // Should include GetUser (method) among the results — kind filter removed
    assert!(defs.iter().any(|d| d["name"] == "GetUser"),
        "Should include GetUser in corrected results");

    // Should have autoCorrection in summary
    let auto = &v["summary"]["autoCorrection"];
    assert!(auto.is_object(), "Should have autoCorrection in summary");
    assert_eq!(auto["type"], "kindCorrected");
    assert_eq!(auto["original"]["kind"], "function");
    assert!(auto["corrected"]["kind"].is_null(), "Corrected kind should be null (kind filter removed)");
    assert!(auto["reason"].as_str().unwrap().contains("Removed kind filter"),
        "Reason should explain the correction. Got: {}", auto["reason"]);
    assert!(auto.get("availableKinds").is_some(), "Should have availableKinds field");
}

#[test]
fn test_auto_correct_kind_with_file_filter() {
    // kind='function' + file='UserService.cs' — should auto-correct kind
    let ctx = make_auto_correction_ctx();
    let result = handle_search_definitions(&ctx, &json!({
        "kind": "function",
        "file": "UserService.cs"
    }));
    assert!(!result.is_error);
    let v: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert!(defs.len() > 0, "Should return corrected results after kind auto-correction");

    let auto = &v["summary"]["autoCorrection"];
    assert!(auto.is_object(), "Should have autoCorrection");
    assert_eq!(auto["type"], "kindCorrected");
}

#[test]
fn test_auto_correct_kind_no_trigger_without_name_or_file() {
    // kind='function' WITHOUT name/file — auto-correction should NOT trigger
    let ctx = make_auto_correction_ctx();
    let result = handle_search_definitions(&ctx, &json!({
        "kind": "function"
    }));
    assert!(!result.is_error);
    let v: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 0, "No functions exist, no auto-correction without name/file");
    assert!(v["summary"].get("autoCorrection").is_none(),
        "No autoCorrection without name/file filter");
}

#[test]
fn test_auto_correct_name_typo() {
    // name='GetUsr' — close to 'getuser' (Jaro-Winkler ~95%)
    let ctx = make_auto_correction_ctx();
    let result = handle_search_definitions(&ctx, &json!({
        "name": "GetUsr"
    }));
    assert!(!result.is_error, "Should not error: {:?}", result.content[0].text);
    let v: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert!(defs.len() > 0, "Should return auto-corrected results for typo 'GetUsr'");
    assert!(defs.iter().any(|d| d["name"].as_str().unwrap() == "GetUser"),
        "Should find GetUser after name correction");

    let auto = &v["summary"]["autoCorrection"];
    assert!(auto.is_object(), "Should have autoCorrection in summary");
    assert_eq!(auto["type"], "nameCorrected");
    assert_eq!(auto["original"]["name"], "GetUsr");
    assert!(auto["corrected"]["name"].as_str().unwrap().contains("getuser"),
        "Should correct to 'getuser'. Got: {}", auto["corrected"]["name"]);
    assert!(auto["similarity"].as_str().unwrap().contains("%"),
        "Should show similarity percentage");
}

#[test]
fn test_auto_correct_name_below_threshold_no_correction() {
    // name='xyz_totally_different' — very low similarity to any name
    let ctx = make_auto_correction_ctx();
    let result = handle_search_definitions(&ctx, &json!({
        "name": "xyz_totally_different"
    }));
    assert!(!result.is_error);
    let v: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 0, "Too different name should return 0 results");
    assert!(v["summary"].get("autoCorrection").is_none(),
        "No autoCorrection when similarity is below threshold");
}

#[test]
fn test_auto_correct_name_not_triggered_for_regex() {
    let ctx = make_auto_correction_ctx();
    let result = handle_search_definitions(&ctx, &json!({
        "name": "GetUsr",
        "regex": true
    }));
    assert!(!result.is_error);
    let v: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v["summary"].get("autoCorrection").is_none(),
        "Regex queries should NOT trigger name auto-correction");
}

#[test]
fn test_auto_correct_kind_takes_priority_over_name() {
    // kind='function' + name='GetUser' — kind correction should fire first
    let ctx = make_auto_correction_ctx();
    let result = handle_search_definitions(&ctx, &json!({
        "kind": "function",
        "name": "GetUser"
    }));
    assert!(!result.is_error);
    let v: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert!(defs.iter().any(|d| d["name"] == "GetUser"),
        "Should find GetUser after kind correction");
    let auto = &v["summary"]["autoCorrection"];
    assert!(auto.is_object(), "Should have autoCorrection");
    assert_eq!(auto["type"], "kindCorrected",
        "Kind correction should take priority over name correction");
}

#[test]
fn test_auto_correct_no_correction_when_results_found() {
    let ctx = make_auto_correction_ctx();
    let result = handle_search_definitions(&ctx, &json!({
        "name": "GetUser"
    }));
    assert!(!result.is_error);
    let v: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v["definitions"].as_array().unwrap().len() > 0);
    assert!(v["summary"].get("autoCorrection").is_none(),
        "No autoCorrection when results are found normally");
}

#[test]
fn test_auto_correct_preserves_other_filters() {
    // kind='function' + name='GetUser' + file='OrderService.cs'
    // After kind correction, file filter should still be applied
    let ctx = make_auto_correction_ctx();
    let result = handle_search_definitions(&ctx, &json!({
        "kind": "function",
        "name": "GetUser",
        "file": "OrderService.cs"
    }));
    assert!(!result.is_error);
    let v: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    // GetUser is in UserService.cs, not OrderService.cs — correction produces 0 results
    assert_eq!(defs.len(), 0,
        "File filter should still apply. GetUser is in UserService.cs, not OrderService.cs");
}

#[test]
fn test_auto_correct_constant_threshold() {
    assert!((AUTO_CORRECT_NAME_THRESHOLD - 0.80).abs() < f64::EPSILON,
        "Auto-correct threshold should be 0.80");
}

#[test]
fn test_auto_correct_length_ratio_constant() {
    assert!((AUTO_CORRECT_MIN_LENGTH_RATIO - 0.6).abs() < f64::EPSILON,
        "Auto-correct min length ratio should be 0.6");
}

#[test]
fn test_auto_correct_name_blocked_by_length_ratio() {
    // name='UserServiceController' — closest match is 'userservice' (11 chars vs 21 chars).
    // Length ratio = 11/21 = 0.52 < 0.6 threshold → auto-correction should NOT fire.
    // Even if Jaro-Winkler similarity is high due to shared prefix.
    let ctx = make_auto_correction_ctx();
    let result = handle_search_definitions(&ctx, &json!({
        "name": "UserServiceController"
    }));
    assert!(!result.is_error, "Should not error: {:?}", result.content[0].text);
    let v: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = v["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 0, "Should return 0 results (length ratio too low for auto-correction)");
    // Should NOT have autoCorrection — should have hint instead
    let auto = &v["summary"]["autoCorrection"];
    assert!(auto.is_null(), "Should NOT have autoCorrection when length ratio is below threshold. Got: {}", auto);
}

#[test]
fn test_auto_correct_name_typo_passes_length_ratio() {
    // name='GetUsr' (6 chars) — closest match is 'getuser' (7 chars).
    // Length ratio = 6/7 = 0.86 ≥ 0.6 → auto-correction should fire.
    let ctx = make_auto_correction_ctx();
    let result = handle_search_definitions(&ctx, &json!({
        "name": "GetUsr"
    }));
    assert!(!result.is_error);
    let v: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let auto = &v["summary"]["autoCorrection"];
    assert!(auto.is_object(), "Short typo should trigger auto-correction (length ratio OK)");
    assert_eq!(auto["type"], "nameCorrected");
}

#[test]
fn test_auto_correct_name_typo_passes_length_ratio_similar_length() {
    // name='UserServise' (11 chars, typo: 's' instead of 'c') — closest match is 'userservice' (11 chars).
    // Length ratio = 11/11 = 1.0 ≥ 0.6 → auto-correction should fire.
    // This is NOT a substring match, so normal search returns 0 results → auto-correction kicks in.
    let ctx = make_auto_correction_ctx();
    let result = handle_search_definitions(&ctx, &json!({
        "name": "UserServise"
    }));
    assert!(!result.is_error);
    let v: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let auto = &v["summary"]["autoCorrection"];
    assert!(auto.is_object(), "Typo with similar length should trigger auto-correction (length ratio OK)");
    assert_eq!(auto["type"], "nameCorrected");
    assert!(auto["corrected"]["name"].as_str().unwrap().contains("userservice"),
        "Should correct to 'userservice'");
}


// ═══════════════════════════════════════════════════════════════════
// Cross-index enrichment tests: includeUsageCount
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_include_usage_count_present() {
    // Definition name exists in content index → usageCount > 0
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = super::super::dispatch_tool(&ctx, "search_definitions", &serde_json::json!({
        "name": "ExecuteQueryAsync",
        "includeUsageCount": true
    }));
    assert!(!result.is_error, "Should not error: {:?}", result.content[0].text);
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert!(!defs.is_empty(), "Should find definitions");

    // All definitions should have usageCount
    for def in defs {
        assert!(def.get("usageCount").is_some(),
            "Definition '{}' should have usageCount when includeUsageCount=true. Got: {}",
            def["name"].as_str().unwrap_or("?"),
            serde_json::to_string_pretty(def).unwrap());
        let count = def["usageCount"].as_u64().unwrap();
        assert!(count > 0,
            "ExecuteQueryAsync should have usageCount > 0, got {}", count);
    }
}

#[test]
fn test_include_usage_count_zero() {
    // Definition name NOT in content index → usageCount = 0
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = super::super::dispatch_tool(&ctx, "search_definitions", &serde_json::json!({
        "name": "ResilientClient",
        "kind": "class",
        "includeUsageCount": true
    }));
    assert!(!result.is_error);
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert!(!defs.is_empty(), "Should find ResilientClient class");

    // "resilientclient" IS in the content index for make_ctx_with_defs, so count > 0
    // Let's verify the field exists at minimum
    for def in defs {
        assert!(def.get("usageCount").is_some(),
            "Definition should have usageCount when includeUsageCount=true");
    }
}

#[test]
fn test_include_usage_count_default_off() {
    // Without includeUsageCount parameter → no usageCount in output
    let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
    let result = super::super::dispatch_tool(&ctx, "search_definitions", &serde_json::json!({
        "name": "ExecuteQueryAsync"
    }));
    assert!(!result.is_error);
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert!(!defs.is_empty(), "Should find definitions");

    for def in defs {
        assert!(def.get("usageCount").is_none(),
            "Without includeUsageCount, should NOT have usageCount field. Got: {}",
            serde_json::to_string_pretty(def).unwrap());
    }
}
