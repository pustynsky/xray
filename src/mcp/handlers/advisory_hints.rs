//! Advisory hints surfaced by `xray_callers` and `xray_definitions` to flag
//! known blind spots of AST-based analysis.
//!
//! Background — `user-stories/xray-response-hints-for-incomplete-results.md`:
//! a real session showed an LLM agent silently building conclusions on an
//! incomplete `xray_callers` result (DI-injected interface call sites missed
//! because the `class=` filter selected the concrete class, not its
//! interface). The fix is not deeper type inference — it's surfacing
//! observable shape signals so the agent (or human) is prompted to
//! cross-check via grep before drawing conclusions.
//!
//! The module exposes three pure helpers:
//!
//! * [`interface_vias_caveat`] — when `xray_callers class=Foo` is used and
//!   `Foo` extends/implements interface(s), suggest re-running with the
//!   interface name.
//! * [`low_count_caveat`] — when caller count is small (1–3), suggest a
//!   `xray_grep ... countOnly=true` cross-check.
//! * [`value_source_hint`] — when a Property/Field carries an attribute
//!   with a string-literal argument, surface those literals as ready-to-grep
//!   keys for external config files.
//!
//! All helpers are deliberately **shape-based**, not name-based. They never
//! claim "this is DI" or "this is config" — only "if X, then try Y". This
//! avoids hard-coded framework lists that go stale per team / per language.

use crate::definitions::{DefinitionEntry, DefinitionIndex, DefinitionKind};

/// Caller-count threshold (inclusive) below which `low_count_caveat` fires.
/// Excludes 0 — the zero-results case is covered by the existing
/// `generate_callers_hint` (nearest-match) path.
pub(crate) const LOW_CALLER_THRESHOLD: usize = 3;

/// AI 1 — interface-receiver caveat for `xray_callers class=Foo`.
///
/// Returns a hint when the resolved class implements/extends one or more
/// interfaces (looked up via `name_index` + `kind == Interface`). The hint
/// names every detected interface and suggests the first as a re-run target.
///
/// Returns `None` when `class_filter` is unset, when the class is not
/// found in the index, when the matched definition is not a class-like
/// kind (Class / Struct / Record), or when no interfaces are found in
/// `base_types`.
///
/// Generic suffixes on base types are stripped (`IFoo<T>` → `IFoo`) before
/// the interface lookup. This matters for C# generic interfaces.
pub(crate) fn interface_vias_caveat(
    method_name: &str,
    class_filter: Option<&str>,
    def_idx: &DefinitionIndex,
) -> Option<String> {
    let cls = class_filter?;
    let cls_lower = cls.to_lowercase();
    let class_def_indices = def_idx.name_index.get(&cls_lower)?;

    let mut interfaces: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for &di in class_def_indices {
        let Some(def) = def_idx.definitions.get(di as usize) else {
            continue;
        };
        if !matches!(
            def.kind,
            DefinitionKind::Class | DefinitionKind::Struct | DefinitionKind::Record
        ) {
            continue;
        }
        for base in &def.base_types {
            // Strip generic suffix `IFoo<T>` → `IFoo` before lookup; tree-sitter
            // captures generics in base type text and we want the bare type name.
            let base_simple = base.split('<').next().unwrap_or(base).trim();
            if base_simple.is_empty() {
                continue;
            }
            let base_lower = base_simple.to_lowercase();
            if !seen.insert(base_lower.clone()) {
                continue;
            }
            let is_interface = def_idx
                .name_index
                .get(&base_lower)
                .map(|idxs| {
                    idxs.iter().any(|&i| {
                        def_idx
                            .definitions
                            .get(i as usize)
                            .is_some_and(|d| d.kind == DefinitionKind::Interface)
                    })
                })
                .unwrap_or(false);
            if is_interface {
                interfaces.push(base_simple.to_string());
            }
        }
    }

    if interfaces.is_empty() {
        return None;
    }

    let interface_list = interfaces
        .iter()
        .map(|s| format!("`{}`", s))
        .collect::<Vec<_>>()
        .join(", ");
    let suggested = interfaces[0].as_str();
    Some(format!(
        "Filtered by concrete class `{cls}`. Class implements interface(s): {interface_list}. \
         If `{method_name}` is invoked through a DI-injected interface field (e.g. `{cls}` \
         is registered as `{suggested}` in DI), those callsites are excluded by the `class=` \
         filter. Re-run without `class=` or with `class={suggested}` to include \
         interface-receiver callsites."
    ))
}

/// AI 2 — low caller-count cross-check hint.
///
/// Fires only for `1..=LOW_CALLER_THRESHOLD` callers (the zero case is
/// owned by the existing nearest-match hint). The hint is generic: it does
/// NOT classify the method as "DI-likely" or similar — it simply states the
/// asymmetry between AST-aware `xray_callers` and text-only `xray_grep`,
/// and gives a copy-paste-able command.
pub(crate) fn low_count_caveat(method_name: &str, caller_count: usize) -> Option<String> {
    if caller_count == 0 || caller_count > LOW_CALLER_THRESHOLD {
        return None;
    }
    let plural = if caller_count == 1 { "" } else { "s" };
    Some(format!(
        "Found {caller_count} caller{plural}. For type-aware-resilient cross-check, run \
         `xray_grep terms='{method_name}' countOnly=true`. If the grep count is much higher \
         than the caller count, some call sites likely use receivers that AST resolution \
         couldn't classify (DI-injected interfaces, dynamic dispatch, var/auto with \
         non-trivial inference)."
    ))
}

/// Extract every double-quoted string literal from each attribute text.
///
/// Used by [`value_source_hint`] (AI 3): attribute texts captured by
/// tree-sitter look like `ConfigurationProperty("foo")`,
/// `Display("Name", Order = 1)`, or `[FromKeyVault("secret-id")]` (square
/// brackets stripped by the parser for C# / TypeScript). The first
/// string-literal argument is almost always the binding key; later ones
/// (group / category / fallback) are kept too so the LLM sees every
/// candidate.
///
/// De-duplicates while preserving first-seen order. Skips empty literals
/// and gracefully ignores malformed (unterminated) attribute text. Backslash
/// escape is honored so `"a\"b"` is treated as a single literal.
pub(crate) fn extract_attribute_string_literals(attrs: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for attr in attrs {
        let bytes = attr.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'"' {
                i += 1;
                continue;
            }
            let start = i + 1;
            let mut j = start;
            let mut terminated = false;
            while j < bytes.len() {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                    continue;
                }
                if bytes[j] == b'"' {
                    terminated = true;
                    break;
                }
                j += 1;
            }
            if !terminated {
                break;
            }
            if j > start
                && let Ok(s) = std::str::from_utf8(&bytes[start..j])
                && !s.is_empty()
                && !out.iter().any(|x| x == s)
            {
                out.push(s.to_string());
            }
            i = j + 1;
        }
    }
    out
}

/// AI 3 — value-source hint for Property/Field carrying attribute(s) with
/// string-literal arguments.
///
/// Returns `None` for non-Property/Field kinds, for symbols without
/// attributes, or for symbols whose attributes carry no string literals.
/// Otherwise returns a hint listing the extracted keys and a concrete
/// `xray_grep` command targeting common config file extensions.
///
/// **Why no hard-coded framework list?** Every team rolls custom binders
/// (`[Knob]`, `[FromManifest]`, `[Tenanted]`...). A name allow-list goes
/// stale; a regex over names hits false positives like `[ConfigureAwait]`.
/// Shape-based detection (Property/Field + attribute with string literal)
/// covers all of these uniformly. The hint is framed conditionally
/// ("**if** any attribute binds…") so a non-binder attribute like
/// `[Display("Name")]` does not mislead.
pub(crate) fn value_source_hint(def: &DefinitionEntry) -> Option<String> {
    if !matches!(def.kind, DefinitionKind::Property | DefinitionKind::Field) {
        return None;
    }
    if def.attributes.is_empty() {
        return None;
    }
    let keys = extract_attribute_string_literals(&def.attributes);
    if keys.is_empty() {
        return None;
    }

    // Build a human-readable list of attribute *names* (text up to the
    // first '(' for `Foo("bar")`, or the whole text for bare `[Required]`).
    let attr_names: Vec<String> = def
        .attributes
        .iter()
        .filter_map(|a| {
            let name = a.split('(').next()?.trim();
            if name.is_empty() {
                None
            } else {
                Some(format!("`[{}]`", name))
            }
        })
        .collect();
    let attr_list = if attr_names.is_empty() {
        "attribute(s)".to_string()
    } else {
        attr_names.join(", ")
    };
    let keys_csv = keys.join(",");
    let keys_quoted = keys
        .iter()
        .map(|k| format!("`{}`", k))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "Property/field `{name}` carries {attr_list} with string-literal argument(s) \
         [{keys_quoted}]. If any attribute binds to external configuration (manifest, \
         appsettings, env vars, secret store), the runtime value lives outside source code. \
         To search external config files for these keys: \
         `xray_grep terms='{keys_csv}' ext='xml,json,config,yaml,manifestxml'`.",
        name = def.name
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn mk_def(
        name: &str,
        kind: DefinitionKind,
        base_types: Vec<&str>,
        attributes: Vec<&str>,
    ) -> DefinitionEntry {
        DefinitionEntry {
            file_id: 0,
            name: name.to_string(),
            kind,
            line_start: 1,
            line_end: 1,
            parent: None,
            signature: None,
            modifiers: vec![],
            attributes: attributes.into_iter().map(|s| s.to_string()).collect(),
            base_types: base_types.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    fn build_index(defs: Vec<DefinitionEntry>) -> DefinitionIndex {
        let mut idx = DefinitionIndex {
            root: ".".to_string(),
            extensions: vec!["cs".to_string()],
            files: vec!["f.cs".to_string()],
            ..Default::default()
        };
        for (i, d) in defs.into_iter().enumerate() {
            idx.name_index
                .entry(d.name.to_lowercase())
                .or_default()
                .push(i as u32);
            idx.kind_index.entry(d.kind).or_default().push(i as u32);
            idx.file_index.entry(d.file_id).or_default().push(i as u32);
            for base in &d.base_types {
                let base_simple = base.split('<').next().unwrap_or(base).trim();
                idx.base_type_index
                    .entry(base_simple.to_lowercase())
                    .or_default()
                    .push(i as u32);
            }
            idx.definitions.push(d);
        }
        idx
    }

    // ── interface_vias_caveat ───────────────────────────────────────────

    #[test]
    fn interface_vias_emits_for_class_implementing_interface() {
        let idx = build_index(vec![
            mk_def("IFoo", DefinitionKind::Interface, vec![], vec![]),
            mk_def("Foo", DefinitionKind::Class, vec!["IFoo"], vec![]),
        ]);
        let hint = interface_vias_caveat("Bar", Some("Foo"), &idx)
            .expect("expected interface-vias hint");
        assert!(hint.contains("`IFoo`"), "hint should name the interface, got: {hint}");
        assert!(hint.contains("class=IFoo"), "hint should suggest class=IFoo, got: {hint}");
        assert!(hint.contains("Bar"), "hint should reference the method name, got: {hint}");
    }

    #[test]
    fn interface_vias_strips_generic_suffix() {
        let idx = build_index(vec![
            mk_def("IRepository", DefinitionKind::Interface, vec![], vec![]),
            mk_def(
                "UserRepository",
                DefinitionKind::Class,
                vec!["IRepository<User>"],
                vec![],
            ),
        ]);
        let hint = interface_vias_caveat("Save", Some("UserRepository"), &idx)
            .expect("expected interface-vias hint after stripping generics");
        assert!(hint.contains("`IRepository`"), "hint must use bare type name, got: {hint}");
    }

    #[test]
    fn interface_vias_skips_base_class() {
        let idx = build_index(vec![
            mk_def("BaseClass", DefinitionKind::Class, vec![], vec![]),
            mk_def("Foo", DefinitionKind::Class, vec!["BaseClass"], vec![]),
        ]);
        assert!(
            interface_vias_caveat("Bar", Some("Foo"), &idx).is_none(),
            "should not fire when base type is a class, not an interface"
        );
    }

    #[test]
    fn interface_vias_returns_none_when_class_filter_unset() {
        let idx = build_index(vec![]);
        assert!(interface_vias_caveat("Bar", None, &idx).is_none());
    }

    #[test]
    fn interface_vias_returns_none_when_class_unknown() {
        let idx = build_index(vec![]);
        assert!(interface_vias_caveat("Bar", Some("Nope"), &idx).is_none());
    }

    #[test]
    fn interface_vias_returns_none_when_no_interfaces_in_base_types() {
        let idx = build_index(vec![mk_def("Foo", DefinitionKind::Class, vec![], vec![])]);
        assert!(interface_vias_caveat("Bar", Some("Foo"), &idx).is_none());
    }

    #[test]
    fn interface_vias_lists_multiple_interfaces() {
        let idx = build_index(vec![
            mk_def("IA", DefinitionKind::Interface, vec![], vec![]),
            mk_def("IB", DefinitionKind::Interface, vec![], vec![]),
            mk_def("Foo", DefinitionKind::Class, vec!["IA", "IB"], vec![]),
        ]);
        let hint = interface_vias_caveat("Bar", Some("Foo"), &idx).unwrap();
        assert!(hint.contains("`IA`"));
        assert!(hint.contains("`IB`"));
    }

    // ── low_count_caveat ───────────────────────────────────────────────

    #[test]
    fn low_count_fires_for_1() {
        let h = low_count_caveat("Foo", 1).expect("should fire for 1");
        assert!(h.contains("Found 1 caller."), "got: {h}");
        assert!(h.contains("xray_grep"));
    }

    #[test]
    fn low_count_fires_for_threshold() {
        assert!(low_count_caveat("Foo", LOW_CALLER_THRESHOLD).is_some());
    }

    #[test]
    fn low_count_does_not_fire_above_threshold() {
        assert!(low_count_caveat("Foo", LOW_CALLER_THRESHOLD + 1).is_none());
    }

    #[test]
    fn low_count_does_not_fire_for_zero() {
        // The zero case is owned by the nearest-match hint path.
        assert!(low_count_caveat("Foo", 0).is_none());
    }

    #[test]
    fn low_count_pluralizes_correctly() {
        assert!(low_count_caveat("X", 1).unwrap().contains("Found 1 caller."));
        assert!(low_count_caveat("X", 2).unwrap().contains("Found 2 callers."));
        assert!(low_count_caveat("X", 3).unwrap().contains("Found 3 callers."));
    }

    // ── extract_attribute_string_literals ──────────────────────────────

    #[test]
    fn extract_literals_basic() {
        let attrs = vec!["ConfigurationProperty(\"foo\")".to_string()];
        assert_eq!(extract_attribute_string_literals(&attrs), vec!["foo"]);
    }

    #[test]
    fn extract_literals_multiple_args() {
        let attrs = vec!["Display(\"Name\", \"Group\", Order = 1)".to_string()];
        assert_eq!(
            extract_attribute_string_literals(&attrs),
            vec!["Name", "Group"]
        );
    }

    #[test]
    fn extract_literals_no_args() {
        let attrs = vec!["Required".to_string()];
        assert_eq!(extract_attribute_string_literals(&attrs), Vec::<String>::new());
    }

    #[test]
    fn extract_literals_dedup_across_attrs() {
        let attrs = vec![
            "Foo(\"k\")".to_string(),
            "Bar(\"k\")".to_string(),
            "Baz(\"q\")".to_string(),
        ];
        assert_eq!(extract_attribute_string_literals(&attrs), vec!["k", "q"]);
    }

    #[test]
    fn extract_literals_handles_escaped_quote() {
        let attrs = vec!["Foo(\"a\\\"b\")".to_string()];
        // a\"b — escape preserved, single literal, single key extracted.
        let got = extract_attribute_string_literals(&attrs);
        assert_eq!(got.len(), 1, "got: {:?}", got);
        assert!(got[0].contains('a'));
    }

    #[test]
    fn extract_literals_skips_empty_string() {
        let attrs = vec!["Foo(\"\")".to_string()];
        assert!(extract_attribute_string_literals(&attrs).is_empty());
    }

    #[test]
    fn extract_literals_handles_unterminated_gracefully() {
        let attrs = vec!["Foo(\"unterminated".to_string()];
        assert!(extract_attribute_string_literals(&attrs).is_empty());
    }

    // ── value_source_hint ──────────────────────────────────────────────

    #[test]
    fn value_source_hint_fires_for_property_with_attribute_literal() {
        let def = mk_def(
            "DefaultIndexName",
            DefinitionKind::Property,
            vec![],
            vec!["ConfigurationProperty(\"DefaultIndexName\")"],
        );
        let h = value_source_hint(&def).expect("expected hint");
        assert!(h.contains("`DefaultIndexName`"), "got: {h}");
        assert!(h.contains("`[ConfigurationProperty]`"), "got: {h}");
        assert!(h.contains("xray_grep"), "got: {h}");
        assert!(h.contains("ext='xml,json,config,yaml,manifestxml'"), "got: {h}");
    }

    #[test]
    fn value_source_hint_fires_for_field() {
        let def = mk_def(
            "ApiKey",
            DefinitionKind::Field,
            vec![],
            vec!["FromKeyVault(\"prod-api-key\")"],
        );
        assert!(value_source_hint(&def).is_some());
    }

    #[test]
    fn value_source_hint_skips_method() {
        let def = mk_def(
            "Get",
            DefinitionKind::Method,
            vec![],
            vec!["HttpGet(\"/api\")"],
        );
        assert!(value_source_hint(&def).is_none());
    }

    #[test]
    fn value_source_hint_skips_property_without_attributes() {
        let def = mk_def("Plain", DefinitionKind::Property, vec![], vec![]);
        assert!(value_source_hint(&def).is_none());
    }

    #[test]
    fn value_source_hint_skips_property_with_attribute_but_no_literal() {
        // [Required] — bare attribute, no string-literal arg → no hint.
        let def = mk_def("Plain", DefinitionKind::Property, vec![], vec!["Required"]);
        assert!(value_source_hint(&def).is_none());
    }

    #[test]
    fn value_source_hint_lists_multiple_attrs_and_keys() {
        let def = mk_def(
            "Multi",
            DefinitionKind::Property,
            vec![],
            vec![
                "ConfigurationProperty(\"primary\")",
                "Fallback(\"secondary\")",
            ],
        );
        let h = value_source_hint(&def).unwrap();
        assert!(h.contains("`[ConfigurationProperty]`"), "got: {h}");
        assert!(h.contains("`[Fallback]`"), "got: {h}");
        assert!(h.contains("primary,secondary"), "csv keys for grep, got: {h}");
    }

    #[test]
    fn value_source_hint_safe_for_display_attribute() {
        // [Display("Name")] — false-positive candidate. Hint still fires
        // because shape matches, but the framing "if any attribute binds…"
        // makes this honest, not misleading. Verify the hint does not
        // assert that this IS a config binding.
        let def = mk_def(
            "FullName",
            DefinitionKind::Property,
            vec![],
            vec!["Display(\"Name\")"],
        );
        let h = value_source_hint(&def).unwrap();
        assert!(
            h.contains("If any attribute binds"),
            "framing must be conditional, got: {h}"
        );
    }

    /// Smoke test: build_index helper round-trips correctly so other tests
    /// can rely on it.
    #[test]
    fn build_index_helper_is_consistent() {
        let idx = build_index(vec![
            mk_def("A", DefinitionKind::Class, vec!["IB"], vec![]),
            mk_def("IB", DefinitionKind::Interface, vec![], vec![]),
        ]);
        assert_eq!(idx.definitions.len(), 2);
        assert!(idx.name_index.contains_key("a"));
        assert!(idx.name_index.contains_key("ib"));
        assert!(idx.base_type_index.contains_key("ib"));
        let _ = HashMap::<u32, u32>::new(); // touch HashMap import
    }
}
