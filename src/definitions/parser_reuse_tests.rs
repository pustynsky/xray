use super::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use tree_sitter::{Language, Parser, Tree};

#[derive(Clone, Copy, Debug)]
enum Grammar {
    #[cfg(feature = "lang-csharp")]
    Csharp,
    #[cfg(feature = "lang-typescript")]
    TypeScript,
    #[cfg(feature = "lang-typescript")]
    Tsx,
    #[cfg(feature = "lang-rust")]
    Rust,
}

impl Grammar {
    fn language(self) -> Language {
        match self {
            #[cfg(feature = "lang-csharp")]
            Self::Csharp => tree_sitter_c_sharp::LANGUAGE.into(),
            #[cfg(feature = "lang-typescript")]
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            #[cfg(feature = "lang-typescript")]
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            #[cfg(feature = "lang-rust")]
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
        }
    }

    fn extension(self) -> &'static str {
        match self {
            #[cfg(feature = "lang-csharp")]
            Self::Csharp => "cs",
            #[cfg(feature = "lang-typescript")]
            Self::TypeScript => "ts",
            #[cfg(feature = "lang-typescript")]
            Self::Tsx => "tsx",
            #[cfg(feature = "lang-rust")]
            Self::Rust => "rs",
        }
    }

    fn old_source(self) -> String {
        match self {
            #[cfg(feature = "lang-csharp")]
            Self::Csharp => "class Old { void OldCaller() { OldTarget(); } }\n",
            #[cfg(feature = "lang-typescript")]
            Self::TypeScript | Self::Tsx => "class Old { oldCaller() { oldTarget(); } }\n",
            #[cfg(feature = "lang-rust")]
            Self::Rust => "fn old_caller() { old_target(); }\n",
        }.repeat(128)
    }

    fn source(self) -> &'static str {
        match self {
            #[cfg(feature = "lang-csharp")]
            Self::Csharp => "class Current {\n public void Target() {}\n public void Caller() { Target(); }\n}\nstatic class Extensions {\n public static void Extra(this Current value) { value.Target(); }\n}\n",
            #[cfg(feature = "lang-typescript")]
            Self::TypeScript | Self::Tsx => "function Target() {}\nfunction Caller() { Target(); }\n@Component({selector: 'x-sentinel', template: '<span></span>'})\nclass Current {}\n",
            #[cfg(feature = "lang-rust")]
            Self::Rust => "fn target() {}\nfn caller() { target(); }\n",
        }
    }

    fn sentinels(self) -> (&'static str, &'static str, u32, u32) {
        match self {
            #[cfg(feature = "lang-csharp")]
            Self::Csharp => ("Target", "Caller", 2, 3),
            #[cfg(feature = "lang-typescript")]
            Self::TypeScript | Self::Tsx => ("Target", "Caller", 1, 2),
            #[cfg(feature = "lang-rust")]
            Self::Rust => ("target", "caller", 1, 2),
        }
    }

    fn parser(self) -> Parser {
        let mut parser = Parser::new();
        parser.set_language(&self.language()).unwrap();
        parser
    }

    fn parse(self, parser: &mut Parser, source: &str) -> ParsedFileResult {
        let mut result = ParsedFileResult {
            path: PathBuf::from(format!("fixture.{}", self.extension())),
            definitions: Vec::new(), call_sites: Vec::new(), code_stats: Vec::new(),
            extension_methods: HashMap::new(), csharp_semantics: CSharpFileContribution::default(),
            angular_components: Vec::new(), fingerprint: DefinitionInputFingerprint::default(),
        };
        match self {
            #[cfg(feature = "lang-csharp")]
            Self::Csharp => {
                (result.definitions, result.call_sites, result.code_stats,
                    result.extension_methods, result.csharp_semantics) =
                    parse_csharp_definitions_with_semantics(parser, source, 0);
            }
            #[cfg(feature = "lang-typescript")]
            Self::TypeScript | Self::Tsx => {
                ((result.definitions, result.call_sites, result.code_stats), result.angular_components) =
                    parse_typescript_definitions_with_components(parser, source, 0);
            }
            #[cfg(feature = "lang-rust")]
            Self::Rust => {
                (result.definitions, result.call_sites, result.code_stats) =
                    parse_rust_definitions(parser, source, 0);
            }
        }
        result
    }
}

#[derive(Clone, Copy, Debug)]
enum Interruption { Cancellation, Timeout }

fn interrupt(parser: &mut Parser, source: &str, cause: Interruption) {
    let flag = AtomicUsize::new(0);
    // SAFETY: The flag outlives its attachment; detach before any assertion can unwind.
    unsafe { parser.set_cancellation_flag(Some(&flag)); }
    parser.set_timeout_micros(match cause {
        Interruption::Cancellation => 0,
        Interruption::Timeout => 100_000,
    });
    let mut furthest_read = 0;
    let mut stalled = false;
    let tree = parser.parse_with(&mut |offset, _| {
        furthest_read = furthest_read.max(offset);
        if offset >= 128 {
            match cause {
                Interruption::Cancellation => flag.store(1, Ordering::SeqCst),
                Interruption::Timeout if !stalled => {
                    stalled = true;
                    std::thread::sleep(Duration::from_millis(150));
                }
                Interruption::Timeout => {}
            }
        }
        &source.as_bytes()[offset.min(source.len())..(offset + 16).min(source.len())]
    }, None);
    // SAFETY: Clear the pointer while the attached flag is still alive.
    unsafe { parser.set_cancellation_flag(None); }
    parser.set_timeout_micros(0);
    assert!(furthest_read >= 128, "interruption must follow real parsing progress");
    assert!(tree.is_none(), "{cause:?} must interrupt, not finish the old document");
}

fn assert_bounded(tree: &Tree, source: &str) {
    let mut pending = vec![tree.root_node()];
    while let Some(node) = pending.pop() {
        assert!(node.start_byte() <= node.end_byte() && node.end_byte() <= source.len());
        let mut cursor = node.walk();
        pending.extend(node.children(&mut cursor));
    }
}

fn projection(result: &ParsedFileResult) -> serde_json::Value {
    let mut index = DefinitionIndex::default();
    apply_parsed_result(&mut index, ParsedFileResult {
        path: result.path.clone(), definitions: result.definitions.clone(),
        call_sites: result.call_sites.clone(), code_stats: result.code_stats.clone(),
        extension_methods: result.extension_methods.clone(),
        csharp_semantics: result.csharp_semantics.clone(),
        angular_components: result.angular_components.clone(), fingerprint: result.fingerprint.clone(),
    });
    let semantics = &index.csharp_semantics;
    let symbols: BTreeMap<_, _> = semantics.symbol_to_definitions.iter()
        .map(|(symbol, definitions)| (symbol.as_public_id(), definitions)).collect();
    serde_json::json!({
        "definitions": index.definitions, "calls": index.method_calls, "stats": index.code_stats,
        "extensions": index.extension_methods, "components": index.angular_components,
        "templates": index.template_children, "fingerprints": index.input_fingerprints,
        "csharp": {
            "strings": semantics.strings, "callables": semantics.callables,
            "def_to_callable": semantics.def_to_callable, "call_shapes": semantics.call_shapes_by_owner,
            "symbols": symbols, "extensions_by_file": semantics.extension_methods_by_file,
        },
    })
}

fn assert_sentinels(result: &ParsedFileResult, grammar: Grammar, line_offset: u32) {
    let (target, caller, target_line, caller_line) = grammar.sentinels();
    assert!(result.definitions.iter().any(|entry|
        entry.name == target && entry.line_start == target_line + line_offset));
    let owner = result.definitions.iter().position(|entry|
        entry.name == caller && entry.line_start == caller_line + line_offset).unwrap();
    assert!(result.call_sites.iter().any(|(index, calls)| *index == owner && calls.iter()
        .any(|call| call.method_name == target && call.line == caller_line + line_offset)));
    assert!(result.code_stats.iter().any(|(index, stats)| *index == owner && stats.call_count > 0));
    #[cfg(feature = "lang-csharp")]
    if matches!(grammar, Grammar::Csharp) {
        assert!(!result.csharp_semantics.callables.is_empty());
        assert!(result.extension_methods.get("Extra").is_some_and(|classes| classes.iter().any(|name| name == "Extensions")));
    }
    #[cfg(feature = "lang-typescript")]
    if matches!(grammar, Grammar::TypeScript | Grammar::Tsx) {
        assert_eq!(result.angular_components.len(), 1);
    }
}

fn check_reuse(grammar: Grammar, in_bounds: bool) {
    let old = grammar.old_source();
    let lengths = if in_bounds { vec![old.len(), old.len() + 100] } else { vec![grammar.source().len()] };
    for length in lengths {
        let mut source = grammar.source().to_string();
        source.push_str(&" ".repeat(length - source.len()));
        for cause in [Interruption::Cancellation, Interruption::Timeout] {
            if in_bounds {
                let mut control = grammar.parser();
                interrupt(&mut control, &old, cause);
                let resumed = control.parse(&source, None).unwrap();
                assert_bounded(&resumed, &source);
                let fresh = grammar.parser().parse(&source, None).unwrap();
                assert_ne!(resumed.root_node().to_sexp(), fresh.root_node().to_sexp(),
                    "control must expose stale, entirely in-bounds syntax");
            }
            let mut parser = grammar.parser();
            interrupt(&mut parser, &old, cause);
            let actual = grammar.parse(&mut parser, &source);
            let expected = grammar.parse(&mut grammar.parser(), &source);
            assert_sentinels(&expected, grammar, 0);
            assert_eq!(projection(&actual), projection(&expected),
                "{grammar:?} {cause:?}, next source length {length}");
            assert_sentinels(&actual, grammar, 0);
        }
    }
}

#[derive(Default)]
struct DispatchParsers {
    #[cfg(feature = "lang-csharp")]
    cs: Option<Parser>,
    #[cfg(feature = "lang-typescript")]
    ts: Option<Parser>,
    #[cfg(feature = "lang-typescript")]
    tsx: Option<Parser>,
    #[cfg(feature = "lang-rust")]
    rs: Option<Parser>,
}

impl DispatchParsers {
    fn active(&mut self, grammar: Grammar) -> &mut Parser {
        match grammar {
            #[cfg(feature = "lang-csharp")]
            Grammar::Csharp => &mut self.cs,
            #[cfg(feature = "lang-typescript")]
            Grammar::TypeScript => &mut self.ts,
            #[cfg(feature = "lang-typescript")]
            Grammar::Tsx => &mut self.tsx,
            #[cfg(feature = "lang-rust")]
            Grammar::Rust => &mut self.rs,
        }.get_or_insert_with(|| grammar.parser())
    }

    fn try_parse(&mut self, path: &Path) -> std::io::Result<Option<ParsedFileResult>> {
        #[cfg(feature = "lang-csharp")]
        self.cs.get_or_insert_with(|| Grammar::Csharp.parser());
        parse_file_with_parsers(
            path, 0,
            #[cfg(feature = "lang-csharp")] self.cs.as_mut().unwrap(),
            #[cfg(feature = "lang-typescript")] &mut self.ts,
            #[cfg(feature = "lang-typescript")] &mut self.tsx,
            #[cfg(feature = "lang-rust")] &mut self.rs,
        )
    }

    fn parse(&mut self, path: &Path) -> ParsedFileResult {
        self.try_parse(path).unwrap().unwrap()
    }
}

fn check_controls_and_dispatch(grammar: Grammar) {
    let mut parser = grammar.parser();
    grammar.parse(&mut parser, &grammar.old_source());
    let actual = grammar.parse(&mut parser, grammar.source());
    let expected = grammar.parse(&mut grammar.parser(), grammar.source());
    assert_eq!(projection(&actual), projection(&expected));
    assert_sentinels(&actual, grammar, 0);

    let fixture = tempfile::tempdir().unwrap();
    let path = |name: &str| fixture.path().join(format!("{name}.{}", grammar.extension()));
    let (target, caller, _, _) = grammar.sentinels();
    std::fs::write(path("before"), grammar.source().replace(target, "BeforeTarget").replace(caller, "BeforeCaller")).unwrap();
    std::fs::write(path("interrupted"), grammar.old_source()).unwrap();
    std::fs::write(path("after"), grammar.source()).unwrap();
    let mut parsers = DispatchParsers::default();
    let mut index = DefinitionIndex::default();
    apply_parsed_result(&mut index, parsers.parse(&path("before")));
    let flag = AtomicUsize::new(1);
    // SAFETY: The flag stays alive until detachment, before the parse result is unwrapped.
    unsafe { parsers.active(grammar).set_cancellation_flag(Some(&flag)); }
    let skipped = parsers.try_parse(&path("interrupted"));
    // SAFETY: Clear the pointer while the attached flag is still alive.
    unsafe { parsers.active(grammar).set_cancellation_flag(None); }
    let skipped = skipped.unwrap().unwrap();
    assert!(skipped.definitions.is_empty() && skipped.call_sites.is_empty() && skipped.code_stats.is_empty());
    assert!(skipped.extension_methods.is_empty() && skipped.angular_components.is_empty());
    assert!(skipped.csharp_semantics.callables.is_empty() && skipped.csharp_semantics.call_sites.is_empty());
    apply_parsed_result(&mut index, skipped);
    interrupt(parsers.active(grammar), &grammar.old_source(), Interruption::Cancellation);
    let after = parsers.parse(&path("after"));
    let fresh = DispatchParsers::default().parse(&path("after"));
    assert_eq!(projection(&after), projection(&fresh));
    assert_sentinels(&after, grammar, 0);
    apply_parsed_result(&mut index, after);
    for (owner, target) in [("BeforeCaller", "BeforeTarget"), (caller, target)] {
        let id = index.definitions.iter().position(|entry| entry.name == owner).unwrap() as u32;
        assert!(index.method_calls[&id].iter().any(|call| call.method_name == target));
    }
    assert_eq!(index.worker_panics, 0);
}

macro_rules! grammar_tests {
    ($grammar:expr, $short:ident, $bounded:ident, $controls:ident) => {
        #[test] fn $short() { check_reuse($grammar, false); }
        #[test] fn $bounded() { check_reuse($grammar, true); }
        #[test] fn $controls() { check_controls_and_dispatch($grammar); }
    };
}

#[cfg(feature = "lang-csharp")]
grammar_tests!(Grammar::Csharp, parser_reuse_csharp_shorter, parser_reuse_csharp_in_bounds, parser_reuse_csharp_controls_dispatch);
#[cfg(feature = "lang-typescript")]
grammar_tests!(Grammar::TypeScript, parser_reuse_typescript_shorter, parser_reuse_typescript_in_bounds, parser_reuse_typescript_controls_dispatch);
#[cfg(feature = "lang-typescript")]
grammar_tests!(Grammar::Tsx, parser_reuse_tsx_shorter, parser_reuse_tsx_in_bounds, parser_reuse_tsx_controls_dispatch);
#[cfg(feature = "lang-rust")]
grammar_tests!(Grammar::Rust, parser_reuse_rust_shorter, parser_reuse_rust_in_bounds, parser_reuse_rust_controls_dispatch);

#[test]
#[cfg(feature = "lang-csharp")]
fn parser_reuse_snapshot_encodings() {
    let fixture = tempfile::tempdir().unwrap();
    let decoded = format!("// ж🙂\n{}", Grammar::Csharp.source());
    let mut utf8_bom = vec![0xef, 0xbb, 0xbf];
    utf8_bom.extend_from_slice(decoded.as_bytes());
    let mut le = vec![0xff, 0xfe];
    let mut be = vec![0xfe, 0xff];
    for unit in decoded.encode_utf16() {
        le.extend_from_slice(&unit.to_le_bytes());
        be.extend_from_slice(&unit.to_be_bytes());
    }
    let mut invalid = b"// \xff\xfe\n".to_vec();
    invalid.extend_from_slice(Grammar::Csharp.source().as_bytes());
    for (bytes, text, lossy) in [
        (decoded.as_bytes().to_vec(), decoded.clone(), false),
        (utf8_bom, decoded.clone(), false), (le, decoded.clone(), false),
        (be, decoded, false), (invalid.clone(), String::from_utf8_lossy(&invalid).into_owned(), true),
    ] {
        let path = fixture.path().join("encoded.cs");
        std::fs::write(&path, bytes).unwrap();
        let (content, was_lossy, _) = read_definition_source_snapshot(&path).unwrap();
        assert_eq!(content, text);
        assert_eq!(was_lossy, lossy);
        let result = DispatchParsers::default().parse(&path);
        let mut expected = Grammar::Csharp.parse(&mut Grammar::Csharp.parser(), &text);
        expected.path = path;
        expected.fingerprint = result.fingerprint.clone();
        assert_eq!(projection(&result), projection(&expected));
        assert_sentinels(&result, Grammar::Csharp, 1);
    }
}

fn build_projection(index: &DefinitionIndex) -> BTreeMap<String, serde_json::Value> {
    index.definitions.iter().enumerate().map(|(id, entry)| {
        let mut definition = serde_json::to_value(entry).unwrap();
        definition["file_id"] = serde_json::Value::Null;
        let key = format!("{}:{}:{}", index.files[entry.file_id as usize], entry.name, entry.line_start);
        (key, serde_json::json!({
            "definition": definition, "calls": index.method_calls.get(&(id as u32)),
            "stats": index.code_stats.get(&(id as u32)),
        }))
    }).collect()
}

#[test]
fn parser_reuse_full_build_serial_parallel() {
    let fixture = tempfile::tempdir().unwrap();
    let grammars = [
        #[cfg(feature = "lang-csharp")] Grammar::Csharp,
        #[cfg(feature = "lang-typescript")] Grammar::TypeScript,
        #[cfg(feature = "lang-typescript")] Grammar::Tsx,
        #[cfg(feature = "lang-rust")] Grammar::Rust,
    ];
    for grammar in grammars {
        std::fs::write(fixture.path().join(format!("sentinel.{}", grammar.extension())), grammar.source()).unwrap();
        std::fs::write(fixture.path().join(format!("empty.{}", grammar.extension())), "").unwrap();
    }
    let args = DefIndexArgs {
        dir: fixture.path().to_string_lossy().into_owned(),
        ext: grammars.iter().map(|g| g.extension()).collect::<Vec<_>>().join(","),
        threads: 1, respect_git_exclude: false,
    };
    let serial = super::super::build_definition_index(&args);
    let parallel = super::super::build_definition_index(&DefIndexArgs { threads: 0, ..args });
    assert_eq!(serial.worker_panics, 0);
    assert_eq!(parallel.worker_panics, 0);
    assert_eq!(build_projection(&serial), build_projection(&parallel));
    assert_eq!(serial.empty_file_ids.len(), grammars.len(), "only genuinely empty controls are skipped");
    assert_eq!(parallel.empty_file_ids.len(), grammars.len());
    for grammar in grammars {
        let (target, caller, _, call_line) = grammar.sentinels();
        let owner = serial.definitions.iter().enumerate().find(|(_, entry)|
            entry.name == caller && serial.files[entry.file_id as usize].ends_with(grammar.extension())).unwrap().0;
        assert!(serial.method_calls[&(owner as u32)].iter().any(|call|
            call.method_name == target && call.line == call_line));
    }
}
