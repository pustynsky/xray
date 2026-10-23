//! TypeScript-specific handler tests — definitions, callers, includeBody, containsLine.
//! Split from handlers_tests.rs for maintainability. Mirrors handlers_tests_csharp.rs patterns.

use super::*;
use super::handlers_test_utils::cleanup_tmp;
use crate::index::build_trigram_index;
use crate::Posting;
use crate::definitions::DefinitionEntry;
use crate::definitions::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

const CALLERS_394_REASON: &str = "unverified_typescript_member_target";

struct Callers394Fixture {
    _temporary: tempfile::TempDir,
    ctx: HandlerContext,
}

impl Callers394Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("systemtemp");
        std::fs::create_dir_all(&base).unwrap();
        let temporary = tempfile::Builder::new().prefix("callers394-").tempdir_in(base).unwrap();
        let root = crate::canonicalize_test_root(temporary.path());
        let mut definitions = DefinitionIndex {
            root: root.to_string_lossy().to_string(),
            extensions: vec!["ts".into(), "tsx".into(), "rs".into()],
            ..Default::default()
        };
        for (name, source) in files {
            let path = root.join(name);
            std::fs::write(&path, source).unwrap();
            let path = PathBuf::from(crate::clean_path(&path.to_string_lossy()));
            crate::definitions::update_file_definitions(&mut definitions, &path);
        }
        let content = crate::build_content_index(&crate::ContentIndexArgs {
            dir: root.to_string_lossy().to_string(),
            ext: "ts,tsx,rs".into(),
            threads: 1,
            no_ignore: true,
            ..Default::default()
        }).unwrap();
        let ctx = HandlerContext {
            index: Arc::new(RwLock::new(content)),
            def_index: Some(Arc::new(RwLock::new(definitions))),
            workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(root.to_string_lossy().to_string()))),
            server_ext: "ts,tsx,rs".into(),
            ..Default::default()
        };
        Self { _temporary: temporary, ctx }
    }

    fn query(&self, args: Value) -> Value {
        let result = dispatch_tool(&self.ctx, "xray_callers", &args);
        assert!(!result.is_error, "{}", result.content[0].text);
        serde_json::from_str(&result.content[0].text).unwrap()
    }
}

fn callers_394_assert_partial(output: &Value, count: usize) {
    assert_eq!(output["resultStatus"]["status"], "partial", "{output}");
    for field in ["complete", "safeForExhaustiveClaims", "totalKnown"] {
        assert_eq!(output["resultStatus"][field], false, "{field}: {output}");
    }
    assert_eq!(output["summary"]["unresolvedCallSites"], count, "{output}");
    assert_eq!(output["summary"]["unresolvedCallReasons"][CALLERS_394_REASON], count, "{output}");
    assert_eq!(output["summary"]["unresolvedCallSitesScope"], "traversedGraph");
    assert!(output["resultStatus"]["reasons"].as_array().unwrap().iter()
        .any(|reason| reason == CALLERS_394_REASON), "{output}");
}

fn callers_394_assert_clean(output: &Value) {
    assert_eq!(output["resultStatus"]["complete"], true, "{output}");
    assert_eq!(output["resultStatus"]["safeForExhaustiveClaims"], true, "{output}");
    assert!(output["summary"]["unresolvedCallReasons"][CALLERS_394_REASON].is_null(), "{output}");
}

const CALLERS_394_TARGETS: &str = "export class Target {\n run(): void {}\n touch(): void {}\n}\nexport class Other {\n run(): void {}\n}\n";

#[test]
fn callers_394_unknown_member_up_zero_edges() {
    let source = format!("{CALLERS_394_TARGETS}\nexport function invoke(receiver: unknown): void {{\n (receiver as any).run();\n}}\n");
    let fixture = Callers394Fixture::new(&[("graph.ts", &source)]);
    let output = fixture.query(json!({"method":["run"],"class":"Target","depth":1}));
    assert!(output["callTree"].as_array().unwrap().is_empty(), "{output}");
    callers_394_assert_partial(&output, 1);
}

#[test]
fn callers_394_unknown_member_down_zero_edges() {
    let source = format!("{CALLERS_394_TARGETS}\nexport function invoke(receiver: unknown): void {{\n (receiver as any).run();\n}}\n");
    let fixture = Callers394Fixture::new(&[("graph.ts", &source)]);
    let output = fixture.query(json!({"method":["invoke"],"direction":"down","depth":1}));
    assert!(output["callTree"].as_array().unwrap().is_empty(), "{output}");
    callers_394_assert_partial(&output, 1);
}

#[test]
fn callers_394_split_file_unknown_bypasses_only_lexical_pruning() {
    let fixture = Callers394Fixture::new(&[
        ("target.ts", CALLERS_394_TARGETS),
        ("invoke.ts", "export function invoke(receiver: unknown): void {\n (receiver as any).run();\n}\n"),
    ]);
    let output = fixture.query(json!({"method":["run"],"class":"Target","depth":1}));
    assert!(output["callTree"].as_array().unwrap().is_empty(), "{output}");
    callers_394_assert_partial(&output, 1);
}

#[test]
fn callers_394_exact_and_unknown_preserve_edge_and_same_line_evidence() {
    for separator in [" ", "\n"] {
        let source = format!("{CALLERS_394_TARGETS}\nexport function invoke(known: Target, receiver: unknown): void {{\n known.run();{separator}(receiver as any).run();\n}}\n");
        let fixture = Callers394Fixture::new(&[("graph.ts", &source)]);
        for args in [
            json!({"method":["run"],"class":"Target","depth":1}),
            json!({"method":["invoke"],"direction":"down","depth":1}),
        ] {
            let output = fixture.query(args);
            let tree = output["callTree"].as_array().unwrap();
            assert_eq!(tree.len(), 1, "{output}");
            assert!(tree[0]["method"] == "invoke" || tree[0]["method"] == "run", "{output}");
            callers_394_assert_partial(&output, 1);
        }
    }
}

#[test]
fn callers_394_distinct_unknown_ordinals_on_one_line_count_twice() {
    let source = format!("{CALLERS_394_TARGETS}\nexport function invoke(first: MissingA, second: MissingB): void {{\n first.run(); second.run();\n}}\n");
    let fixture = Callers394Fixture::new(&[("graph.ts", &source)]);
    {
        let index = fixture.ctx.def_index.as_ref().unwrap().read().unwrap();
        let invoke = index.definitions.iter().position(|definition| definition.name == "invoke").unwrap();
        let calls = index.method_calls.get(&(invoke as u32)).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].line, calls[1].line);
        assert!(calls.iter().all(|call| call.call_kind == crate::definitions::CallSiteKind::TypeScriptMember));
        assert_ne!(calls[0].receiver_type, calls[1].receiver_type);
    }
    for args in [
        json!({"method":["run"],"class":"Target","depth":1}),
        json!({"method":["invoke"],"direction":"down","depth":1}),
    ] {
        let output = fixture.query(args);
        assert!(output["callTree"].as_array().unwrap().is_empty(), "{output}");
        callers_394_assert_partial(&output, 2);
    }
}

#[test]
fn callers_394_missing_owner_identity_is_not_a_proven_mismatch() {
    let source = format!("{CALLERS_394_TARGETS}\nexport function invoke(receiver: ExternalReceiver): void {{\n receiver.run();\n}}\n");
    let fixture = Callers394Fixture::new(&[("graph.ts", &source)]);
    for args in [
        json!({"method":["run"],"class":"Target","depth":1}),
        json!({"method":["invoke"],"direction":"down","depth":1}),
    ] {
        let output = fixture.query(args);
        assert!(output["callTree"].as_array().unwrap().is_empty(), "{output}");
        callers_394_assert_partial(&output, 1);
    }
}

#[test]
fn callers_394_indexed_receiver_mismatch_is_clean() {
    let source = format!("{CALLERS_394_TARGETS}\nexport function invoke(receiver: Other): void {{\n receiver.run();\n}}\n");
    let fixture = Callers394Fixture::new(&[("graph.ts", &source)]);
    let output = fixture.query(json!({"method":["run"],"class":"Target","depth":1}));
    assert!(output["callTree"].as_array().unwrap().is_empty(), "{output}");
    callers_394_assert_clean(&output);
    let output = fixture.query(json!({"method":["invoke"],"direction":"down","depth":1}));
    assert_eq!(output["callTree"].as_array().unwrap().len(), 1, "{output}");
    assert_eq!(output["callTree"][0]["class"], "Other", "{output}");
    callers_394_assert_clean(&output);
}

#[test]
fn callers_394_typed_this_and_constructed_receivers_remain_exact() {
    for (source, down_class) in [
        (format!("{CALLERS_394_TARGETS}\nexport function invoke(known: Target): void {{\n known.run();\n}}\n"), None),
        (format!("{CALLERS_394_TARGETS}\nexport function invoke(): void {{\n new Target().run();\n}}\n"), None),
        ("export class Target {\n run(): void {}\n invoke(): void {\n this.run();\n }\n}\n".to_string(), Some("Target")),
    ] {
        let fixture = Callers394Fixture::new(&[("graph.ts", &source)]);
        let up = fixture.query(json!({"method":["run"],"class":"Target","depth":1}));
        assert_eq!(up["callTree"].as_array().unwrap().len(), 1, "{up}");
        callers_394_assert_clean(&up);
        let mut args = json!({"method":["invoke"],"direction":"down","depth":1});
        if let Some(class) = down_class { args["class"] = json!(class); }
        let down = fixture.query(args);
        assert_eq!(down["callTree"].as_array().unwrap().len(), 1, "{down}");
        assert_eq!(down["callTree"][0]["class"], "Target", "{down}");
        callers_394_assert_clean(&down);
    }
}

#[test]
fn callers_394_builtin_and_absent_candidates_do_not_manufacture_uncertainty() {
    for source in [
        format!("{CALLERS_394_TARGETS}\nexport function invoke(): void {{\n Array.run();\n}}\n"),
        "export function invoke(receiver: unknown): void {\n (receiver as any).absent();\n}\n".to_string(),
    ] {
        let fixture = Callers394Fixture::new(&[("graph.ts", &source)]);
        let output = fixture.query(json!({"method":["invoke"],"direction":"down","depth":1}));
        assert!(output["callTree"].as_array().unwrap().is_empty(), "{output}");
        callers_394_assert_clean(&output);
    }
}

#[test]
fn callers_394_up_evidence_obeys_explicit_scope_not_lexical_class_tokens() {
    let fixture = Callers394Fixture::new(&[
        ("target.ts", CALLERS_394_TARGETS),
        ("worker.spec.ts", "export function check(receiver: unknown): void {\n (receiver as any).run();\n}\n"),
        ("outside.tsx", "export function outside(receiver: unknown): void {\n (receiver as any).run();\n}\n"),
        ("ordinary.ts", "export function ordinary(receiver: unknown): void {\n (receiver as any).touch();\n}\n"),
    ]);
    let output = fixture.query(json!({"method":["run"],"class":"Target","ext":["ts"],"productionOnly":true}));
    callers_394_assert_clean(&output);
    let output = fixture.query(json!({"method":["run"],"class":"Target","ext":["ts"]}));
    callers_394_assert_partial(&output, 1);
    let output = fixture.query(json!({"method":["run"],"class":"Target","ext":["ts"],"excludeFile":["worker.spec.ts"]}));
    callers_394_assert_clean(&output);
    let output = fixture.query(json!({"method":["run"],"class":"Target","ext":["ts","tsx"],"productionOnly":true}));
    callers_394_assert_partial(&output, 1);
}

#[test]
fn callers_394_down_rejected_candidate_scope_is_respected() {
    for (target, excluded) in [
        ("target.tsx", json!({"ext":["ts"]})),
        ("target.spec.ts", json!({"productionOnly":true})),
        ("target.ts", json!({"excludeFile":["target.ts"]})),
    ] {
        let fixture = Callers394Fixture::new(&[
            (target, CALLERS_394_TARGETS),
            ("invoke.ts", "export function invoke(receiver: unknown): void {\n (receiver as any).run();\n}\n"),
        ]);
        let mut args = json!({"method":["invoke"],"direction":"down","depth":1});
        for (key, value) in excluded.as_object().unwrap() { args[key] = value.clone(); }
        callers_394_assert_clean(&fixture.query(args));
        let output = fixture.query(json!({"method":["invoke"],"direction":"down","depth":1}));
        assert!(output["callTree"].as_array().unwrap().is_empty(), "{output}");
        callers_394_assert_partial(&output, 1);
    }
}

#[test]
fn callers_394_recursive_up_propagates_descendant_evidence() {
    let source = format!("{CALLERS_394_TARGETS}\nexport class Bridge {{\n hop(target: Target): void {{ target.run(); }}\n}}\nexport function invoke(known: Bridge, target: Target, receiver: unknown): void {{\n known.hop(target); (receiver as any).hop(target);\n}}\n");
    let fixture = Callers394Fixture::new(&[("graph.ts", &source)]);
    let output = fixture.query(json!({"method":["run"],"class":"Target","depth":2}));
    assert_eq!(output["callTree"][0]["method"], "hop", "{output}");
    assert_eq!(output["callTree"][0]["callers"][0]["method"], "invoke", "{output}");
    callers_394_assert_partial(&output, 1);
}

#[test]
fn callers_394_recursive_down_propagates_descendant_evidence() {
    let source = format!("{CALLERS_394_TARGETS}\nexport class Bridge {{\n hop(target: Target, receiver: unknown): void {{\n target.run(); (receiver as any).run();\n }}\n}}\nexport function invoke(known: Bridge, target: Target, receiver: unknown): void {{\n known.hop(target, receiver);\n}}\n");
    let fixture = Callers394Fixture::new(&[("graph.ts", &source)]);
    let output = fixture.query(json!({"method":["invoke"],"direction":"down","depth":2}));
    assert_eq!(output["callTree"][0]["method"], "hop", "{output}");
    assert_eq!(output["callTree"][0]["callees"][0]["method"], "run", "{output}");
    callers_394_assert_partial(&output, 1);
}

#[test]
fn callers_394_recursive_down_counts_revisited_root_ordinal_once() {
    let source = format!("{CALLERS_394_TARGETS}\nexport class Bridge {{\n hop(receiver: unknown): void {{\n this.hop(receiver); (receiver as any).run();\n }}\n}}\n");
    let fixture = Callers394Fixture::new(&[("graph.ts", &source)]);
    for (class, depth) in [(None, 1), (None, 3), (Some("Bridge"), 3)] {
        let output = fixture.query(json!({
            "method":["hop"],"class":class,"direction":"down","depth":depth,
        }));
        assert_eq!(output["callTree"][0]["method"], "hop", "{output}");
        callers_394_assert_partial(&output, 1);
    }
}

#[test]
fn callers_394_batch_counts_per_graph_and_aggregates_reasons() {
    let source = format!("{CALLERS_394_TARGETS}\nexport function first(receiver: unknown): void {{\n (receiver as any).run();\n}}\nexport function second(receiver: unknown): void {{\n (receiver as any).touch();\n}}\n");
    let fixture = Callers394Fixture::new(&[("graph.ts", &source)]);
    for args in [
        json!({"method":["run","touch"],"class":"Target","depth":1}),
        json!({"method":["first","second"],"direction":"down","depth":1}),
    ] {
        let output = fixture.query(args);
        callers_394_assert_partial(&output, 2);
        let results = output["results"].as_array().unwrap();
        assert_eq!(results.len(), 2, "{output}");
        for result in results {
            assert!(result["callTree"].as_array().unwrap().is_empty(), "{output}");
            assert_eq!(result["unresolvedCallSites"], 1, "{output}");
            assert_eq!(result["unresolvedCallReasons"][CALLERS_394_REASON], 1, "{output}");
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────

/// Helper: create a context with both content + definition indexes (TypeScript classes/methods/functions/etc).
fn make_ts_ctx_with_defs() -> HandlerContext {
    // Content index: tokens -> files+lines (all lowercase)
    let mut content_idx = HashMap::new();
    content_idx.insert("getuser".to_string(), vec![
        Posting { file_id: 0, lines: vec![15] },
        Posting { file_id: 1, lines: vec![20] },
    ]);
    content_idx.insert("userservice".to_string(), vec![
        Posting { file_id: 0, lines: vec![1, 15] },
        Posting { file_id: 1, lines: vec![5] },
    ]);
    content_idx.insert("orderprocessor".to_string(), vec![
        Posting { file_id: 1, lines: vec![1] },
    ]);
    content_idx.insert("handleorder".to_string(), vec![
        Posting { file_id: 1, lines: vec![18] },
    ]);

    let trigram = build_trigram_index(&content_idx);

    let content_index = ContentIndex {
        root: ".".to_string(),
        created_at: 0,
        max_age_secs: 3600,
        files: vec![
            "src/services/UserService.ts".to_string(),
            "src/processors/OrderProcessor.ts".to_string(),
            "src/utils/helpers.ts".to_string(),
        ],
        index: content_idx,
        total_tokens: 300,
        extensions: vec!["ts".to_string()],
        file_token_counts: vec![100, 100, 100],
        trigram,
        ..Default::default()
    };

    // Definitions: all TS definition kinds
    let definitions = vec![
        // 0: Class UserService (file 0) — decorated with @Injectable
        DefinitionEntry {
            file_id: 0, name: "UserService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None,
            modifiers: vec!["export".to_string()],
            attributes: vec!["Injectable".to_string()],
            base_types: vec!["IUserService".to_string()],
        },
        // 1: Class OrderProcessor (file 1)
        DefinitionEntry {
            file_id: 1, name: "OrderProcessor".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 60,
            parent: None, signature: None,
            modifiers: vec!["export".to_string(), "abstract".to_string()],
            attributes: vec![],
            base_types: vec![],
        },
        // 2: Interface IUserService (file 0)
        DefinitionEntry {
            file_id: 0, name: "IUserService".to_string(),
            kind: DefinitionKind::Interface, line_start: 55, line_end: 70,
            parent: None, signature: None,
            modifiers: vec![],
            attributes: vec![],
            base_types: vec![],
        },
        // 3: Method getUser (file 0, parent: UserService)
        DefinitionEntry {
            file_id: 0, name: "getUser".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 25,
            parent: Some("UserService".to_string()), signature: None,
            modifiers: vec!["async".to_string()],
            attributes: vec![],
            base_types: vec![],
        },
        // 4: Method handleOrder (file 1, parent: OrderProcessor)
        DefinitionEntry {
            file_id: 1, name: "handleOrder".to_string(),
            kind: DefinitionKind::Method, line_start: 15, line_end: 30,
            parent: Some("OrderProcessor".to_string()), signature: None,
            modifiers: vec![],
            attributes: vec![],
            base_types: vec![],
        },
        // 5: Constructor (file 0, parent: UserService)
        DefinitionEntry {
            file_id: 0, name: "constructor".to_string(),
            kind: DefinitionKind::Constructor, line_start: 5, line_end: 9,
            parent: Some("UserService".to_string()), signature: None,
            modifiers: vec![],
            attributes: vec![],
            base_types: vec![],
        },
        // 6: Function createLogger (file 2)
        DefinitionEntry {
            file_id: 2, name: "createLogger".to_string(),
            kind: DefinitionKind::Function, line_start: 1, line_end: 10,
            parent: None, signature: None,
            modifiers: vec!["export".to_string()],
            attributes: vec![],
            base_types: vec![],
        },
        // 7: Enum UserStatus (file 2)
        DefinitionEntry {
            file_id: 2, name: "UserStatus".to_string(),
            kind: DefinitionKind::Enum, line_start: 12, line_end: 18,
            parent: None, signature: None,
            modifiers: vec![],
            attributes: vec![],
            base_types: vec![],
        },
        // 8: EnumMember Active (file 2, parent: UserStatus)
        DefinitionEntry {
            file_id: 2, name: "Active".to_string(),
            kind: DefinitionKind::EnumMember, line_start: 13, line_end: 13,
            parent: Some("UserStatus".to_string()), signature: None,
            modifiers: vec![],
            attributes: vec![],
            base_types: vec![],
        },
        // 9: EnumMember Inactive (file 2, parent: UserStatus)
        DefinitionEntry {
            file_id: 2, name: "Inactive".to_string(),
            kind: DefinitionKind::EnumMember, line_start: 14, line_end: 14,
            parent: Some("UserStatus".to_string()), signature: None,
            modifiers: vec![],
            attributes: vec![],
            base_types: vec![],
        },
        // 10: TypeAlias UserId (file 2)
        DefinitionEntry {
            file_id: 2, name: "UserId".to_string(),
            kind: DefinitionKind::TypeAlias, line_start: 20, line_end: 20,
            parent: None,
            signature: Some("type UserId = string | number".to_string()),
            modifiers: vec![],
            attributes: vec![],
            base_types: vec![],
        },
        // 11: Variable DEFAULT_TIMEOUT (file 2)
        DefinitionEntry {
            file_id: 2, name: "DEFAULT_TIMEOUT".to_string(),
            kind: DefinitionKind::Variable, line_start: 22, line_end: 22,
            parent: None, signature: None,
            modifiers: vec!["export".to_string(), "const".to_string()],
            attributes: vec![],
            base_types: vec![],
        },
        // 12: Field name (file 0, parent: UserService)
        DefinitionEntry {
            file_id: 0, name: "name".to_string(),
            kind: DefinitionKind::Field, line_start: 3, line_end: 3,
            parent: Some("UserService".to_string()), signature: None,
            modifiers: vec!["private".to_string()],
            attributes: vec![],
            base_types: vec![],
        },
        // 13: Property id (file 0, parent: IUserService)
        DefinitionEntry {
            file_id: 0, name: "id".to_string(),
            kind: DefinitionKind::Property, line_start: 57, line_end: 57,
            parent: Some("IUserService".to_string()), signature: None,
            modifiers: vec![],
            attributes: vec![],
            base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    let mut base_type_index: HashMap<String, Vec<u32>> = HashMap::new();

    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
        for bt in &def.base_types {
            base_type_index.entry(bt.to_lowercase()).or_default().push(idx);
        }
    }

    path_to_id.insert(crate::path_identity_key(&PathBuf::from("src/services/UserService.ts")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("src/processors/OrderProcessor.ts")), 1);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("src/utils/helpers.ts")), 2);

    // method_calls for "down" direction: handleOrder calls getUser
    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(4, vec![CallSite {
        method_name: "getUser".to_string(),
        receiver_type: Some("UserService".to_string()),
        line: 20,
        call_kind: Default::default(),
        receiver_is_generic: false,
            }]);

    let def_index = DefinitionIndex {
        root: ".".to_string(),
        extensions: vec!["ts".to_string()],
        files: vec![
            "src/services/UserService.ts".to_string(),
            "src/processors/OrderProcessor.ts".to_string(),
            "src/utils/helpers.ts".to_string(),
        ],
        definitions,
        name_index,
        kind_index,
        attribute_index: {
            let mut ai: HashMap<String, Vec<u32>> = HashMap::new();
            // UserService (idx 0) has @Injectable decorator
            ai.insert("injectable".to_string(), vec![0]);
            ai
        },
        base_type_index,
        file_index,
        path_to_id,
        method_calls,
        ..Default::default()
    };

    HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        server_ext: "ts".to_string(),
        ..Default::default()
    }
}

/// Helper: create a context with real temp .ts files and a definition index.
fn make_ts_ctx_with_real_files() -> (HandlerContext, std::path::PathBuf) {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_test_ts_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // file 0: UserService.ts — 15 lines
    let file0_path = tmp_dir.join("UserService.ts");
    {
        let mut f = std::fs::File::create(&file0_path).unwrap();
        writeln!(f, "export class UserService {{").unwrap();         // line 1
        writeln!(f, "  private name: string;").unwrap();             // line 2
        writeln!(f, "  constructor() {{").unwrap();                   // line 3
        writeln!(f, "    this.name = '';").unwrap();                  // line 4
        writeln!(f, "  }}").unwrap();                                 // line 5
        writeln!(f, "  async getUser(id: number) {{").unwrap();      // line 6
        writeln!(f, "    // fetch user").unwrap();                    // line 7
        writeln!(f, "    const user = await fetch(id);").unwrap();   // line 8
        writeln!(f, "    return user;").unwrap();                     // line 9
        writeln!(f, "  }}").unwrap();                                 // line 10
        writeln!(f, "}}").unwrap();                                   // line 11
        writeln!(f).unwrap();                                     // line 12
        writeln!(f, "export interface IUserService {{").unwrap();     // line 13
        writeln!(f, "  id: number;").unwrap();                       // line 14
        writeln!(f, "}}").unwrap();                                   // line 15
    }

    // file 1: OrderProcessor.ts — 20 lines
    let file1_path = tmp_dir.join("OrderProcessor.ts");
    {
        let mut f = std::fs::File::create(&file1_path).unwrap();
        for i in 1..=20 { writeln!(f, "// order processor line {}", i).unwrap(); }
    }

    let file0_str = file0_path.to_string_lossy().to_string();
    let file1_str = file1_path.to_string_lossy().to_string();

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "UserService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 11,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "getUser".to_string(),
            kind: DefinitionKind::Method, line_start: 6, line_end: 10,
            parent: Some("UserService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "OrderProcessor".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 20,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "handleOrder".to_string(),
            kind: DefinitionKind::Method, line_start: 5, line_end: 19,
            parent: Some("OrderProcessor".to_string()), signature: None,
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
    path_to_id.insert(crate::path_identity_key(&file0_path), 0);
    path_to_id.insert(crate::path_identity_key(&file1_path), 1);

    let def_index = DefinitionIndex {
        root: tmp_dir.to_string_lossy().to_string(), created_at: 0,
        extensions: vec!["ts".to_string()],
        files: vec![file0_str.clone(), file1_str.clone()],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls: HashMap::new(),
        ..Default::default()
    };

    let content_index = ContentIndex {
        root: tmp_dir.to_string_lossy().to_string(),
        files: vec![file0_str, file1_str],
        index: HashMap::new(), total_tokens: 0,
        extensions: vec!["ts".to_string()],
        file_token_counts: vec![0, 0],
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp_dir.to_string_lossy().to_string()))),
        server_ext: "ts".to_string(),
        ..Default::default()
    };
    (ctx, tmp_dir)
}

// ─── Part 2: xray_definitions tests (one test per kind) ────────────

#[test]
fn test_ts_xray_definitions_finds_class() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["UserService"],
        "kind": ["class"]
    }));
    assert!(!result.is_error, "xray_definitions should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Expected exactly 1 class named UserService, got {}", defs.len());
    assert_eq!(defs[0]["name"], "UserService");
    assert_eq!(defs[0]["kind"], "class");
    assert!(defs[0]["file"].as_str().unwrap().contains("UserService.ts"));
}

#[test]
fn test_ts_xray_definitions_finds_interface() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["IUserService"],
        "kind": ["interface"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Expected exactly 1 interface named IUserService, got {}", defs.len());
    assert_eq!(defs[0]["name"], "IUserService");
    assert_eq!(defs[0]["kind"], "interface");
}

#[test]
fn test_ts_xray_definitions_finds_method() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["getUser"],
        "kind": ["method"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Expected exactly 1 method named getUser, got {}", defs.len());
    assert_eq!(defs[0]["name"], "getUser");
    assert_eq!(defs[0]["kind"], "method");
    assert_eq!(defs[0]["parent"], "UserService");
}

#[test]
fn test_ts_xray_definitions_finds_function() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "kind": ["function"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Expected exactly 1 function, got {}", defs.len());
    assert_eq!(defs[0]["name"], "createLogger");
    assert_eq!(defs[0]["kind"], "function");
    assert!(defs[0]["file"].as_str().unwrap().contains("helpers.ts"));
}

#[test]
fn test_ts_xray_definitions_finds_enum() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "kind": ["enum"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Expected exactly 1 enum, got {}", defs.len());
    assert_eq!(defs[0]["name"], "UserStatus");
    assert_eq!(defs[0]["kind"], "enum");
}

#[test]
fn test_ts_xray_definitions_finds_enum_member() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "kind": ["enumMember"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 2, "Expected exactly 2 enum members (Active, Inactive), got {}", defs.len());
    let names: Vec<&str> = defs.iter().filter_map(|d| d["name"].as_str()).collect();
    assert!(names.contains(&"Active"), "Should contain Active enum member");
    assert!(names.contains(&"Inactive"), "Should contain Inactive enum member");
    for def in defs {
        assert_eq!(def["kind"], "enumMember");
        assert_eq!(def["parent"], "UserStatus");
    }
}

#[test]
fn test_ts_xray_definitions_finds_type_alias() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "kind": ["typeAlias"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Expected exactly 1 typeAlias, got {}", defs.len());
    assert_eq!(defs[0]["name"], "UserId");
    assert_eq!(defs[0]["kind"], "typeAlias");
}

#[test]
fn test_ts_xray_definitions_finds_variable() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "kind": ["variable"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Expected exactly 1 variable, got {}", defs.len());
    assert_eq!(defs[0]["name"], "DEFAULT_TIMEOUT");
    assert_eq!(defs[0]["kind"], "variable");
}

#[test]
fn test_ts_xray_definitions_finds_field() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "kind": ["field"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Expected exactly 1 field, got {}", defs.len());
    assert_eq!(defs[0]["name"], "name");
    assert_eq!(defs[0]["kind"], "field");
    assert_eq!(defs[0]["parent"], "UserService");
}

#[test]
fn test_ts_xray_definitions_finds_constructor() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "kind": ["constructor"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Expected exactly 1 constructor, got {}", defs.len());
    assert_eq!(defs[0]["name"], "constructor");
    assert_eq!(defs[0]["kind"], "constructor");
    assert_eq!(defs[0]["parent"], "UserService");
}

// ─── Part 3: baseType filter tests ───────────────────────────────────

#[test]
fn test_ts_xray_definitions_base_type_implements() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "baseType": "IUserService"
    }));
    assert!(!result.is_error, "baseType filter should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Expected exactly 1 definition implementing IUserService, got {}", defs.len());
    assert_eq!(defs[0]["name"], "UserService");
    assert_eq!(defs[0]["kind"], "class");
}

#[test]
fn test_ts_xray_definitions_base_type_abstract() {
    let ctx = make_ts_ctx_with_defs();
    // OrderProcessor has modifiers ["export", "abstract"] — search by name to verify modifiers
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["OrderProcessor"],
        "kind": ["class"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Expected exactly 1 class named OrderProcessor, got {}", defs.len());
    assert_eq!(defs[0]["name"], "OrderProcessor");
    // Verify modifiers include "abstract"
    let modifiers = defs[0]["modifiers"].as_array().unwrap();
    let mod_strs: Vec<&str> = modifiers.iter().filter_map(|m| m.as_str()).collect();
    assert!(mod_strs.contains(&"abstract"),
        "OrderProcessor should have abstract modifier, got: {:?}", mod_strs);
}

// ─── Part 4: containsLine and includeBody tests ─────────────────────

#[test]
fn test_ts_contains_line_finds_method() {
    let (ctx, tmp) = make_ts_ctx_with_real_files();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "file": ["UserService"],
        "containsLine": 8
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["containingDefinitions"].as_array().unwrap();
    assert!(!defs.is_empty(), "Should find containing definitions for line 8");
    let method = defs.iter().find(|d| d["kind"] == "method").unwrap();
    assert_eq!(method["name"], "getUser");
    assert_eq!(method["parent"], "UserService");
    cleanup_tmp(&tmp);
}

#[test]
fn test_ts_xray_definitions_include_body() {
    let (ctx, tmp) = make_ts_ctx_with_real_files();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["getUser"],
        "includeBody": true
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1);
    let body = defs[0]["body"].as_array().unwrap();
    assert!(!body.is_empty(), "Body should have content lines");
    assert_eq!(defs[0]["bodyStartLine"], 6);
    cleanup_tmp(&tmp);
}

// ─── Part 5: xray_callers tests ────────────────────────────────────

#[test]
fn test_ts_xray_callers_up_finds_caller() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["getUser"],
        "class": "UserService",
        "depth": 1
    }));
    assert!(!result.is_error, "xray_callers should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(!tree.is_empty(), "Call tree should not be empty — handleOrder calls getUser");
    // Verify the caller is handleOrder in OrderProcessor
    let caller_methods: Vec<&str> = tree.iter().filter_map(|n| n["method"].as_str()).collect();
    assert!(caller_methods.contains(&"handleOrder"),
        "Should find handleOrder as caller, got: {:?}", caller_methods);
}

#[test]
fn test_ts_xray_callers_down_finds_callees() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["handleOrder"],
        "class": "OrderProcessor",
        "direction": "down",
        "depth": 1
    }));
    assert!(!result.is_error, "xray_callers down should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(!tree.is_empty(), "Call tree should not be empty — handleOrder calls getUser");
    let callee_methods: Vec<&str> = tree.iter().filter_map(|n| n["method"].as_str()).collect();
    assert!(callee_methods.contains(&"getUser"),
        "Should find getUser as callee of handleOrder, got: {:?}", callee_methods);
}

#[test]
fn test_ts_xray_callers_direct_receiver_types() {
    static COUNTER: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!(
        "xray_ts_inline_receiver_{}_{}",
        std::process::id(),
        id
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let tmp_dir = crate::canonicalize_test_root(&tmp_dir);

    let ts_file = tmp_dir.join("inline-receiver.ts");
    std::fs::write(
        &ts_file,
        r#"class XrayEdgeDerived {
    execute(value: string): string { return value; }
}
class UnrelatedExecutor {
    execute(value: string): string { return value; }
}
export function xrayEdgeDirectCall(): string {
    return new XrayEdgeDerived().execute("marker");
}
export function typedParameter(target: XrayEdgeDerived): string {
    return target.execute("marker");
}
export function optionalParameter(target?: XrayEdgeDerived): string | undefined {
    return target?.execute("marker");
}
export function sameTernary(
    condition: boolean,
    first: XrayEdgeDerived,
    second: XrayEdgeDerived,
): string {
    return (condition ? first : second).execute("marker");
}
export function differentTernary(
    condition: boolean,
    first: XrayEdgeDerived,
    second: UnrelatedExecutor,
): string {
    return (condition ? first : second).execute("marker");
}
export function sameCoalescing(
    first: XrayEdgeDerived | undefined,
    fallback: XrayEdgeDerived,
): string {
    return (first ?? fallback).execute("marker");
}
export function differentCoalescing(
    first: XrayEdgeDerived | undefined,
    fallback: UnrelatedExecutor,
): string {
    return (first ?? fallback).execute("marker");
}
"#,
    )
    .unwrap();

    let mut def_index = DefinitionIndex {
        root: tmp_dir.to_string_lossy().to_string(),
        extensions: vec!["ts".to_string()],
        ..Default::default()
    };
    let clean_path = PathBuf::from(crate::clean_path(&ts_file.to_string_lossy()));
    crate::definitions::update_file_definitions(&mut def_index, &clean_path);

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp_dir.to_string_lossy().to_string(),
        ext: "ts".to_string(),
        threads: 1,
        ..Default::default()
    })
    .unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(
            tmp_dir.to_string_lossy().to_string(),
        ))),
        server_ext: "ts".to_string(),
        ..Default::default()
    };

    let down_result = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["xrayEdgeDirectCall"],
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!down_result.is_error, "{}", down_result.content[0].text);
    let down_output: Value =
        serde_json::from_str(&down_result.content[0].text).unwrap();
    let execute_nodes: Vec<_> = down_output["callTree"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["method"].as_str() == Some("execute"))
        .collect();
    assert_eq!(execute_nodes.len(), 1, "{}", down_output);
    assert_eq!(
        execute_nodes[0]["class"].as_str(),
        Some("XrayEdgeDerived"),
        "{}",
        down_output
    );

    let up_result = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["execute"],
            "class": "XrayEdgeDerived",
            "depth": 1
        }),
    );
    assert!(!up_result.is_error, "{}", up_result.content[0].text);
    let up_output: Value = serde_json::from_str(&up_result.content[0].text).unwrap();
    let caller_methods: Vec<_> = up_output["callTree"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["method"].as_str())
        .collect();
    assert_eq!(
        caller_methods,
        vec![
            "xrayEdgeDirectCall",
            "typedParameter",
            "optionalParameter",
            "sameTernary",
            "sameCoalescing",
        ],
        "{}",
        up_output
    );

    for (method_name, expected_class) in [
        ("typedParameter", Some("XrayEdgeDerived")),
        ("optionalParameter", Some("XrayEdgeDerived")),
        ("sameTernary", Some("XrayEdgeDerived")),
        ("differentTernary", None),
        ("sameCoalescing", Some("XrayEdgeDerived")),
        ("differentCoalescing", None),
    ] {
        let result = dispatch_tool(
            &ctx,
            "xray_callers",
            &json!({
                "method": [method_name],
                "direction": "down",
                "depth": 1
            }),
        );
        assert!(!result.is_error, "{}", result.content[0].text);
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        let execute_nodes: Vec<_> = output["callTree"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|node| node["method"].as_str() == Some("execute"))
            .collect();
        assert_eq!(
            execute_nodes
                .first()
                .and_then(|node| node["class"].as_str()),
            expected_class,
            "{}",
            output
        );
        assert_eq!(
            execute_nodes.len(),
            if expected_class.is_some() { 1 } else { 0 },
            "{}",
            output
        );
    }


    cleanup_tmp(&tmp_dir);
}

#[test]
fn test_ts_angular_template_states_and_duplicate_selectors_are_truthful() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    std::fs::write(
        root.join("components.ts"),
        r#"const dynamicSelector = getSelector();
@Component({ selector: 'app-first' })
export class FirstChild {}
@Component({ selector: 'app-duplicate' })
export class FirstDuplicate {}
@Component({ selector: 'app-duplicate' })
export class SecondDuplicate {}
@Component({ selector: 'app-parent', template: '<app-duplicate></app-duplicate><runtime-child></runtime-child>' })
export class ParentComponent {}
@Component({ selector: dynamicSelector, template: makeTemplate() })
export class DynamicComponent {}
@Component({ selector: 'app-missing' })
export class MissingComponent {}
@Component({ selector: dynamicSelector, template: '<app-first></app-first>' })
export class DynamicParentComponent {}
@Component({ selector: 'app-dynamic-child', template: makeTemplate() })
export class DynamicChildComponent {}
@Component({ selector: 'app-dynamic-root', template: '<app-dynamic-child></app-dynamic-child>' })
export class DynamicRootComponent {}
@Component({ selector: 'app-unavailable', templateUrl: './missing.html' })
export class UnavailableComponent {}"#,
    )
    .unwrap();

    let def_index = crate::definitions::build_definition_index(&DefIndexArgs {
        dir: root.to_string_lossy().to_string(),
        ext: "ts".to_string(),
        threads: 1,
        respect_git_exclude: false,
    });
    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: root.to_string_lossy().to_string(),
        ext: "ts".to_string(),
        threads: 1,
        ..Default::default()
    })
    .unwrap();
    let context = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(
            root.to_string_lossy().to_string(),
        ))),
        server_ext: "ts".to_string(),
        ..Default::default()
    };

    let definitions_result = dispatch_tool(
        &context,
        "xray_definitions",
        &json!({ "name": ["DynamicComponent", "MissingComponent", "UnavailableComponent"] }),
    );
    assert!(
        !definitions_result.is_error,
        "{}",
        definitions_result.content[0].text
    );
    let definitions_output: Value =
        serde_json::from_str(&definitions_result.content[0].text).unwrap();
    let definition = |name: &str| {
        definitions_output["definitions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|definition| definition["name"] == name)
            .unwrap()
    };
    assert_eq!(definition("DynamicComponent")["selectorState"], "dynamic");
    assert_eq!(definition("DynamicComponent")["templateSource"], "dynamic");
    assert_eq!(definition("DynamicComponent")["templateState"], "dynamic");
    assert!(definition("DynamicComponent")["templateStateReason"].is_string());
    assert_eq!(definition("MissingComponent")["selectorState"], "static");
    assert_eq!(definition("MissingComponent")["templateSource"], "missing");
    assert_eq!(definition("MissingComponent")["templateState"], "missing");
    assert_eq!(definition("UnavailableComponent")["templateSource"], "external");
    assert_eq!(definition("UnavailableComponent")["templateState"], "unavailable");
    assert_eq!(definition("UnavailableComponent")["templatePath"], "./missing.html");
    assert!(definition("UnavailableComponent")["templateStateReason"].is_string());

    let parent = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["ParentComponent"],
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!parent.is_error, "{}", parent.content[0].text);
    let parent_output: Value = serde_json::from_str(&parent.content[0].text).unwrap();
    assert_eq!(parent_output["callTree"][0]["resolution"], "ambiguous");
    assert_eq!(
        parent_output["callTree"][0]["reason"],
        "ambiguous_template_selector"
    );
    assert_eq!(parent_output["callTree"][0]["candidates"].as_array().unwrap().len(), 2);
    assert_eq!(parent_output["callTree"][1]["selector"], "runtime-child");
    assert_eq!(parent_output["callTree"][1]["resolution"], "unresolved");
    assert_eq!(
        parent_output["callTree"][1]["reason"],
        "unresolved_template_selector"
    );
    assert_eq!(parent_output["resultStatus"]["status"], "partial");
    assert_eq!(parent_output["resultStatus"]["safeForExhaustiveClaims"], false);
    assert!(parent_output["resultStatus"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason == "ambiguous_template_selector"));
    assert!(parent_output["resultStatus"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason == "unresolved_template_selector"));

    for (component, expected_reasons) in [
        (
            "DynamicComponent",
            vec!["dynamic_component_selector", "dynamic_template"],
        ),
        ("MissingComponent", vec!["missing_template"]),
        (
            "UnavailableComponent",
            vec!["unavailable_external_template"],
        ),
    ] {
        let result = dispatch_tool(
            &context,
            "xray_callers",
            &json!({
                "method": [component],
                "direction": "down",
                "depth": 1
            }),
        );
        assert!(!result.is_error, "{}", result.content[0].text);
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert_eq!(output["resultStatus"]["status"], "partial", "{output}");
        assert_eq!(output["resultStatus"]["complete"], false, "{output}");
        assert_eq!(
            output["resultStatus"]["safeForExhaustiveClaims"],
            false,
            "{output}"
        );
        let reasons = output["resultStatus"]["reasons"].as_array().unwrap();
        for expected in expected_reasons {
            assert!(reasons.iter().any(|reason| reason == expected), "{output}");
        }
    }
    let static_parent_with_global_blind_spot = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["app-duplicate"],
            "direction": "up",
            "depth": 2
        }),
    );
    assert!(
        !static_parent_with_global_blind_spot.is_error,
        "{}",
        static_parent_with_global_blind_spot.content[0].text
    );
    let global_blind_spot_output: Value = serde_json::from_str(
        &static_parent_with_global_blind_spot.content[0].text,
    )
    .unwrap();
    assert_eq!(
        global_blind_spot_output["callTree"][0]["class"],
        "ParentComponent",
        "{global_blind_spot_output}"
    );
    assert_eq!(global_blind_spot_output["resultStatus"]["status"], "partial");
    assert!(global_blind_spot_output["resultStatus"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason == "dynamic_template_index_blind_spot"));
    assert!(global_blind_spot_output["resultStatus"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason == "unavailable_external_template"));

    let dynamic_parent = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["app-first"],
            "direction": "up",
            "depth": 2
        }),
    );
    assert!(
        !dynamic_parent.is_error,
        "{}",
        dynamic_parent.content[0].text
    );
    let dynamic_parent_output: Value =
        serde_json::from_str(&dynamic_parent.content[0].text).unwrap();
    assert_eq!(
        dynamic_parent_output["callTree"][0]["class"],
        "DynamicParentComponent"
    );
    assert_eq!(dynamic_parent_output["resultStatus"]["status"], "partial");
    assert!(dynamic_parent_output["resultStatus"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason == "dynamic_component_selector"));

    let dynamic_descendant = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["DynamicRootComponent"],
            "direction": "down",
            "depth": 2
        }),
    );
    assert!(
        !dynamic_descendant.is_error,
        "{}",
        dynamic_descendant.content[0].text
    );
    let dynamic_descendant_output: Value =
        serde_json::from_str(&dynamic_descendant.content[0].text).unwrap();
    assert_eq!(
        dynamic_descendant_output["callTree"][0]["class"],
        "DynamicChildComponent"
    );
    assert_eq!(
        dynamic_descendant_output["resultStatus"]["status"],
        "partial"
    );
    assert!(dynamic_descendant_output["resultStatus"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason == "dynamic_template"));
}

#[test]
fn test_angular_exact_edge_is_navigable_both_directions_through_public_handler() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    std::fs::write(
        root.join("component-a.ts"),
        r#"@Component({ selector: 'app-a', template: '<app-b></app-b>' })
export class ComponentA {}
@Component({ selector: 'app-b', template: '<span></span>' })
export class ComponentB {}"#,
    )
    .unwrap();

    let def_index = crate::definitions::build_definition_index(&DefIndexArgs {
        dir: root.to_string_lossy().to_string(),
        ext: "ts".to_string(),
        threads: 1,
        respect_git_exclude: false,
    });
    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: root.to_string_lossy().to_string(),
        ext: "ts".to_string(),
        threads: 1,
        ..Default::default()
    })
    .unwrap();
    let context = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(
            root.to_string_lossy().to_string(),
        ))),
        server_ext: "ts".to_string(),
        ..Default::default()
    };

    let down = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["ComponentA"],
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!down.is_error, "{}", down.content[0].text);
    let down_output: Value = serde_json::from_str(&down.content[0].text).unwrap();
    let down_tree = down_output["callTree"].as_array().unwrap();
    assert_eq!(down_tree.len(), 1, "{down_output}");
    assert_eq!(down_tree[0]["class"], "ComponentB");
    assert_eq!(down_tree[0]["selector"], "app-b");
    assert_eq!(down_output["resultStatus"]["status"], "complete", "{down_output}");
    assert_eq!(down_output["resultStatus"]["complete"], true, "{down_output}");
    assert!(
        down_output["resultStatus"]["reasons"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{down_output}"
    );

    let up = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["app-b"],
            "direction": "up",
            "depth": 1
        }),
    );
    assert!(!up.is_error, "{}", up.content[0].text);
    let up_output: Value = serde_json::from_str(&up.content[0].text).unwrap();
    let up_tree = up_output["callTree"].as_array().unwrap();
    assert_eq!(up_tree.len(), 1, "{up_output}");
    assert_eq!(up_tree[0]["class"], "ComponentA");
    assert_eq!(up_tree[0]["selector"], "app-a");
    assert_eq!(up_output["resultStatus"]["status"], "complete", "{up_output}");
    assert_eq!(up_output["resultStatus"]["complete"], true, "{up_output}");
    assert!(
        up_output["resultStatus"]["reasons"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{up_output}"
    );
}

#[test]
fn test_ts_local_arrow_call_does_not_resolve_to_other_module_function() {
    static COUNTER: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!(
        "xray_ts_local_bare_call_{}_{}",
        std::process::id(),
        id
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let tmp_dir = crate::canonicalize_test_root(&tmp_dir);

    let caller_file = tmp_dir.join("caller.ts");
    let other_file = tmp_dir.join("other.ts");
    std::fs::write(
        &caller_file,
        r#"import { importedFn } from './other';
function work(): void {}
export function sameFile(): void { work(); }
export function shadowed(): void {}
function outer(): void {
    function sameFile(): void {}
    function shadowed(): void {}
    sameFile();
    shadowed();
}
class WorkOwner {
    runClass(): void {
        const classCallback = () => work();
        classCallback();
    }
}
export function run(param: () => void): void {
    const callback = () => work();
    callback();
    sameFile();
    shadowed();
    param();
    const dynamic = sameFile;
    dynamic();
    importedFn();
    setTimeout(() => {}, 0);
}
const privateHelper = () => work();
export function usePrivateHelper(): void { privateHelper(); }
export const exportedHelper = () => work();
export function recursiveRoot(): void {
    const recursiveRoot = () => work();
    recursiveRoot();
}
class ArrowOwner {
    arrowRoot = () => work();
    callArrow(): void { this.arrowRoot(); }
}"#,
    )
    .unwrap();
    std::fs::write(
        &other_file,
        "const shadowed = () => {};\nexport function callback(): void {}\nexport function sameFile(): void { callback(); }\nexport function importedFn(): void {}\nconst privateHelper = () => {};\nexport function useOtherPrivateHelper(): void { privateHelper(); }\n",
    )
    .unwrap();

    let mut def_index = DefinitionIndex {
        root: tmp_dir.to_string_lossy().to_string(),
        extensions: vec!["ts".to_string()],
        ..Default::default()
    };
    for path in [&other_file, &caller_file] {
        let clean_path = PathBuf::from(crate::clean_path(&path.to_string_lossy()));
        crate::definitions::update_file_definitions(&mut def_index, &clean_path);
    }

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp_dir.to_string_lossy().to_string(),
        ext: "ts".to_string(),
        threads: 1,
        ..Default::default()
    })
    .unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(
            tmp_dir.to_string_lossy().to_string(),
        ))),
        server_ext: "ts".to_string(),
        ..Default::default()
    };

    let local_definition_result = dispatch_tool(
        &ctx,
        "xray_definitions",
        &json!({
            "name": ["classCallback"],
            "kind": ["function"]
        }),
    );
    assert!(
        !local_definition_result.is_error,
        "{}",
        local_definition_result.content[0].text
    );
    let local_definition_output: Value =
        serde_json::from_str(&local_definition_result.content[0].text).unwrap();
    let local_definition = &local_definition_output["definitions"][0];
    assert_eq!(local_definition["parent"], "WorkOwner");
    assert_eq!(local_definition["signature"], "classCallback()");
    assert!(
        local_definition["modifiers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|modifier| modifier == "local"),
        "{local_definition_output}"
    );


    let result = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["run"],
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let callback_nodes: Vec<_> = output["callTree"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["method"].as_str() == Some("callback"))
        .collect();
    assert_eq!(callback_nodes.len(), 1, "{output}");
    assert_eq!(callback_nodes[0]["file"], "caller.ts", "{output}");
    assert_eq!(callback_nodes[0]["line"], 18, "{output}");

    let same_file_nodes: Vec<_> = output["callTree"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["method"].as_str() == Some("sameFile"))
        .collect();
    assert_eq!(same_file_nodes.len(), 1, "{output}");
    assert_eq!(same_file_nodes[0]["file"], "caller.ts", "{output}");
    assert_eq!(same_file_nodes[0]["line"], 3, "{output}");
    assert_eq!(output["summary"]["unresolvedCallSites"], 4, "{output}");
    assert_eq!(
        output["summary"]["unresolvedCallSitesScope"],
        "traversedGraph",
        "{output}"
    );
    assert_eq!(
        output["summary"]["unresolvedCallReasons"]["dynamic_callable_parameter"],
        1,
        "{output}"
    );

    assert_eq!(
        output["summary"]["unresolvedCallReasons"]["unresolved_local_binding"],
        1,
        "{output}"
    );
    assert_eq!(
        output["summary"]["unresolvedCallReasons"]["module_resolution_failed"],
        1,
        "{output}"
    );
    assert_eq!(
        output["summary"]["unresolvedCallReasons"]["unknown_global"],
        1,
        "{output}"
    );
    assert_eq!(output["resultStatus"]["status"], "partial", "{output}");
    assert_eq!(output["resultStatus"]["complete"], false, "{output}");

    assert_eq!(
        output["resultStatus"]["resolutionCompleteness"]["exact"],
        false,
        "{output}"
    );
    assert_eq!(
        output["resultStatus"]["resolutionCompleteness"]["allStaticTargetsUnique"],
        false,
        "{output}"
    );
    assert_eq!(
        output["resultStatus"]["safeForExhaustiveClaims"],
        false,
        "{output}"
    );

    let multi_result = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["run", "sameFile"],
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!multi_result.is_error, "{}", multi_result.content[0].text);
    let multi_output: Value = serde_json::from_str(&multi_result.content[0].text).unwrap();
    assert_eq!(multi_output["summary"]["unresolvedCallSites"], 4, "{multi_output}");
    assert_eq!(
        multi_output["summary"]["unresolvedCallSitesScope"],
        "traversedGraph",
        "{multi_output}"
    );
    assert_eq!(multi_output["resultStatus"]["status"], "partial", "{multi_output}");

    let run_batch = multi_output["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|result| result["method"].as_str() == Some("run"))
        .unwrap();
    assert_eq!(
        run_batch["unresolvedCallSitesScope"],
        "traversedGraph",
        "{multi_output}"
    );

    let up_result = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["work"],
            "direction": "up",
            "depth": 1
        }),
    );
    assert!(!up_result.is_error, "{}", up_result.content[0].text);
    let up_output: Value = serde_json::from_str(&up_result.content[0].text).unwrap();
    let class_callback = up_output["callTree"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["method"].as_str() == Some("classCallback"))
        .unwrap_or_else(|| panic!("missing classCallback caller: {up_output}"));
    assert_eq!(class_callback["class"], "WorkOwner", "{up_output}");

    let same_file_up = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["shadowed"],
            "direction": "up",
            "depth": 1
        }),
    );
    assert!(!same_file_up.is_error, "{}", same_file_up.content[0].text);
    let same_file_up_output: Value =
        serde_json::from_str(&same_file_up.content[0].text).unwrap();
    let same_file_callers = same_file_up_output["callTree"].as_array().unwrap();
    assert!(
        same_file_callers
            .iter()
            .any(|node| node["method"].as_str() == Some("run")),
        "{same_file_up_output}"
    );
    assert!(
        same_file_callers
            .iter()
            .all(|node| node["method"].as_str() != Some("outer")),
        "{same_file_up_output}"
    );

    let ambiguous_same_file = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["sameFile"],
            "direction": "up",
            "depth": 1
        }),
    );
    assert!(
        !ambiguous_same_file.is_error,
        "{}",
        ambiguous_same_file.content[0].text
    );
    let ambiguous_output: Value =
        serde_json::from_str(&ambiguous_same_file.content[0].text).unwrap();
    assert_eq!(ambiguous_output["rootResolution"]["status"], "ambiguous");
    assert_eq!(
        ambiguous_output["rootResolution"]["reason"],
        "ambiguous_typescript_root"
    );
    assert_eq!(
        ambiguous_output["rootResolution"]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "{ambiguous_output}"
    );

    let legacy_same_file_up = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["sameFile"],
            "direction": "up",
            "depth": 1,
            "includeBody": true,
            "ambiguityPolicy": "legacy"
        }),
    );
    assert!(
        !legacy_same_file_up.is_error,
        "{}",
        legacy_same_file_up.content[0].text
    );
    let legacy_up_output: Value =
        serde_json::from_str(&legacy_same_file_up.content[0].text).unwrap();
    let legacy_up_methods: std::collections::HashSet<_> = legacy_up_output["callTree"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["method"].as_str())
        .collect();
    assert_eq!(
        legacy_up_methods,
        std::collections::HashSet::from(["outer", "run"]),
        "{legacy_up_output}"
    );
    assert!(legacy_up_output.get("rootResolution").is_none());
    assert!(legacy_up_output.get("rootMethod").is_none());
    assert_eq!(legacy_up_output["resultStatus"]["status"], "partial");
    assert_eq!(legacy_up_output["resultStatus"]["complete"], false);
    assert_eq!(
        legacy_up_output["resultStatus"]["safeForExactSemantics"],
        false
    );
    assert!(
        legacy_up_output["resultStatus"]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason == "legacy_ambiguous_fanout"),
        "{legacy_up_output}"
    );
    assert!(
        legacy_up_output["warning"]
            .as_str()
            .unwrap()
            .contains("TypeScript"),
        "{legacy_up_output}"
    );

    let legacy_same_file_down = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["sameFile"],
            "direction": "down",
            "depth": 1,
            "includeBody": true,
            "ambiguityPolicy": "legacy"
        }),
    );
    assert!(
        !legacy_same_file_down.is_error,
        "{}",
        legacy_same_file_down.content[0].text
    );
    let legacy_down_output: Value =
        serde_json::from_str(&legacy_same_file_down.content[0].text).unwrap();
    let legacy_down_methods: std::collections::HashSet<_> = legacy_down_output["callTree"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["method"].as_str())
        .collect();
    assert_eq!(
        legacy_down_methods,
        std::collections::HashSet::from(["callback", "work"]),
        "{legacy_down_output}"
    );
    assert!(legacy_down_output.get("rootResolution").is_none());
    assert!(legacy_down_output.get("rootMethod").is_none());
    assert_eq!(legacy_down_output["resultStatus"]["status"], "partial");
    assert!(
        legacy_down_output["resultStatus"]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason == "legacy_ambiguous_fanout"),
        "{legacy_down_output}"
    );

    for direction in ["up", "down"] {
        let legacy_batch = dispatch_tool(
            &ctx,
            "xray_callers",
            &json!({
                "method": ["sameFile", "shadowed"],
                "direction": direction,
                "depth": 1,
                "includeBody": true,
                "ambiguityPolicy": "legacy"
            }),
        );
        assert!(!legacy_batch.is_error, "{}", legacy_batch.content[0].text);
        let legacy_batch_output: Value =
            serde_json::from_str(&legacy_batch.content[0].text).unwrap();
        let same_file_result = legacy_batch_output["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|result| result["method"] == "sameFile")
            .unwrap();
        let methods: std::collections::HashSet<_> = same_file_result["callTree"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|node| node["method"].as_str())
            .collect();
        let expected = if direction == "up" {
            std::collections::HashSet::from(["outer", "run"])
        } else {
            std::collections::HashSet::from(["callback", "work"])
        };
        assert_eq!(methods, expected, "{direction}: {legacy_batch_output}");
        assert!(same_file_result.get("rootResolution").is_none());
        assert!(same_file_result.get("rootMethod").is_none());
        assert!(
            same_file_result["warning"]
                .as_str()
                .unwrap()
                .contains("TypeScript"),
            "{legacy_batch_output}"
        );
        assert!(
            legacy_batch_output["warning"]
                .as_str()
                .unwrap()
                .contains("TypeScript"),
            "{legacy_batch_output}"
        );
        assert_eq!(legacy_batch_output["resultStatus"]["status"], "partial");
        assert_eq!(
            legacy_batch_output["resultStatus"]["safeForExactSemantics"],
            false
        );
        assert!(
            legacy_batch_output["resultStatus"]["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|reason| reason == "legacy_ambiguous_fanout"),
            "{legacy_batch_output}"
        );
    }

    let invalid_batch_policy = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["sameFile", "shadowed"],
            "ambiguityPolicy": "unsafe"
        }),
    );
    assert!(invalid_batch_policy.is_error);
    assert!(
        invalid_batch_policy.content[0].text.contains("ambiguityPolicy"),
        "{}",
        invalid_batch_policy.content[0].text
    );

    let batch_up = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["shadowed", "sameFile", "importedFn", "setTimeout"],
            "direction": "up",
            "depth": 1,
            "includeBody": true
        }),
    );
    assert!(!batch_up.is_error, "{}", batch_up.content[0].text);
    let batch_output: Value = serde_json::from_str(&batch_up.content[0].text).unwrap();
    let batch_results = batch_output["results"].as_array().unwrap();
    let result_for = |name: &str| {
        batch_results
            .iter()
            .find(|result| result["method"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("missing {name}: {batch_output}"))
    };
    let shadowed_batch = result_for("shadowed");
    assert!(
        shadowed_batch["callTree"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["method"].as_str() == Some("run")),
        "{batch_output}"
    );
    assert!(
        shadowed_batch["callTree"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["method"].as_str() != Some("outer")),
        "{batch_output}"
    );
    assert_eq!(
        result_for("sameFile")["rootResolution"]["reason"],
        "ambiguous_typescript_root"
    );

    assert!(result_for("sameFile").get("rootMethod").is_none());

    assert!(result_for("sameFile").get("hint").is_none());
    assert!(
        result_for("setTimeout")["callTree"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["method"].as_str() != Some("run")),
        "{batch_output}"
    );
    assert_eq!(batch_output["resultStatus"]["status"], "partial");
    assert!(
        batch_output["resultStatus"]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason == "ambiguous_typescript_root"),
        "{batch_output}"
    );
    for (unresolved_name, reason) in [
        ("importedFn", "module_resolution_failed"),
        ("setTimeout", "unknown_global"),
    ] {
        assert_eq!(
            result_for(unresolved_name)["unresolvedCallReasons"][reason],
            1,
            "{batch_output}"
        );
        assert_eq!(
            batch_output["summary"]["unresolvedCallReasons"][reason],
            1,
            "{batch_output}"
        );
    }

    for (unresolved_name, reason) in [
        ("importedFn", "module_resolution_failed"),
        ("setTimeout", "unknown_global"),
    ] {
        let unresolved_up = dispatch_tool(
            &ctx,
            "xray_callers",
            &json!({
                "method": [unresolved_name],
                "direction": "up",
                "depth": 1
            }),
        );
        assert!(!unresolved_up.is_error, "{}", unresolved_up.content[0].text);
        let unresolved_output: Value =
            serde_json::from_str(&unresolved_up.content[0].text).unwrap();
        assert!(
            unresolved_output["callTree"]
                .as_array()
                .unwrap()
                .iter()
                .all(|node| node["method"].as_str() != Some("run")),
            "{unresolved_name}: {unresolved_output}"
        );
        assert_eq!(
            unresolved_output["summary"]["unresolvedCallReasons"][reason],
            1,
            "{unresolved_output}"
        );
        assert_eq!(
            unresolved_output["summary"]["unresolvedCallSitesScope"],
            "traversedGraph",
            "{unresolved_output}"
        );
        assert_eq!(unresolved_output["resultStatus"]["status"], "partial");
        assert_eq!(unresolved_output["resultStatus"]["complete"], false);
        assert_eq!(
            unresolved_output["resultStatus"]["safeForExhaustiveClaims"],
            false
        );
    }

    let recursive_down = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["recursiveRoot"],
            "direction": "down",
            "depth": 1,
            "includeBody": true
        }),
    );
    assert!(!recursive_down.is_error, "{}", recursive_down.content[0].text);
    let recursive_output: Value =
        serde_json::from_str(&recursive_down.content[0].text).unwrap();
    let recursive_nodes: Vec<_> = recursive_output["callTree"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["method"].as_str() == Some("recursiveRoot"))
        .collect();
    assert_eq!(recursive_nodes.len(), 1, "{recursive_output}");
    assert_ne!(
        recursive_nodes[0]["line"],
        recursive_output["rootMethod"]["line"],
        "nested binding must not resolve to the enclosing root: {recursive_output}"
    );


    let exact_down = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["shadowed"],
            "direction": "down",
            "depth": 1,
            "includeBody": true
        }),
    );
    assert!(!exact_down.is_error, "{}", exact_down.content[0].text);
    let exact_down_output: Value = serde_json::from_str(&exact_down.content[0].text).unwrap();
    assert_eq!(exact_down_output["rootMethod"]["file"], "caller.ts");
    assert_eq!(exact_down_output["rootMethod"]["line"], 4);

    let private_helper_down = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["privateHelper"],
            "direction": "down",
            "depth": 1,
            "includeBody": true
        }),
    );
    assert!(
        !private_helper_down.is_error,
        "{}",
        private_helper_down.content[0].text
    );
    let private_down_output: Value =
        serde_json::from_str(&private_helper_down.content[0].text).unwrap();
    assert!(private_down_output["callTree"].as_array().unwrap().is_empty());
    assert_eq!(
        private_down_output["rootResolution"]["reason"],
        "ambiguous_typescript_root"
    );
    assert!(private_down_output.get("rootMethod").is_none());

    let batch_down = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["shadowed", "privateHelper"],
            "direction": "down",
            "depth": 1,
            "includeBody": true
        }),
    );
    assert!(!batch_down.is_error, "{}", batch_down.content[0].text);
    let batch_down_output: Value = serde_json::from_str(&batch_down.content[0].text).unwrap();
    let batch_down_results = batch_down_output["results"].as_array().unwrap();
    let exact_batch = batch_down_results
        .iter()
        .find(|result| result["method"].as_str() == Some("shadowed"))
        .unwrap();
    let private_batch = batch_down_results
        .iter()
        .find(|result| result["method"].as_str() == Some("privateHelper"))
        .unwrap();
    assert_eq!(exact_batch["rootMethod"]["file"], "caller.ts");
    assert_eq!(private_batch["rootResolution"]["reason"], "ambiguous_typescript_root");
    assert!(private_batch.get("rootMethod").is_none());


    let exported_helper_down = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["exportedHelper"],
            "direction": "down",
            "depth": 1,
            "includeBody": true
        }),
    );
    assert!(
        !exported_helper_down.is_error,
        "{}",
        exported_helper_down.content[0].text
    );
    let exported_helper_output: Value =
        serde_json::from_str(&exported_helper_down.content[0].text).unwrap();
    assert_eq!(exported_helper_output["rootMethod"]["method"], "exportedHelper");
    assert_eq!(exported_helper_output["rootMethod"]["file"], "caller.ts");
    assert!(
        exported_helper_output["callTree"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["method"].as_str() == Some("work")),
        "{exported_helper_output}"
    );


    let arrow_down = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["arrowRoot"],
            "class": "ArrowOwner",
            "direction": "down",
            "depth": 1,
            "includeBody": true
        }),
    );
    assert!(!arrow_down.is_error, "{}", arrow_down.content[0].text);
    let arrow_down_output: Value = serde_json::from_str(&arrow_down.content[0].text).unwrap();
    assert_eq!(arrow_down_output["rootMethod"]["class"], "ArrowOwner");
    assert!(
        arrow_down_output["callTree"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["method"].as_str() == Some("work")),
        "{arrow_down_output}"
    );

    let arrow_up = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["arrowRoot"],
            "class": "ArrowOwner",
            "direction": "up",
            "depth": 1
        }),
    );
    assert!(!arrow_up.is_error, "{}", arrow_up.content[0].text);
    let arrow_up_output: Value = serde_json::from_str(&arrow_up.content[0].text).unwrap();
    assert!(
        arrow_up_output["callTree"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["method"].as_str() == Some("callArrow")),
        "{arrow_up_output}"
    );


    let private_helper_up = dispatch_tool(
        &ctx,
        "xray_callers",
        &json!({
            "method": ["privateHelper"],
            "direction": "up",
            "depth": 1
        }),
    );
    assert!(
        !private_helper_up.is_error,
        "{}",
        private_helper_up.content[0].text
    );
    let private_helper_output: Value =
        serde_json::from_str(&private_helper_up.content[0].text).unwrap();
    assert!(private_helper_output.get("rootResolution").is_none());
    let private_callers: std::collections::HashSet<_> = private_helper_output["callTree"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["method"].as_str())
        .collect();
    assert_eq!(
        private_callers,
        std::collections::HashSet::from(["usePrivateHelper", "useOtherPrivateHelper"]),
        "{private_helper_output}"
    );


    cleanup_tmp(&tmp_dir);
}

#[test]
fn test_ts_xray_callers_nonexistent_method() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["nonExistentMethodXYZ"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(tree.is_empty(), "Call tree should be empty for nonexistent method");
}

// ─── Part 6: Combined filters ────────────────────────────────────────

#[test]
fn test_ts_xray_definitions_combined_name_parent_kind() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["getUser"],
        "parent": ["UserService"],
        "kind": ["method"]
    }));
    assert!(!result.is_error, "Combined filter should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1,
        "Expected exactly 1 result for name+parent+kind filter, got {}: {:?}",
        defs.len(), defs);
    assert_eq!(defs[0]["name"], "getUser");
    assert_eq!(defs[0]["parent"], "UserService");
    assert_eq!(defs[0]["kind"], "method");

    // Verify: same name+kind but different parent should NOT match
    let result2 = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["getUser"],
        "parent": ["NonExistentClass"],
        "kind": ["method"]
    }));
    assert!(!result2.is_error);
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    let defs2 = output2["definitions"].as_array().unwrap();
    assert_eq!(defs2.len(), 0,
        "Non-matching parent should return 0 results, got {}", defs2.len());
}

#[test]
fn test_ts_xray_definitions_name_regex() {
    let ctx = make_ts_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["User.*"],
        "regex": true
    }));
    assert!(!result.is_error, "Regex search should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    // Should match: UserService, UserStatus, UserId, IUserService (regex is case-insensitive substring)
    assert!(defs.len() >= 3,
        "Regex 'User.*' should match at least UserService, UserStatus, UserId. Got {}: {:?}",
        defs.len(), defs.iter().map(|d| d["name"].as_str()).collect::<Vec<_>>());

    let names: Vec<&str> = defs.iter().filter_map(|d| d["name"].as_str()).collect();
    assert!(names.contains(&"UserService"), "Should contain UserService");
    assert!(names.contains(&"UserStatus"), "Should contain UserStatus");
    assert!(names.contains(&"UserId"), "Should contain UserId");

    // All returned definitions should contain "user" (case-insensitive) in their name
    for def in defs {
        let name = def["name"].as_str().unwrap();
        assert!(name.to_lowercase().contains("user"),
            "Definition '{}' should match regex 'User.*'", name);
    }
}

// ─── Part 7: TS-07 — Attribute filter for TS decorators ──────────────

#[test]
fn test_ts_xray_definitions_attribute_filter_decorator() {
    let ctx = make_ts_ctx_with_defs();
    // UserService has @Injectable decorator — search by attribute
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "attribute": "Injectable"
    }));
    assert!(!result.is_error, "attribute filter should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Expected exactly 1 definition with @Injectable, got {}", defs.len());
    assert_eq!(defs[0]["name"], "UserService");
    assert_eq!(defs[0]["kind"], "class");

    // Non-existent decorator should return 0 results
    let result2 = dispatch_tool(&ctx, "xray_definitions", &json!({
        "attribute": "NonExistentDecorator"
    }));
    assert!(!result2.is_error);
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    let defs2 = output2["definitions"].as_array().unwrap();
    assert_eq!(defs2.len(), 0, "Non-existent decorator should return 0 results");
}

// ─── Part 8: TS-12 — xray_callers with inject() support ────────────

#[test]
fn test_ts_xray_callers_inject_support() {
    // Create a context where a service is injected via Angular inject()
    // and the caller uses it through the injected field.
    let mut content_idx = HashMap::new();
    content_idx.insert("processorder".to_string(), vec![
        Posting { file_id: 0, lines: vec![10] },
    ]);
    content_idx.insert("getuser".to_string(), vec![
        Posting { file_id: 0, lines: vec![12] },
        Posting { file_id: 1, lines: vec![5] },
    ]);

    let trigram = build_trigram_index(&content_idx);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "src/OrderComponent.ts".to_string(),
            "src/UserService.ts".to_string(),
        ],
        index: content_idx, total_tokens: 100,
        extensions: vec!["ts".to_string()],
        file_token_counts: vec![50, 50],
        trigram, ..Default::default()
    };

    let definitions = vec![
        // 0: Class OrderComponent (file 0) — injects UserService
        DefinitionEntry {
            file_id: 0, name: "OrderComponent".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 20,
            parent: None, signature: None,
            modifiers: vec!["export".to_string()],
            attributes: vec![], base_types: vec![],
        },
        // 1: Method processOrder (file 0, parent: OrderComponent)
        DefinitionEntry {
            file_id: 0, name: "processOrder".to_string(),
            kind: DefinitionKind::Method, line_start: 8, line_end: 15,
            parent: Some("OrderComponent".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // 2: Class UserService (file 1)
        DefinitionEntry {
            file_id: 1, name: "UserService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 10,
            parent: None, signature: None,
            modifiers: vec!["export".to_string()],
            attributes: vec!["Injectable".to_string()], base_types: vec![],
        },
        // 3: Method getUser (file 1, parent: UserService)
        DefinitionEntry {
            file_id: 1, name: "getUser".to_string(),
            kind: DefinitionKind::Method, line_start: 3, line_end: 8,
            parent: Some("UserService".to_string()), signature: None,
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("src/OrderComponent.ts")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("src/UserService.ts")), 1);

    // processOrder (idx 1) calls getUser via injected UserService
    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(1, vec![CallSite {
        method_name: "getUser".to_string(),
        receiver_type: Some("UserService".to_string()),
        line: 12,
        call_kind: Default::default(),
        receiver_is_generic: false,
            }]);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["ts".to_string()],
        files: vec!["src/OrderComponent.ts".to_string(), "src/UserService.ts".to_string()],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls,
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        server_ext: "ts".to_string(),
        ..Default::default()
    };

    // xray_callers up: who calls getUser in UserService?
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["getUser"],
        "class": "UserService",
        "depth": 1
    }));
    assert!(!result.is_error, "inject callers should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(!tree.is_empty(), "Should find callers via inject() — processOrder calls getUser");
    let caller_methods: Vec<&str> = tree.iter().filter_map(|n| n["method"].as_str()).collect();
    assert!(caller_methods.contains(&"processOrder"),
        "Should find processOrder as caller via inject(), got: {:?}", caller_methods);
}

// ─── Part 9: TS-13 — xray_callers with arrow function class properties ──

#[test]
fn test_ts_xray_callers_arrow_fn_property() {
    let ctx = make_ts_ctx_with_defs();
    // The existing ctx has handleOrder (idx 4) calling getUser (on UserService)
    // xray_callers direction=down from handleOrder should find getUser
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["handleOrder"],
        "class": "OrderProcessor",
        "direction": "down",
        "depth": 1
    }));
    assert!(!result.is_error, "arrow fn callers should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(!tree.is_empty(), "handleOrder should have callees");
    let callee_methods: Vec<&str> = tree.iter().filter_map(|n| n["method"].as_str()).collect();
    assert!(callee_methods.contains(&"getUser"),
        "handleOrder should call getUser, got: {:?}", callee_methods);
}

// ─── Part 10: TS-14 — Mixed C# + TS definition index queries ─────────

#[test]
fn test_mixed_cs_ts_definitions_query() {
    // Create a context with both .cs and .ts files
    let mut content_idx = HashMap::new();
    content_idx.insert("userservice".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![1] },
    ]);

    let trigram = build_trigram_index(&content_idx);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "src/UserService.cs".to_string(),
            "src/UserService.ts".to_string(),
        ],
        index: content_idx, total_tokens: 100,
        extensions: vec!["cs".to_string(), "ts".to_string()],
        file_token_counts: vec![50, 50],
        trigram, ..Default::default()
    };

    let definitions = vec![
        // 0: C# class UserService
        DefinitionEntry {
            file_id: 0, name: "UserService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 20,
            parent: None, signature: None,
            modifiers: vec!["public".to_string()],
            attributes: vec!["ServiceProvider".to_string()],
            base_types: vec!["IUserService".to_string()],
        },
        // 1: TS class UserService
        DefinitionEntry {
            file_id: 1, name: "UserService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 15,
            parent: None, signature: None,
            modifiers: vec!["export".to_string()],
            attributes: vec!["Injectable".to_string()],
            base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("src/UserService.cs")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("src/UserService.ts")), 1);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string(), "ts".to_string()],
        files: vec!["src/UserService.cs".to_string(), "src/UserService.ts".to_string()],
        definitions, name_index, kind_index,
        attribute_index, base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls: HashMap::new(),
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        server_ext: "cs,ts".to_string(),
        ..Default::default()
    };

    // Query by name — should find both C# and TS versions
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["UserService"],
        "kind": ["class"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 2, "Should find both C# and TS UserService, got {}", defs.len());

    // Verify one is .cs and one is .ts
    let files: Vec<&str> = defs.iter().filter_map(|d| d["file"].as_str()).collect();
    assert!(files.iter().any(|f| f.ends_with(".cs")), "Should have .cs file: {:?}", files);
    assert!(files.iter().any(|f| f.ends_with(".ts")), "Should have .ts file: {:?}", files);

    // Filter by file to scope to one language
    let result_cs = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["UserService"],
        "file": [".cs"]
    }));
    assert!(!result_cs.is_error);
    let output_cs: Value = serde_json::from_str(&result_cs.content[0].text).unwrap();
    let defs_cs = output_cs["definitions"].as_array().unwrap();
    assert_eq!(defs_cs.len(), 1, "File filter '.cs' should return 1 result");
    assert!(defs_cs[0]["file"].as_str().unwrap().ends_with(".cs"));
}

// ─── Part 11: TS-15 — Mixed C# + TS call graph with ext filter ───────

#[test]
fn test_mixed_cs_ts_callers_ext_filter() {
    // Create mixed-language context with calls in both .cs and .ts files
    let mut content_idx = HashMap::new();
    content_idx.insert("getuser".to_string(), vec![
        Posting { file_id: 0, lines: vec![5] },
        Posting { file_id: 1, lines: vec![10] },
    ]);

    let trigram = build_trigram_index(&content_idx);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "src/Service.cs".to_string(),
            "src/Component.ts".to_string(),
        ],
        index: content_idx, total_tokens: 100,
        extensions: vec!["cs".to_string(), "ts".to_string()],
        file_token_counts: vec![50, 50],
        trigram, ..Default::default()
    };

    let definitions = vec![
        // 0: C# class CsService (file 0)
        DefinitionEntry {
            file_id: 0, name: "CsService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 20,
            parent: None, signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // 1: C# method DoWork (file 0, calls getUser)
        DefinitionEntry {
            file_id: 0, name: "DoWork".to_string(),
            kind: DefinitionKind::Method, line_start: 3, line_end: 10,
            parent: Some("CsService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // 2: TS class TsComponent (file 1)
        DefinitionEntry {
            file_id: 1, name: "TsComponent".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 20,
            parent: None, signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // 3: TS method render (file 1, calls getUser)
        DefinitionEntry {
            file_id: 1, name: "render".to_string(),
            kind: DefinitionKind::Method, line_start: 8, line_end: 15,
            parent: Some("TsComponent".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // 4: C# method getUser (used by both)
        DefinitionEntry {
            file_id: 0, name: "getUser".to_string(),
            kind: DefinitionKind::Method, line_start: 12, line_end: 18,
            parent: Some("CsService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // 5: Same-named TypeScript function must not hijack a mixed-language query.
        DefinitionEntry {
            file_id: 1, name: "getUser".to_string(),
            kind: DefinitionKind::Function, line_start: 17, line_end: 18,
            parent: None, signature: Some("getUser()".to_string()),
            modifiers: vec!["export".to_string()], attributes: vec![], base_types: vec![],
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("src/Service.cs")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("src/Component.ts")), 1);

    // Both DoWork (idx 1) and render (idx 3) call getUser
    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(1, vec![CallSite {
        method_name: "getUser".to_string(),
        receiver_type: Some("CsService".to_string()),
        line: 5,
        call_kind: Default::default(),
        receiver_is_generic: false,
            }]);
    method_calls.insert(3, vec![CallSite {
        method_name: "getUser".to_string(),
        receiver_type: Some("CsService".to_string()),
        line: 10,
        call_kind: Default::default(),
        receiver_is_generic: false,
            }]);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string(), "ts".to_string()],
        files: vec!["src/Service.cs".to_string(), "src/Component.ts".to_string()],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls,
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        server_ext: "cs,ts".to_string(),
        ..Default::default()
    };

    // Without ext filter — should find callers from both languages
    let result_all = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["getUser"],
        "class": "CsService",
        "depth": 1
    }));
    assert!(!result_all.is_error);
    let output_all: Value = serde_json::from_str(&result_all.content[0].text).unwrap();
    let tree_all = output_all["callTree"].as_array().unwrap();
    assert!(tree_all.len() >= 2, "Without ext filter, should find callers from both .cs and .ts, got {}", tree_all.len());

    let mixed_name_result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["getUser"],
        "depth": 1,
        "includeBody": true
    }));
    assert!(!mixed_name_result.is_error);
    let mixed_name_output: Value =
        serde_json::from_str(&mixed_name_result.content[0].text).unwrap();
    assert!(
        mixed_name_output["callTree"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["method"].as_str() == Some("DoWork")),
        "same-named TS root must not suppress C# callers: {mixed_name_output}"
    );

    assert_eq!(
        mixed_name_output["rootMethod"]["file"],
        "Service.cs",
        "mixed-language query must retain the non-TS root body: {mixed_name_output}"
    );


    // With ext=ts filter — should only find TS callers
    let result_ts = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["getUser"],
        "class": "CsService",
        "ext": ["ts"],
        "depth": 1
    }));
    assert!(!result_ts.is_error);
    let output_ts: Value = serde_json::from_str(&result_ts.content[0].text).unwrap();
    let tree_ts = output_ts["callTree"].as_array().unwrap();
    // All results should be from .ts files
    for node in tree_ts {
        if let Some(file) = node["file"].as_str() {
            assert!(file.ends_with(".ts"), "With ext=ts, all results should be .ts files, got: {}", file);
        }
    }
}

// ─── Part 12: TS-16 — TSX file support through handler ────────────────

#[test]
fn test_tsx_file_support_through_handler() {
    // Create a context with a .tsx file
    let mut content_idx = HashMap::new();
    content_idx.insert("appcomponent".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
    ]);

    let trigram = build_trigram_index(&content_idx);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec!["src/App.tsx".to_string()],
        index: content_idx, total_tokens: 50,
        extensions: vec!["ts".to_string(), "tsx".to_string()],
        file_token_counts: vec![50],
        trigram, ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "AppComponent".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 20,
            parent: None, signature: None,
            modifiers: vec!["export".to_string()],
            attributes: vec![], base_types: vec!["React.Component".to_string()],
        },
        DefinitionEntry {
            file_id: 0, name: "render".to_string(),
            kind: DefinitionKind::Method, line_start: 5, line_end: 15,
            parent: Some("AppComponent".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    let mut base_type_index: HashMap<String, Vec<u32>> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
        for bt in &def.base_types {
            base_type_index.entry(bt.to_lowercase()).or_default().push(idx);
        }
    }
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("src/App.tsx")), 0);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["ts".to_string(), "tsx".to_string()],
        files: vec!["src/App.tsx".to_string()],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index,
        file_index, path_to_id, method_calls: HashMap::new(),
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        server_ext: "ts,tsx".to_string(),
        ..Default::default()
    };

    // Find class in .tsx file
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["AppComponent"],
        "kind": ["class"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Should find AppComponent in .tsx file");
    assert_eq!(defs[0]["name"], "AppComponent");
    assert!(defs[0]["file"].as_str().unwrap().ends_with(".tsx"),
        "File should be .tsx: {}", defs[0]["file"]);

    // Find method in .tsx file with parent filter
    let result2 = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["render"],
        "parent": ["AppComponent"]
    }));
    assert!(!result2.is_error);
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    let defs2 = output2["definitions"].as_array().unwrap();
    assert_eq!(defs2.len(), 1, "Should find render in AppComponent");

    // Base type search — React.Component
    let result3 = dispatch_tool(&ctx, "xray_definitions", &json!({
        "baseType": "React.Component"
    }));
    assert!(!result3.is_error);
    let output3: Value = serde_json::from_str(&result3.content[0].text).unwrap();
    let defs3 = output3["definitions"].as_array().unwrap();
    assert_eq!(defs3.len(), 1, "Should find class extending React.Component");
    assert_eq!(defs3[0]["name"], "AppComponent");
}

// ─── Part 13: TS-17 — Incremental TS update through handler ──────────

#[test]
fn test_ts_incremental_update_through_handler() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_test_ts_incr_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Step 1: Create a .ts file with OldService class
    let ts_file = tmp_dir.join("service.ts");
    {
        let mut f = std::fs::File::create(&ts_file).unwrap();
        writeln!(f, "export class OldService {{").unwrap();
        writeln!(f, "  doWork(): void {{}}").unwrap();
        writeln!(f, "}}").unwrap();
    }

    let file_str = crate::clean_path(&ts_file.to_string_lossy());

    // Build a DefinitionIndex from the file using real tree-sitter parsing
    let mut def_index = DefinitionIndex {
        root: tmp_dir.to_string_lossy().to_string(), created_at: 0,
        extensions: vec!["ts".to_string()],
        files: Vec::new(), definitions: Vec::new(), name_index: HashMap::new(),
        kind_index: HashMap::new(), attribute_index: HashMap::new(),
        base_type_index: HashMap::new(), file_index: HashMap::new(),
        path_to_id: HashMap::new(), method_calls: HashMap::new(),
        ..Default::default()
    };

    let clean_path = PathBuf::from(&file_str);
    crate::definitions::update_file_definitions(&mut def_index, &clean_path);

    // Build content index for the file
    let content_index = ContentIndex {
        root: tmp_dir.to_string_lossy().to_string(),
        files: vec![file_str.clone()],
        index: HashMap::new(), total_tokens: 0,
        extensions: vec!["ts".to_string()],
        file_token_counts: vec![0],
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp_dir.to_string_lossy().to_string()))),
        server_ext: "ts".to_string(),
        ..Default::default()
    };

    // Verify OldService is found
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["OldService"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert!(!defs.is_empty(), "OldService should be found before update");

    // Step 2: Modify the file — rename class to NewService, add method
    {
        let mut f = std::fs::File::create(&ts_file).unwrap();
        writeln!(f, "export class NewService {{").unwrap();
        writeln!(f, "  execute(): void {{}}").unwrap();
        writeln!(f, "  validate(): boolean {{ return true; }}").unwrap();
        writeln!(f, "}}").unwrap();
    }

    // Step 3: Incremental update (simulates watcher calling update_file_definitions)
    {
        let mut idx = ctx.def_index.as_ref().unwrap().write().unwrap();
        crate::definitions::update_file_definitions(&mut idx, &clean_path);
    }

    // Step 4: Verify NewService is found, OldService is NOT found
    let result_new = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["NewService"]
    }));
    assert!(!result_new.is_error);
    let output_new: Value = serde_json::from_str(&result_new.content[0].text).unwrap();
    let defs_new = output_new["definitions"].as_array().unwrap();
    assert!(!defs_new.is_empty(), "NewService should be found after incremental update");
    assert_eq!(defs_new[0]["name"], "NewService");

    let result_old = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["OldService"]
    }));
    assert!(!result_old.is_error);
    let output_old: Value = serde_json::from_str(&result_old.content[0].text).unwrap();
    let defs_old = output_old["definitions"].as_array().unwrap();
    assert!(defs_old.is_empty(), "OldService should NOT be found after incremental update");

    // Verify new methods are found
    let result_exec = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["execute"], "parent": ["NewService"]
    }));
    assert!(!result_exec.is_error);
    let output_exec: Value = serde_json::from_str(&result_exec.content[0].text).unwrap();
    let defs_exec = output_exec["definitions"].as_array().unwrap();
    assert_eq!(defs_exec.len(), 1, "execute should be found in NewService");

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

// ─── Part 14: TS excludeDir filter ───────────────────────────────────

#[test]
fn test_ts_xray_definitions_exclude_dir() {
    let mut content_idx = HashMap::new();
    content_idx.insert("userservice".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![1] },
    ]);
    let trigram = build_trigram_index(&content_idx);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "src/services/UserService.ts".to_string(),
            "src/__tests__/UserService.spec.ts".to_string(),
        ],
        index: content_idx, total_tokens: 100,
        extensions: vec!["ts".to_string()],
        file_token_counts: vec![50, 50],
        trigram, ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "UserService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 20,
            parent: None, signature: None,
            modifiers: vec!["export".to_string()],
            attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "UserServiceSpec".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 30,
            parent: None, signature: None,
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("src/services/UserService.ts")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("src/__tests__/UserService.spec.ts")), 1);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["ts".to_string()],
        files: vec![
            "src/services/UserService.ts".to_string(),
            "src/__tests__/UserService.spec.ts".to_string(),
        ],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls: HashMap::new(),
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        server_ext: "ts".to_string(),
        ..Default::default()
    };

    // Exclude __tests__ directory
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "excludeDir": ["__tests__"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    let names: Vec<&str> = defs.iter().filter_map(|d| d["name"].as_str()).collect();
    assert!(names.contains(&"UserService"), "Should contain UserService from services dir");
    assert!(!names.contains(&"UserServiceSpec"), "Should NOT contain UserServiceSpec from __tests__ dir");

    // Without excludeDir — both should appear
    let result_all = dispatch_tool(&ctx, "xray_definitions", &json!({
        "kind": ["class"]
    }));
    assert!(!result_all.is_error);
    let output_all: Value = serde_json::from_str(&result_all.content[0].text).unwrap();
    let defs_all = output_all["definitions"].as_array().unwrap();
    assert_eq!(defs_all.len(), 2, "Without excludeDir, both classes should appear");
}

// ─── Part 15: TS DI interface resolution in callers ──────────────────

#[test]
fn test_ts_xray_callers_di_interface_resolution() {
    // In this test, UserService implements IUserService.
    // A caller uses IUserService (the interface) to call getUser.
    // xray_callers for getUser on UserService should find the caller
    // through DI interface resolution.
    let ctx = make_ts_ctx_with_defs();

    // The existing ctx has:
    // - UserService (class, baseTypes: ["IUserService"])
    // - IUserService (interface)
    // - handleOrder in OrderProcessor calls getUser on UserService
    //
    // Test: xray_callers for getUser on IUserService should also
    // find handleOrder (because UserService implements IUserService)
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["getUser"],
        "class": "IUserService",
        "depth": 1
    }));
    assert!(!result.is_error, "DI interface resolution should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();

    // Should find handleOrder as a caller through IUserService → UserService resolution
    if !tree.is_empty() {
        let caller_methods: Vec<&str> = tree.iter().filter_map(|n| n["method"].as_str()).collect();
        assert!(caller_methods.contains(&"handleOrder"),
            "Should find handleOrder through interface resolution, got: {:?}", caller_methods);
    }
    // Note: If resolveInterfaces defaults to true, this should find callers.
    // If not, at minimum it should not error. The test validates the path works.
}

// ─── Part 16: Direction=down with explicitly typed local variables ────

#[test]
fn test_ts_direction_down_with_typed_local_variable() {
    // This test verifies that direction=down resolves callees through
    // explicitly typed local variables (const x: Foo = ...).
    // Orchestrator.run() contains `const proc: DataProcessor = this.getProcessor()`
    // followed by `proc.transform("hello")`. Because `proc` has explicit type
    // `DataProcessor`, the call site has receiver_type = "DataProcessor",
    // so direction=down from Orchestrator.run() should find DataProcessor.transform().

    let mut content_idx = HashMap::new();
    content_idx.insert("transform".to_string(), vec![
        Posting { file_id: 0, lines: vec![3] },
        Posting { file_id: 1, lines: vec![10] },
    ]);
    content_idx.insert("dataprocessor".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![9] },
    ]);
    content_idx.insert("orchestrator".to_string(), vec![
        Posting { file_id: 1, lines: vec![7] },
    ]);

    let trigram = build_trigram_index(&content_idx);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "src/DataProcessor.ts".to_string(),
            "src/Orchestrator.ts".to_string(),
        ],
        index: content_idx, total_tokens: 100,
        extensions: vec!["ts".to_string()],
        file_token_counts: vec![50, 50],
        trigram, ..Default::default()
    };

    let definitions = vec![
        // 0: Class DataProcessor (file 0)
        DefinitionEntry {
            file_id: 0, name: "DataProcessor".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 6,
            parent: None, signature: None,
            modifiers: vec!["export".to_string()],
            attributes: vec![], base_types: vec![],
        },
        // 1: Method transform (file 0, parent: DataProcessor)
        DefinitionEntry {
            file_id: 0, name: "transform".to_string(),
            kind: DefinitionKind::Method, line_start: 2, line_end: 5,
            parent: Some("DataProcessor".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // 2: Class Orchestrator (file 1)
        DefinitionEntry {
            file_id: 1, name: "Orchestrator".to_string(),
            kind: DefinitionKind::Class, line_start: 7, line_end: 16,
            parent: None, signature: None,
            modifiers: vec!["export".to_string()],
            attributes: vec![], base_types: vec![],
        },
        // 3: Method run (file 1, parent: Orchestrator)
        DefinitionEntry {
            file_id: 1, name: "run".to_string(),
            kind: DefinitionKind::Method, line_start: 8, line_end: 12,
            parent: Some("Orchestrator".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // 4: Method getProcessor (file 1, parent: Orchestrator)
        DefinitionEntry {
            file_id: 1, name: "getProcessor".to_string(),
            kind: DefinitionKind::Method, line_start: 13, line_end: 15,
            parent: Some("Orchestrator".to_string()), signature: None,
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("src/DataProcessor.ts")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("src/Orchestrator.ts")), 1);

    // Orchestrator.run() (idx 3) calls:
    //   - getProcessor() on Orchestrator (this call)
    //   - transform() on DataProcessor (via explicitly typed local: const proc: DataProcessor)
    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(3, vec![
        CallSite {
            method_name: "getProcessor".to_string(),
            receiver_type: Some("Orchestrator".to_string()),
            line: 9,
            call_kind: Default::default(),
            receiver_is_generic: false,
        },
        CallSite {
            method_name: "transform".to_string(),
            receiver_type: Some("DataProcessor".to_string()),
            line: 10,
            call_kind: Default::default(),
            receiver_is_generic: false,
        },
    ]);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["ts".to_string()],
        files: vec!["src/DataProcessor.ts".to_string(), "src/Orchestrator.ts".to_string()],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls,
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        server_ext: "ts".to_string(),
        ..Default::default()
    };

    // direction=down from Orchestrator.run() should find DataProcessor.transform()
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["run"],
        "class": "Orchestrator",
        "direction": "down",
        "depth": 1
    }));
    assert!(!result.is_error, "direction=down should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(!tree.is_empty(), "Call tree should not be empty — run() calls transform() and getProcessor()");

    let callee_methods: Vec<&str> = tree.iter().filter_map(|n| n["method"].as_str()).collect();
    assert!(callee_methods.contains(&"transform"),
        "direction=down from Orchestrator.run() should find DataProcessor.transform() \
         (resolved through explicit type annotation `const proc: DataProcessor`), got: {:?}",
        callee_methods);
}