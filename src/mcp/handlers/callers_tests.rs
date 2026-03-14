use super::*;
use crate::definitions::{CallSite, DefinitionEntry, DefinitionIndex, DefinitionKind};
use std::collections::HashMap;


/// Helper: build a minimal DefinitionIndex with given definitions and method_calls.
fn make_def_index(
    definitions: Vec<DefinitionEntry>,
    method_calls: HashMap<u32, Vec<CallSite>>,
) -> DefinitionIndex {
    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();

    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }

    DefinitionIndex {
        root: ".".to_string(),
        created_at: 0,
        extensions: vec!["ts".to_string()],
        files: vec!["src/OrderController.ts".to_string(), "src/OrderValidator.ts".to_string()],
        definitions,
        name_index,
        kind_index,
        attribute_index: HashMap::new(),
        base_type_index: HashMap::new(),
        file_index,
        path_to_id: HashMap::new(),
        method_calls,
        ..Default::default()
    }
}

/// Helper: create a DefinitionEntry for a class.
fn class_def(file_id: u32, name: &str, base_types: Vec<&str>) -> DefinitionEntry {
    DefinitionEntry {
        file_id,
        name: name.to_string(),
        kind: DefinitionKind::Class,
        line_start: 1,
        line_end: 100,
        parent: None,
        signature: None,
        modifiers: vec![],
        attributes: vec![],
        base_types: base_types.into_iter().map(|s| s.to_string()).collect(),
    }
}

/// Helper: create a DefinitionEntry for a method inside a class.
fn method_def(file_id: u32, name: &str, parent: &str, line_start: u32, line_end: u32) -> DefinitionEntry {
    DefinitionEntry {
        file_id,
        name: name.to_string(),
        kind: DefinitionKind::Method,
        line_start,
        line_end,
        parent: Some(parent.to_string()),
        signature: None,
        modifiers: vec![],
        attributes: vec![],
        base_types: vec![],
    }
}

// ─── Test 1: Direct receiver match ──────────────────────────────

#[test]
fn test_verify_call_site_target_direct_match() {
    // OrderController.processOrder() calls validator.validate() at line 25
    // receiver_type = "OrderValidator"
    let definitions = vec![
        class_def(0, "OrderController", vec![]),              // idx 0
        method_def(0, "processOrder", "OrderController", 20, 40), // idx 1
        class_def(1, "OrderValidator", vec![]),                // idx 2
        method_def(1, "validate", "OrderValidator", 10, 30),  // idx 3
    ];

    let mut method_calls = HashMap::new();
    method_calls.insert(1u32, vec![
        CallSite {
            method_name: "validate".to_string(),
            receiver_type: Some("OrderValidator".to_string()),
            line: 25,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);

    // caller_di=1 (processOrder), call_line=25, method="validate", target="OrderValidator"
    assert!(verify_call_site_target(&def_idx, 1, 25, "validate", Some("OrderValidator")));
}

// ─── Test 2: Different receiver → should reject ─────────────────

#[test]
fn test_verify_call_site_target_different_receiver() {
    // OrderController.processOrder() calls path.resolve() at line 25
    // receiver_type = "Path" — target is "DependencyTask", should NOT match
    let definitions = vec![
        class_def(0, "OrderController", vec![]),              // idx 0
        method_def(0, "processOrder", "OrderController", 20, 40), // idx 1
        class_def(1, "DependencyTask", vec![]),               // idx 2
        method_def(1, "resolve", "DependencyTask", 10, 30),  // idx 3
    ];

    let mut method_calls = HashMap::new();
    method_calls.insert(1u32, vec![
        CallSite {
            method_name: "resolve".to_string(),
            receiver_type: Some("Path".to_string()),
            line: 25,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);

    // receiver is "Path" but target class is "DependencyTask" — should return false
    assert!(!verify_call_site_target(&def_idx, 1, 25, "resolve", Some("DependencyTask")));
}

// ─── Test 3: No receiver, same class (implicit this) ────────────

#[test]
fn test_verify_call_site_target_no_receiver_same_class() {
    // OrderValidator.check() calls this.validate() at line 55
    // receiver_type = None (implicit this), caller is in OrderValidator
    let definitions = vec![
        class_def(1, "OrderValidator", vec![]),                // idx 0
        method_def(1, "check", "OrderValidator", 50, 70),     // idx 1
        method_def(1, "validate", "OrderValidator", 10, 30),  // idx 2
    ];

    let mut method_calls = HashMap::new();
    method_calls.insert(1u32, vec![
        CallSite {
            method_name: "validate".to_string(),
            receiver_type: None,
            line: 55,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);

    // caller is in OrderValidator, target is OrderValidator, no receiver → true
    assert!(verify_call_site_target(&def_idx, 1, 55, "validate", Some("OrderValidator")));
}

// ─── Test 4: No receiver, different class ───────────────────────

#[test]
fn test_verify_call_site_target_no_receiver_different_class() {
    // OrderController.processOrder() calls validate() at line 25
    // receiver_type = None, caller is in OrderController, target is OrderValidator
    let definitions = vec![
        class_def(0, "OrderController", vec![]),              // idx 0
        method_def(0, "processOrder", "OrderController", 20, 40), // idx 1
        class_def(1, "OrderValidator", vec![]),                // idx 2
        method_def(1, "validate", "OrderValidator", 10, 30),  // idx 3
    ];

    let mut method_calls = HashMap::new();
    method_calls.insert(1u32, vec![
        CallSite {
            method_name: "validate".to_string(),
            receiver_type: None,
            line: 25,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);

    // caller is in OrderController, target is OrderValidator, no receiver → false
    assert!(!verify_call_site_target(&def_idx, 1, 25, "validate", Some("OrderValidator")));
}

// ─── Test 5: No target class → always accept ────────────────────

#[test]
fn test_verify_call_site_target_no_target_class() {
    let definitions = vec![
        class_def(0, "OrderController", vec![]),              // idx 0
        method_def(0, "processOrder", "OrderController", 20, 40), // idx 1
    ];

    let mut method_calls = HashMap::new();
    method_calls.insert(1u32, vec![
        CallSite {
            method_name: "validate".to_string(),
            receiver_type: Some("SomeRandomClass".to_string()),
            line: 25,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);

    // target_class = None → should always return true (no filtering)
    assert!(verify_call_site_target(&def_idx, 1, 25, "validate", None));
}

// ─── Test 6: No call-site data → graceful fallback (true) ───────

#[test]
fn test_verify_call_site_target_no_call_site_data() {
    let definitions = vec![
        class_def(0, "OrderController", vec![]),              // idx 0
        method_def(0, "processOrder", "OrderController", 20, 40), // idx 1
    ];

    // Empty method_calls — no call-site data for any method
    let method_calls = HashMap::new();

    let def_idx = make_def_index(definitions, method_calls);

    // No call-site data → rejection (parser covers all patterns now)
    assert!(!verify_call_site_target(&def_idx, 1, 25, "validate", Some("OrderValidator")));
}

// ─── Test 7: Interface match (IOrderValidator → OrderValidator) ─

#[test]
fn test_verify_call_site_target_interface_match() {
    // OrderController.processOrder() calls validator.validate() at line 25
    // receiver_type = "IOrderValidator", target_class = "OrderValidator"
    // Should match via interface I-prefix convention
    let definitions = vec![
        class_def(0, "OrderController", vec![]),              // idx 0
        method_def(0, "processOrder", "OrderController", 20, 40), // idx 1
        class_def(1, "OrderValidator", vec!["IOrderValidator"]), // idx 2
        method_def(1, "validate", "OrderValidator", 10, 30),  // idx 3
    ];

    let mut method_calls = HashMap::new();
    method_calls.insert(1u32, vec![
        CallSite {
            method_name: "validate".to_string(),
            receiver_type: Some("IOrderValidator".to_string()),
            line: 25,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);

    // receiver is "IOrderValidator", target is "OrderValidator" → should match via I-prefix
    assert!(verify_call_site_target(&def_idx, 1, 25, "validate", Some("OrderValidator")));
}

// ─── Test 8: Comment line — method has call sites but not at queried line ─

#[test]
fn test_verify_call_site_target_comment_line_not_real_call() {
    // OrderController.processOrder() has a call to endsWith() at line 10
    // but we query for "resolve" at line 5 where no call site exists
    // → content index matched a comment or non-code text → should return false
    let definitions = vec![
        class_def(0, "OrderController", vec![]),              // idx 0
        method_def(0, "processOrder", "OrderController", 1, 20), // idx 1
    ];

    let mut method_calls = HashMap::new();
    method_calls.insert(1u32, vec![
        CallSite {
            method_name: "endsWith".to_string(),
            receiver_type: Some("String".to_string()),
            line: 10,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);

    // Method has call-site data (endsWith at line 10), but no call at line 5
    // → this is a false positive from content index → should return false
    assert!(!verify_call_site_target(&def_idx, 1, 5, "resolve", Some("PathUtils")));
}

// ─── Test 9: Pre-filter does NOT expand by base_types ────────────

#[test]
fn test_prefilter_does_not_expand_by_base_types() {
    // Scenario:
    // - ResourceManager implements IDisposable, has method Dispose
    // - Many files mention "idisposable" (simulating a large codebase)
    // - Only one file actually calls resourceManager.Dispose()
    // - The pre-filter should NOT include all IDisposable files
    //
    // We test this by running build_caller_tree and verifying that
    // only the file with the actual call is in the results.

    use crate::{ContentIndex, Posting};
    use std::sync::atomic::AtomicUsize;
    use std::path::PathBuf;

    // --- Definition Index ---
    // file 0: ResourceManager.cs (defines ResourceManager : IDisposable + Dispose)
    // file 1: Caller.cs (calls resourceManager.Dispose())
    // files 2..11: IDisposable-mentioning files (no actual Dispose call on ResourceManager)

    let definitions = vec![
        // idx 0: class ResourceManager : IDisposable
        DefinitionEntry {
            file_id: 0,
            name: "ResourceManager".to_string(),
            kind: DefinitionKind::Class,
            line_start: 1,
            line_end: 50,
            parent: None,
            signature: None,
            modifiers: vec![],
            attributes: vec![],
            base_types: vec!["IDisposable".to_string()],
        },
        // idx 1: method ResourceManager.Dispose
        DefinitionEntry {
            file_id: 0,
            name: "Dispose".to_string(),
            kind: DefinitionKind::Method,
            line_start: 10,
            line_end: 20,
            parent: Some("ResourceManager".to_string()),
            signature: None,
            modifiers: vec![],
            attributes: vec![],
            base_types: vec![],
        },
        // idx 2: class Caller (in file 1)
        DefinitionEntry {
            file_id: 1,
            name: "Caller".to_string(),
            kind: DefinitionKind::Class,
            line_start: 1,
            line_end: 30,
            parent: None,
            signature: None,
            modifiers: vec![],
            attributes: vec![],
            base_types: vec![],
        },
        // idx 3: method Caller.DoWork (contains the actual call)
        DefinitionEntry {
            file_id: 1,
            name: "DoWork".to_string(),
            kind: DefinitionKind::Method,
            line_start: 5,
            line_end: 25,
            parent: Some("Caller".to_string()),
            signature: None,
            modifiers: vec![],
            attributes: vec![],
            base_types: vec![],
        },
    ];

    // Call site: Caller.DoWork calls resourceManager.Dispose() at line 15
    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(3, vec![
        CallSite {
            method_name: "Dispose".to_string(),
            receiver_type: Some("ResourceManager".to_string()),
            line: 15,
            receiver_is_generic: false,
        },
    ]);

    // Build def index
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

    let num_files = 12u32; // file 0 + file 1 + 10 IDisposable files
    let mut files_list: Vec<String> = vec![
        "src/ResourceManager.cs".to_string(),
        "src/Caller.cs".to_string(),
    ];
    path_to_id.insert(PathBuf::from("src/ResourceManager.cs"), 0);
    path_to_id.insert(PathBuf::from("src/Caller.cs"), 1);

    for i in 2..num_files {
        let path = format!("src/Service{}.cs", i);
        files_list.push(path.clone());
        path_to_id.insert(PathBuf::from(&path), i);
    }

    let def_idx = DefinitionIndex {
        root: ".".to_string(),
        created_at: 0,
        extensions: vec!["cs".to_string()],
        files: files_list.clone(),
        definitions,
        name_index,
        kind_index,
        attribute_index: HashMap::new(),
        base_type_index: HashMap::new(),
        file_index,
        path_to_id,
        method_calls,
        ..Default::default()
    };

    // --- Content Index ---
    // Token "dispose" appears in file 0 (definition) and file 1 (actual call)
    // Token "idisposable" appears in files 2..11 (many files mentioning the interface)
    // Token "resourcemanager" appears only in file 0 and file 1
    let mut index: HashMap<String, Vec<Posting>> = HashMap::new();

    // "dispose" in file 0 (definition, line 10) and file 1 (call, line 15)
    index.insert("dispose".to_string(), vec![
        Posting { file_id: 0, lines: vec![10] },
        Posting { file_id: 1, lines: vec![15] },
    ]);

    // "resourcemanager" in file 0 and file 1
    index.insert("resourcemanager".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![15] },
    ]);

    // "idisposable" in many files (simulating common interface)
    let idisposable_postings: Vec<Posting> = (2..num_files)
        .map(|fid| Posting { file_id: fid, lines: vec![1, 5, 10] })
        .collect();
    index.insert("idisposable".to_string(), idisposable_postings);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: files_list,
        index,
        total_tokens: 100,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50; num_files as usize],
        ..Default::default()
    };

    // --- Run build_caller_tree ---
    let mut visited = HashSet::new();
    let limits = CallerLimits {
        max_callers_per_level: 50,
        max_total_nodes: 200,
    };
    let node_count = AtomicUsize::new(0);

    let caller_ctx = CallerTreeContext {
        content_index: &content_index,
        def_idx: &def_idx,
        ext_filter: "cs",
        exclude_dir: &[],
        exclude_file: &[],
        resolve_interfaces: false,
        limits: &limits,
        node_count: &node_count,
        include_body: false,
        include_doc_comments: false,
        max_body_lines: 0,
        max_total_body_lines: 0,
        impact_analysis: false,
    };
    let mut file_cache = HashMap::new();
    let mut total_body_lines = 0usize;
    let mut tests_found = Vec::new();
    let callers = build_caller_tree(
        "Dispose",
        Some("ResourceManager"),
        3,
        0,
        &caller_ctx,
        &mut visited,
        &mut file_cache,
        &mut total_body_lines,
        &mut tests_found,
        &[],
    );

    // Should find exactly one caller: Caller.DoWork
    assert_eq!(callers.len(), 1, "Expected exactly 1 caller, got {}: {:?}", callers.len(), callers);
    let caller = &callers[0];
    assert_eq!(caller["method"].as_str().unwrap(), "DoWork");
    assert_eq!(caller["class"].as_str().unwrap(), "Caller");

    // Verify no false positives from IDisposable files
    let caller_file = caller["file"].as_str().unwrap();
    assert_eq!(caller_file, "Caller.cs", "Caller should be from Caller.cs, not an IDisposable file");
}

// ─── Test 10: resolve_call_site scopes by caller_parent when no receiver_type ──

#[test]
fn test_resolve_call_site_scopes_by_caller_parent() {
    let definitions = vec![
        class_def(0, "ClassA", vec![]),
        method_def(0, "doWork", "ClassA", 5, 15),
        class_def(1, "ClassB", vec![]),
        method_def(1, "doWork", "ClassB", 5, 15),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let call = CallSite {
        method_name: "doWork".to_string(),
        receiver_type: None,
        line: 10,
        receiver_is_generic: false,
    };

    let resolved_a = resolve_call_site(&call, &def_idx, Some("ClassA"));
    assert_eq!(resolved_a.len(), 1);
    assert_eq!(def_idx.definitions[resolved_a[0] as usize].parent.as_deref(), Some("ClassA"));

    let resolved_b = resolve_call_site(&call, &def_idx, Some("ClassB"));
    assert_eq!(resolved_b.len(), 1);
    assert_eq!(def_idx.definitions[resolved_b[0] as usize].parent.as_deref(), Some("ClassB"));

    let resolved_all = resolve_call_site(&call, &def_idx, None);
    assert_eq!(resolved_all.len(), 2);
}

// ─── Test 11: build_callee_tree depth=2 no cross-class pollution ──

#[test]
fn test_callee_tree_depth2_no_cross_class_pollution() {
    use std::sync::atomic::AtomicUsize;

    let definitions = vec![
        DefinitionEntry { file_id: 0, name: "ClassA".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 0, name: "process".to_string(), kind: DefinitionKind::Method, line_start: 5, line_end: 20, parent: Some("ClassA".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 0, name: "internalWork".to_string(), kind: DefinitionKind::Method, line_start: 22, line_end: 30, parent: Some("ClassA".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 1, name: "Helper".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 40, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 1, name: "run".to_string(), kind: DefinitionKind::Method, line_start: 5, line_end: 20, parent: Some("Helper".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 1, name: "helperStep".to_string(), kind: DefinitionKind::Method, line_start: 22, line_end: 35, parent: Some("Helper".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 2, name: "ClassB".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 40, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 2, name: "internalWork".to_string(), kind: DefinitionKind::Method, line_start: 5, line_end: 15, parent: Some("ClassB".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 2, name: "helperStep".to_string(), kind: DefinitionKind::Method, line_start: 17, line_end: 30, parent: Some("ClassB".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
    ];

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(1, vec![
        CallSite { method_name: "run".to_string(), receiver_type: Some("Helper".to_string()), line: 10, receiver_is_generic: false },
        CallSite { method_name: "internalWork".to_string(), receiver_type: None, line: 15, receiver_is_generic: false },
    ]);
    method_calls.insert(4, vec![
        CallSite { method_name: "helperStep".to_string(), receiver_type: None, line: 12, receiver_is_generic: false },
    ]);

    let def_idx = make_def_index(definitions, method_calls);
    let mut visited = HashSet::new();
    let limits = CallerLimits { max_callers_per_level: 50, max_total_nodes: 200 };
    let node_count = AtomicUsize::new(0);

    let caller_ctx = CallerTreeContext {
        content_index: &crate::ContentIndex::default(),
        def_idx: &def_idx,
        ext_filter: "ts",
        exclude_dir: &[],
        exclude_file: &[],
        resolve_interfaces: false,
        limits: &limits,
        node_count: &node_count,
        include_body: false,
        include_doc_comments: false,
        max_body_lines: 0,
        max_total_body_lines: 0,
        impact_analysis: false,
    };
    let mut file_cache = HashMap::new();
    let mut total_body_lines = 0usize;
    let callees = build_callee_tree("process", Some("ClassA"), 3, 0, &caller_ctx, &mut visited, &mut file_cache, &mut total_body_lines);

    assert_eq!(callees.len(), 2, "Should have 2 callees, got {:?}", callees);
    let callee_names: Vec<(&str, &str)> = callees.iter()
        .map(|c| (c["method"].as_str().unwrap(), c["class"].as_str().unwrap_or("?")))
        .collect();
    assert!(callee_names.contains(&("run", "Helper")));
    assert!(callee_names.contains(&("internalWork", "ClassA")));
    assert!(!callee_names.contains(&("internalWork", "ClassB")), "ClassB.internalWork should NOT appear");

    let run_node = callees.iter().find(|c| c["method"] == "run").unwrap();
    let run_callees = run_node["callees"].as_array().unwrap();
    assert_eq!(run_callees.len(), 1);
    assert_eq!(run_callees[0]["method"].as_str().unwrap(), "helperStep");
    assert_eq!(run_callees[0]["class"].as_str().unwrap(), "Helper", "helperStep should be Helper, not ClassB");
}

// ─── Test 12: Generic arity mismatch filters out non-generic class ──

#[test]
fn test_resolve_call_site_generic_arity_mismatch() {
    // Scenario: new DataList<int>() should NOT resolve to a non-generic DataList class
    // that happens to have the same name (e.g. ReportRenderingModel.DataList : DataRegion)
    // Note: uses "DataList" instead of "List" because "List" is on the built-in blocklist.
    let definitions = vec![
        // idx 0: non-generic DataList class (user-defined)
        DefinitionEntry {
            file_id: 0, name: "DataList".to_string(), kind: DefinitionKind::Class,
            line_start: 1, line_end: 50, parent: None,
            signature: Some("internal sealed class DataList : DataRegion".to_string()),
            modifiers: vec![], attributes: vec![], base_types: vec!["DataRegion".to_string()],
        },
        // idx 1: constructor of non-generic DataList
        DefinitionEntry {
            file_id: 0, name: "DataList".to_string(), kind: DefinitionKind::Constructor,
            line_start: 10, line_end: 20, parent: Some("DataList".to_string()),
            signature: Some("internal DataList(int, ReportProcessing.DataList, ListInstance, RenderingContext)".to_string()),
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let def_idx = make_def_index(definitions, HashMap::new());

    // Call site: new DataList<CatalogEntry>() — generic call
    let call_generic = CallSite {
        method_name: "DataList".to_string(),
        receiver_type: Some("DataList".to_string()),
        line: 252,
        receiver_is_generic: true, // <-- the key: call site had generics
    };

    // Should NOT resolve because the only DataList class is non-generic
    let resolved = resolve_call_site(&call_generic, &def_idx, None);
    assert!(resolved.is_empty(),
        "Generic call new DataList<CatalogEntry>() should NOT resolve to non-generic DataList class, got {:?}", resolved);

    // Call site: new DataList() — non-generic call
    let call_non_generic = CallSite {
        method_name: "DataList".to_string(),
        receiver_type: Some("DataList".to_string()),
        line: 300,
        receiver_is_generic: false,
    };

    // SHOULD resolve — both non-generic
    let resolved2 = resolve_call_site(&call_non_generic, &def_idx, None);
    assert!(!resolved2.is_empty(),
        "Non-generic call new DataList() SHOULD resolve to non-generic DataList class");
}

// ─── Test 13: Built-in Promise.resolve() not matched to user Deferred.resolve() ──

#[test]
fn test_builtin_promise_resolve_not_matched() {
    // Scenario: doWork() calls Promise.resolve(42).
    // User has classes UserService.resolve() and Deferred.resolve().
    // The built-in blocklist should prevent resolving Promise.resolve()
    // to any user-defined class.
    let definitions = vec![
        class_def(0, "UserService", vec![]),                          // idx 0
        method_def(0, "resolve", "UserService", 10, 20),             // idx 1
        class_def(1, "Deferred", vec![]),                             // idx 2
        method_def(1, "resolve", "Deferred", 5, 15),                 // idx 3
        class_def(2, "Worker", vec![]),                               // idx 4
        method_def(2, "doWork", "Worker", 1, 30),                    // idx 5
    ];

    let mut method_calls = HashMap::new();
    // doWork calls Promise.resolve(42)
    method_calls.insert(5u32, vec![
        CallSite {
            method_name: "resolve".to_string(),
            receiver_type: Some("Promise".to_string()),
            line: 10,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);

    // resolve_call_site with receiver_type = Promise should return empty
    let call = CallSite {
        method_name: "resolve".to_string(),
        receiver_type: Some("Promise".to_string()),
        line: 10,
        receiver_is_generic: false,
    };

    let resolved = resolve_call_site(&call, &def_idx, Some("Worker"));
    assert!(resolved.is_empty(),
        "Promise.resolve() should NOT resolve to any user-defined class, got {:?}", resolved);
}

// ─── Test 14: Built-in Array.map() not matched to user MyCollection.map() ──

#[test]
fn test_builtin_array_map_not_matched() {
    // Scenario: processItems() calls Array.map().
    // User has MyCollection.map().
    // The blocklist should prevent resolving Array.map() to MyCollection.map().
    let definitions = vec![
        class_def(0, "MyCollection", vec![]),                         // idx 0
        method_def(0, "map", "MyCollection", 5, 15),                 // idx 1
        class_def(1, "Processor", vec![]),                            // idx 2
        method_def(1, "processItems", "Processor", 1, 20),           // idx 3
    ];

    let mut method_calls = HashMap::new();
    method_calls.insert(3u32, vec![
        CallSite {
            method_name: "map".to_string(),
            receiver_type: Some("Array".to_string()),
            line: 10,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);

    let call = CallSite {
        method_name: "map".to_string(),
        receiver_type: Some("Array".to_string()),
        line: 10,
        receiver_is_generic: false,
    };

    let resolved = resolve_call_site(&call, &def_idx, Some("Processor"));
    assert!(resolved.is_empty(),
        "Array.map() should NOT resolve to MyCollection.map(), got {:?}", resolved);
}

// ─── Test 15: Non-built-in type still matches normally ──

#[test]
fn test_non_builtin_type_still_matches() {
    // Scenario: caller calls MyService.process().
    // MyService is NOT a built-in type, so it should still resolve normally.
    let definitions = vec![
        class_def(0, "MyService", vec![]),                            // idx 0
        method_def(0, "process", "MyService", 5, 15),                // idx 1
        class_def(1, "Controller", vec![]),                           // idx 2
        method_def(1, "handleRequest", "Controller", 1, 20),         // idx 3
    ];

    let mut method_calls = HashMap::new();
    method_calls.insert(3u32, vec![
        CallSite {
            method_name: "process".to_string(),
            receiver_type: Some("MyService".to_string()),
            line: 10,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);

    let call = CallSite {
        method_name: "process".to_string(),
        receiver_type: Some("MyService".to_string()),
        line: 10,
        receiver_is_generic: false,
    };

    let resolved = resolve_call_site(&call, &def_idx, Some("Controller"));
    assert!(!resolved.is_empty(),
        "MyService.process() SHOULD resolve (not a built-in type), got empty");
    assert_eq!(resolved.len(), 1);
    assert_eq!(def_idx.definitions[resolved[0] as usize].parent.as_deref(), Some("MyService"));
}

// ─── Test 16: is_implementation_of — exact I-prefix convention ──

#[test]
fn test_is_implementation_of_exact_prefix() {
    // IFooService → FooService (exact match after stripping I)
    assert!(is_implementation_of("FooService", "IFooService"));
    assert!(is_implementation_of("fooservice", "IFooService")); // case-insensitive
    assert!(is_implementation_of("FOOSERVICE", "IFooService")); // case-insensitive
}

// ─── Test 17: is_implementation_of — suffix-tolerant ──

#[test]
fn test_is_implementation_of_suffix_tolerant() {
    // IDataModelService → DataModelWebService (class contains the stem "DataModelService")
    assert!(is_implementation_of("DataModelWebService", "IDataModelService"));
    // stem "DataModelService" is contained in "MyDataModelServiceImpl"
    assert!(is_implementation_of("MyDataModelServiceImpl", "IDataModelService"));
}

// ─── Test 18: is_implementation_of — no false positive for short stems ──

#[test]
fn test_is_implementation_of_short_stem_no_match() {
    // IFoo → stem "Foo" (3 chars < 5 minimum) → no fuzzy match
    assert!(!is_implementation_of("FooBar", "IFoo"));
    // IFoo → "Foo" exact match should still work
    assert!(is_implementation_of("Foo", "IFoo"));
    // IData → stem "Data" (4 chars < 5 minimum) → no fuzzy match
    // This prevents false positives like DataProcessor matching IData
    assert!(!is_implementation_of("DataProcessor", "IData"));
    // IData → "Data" exact match should still work
    assert!(is_implementation_of("Data", "IData"));
    // IDataService → stem "DataService" (11 chars >= 5) → fuzzy match should work
    assert!(is_implementation_of("DataServiceImpl", "IDataService"));
}

// ─── Test 19: is_implementation_of — no false positive for unrelated classes ──

#[test]
fn test_is_implementation_of_no_false_positive() {
    // IService → stem "Service" (7 chars, ≥ 4) but "UnrelatedRunner" does NOT contain "service"
    assert!(!is_implementation_of("UnrelatedRunner", "IService"));
    // Not an interface (doesn't start with I + uppercase)
    assert!(!is_implementation_of("FooService", "fooService"));
    assert!(!is_implementation_of("FooService", "invalidName"));
    // "I" alone is not valid
    assert!(!is_implementation_of("Foo", "I"));
}

#[test]
fn test_is_implementation_of_edge_cases_no_panic() {
    // Empty string
    assert!(!is_implementation_of("Foo", ""));
    // Single char "I" — already tested above, but confirm no panic
    assert!(!is_implementation_of("Foo", "I"));
    // "I" followed by lowercase — not a valid interface
    assert!(!is_implementation_of("Foo", "Ifoo"));
    // Multi-byte second char after "I" — should not panic
    assert!(!is_implementation_of("Foo", "Iй"));
    // Pure non-ASCII
    assert!(!is_implementation_of("Foo", "йцу"));
    // Two chars, second is uppercase — valid interface pattern
    assert!(is_implementation_of("X", "IX"));
}

// ─── Test 20: Fuzzy DI interface matching via verify_call_site_target ──

#[test]
fn test_verify_call_site_target_fuzzy_interface_match() {
    // IDataModelService → DataModelWebService (suffix-tolerant)
    // Caller calls svc.getData() with receiver_type = "IDataModelService"
    // Target class is "DataModelWebService" — should match via fuzzy DI
    let definitions = vec![
        class_def(0, "SomeController", vec![]),                        // idx 0
        method_def(0, "process", "SomeController", 20, 40),           // idx 1
        class_def(1, "DataModelWebService", vec!["IDataModelService"]), // idx 2
        method_def(1, "getData", "DataModelWebService", 10, 30),      // idx 3
    ];

    let mut method_calls = HashMap::new();
    method_calls.insert(1u32, vec![
        CallSite {
            method_name: "getData".to_string(),
            receiver_type: Some("IDataModelService".to_string()),
            line: 25,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);

    // receiver is IDataModelService, target is DataModelWebService
    // Should match via fuzzy DI: stem "DataModelService" is contained in "DataModelWebService"
    assert!(verify_call_site_target(&def_idx, 1, 25, "getData", Some("DataModelWebService")),
        "IDataModelService → DataModelWebService should match via fuzzy DI");
}

// ─── Test 21: Fuzzy DI — no false positive for unrelated class ──

#[test]
fn test_fuzzy_di_no_false_positive() {
    // IService → stem "Service", but "UnrelatedRunner" does NOT contain "Service"
    // So receiver IService should NOT match target UnrelatedRunner
    let definitions = vec![
        class_def(0, "SomeController", vec![]),                     // idx 0
        method_def(0, "process", "SomeController", 20, 40),        // idx 1
        class_def(1, "UnrelatedRunner", vec![]),                    // idx 2
        method_def(1, "run", "UnrelatedRunner", 10, 30),           // idx 3
    ];

    let mut method_calls = HashMap::new();
    method_calls.insert(1u32, vec![
        CallSite {
            method_name: "run".to_string(),
            receiver_type: Some("IService".to_string()),
            line: 25,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);

    // receiver is IService, target is UnrelatedRunner
    // "UnrelatedRunner" does NOT contain "Service" → should NOT match
    assert!(!verify_call_site_target(&def_idx, 1, 25, "run", Some("UnrelatedRunner")),
        "IService → UnrelatedRunner should NOT match (no 'Service' in class name)");
}

// ─── Test 22a: Fuzzy DI via verify_call_site_target WITHOUT base_types ──
// This is the key regression test for BUG #2: is_implementation_of was dead code
// because verify_call_site_target passed lowercased inputs, but the function
// checked for uppercase 'I'. Now we pass original-case values.

#[test]
fn test_verify_fuzzy_di_without_base_types() {
    // DataModelWebService does NOT declare IDataModelService in base_types
    // but follows the naming convention (contains stem "DataModelService")
    // Receiver is IDataModelService → should match via is_implementation_of
    let definitions = vec![
        class_def(0, "SomeController", vec![]),                        // idx 0
        method_def(0, "process", "SomeController", 20, 40),           // idx 1
        class_def(1, "DataModelWebService", vec![]),                   // idx 2 — NO base_types!
        method_def(1, "getData", "DataModelWebService", 10, 30),      // idx 3
    ];

    let mut method_calls = HashMap::new();
    method_calls.insert(1u32, vec![
        CallSite {
            method_name: "getData".to_string(),
            receiver_type: Some("IDataModelService".to_string()),
            line: 25,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);

    // Without base_types, the only way to match is via is_implementation_of
    // This test would FAIL before the BUG #2 fix (lowercased inputs)
    assert!(verify_call_site_target(&def_idx, 1, 25, "getData", Some("DataModelWebService")),
        "IDataModelService → DataModelWebService should match via is_implementation_of (fuzzy DI) even without base_types");
}

// ─── Test 22b: Reverse fuzzy DI — target is interface, receiver is implementation ──

#[test]
fn test_verify_reverse_fuzzy_di_without_base_types() {
    // Target is IDataModelService, receiver is DataModelWebService
    let definitions = vec![
        class_def(0, "SomeController", vec![]),                        // idx 0
        method_def(0, "process", "SomeController", 20, 40),           // idx 1
        class_def(1, "SomeService", vec![]),                           // idx 2
        method_def(1, "getData", "SomeService", 10, 30),              // idx 3
    ];

    let mut method_calls = HashMap::new();
    method_calls.insert(1u32, vec![
        CallSite {
            method_name: "getData".to_string(),
            receiver_type: Some("DataModelWebService".to_string()),
            line: 25,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);

    // Reverse: target is IDataModelService, receiver is DataModelWebService
    assert!(verify_call_site_target(&def_idx, 1, 25, "getData", Some("IDataModelService")),
        "DataModelWebService → IDataModelService should match via reverse is_implementation_of");
}

// ─── Test 22: find_implementations_of_interface via base_type_index ──

#[test]
fn test_find_implementations_of_interface() {
    let definitions = vec![
        class_def(0, "DataModelWebService", vec!["IDataModelService"]), // idx 0
        class_def(1, "AnotherService", vec!["IDataModelService"]),      // idx 1
    ];

    let mut def_idx = make_def_index(definitions, HashMap::new());
    // Manually populate base_type_index
    def_idx.base_type_index.insert(
        "idatamodelservice".to_string(),
        vec![0, 1],
    );

    let impls = find_implementations_of_interface(&def_idx, "idatamodelservice");
    assert_eq!(impls.len(), 2);
    assert!(impls.contains(&"datamodelwebservice".to_string()));
    assert!(impls.contains(&"anotherservice".to_string()));
}

// ─── Test 25: Extension method — verify_call_site_target accepts mismatched receiver ──

#[test]
fn test_verify_call_site_target_extension_method() {
    // TokenExtensions (static class) defines IsValidClrValue as an extension method.
    // Consumer.Process calls token.IsValidClrValue() with receiver_type = "TokenType".
    // verify_call_site_target(target="TokenExtensions") should return true because
    // the extension_methods map tells us IsValidClrValue is an extension from TokenExtensions.
    let definitions = vec![
        class_def(0, "TokenExtensions", vec![]),                   // idx 0
        method_def(0, "IsValidClrValue", "TokenExtensions", 5, 10), // idx 1
        class_def(1, "Consumer", vec![]),                          // idx 2
        method_def(1, "Process", "Consumer", 10, 25),              // idx 3
    ];

    let mut method_calls = HashMap::new();
    method_calls.insert(3u32, vec![
        CallSite {
            method_name: "IsValidClrValue".to_string(),
            receiver_type: Some("TokenType".to_string()),
            line: 15,
            receiver_is_generic: false,
        },
    ]);

    let mut def_idx = make_def_index(definitions, method_calls);
    // Add extension method mapping
    def_idx.extension_methods.insert(
        "IsValidClrValue".to_string(),
        vec!["TokenExtensions".to_string()],
    );

    // Should match: target is TokenExtensions, which is an extension class for IsValidClrValue
    assert!(verify_call_site_target(&def_idx, 3, 15, "isvalidclrvalue", Some("TokenExtensions")),
        "Extension method IsValidClrValue should match when target class is the extension class");
}

#[test]
fn test_verify_call_site_target_extension_method_no_match_without_map() {
    // Same setup but WITHOUT extension_methods map → should NOT match
    let definitions = vec![
        class_def(0, "TokenExtensions", vec![]),
        method_def(0, "IsValidClrValue", "TokenExtensions", 5, 10),
        class_def(1, "Consumer", vec![]),
        method_def(1, "Process", "Consumer", 10, 25),
    ];

    let mut method_calls = HashMap::new();
    method_calls.insert(3u32, vec![
        CallSite {
            method_name: "IsValidClrValue".to_string(),
            receiver_type: Some("TokenType".to_string()),
            line: 15,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);
    // No extension_methods mapping

    // Should NOT match: receiver is TokenType, target is TokenExtensions, no relationship
    assert!(!verify_call_site_target(&def_idx, 3, 15, "isvalidclrvalue", Some("TokenExtensions")),
        "Without extension_methods map, receiver=TokenType should NOT match target=TokenExtensions");
}

// ─── Test 24: Generic method call site — verify_call_site_target matches stripped name ──

#[test]
fn test_verify_call_site_target_generic_method_call() {
    // Bug: call sites for generic methods like SearchAsync<T>() were stored
    // with method_name = "SearchAsync<T>" (including type args), causing
    // verify_call_site_target to fail because it compared against "SearchAsync".
    // After the fix, method_name is stored as "SearchAsync" (stripped).
    let definitions = vec![
        class_def(0, "Controller", vec![]),                          // idx 0
        method_def(0, "Handle", "Controller", 10, 30),              // idx 1
        class_def(1, "SearchService", vec!["ISearchService"]),       // idx 2
        method_def(1, "SearchAsync", "SearchService", 5, 20),       // idx 3
    ];

    let mut method_calls = HashMap::new();
    // After fix: method_name is "SearchAsync" (not "SearchAsync<T>")
    method_calls.insert(1u32, vec![
        CallSite {
            method_name: "SearchAsync".to_string(),
            receiver_type: Some("ISearchService".to_string()),
            line: 20,
            receiver_is_generic: false,
        },
    ]);

    let def_idx = make_def_index(definitions, method_calls);

    // This should match: receiver ISearchService, target SearchService
    assert!(verify_call_site_target(&def_idx, 1, 20, "SearchAsync", Some("SearchService")),
        "Generic method call SearchAsync should match when method_name is properly stripped of type args");
}

// ─── B3: Template tree navigation tests ─────────────────────────────

#[test]
fn test_build_template_callee_tree_one_level() {
    // Parent component has template_children pointing to child selectors
    let definitions = vec![
        class_def(0, "ParentComponent", vec![]),    // idx 0
        class_def(0, "ChildWidget", vec![]),        // idx 1
        class_def(0, "DataGrid", vec![]),           // idx 2
    ];
    let def_idx = {
        let mut idx = make_def_index(definitions, HashMap::new());
        // ParentComponent (idx 0) has children: ["child-widget", "data-grid"]
        idx.template_children.insert(0, vec!["child-widget".to_string(), "data-grid".to_string()]);
        // Register selectors mapping to separate components
        idx.selector_index.insert("child-widget".to_string(), vec![1]);
        idx.selector_index.insert("data-grid".to_string(), vec![2]);
        idx
    };

    let mut visited = HashSet::new();
    let result = build_template_callee_tree("ParentComponent", 2, 0, &def_idx, &mut visited);
    assert_eq!(result.len(), 2, "Should find 2 template children");
    let selectors: Vec<&str> = result.iter().filter_map(|n| n["selector"].as_str()).collect();
    assert!(selectors.contains(&"child-widget"));
    assert!(selectors.contains(&"data-grid"));
}

#[test]
fn test_build_template_callee_tree_recursive_depth2() {
    // Parent → child → grandchild
    let definitions = vec![
        class_def(0, "GrandParent", vec![]),   // idx 0
        class_def(0, "ChildComp", vec![]),      // idx 1
        class_def(0, "GrandChild", vec![]),     // idx 2
    ];
    let def_idx = {
        let mut idx = make_def_index(definitions, HashMap::new());
        idx.template_children.insert(0, vec!["child-comp".to_string()]);
        idx.template_children.insert(1, vec!["grand-child".to_string()]);
        idx.selector_index.insert("child-comp".to_string(), vec![1]);
        idx.selector_index.insert("grand-child".to_string(), vec![2]);
        idx
    };

    let mut visited = HashSet::new();
    let result = build_template_callee_tree("GrandParent", 3, 0, &def_idx, &mut visited);
    assert_eq!(result.len(), 1, "Should find 1 direct child");
    assert_eq!(result[0]["selector"].as_str().unwrap(), "child-comp");
    let children = result[0]["children"].as_array().unwrap();
    assert_eq!(children.len(), 1, "Child should have 1 grandchild");
    assert_eq!(children[0]["selector"].as_str().unwrap(), "grand-child");
}

#[test]
fn test_build_template_callee_tree_cyclic() {
    // Component A uses component B, component B uses component A
    let definitions = vec![
        class_def(0, "CompA", vec![]),  // idx 0
        class_def(0, "CompB", vec![]),  // idx 1
    ];
    let def_idx = {
        let mut idx = make_def_index(definitions, HashMap::new());
        idx.template_children.insert(0, vec!["comp-b".to_string()]);
        idx.template_children.insert(1, vec!["comp-a".to_string()]);
        idx.selector_index.insert("comp-a".to_string(), vec![0]);
        idx.selector_index.insert("comp-b".to_string(), vec![1]);
        idx
    };

    let mut visited = HashSet::new();
    let result = build_template_callee_tree("CompA", 10, 0, &def_idx, &mut visited);
    // Should not infinite loop — visited set prevents it
    assert_eq!(result.len(), 1, "Should find comp-b as child");
    assert_eq!(result[0]["selector"].as_str().unwrap(), "comp-b");
    // comp-b recurses into CompB which has comp-a as child.
    // comp-a selector was already visited (added when processing CompA's children)
    // so it should be skipped → comp-b has children with comp-a but comp-a has no further children
    // The visited set tracks selectors, not class names:
    // - "comp-b" was inserted when processing CompA's children
    // - When recursing into CompB, "comp-a" is NOT yet in visited (only "comp-b" is)
    // - So comp-a IS added as a child of comp-b
    // - But when comp-a recurses into CompA, "comp-b" is already visited → no further recursion
    // Total tree: CompA -> comp-b -> comp-a (with no further children)
    let children = result[0].get("children");
    assert!(children.is_some(), "comp-b should have children (comp-a)");
    let comp_a_children = children.unwrap().as_array().unwrap();
    assert_eq!(comp_a_children.len(), 1, "comp-b should have exactly 1 child (comp-a)");
    assert_eq!(comp_a_children[0]["selector"].as_str().unwrap(), "comp-a");
    // comp-a should have no further children (comp-b already visited)
    let grandchildren = comp_a_children[0].get("children");
    assert!(grandchildren.is_none() || grandchildren.unwrap().as_array().unwrap().is_empty(),
        "Cycle should be stopped: comp-a -> comp-b already visited");
}

#[test]
fn test_build_template_callee_tree_no_children() {
    // Component with no template_children → empty result
    let definitions = vec![
        class_def(0, "LeafComponent", vec![]),  // idx 0
    ];
    let def_idx = make_def_index(definitions, HashMap::new());

    let mut visited = HashSet::new();
    let result = build_template_callee_tree("LeafComponent", 3, 0, &def_idx, &mut visited);
    assert!(result.is_empty(), "Component with no template_children should return empty");
}

#[test]
fn test_find_template_parents_found() {
    // Selector appears in one parent's template_children
    let definitions = vec![
        class_def(0, "ParentComp", vec![]),  // idx 0
        class_def(0, "ChildComp", vec![]),   // idx 1
    ];
    let def_idx = {
        let mut idx = make_def_index(definitions, HashMap::new());
        idx.template_children.insert(0, vec!["child-comp".to_string()]);
        idx.selector_index.insert("parent-comp".to_string(), vec![0]);
        idx
    };

    let mut visited = HashSet::new();
    let result = find_template_parents("child-comp", 3, 0, &def_idx, &mut visited);
    assert_eq!(result.len(), 1, "Should find 1 parent");
    assert_eq!(result[0]["class"].as_str().unwrap(), "ParentComp");
}

#[test]
fn test_find_template_parents_multiple() {
    // Selector appears in two parents
    let definitions = vec![
        class_def(0, "ParentA", vec![]),  // idx 0
        class_def(0, "ParentB", vec![]),  // idx 1
        class_def(0, "SharedChild", vec![]),   // idx 2
    ];
    let def_idx = {
        let mut idx = make_def_index(definitions, HashMap::new());
        idx.template_children.insert(0, vec!["shared-child".to_string()]);
        idx.template_children.insert(1, vec!["shared-child".to_string()]);
        idx
    };

    let mut visited = HashSet::new();
    let result = find_template_parents("shared-child", 3, 0, &def_idx, &mut visited);
    assert_eq!(result.len(), 2, "Should find 2 parents using the selector");
    let parent_names: Vec<&str> = result.iter().filter_map(|n| n["class"].as_str()).collect();
    assert!(parent_names.contains(&"ParentA"));
    assert!(parent_names.contains(&"ParentB"));
}

#[test]
fn test_find_template_parents_recursive_depth() {
    // grandchild → child → parent
    // Searching UP from "grand-child" with depth=3 should find:
    //   level 1: ChildComp (direct parent)
    //   level 2: GrandParent (parent of ChildComp, nested in "parents")
    let definitions = vec![
        class_def(0, "GrandParent", vec![]),   // idx 0
        class_def(0, "ChildComp", vec![]),      // idx 1
        class_def(0, "GrandChild", vec![]),     // idx 2
    ];
    let def_idx = {
        let mut idx = make_def_index(definitions, HashMap::new());
        // GrandParent uses child-comp in its template
        idx.template_children.insert(0, vec!["child-comp".to_string()]);
        // ChildComp uses grand-child in its template
        idx.template_children.insert(1, vec!["grand-child".to_string()]);
        // Register selectors
        idx.selector_index.insert("grand-parent".to_string(), vec![0]);
        idx.selector_index.insert("child-comp".to_string(), vec![1]);
        idx.selector_index.insert("grand-child".to_string(), vec![2]);
        idx
    };

    // Search UP from "grand-child" with depth=3
    let mut visited = HashSet::new();
    let result = find_template_parents("grand-child", 3, 0, &def_idx, &mut visited);

    // Level 1: ChildComp is a direct parent (its template contains "grand-child")
    assert!(result.iter().any(|n| n["class"].as_str() == Some("ChildComp")),
        "Should find ChildComp as direct parent, got {:?}", result);

    // Level 2: GrandParent is a grandparent, nested in ChildComp's "parents" field
    let child_node = result.iter().find(|n| n["class"].as_str() == Some("ChildComp")).unwrap();
    let grandparents = child_node["parents"].as_array()
        .expect("ChildComp should have 'parents' field with grandparents");
    assert!(grandparents.iter().any(|p| p["class"].as_str() == Some("GrandParent")),
        "Should find GrandParent as grandparent (depth 2). Got parents: {:?}", grandparents);
}

#[test]
fn test_find_template_parents_respects_max_depth() {
    // Same 3-level hierarchy, but depth=1 should only return direct parent
    let definitions = vec![
        class_def(0, "GrandParent", vec![]),   // idx 0
        class_def(0, "ChildComp", vec![]),      // idx 1
        class_def(0, "GrandChild", vec![]),     // idx 2
    ];
    let def_idx = {
        let mut idx = make_def_index(definitions, HashMap::new());
        idx.template_children.insert(0, vec!["child-comp".to_string()]);
        idx.template_children.insert(1, vec!["grand-child".to_string()]);
        idx.selector_index.insert("grand-parent".to_string(), vec![0]);
        idx.selector_index.insert("child-comp".to_string(), vec![1]);
        idx.selector_index.insert("grand-child".to_string(), vec![2]);
        idx
    };

    let mut visited = HashSet::new();
    let result = find_template_parents("grand-child", 1, 0, &def_idx, &mut visited);

    // depth=1: only direct parent, no recursion
    assert_eq!(result.len(), 1, "Should find exactly 1 parent with depth=1");
    assert_eq!(result[0]["class"].as_str().unwrap(), "ChildComp");
    assert!(result[0].get("parents").is_none(),
        "With depth=1, should NOT recurse to find grandparents");
}

#[test]
fn test_find_template_parents_cyclic() {
    // CompA uses comp-b, CompB uses comp-a — upward should not infinite loop
    let definitions = vec![
        class_def(0, "CompA", vec![]),  // idx 0
        class_def(0, "CompB", vec![]),  // idx 1
    ];
    let def_idx = {
        let mut idx = make_def_index(definitions, HashMap::new());
        idx.template_children.insert(0, vec!["comp-b".to_string()]);
        idx.template_children.insert(1, vec!["comp-a".to_string()]);
        idx.selector_index.insert("comp-a".to_string(), vec![0]);
        idx.selector_index.insert("comp-b".to_string(), vec![1]);
        idx
    };

    let mut visited = HashSet::new();
    let result = find_template_parents("comp-a", 10, 0, &def_idx, &mut visited);
    // Should find CompB as parent (uses comp-a), then CompB's selector is comp-b,
    // searching for comp-b finds CompA as parent, but comp-a is already visited → stops
    assert!(!result.is_empty(), "Should find at least CompB as parent");
    // Should not panic or infinite loop
}

#[test]
fn test_find_template_parents_not_found() {
    // Non-existent selector → empty result
    let definitions = vec![
        class_def(0, "SomeComp", vec![]),  // idx 0
    ];
    let def_idx = {
        let mut idx = make_def_index(definitions, HashMap::new());
        idx.template_children.insert(0, vec!["existing-child".to_string()]);
        idx
    };

    let mut visited = HashSet::new();
    let result = find_template_parents("nonexistent-selector", 3, 0, &def_idx, &mut visited);
    assert!(result.is_empty(), "Non-existent selector should return empty");
}

// ─── Test: F-10 fix — class filter preserved during upward recursion ──

#[test]
fn test_caller_tree_preserves_class_filter_during_recursion() {
    // Scenario: We search for callers of ClassA.Process() (a common method name).
    // ClassB.Handle() calls classA.Process() → this is a valid caller at depth 0.
    // ClassC.Run() calls classB.Handle() → this is a valid caller at depth 1.
    // ClassD.Execute() also has a method named "Handle" but it's ClassD.Handle(),
    //   NOT ClassB.Handle(). Without the fix (passing None at recursion), depth 1
    //   would find ClassD.Execute() as a false positive because it searches for
    //   ALL "Handle" methods without class scoping.
    //
    // With the fix: at depth 1, we search for callers of "Handle" with class filter
    // "ClassB" (the caller's parent), so ClassD.Execute() is correctly excluded.

    use crate::{ContentIndex, Posting};
    use std::sync::atomic::AtomicUsize;
    use std::path::PathBuf;

    // --- Definition Index ---
    let definitions = vec![
        // idx 0: class ClassA
        DefinitionEntry { file_id: 0, name: "ClassA".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        // idx 1: ClassA.Process (target method)
        DefinitionEntry { file_id: 0, name: "Process".to_string(), kind: DefinitionKind::Method, line_start: 10, line_end: 20, parent: Some("ClassA".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        // idx 2: class ClassB
        DefinitionEntry { file_id: 1, name: "ClassB".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        // idx 3: ClassB.Handle (calls ClassA.Process)
        DefinitionEntry { file_id: 1, name: "Handle".to_string(), kind: DefinitionKind::Method, line_start: 10, line_end: 30, parent: Some("ClassB".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        // idx 4: class ClassC
        DefinitionEntry { file_id: 2, name: "ClassC".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        // idx 5: ClassC.Run (calls ClassB.Handle)
        DefinitionEntry { file_id: 2, name: "Run".to_string(), kind: DefinitionKind::Method, line_start: 10, line_end: 30, parent: Some("ClassC".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        // idx 6: class ClassD (unrelated class with same method name "Handle")
        DefinitionEntry { file_id: 3, name: "ClassD".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        // idx 7: ClassD.Handle (DIFFERENT method, same name — should NOT appear)
        DefinitionEntry { file_id: 3, name: "Handle".to_string(), kind: DefinitionKind::Method, line_start: 10, line_end: 30, parent: Some("ClassD".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        // idx 8: class ClassE
        DefinitionEntry { file_id: 4, name: "ClassE".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        // idx 9: ClassE.Execute (calls ClassD.Handle — should NOT be in results)
        DefinitionEntry { file_id: 4, name: "Execute".to_string(), kind: DefinitionKind::Method, line_start: 10, line_end: 30, parent: Some("ClassE".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
    ];

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    // ClassB.Handle calls ClassA.Process at line 20
    method_calls.insert(3, vec![
        CallSite { method_name: "Process".to_string(), receiver_type: Some("ClassA".to_string()), line: 20, receiver_is_generic: false },
    ]);
    // ClassC.Run calls ClassB.Handle at line 15
    method_calls.insert(5, vec![
        CallSite { method_name: "Handle".to_string(), receiver_type: Some("ClassB".to_string()), line: 15, receiver_is_generic: false },
    ]);
    // ClassE.Execute calls ClassD.Handle at line 15
    method_calls.insert(9, vec![
        CallSite { method_name: "Handle".to_string(), receiver_type: Some("ClassD".to_string()), line: 15, receiver_is_generic: false },
    ]);

    // Build def index
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

    let files_list = vec![
        "src/ClassA.cs".to_string(),
        "src/ClassB.cs".to_string(),
        "src/ClassC.cs".to_string(),
        "src/ClassD.cs".to_string(),
        "src/ClassE.cs".to_string(),
    ];
    for (i, f) in files_list.iter().enumerate() {
        path_to_id.insert(PathBuf::from(f), i as u32);
    }

    let def_idx = DefinitionIndex {
        root: ".".to_string(),
        extensions: vec!["cs".to_string()],
        files: files_list.clone(),
        definitions,
        name_index,
        kind_index,
        file_index,
        path_to_id,
        method_calls,
        ..Default::default()
    };

    // --- Content Index ---
    let mut index: HashMap<String, Vec<Posting>> = HashMap::new();
    // "process" appears in file 0 (definition) and file 1 (ClassB calls it)
    index.insert("process".to_string(), vec![
        Posting { file_id: 0, lines: vec![10] },
        Posting { file_id: 1, lines: vec![20] },
    ]);
    // "handle" appears in files 1, 2, 3, 4
    index.insert("handle".to_string(), vec![
        Posting { file_id: 1, lines: vec![10] },  // definition of ClassB.Handle
        Posting { file_id: 2, lines: vec![15] },  // ClassC calls ClassB.Handle
        Posting { file_id: 3, lines: vec![10] },  // definition of ClassD.Handle
        Posting { file_id: 4, lines: vec![15] },  // ClassE calls ClassD.Handle
    ]);
    // Class name tokens for pre-filtering
    index.insert("classa".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![20] },
    ]);
    index.insert("classb".to_string(), vec![
        Posting { file_id: 1, lines: vec![1] },
        Posting { file_id: 2, lines: vec![15] },
    ]);
    index.insert("classd".to_string(), vec![
        Posting { file_id: 3, lines: vec![1] },
        Posting { file_id: 4, lines: vec![15] },
    ]);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: files_list,
        index,
        total_tokens: 100,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50; 5],
        ..Default::default()
    };

    // --- Run build_caller_tree with depth=3 ---
    let mut visited = HashSet::new();
    let limits = CallerLimits { max_callers_per_level: 50, max_total_nodes: 200 };
    let node_count = AtomicUsize::new(0);

    let caller_ctx = CallerTreeContext {
        content_index: &content_index,
        def_idx: &def_idx,
        ext_filter: "cs",
        exclude_dir: &[],
        exclude_file: &[],
        resolve_interfaces: false,
        limits: &limits,
        node_count: &node_count,
        include_body: false,
        include_doc_comments: false,
        max_body_lines: 0,
        max_total_body_lines: 0,
        impact_analysis: false,
    };
    let mut file_cache = HashMap::new();
    let mut total_body_lines = 0usize;
    let mut tests_found = Vec::new();
    let callers = build_caller_tree(
        "Process",
        Some("ClassA"),
        3,  // depth 3: should find ClassB.Handle (depth 0) → ClassC.Run (depth 1)
        0,
        &caller_ctx,
        &mut visited,
        &mut file_cache,
        &mut total_body_lines,
        &mut tests_found,
        &[],
    );

    // Depth 0: should find ClassB.Handle as the caller of ClassA.Process
    assert_eq!(callers.len(), 1, "Expected 1 caller at depth 0, got {}: {:?}", callers.len(), callers);
    assert_eq!(callers[0]["method"].as_str().unwrap(), "Handle");
    assert_eq!(callers[0]["class"].as_str().unwrap(), "ClassB");

    // Depth 1: ClassB.Handle should have ClassC.Run as its caller
    let sub_callers = callers[0]["callers"].as_array()
        .expect("ClassB.Handle should have sub-callers");
    assert_eq!(sub_callers.len(), 1, "Expected 1 sub-caller of ClassB.Handle, got {}: {:?}", sub_callers.len(), sub_callers);
    assert_eq!(sub_callers[0]["method"].as_str().unwrap(), "Run");
    assert_eq!(sub_callers[0]["class"].as_str().unwrap(), "ClassC");

    // CRITICAL: ClassE.Execute should NOT appear anywhere in the tree.
    // It calls ClassD.Handle, not ClassB.Handle. Without the F-10 fix,
    // it would appear as a false positive because the class filter was
    // dropped at depth > 0.
    let total_nodes = node_count.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(total_nodes, 2, "Should have exactly 2 nodes (ClassB.Handle + ClassC.Run), got {}", total_nodes);
}

// ─── F-1: Hint when 0 callers with class filter ─────────────────

#[test]
fn test_xray_callers_hint_when_empty_with_class_filter() {
    // When class filter is set and callTree is empty, response should include a hint
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "process", "OrderService", 5, 15),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());

    let content_index = crate::ContentIndex {
        root: ".".to_string(),
        files: vec!["src/OrderController.ts".to_string()],
        index: HashMap::new(),
        total_tokens: 0,
        extensions: vec!["ts".to_string()],
        file_token_counts: vec![0],
        ..Default::default()
    };

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    // Search for callers of a method with a class filter that yields 0 results
    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": "process",
        "class": "NonExistentClass",
        "depth": 1
    }));
    assert!(!result.is_error, "Should not error: {:?}", result.content[0].text);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v.get("hint").is_some(),
        "Should have hint when callTree is empty with class filter. Got: {}",
        serde_json::to_string_pretty(&v).unwrap());
    let hint = v["hint"].as_str().unwrap();
    assert!(hint.contains("class"), "Hint should mention class parameter");
}

#[test]
fn test_xray_callers_no_hint_without_class_filter() {
    // When no class filter is set, no hint should appear even if tree is empty
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "process", "OrderService", 5, 15),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());

    let content_index = crate::ContentIndex {
        root: ".".to_string(),
        files: vec!["src/OrderController.ts".to_string()],
        index: HashMap::new(),
        total_tokens: 0,
        extensions: vec!["ts".to_string()],
        file_token_counts: vec![0],
        ..Default::default()
    };

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    // Search without class filter — no hint expected
    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": "nonexistentmethod",
        "depth": 1
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v.get("hint").is_none(),
        "Should NOT have hint when no class filter is set");
}

#[test]
fn test_xray_callers_no_hint_when_results_found() {
    // When callers ARE found with class filter, no hint should appear
    use crate::{ContentIndex, Posting};
    use std::path::PathBuf;

    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "process", "OrderService", 5, 15),
        class_def(1, "Consumer", vec![]),
        method_def(1, "run", "Consumer", 5, 20),
    ];

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(3, vec![
        CallSite {
            method_name: "process".to_string(),
            receiver_type: Some("OrderService".to_string()),
            line: 10,
            receiver_is_generic: false,
        },
    ]);

    let mut def_idx = make_def_index(definitions, method_calls);
    def_idx.path_to_id.insert(PathBuf::from("src/OrderController.ts"), 0);
    def_idx.path_to_id.insert(PathBuf::from("src/OrderValidator.ts"), 1);

    let mut index: HashMap<String, Vec<Posting>> = HashMap::new();
    index.insert("process".to_string(), vec![
        Posting { file_id: 0, lines: vec![5] },
        Posting { file_id: 1, lines: vec![10] },
    ]);
    index.insert("orderservice".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![10] },
    ]);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "src/OrderController.ts".to_string(),
            "src/OrderValidator.ts".to_string(),
        ],
        index,
        total_tokens: 100,
        extensions: vec!["ts".to_string()],
        file_token_counts: vec![50, 50],
        ..Default::default()
    };

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": "process",
        "class": "OrderService",
        "depth": 1
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = v["callTree"].as_array().unwrap();
    if !tree.is_empty() {
        assert!(v.get("hint").is_none(),
            "Should NOT have hint when callers are found");
    }
}

// ─── Test 23: resolve_call_site resolves via base_types (existing behavior preserved) ──

#[test]
fn test_resolve_call_site_via_base_types() {
    // IDataModelService.getData() should resolve to DataModelWebService.getData()
    // via base_types in the class definition
    let definitions = vec![
        class_def(0, "DataModelWebService", vec!["IDataModelService"]), // idx 0
        method_def(0, "getData", "DataModelWebService", 10, 30),       // idx 1
    ];

    let def_idx = make_def_index(definitions, HashMap::new());

    let call = CallSite {
        method_name: "getData".to_string(),
        receiver_type: Some("IDataModelService".to_string()),
        line: 5,
        receiver_is_generic: false,
    };

    let resolved = resolve_call_site(&call, &def_idx, None);
    assert!(!resolved.is_empty(),
        "IDataModelService.getData() should resolve to DataModelWebService.getData via base_types");
}

// ═══════════════════════════════════════════════════════════════════
// SQL Caller Tests — SP/SqlFunction integration with xray_callers
// ═══════════════════════════════════════════════════════════════════

/// Helper: create a DefinitionEntry for a stored procedure.
/// `schema` becomes the `parent` (analogous to class in C#/TS).
fn sp_def(file_id: u32, name: &str, schema: Option<&str>, line_start: u32, line_end: u32) -> DefinitionEntry {
    DefinitionEntry {
        file_id,
        name: name.to_string(),
        kind: DefinitionKind::StoredProcedure,
        line_start,
        line_end,
        parent: schema.map(|s| s.to_string()),
        signature: None,
        modifiers: vec![],
        attributes: vec![],
        base_types: vec![],
    }
}

/// Helper: create a DefinitionEntry for a SQL function.
fn sqlfn_def(file_id: u32, name: &str, schema: Option<&str>, line_start: u32, line_end: u32) -> DefinitionEntry {
    DefinitionEntry {
        file_id,
        name: name.to_string(),
        kind: DefinitionKind::SqlFunction,
        line_start,
        line_end,
        parent: schema.map(|s| s.to_string()),
        signature: None,
        modifiers: vec![],
        attributes: vec![],
        base_types: vec![],
    }
}

/// Helper: create a DefinitionEntry for a SQL table (should NOT appear in caller results).
fn table_def(file_id: u32, name: &str, schema: Option<&str>, line_start: u32, line_end: u32) -> DefinitionEntry {
    DefinitionEntry {
        file_id,
        name: name.to_string(),
        kind: DefinitionKind::Table,
        line_start,
        line_end,
        parent: schema.map(|s| s.to_string()),
        signature: None,
        modifiers: vec![],
        attributes: vec![],
        base_types: vec![],
    }
}

// ─── SQL Test 1: resolve_call_site resolves EXEC to SP definition ──

#[test]
fn test_resolve_call_site_sql_exec() {
    // SP usp_ProcessBatch calls EXEC [Sales].[usp_ValidateOrder]
    // Should resolve to the SP definition in Sales schema
    let definitions = vec![
        sp_def(0, "usp_ProcessBatch", Some("dbo"), 1, 50),      // idx 0
        sp_def(1, "usp_ValidateOrder", Some("Sales"), 1, 30),   // idx 1
    ];
    let def_idx = make_def_index(definitions, HashMap::new());

    let call = CallSite {
        method_name: "usp_ValidateOrder".to_string(),
        receiver_type: Some("Sales".to_string()),
        line: 10,
        receiver_is_generic: false,
    };

    let resolved = resolve_call_site(&call, &def_idx, Some("dbo"));
    assert_eq!(resolved.len(), 1, "EXEC [Sales].[usp_ValidateOrder] should resolve to 1 SP");
    assert_eq!(def_idx.definitions[resolved[0] as usize].name, "usp_ValidateOrder");
    assert_eq!(def_idx.definitions[resolved[0] as usize].parent.as_deref(), Some("Sales"));
}

// ─── SQL Test 2: resolve_call_site does NOT resolve FROM to Table ──

#[test]
fn test_resolve_call_site_sql_table_excluded() {
    // SP calls FROM [dbo].[Orders] — Table kind is NOT in the resolve filter
    let definitions = vec![
        sp_def(0, "usp_GetOrders", Some("dbo"), 1, 30),    // idx 0
        table_def(1, "Orders", Some("dbo"), 1, 10),         // idx 1
    ];
    let def_idx = make_def_index(definitions, HashMap::new());

    let call = CallSite {
        method_name: "Orders".to_string(),
        receiver_type: Some("dbo".to_string()),
        line: 5,
        receiver_is_generic: false,
    };

    let resolved = resolve_call_site(&call, &def_idx, Some("dbo"));
    assert!(resolved.is_empty(),
        "FROM [dbo].[Orders] should NOT resolve to Table definition (tables excluded from callers)");
}

// ─── SQL Test 3: find_containing_method finds containing SP ──

#[test]
fn test_find_containing_method_sql_sp() {
    // A call site at line 15 is inside SP usp_ProcessBatch (lines 1-50)
    let definitions = vec![
        sp_def(0, "usp_ProcessBatch", Some("dbo"), 1, 50),   // idx 0
    ];
    let def_idx = make_def_index(definitions, HashMap::new());

    let result = find_containing_method(&def_idx, 0, 15);
    assert!(result.is_some(), "Should find SP containing line 15");
    let (name, parent, line_start, _di) = result.unwrap();
    assert_eq!(name, "usp_ProcessBatch");
    assert_eq!(parent.as_deref(), Some("dbo"), "SP parent should be the schema name");
    assert_eq!(line_start, 1);
}

// ─── SQL Test 4: build_callee_tree (direction=down) for SP ──

#[test]
fn test_sql_callee_tree_exec_dependencies() {
    use std::sync::atomic::AtomicUsize;

    // usp_ProcessBatch (dbo) calls:
    //   EXEC [Sales].[usp_ValidateOrder]
    //   EXEC [dbo].[usp_ReserveStock]
    //   FROM [dbo].[Orders] (table — should NOT appear)
    let definitions = vec![
        sp_def(0, "usp_ProcessBatch", Some("dbo"), 1, 50),      // idx 0
        sp_def(1, "usp_ValidateOrder", Some("Sales"), 1, 30),   // idx 1
        sp_def(2, "usp_ReserveStock", Some("dbo"), 1, 25),      // idx 2
        table_def(3, "Orders", Some("dbo"), 1, 5),               // idx 3
    ];

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(0, vec![
        CallSite { method_name: "usp_ValidateOrder".to_string(), receiver_type: Some("Sales".to_string()), line: 10, receiver_is_generic: false },
        CallSite { method_name: "usp_ReserveStock".to_string(), receiver_type: Some("dbo".to_string()), line: 20, receiver_is_generic: false },
        CallSite { method_name: "Orders".to_string(), receiver_type: Some("dbo".to_string()), line: 15, receiver_is_generic: false },
    ]);

    // Build DefinitionIndex with proper .sql files
    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();

    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }

    let files_list = vec![
        "sql/usp_ProcessBatch.sql".to_string(),
        "sql/usp_ValidateOrder.sql".to_string(),
        "sql/usp_ReserveStock.sql".to_string(),
        "sql/Orders.sql".to_string(),
    ];

    let def_idx = DefinitionIndex {
        root: ".".to_string(),
        created_at: 0,
        extensions: vec!["sql".to_string()],
        files: files_list,
        definitions,
        name_index,
        kind_index,
        attribute_index: HashMap::new(),
        base_type_index: HashMap::new(),
        file_index,
        path_to_id: HashMap::new(),
        method_calls,
        ..Default::default()
    };

    let mut visited = HashSet::new();
    let limits = CallerLimits { max_callers_per_level: 50, max_total_nodes: 200 };
    let node_count = AtomicUsize::new(0);

    let caller_ctx = CallerTreeContext {
        content_index: &crate::ContentIndex::default(),
        def_idx: &def_idx,
        ext_filter: "sql",
        exclude_dir: &[],
        exclude_file: &[],
        resolve_interfaces: false,
        limits: &limits,
        node_count: &node_count,
        include_body: false,
        include_doc_comments: false,
        max_body_lines: 0,
        max_total_body_lines: 0,
        impact_analysis: false,
    };

    let mut file_cache = HashMap::new();
    let mut total_body_lines = 0usize;
    let callees = build_callee_tree("usp_ProcessBatch", Some("dbo"), 3, 0, &caller_ctx, &mut visited, &mut file_cache, &mut total_body_lines);

    // Should find 2 SP callees (usp_ValidateOrder and usp_ReserveStock)
    // Should NOT find Orders (Table kind excluded)
    assert_eq!(callees.len(), 2, "Should have 2 SP callees, got {:?}", callees);

    let callee_names: Vec<&str> = callees.iter()
        .map(|c| c["method"].as_str().unwrap())
        .collect();
    assert!(callee_names.contains(&"usp_ValidateOrder"), "Should find usp_ValidateOrder");
    assert!(callee_names.contains(&"usp_ReserveStock"), "Should find usp_ReserveStock");
    assert!(!callee_names.contains(&"Orders"), "Table Orders should NOT be in callees");
}

// ─── SQL Test 5: build_caller_tree (direction=up) for SP ──

#[test]
fn test_sql_caller_tree_who_calls_sp() {
    use crate::{ContentIndex, Posting};
    use std::sync::atomic::AtomicUsize;
    use std::path::PathBuf;

    // usp_ProcessBatch (dbo) calls EXEC [Sales].[usp_ValidateOrder]
    // Question: who calls usp_ValidateOrder? → answer: usp_ProcessBatch
    let definitions = vec![
        sp_def(0, "usp_ProcessBatch", Some("dbo"), 1, 50),      // idx 0
        sp_def(1, "usp_ValidateOrder", Some("Sales"), 1, 30),   // idx 1
    ];

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(0, vec![
        CallSite {
            method_name: "usp_ValidateOrder".to_string(),
            receiver_type: Some("Sales".to_string()),
            line: 20,
            receiver_is_generic: false,
        },
    ]);

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

    let files_list = vec![
        "sql/usp_ProcessBatch.sql".to_string(),
        "sql/usp_ValidateOrder.sql".to_string(),
    ];
    path_to_id.insert(PathBuf::from("sql/usp_ProcessBatch.sql"), 0);
    path_to_id.insert(PathBuf::from("sql/usp_ValidateOrder.sql"), 1);

    let def_idx = DefinitionIndex {
        root: ".".to_string(),
        created_at: 0,
        extensions: vec!["sql".to_string()],
        files: files_list.clone(),
        definitions,
        name_index,
        kind_index,
        attribute_index: HashMap::new(),
        base_type_index: HashMap::new(),
        file_index,
        path_to_id,
        method_calls,
        ..Default::default()
    };

    // Content index: "usp_validateorder" appears in file 0 (call) and file 1 (definition)
    let mut index: HashMap<String, Vec<Posting>> = HashMap::new();
    index.insert("usp_validateorder".to_string(), vec![
        Posting { file_id: 0, lines: vec![20] },   // call in usp_ProcessBatch
        Posting { file_id: 1, lines: vec![1] },     // definition
    ]);
    index.insert("sales".to_string(), vec![
        Posting { file_id: 0, lines: vec![20] },
        Posting { file_id: 1, lines: vec![1] },
    ]);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: files_list,
        index,
        total_tokens: 50,
        extensions: vec!["sql".to_string()],
        file_token_counts: vec![25, 25],
        ..Default::default()
    };

    let mut visited = HashSet::new();
    let limits = CallerLimits { max_callers_per_level: 50, max_total_nodes: 200 };
    let node_count = AtomicUsize::new(0);

    let caller_ctx = CallerTreeContext {
        content_index: &content_index,
        def_idx: &def_idx,
        ext_filter: "sql",
        exclude_dir: &[],
        exclude_file: &[],
        resolve_interfaces: false,
        limits: &limits,
        node_count: &node_count,
        include_body: false,
        include_doc_comments: false,
        max_body_lines: 0,
        max_total_body_lines: 0,
        impact_analysis: false,
    };

    let mut file_cache = HashMap::new();
    let mut total_body_lines = 0usize;
    let mut tests_found = Vec::new();
    let callers = build_caller_tree(
        "usp_ValidateOrder",
        Some("Sales"),
        3,
        0,
        &caller_ctx,
        &mut visited,
        &mut file_cache,
        &mut total_body_lines,
        &mut tests_found,
        &[],
    );

    assert_eq!(callers.len(), 1, "Expected 1 caller of usp_ValidateOrder, got {:?}", callers);
    assert_eq!(callers[0]["method"].as_str().unwrap(), "usp_ProcessBatch");
    assert_eq!(callers[0]["file"].as_str().unwrap(), "usp_ProcessBatch.sql");
}

// ─── SQL Test 6: SqlFunction included in callee tree ──

#[test]
fn test_resolve_call_site_sql_function() {
    // SP calls a SQL function fn_CalculateTotal
    let definitions = vec![
        sp_def(0, "usp_GetReport", Some("dbo"), 1, 30),           // idx 0
        sqlfn_def(1, "fn_CalculateTotal", Some("dbo"), 1, 15),    // idx 1
    ];
    let def_idx = make_def_index(definitions, HashMap::new());

    let call = CallSite {
        method_name: "fn_CalculateTotal".to_string(),
        receiver_type: Some("dbo".to_string()),
        line: 10,
        receiver_is_generic: false,
    };

    let resolved = resolve_call_site(&call, &def_idx, Some("dbo"));
    assert_eq!(resolved.len(), 1, "Should resolve SQL function call");
    assert_eq!(def_idx.definitions[resolved[0] as usize].name, "fn_CalculateTotal");
    assert_eq!(def_idx.definitions[resolved[0] as usize].kind, DefinitionKind::SqlFunction);
}

// ─── SQL Test 7: Cross-schema EXEC resolution ──

#[test]
fn test_resolve_call_site_sql_cross_schema() {
    // SP in dbo schema calls SP in Sales schema
    let definitions = vec![
        sp_def(0, "usp_ProcessBatch", Some("dbo"), 1, 50),
        sp_def(1, "usp_ValidateOrder", Some("Sales"), 1, 30),
        sp_def(2, "usp_ValidateOrder", Some("Inventory"), 1, 25), // same name, different schema
    ];
    let def_idx = make_def_index(definitions, HashMap::new());

    // Call with Sales schema → should only match Sales version
    let call_sales = CallSite {
        method_name: "usp_ValidateOrder".to_string(),
        receiver_type: Some("Sales".to_string()),
        line: 10,
        receiver_is_generic: false,
    };
    let resolved = resolve_call_site(&call_sales, &def_idx, Some("dbo"));
    assert_eq!(resolved.len(), 1, "Should resolve to exactly 1 SP (Sales schema)");
    assert_eq!(def_idx.definitions[resolved[0] as usize].parent.as_deref(), Some("Sales"));

    // Call with Inventory schema → should only match Inventory version
    let call_inv = CallSite {
        method_name: "usp_ValidateOrder".to_string(),
        receiver_type: Some("Inventory".to_string()),
        line: 15,
        receiver_is_generic: false,
    };
    let resolved_inv = resolve_call_site(&call_inv, &def_idx, Some("dbo"));
    assert_eq!(resolved_inv.len(), 1, "Should resolve to exactly 1 SP (Inventory schema)");
    assert_eq!(def_idx.definitions[resolved_inv[0] as usize].parent.as_deref(), Some("Inventory"));
}

// ═══════════════════════════════════════════════════════════════════
// Tests for extracted helper functions (complexity reduction)
// ═══════════════════════════════════════════════════════════════════

// ─── find_target_line tests ──────────────────────────────────────

#[test]
fn test_find_target_line_method_found() {
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "process", "OrderService", 10, 20),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    assert_eq!(find_target_line(&def_idx, "process", None), Some(10));
}

#[test]
fn test_find_target_line_with_class_filter() {
    let definitions = vec![
        class_def(0, "ClassA", vec![]),
        method_def(0, "doWork", "ClassA", 5, 15),
        class_def(1, "ClassB", vec![]),
        method_def(1, "doWork", "ClassB", 10, 20),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    assert_eq!(find_target_line(&def_idx, "dowork", Some("ClassA")), Some(5));
    assert_eq!(find_target_line(&def_idx, "dowork", Some("ClassB")), Some(10));
}

#[test]
fn test_find_target_line_not_found() {
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    assert_eq!(find_target_line(&def_idx, "nonexistent", None), None);
}

#[test]
fn test_find_target_line_skips_non_callable_kinds() {
    // A class definition should not be returned — only methods/constructors/functions
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    assert_eq!(find_target_line(&def_idx, "orderservice", None), None);
}

#[test]
fn test_find_target_line_class_filter_no_match() {
    let definitions = vec![
        class_def(0, "ClassA", vec![]),
        method_def(0, "doWork", "ClassA", 5, 15),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    assert_eq!(find_target_line(&def_idx, "dowork", Some("ClassB")), None);
}

// ─── collect_definition_locations tests ──────────────────────────

#[test]
fn test_collect_definition_locations_single_method() {
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "process", "OrderService", 10, 20),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let locations = collect_definition_locations(&def_idx, "process");
    assert_eq!(locations.len(), 1);
    assert!(locations.contains(&(0, 10)));
}

#[test]
fn test_collect_definition_locations_multiple_overloads() {
    let definitions = vec![
        class_def(0, "ClassA", vec![]),
        method_def(0, "doWork", "ClassA", 5, 15),
        class_def(1, "ClassB", vec![]),
        method_def(1, "doWork", "ClassB", 10, 20),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let locations = collect_definition_locations(&def_idx, "dowork");
    assert_eq!(locations.len(), 2);
    assert!(locations.contains(&(0, 5)));
    assert!(locations.contains(&(1, 10)));
}

#[test]
fn test_collect_definition_locations_empty() {
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let locations = collect_definition_locations(&def_idx, "nonexistent");
    assert!(locations.is_empty());
}

#[test]
fn test_collect_definition_locations_excludes_classes() {
    // Class definitions should not be included
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let locations = collect_definition_locations(&def_idx, "orderservice");
    assert!(locations.is_empty());
}

// ─── passes_caller_file_filters tests ───────────────────────────

#[test]
fn test_passes_caller_file_filters_matching_ext() {
    assert!(passes_caller_file_filters("src/OrderService.cs", "cs", &[], &[]));
    assert!(passes_caller_file_filters("src/app.ts", "ts", &[], &[]));
}

#[test]
fn test_passes_caller_file_filters_non_matching_ext() {
    assert!(!passes_caller_file_filters("src/OrderService.cs", "ts", &[], &[]));
    assert!(!passes_caller_file_filters("src/README.md", "cs", &[], &[]));
}

#[test]
fn test_passes_caller_file_filters_multi_ext() {
    assert!(passes_caller_file_filters("src/OrderService.cs", "cs,ts", &[], &[]));
    assert!(passes_caller_file_filters("src/app.ts", "cs,ts", &[], &[]));
    assert!(!passes_caller_file_filters("src/README.md", "cs,ts", &[], &[]));
}

#[test]
fn test_passes_caller_file_filters_exclude_dir() {
    let exclude_dir = vec!["test".to_string()];
    assert!(!passes_caller_file_filters("src/test/OrderService.cs", "cs", &exclude_dir, &[]));
    assert!(passes_caller_file_filters("src/main/OrderService.cs", "cs", &exclude_dir, &[]));
}

#[test]
fn test_passes_caller_file_filters_exclude_file() {
    let exclude_file = vec!["mock".to_string()];
    assert!(!passes_caller_file_filters("src/MockOrderService.cs", "cs", &[], &exclude_file));
    assert!(passes_caller_file_filters("src/OrderService.cs", "cs", &[], &exclude_file));
}

// ─── build_caller_node tests ────────────────────────────────────

#[test]
fn test_build_caller_node_basic() {
    let node = build_caller_node("doWork", Some("OrderService"), 10, &[25], "src/OrderService.cs", vec![]);
    assert_eq!(node["method"].as_str().unwrap(), "doWork");
    assert_eq!(node["class"].as_str().unwrap(), "OrderService");
    assert_eq!(node["line"].as_u64().unwrap(), 10);
    assert_eq!(node["callSite"].as_u64().unwrap(), 25);
    assert_eq!(node["file"].as_str().unwrap(), "OrderService.cs");
    assert!(node.get("callers").is_none());
    // Single call site → callSites array should NOT be present
    assert!(node.get("callSites").is_none(), "Single call site should not have callSites array");
}

#[test]
fn test_build_caller_node_no_parent() {
    let node = build_caller_node("doWork", None, 10, &[25], "src/OrderService.cs", vec![]);
    assert_eq!(node["method"].as_str().unwrap(), "doWork");
    assert!(node.get("class").is_none());
}

#[test]
fn test_build_caller_node_with_sub_callers() {
    let sub = vec![json!({"method": "run", "line": 5, "callSite": 12})];
    let node = build_caller_node("doWork", Some("OrderService"), 10, &[25], "src/OrderService.cs", sub);
    assert!(node.get("callers").is_some());
    assert_eq!(node["callers"].as_array().unwrap().len(), 1);
}

#[test]
fn test_build_caller_node_extracts_filename() {
    let node = build_caller_node("doWork", None, 10, &[25], "src/deep/nested/OrderService.cs", vec![]);
    assert_eq!(node["file"].as_str().unwrap(), "OrderService.cs");
}

#[test]
fn test_build_caller_node_multiple_call_sites() {
    // When a method is called 3 times within the same caller, callSites array should be present
    let node = build_caller_node("doWork", Some("OrderService"), 10, &[25, 40, 55], "src/OrderService.cs", vec![]);
    assert_eq!(node["callSite"].as_u64().unwrap(), 25, "callSite should be the first call site");
    let call_sites = node["callSites"].as_array().expect("callSites array should be present for >1 call sites");
    assert_eq!(call_sites.len(), 3);
    assert_eq!(call_sites[0].as_u64().unwrap(), 25);
    assert_eq!(call_sites[1].as_u64().unwrap(), 40);
    assert_eq!(call_sites[2].as_u64().unwrap(), 55);
}

// ─── SQL Test 8: SP with no schema (parent=None) resolved via no-receiver call ──

#[test]
fn test_resolve_call_site_sql_no_schema() {
    // SP with no schema, called without schema prefix
    let definitions = vec![
        sp_def(0, "usp_Cleanup", None, 1, 20),    // idx 0 — no schema
        sp_def(1, "usp_Main", Some("dbo"), 1, 50), // idx 1
    ];
    let def_idx = make_def_index(definitions, HashMap::new());

    // Call without receiver_type (no schema prefix)
    let call = CallSite {
        method_name: "usp_Cleanup".to_string(),
        receiver_type: None,
        line: 10,
        receiver_is_generic: false,
    };

    // With caller_parent=None, all matching methods are accepted
    let resolved = resolve_call_site(&call, &def_idx, None);
    assert_eq!(resolved.len(), 1, "Should resolve to usp_Cleanup without schema");
    assert_eq!(def_idx.definitions[resolved[0] as usize].name, "usp_Cleanup");
}


// ═══════════════════════════════════════════════════════════════════
// Impact Analysis Tests — is_test_method + impactAnalysis parameter
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_is_test_method_csharp_test_attribute() {
    let def = DefinitionEntry {
        file_id: 0, name: "TestSaveOrder".to_string(), kind: DefinitionKind::Method,
        line_start: 10, line_end: 30, parent: Some("OrderTests".to_string()),
        signature: None, modifiers: vec![], base_types: vec![],
        attributes: vec!["Test".to_string()],
    };
    assert!(is_test_method(&def, "src/OrderTests.cs"));
}

#[test]
fn test_is_test_method_xunit_fact() {
    let def = DefinitionEntry {
        file_id: 0, name: "ShouldSave".to_string(), kind: DefinitionKind::Method,
        line_start: 10, line_end: 30, parent: Some("OrderTests".to_string()),
        signature: None, modifiers: vec![], base_types: vec![],
        attributes: vec!["Fact".to_string()],
    };
    assert!(is_test_method(&def, "src/OrderTests.cs"));
}

#[test]
fn test_is_test_method_xunit_theory() {
    let def = DefinitionEntry {
        file_id: 0, name: "ShouldValidate".to_string(), kind: DefinitionKind::Method,
        line_start: 10, line_end: 30, parent: Some("ValidationTests".to_string()),
        signature: None, modifiers: vec![], base_types: vec![],
        attributes: vec!["Theory".to_string(), "InlineData(1)".to_string()],
    };
    assert!(is_test_method(&def, "src/ValidationTests.cs"));
}

#[test]
fn test_is_test_method_mstest_testmethod() {
    let def = DefinitionEntry {
        file_id: 0, name: "TestProcess".to_string(), kind: DefinitionKind::Method,
        line_start: 10, line_end: 30, parent: Some("ProcessTests".to_string()),
        signature: None, modifiers: vec![], base_types: vec![],
        attributes: vec!["TestMethod".to_string()],
    };
    assert!(is_test_method(&def, "src/ProcessTests.cs"));
}

#[test]
fn test_is_test_method_rust_test() {
    let def = DefinitionEntry {
        file_id: 0, name: "test_save_order".to_string(), kind: DefinitionKind::Function,
        line_start: 10, line_end: 30, parent: None,
        signature: None, modifiers: vec![], base_types: vec![],
        attributes: vec!["test".to_string()],
    };
    assert!(is_test_method(&def, "src/lib.rs"));
}

#[test]
fn test_is_test_method_rust_tokio_test() {
    let def = DefinitionEntry {
        file_id: 0, name: "test_async_save".to_string(), kind: DefinitionKind::Function,
        line_start: 10, line_end: 30, parent: None,
        signature: None, modifiers: vec![], base_types: vec![],
        attributes: vec!["tokio::test".to_string()],
    };
    assert!(is_test_method(&def, "src/lib.rs"));
}

#[test]
fn test_is_test_method_ts_spec_file() {
    let def = DefinitionEntry {
        file_id: 0, name: "shouldSaveOrder".to_string(), kind: DefinitionKind::Method,
        line_start: 10, line_end: 30, parent: Some("OrderSpec".to_string()),
        signature: None, modifiers: vec![], base_types: vec![],
        attributes: vec![],  // no attributes — TS uses file heuristic
    };
    assert!(is_test_method(&def, "src/order.spec.ts"));
}

#[test]
fn test_is_test_method_ts_test_file() {
    let def = DefinitionEntry {
        file_id: 0, name: "shouldProcess".to_string(), kind: DefinitionKind::Method,
        line_start: 10, line_end: 30, parent: Some("ProcessTest".to_string()),
        signature: None, modifiers: vec![], base_types: vec![],
        attributes: vec![],
    };
    assert!(is_test_method(&def, "src/process.test.ts"));
}

#[test]
fn test_is_test_method_tsx_spec_file() {
    let def = DefinitionEntry {
        file_id: 0, name: "shouldRender".to_string(), kind: DefinitionKind::Method,
        line_start: 10, line_end: 30, parent: Some("ComponentSpec".to_string()),
        signature: None, modifiers: vec![], base_types: vec![],
        attributes: vec![],
    };
    assert!(is_test_method(&def, "src/component.spec.tsx"));
}

#[test]
fn test_is_test_method_negative_no_attributes_no_test_file() {
    let def = DefinitionEntry {
        file_id: 0, name: "processOrder".to_string(), kind: DefinitionKind::Method,
        line_start: 10, line_end: 30, parent: Some("OrderService".to_string()),
        signature: None, modifiers: vec![], base_types: vec![],
        attributes: vec![],
    };
    assert!(!is_test_method(&def, "src/OrderService.cs"));
}

#[test]
fn test_is_test_method_negative_non_test_attribute() {
    let def = DefinitionEntry {
        file_id: 0, name: "processOrder".to_string(), kind: DefinitionKind::Method,
        line_start: 10, line_end: 30, parent: Some("OrderService".to_string()),
        signature: None, modifiers: vec![], base_types: vec![],
        attributes: vec!["Authorize".to_string(), "HttpGet".to_string()],
    };
    assert!(!is_test_method(&def, "src/OrderService.cs"));
}

#[test]
fn test_is_test_method_negative_ts_non_test_file() {
    let def = DefinitionEntry {
        file_id: 0, name: "processOrder".to_string(), kind: DefinitionKind::Method,
        line_start: 10, line_end: 30, parent: Some("OrderService".to_string()),
        signature: None, modifiers: vec![], base_types: vec![],
        attributes: vec![],
    };
    assert!(!is_test_method(&def, "src/order.service.ts"));
}

// ─── Impact analysis: handler-level tests ───────────────────────

#[test]
fn test_impact_analysis_rejects_direction_down() {
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "process", "OrderService", 5, 15),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let content_index = crate::ContentIndex::default();

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": "process",
        "direction": "down",
        "impactAnalysis": true
    }));
    assert!(result.is_error, "impactAnalysis with direction=down should return error");
    assert!(result.content[0].text.contains("direction='up'"),
        "Error should mention direction='up'");
}

#[test]
fn test_impact_analysis_false_no_tests_covering() {
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "process", "OrderService", 5, 15),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let content_index = crate::ContentIndex::default();

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    // Without impactAnalysis, response should NOT have testsCovering
    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": "process",
        "depth": 1
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v.get("testsCovering").is_none(),
        "Without impactAnalysis, should NOT have testsCovering field");
}

#[test]
fn test_impact_analysis_finds_test_methods() {
    use crate::{ContentIndex, Posting};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    // OrderService.process() is called by OrderTests.testProcess() which has [Test] attribute
    let definitions = vec![
        // idx 0: class OrderService
        DefinitionEntry { file_id: 0, name: "OrderService".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        // idx 1: OrderService.process (target method)
        DefinitionEntry { file_id: 0, name: "process".to_string(), kind: DefinitionKind::Method, line_start: 10, line_end: 20, parent: Some("OrderService".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        // idx 2: class OrderTests
        DefinitionEntry { file_id: 1, name: "OrderTests".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        // idx 3: OrderTests.testProcess — has [Test] attribute
        DefinitionEntry { file_id: 1, name: "testProcess".to_string(), kind: DefinitionKind::Method, line_start: 10, line_end: 30, parent: Some("OrderTests".to_string()), signature: None, modifiers: vec![], attributes: vec!["Test".to_string()], base_types: vec![] },
    ];

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(3, vec![
        CallSite { method_name: "process".to_string(), receiver_type: Some("OrderService".to_string()), line: 20, receiver_is_generic: false },
    ]);

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

    let files_list = vec![
        "src/OrderService.cs".to_string(),
        "src/OrderTests.cs".to_string(),
    ];
    path_to_id.insert(PathBuf::from("src/OrderService.cs"), 0);
    path_to_id.insert(PathBuf::from("src/OrderTests.cs"), 1);

    let def_idx = DefinitionIndex {
        root: ".".to_string(),
        extensions: vec!["cs".to_string()],
        files: files_list.clone(),
        definitions,
        name_index,
        kind_index,
        file_index,
        path_to_id,
        method_calls,
        ..Default::default()
    };

    // Content index
    let mut index: HashMap<String, Vec<Posting>> = HashMap::new();
    index.insert("process".to_string(), vec![
        Posting { file_id: 0, lines: vec![10] },
        Posting { file_id: 1, lines: vec![20] },
    ]);
    index.insert("orderservice".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![20] },
    ]);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: files_list,
        index,
        total_tokens: 50,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![25, 25],
        ..Default::default()
    };

    // Test via build_caller_tree directly
    let limits = CallerLimits { max_callers_per_level: 50, max_total_nodes: 200 };
    let node_count = AtomicUsize::new(0);
    let caller_ctx = CallerTreeContext {
        content_index: &content_index,
        def_idx: &def_idx,
        ext_filter: "cs",
        exclude_dir: &[],
        exclude_file: &[],
        resolve_interfaces: false,
        limits: &limits,
        node_count: &node_count,
        include_body: false,
        include_doc_comments: false,
        max_body_lines: 0,
        max_total_body_lines: 0,
        impact_analysis: true,
    };

    let mut visited = HashSet::new();
    let mut file_cache = HashMap::new();
    let mut total_body_lines = 0usize;
    let mut tests_found = Vec::new();

    let initial_chain = vec!["process".to_string()];
    let callers = build_caller_tree(
        "process", Some("OrderService"), 5, 0,
        &caller_ctx, &mut visited, &mut file_cache,
        &mut total_body_lines, &mut tests_found, &initial_chain,
    );

    // Should find the test method
    assert_eq!(callers.len(), 1, "Should find 1 caller");
    assert_eq!(callers[0]["method"].as_str().unwrap(), "testProcess");
    assert_eq!(callers[0]["isTest"].as_bool(), Some(true), "Node should be marked isTest=true");

    // tests_found collector should have the test info with full path, depth, callChain
    assert_eq!(tests_found.len(), 1, "Should find 1 test method");
    assert_eq!(tests_found[0]["method"].as_str().unwrap(), "testProcess");
    assert_eq!(tests_found[0]["class"].as_str().unwrap(), "OrderTests");
    assert_eq!(tests_found[0]["file"].as_str().unwrap(), "src/OrderTests.cs",
        "Should have full file path, not just filename");
    assert_eq!(tests_found[0]["depth"].as_u64().unwrap(), 1,
        "Direct caller should have depth=1");
    let chain = tests_found[0]["callChain"].as_array().unwrap();
    assert_eq!(chain.len(), 2, "callChain should be [target, test]");
    assert_eq!(chain[0].as_str().unwrap(), "process");
    assert_eq!(chain[1].as_str().unwrap(), "testProcess");

    // No sub-callers on test nodes (leaf)
    assert!(callers[0].get("callers").is_none(),
        "Test method nodes should not have sub-callers");
}

#[test]
fn test_impact_analysis_non_test_method_recurses_normally() {
    use crate::{ContentIndex, Posting};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    // OrderService.process() → Controller.handle() → TestController.testHandle()
    // At depth 0: Controller.handle() is NOT a test → should recurse
    // At depth 1: TestController.testHandle() IS a test → should stop
    let definitions = vec![
        DefinitionEntry { file_id: 0, name: "OrderService".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 0, name: "process".to_string(), kind: DefinitionKind::Method, line_start: 10, line_end: 20, parent: Some("OrderService".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 1, name: "Controller".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 1, name: "handle".to_string(), kind: DefinitionKind::Method, line_start: 10, line_end: 30, parent: Some("Controller".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 2, name: "ControllerTests".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 2, name: "testHandle".to_string(), kind: DefinitionKind::Method, line_start: 10, line_end: 30, parent: Some("ControllerTests".to_string()), signature: None, modifiers: vec![], attributes: vec!["Fact".to_string()], base_types: vec![] },
    ];

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(3, vec![
        CallSite { method_name: "process".to_string(), receiver_type: Some("OrderService".to_string()), line: 20, receiver_is_generic: false },
    ]);
    method_calls.insert(5, vec![
        CallSite { method_name: "handle".to_string(), receiver_type: Some("Controller".to_string()), line: 20, receiver_is_generic: false },
    ]);

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

    let files_list = vec![
        "src/OrderService.cs".to_string(),
        "src/Controller.cs".to_string(),
        "test/ControllerTests.cs".to_string(),
    ];
    for (i, f) in files_list.iter().enumerate() {
        path_to_id.insert(PathBuf::from(f), i as u32);
    }

    let def_idx = DefinitionIndex {
        root: ".".to_string(),
        extensions: vec!["cs".to_string()],
        files: files_list.clone(),
        definitions,
        name_index,
        kind_index,
        file_index,
        path_to_id,
        method_calls,
        ..Default::default()
    };

    let mut index: HashMap<String, Vec<Posting>> = HashMap::new();
    index.insert("process".to_string(), vec![
        Posting { file_id: 0, lines: vec![10] },
        Posting { file_id: 1, lines: vec![20] },
    ]);
    index.insert("handle".to_string(), vec![
        Posting { file_id: 1, lines: vec![10] },
        Posting { file_id: 2, lines: vec![20] },
    ]);
    index.insert("orderservice".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![20] },
    ]);
    index.insert("controller".to_string(), vec![
        Posting { file_id: 1, lines: vec![1] },
        Posting { file_id: 2, lines: vec![20] },
    ]);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: files_list,
        index,
        total_tokens: 100,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![30, 30, 30],
        ..Default::default()
    };

    let limits = CallerLimits { max_callers_per_level: 50, max_total_nodes: 200 };
    let node_count = AtomicUsize::new(0);
    let caller_ctx = CallerTreeContext {
        content_index: &content_index,
        def_idx: &def_idx,
        ext_filter: "cs",
        exclude_dir: &[],
        exclude_file: &[],
        resolve_interfaces: false,
        limits: &limits,
        node_count: &node_count,
        include_body: false,
        include_doc_comments: false,
        max_body_lines: 0,
        max_total_body_lines: 0,
        impact_analysis: true,
    };

    let mut visited = HashSet::new();
    let mut file_cache = HashMap::new();
    let mut total_body_lines = 0usize;
    let mut tests_found = Vec::new();

    let initial_chain = vec!["process".to_string()];
    let callers = build_caller_tree(
        "process", Some("OrderService"), 5, 0,
        &caller_ctx, &mut visited, &mut file_cache,
        &mut total_body_lines, &mut tests_found, &initial_chain,
    );

    // Depth 0: Controller.handle (NOT a test) → should have sub-callers
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0]["method"].as_str().unwrap(), "handle");
    assert!(callers[0].get("isTest").is_none(), "Non-test method should NOT have isTest");

    // Depth 1: ControllerTests.testHandle (IS a test) → leaf node
    let sub = callers[0]["callers"].as_array().expect("Non-test method should have callers array");
    assert_eq!(sub.len(), 1);
    assert_eq!(sub[0]["method"].as_str().unwrap(), "testHandle");
    assert_eq!(sub[0]["isTest"].as_bool(), Some(true));

    // tests_found should have the test with depth and callChain
    assert_eq!(tests_found.len(), 1);
    assert_eq!(tests_found[0]["method"].as_str().unwrap(), "testHandle");
    assert_eq!(tests_found[0]["class"].as_str().unwrap(), "ControllerTests");
    assert_eq!(tests_found[0]["depth"].as_u64().unwrap(), 2,
        "Test at depth 2: process → handle → testHandle");
    let chain = tests_found[0]["callChain"].as_array().unwrap();
    assert_eq!(chain.len(), 3, "callChain: [process, handle, testHandle]");
    assert_eq!(chain[0].as_str().unwrap(), "process");
    assert_eq!(chain[1].as_str().unwrap(), "handle");
    assert_eq!(chain[2].as_str().unwrap(), "testHandle");
}

// ═══════════════════════════════════════════════════════════════════
// Multi-method batch tests
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_multi_method_returns_results_array() {
    // When method contains commas, response should have "results" array
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "process", "OrderService", 5, 15),
        method_def(0, "validate", "OrderService", 20, 30),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let content_index = crate::ContentIndex::default();

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": "process,validate",
        "depth": 1
    }));
    assert!(!result.is_error, "Multi-method should not error: {:?}", result.content[0].text);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();

    // Should have "results" array with 2 entries
    let results = v["results"].as_array().expect("Should have results array");
    assert_eq!(results.len(), 2, "Should have 2 method results");
    assert_eq!(results[0]["method"].as_str().unwrap(), "process");
    assert_eq!(results[1]["method"].as_str().unwrap(), "validate");

    // Each result should have callTree
    assert!(results[0].get("callTree").is_some(), "Each result should have callTree");
    assert!(results[1].get("callTree").is_some(), "Each result should have callTree");

    // Summary should have totalMethods
    assert_eq!(v["summary"]["totalMethods"].as_u64().unwrap(), 2);

    // Query should have methods array
    let methods = v["query"]["methods"].as_array().expect("Should have methods in query");
    assert_eq!(methods.len(), 2);
}

#[test]
fn test_single_method_no_comma_returns_calltree_directly() {
    // Single method (no comma) should return backward-compatible format
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "process", "OrderService", 5, 15),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let content_index = crate::ContentIndex::default();

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": "process",
        "depth": 1
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();

    // Should have callTree directly (not results array)
    assert!(v.get("callTree").is_some(), "Single method should have callTree directly");
    assert!(v.get("results").is_none(), "Single method should NOT have results array");

    // Query should have "method" (string), not "methods" (array)
    assert!(v["query"]["method"].is_string(), "Query should have method string");
}

#[test]
fn test_multi_method_with_spaces_trimmed() {
    // Spaces around method names should be trimmed
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "process", "OrderService", 5, 15),
        method_def(0, "validate", "OrderService", 20, 30),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let content_index = crate::ContentIndex::default();

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": " process , validate ",
        "depth": 1
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let results = v["results"].as_array().expect("Should have results");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["method"].as_str().unwrap(), "process");
    assert_eq!(results[1]["method"].as_str().unwrap(), "validate");
}

#[test]
fn test_multi_method_empty_after_split_returns_error() {
    // Edge case: only commas and spaces
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let content_index = crate::ContentIndex::default();

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": " , , ",
        "depth": 1
    }));
    assert!(result.is_error, "Empty method list should return error");
}

#[test]
fn test_multi_method_each_gets_independent_nodes() {
    // Each method should get its own nodesInTree count
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "process", "OrderService", 5, 15),
        method_def(0, "validate", "OrderService", 20, 30),
        method_def(0, "save", "OrderService", 35, 45),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let content_index = crate::ContentIndex::default();

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": "process,validate,save",
        "depth": 1
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let results = v["results"].as_array().unwrap();
    assert_eq!(results.len(), 3);

    // Each should have nodesInTree field
    for r in results {
        assert!(r.get("nodesInTree").is_some(), "Each result should have nodesInTree");
    }
}

#[test]
fn test_multi_method_with_class_filter() {
    // Class filter should be applied to all methods
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "process", "OrderService", 5, 15),
        method_def(0, "validate", "OrderService", 20, 30),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let content_index = crate::ContentIndex::default();

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": "process,validate",
        "class": "OrderService",
        "depth": 1
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(v["query"]["class"].as_str().unwrap(), "OrderService");
}

#[test]
fn test_multi_method_direction_down() {
    // Multi-method should work with direction=down too
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "process", "OrderService", 5, 15),
        method_def(0, "validate", "OrderService", 20, 30),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let content_index = crate::ContentIndex::default();

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": "process,validate",
        "direction": "down",
        "depth": 1
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(v["query"]["direction"].as_str().unwrap(), "down");
    let results = v["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
}


// ═══════════════════════════════════════════════════════════════════
// Cross-index enrichment tests: includeGrepReferences
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_include_grep_references_finds_extra_files() {
    // Content index has method name in files not present in call tree
    use crate::{ContentIndex, Posting};
    use std::path::PathBuf;

    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "ProcessOrder", "OrderService", 5, 15),
        class_def(1, "Consumer", vec![]),
        method_def(1, "Run", "Consumer", 5, 20),
    ];

    let mut method_calls_map: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls_map.insert(3, vec![
        CallSite {
            method_name: "ProcessOrder".to_string(),
            receiver_type: Some("OrderService".to_string()),
            line: 10,
            receiver_is_generic: false,
        },
    ]);

    let mut def_idx = make_def_index(definitions, method_calls_map);
    def_idx.path_to_id.insert(PathBuf::from("src/OrderController.ts"), 0);
    def_idx.path_to_id.insert(PathBuf::from("src/OrderValidator.ts"), 1);

    let mut index: HashMap<String, Vec<Posting>> = HashMap::new();
    index.insert("processorder".to_string(), vec![
        Posting { file_id: 0, lines: vec![5] },
        Posting { file_id: 1, lines: vec![10] },
        Posting { file_id: 2, lines: vec![42] },  // extra file not in def index
    ]);
    index.insert("orderservice".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![10] },
    ]);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "src/OrderController.ts".to_string(),
            "src/OrderValidator.ts".to_string(),
            "src/Pipelines/ValidationPipeline.ts".to_string(),
        ],
        index,
        total_tokens: 100,
        extensions: vec!["ts".to_string()],
        file_token_counts: vec![50, 50, 50],
        ..Default::default()
    };

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": "ProcessOrder",
        "class": "OrderService",
        "depth": 1,
        "includeGrepReferences": true
    }));
    assert!(!result.is_error, "Should not error: {:?}", result.content[0].text);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();

    // grepReferences should include ValidationPipeline.ts (not in call tree)
    let grep_refs = v.get("grepReferences");
    assert!(grep_refs.is_some(), "Should have grepReferences. Got: {}", serde_json::to_string_pretty(&v).unwrap());
    let refs = grep_refs.unwrap().as_array().unwrap();
    assert!(!refs.is_empty(), "grepReferences should not be empty");
    assert!(v.get("grepReferencesNote").is_some(), "Should have grepReferencesNote");
}

#[test]
fn test_include_grep_references_default_off() {
    // Without includeGrepReferences param, no grepReferences in output
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "ProcessOrder", "OrderService", 5, 15),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let content_index = crate::ContentIndex::default();

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": "ProcessOrder",
        "depth": 1
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v.get("grepReferences").is_none(),
        "Without includeGrepReferences, should NOT have grepReferences in output");
}

// ═══════════════════════════════════════════════════════════════════
// Nearest-match hint tests for xray_callers
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_callers_hint_nearest_method_name() {
    // Typo in method name → hint should suggest nearest match
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "ProcessOrder", "OrderService", 5, 15),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let content_index = crate::ContentIndex::default();

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    // Typo: "ProcessOrdr" instead of "ProcessOrder"
    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": "ProcessOrdr",
        "depth": 1
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let hint = v.get("hint").and_then(|h| h.as_str());
    assert!(hint.is_some(), "Should have hint for typo. Got: {}", serde_json::to_string_pretty(&v).unwrap());
    let h = hint.unwrap();
    assert!(h.contains("Nearest match") || h.contains("not found"),
        "Hint should mention nearest match. Got: {}", h);
}

#[test]
fn test_callers_hint_nearest_class_name() {
    // Typo in class name → hint should suggest nearest class
    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "ProcessOrder", "OrderService", 5, 15),
    ];
    let def_idx = make_def_index(definitions, HashMap::new());
    let content_index = crate::ContentIndex::default();

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    // Typo: "OrderServise" instead of "OrderService"
    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": "ProcessOrder",
        "class": "OrderServise",
        "depth": 1
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let hint = v.get("hint").and_then(|h| h.as_str());
    assert!(hint.is_some(), "Should have hint for class typo. Got: {}", serde_json::to_string_pretty(&v).unwrap());
    let h = hint.unwrap();
    assert!(h.contains("Nearest") || h.contains("not found"),
        "Hint should suggest nearest class. Got: {}", h);
}

#[test]
fn test_callers_hint_not_shown_when_results_exist() {
    // When callers ARE found, no hint should appear (regression guard)
    use crate::{ContentIndex, Posting};
    use std::path::PathBuf;

    let definitions = vec![
        class_def(0, "OrderService", vec![]),
        method_def(0, "process", "OrderService", 5, 15),
        class_def(1, "Consumer", vec![]),
        method_def(1, "run", "Consumer", 5, 20),
    ];

    let mut method_calls_map: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls_map.insert(3, vec![
        CallSite {
            method_name: "process".to_string(),
            receiver_type: Some("OrderService".to_string()),
            line: 10,
            receiver_is_generic: false,
        },
    ]);

    let mut def_idx = make_def_index(definitions, method_calls_map);
    def_idx.path_to_id.insert(PathBuf::from("src/OrderController.ts"), 0);
    def_idx.path_to_id.insert(PathBuf::from("src/OrderValidator.ts"), 1);

    let mut index: HashMap<String, Vec<Posting>> = HashMap::new();
    index.insert("process".to_string(), vec![
        Posting { file_id: 0, lines: vec![5] },
        Posting { file_id: 1, lines: vec![10] },
    ]);
    index.insert("orderservice".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![10] },
    ]);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec!["src/OrderController.ts".to_string(), "src/OrderValidator.ts".to_string()],
        index,
        total_tokens: 100,
        extensions: vec!["ts".to_string()],
        file_token_counts: vec![50, 50],
        ..Default::default()
    };

    let ctx = super::HandlerContext {
        index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
        def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
        ..Default::default()
    };

    let result = handle_xray_callers(&ctx, &serde_json::json!({
        "method": "process",
        "class": "OrderService",
        "depth": 1
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = v["callTree"].as_array().unwrap();
    if !tree.is_empty() {
        assert!(v.get("hint").is_none(),
            "Should NOT have hint when callers are found. Got: {:?}", v.get("hint"));
    }
}
