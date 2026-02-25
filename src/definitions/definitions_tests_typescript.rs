//! TypeScript parser tests — split from definitions_tests.rs.

use super::*;
use super::parser_typescript::parse_typescript_definitions;
use super::parser_csharp::parse_csharp_definitions;  // needed for test_ts_csharp_callers_still_work
use std::collections::HashMap;
use std::path::PathBuf;

// ─── TypeScript Parsing Tests ────────────────────────────────────────

#[test]
fn test_parse_ts_class() {
    let source = "export class UserService extends BaseService implements IUserService { }";
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let class_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Class).collect();
    assert_eq!(class_defs.len(), 1);
    assert_eq!(class_defs[0].name, "UserService");
    assert!(class_defs[0].base_types.iter().any(|b| b.contains("BaseService")));
    assert!(class_defs[0].base_types.iter().any(|b| b.contains("IUserService")));
    assert!(class_defs[0].modifiers.contains(&"export".to_string()));
}

#[test]
fn test_parse_ts_abstract_class() {
    let source = r#"abstract class AbstractHandler {
    abstract handle(): void;
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let class_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Class).collect();
    assert_eq!(class_defs.len(), 1);
    assert_eq!(class_defs[0].name, "AbstractHandler");
    assert!(class_defs[0].modifiers.contains(&"abstract".to_string()));

    let method_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Method).collect();
    assert!(method_defs.len() >= 1);
    assert_eq!(method_defs[0].name, "handle");
    assert!(method_defs[0].modifiers.contains(&"abstract".to_string()));
}

#[test]
fn test_parse_ts_interface() {
    let source = r#"export interface IOrderProcessor {
    process(order: Order): Promise<void>;
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let iface_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Interface).collect();
    assert_eq!(iface_defs.len(), 1);
    assert_eq!(iface_defs[0].name, "IOrderProcessor");
    assert!(iface_defs[0].modifiers.contains(&"export".to_string()));

    // Interface should have a property child for the method signature
    let prop_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Property).collect();
    assert!(prop_defs.len() >= 1);
}

#[test]
fn test_parse_ts_function() {
    let source = "export async function fetchUser(id: string): Promise<User> { return {} as User; }";
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let fn_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Function).collect();
    assert_eq!(fn_defs.len(), 1);
    assert_eq!(fn_defs[0].name, "fetchUser");
    assert!(fn_defs[0].modifiers.contains(&"export".to_string()));
    assert!(fn_defs[0].modifiers.contains(&"async".to_string()));
    assert!(fn_defs[0].signature.is_some());
    let sig = fn_defs[0].signature.as_ref().unwrap();
    assert!(sig.contains("id: string"));
}

#[test]
fn test_parse_ts_method() {
    let source = r#"class UserManager {
    public async getUser(id: string): Promise<User> { return {} as User; }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let method_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Method).collect();
    assert_eq!(method_defs.len(), 1);
    assert_eq!(method_defs[0].name, "getUser");
    assert!(method_defs[0].modifiers.contains(&"public".to_string()));
    assert!(method_defs[0].modifiers.contains(&"async".to_string()));
    assert_eq!(method_defs[0].parent, Some("UserManager".to_string()));
}

#[test]
fn test_parse_ts_constructor() {
    let source = r#"class OrderService {
    constructor(private userService: IUserService) { }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let ctor_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Constructor).collect();
    assert_eq!(ctor_defs.len(), 1);
    assert_eq!(ctor_defs[0].name, "constructor");
    assert_eq!(ctor_defs[0].parent, Some("OrderService".to_string()));
    assert!(ctor_defs[0].signature.is_some());
    let sig = ctor_defs[0].signature.as_ref().unwrap();
    assert!(sig.contains("userService"));
}

#[test]
fn test_parse_ts_enum() {
    let source = r#"export enum OrderStatus {
    Pending,
    Active,
    Completed
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let enum_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Enum).collect();
    assert_eq!(enum_defs.len(), 1);
    assert_eq!(enum_defs[0].name, "OrderStatus");

    let member_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::EnumMember).collect();
    assert_eq!(member_defs.len(), 3);
}

#[test]
fn test_parse_ts_const_enum() {
    let source = r#"const enum Foo {
    Alpha,
    Beta,
    Gamma
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let enum_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Enum).collect();
    assert_eq!(enum_defs.len(), 1);
    assert_eq!(enum_defs[0].name, "Foo");

    let member_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::EnumMember).collect();
    assert_eq!(member_defs.len(), 3);
    let member_names: Vec<&str> = member_defs.iter().map(|d| d.name.as_str()).collect();
    assert!(member_names.contains(&"Alpha"));
    assert!(member_names.contains(&"Beta"));
    assert!(member_names.contains(&"Gamma"));
    for m in &member_defs {
        assert_eq!(m.parent.as_deref(), Some("Foo"));
    }
}

#[test]
fn test_parse_ts_type_alias() {
    let source = "export type UserId = string | number;";
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let ta_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::TypeAlias).collect();
    assert_eq!(ta_defs.len(), 1);
    assert_eq!(ta_defs[0].name, "UserId");
    assert!(ta_defs[0].modifiers.contains(&"export".to_string()));
}

#[test]
fn test_parse_ts_variable() {
    let source = "export const MAX_RETRIES = 3;";
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let var_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Variable).collect();
    assert_eq!(var_defs.len(), 1);
    assert_eq!(var_defs[0].name, "MAX_RETRIES");
    assert!(var_defs[0].modifiers.contains(&"export".to_string()));
}

#[test]
fn test_parse_ts_decorators() {
    let source = r#"@Injectable()
@Component({selector: 'app'})
class AppComponent {}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let class_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Class).collect();
    assert_eq!(class_defs.len(), 1);
    assert_eq!(class_defs[0].name, "AppComponent");
    assert_eq!(class_defs[0].attributes.len(), 2);
    assert!(class_defs[0].attributes.iter().any(|a| a.contains("Injectable")));
    assert!(class_defs[0].attributes.iter().any(|a| a.contains("Component")));
}

#[test]
fn test_parse_ts_field() {
    let source = r#"class DataHolder {
    private readonly name: string = '';
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let field_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Field).collect();
    assert_eq!(field_defs.len(), 1);
    assert_eq!(field_defs[0].name, "name");
    assert!(field_defs[0].modifiers.contains(&"private".to_string()));
    assert!(field_defs[0].modifiers.contains(&"readonly".to_string()));
}

#[test]
fn test_parse_ts_interface_property() {
    let source = r#"interface IEntity {
    readonly id: string;
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let prop_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Property).collect();
    assert_eq!(prop_defs.len(), 1);
    assert_eq!(prop_defs[0].name, "id");
    assert!(prop_defs[0].modifiers.contains(&"readonly".to_string()));
}

#[test]
fn test_parse_tsx_file() {
    let source = r#"export class AppComponent {
    render() { return <div/>; }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TSX.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let class_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Class).collect();
    assert_eq!(class_defs.len(), 1);
    assert_eq!(class_defs[0].name, "AppComponent");

    let method_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Method).collect();
    assert_eq!(method_defs.len(), 1);
    assert_eq!(method_defs[0].name, "render");
}

#[test]
fn test_ts_incremental_update() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // Step 1: Create a .ts file and add it to the index
    let test_file = dir.join("service.ts");
    std::fs::write(&test_file, "export class OrderService { process(): void {} }").unwrap();

    let mut index = DefinitionIndex {
        root: ".".to_string(), created_at: 0, extensions: vec!["ts".to_string()],
        files: Vec::new(), definitions: Vec::new(), name_index: HashMap::new(),
        kind_index: HashMap::new(), attribute_index: HashMap::new(),
        base_type_index: HashMap::new(), file_index: HashMap::new(),
        path_to_id: HashMap::new(), method_calls: HashMap::new(), code_stats: HashMap::new(), parse_errors: 0, lossy_file_count: 0, empty_file_ids: Vec::new(), extension_methods: HashMap::new(), selector_index: HashMap::new(), template_children: HashMap::new(),
    };

    let clean = PathBuf::from(crate::clean_path(&test_file.to_string_lossy()));
    update_file_definitions(&mut index, &clean);

    assert!(!index.definitions.is_empty());
    assert!(index.name_index.contains_key("orderservice"));
    assert!(index.name_index.contains_key("process"));
    assert_eq!(index.files.len(), 1);

    // Step 2: Modify the .ts file — rename class, add a method
    std::fs::write(&test_file, r#"export class UpdatedService {
    execute(): void {}
    validate(): boolean { return true; }
}"#).unwrap();

    update_file_definitions(&mut index, &clean);

    assert!(!index.name_index.contains_key("orderservice"));
    assert!(!index.name_index.contains_key("process"));
    assert!(index.name_index.contains_key("updatedservice"));
    assert!(index.name_index.contains_key("execute"));
    assert!(index.name_index.contains_key("validate"));

    // Step 3: Remove the file (simulate deletion by writing empty)
    std::fs::write(&test_file, "").unwrap();
    update_file_definitions(&mut index, &clean);

    // All named definitions from that file should be gone from name index
    assert!(!index.name_index.contains_key("updatedservice"));
    assert!(!index.name_index.contains_key("execute"));
    assert!(!index.name_index.contains_key("validate"));
}


// ─── TypeScript Call-Site Extraction Tests ────────────────────────────

#[test]
fn test_ts_this_method_call() {
    let source = r#"class OrderService {
    process(): void {
        this.doSomething();
    }
    doSomething(): void {}
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let pi = defs.iter().position(|d| d.name == "process").unwrap();
    let pc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == pi).collect();
    assert!(!pc.is_empty(), "Expected call sites for 'process' method");
    let ds = pc[0].1.iter().find(|c| c.method_name == "doSomething");
    assert!(ds.is_some(), "Expected call to 'doSomething'");
    assert_eq!(ds.unwrap().receiver_type.as_deref(), Some("OrderService"));
}

#[test]
fn test_ts_this_field_method_call() {
    let source = r#"class OrderController {
    constructor(private userService: UserService) {}
    handle(): void {
        this.userService.getUser();
    }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let hi = defs.iter().position(|d| d.name == "handle").unwrap();
    let hc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == hi).collect();
    assert!(!hc.is_empty(), "Expected call sites for 'handle' method");
    let gu = hc[0].1.iter().find(|c| c.method_name == "getUser");
    assert!(gu.is_some(), "Expected call to 'getUser'");
    assert_eq!(gu.unwrap().receiver_type.as_deref(), Some("UserService"));
}

#[test]
fn test_ts_standalone_function_call() {
    let source = r#"function processOrder(): void {
    someHelper();
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let pi = defs.iter().position(|d| d.name == "processOrder").unwrap();
    let pc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == pi).collect();
    assert!(!pc.is_empty(), "Expected call sites for 'processOrder'");
    let sh = pc[0].1.iter().find(|c| c.method_name == "someHelper");
    assert!(sh.is_some(), "Expected call to 'someHelper'");
    assert_eq!(sh.unwrap().receiver_type, None);
}

#[test]
fn test_ts_new_expression() {
    let source = r#"class Factory {
    create(): void {
        const svc = new UserService();
    }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let ci = defs.iter().position(|d| d.name == "create").unwrap();
    let cc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == ci).collect();
    assert!(!cc.is_empty(), "Expected call sites for 'create'");
    let nc = cc[0].1.iter().find(|c| c.method_name == "UserService");
    assert!(nc.is_some(), "Expected new UserService call");
    assert_eq!(nc.unwrap().receiver_type.as_deref(), Some("UserService"));
}

#[test]
fn test_ts_static_method_call() {
    let source = r#"class Processor {
    run(): void {
        MathUtils.calculate();
    }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let ri = defs.iter().position(|d| d.name == "run").unwrap();
    let rc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == ri).collect();
    assert!(!rc.is_empty(), "Expected call sites for 'run'");
    let mc = rc[0].1.iter().find(|c| c.method_name == "calculate");
    assert!(mc.is_some(), "Expected call to 'calculate'");
    assert_eq!(mc.unwrap().receiver_type.as_deref(), Some("MathUtils"));
}

#[test]
fn test_ts_arrow_function_class_property() {
    let source = r#"class ItemProcessor {
    processItem = (item: string): void => {
        this.validate(item);
    };
    validate(item: string): void {}
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let pi = defs.iter().position(|d| d.name == "processItem").unwrap();
    let pc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == pi).collect();
    assert!(!pc.is_empty(), "Expected call sites for 'processItem' arrow function");
    let vc = pc[0].1.iter().find(|c| c.method_name == "validate");
    assert!(vc.is_some(), "Expected call to 'validate'");
    assert_eq!(vc.unwrap().receiver_type.as_deref(), Some("ItemProcessor"));
}

#[test]
fn test_ts_constructor_di_field_types() {
    let source = r#"class OrderHandler {
    constructor(private orderRepo: OrderRepository, private logger: Logger) {}
    execute(): void {
        this.orderRepo.save();
        this.logger.info("done");
    }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let ei = defs.iter().position(|d| d.name == "execute").unwrap();
    let ec: Vec<_> = call_sites.iter().filter(|(i, _)| *i == ei).collect();
    assert!(!ec.is_empty(), "Expected call sites for 'execute'");

    let save = ec[0].1.iter().find(|c| c.method_name == "save");
    assert!(save.is_some(), "Expected call to 'save'");
    assert_eq!(save.unwrap().receiver_type.as_deref(), Some("OrderRepository"));

    let info = ec[0].1.iter().find(|c| c.method_name == "info");
    assert!(info.is_some(), "Expected call to 'info'");
    assert_eq!(info.unwrap().receiver_type.as_deref(), Some("Logger"));
}

#[test]
fn test_ts_multiple_calls_in_method() {
    let source = r#"class DataService {
    constructor(private repo: DataRepository) {}
    process(): void {
        this.validate();
        this.repo.findAll();
        const result = new ResultSet();
        helperFn();
        Formatter.format();
    }
    validate(): void {}
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let pi = defs.iter().position(|d| d.name == "process").unwrap();
    let pc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == pi).collect();
    assert!(!pc.is_empty(), "Expected call sites for 'process'");

    let names: Vec<&str> = pc[0].1.iter().map(|c| c.method_name.as_str()).collect();
    assert!(names.contains(&"validate"), "Expected call to 'validate'");
    assert!(names.contains(&"findAll"), "Expected call to 'findAll'");
    assert!(names.contains(&"ResultSet"), "Expected new ResultSet");
    assert!(names.contains(&"helperFn"), "Expected call to 'helperFn'");
    assert!(names.contains(&"format"), "Expected call to 'format'");

    // Check receiver types
    let validate_call = pc[0].1.iter().find(|c| c.method_name == "validate").unwrap();
    assert_eq!(validate_call.receiver_type.as_deref(), Some("DataService"));
    let find_call = pc[0].1.iter().find(|c| c.method_name == "findAll").unwrap();
    assert_eq!(find_call.receiver_type.as_deref(), Some("DataRepository"));
    let helper_call = pc[0].1.iter().find(|c| c.method_name == "helperFn").unwrap();
    assert_eq!(helper_call.receiver_type, None);
    let fmt_call = pc[0].1.iter().find(|c| c.method_name == "format").unwrap();
    assert_eq!(fmt_call.receiver_type.as_deref(), Some("Formatter"));
}

#[test]
fn test_ts_no_calls_empty_body() {
    let source = r#"class EmptyService {
    doNothing(): void {}
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let ni = defs.iter().position(|d| d.name == "doNothing").unwrap();
    let nc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == ni).collect();
    assert!(nc.is_empty(), "Expected no call sites for empty method");
}

#[test]
fn test_ts_class_field_type() {
    let source = r#"class CachedService {
    private cache: CacheService;
    lookup(): void {
        this.cache.get();
    }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let li = defs.iter().position(|d| d.name == "lookup").unwrap();
    let lc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == li).collect();
    assert!(!lc.is_empty(), "Expected call sites for 'lookup'");
    let gc = lc[0].1.iter().find(|c| c.method_name == "get");
    assert!(gc.is_some(), "Expected call to 'get'");
    assert_eq!(gc.unwrap().receiver_type.as_deref(), Some("CacheService"));
}

#[test]
fn test_ts_csharp_callers_still_work() {
    let source = r#"
public class NotificationService {
    private readonly IEmailSender _sender;
    public NotificationService(IEmailSender sender) { _sender = sender; }
    public void Notify(string message) { _sender.Send(message); this.LogResult(); }
    private void LogResult() {}
}
"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_c_sharp::LANGUAGE.into()).unwrap();
    let (defs, cs, _, _) = parse_csharp_definitions(&mut parser, source, 0);

    let ni = defs.iter().position(|d| d.name == "Notify").unwrap();
    let nc: Vec<_> = cs.iter().filter(|(i, _)| *i == ni).collect();
    assert!(!nc.is_empty(), "Expected call sites for 'Notify' (C# regression)");

    let send = nc[0].1.iter().find(|c| c.method_name == "Send");
    assert!(send.is_some(), "Expected call to 'Send'");
    assert_eq!(send.unwrap().receiver_type.as_deref(), Some("IEmailSender"));

    let log = nc[0].1.iter().find(|c| c.method_name == "LogResult");
    assert!(log.is_some(), "Expected call to 'LogResult'");
    assert_eq!(log.unwrap().receiver_type.as_deref(), Some("NotificationService"));
}


#[test]
fn test_ts_inject_field_initializer() {
    let source = r#"class MyComponent {
    private readonly zone = inject(NgZone);
    private readonly userService = inject(UserService);
    run(): void {
        this.zone.run();
        this.userService.getUser();
    }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let ri = defs.iter().position(|d| d.name == "run").unwrap();
    let rc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == ri).collect();
    assert!(!rc.is_empty(), "Expected call sites for 'run' method");

    let zone_call = rc[0].1.iter().find(|c| c.method_name == "run" && c.receiver_type.is_some());
    assert!(zone_call.is_some(), "Expected call to 'zone.run()'");
    assert_eq!(zone_call.unwrap().receiver_type.as_deref(), Some("NgZone"));

    let user_call = rc[0].1.iter().find(|c| c.method_name == "getUser");
    assert!(user_call.is_some(), "Expected call to 'userService.getUser()'");
    assert_eq!(user_call.unwrap().receiver_type.as_deref(), Some("UserService"));
}

#[test]
fn test_ts_inject_constructor_assignment() {
    let source = r#"class MyComponent {
    constructor() {
        this.store = inject(Store);
        this.router = inject(Router);
    }
    navigate(): void {
        this.store.dispatch();
        this.router.navigate();
    }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let ni = defs.iter().position(|d| d.name == "navigate").unwrap();
    let nc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == ni).collect();
    assert!(!nc.is_empty(), "Expected call sites for 'navigate' method");

    let store_call = nc[0].1.iter().find(|c| c.method_name == "dispatch");
    assert!(store_call.is_some(), "Expected call to 'store.dispatch()'");
    assert_eq!(store_call.unwrap().receiver_type.as_deref(), Some("Store"));

    let router_call = nc[0].1.iter().find(|c| c.method_name == "navigate" && c.receiver_type.is_some());
    assert!(router_call.is_some(), "Expected call to 'router.navigate()'");
    assert_eq!(router_call.unwrap().receiver_type.as_deref(), Some("Router"));
}

#[test]
fn test_ts_inject_with_generic() {
    let source = r#"class MyComponent {
    private store = inject(Store<AppState>);
    doWork(): void {
        this.store.dispatch();
    }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let di = defs.iter().position(|d| d.name == "doWork").unwrap();
    let dc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == di).collect();
    assert!(!dc.is_empty(), "Expected call sites for 'doWork' method");

    let store_call = dc[0].1.iter().find(|c| c.method_name == "dispatch");
    assert!(store_call.is_some(), "Expected call to 'store.dispatch()'");
    assert_eq!(store_call.unwrap().receiver_type.as_deref(), Some("Store"));
}


// ─── TypeScript Interface Resolution Tests ───────────────────────────

#[test]
fn test_ts_interface_implements_extracted() {
    let source = r#"
interface IUserService {
    getUser(): void;
}

class UserService implements IUserService {
    getUser(): void {}
}
"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let class_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Class).collect();
    assert_eq!(class_defs.len(), 1);
    assert_eq!(class_defs[0].name, "UserService");
    assert!(
        class_defs[0].base_types.iter().any(|b| b.contains("IUserService")),
        "Expected base_types to contain 'IUserService', got: {:?}",
        class_defs[0].base_types
    );
}

#[test]
fn test_ts_interface_call_through_field() {
    let source = r#"
interface IOrderService {
    processOrder(): void;
}

class OrderProcessor {
    constructor(private orderService: IOrderService) {}
    run(): void {
        this.orderService.processOrder();
    }
}
"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let ri = defs.iter().position(|d| d.name == "run").unwrap();
    let rc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == ri).collect();
    assert!(!rc.is_empty(), "Expected call sites for 'run' method");

    let po = rc[0].1.iter().find(|c| c.method_name == "processOrder");
    assert!(po.is_some(), "Expected call to 'processOrder'");
    assert_eq!(
        po.unwrap().receiver_type.as_deref(),
        Some("IOrderService"),
        "Expected receiver_type to be 'IOrderService'"
    );
}

#[test]
fn test_ts_multiple_implements() {
    let source = r#"
interface IReader {
    read(): void;
}
interface IWriter {
    write(): void;
}
class DataService implements IReader, IWriter {
    read(): void {}
    write(): void {}
}
"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let class_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Class && d.name == "DataService").collect();
    assert_eq!(class_defs.len(), 1);
    assert!(
        class_defs[0].base_types.iter().any(|b| b.contains("IReader")),
        "Expected base_types to contain 'IReader', got: {:?}",
        class_defs[0].base_types
    );
    assert!(
        class_defs[0].base_types.iter().any(|b| b.contains("IWriter")),
        "Expected base_types to contain 'IWriter', got: {:?}",
        class_defs[0].base_types
    );
}

#[test]
fn test_ts_extends_and_implements() {
    let source = r#"
class BaseService {
    init(): void {}
}
interface IAdminService {
    manage(): void;
}
class AdminService extends BaseService implements IAdminService {
    manage(): void {}
}
"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let class_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Class && d.name == "AdminService").collect();
    assert_eq!(class_defs.len(), 1);
    assert!(
        class_defs[0].base_types.iter().any(|b| b.contains("BaseService")),
        "Expected base_types to contain 'BaseService', got: {:?}",
        class_defs[0].base_types
    );
    assert!(
        class_defs[0].base_types.iter().any(|b| b.contains("IAdminService")),
        "Expected base_types to contain 'IAdminService', got: {:?}",
        class_defs[0].base_types
    );
}

#[test]
fn test_parse_ts_injection_token_variable() {
    let source = "export const AUTH_TOKEN = new InjectionToken<IAuthService>('AUTH_TOKEN');";
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, _call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let var_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Variable).collect();
    assert_eq!(var_defs.len(), 1, "Expected exactly one variable definition");
    assert_eq!(var_defs[0].name, "AUTH_TOKEN");
    assert!(var_defs[0].modifiers.contains(&"export".to_string()));
    assert!(var_defs[0].modifiers.contains(&"const".to_string()));

    // The parser currently captures type annotations but NOT initializer expressions.
    // For `const AUTH_TOKEN = new InjectionToken<IAuthService>(...)`, there is no explicit
    // type annotation, so the signature will be "const AUTH_TOKEN" without InjectionToken info.
    // TODO: To fully support InjectionToken patterns, the parser would need to extract
    // the initializer's constructor name (InjectionToken<IAuthService>) into the signature.
    let sig = var_defs[0].signature.as_ref().expect("Expected a signature");
    assert!(sig.contains("AUTH_TOKEN"), "Signature should contain the variable name");

    if sig.contains("InjectionToken") {
        // Parser captures initializer type — ideal behavior
        assert!(sig.contains("InjectionToken<IAuthService>"));
    } else {
        // Parser does NOT capture initializer — document the gap
        eprintln!(
            "NOTE: InjectionToken<IAuthService> NOT captured in signature. Signature: '{}'",
            sig
        );
    }
}

// ─── TypeScript Local Variable Type Extraction Tests ─────────────────

#[test]
fn test_ts_local_var_explicit_type_annotation() {
    let source = r#"class UserService {
    private repo: UserRepository;

    getUser(id: number): void {
        const result: UserResult = this.repo.findById(id);
        result.validate();
    }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let gi = defs.iter().position(|d| d.name == "getUser").unwrap();
    let gc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == gi).collect();
    assert!(!gc.is_empty(), "Expected call sites for 'getUser'");

    let validate = gc[0].1.iter().find(|c| c.method_name == "validate");
    assert!(validate.is_some(), "Expected call to 'validate'");
    assert_eq!(
        validate.unwrap().receiver_type.as_deref(),
        Some("UserResult"),
        "Local var 'result' with explicit type annotation ':UserResult' should resolve receiver_type"
    );
}

#[test]
fn test_ts_local_var_new_expression() {
    let source = r#"class OrderService {
    processOrder(): void {
        const validator = new OrderValidator();
        validator.check();
    }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let pi = defs.iter().position(|d| d.name == "processOrder").unwrap();
    let pc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == pi).collect();
    assert!(!pc.is_empty(), "Expected call sites for 'processOrder'");

    let check = pc[0].1.iter().find(|c| c.method_name == "check");
    assert!(check.is_some(), "Expected call to 'check'");
    assert_eq!(
        check.unwrap().receiver_type.as_deref(),
        Some("OrderValidator"),
        "Local var 'validator' assigned from 'new OrderValidator()' should resolve receiver_type"
    );
}

#[test]
fn test_ts_local_var_new_expression_with_generics() {
    let source = r#"class DataService {
    loadData(): void {
        const cache = new DataCache<string>();
        cache.get("key");
    }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let li = defs.iter().position(|d| d.name == "loadData").unwrap();
    let lc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == li).collect();
    assert!(!lc.is_empty(), "Expected call sites for 'loadData'");

    let get = lc[0].1.iter().find(|c| c.method_name == "get");
    assert!(get.is_some(), "Expected call to 'get'");
    assert_eq!(
        get.unwrap().receiver_type.as_deref(),
        Some("DataCache"),
        "Local var 'cache' from 'new DataCache<string>()' should resolve receiver_type to 'DataCache' (stripped generics)"
    );
}

#[test]
fn test_ts_local_var_no_type_annotation() {
    let source = r#"class SomeService {
    doWork(): void {
        const result = this.calculate();
        result.process();
    }
    calculate(): any { return null; }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let di = defs.iter().position(|d| d.name == "doWork").unwrap();
    let dc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == di).collect();
    assert!(!dc.is_empty(), "Expected call sites for 'doWork'");

    let process = dc[0].1.iter().find(|c| c.method_name == "process");
    assert!(process.is_some(), "Expected call to 'process'");
    assert_eq!(
        process.unwrap().receiver_type.as_deref(),
        Some("result"),
        "Local var 'result' with no type annotation and no new expression should preserve receiver name"
    );
}

#[test]
fn test_ts_local_var_field_types_take_precedence() {
    let source = r#"class MyComponent {
    private result: FieldType;

    doWork(): void {
        const result: LocalType = getValue();
        this.result.fieldMethod();
    }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let di = defs.iter().position(|d| d.name == "doWork").unwrap();
    let dc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == di).collect();
    assert!(!dc.is_empty(), "Expected call sites for 'doWork'");

    let field_method = dc[0].1.iter().find(|c| c.method_name == "fieldMethod");
    assert!(field_method.is_some(), "Expected call to 'fieldMethod'");
    assert_eq!(
        field_method.unwrap().receiver_type.as_deref(),
        Some("FieldType"),
        "this.result.fieldMethod() should resolve to field type 'FieldType', not local var type 'LocalType'"
    );
}

// ─── TypeScript Local Variable Type — let Declaration Without Initializer ─────

#[test]
fn test_ts_local_var_let_declaration_without_initializer() {
    let source = r#"class TestClass {
    process(): void {
        let task: DependencyTask;
        task = this.createTask();
        task.resolve();
    }
    createTask(): any { return null; }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let pi = defs.iter().position(|d| d.name == "process").unwrap();
    let pc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == pi).collect();
    assert!(!pc.is_empty(), "Expected call sites for 'process'");

    let resolve = pc[0].1.iter().find(|c| c.method_name == "resolve");
    assert!(resolve.is_some(), "Expected call to 'resolve'");
    assert_eq!(
        resolve.unwrap().receiver_type.as_deref(),
        Some("DependencyTask"),
        "Local var 'task' declared as 'let task: DependencyTask' (no initializer) should resolve receiver_type to 'DependencyTask'"
    );
}

// ─── Lambda / Arrow Function Parsing Tests ───────────────────────────

#[test]
fn test_ts_arrow_function_in_argument_calls_captured() {
    let source = r#"class ItemProcessor {
    process() {
        items.forEach(item => item.validate());
        promise.then(result => result.transform());
    }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let pi = defs.iter().position(|d| d.name == "process").unwrap();
    let pc: Vec<_> = call_sites.iter().filter(|(i, _)| *i == pi).collect();
    assert!(!pc.is_empty(), "Expected call sites for 'process'");

    let names: Vec<&str> = pc[0].1.iter().map(|c| c.method_name.as_str()).collect();
    assert!(names.contains(&"forEach"), "Expected call to 'forEach', got: {:?}", names);
    assert!(names.contains(&"validate"), "Expected call to 'validate' inside arrow function, got: {:?}", names);
    assert!(names.contains(&"then"), "Expected call to 'then', got: {:?}", names);
    assert!(names.contains(&"transform"), "Expected call to 'transform' inside arrow function, got: {:?}", names);
}

#[test]
fn test_ts_multiline_arrow_function_calls_captured() {
    let source = r#"class TaskRunner {
    execute() {
        tasks.map(t => {
            t.initialize();
            t.run();
            return t.getResult();
        });
    }
}"#;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let (defs, call_sites, _) = parse_typescript_definitions(&mut parser, source, 0);

    let ei = defs.iter().position(|d| d.name == "execute").unwrap();
    let ec: Vec<_> = call_sites.iter().filter(|(i, _)| *i == ei).collect();
    assert!(!ec.is_empty(), "Expected call sites for 'execute'");

    let names: Vec<&str> = ec[0].1.iter().map(|c| c.method_name.as_str()).collect();
    assert!(names.contains(&"initialize"), "Expected call to 'initialize' inside multiline arrow function, got: {:?}", names);
    assert!(names.contains(&"run"), "Expected call to 'run' inside multiline arrow function, got: {:?}", names);
    assert!(names.contains(&"getResult"), "Expected call to 'getResult' inside multiline arrow function, got: {:?}", names);
}
// ─── Angular Template Metadata Tests ─────────────────────────────────

// B1: extract_component_metadata tests

#[test]
fn test_extract_component_metadata_standard() {
    use super::parser_typescript::extract_component_metadata;
    let text = "Component({\n    selector: 'dashboard-embed',\n    templateUrl: './dashboard-embed.component.html',\n})";
    let result = extract_component_metadata(text);
    assert!(result.is_some());
    let (selector, tpl) = result.unwrap();
    assert_eq!(selector, "dashboard-embed");
    assert_eq!(tpl, Some("./dashboard-embed.component.html".to_string()));
}

#[test]
fn test_extract_component_metadata_double_quotes() {
    use super::parser_typescript::extract_component_metadata;
    let text = r#"Component({selector: "my-widget", templateUrl: "./my-widget.html"})"#;
    let result = extract_component_metadata(text);
    assert!(result.is_some());
    let (selector, tpl) = result.unwrap();
    assert_eq!(selector, "my-widget");
    assert_eq!(tpl, Some("./my-widget.html".to_string()));
}

#[test]
fn test_extract_component_metadata_no_template_url() {
    use super::parser_typescript::extract_component_metadata;
    let text = "Component({\n    selector: 'simple-comp',\n    template: '<div>hello</div>',\n})";
    let result = extract_component_metadata(text);
    assert!(result.is_some());
    let (selector, tpl) = result.unwrap();
    assert_eq!(selector, "simple-comp");
    assert_eq!(tpl, None);
}

#[test]
fn test_extract_component_metadata_no_selector() {
    use super::parser_typescript::extract_component_metadata;
    let text = "Component({\n    templateUrl: './file.html',\n})";
    let result = extract_component_metadata(text);
    assert!(result.is_none());
}

#[test]
fn test_extract_component_metadata_not_component() {
    use super::parser_typescript::extract_component_metadata;
    let text = "Injectable({ providedIn: 'root' })";
    let result = extract_component_metadata(text);
    assert!(result.is_none());
}

#[test]
fn test_extract_component_metadata_multiline() {
    use super::parser_typescript::extract_component_metadata;
    let text = "Component({\n    selector:\n        'multi-line-comp',\n    templateUrl:\n        './multi-line.html',\n})";
    let result = extract_component_metadata(text);
    assert!(result.is_some());
    let (selector, tpl) = result.unwrap();
    assert_eq!(selector, "multi-line-comp");
    assert_eq!(tpl, Some("./multi-line.html".to_string()));
}

// B2: extract_custom_elements tests

#[test]
fn test_extract_custom_elements_basic() {
    let html = "<div><my-component></my-component><span>text</span></div>";
    let result = super::extract_custom_elements(html);
    assert_eq!(result, vec!["my-component"]);
}

#[test]
fn test_extract_custom_elements_self_closing() {
    let html = "<my-widget /><another-comp/>";
    let result = super::extract_custom_elements(html);
    assert_eq!(result, vec!["another-comp", "my-widget"]);
}

#[test]
fn test_extract_custom_elements_with_attributes() {
    let html = r#"<my-comp [input]="value" (output)="handler($event)"></my-comp>"#;
    let result = super::extract_custom_elements(html);
    assert_eq!(result, vec!["my-comp"]);
}

#[test]
fn test_extract_custom_elements_excludes_standard_html() {
    let html = "<div><span><p><h1><input><br><table><tr><td></td></tr></table></h1></p></span></div>";
    let result = super::extract_custom_elements(html);
    assert!(result.is_empty());
}

#[test]
fn test_extract_custom_elements_excludes_ng_builtins() {
    let html = "<ng-container><ng-content></ng-content><ng-template></ng-template></ng-container>";
    let result = super::extract_custom_elements(html);
    assert!(result.is_empty());
}

#[test]
fn test_extract_custom_elements_dedup_and_case_insensitive() {
    let html = "<My-Component></My-Component><my-component></my-component><MY-COMPONENT></MY-COMPONENT>";
    let result = super::extract_custom_elements(html);
    assert_eq!(result, vec!["my-component"]);
}

#[test]
fn test_extract_custom_elements_empty_html() {
    let result = super::extract_custom_elements("");
    assert!(result.is_empty());
}

#[test]
fn test_extract_custom_elements_mixed() {
    let html = r#"
        <div class="container">
            <ng-container *ngIf="show">
                <data-grid [config]="gridConfig"></data-grid>
                <app-spinner size="large"></app-spinner>
                <span>Loading...</span>
            </ng-container>
            <app-footer></app-footer>
        </div>
    "#;
    let result = super::extract_custom_elements(html);
    assert_eq!(result, vec!["app-footer", "app-spinner", "data-grid"]);
}
