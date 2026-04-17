//! Unit tests for XML on-demand parser.

use super::parser_xml::{is_xml_extension, parse_xml_on_demand, parse_xml_on_demand_with_warnings};
use crate::definitions::DefinitionKind;

// ─── is_xml_extension ───────────────────────────────────────────────

#[test]
fn test_xml_extension_xml() {
    assert!(is_xml_extension("xml"));
    assert!(is_xml_extension("XML"));
    assert!(is_xml_extension("Xml"));
}

#[test]
fn test_xml_extension_config() {
    assert!(is_xml_extension("config"));
    assert!(is_xml_extension("Config"));
    assert!(is_xml_extension("CONFIG"));
}

#[test]
fn test_xml_extension_csproj() {
    assert!(is_xml_extension("csproj"));
}

#[test]
fn test_xml_extension_props_targets() {
    assert!(is_xml_extension("props"));
    assert!(is_xml_extension("targets"));
}

#[test]
fn test_xml_extension_manifestxml() {
    assert!(is_xml_extension("manifestxml"));
}

#[test]
fn test_xml_extension_resx() {
    assert!(is_xml_extension("resx"));
}

#[test]
fn test_xml_extension_non_xml() {
    assert!(!is_xml_extension("cs"));
    assert!(!is_xml_extension("rs"));
    assert!(!is_xml_extension("json"));
    assert!(!is_xml_extension("yaml"));
    assert!(!is_xml_extension("toml"));
}

// ─── Basic XML Parsing ──────────────────────────────────────────────

#[test]
fn test_parse_simple_xml() {
    let xml = r#"<?xml version="1.0"?>
<Root>
  <Name>Test</Name>
  <Value>42</Value>
</Root>"#;
    let defs = parse_xml_on_demand(xml, "test.xml").unwrap();
    assert!(defs.len() >= 3, "Expected Root + Name + Value, got {}", defs.len());

    let root = defs.iter().find(|d| d.entry.name == "Root").unwrap();
    assert_eq!(root.entry.kind, DefinitionKind::XmlElement);
    assert!(root.has_child_elements);
    assert!(root.entry.parent.is_none());
    assert!(root.text_content.is_none());

    let name = defs.iter().find(|d| d.entry.name == "Name").unwrap();
    assert_eq!(name.entry.parent.as_deref(), Some("Root"));
    assert_eq!(name.text_content.as_deref(), Some("Test"));
    assert!(!name.has_child_elements);

    let value = defs.iter().find(|d| d.entry.name == "Value").unwrap();
    assert_eq!(value.text_content.as_deref(), Some("42"));
}

// ─── SearchService Example ──────────────────────────────────────────

#[test]
fn test_parse_search_service_xml() {
    let xml = r#"<SearchService>
  <Deploy>true</Deploy>
  <ServiceType>Search</ServiceType>
  <Name>DF-MSIT-SCUS-Idx-1</Name>
  <Sku>standard</Sku>
  <Location>West Central US</Location>
  <ReplicaCount>3</ReplicaCount>
  <PartitionCount>1</PartitionCount>
  <SemanticSearch>standard</SemanticSearch>
</SearchService>"#;

    let defs = parse_xml_on_demand(xml, "test.xml").unwrap();

    // SearchService is a block (has children)
    let ss = defs.iter().find(|d| d.entry.name == "SearchService").unwrap();
    assert!(ss.has_child_elements);
    assert!(ss.text_content.is_none());

    // ServiceType is a leaf with text "Search"
    let st = defs.iter().find(|d| d.entry.name == "ServiceType").unwrap();
    assert!(!st.has_child_elements);
    assert_eq!(st.text_content.as_deref(), Some("Search"));
    assert_eq!(st.entry.parent.as_deref(), Some("SearchService"));

    // All children should have SearchService as parent
    let children = defs.iter()
        .filter(|d| d.entry.parent.as_deref() == Some("SearchService"))
        .count();
    assert_eq!(children, 8);
}

// ─── Signature (XPath-like) ─────────────────────────────────────────

#[test]
fn test_signature_xpath() {
    let xml = r#"<configuration>
  <appSettings>
    <add key="DbConnection" value="test" />
  </appSettings>
</configuration>"#;

    let defs = parse_xml_on_demand(xml, "test.config").unwrap();

    let config = defs.iter().find(|d| d.entry.name == "configuration").unwrap();
    assert_eq!(config.entry.signature.as_deref(), Some("configuration"));

    let app_settings = defs.iter().find(|d| d.entry.name == "appSettings").unwrap();
    assert_eq!(
        app_settings.entry.signature.as_deref(),
        Some("configuration > appSettings")
    );

    let add_elem = defs.iter().find(|d| d.entry.name == "add").unwrap();
    // Should include key attribute in signature
    assert!(
        add_elem.entry.signature.as_deref().unwrap().contains("add[@key=DbConnection]"),
        "Signature should include key attr: {:?}",
        add_elem.entry.signature
    );
}

// ─── XML Attributes ─────────────────────────────────────────────────

#[test]
fn test_xml_attributes_extraction() {
    let xml = r#"<root>
  <add key="DbConnection" value="Server=." />
</root>"#;

    let defs = parse_xml_on_demand(xml, "test.config").unwrap();
    let add_elem = defs.iter().find(|d| d.entry.name == "add").unwrap();

    assert!(add_elem.entry.attributes.contains(&"key=DbConnection".to_string()));
    assert!(add_elem.entry.attributes.contains(&"value=Server=.".to_string()));
}

// ─── Self-closing Elements ──────────────────────────────────────────

#[test]
fn test_self_closing_element() {
    let xml = r#"<configuration>
  <appSettings>
    <add key="Timeout" value="30" />
    <add key="Mode" value="prod" />
  </appSettings>
</configuration>"#;

    let defs = parse_xml_on_demand(xml, "test.config").unwrap();

    let adds: Vec<_> = defs.iter().filter(|d| d.entry.name == "add").collect();
    assert_eq!(adds.len(), 2);

    // Self-closing elements are leaves (no children)
    for add in &adds {
        assert!(!add.has_child_elements);
        assert_eq!(add.entry.parent.as_deref(), Some("appSettings"));
    }

    // appSettings should be a block
    let app_settings = defs.iter().find(|d| d.entry.name == "appSettings").unwrap();
    assert!(app_settings.has_child_elements);
}

// ─── Line Numbers ───────────────────────────────────────────────────

#[test]
fn test_line_numbers() {
    let xml = r#"<Root>
  <First>hello</First>
  <Second>world</Second>
</Root>"#;

    let defs = parse_xml_on_demand(xml, "test.xml").unwrap();

    let root = defs.iter().find(|d| d.entry.name == "Root").unwrap();
    assert_eq!(root.entry.line_start, 1);
    assert_eq!(root.entry.line_end, 4);

    let first = defs.iter().find(|d| d.entry.name == "First").unwrap();
    assert_eq!(first.entry.line_start, 2);
    assert_eq!(first.entry.line_end, 2);
}

// ─── containsLine logic ─────────────────────────────────────────────

#[test]
fn test_find_containing_element() {
    let xml = r#"<Root>
  <Section>
    <Item>hello</Item>
  </Section>
</Root>"#;

    let defs = parse_xml_on_demand(xml, "test.xml").unwrap();

    // Line 3 should be contained by Item, Section, and Root
    let line3_containing: Vec<_> = defs.iter()
        .filter(|d| d.entry.line_start <= 3 && d.entry.line_end >= 3)
        .collect();

    assert!(line3_containing.len() >= 3);

    // Innermost (smallest range) should be Item
    let innermost = line3_containing.iter()
        .min_by_key(|d| d.entry.line_end - d.entry.line_start)
        .unwrap();
    assert_eq!(innermost.entry.name, "Item");
}

// ─── Parent Promotion Logic ─────────────────────────────────────────

#[test]
fn test_leaf_should_promote() {
    let xml = r#"<SearchService>
  <ServiceType>Search</ServiceType>
  <Name>Test</Name>
</SearchService>"#;

    let defs = parse_xml_on_demand(xml, "test.xml").unwrap();

    let service_type = defs.iter().find(|d| d.entry.name == "ServiceType").unwrap();
    assert!(!service_type.has_child_elements, "ServiceType should be leaf");

    let search_service = defs.iter().find(|d| d.entry.name == "SearchService").unwrap();
    assert!(search_service.has_child_elements, "SearchService should be block");
}

#[test]
fn test_block_should_not_promote() {
    let xml = r#"<Root>
  <Section>
    <Item>hello</Item>
  </Section>
</Root>"#;

    let defs = parse_xml_on_demand(xml, "test.xml").unwrap();

    let section = defs.iter().find(|d| d.entry.name == "Section").unwrap();
    assert!(section.has_child_elements, "Section should be block (has Item child)");
}

// ─── Text Content ───────────────────────────────────────────────────

#[test]
fn test_text_content_leaf() {
    let xml = "<Item>hello world</Item>";
    let defs = parse_xml_on_demand(xml, "test.xml").unwrap();
    let item = defs.iter().find(|d| d.entry.name == "Item").unwrap();
    assert_eq!(item.text_content.as_deref(), Some("hello world"));
}

#[test]
fn test_text_content_block_none() {
    let xml = "<Items><Item>hello</Item></Items>";
    let defs = parse_xml_on_demand(xml, "test.xml").unwrap();
    let items = defs.iter().find(|d| d.entry.name == "Items").unwrap();
    assert!(items.text_content.is_none(), "Block elements should have no textContent");
}

#[test]
fn test_text_content_truncation() {
    let long_text = "A".repeat(300);
    let xml = format!("<Item>{}</Item>", long_text);
    let defs = parse_xml_on_demand(&xml, "test.xml").unwrap();
    let item = defs.iter().find(|d| d.entry.name == "Item").unwrap();
    let tc = item.text_content.as_ref().unwrap();
    // ASCII: 200 chars = 200 bytes exactly, plus "..." suffix means total byte len <= 203
    assert_eq!(tc.chars().count(), 200, "textContent should be truncated to 200 chars");
    assert!(tc.ends_with("..."));
}

/// Regression test for UTF-8 panic BLOCKER-1.
/// Before the fix: `&combined[..197]` would panic with
/// "byte index 197 is not a char boundary" for Cyrillic (2 bytes per char).
/// After the fix: char-aware truncation — no panic, exact 200-char output.
#[test]
fn test_text_content_truncation_utf8_cyrillic() {
    // "абвгд" = 5 Cyrillic chars = 10 bytes; repeat 50 times = 250 chars = 500 bytes.
    let long_text = "абвгд".repeat(50);
    let xml = format!("<Item>{}</Item>", long_text);

    // This call must NOT panic.
    let defs = parse_xml_on_demand(&xml, "test.xml")
        .expect("parse_xml_on_demand should succeed on Cyrillic input without panicking");

    let item = defs
        .iter()
        .find(|d| d.entry.name == "Item")
        .expect("Item element should be parsed");
    let tc = item.text_content.as_ref().expect("Item should have text_content");

    // Exactly 200 chars (197 content + 3 dots) in the truncated form.
    assert_eq!(
        tc.chars().count(),
        200,
        "UTF-8 content should be truncated by char count, not bytes"
    );
    assert!(tc.ends_with("..."), "truncated marker must be present");
    // The truncated content (without trailing "...") must be valid Cyrillic — if
    // we reached this line, UTF-8 validity is already guaranteed by the String type.
    let content_part = tc.trim_end_matches('.');
    assert!(
        content_part.chars().all(|c| c == 'а' || c == 'б' || c == 'в' || c == 'г' || c == 'д'),
        "only Cyrillic chars from input should remain, got: {:?}",
        content_part
    );
}

/// Regression test for UTF-8 panic BLOCKER-1 (4-byte chars).
/// Emoji occupy 4 bytes per char in UTF-8. Without the fix,
/// `&combined[..197]` would land in the middle of an emoji sequence.
#[test]
fn test_text_content_truncation_utf8_emoji() {
    // "🎉" = 1 char = 4 bytes; repeat 250 times = 250 chars = 1000 bytes.
    // Must exceed the 200-char limit to trigger the truncation branch.
    let long_text = "🎉".repeat(250);
    let xml = format!("<Item>{}</Item>", long_text);

    // Before the fix: combined.len()=1000 > 200 ⇒ truncate branch ⇒
    // `&combined[..197]` lands inside a 4-byte emoji ⇒ panic on char boundary.
    let defs = parse_xml_on_demand(&xml, "test.xml")
        .expect("parse_xml_on_demand should succeed on emoji input without panicking");

    let item = defs
        .iter()
        .find(|d| d.entry.name == "Item")
        .expect("Item element should be parsed");
    let tc = item.text_content.as_ref().expect("Item should have text_content");

    // After the fix: truncated to 200 chars total (197 emoji + "...").
    assert_eq!(
        tc.chars().count(),
        200,
        "emoji input of 250 chars (1000 bytes) should be truncated by char count"
    );
    assert!(tc.ends_with("..."), "truncated marker must be present");
}

// ─── Nested XML ─────────────────────────────────────────────────────

#[test]
fn test_deeply_nested_xml() {
    let xml = r#"<A>
  <B>
    <C>
      <D>leaf</D>
    </C>
  </B>
</A>"#;

    let defs = parse_xml_on_demand(xml, "test.xml").unwrap();

    let d = defs.iter().find(|d| d.entry.name == "D").unwrap();
    assert_eq!(d.entry.parent.as_deref(), Some("C"));
    assert_eq!(d.text_content.as_deref(), Some("leaf"));
    assert!(d.entry.signature.as_deref().unwrap().contains("A > B > C > D"));
}

// ─── Empty / Malformed XML ──────────────────────────────────────────

#[test]
fn test_empty_xml() {
    let xml = "";
    let defs = parse_xml_on_demand(xml, "test.xml").unwrap();
    assert!(defs.is_empty());
}

#[test]
fn test_prolog_only() {
    let xml = r#"<?xml version="1.0" encoding="utf-8"?>"#;
    let defs = parse_xml_on_demand(xml, "test.xml").unwrap();
    assert!(defs.is_empty());
}

// ─── Mixed Content ──────────────────────────────────────────────────

#[test]
fn test_mixed_content_no_text_content() {
    let xml = "<Description>Some text <b>bold</b> more text</Description>";
    let defs = parse_xml_on_demand(xml, "test.xml").unwrap();

    let desc = defs.iter().find(|d| d.entry.name == "Description").unwrap();
    // Mixed content — has child elements, so textContent should be None
    assert!(desc.has_child_elements);
    assert!(desc.text_content.is_none());
}

// ─── Multiple Roots / siblings ──────────────────────────────────────

#[test]
fn test_web_config_structure() {
    let xml = r#"<?xml version="1.0"?>
<configuration>
  <connectionStrings>
    <add name="Default" connectionString="Server=." />
  </connectionStrings>
  <appSettings>
    <add key="Timeout" value="30" />
  </appSettings>
</configuration>"#;

    let defs = parse_xml_on_demand(xml, "web.config").unwrap();

    // configuration is a block
    let config = defs.iter().find(|d| d.entry.name == "configuration").unwrap();
    assert!(config.has_child_elements);

    // connectionStrings and appSettings are blocks
    let conn = defs.iter().find(|d| d.entry.name == "connectionStrings").unwrap();
    assert!(conn.has_child_elements);

    let app = defs.iter().find(|d| d.entry.name == "appSettings").unwrap();
    assert!(app.has_child_elements);

    // Two <add> elements
    let adds: Vec<_> = defs.iter().filter(|d| d.entry.name == "add").collect();
    assert_eq!(adds.len(), 2);
}

// ─── Parent Index ───────────────────────────────────────────────────

#[test]
fn test_parent_index_correctness() {
    let xml = r#"<Root>
  <Child1>text</Child1>
  <Child2>text</Child2>
</Root>"#;

    let defs = parse_xml_on_demand(xml, "test.xml").unwrap();

    let root_idx = defs.iter().position(|d| d.entry.name == "Root").unwrap();
    let c1 = defs.iter().find(|d| d.entry.name == "Child1").unwrap();
    let c2 = defs.iter().find(|d| d.entry.name == "Child2").unwrap();

    assert_eq!(c1.parent_index, Some(root_idx));
    assert_eq!(c2.parent_index, Some(root_idx));
    assert!(defs[root_idx].parent_index.is_none());
}

// =========================================================================
// Phase 2 regression tests (2026-04-17)
// =========================================================================
// These tests cover the walker refactor (WALKER-1/2/3/7), the `ParseResult`
// warnings surface, and the extended extension set (vcxproj, nuspec, etc.).
// -------------------------------------------------------------------------

/// WALKER-2: even when tree-sitter cannot recover an element name from a
/// malformed opening tag, the walker must still descend into the children
/// so well-formed nested elements are not silently lost. A warning is
/// recorded for the problematic node.
#[test]
fn test_walker_malformed_recursion() {
    // Unterminated outer tag (missing '>') — tree-sitter-xml usually still
    // parses the inner <Child> as an element node. We want to see it in the
    // output regardless of what happens to <Broken.
    let xml = "<Broken <Child>ok</Child>";
    let result = parse_xml_on_demand_with_warnings(xml, "test.xml").unwrap();

    // The malformed outer node may or may not produce a warning (depends on
    // tree-sitter-xml's error-recovery shape), but the nested <Child> must
    // still be captured — that's the contract we care about.
    let child_count = result
        .definitions
        .iter()
        .filter(|d| d.entry.name == "Child")
        .count();
    assert!(
        child_count >= 1,
        "WALKER-2: nested <Child> must survive malformed parent, got defs: {:?}",
        result
            .definitions
            .iter()
            .map(|d| d.entry.name.as_str())
            .collect::<Vec<_>>()
    );
}

/// Well-formed input must yield an empty warnings list. This is a sanity
/// check that the warnings surface does not spuriously fire.
#[test]
fn test_warnings_empty_on_wellformed() {
    let xml = r#"<Root>
  <Child1 attr="value">text</Child1>
  <Child2>
    <Nested>deep</Nested>
  </Child2>
</Root>"#;
    let result = parse_xml_on_demand_with_warnings(xml, "test.xml").unwrap();

    assert!(
        result.warnings.is_empty(),
        "Well-formed XML should produce zero warnings, got: {:?}",
        result.warnings
    );
    assert!(
        !result.definitions.is_empty(),
        "Well-formed XML should produce definitions"
    );
}

/// WALKER-3: deeply nested XML must NEVER stack-overflow, regardless of
/// whether tree-sitter-xml's own parser produces a deep AST or flattens it.
/// Our walker owns a tripwire at MAX_RECURSION_DEPTH (1024) as defense in
/// depth — this test proves the path is safe for adversarial input. If the
/// tripwire fires, we also assert it produces a well-formed warning; if
/// tree-sitter collapses the structure before our walker sees it, the test
/// still passes (we care about no-panic, not about which layer defends).
#[test]
fn test_deeply_nested_no_stack_overflow() {
    // Build a pathologically deep document: 2500 levels, well past our 1024
    // tripwire so that IF tree-sitter surfaces the full depth, our limit
    // will trip.
    const DEPTH: usize = 2500;
    let mut xml = String::with_capacity(DEPTH * 10);
    for i in 0..DEPTH {
        xml.push_str(&format!("<L{}>", i));
    }
    xml.push_str("inner");
    for i in (0..DEPTH).rev() {
        xml.push_str(&format!("</L{}>", i));
    }

    // Primary contract: no panic, no stack overflow — Result::Ok or an
    // explicit Err with typed `XmlParseError` is acceptable. We do NOT
    // assert on the warnings vec because tree-sitter-xml may clamp the AST
    // depth internally, producing a shallower tree that our walker never
    // needs to truncate.
    let outcome = parse_xml_on_demand_with_warnings(&xml, "test.xml");
    assert!(
        outcome.is_ok(),
        "Deeply nested XML must never cause a hard error; got {:?}",
        outcome.err()
    );

    // Secondary contract: IF a depth warning is present, its message must be
    // well-formed (contains the sentinel text the caller surfaces to users).
    // This protects against silent regressions where the format string
    // diverges from the one the handler matches on.
    let result = outcome.unwrap();
    for w in &result.warnings {
        if w.contains("nesting exceeded") {
            assert!(
                w.contains("truncated"),
                "Depth warning must mention truncation: {}",
                w
            );
        }
    }
}

// --- Extended extension set (Phase 2) ----------------------------------

#[test]
fn test_xml_extension_vcxproj() {
    assert!(is_xml_extension("vcxproj"));
}

#[test]
fn test_xml_extension_vbproj_fsproj() {
    assert!(is_xml_extension("vbproj"));
    assert!(is_xml_extension("fsproj"));
}

#[test]
fn test_xml_extension_nuspec() {
    assert!(is_xml_extension("nuspec"));
}

#[test]
fn test_xml_extension_vsixmanifest() {
    assert!(is_xml_extension("vsixmanifest"));
}

#[test]
fn test_xml_extension_appxmanifest() {
    assert!(is_xml_extension("appxmanifest"));
}