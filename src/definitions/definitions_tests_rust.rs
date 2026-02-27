//! Rust parser tests — definitions, call sites, code stats.

use super::*;
use super::parser_rust::parse_rust_definitions;

fn make_rust_parser() -> tree_sitter::Parser {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    parser
}

// ─── Definition extraction tests ────────────────────────────────────

#[test]
fn test_rust_parse_function() {
    let source = r#"
pub fn tokenize(line: &str, min_len: usize) -> Vec<String> {
    Vec::new()
}
"#;
    let mut parser = make_rust_parser();
    let (defs, _, _) = parse_rust_definitions(&mut parser, source, 0);

    let fn_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Function).collect();
    assert_eq!(fn_defs.len(), 1, "Expected 1 function, got {:?}", fn_defs);
    assert_eq!(fn_defs[0].name, "tokenize");
    assert!(fn_defs[0].parent.is_none(), "Top-level function should have no parent");
    assert!(fn_defs[0].signature.as_ref().unwrap().contains("tokenize"));
    assert!(fn_defs[0].modifiers.iter().any(|m| m.contains("pub")));
}

#[test]
fn test_rust_parse_struct() {
    let source = r#"
pub struct ContentIndex {
    pub root: String,
    pub files: Vec<String>,
    total_tokens: usize,
}
"#;
    let mut parser = make_rust_parser();
    let (defs, _, _) = parse_rust_definitions(&mut parser, source, 0);

    let struct_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Struct).collect();
    assert_eq!(struct_defs.len(), 1);
    assert_eq!(struct_defs[0].name, "ContentIndex");

    let field_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Field).collect();
    assert_eq!(field_defs.len(), 3, "Expected 3 fields, got {:?}", field_defs);
    assert!(field_defs.iter().any(|d| d.name == "root"));
    assert!(field_defs.iter().any(|d| d.name == "files"));
    assert!(field_defs.iter().any(|d| d.name == "total_tokens"));
    for f in &field_defs {
        assert_eq!(f.parent.as_deref(), Some("ContentIndex"));
    }
}

#[test]
fn test_rust_parse_enum() {
    let source = r#"
pub enum DefinitionKind {
    Class,
    Method,
    Function,
}
"#;
    let mut parser = make_rust_parser();
    let (defs, _, _) = parse_rust_definitions(&mut parser, source, 0);

    let enum_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Enum).collect();
    assert_eq!(enum_defs.len(), 1);
    assert_eq!(enum_defs[0].name, "DefinitionKind");

    let member_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::EnumMember).collect();
    assert_eq!(member_defs.len(), 3);
    let member_names: Vec<&str> = member_defs.iter().map(|d| d.name.as_str()).collect();
    assert!(member_names.contains(&"Class"));
    assert!(member_names.contains(&"Method"));
    assert!(member_names.contains(&"Function"));
    for m in &member_defs {
        assert_eq!(m.parent.as_deref(), Some("DefinitionKind"));
    }
}

#[test]
fn test_rust_parse_impl_method() {
    let source = r#"
struct OrderService {
    repo: String,
}

impl OrderService {
    pub fn process(&self, id: u32) -> bool {
        true
    }

    fn validate(&mut self) {
    }
}
"#;
    let mut parser = make_rust_parser();
    let (defs, _, _) = parse_rust_definitions(&mut parser, source, 0);

    let method_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Method).collect();
    assert_eq!(method_defs.len(), 2, "Expected 2 methods, got {:?}", method_defs);
    assert!(method_defs.iter().any(|d| d.name == "process"));
    assert!(method_defs.iter().any(|d| d.name == "validate"));
    for m in &method_defs {
        assert_eq!(m.parent.as_deref(), Some("OrderService"),
            "Method '{}' should have parent OrderService", m.name);
    }
}

#[test]
fn test_rust_parse_trait() {
    let source = r#"
pub trait Parser {
    fn parse(&self, source: &str) -> Vec<String>;
    fn name(&self) -> &str;
}
"#;
    let mut parser = make_rust_parser();
    let (defs, _, _) = parse_rust_definitions(&mut parser, source, 0);

    let trait_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Interface).collect();
    assert_eq!(trait_defs.len(), 1);
    assert_eq!(trait_defs[0].name, "Parser");

    // Methods inside trait should have parent = "Parser"
    let method_defs: Vec<_> = defs.iter()
        .filter(|d| (d.kind == DefinitionKind::Method || d.kind == DefinitionKind::Function) && d.parent.as_deref() == Some("Parser"))
        .collect();
    assert!(method_defs.len() >= 2, "Expected at least 2 trait methods, got {:?}", method_defs);
}

#[test]
fn test_rust_parse_trait_impl_base_types() {
    let source = r#"
struct UserService;

impl Display for UserService {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        Ok(())
    }
}
"#;
    let mut parser = make_rust_parser();
    let (defs, _, _) = parse_rust_definitions(&mut parser, source, 0);

    // The fmt method in `impl Display for UserService` should have base_types = ["Display"]
    let fmt_def = defs.iter().find(|d| d.name == "fmt");
    assert!(fmt_def.is_some(), "Expected fmt method");
    let fmt = fmt_def.unwrap();
    assert_eq!(fmt.parent.as_deref(), Some("UserService"));
    assert!(fmt.base_types.contains(&"Display".to_string()),
        "Method in trait impl should have base_types containing the trait. Got: {:?}", fmt.base_types);
}

#[test]
fn test_rust_parse_const_static() {
    let source = r#"
pub const DEFAULT_MIN_TOKEN_LEN: usize = 2;
static MAX_THREADS: usize = 8;
"#;
    let mut parser = make_rust_parser();
    let (defs, _, _) = parse_rust_definitions(&mut parser, source, 0);

    let var_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Variable).collect();
    assert_eq!(var_defs.len(), 2, "Expected 2 variables (const + static), got {:?}", var_defs);
    assert!(var_defs.iter().any(|d| d.name == "DEFAULT_MIN_TOKEN_LEN"));
    assert!(var_defs.iter().any(|d| d.name == "MAX_THREADS"));
}

#[test]
fn test_rust_parse_type_alias() {
    let source = r#"
pub type Result<T> = std::result::Result<T, Error>;
"#;
    let mut parser = make_rust_parser();
    let (defs, _, _) = parse_rust_definitions(&mut parser, source, 0);

    let ta_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::TypeAlias).collect();
    assert_eq!(ta_defs.len(), 1);
    assert_eq!(ta_defs[0].name, "Result");
}

// ─── Call site extraction tests ─────────────────────────────────────

#[test]
fn test_rust_call_site_method_call() {
    let source = r#"
struct IndexService {
    searcher: Searcher,
}

impl IndexService {
    fn search(&self, query: &str) {
        self.searcher.find(query);
        self.validate();
    }
    fn validate(&self) {}
}
"#;
    let mut parser = make_rust_parser();
    let (defs, call_sites, _) = parse_rust_definitions(&mut parser, source, 0);

    let si = defs.iter().position(|d| d.name == "search").unwrap();
    let sc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == si).collect();
    assert!(!sc.is_empty(), "Expected call sites for 'search'");

    let names: Vec<&str> = sc[0].1.iter().map(|c| c.method_name.as_str()).collect();
    assert!(names.contains(&"find"), "Expected call to 'find', got {:?}", names);
    assert!(names.contains(&"validate"), "Expected call to 'validate', got {:?}", names);

    // self.searcher.find() → receiver should resolve to field type or "searcher"
    let find_call = sc[0].1.iter().find(|c| c.method_name == "find");
    assert!(find_call.is_some());
}

#[test]
fn test_rust_call_site_static_call() {
    let source = r#"
struct Factory;
impl Factory {
    fn create(&self) {
        let map = HashMap::new();
        let items = Vec::with_capacity(100);
    }
}
"#;
    let mut parser = make_rust_parser();
    let (defs, call_sites, _) = parse_rust_definitions(&mut parser, source, 0);

    let ci = defs.iter().position(|d| d.name == "create").unwrap();
    let cc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == ci).collect();
    assert!(!cc.is_empty(), "Expected call sites for 'create'");

    let names: Vec<&str> = cc[0].1.iter().map(|c| c.method_name.as_str()).collect();
    assert!(names.contains(&"new"), "Expected call to HashMap::new(), got {:?}", names);
    assert!(names.contains(&"with_capacity"), "Expected call to Vec::with_capacity(), got {:?}", names);

    // Check receiver types
    let new_call = cc[0].1.iter().find(|c| c.method_name == "new").unwrap();
    assert_eq!(new_call.receiver_type.as_deref(), Some("HashMap"),
        "HashMap::new() receiver should be HashMap");

    let wc_call = cc[0].1.iter().find(|c| c.method_name == "with_capacity").unwrap();
    assert_eq!(wc_call.receiver_type.as_deref(), Some("Vec"),
        "Vec::with_capacity() receiver should be Vec");
}

#[test]
fn test_rust_call_site_free_function() {
    let source = r#"
fn process() {
    let tokens = tokenize("hello world");
    validate(tokens);
}
fn tokenize(s: &str) -> Vec<String> { Vec::new() }
fn validate(t: Vec<String>) {}
"#;
    let mut parser = make_rust_parser();
    let (defs, call_sites, _) = parse_rust_definitions(&mut parser, source, 0);

    let pi = defs.iter().position(|d| d.name == "process").unwrap();
    let pc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == pi).collect();
    assert!(!pc.is_empty(), "Expected call sites for 'process'");

    let names: Vec<&str> = pc[0].1.iter().map(|c| c.method_name.as_str()).collect();
    assert!(names.contains(&"tokenize"), "Expected call to 'tokenize', got {:?}", names);
    assert!(names.contains(&"validate"), "Expected call to 'validate', got {:?}", names);

    // Free function calls should have no receiver
    for call in &pc[0].1 {
        if call.method_name == "tokenize" || call.method_name == "validate" {
            assert_eq!(call.receiver_type, None,
                "Free function call '{}' should have no receiver", call.method_name);
        }
    }
}

// ─── Code stats tests ──────────────────────────────────────────────

#[test]
fn test_rust_code_stats_basic() {
    let source = r#"
struct Service;
impl Service {
    fn process(&self, items: Vec<i32>) -> i32 {
        for item in &items {
            if *item > 0 {
                return *item;
            }
        }
        match items.len() {
            0 => -1,
            1 => items[0],
            _ => items[0] + items[1],
        }
    }
}
"#;
    let mut parser = make_rust_parser();
    let (defs, _, stats) = parse_rust_definitions(&mut parser, source, 0);

    let method = defs.iter().position(|d| d.name == "process").unwrap();
    let s = stats.iter().find(|(i, _)| *i == method).map(|(_, s)| s).unwrap();

    // base(1) + for(1) + if(1) + match(1) + 3 match_arms(3) = 7
    assert!(s.cyclomatic_complexity >= 5,
        "Expected CC >= 5, got {}", s.cyclomatic_complexity);
    assert!(s.max_nesting_depth >= 2,
        "Expected nesting >= 2 (for > if), got {}", s.max_nesting_depth);
    assert!(s.return_count >= 1,
        "Expected at least 1 return, got {}", s.return_count);
}

#[test]
fn test_rust_code_stats_question_mark() {
    let source = r#"
struct Reader;
impl Reader {
    fn read_data(&self) -> Result<String, Error> {
        let file = open_file()?;
        let content = read_content(&file)?;
        Ok(content)
    }
}
"#;
    let mut parser = make_rust_parser();
    let (defs, _, stats) = parse_rust_definitions(&mut parser, source, 0);

    let method = defs.iter().position(|d| d.name == "read_data").unwrap();
    let s = stats.iter().find(|(i, _)| *i == method).map(|(_, s)| s).unwrap();

    // Two ? operators should count as 2 return points
    assert!(s.return_count >= 2,
        "Expected return_count >= 2 (two ? operators), got {}", s.return_count);
}

#[test]
fn test_rust_code_stats_closures() {
    let source = r#"
fn process(items: Vec<i32>) -> Vec<i32> {
    let doubled = items.iter().map(|x| x * 2).collect();
    let filtered = items.iter().filter(|&x| *x > 0).collect();
    doubled
}
"#;
    let mut parser = make_rust_parser();
    let (defs, _, stats) = parse_rust_definitions(&mut parser, source, 0);

    let method = defs.iter().position(|d| d.name == "process").unwrap();
    let s = stats.iter().find(|(i, _)| *i == method).map(|(_, s)| s).unwrap();

    assert_eq!(s.lambda_count, 2,
        "Expected 2 closures, got {}", s.lambda_count);
}

#[test]
fn test_rust_constructor_detection() {
    let source = r#"
struct Config {
    name: String,
}

impl Config {
    pub fn new(name: String) -> Self {
        Config { name }
    }

    pub fn default() -> Self {
        Config { name: String::new() }
    }

    pub fn validate(&self) -> bool {
        true
    }
}
"#;
    let mut parser = make_rust_parser();
    let (defs, _, _) = parse_rust_definitions(&mut parser, source, 0);

    let ctor_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Constructor).collect();
    assert_eq!(ctor_defs.len(), 2, "Expected 2 constructors (new + default), got {:?}", ctor_defs);
    assert!(ctor_defs.iter().any(|d| d.name == "new"));
    assert!(ctor_defs.iter().any(|d| d.name == "default"));
    for c in &ctor_defs {
        assert_eq!(c.parent.as_deref(), Some("Config"));
    }

    let method_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Method).collect();
    assert!(method_defs.iter().any(|d| d.name == "validate"),
        "validate should be a Method, not Constructor");
}

#[test]
fn test_rust_modifiers_and_attributes() {
    let source = r#"
#[derive(Debug, Clone)]
#[serde(default)]
pub struct DataModel {
    pub name: String,
}

#[test]
fn test_something() {
    assert!(true);
}

pub async unsafe fn dangerous_async() {}
"#;
    let mut parser = make_rust_parser();
    let (defs, _, _) = parse_rust_definitions(&mut parser, source, 0);

    // Check struct attributes
    let struct_def = defs.iter().find(|d| d.name == "DataModel").unwrap();
    assert!(struct_def.attributes.iter().any(|a| a.contains("derive")),
        "DataModel should have derive attribute, got {:?}", struct_def.attributes);
    assert!(struct_def.attributes.iter().any(|a| a.contains("serde")),
        "DataModel should have serde attribute, got {:?}", struct_def.attributes);
    assert!(struct_def.modifiers.iter().any(|m| m.contains("pub")),
        "DataModel should have pub modifier");

    // Check test function attributes
    let test_fn = defs.iter().find(|d| d.name == "test_something").unwrap();
    assert!(test_fn.attributes.iter().any(|a| a.contains("test")),
        "test_something should have #[test] attribute, got {:?}", test_fn.attributes);

    // Check async unsafe function modifiers
    let dangerous_fn = defs.iter().find(|d| d.name == "dangerous_async").unwrap();
    let mod_strs: Vec<&str> = dangerous_fn.modifiers.iter().map(|s| s.as_str()).collect();
    assert!(mod_strs.iter().any(|m| m.contains("pub")),
        "dangerous_async should have pub modifier, got {:?}", mod_strs);
}

#[test]
fn test_rust_self_not_counted_as_param() {
    let source = r#"
struct Service;
impl Service {
    fn no_params(&self) {}
    fn one_param(&self, x: i32) {}
    fn two_params(&mut self, x: i32, y: String) {}
    fn static_one(x: i32) {}
}
"#;
    let mut parser = make_rust_parser();
    let (defs, _, stats) = parse_rust_definitions(&mut parser, source, 0);

    let check = |name: &str, expected: u8| {
        let idx = defs.iter().position(|d| d.name == name).unwrap();
        let s = stats.iter().find(|(i, _)| *i == idx).map(|(_, s)| s).unwrap();
        assert_eq!(s.param_count, expected,
            "Method '{}' should have {} params (excluding self), got {}", name, expected, s.param_count);
    };

    check("no_params", 0);
    check("one_param", 1);
    check("two_params", 2);
    check("static_one", 1);
}

// ─── Incremental update test ────────────────────────────────────────

#[test]
fn test_rust_incremental_update() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let test_file = dir.join("service.rs");
    std::fs::write(&test_file, r#"
pub struct OrderService;
impl OrderService {
    pub fn process(&self) {}
}
"#).unwrap();

    let mut index = DefinitionIndex {
        root: ".".to_string(),
        extensions: vec!["rs".to_string()],
        ..Default::default()
    };

    let clean = std::path::PathBuf::from(crate::clean_path(&test_file.to_string_lossy()));
    update_file_definitions(&mut index, &clean);

    assert!(!index.definitions.is_empty(), "Should have definitions after update");
    assert!(index.name_index.contains_key("orderservice"), "Should find OrderService");
    assert!(index.name_index.contains_key("process"), "Should find process");

    // Update: rename struct and method
    std::fs::write(&test_file, r#"
pub struct UserService;
impl UserService {
    pub fn get_user(&self, id: u32) -> String { String::new() }
}
"#).unwrap();

    update_file_definitions(&mut index, &clean);

    assert!(!index.name_index.contains_key("orderservice"), "Old name should be gone");
    assert!(!index.name_index.contains_key("process"), "Old method should be gone");
    assert!(index.name_index.contains_key("userservice"), "New name should exist");
    assert!(index.name_index.contains_key("get_user"), "New method should exist");
}