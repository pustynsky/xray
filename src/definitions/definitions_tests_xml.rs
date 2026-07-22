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
fn test_text_content_cdata_matches_plain_text() {
    let xml = "<Root><Plain>marker</Plain><Cdata><![CDATA[marker]]></Cdata></Root>";
    let result = parse_xml_on_demand_with_warnings(xml, "test.xml").unwrap();

    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    for element_name in ["Plain", "Cdata"] {
        let element = result
            .definitions
            .iter()
            .find(|definition| definition.entry.name == element_name)
            .unwrap();
        assert_eq!(element.text_content.as_deref(), Some("marker"));
    }
}

#[test]
fn test_text_content_preserves_entity_references() {
    let xml = "<Root><Named>alpha &amp; beta</Named><Numeric>left &#x26; right</Numeric></Root>";
    let result = parse_xml_on_demand_with_warnings(xml, "test.xml").unwrap();

    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    for (element_name, expected_text) in
        [("Named", "alpha &amp; beta"), ("Numeric", "left &#x26; right")]
    {
        let element = result
            .definitions
            .iter()
            .find(|definition| definition.entry.name == element_name)
            .unwrap();
        assert_eq!(element.text_content.as_deref(), Some(expected_text));
    }
}

#[test]
fn test_text_content_preserves_segment_boundaries() {
    let xml = "<Mixed>ONE<![CDATA[TWO]]>&amp;THREE</Mixed>";
    let definitions = parse_xml_on_demand(xml, "test.xml").unwrap();
    let element = definitions
        .iter()
        .find(|definition| definition.entry.name == "Mixed")
        .unwrap();

    assert_eq!(element.text_content.as_deref(), Some("ONETWO&amp;THREE"));
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

#[test]
fn test_mismatched_tags_produce_warning() {
    let xml = "<Root><Child></Root>";
    let result = parse_xml_on_demand_with_warnings(xml, "test.xml").unwrap();

    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.contains("expected `</Child>`")),
        "Malformed XML should report the specific mismatch: {:?}",
        result.warnings
    );
    assert!(!result.warnings.iter().any(|warning| warning.contains("syntax errors")));
}

#[test]
fn test_encoding_declaration_mismatch_produces_warning() {
    let xml = r#"<?xml version="1.0" encoding="utf-16"?><Root><Value>marker</Value></Root>"#;
    let result = parse_xml_on_demand_with_warnings(xml, "test.props").unwrap();

    assert!(result.definitions.iter().any(|definition| definition.entry.name == "Value"));
    assert!(
        result.warnings.iter().any(|warning| {
            warning.contains("declares encoding 'utf-16'") && warning.contains("UTF-8")
        }),
        "encoding mismatch must be explicit: {:?}",
        result.warnings
    );
}

#[test]
fn test_utf8_encoding_declaration_does_not_warn() {
    for xml in [
        r#"<?xml version="1.0" encoding="utf-8"?><Root/>"#,
        concat!("\u{feff}", "<?xml version='1.0' encoding = 'UTF-8'?><Root/>"),
    ] {
        let result = parse_xml_on_demand_with_warnings(xml, "test.xml").unwrap();
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    }
}

#[test]
fn test_encoding_attribute_outside_declaration_does_not_warn() {
    let xml = r#"<Root encoding="utf-16"><Value>marker</Value></Root>"#;
    let result = parse_xml_on_demand_with_warnings(xml, "test.xml").unwrap();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
}

#[test]
fn test_non_declaration_processing_instruction_does_not_warn() {
    let xml = r#"<?xml-stylesheet encoding="utf-16"?><Root/>"#;
    let result = parse_xml_on_demand_with_warnings(xml, "test.xml").unwrap();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
}

#[test]
fn test_us_ascii_declaration_requires_ascii_source() {
    let ascii = r#"<?xml version="1.0" encoding="us-ascii"?><Root>plain</Root>"#;
    let ascii_result = parse_xml_on_demand_with_warnings(ascii, "test.xml").unwrap();
    assert!(ascii_result.warnings.is_empty(), "{:?}", ascii_result.warnings);

    let non_ascii = r#"<?xml version="1.0" encoding="us-ascii"?><Root>Привет</Root>"#;
    let non_ascii_result = parse_xml_on_demand_with_warnings(non_ascii, "test.xml").unwrap();
    assert!(
        non_ascii_result
            .warnings
            .iter()
            .any(|warning| warning.contains("declares encoding 'us-ascii'")),
        "{:?}",
        non_ascii_result.warnings
    );
}

#[test]
fn test_unterminated_encoding_value_relies_on_syntax_warning() {
    let xml = r#"<?xml version="1.0" encoding="utf-16?><Root/>"#;
    let result = parse_xml_on_demand_with_warnings(xml, "test.xml").unwrap();
    assert!(result.warnings.iter().any(|warning| warning.contains("syntax errors")));
    assert!(!result.warnings.iter().any(|warning| warning.contains("declares encoding")));
}

#[test]
fn test_forbidden_xml_1_0_characters_produce_warning() {
    for (character, code_point) in [('\u{1}', "U+0001"), ('\u{ffff}', "U+FFFF")] {
        let xml = format!("<Root><Value>A{character}B</Value></Root>");
        let result = parse_xml_on_demand_with_warnings(&xml, "test.targets").unwrap();

        let value = result
            .definitions
            .iter()
            .find(|definition| definition.entry.name == "Value")
            .expect("recovered Value element");
        let expected_text = format!("A{character}B");
        assert_eq!(value.text_content.as_deref(), Some(expected_text.as_str()));
        assert!(
            result.warnings.iter().any(|warning| warning.contains(code_point)),
            "forbidden {code_point} must be diagnosed: {:?}",
            result.warnings
        );
    }
}

#[test]
fn test_valid_xml_1_0_characters_do_not_warn() {
    let xml = "<Root><Value>tab\tline\nreturn\rПривет 😀</Value></Root>";
    let result = parse_xml_on_demand_with_warnings(xml, "test.xml").unwrap();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
}

#[test]
fn test_forbidden_character_location_is_reported() {
    let xml = "<Root>\r\n  <Value>A\u{0}B</Value>\n</Root>";
    let result = parse_xml_on_demand_with_warnings(xml, "test.xml").unwrap();
    assert!(
        result.warnings.iter().any(|warning| {
            warning.contains("U+0000")
                && warning.contains("line 2")
                && warning.contains("column 11")
        }),
        "{:?}",
        result.warnings
    );
}

#[test]
fn test_forbidden_characters_are_diagnosed_in_all_xml_regions() {
    for xml in [
        "<Root bad=\"A\u{1}B\"/>",
        "<Root><!-- A\u{1}B --></Root>",
        "<Root><![CDATA[A\u{1}B]]></Root>",
    ] {
        let result = parse_xml_on_demand_with_warnings(xml, "test.xml").unwrap();
        assert!(
            result.warnings.iter().any(|warning| warning.contains("U+0001")),
            "{:?}",
            result.warnings
        );
    }
}

#[test]
fn test_forbidden_character_warnings_are_bounded() {
    const EXPECTED_WARNING_CAP: usize = 8;
    let controls = "\u{1}".repeat(EXPECTED_WARNING_CAP + 3);
    let xml = format!("<Root>{controls}</Root>");
    let result = parse_xml_on_demand_with_warnings(&xml, "test.xml").unwrap();
    let detail_count = result
        .warnings
        .iter()
        .filter(|warning| warning.contains("forbids raw character"))
        .count();
    assert_eq!(detail_count, EXPECTED_WARNING_CAP);
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.contains("3 additional forbidden")));
}

#[test]
fn test_malformed_cdata_produces_warning() {
    let xml = "<Root><Item><![CDATA[invalid</Item></Root>";
    let result = parse_xml_on_demand_with_warnings(xml, "test.xml").unwrap();

    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.contains("CDATA not closed")),
        "Malformed CDATA should report the specific error: {:?}",
        result.warnings
    );
    assert!(!result.warnings.iter().any(|warning| warning.contains("syntax errors")));
}

#[test]
fn test_unterminated_attribute_produces_warning() {
    let xml = "<Root><Item Name=\"unterminated></Item></Root>";
    let result = parse_xml_on_demand_with_warnings(xml, "test.xml").unwrap();

    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.contains("namespace/well-formedness validation failed")),
        "Malformed attribute should report a specific parse warning: {:?}",
        result.warnings
    );
    assert_eq!(
        result
            .warnings
            .iter()
            .filter(|warning| warning.contains("syntax errors"))
            .count(),
        0,
        "specific strict-reader warning should suppress the generic syntax warning"
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


#[test]
fn test_duplicate_xml_attribute_produces_warning_and_recovered_definitions() {
    let xml = r#"<Root A="1" A="2"><Child>value</Child></Root>"#;
    let result = parse_xml_on_demand_with_warnings(xml, "duplicate.xml").unwrap();

    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.contains("duplicate attribute") && warning.contains("A")),
        "duplicate attribute should be diagnosed: {:?}",
        result.warnings
    );
    assert!(
        result
            .definitions
            .iter()
            .any(|definition| definition.entry.name == "Root"),
        "recovery definitions should remain available"
    );
}

#[test]
fn test_duplicate_expanded_xml_attribute_produces_warning() {
    let xml = r#"<Root xmlns:a="urn:same" xmlns:b="urn:same" a:Id="1" b:Id="2" />"#;
    let result = parse_xml_on_demand_with_warnings(xml, "expanded-duplicate.xml").unwrap();

    assert!(
        result.warnings.iter().any(|warning| {
            warning.contains("duplicate attribute")
                && warning.contains("urn:same")
                && warning.contains("Id")
        }),
        "duplicate expanded attribute should be diagnosed: {:?}",
        result.warnings
    );
}

#[test]
fn test_undeclared_xml_prefixes_produce_warnings_and_recovered_definitions() {
    let xml = r#"<Root bad:Attr="1"><bad:Child>value</bad:Child></Root>"#;
    let result = parse_xml_on_demand_with_warnings(xml, "prefix.xml").unwrap();

    let prefix_warnings = result
        .warnings
        .iter()
        .filter(|warning| warning.contains("undeclared prefix 'bad'"))
        .count();
    assert_eq!(prefix_warnings, 2, "element and attribute prefixes: {:?}", result.warnings);
    assert!(
        result
            .definitions
            .iter()
            .any(|definition| definition.entry.name == "Root"),
        "recovery definitions should remain available"
    );
}

#[test]
fn test_declared_and_reserved_xml_prefixes_do_not_warn() {
    let xml = r#"<Root xmlns="urn:default" xmlns:p="urn:child" xml:lang="en">
  <Container><p:Child p:Id="1" /></Container>
</Root>"#;
    let result = parse_xml_on_demand_with_warnings(xml, "namespaces.xml").unwrap();

    assert!(
        result.warnings.is_empty(),
        "valid inherited/default/xml namespaces should not warn: {:?}",
        result.warnings
    );
}


#[test]
fn test_legal_reserved_xml_prefix_self_binding_does_not_warn() {
    let xml = r#"<Root xmlns:xml="http://www.w3.org/XML/1998/namespace" xml:lang="en" />"#;
    let result = parse_xml_on_demand_with_warnings(xml, "reserved-self-binding.xml").unwrap();

    assert!(
        result.warnings.is_empty(),
        "legal xml prefix self-binding should not warn: {:?}",
        result.warnings
    );
}


#[test]
fn test_distinct_expanded_xml_attributes_and_prefix_shadowing_do_not_warn() {
    let xml = r#"<Root xmlns:a="urn:a" xmlns:b="urn:b" a:Id="1" b:Id="2" Id="3">
  <Container xmlns:a="urn:shadow"><a:Child a:Id="4" /></Container>
</Root>"#;
    let result = parse_xml_on_demand_with_warnings(xml, "shadowing.xml").unwrap();

    assert!(
        result.warnings.is_empty(),
        "distinct expanded names and legal shadowing should not warn: {:?}",
        result.warnings
    );
}

#[test]
fn test_duplicate_namespace_declaration_produces_warning() {
    let xml = r#"<Root xmlns:p="urn:first" xmlns:p="urn:second" />"#;
    let result = parse_xml_on_demand_with_warnings(xml, "duplicate-xmlns.xml").unwrap();

    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.contains("duplicate attribute") && warning.contains("xmlns:p")),
        "duplicate namespace declaration should warn: {:?}",
        result.warnings
    );
}

#[test]
fn test_invalid_reserved_xml_prefix_binding_produces_warning() {
    let xml = r#"<Root xmlns:xml="urn:not-the-reserved-namespace"><Child /></Root>"#;
    let result = parse_xml_on_demand_with_warnings(xml, "reserved-prefix.xml").unwrap();

    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.contains("namespace/well-formedness validation failed")),
        "invalid reserved binding should warn: {:?}",
        result.warnings
    );
    assert!(
        result
            .definitions
            .iter()
            .any(|definition| definition.entry.name == "Root"),
        "tree-sitter recovery definitions should remain available"
    );
}

#[test]
fn test_xml_structure_warnings_are_bounded() {
    const EXPECTED_WARNING_LIMIT: usize = 16;

    let mut xml = String::from("<Root");
    for index in 0..32 {
        xml.push_str(&format!(" p{index}:A{index}=\"1\""));
    }
    xml.push_str(" />");
    let result = parse_xml_on_demand_with_warnings(&xml, "warning-cap.xml").unwrap();

    let structure_warning_count = result
        .warnings
        .iter()
        .filter(|warning| warning.contains("undeclared prefix"))
        .count();
    assert_eq!(structure_warning_count, EXPECTED_WARNING_LIMIT);
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
    // Build a pathologically deep document above our 1024-level tripwire,
    // but keep it small enough that tree-sitter-xml reliably returns an AST
    // under full-suite parallel CI load.
    const DEPTH: usize = 1100;
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


#[test]
fn test_deep_xml_depth_496_survives_mcp_sized_stack() {
    let current_exe = std::env::current_exe().expect("resolve current test executable");
    let output = std::process::Command::new(current_exe)
        .args([
            "--exact",
            "definitions::tests_xml::deep_xml_depth_496_child_case",
            "--nocapture",
        ])
        .env("RUST_MIN_STACK", "1048576")
        .env("XRAY_DEEP_XML_CHILD", "1")
        .output()
        .expect("run isolated deep XML parser test");

    assert!(
        output.status.success(),
        "deep XML child failed with {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn deep_xml_depth_496_child_case() {
    if std::env::var_os("XRAY_DEEP_XML_CHILD").is_none() {
        return;
    }

    const DEPTH: usize = 496;
    let mut xml = String::from("<?xml version=\"1.0\"?>\r\n<Root>\r\n");
    for _ in 0..DEPTH {
        xml.push_str("<N>\r\n");
    }
    xml.push_str("<Leaf>deepest</Leaf>\r\n");
    for _ in 0..DEPTH {
        xml.push_str("</N>\r\n");
    }
    xml.push_str("</Root>\r\n");

    let result = parse_xml_on_demand_with_warnings(&xml, "deep-496.xml")
        .expect("valid deeply nested XML should parse");
    assert_eq!(result.definitions.len(), DEPTH + 2);
    let leaf = result
        .definitions
        .last()
        .expect("deepest leaf definition should be retained");
    assert_eq!(leaf.entry.name, "Leaf");
    assert_eq!(leaf.text_content.as_deref(), Some("deepest"));
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