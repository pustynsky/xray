use serde_json::{json, Value};
use crate::mcp::protocol::ToolCallResult;

const COLLECTIONS: &[&str] = &["definitions", "containingDefinitions", "callTree", "callTreeNodes", "results", "callers", "callees", "children"];

pub(crate) fn transport_limit() -> Result<Option<usize>, ToolCallResult> {
    #[cfg(test)]
    if let Some(limit) = TRANSPORT_LIMIT.with(|value| value.get()) {
        return Ok(Some(limit));
    }
    parse_transport_limit(std::env::var("XRAY_TRANSPORT_MAX_BYTES"))
}

fn parse_transport_limit(value: Result<String, std::env::VarError>) -> Result<Option<usize>, ToolCallResult> {
    let message = match value {
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Ok(value) => {
            if let Some(limit) = value.parse::<usize>().ok().filter(|limit| *limit >= 512) {
                return Ok(Some(limit));
            }
            "XRAY_TRANSPORT_MAX_BYTES must be an integer of at least 512 bytes"
        }
        Err(std::env::VarError::NotUnicode(_)) => "XRAY_TRANSPORT_MAX_BYTES is not valid Unicode",
    };
    Err(ToolCallResult::error(super::utils::json_to_string(&json!({
        "error": {"code": "invalid_transport_max_bytes", "parameter": "XRAY_TRANSPORT_MAX_BYTES", "message": message},
        "resultStatus": {"status": "error", "complete": false, "safeForExhaustiveClaims": false}
    }))))
}

#[cfg(test)]
thread_local! {
    static TRANSPORT_LIMIT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_transport_limit<T>(limit: usize, action: impl FnOnce() -> T) -> T {
    struct Reset(Option<usize>);
    impl Drop for Reset {
        fn drop(&mut self) { TRANSPORT_LIMIT.with(|value| value.set(self.0)); }
    }
    let _reset = Reset(TRANSPORT_LIMIT.with(|value| value.replace(Some(limit))));
    action()
}

pub(crate) fn budget_error() -> ToolCallResult {
    ToolCallResult::error(super::utils::json_to_string(&json!({
        "error": {"code": "single_item_exceeds_response_budget", "message": "A useful fragment and its continuation cannot fit the response budget. Narrow the request or increase the transport limit."},
        "resultStatus": {"status": "error", "complete": false, "safeForExhaustiveClaims": false}
    })))
}

fn visit_entries(value: &mut Value, action: &mut impl FnMut(&mut Value)) {
    action(value);
    for key in COLLECTIONS {
        if let Some(items) = value.get_mut(*key).and_then(Value::as_array_mut) {
            for item in items { visit_entries(item, action); }
        }
    }
    if let Some(root) = value.get_mut("rootMethod") { visit_entries(root, action); }
}

pub(crate) fn prepare(result: ToolCallResult, tool: &str, args: &Value) -> ToolCallResult {
    let Some(text) = result.content.first().map(|content| &content.text) else { return result; };
    let Ok(mut output) = serde_json::from_str::<Value>(text) else { return result; };
    let known_arguments = super::arg_validation::check_unknown_args(tool, args).is_none();
    if let Some(page) = output.pointer_mut("/resultStatus/page").and_then(Value::as_object_mut)
        && known_arguments
    {
        let mut query = args.clone();
        if let Some(query) = query.as_object_mut() {
            query.remove("offset");
            query.remove("continuationToken");
        }
        page.insert("queryArgs".to_string(), query);
    }
    if !known_arguments {
        output["continuationUnavailable"] = json!("Correct unknown arguments and restart the query before requesting generated continuation arguments.");
    }
    visit_entries(&mut output, &mut |entry| {
        if !known_arguments || entry.get("bodySourceHash").is_none() { return; }
        let Some(file) = entry.get("bodyFile").and_then(Value::as_str) else { return; };
        let Some(name) = entry.get("name").or_else(|| entry.get("method")).and_then(Value::as_str) else { return; };
        let mut query = if tool == "xray_definitions" { args.clone() } else { json!({}) };
        if tool != "xray_definitions" {
            let unsupported = ["excludeFile", "ext", "productionOnly"].iter().any(|key| {
                args.get(*key).is_some_and(|value| value != &Value::Bool(false) && value != &json!([]))
            });
            if unsupported {
                entry["bodyContinuationUnavailable"] = json!("The source query has restrictions not expressible by xray_definitions; no equivalent read query was generated.");
                return;
            }
            for key in ["excludeDir", "maxBodyLines", "maxTotalBodyLines", "includeDocComments"] {
                if let Some(value) = args.get(key) { query[key] = value.clone(); }
            }
        }
        let Some(object) = query.as_object_mut() else { return; };
        for key in ["containsLine", "offset", "continuationToken", "audit", "crossValidate", "regex", "bodyLineStart", "bodyLineEnd", "bodyTarget"] {
            object.remove(key);
        }
        query["file"] = json!([file]);
        query["name"] = json!([name]);
        query["exactNameOnly"] = json!(true);
        query["autoCorrect"] = json!(false);
        query["includeBody"] = json!(true);
        if let Some(kind) = entry.get("kind").and_then(Value::as_str) { query["kind"] = json!([kind]); }
        if let Some(parent) = entry.get("parent").and_then(Value::as_str) { query["parent"] = json!([parent]); }
        query["bodyTarget"] = json!({
            "sourceHash": entry["bodySourceHash"],
            "startLine": entry["bodyDefinitionStartLine"],
            "endLine": entry["bodyDefinitionEndLine"],
        });
        entry["bodyRead"] = json!({"tool": "xray_definitions", "args": query});
        refresh_continuation(entry);
    });
    refresh_page_args(&mut output);
    refresh_accounting(&mut output);
    if has_body_reads(&output) {
        output.as_object_mut().unwrap().remove("recommendedNextQueries");
    }
    ToolCallResult { is_error: result.is_error, ..ToolCallResult::success(super::utils::json_to_string(&output)) }
}

pub(crate) fn refresh_page_args(output: &mut Value) {
    let Some(page) = output.pointer_mut("/resultStatus/page").and_then(Value::as_object_mut) else { return; };
    let Some(mut query) = page.get("queryArgs").or_else(|| page.get("nextArgs")).cloned() else { return; };
    if let Some(object) = query.as_object_mut() { object.remove("continuationToken"); }
    page.remove("nextArgs");
    page.remove("queryArgs");
    if let Some(token) = page.get("continuationToken").cloned() {
        query["continuationToken"] = token;
        page.insert("nextArgs".to_string(), query);
    } else {
        page.insert("queryArgs".to_string(), query);
    }
}

fn refresh_continuation(entry: &mut Value) {
    let Some(read) = entry.get("bodyRead").cloned() else { return; };
    let start = entry["bodyStartLine"].as_u64().unwrap_or(1);
    let end = entry["bodyEndLine"].as_u64().unwrap_or(start.saturating_sub(1));
    let requested_start = entry["bodyRequestedStartLine"].as_u64().unwrap_or(start);
    let requested_end = entry["bodyRequestedEndLine"].as_u64().unwrap_or(end);
    entry.as_object_mut().unwrap().remove("bodyContinuation");
    let mut continuation = json!({"tool": "xray_definitions"});
    let mut missing = false;
    for (key, lower, upper) in [("beforeArgs", requested_start, start.saturating_sub(1)), ("nextArgs", end.saturating_add(1).max(requested_start), requested_end)] {
        if lower <= upper {
            let mut next = read["args"].clone();
            next["bodyLineStart"] = json!(lower);
            next["bodyLineEnd"] = json!(upper);
            continuation[key] = next;
            missing = true;
        }
    }
    if missing { entry["bodyContinuation"] = continuation; }
}

pub(crate) fn has_body_reads(output: &Value) -> bool {
    if output.get("bodySourceHash").is_some() { return true; }
    COLLECTIONS.iter().any(|key| output.get(*key).and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(has_body_reads)))
        || output.get("rootMethod").is_some_and(has_body_reads)
}

pub(crate) fn refresh_accounting(output: &mut Value) {
    let mut total = 0usize;
    let mut partial = false;
    let mut found = false;
    visit_entries(output, &mut |entry| {
        if let Some(body) = entry.get("body").and_then(Value::as_array) {
            total += body.len();
            found = true;
            partial |= entry["bodyComplete"] == false;
        }
    });
    if !found { return; }
    if let Some(summary) = output.get_mut("summary") {
        summary["totalBodyLinesReturned"] = json!(total);
    }
    if let Some(status) = output.get_mut("resultStatus").and_then(Value::as_object_mut) {
        if let Some(shown) = status.get_mut("shown") { shown["bodyLines"] = json!(total); }
        if let Some(available) = status.get("total").and_then(|total| total.get("bodyLines")).and_then(Value::as_u64)
            && let Some(omitted) = status.get_mut("omitted")
        {
            omitted["bodyLines"] = json!(available.saturating_sub(total as u64));
        }
        if partial {
            status.insert("complete".to_string(), json!(false));
            status.insert("safeForExhaustiveClaims".to_string(), json!(false));
            status.insert("safeForExactSemantics".to_string(), json!(false));
            status.insert("status".to_string(), json!("partial"));
            status.insert("evidenceLevel".to_string(), json!("truncated_body"));
        }
    }
}

pub(crate) fn fit_body_fragments(output: &mut Value, max_bytes: usize) {
    loop {
        if delivered_size(output) <= max_bytes { break; }
        let mut selected = None;
        let mut position = 0;
        let mut largest = 0;
        visit_entries(output, &mut |entry| {
            if entry.get("bodySourceHash").is_some()
                && let Some(body) = entry.get("body").and_then(Value::as_array)
                && body.len() > 1
            {
                let bytes: usize = body.iter().filter_map(Value::as_str).map(str::len).sum();
                if selected.is_none() || bytes > largest {
                    selected = Some((position, body.len()));
                    largest = bytes;
                }
            }
            position += 1;
        });
        let Some((selected, count)) = selected else { break; };
        let mut original = Value::Null;
        let mut minimum_size = 0;
        position = 0;
        visit_entries(output, &mut |entry| {
            if position == selected {
                original = entry.clone();
                crop_body(entry, 1);
                minimum_size = delivered_size(entry);
            }
            position += 1;
        });
        refresh_accounting(output);
        let other_bytes = delivered_size(output).saturating_sub(minimum_size);
        let mut low = 2;
        let mut high = count - 1;
        let mut best = None;
        while low <= high {
            let keep = low + (high - low) / 2;
            let mut candidate = original.clone();
            crop_body(&mut candidate, keep);
            if other_bytes + delivered_size(&candidate) <= max_bytes {
                best = Some(candidate);
                low = keep + 1;
            } else { high = keep - 1; }
        }
        if let Some(candidate) = best {
            position = 0;
            let mut candidate = Some(candidate);
            visit_entries(output, &mut |entry| {
                if position == selected { *entry = candidate.take().unwrap(); }
                position += 1;
            });
            refresh_accounting(output);
        }
    }
}

fn crop_body(entry: &mut Value, keep: usize) {
    let start = entry["bodyStartLine"].as_u64().unwrap_or(1);
    let count = entry["body"].as_array().map(Vec::len).unwrap_or(0);
    debug_assert!(keep > 0 && keep <= count);
    let anchor = entry["bodyAnchorLine"].as_u64();
    let offset = anchor.filter(|anchor| entry["bodyAnchorVisible"] == true && *anchor >= start)
        .map(|anchor| ((anchor - start) as usize).saturating_sub((keep - 1) / 2).min(count - keep))
        .unwrap_or(0);
    if let Some(body) = entry.get_mut("body").and_then(Value::as_array_mut) {
        body.drain(..offset);
        body.truncate(keep);
    }
    entry["bodyStartLine"] = json!(start + offset as u64);
    super::utils::refresh_body_metadata(entry);
    refresh_continuation(entry);
}


pub(crate) fn delivered_size(output: &Value) -> usize {
    if !has_body_reads(output) && output.pointer("/resultStatus/page/queryArgs").is_none() {
        return super::utils::json_to_string(output).len();
    }
    let mut delivered = output.clone();
    finalize_metadata(&mut delivered);
    super::utils::json_to_string(&delivered).len()
}

pub(crate) fn finalize_metadata(output: &mut Value) {
    visit_entries(output, &mut |entry| {
        if let Some(object) = entry.as_object_mut() {
            for key in ["bodyRead", "bodyFile", "bodySourceHash", "bodyDefinitionStartLine",
                "bodyDefinitionEndLine", "bodyAvailableStartLine", "bodyRequestedStartLine",
                "bodyRequestedEndLine", "bodyExplicitRange"]
            {
                object.remove(key);
            }
        }
        if let Some(page) = entry.pointer_mut("/resultStatus/page").and_then(Value::as_object_mut) {
            page.remove("queryArgs");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, RwLock};
    use crate::definitions::{DefinitionEntry, DefinitionIndex, DefinitionKind};
    use super::super::{dispatch_tool, HandlerContext, WorkspaceBinding};

    fn fixture(methods: u32, lines_per_method: u32, width: usize) -> (tempfile::TempDir, HandlerContext, String, Vec<String>) {
        let temp = tempfile::tempdir().unwrap();
        let root = crate::canonicalize_test_root(temp.path());
        let path = root.join("sample.rs");
        let lines: Vec<String> = (1..=methods * lines_per_method)
            .map(|line| format!("// line {line}: {}", "\u{03bb}\"\\".repeat(width))).collect();
        std::fs::write(&path, lines.join("\n")).unwrap();
        let path = crate::clean_path(&path.to_string_lossy());
        let mut index = DefinitionIndex {
            root: crate::clean_path(&root.to_string_lossy()),
            files: vec![path.clone()],
            extensions: vec!["rs".to_string()],
            ..Default::default()
        };
        for method in 0..methods {
            index.definitions.push(DefinitionEntry {
                name: "Compute".to_string(), kind: DefinitionKind::Function, file_id: 0,
                line_start: method * lines_per_method + 1,
                line_end: (method + 1) * lines_per_method,
                signature: Some("fn Compute()".to_string()), parent: None,
                modifiers: vec![], attributes: vec![], base_types: vec![],
            });
            index.name_index.entry("compute".to_string()).or_default().push(method);
            index.kind_index.entry(DefinitionKind::Function).or_default().push(method);
            index.file_index.entry(0).or_default().push(method);
        }
        index.path_to_id.insert(crate::path_identity_key(std::path::Path::new(&path)), 0);
        let ctx = HandlerContext {
            def_index: Some(Arc::new(RwLock::new(index))),
            server_ext: "rs".to_string(), def_extensions: vec!["rs".to_string()],
            workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(crate::clean_path(&root.to_string_lossy())))),
            ..Default::default()
        };
        (temp, ctx, path, lines)
    }

    #[test]
    fn executable_hints_select_overload_and_preserve_limits() {
        let (_temp, ctx, path, _) = fixture(2, 50, 4);
        let request = json!({
            "file": [path], "name": ["Comp", "Absent"], "kind": ["function"],
            "excludeDir": ["excluded"], "maxBodyLines": 3, "maxTotalBodyLines": 7,
            "bodyLineStart": 55, "bodyLineEnd": 65, "includeUsageCount": true,
        });
        let result = dispatch_tool(&ctx, "xray_definitions", &request);
        assert!(!result.is_error, "{}", result.content[0].text);
        let output = payload(&result, 16384);
        let hints = output["recommendedNextQueries"].as_array().unwrap();
        assert_eq!(hints.len(), 2);
        for hint in hints {
            super::super::arg_validation::assert_generated_query("xray_definitions", &hint["args"]);
        }
        let next = &hints[1]["args"];
        let schema = super::super::arg_validation::test_tool_schema("xray_definitions");
        for key in ["file", "name", "kind"] {
            let mut wrong = next.clone();
            wrong[key] = wrong[key][0].clone();
            assert!(!schema.is_valid(&wrong), "scalar {key} was accepted");
        }
        let mut wrong = next.clone();
        wrong["bodyTarget"]["startLine"] = json!("51");
        assert!(!schema.is_valid(&wrong));
        assert_eq!(next["name"], json!(["Compute"]));
        assert_eq!(next["file"], request["file"]);
        assert_eq!(next["exactNameOnly"], true);
        assert_eq!(next["autoCorrect"], false);
        for key in ["kind", "excludeDir", "maxBodyLines", "maxTotalBodyLines", "bodyLineStart", "bodyLineEnd", "includeUsageCount"] {
            assert_eq!(next[key], request[key], "{key}");
        }
        let read = dispatch_tool(&ctx, "xray_definitions", next);
        assert!(!read.is_error, "{}", read.content[0].text);
        let read = payload(&read, 16384);
        assert_eq!(read["definitions"].as_array().unwrap().len(), 1);
        assert_eq!(read["definitions"][0]["lines"], "51-100");
        assert_eq!(read["definitions"][0]["bodyStartLine"], 55);
        assert_eq!(read["definitions"][0]["body"].as_array().unwrap().len(), 3);
        assert!(read.get("recommendedNextQueries").is_none());
        assert!(read["definitions"][0].get("bodyContinuation").is_some());
        super::super::arg_validation::assert_generated_query("xray_definitions", &read["definitions"][0]["bodyContinuation"]["nextArgs"]);
    }

    #[test]
    fn executable_hints_preserve_regex_discovery_and_all_definition_filters() {
        let (_temp, ctx, path, _) = fixture(1, 30, 3);
        {
            let mut index = ctx.def_index.as_ref().unwrap().write().unwrap();
            index.definitions[0].parent = Some("Container".to_string());
            index.definitions[0].attributes = vec!["Marker".to_string()];
            index.definitions[0].base_types = vec!["Base".to_string()];
            index.attribute_index.insert("marker".to_string(), vec![0]);
            index.base_type_index.insert("base".to_string(), vec![0]);
            index.code_stats.insert(0, crate::definitions::CodeStats {
                cyclomatic_complexity: 5, cognitive_complexity: 5, max_nesting_depth: 3,
                param_count: 3, return_count: 3, call_count: 5, lambda_count: 1,
            });
        }
        for names in [json!(["^Comp.*$"]), json!(["Comp", "Absent"])] {
            let regex = names[0].as_str().unwrap().starts_with('^');
            let request = json!({
                "file": [path, "missing.rs"], "name": names, "regex": regex,
                "kind": ["function", "method"], "parent": ["Container", "Other"],
                "attribute": "Marker", "baseType": "Base", "baseTypeTransitive": true,
                "excludeDir": ["excluded"], "minComplexity": 2, "minCognitive": 2,
                "minNesting": 2, "paramCount": 3, "minParams": 2, "minReturns": 2, "minCalls": 2,
                "sortBy": "cognitiveComplexity", "includeCodeStats": true, "includeUsageCount": true,
                "bodyLineStart": 5, "bodyLineEnd": 25, "includeDocComments": false,
                "maxBodyLines": 4, "maxTotalBodyLines": 6, "maxResults": 2,
                "autoCorrect": false,
            });
            let found = dispatch_tool(&ctx, "xray_definitions", &request);
            assert!(!found.is_error, "{}", found.content[0].text);
            let found = payload(&found, 16384);
            assert_eq!(found["definitions"].as_array().unwrap().len(), 1, "{found}");
            assert_eq!(found["resultStatus"]["request"]["exactNameOnly"], false);
            let next = &found["recommendedNextQueries"][0]["args"];
            super::super::arg_validation::assert_generated_query("xray_definitions", next);
            assert_eq!(next["file"], json!([path]));
            assert_eq!(next["name"], json!(["Compute"]));
            assert_eq!(next["parent"], json!(["Container"]));
            assert_eq!(next["kind"], json!(["function"]));
            assert!(next.get("regex").is_none());
            for key in ["attribute", "baseType", "baseTypeTransitive", "excludeDir", "minComplexity",
                "minCognitive", "minNesting", "paramCount", "minParams", "minReturns", "minCalls", "sortBy", "includeCodeStats",
                "includeUsageCount", "bodyLineStart", "bodyLineEnd", "includeDocComments", "maxBodyLines", "maxTotalBodyLines", "maxResults"] {
                assert_eq!(next[key], request[key], "{key}");
            }
            let read = dispatch_tool(&ctx, "xray_definitions", next);
            assert!(!read.is_error, "{}", read.content[0].text);
            let read = payload(&read, 16384);
            assert_eq!(read["definitions"].as_array().unwrap().len(), 1);
            assert_eq!(read["definitions"][0]["parent"], "Container");
            assert_eq!(read["definitions"][0]["bodyStartLine"], 5);
            assert_eq!(read["definitions"][0]["body"].as_array().unwrap().len(), 4);
            let mut with_docs = next.clone();
            with_docs["includeDocComments"] = json!(true);
            let read = dispatch_tool(&ctx, "xray_definitions", &with_docs);
            assert!(!read.is_error, "{}", read.content[0].text);
            let read = payload(&read, 16384);
            assert_eq!(read["definitions"][0]["bodyContinuation"]["nextArgs"]["includeDocComments"], true);
        }
    }

    #[test]
    fn executable_hints_duplicate_basenames_and_ambiguous_ranges() {
        let (_temp, ctx, path, _) = fixture(1, 10, 1);
        let nested = std::path::Path::new(&path).parent().unwrap().join("nested").join("sample.rs");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(&nested, "// nested\n".repeat(10)).unwrap();
        let nested = crate::clean_path(&nested.to_string_lossy());
        {
            let mut index = ctx.def_index.as_ref().unwrap().write().unwrap();
            index.files.push(nested.clone());
            let mut duplicate = index.definitions[0].clone();
            duplicate.file_id = 1;
            index.definitions.push(duplicate);
            index.name_index.get_mut("compute").unwrap().push(1);
            index.kind_index.get_mut(&DefinitionKind::Function).unwrap().push(1);
            index.file_index.insert(1, vec![1]);
            index.path_to_id.insert(crate::path_identity_key(std::path::Path::new(&nested)), 1);
        }
        let request = json!({"file": ["sample.rs"], "name": ["Comp"]});
        let result = dispatch_tool(&ctx, "xray_definitions", &request);
        assert!(!result.is_error);
        let output = payload(&result, 16384);
        let hints = output["recommendedNextQueries"].as_array().unwrap();
        assert_eq!(hints.len(), 2);
        for hint in hints {
            let args = &hint["args"];
            super::super::arg_validation::assert_generated_query("xray_definitions", args);
            assert!(args.get("maxBodyLines").is_none());
            let read = dispatch_tool(&ctx, "xray_definitions", args);
            assert!(!read.is_error, "{}", read.content[0].text);
            let read = payload(&read, 16384);
            assert_eq!(read["definitions"].as_array().unwrap().len(), 1);
            assert_eq!(read["definitions"][0]["file"], args["file"][0]);
        }
        {
            let mut index = ctx.def_index.as_ref().unwrap().write().unwrap();
            let mut duplicate = index.definitions[0].clone();
            duplicate.parent = Some("Nested".to_string());
            index.definitions.push(duplicate);
            index.name_index.get_mut("compute").unwrap().push(2);
            index.kind_index.get_mut(&DefinitionKind::Function).unwrap().push(2);
            index.file_index.get_mut(&0).unwrap().push(2);
        }
        let result = dispatch_tool(&ctx, "xray_definitions", &json!({"file": [path], "name": ["Comp"]}));
        let output = payload(&result, 16384);
        assert!(output.get("recommendedNextQueries").is_none());
        assert!(output["nextQueryUnavailable"].as_str().unwrap().contains("ambiguous"));
        let summary = dispatch_tool(&ctx, "xray_definitions", &json!({"file": [path], "maxResults": 1}));
        let summary = payload(&summary, 16384);
        assert!(summary.get("autoSummary").is_some());
        assert!(summary.get("recommendedNextQueries").is_none());
    }

    #[test]
    fn executable_hints_transport_prefix_metrics_and_bound() {
        struct PrefixReset(Option<bool>);
        impl Drop for PrefixReset {
            fn drop(&mut self) { super::super::utils::set_guidance_prefix_override_for_test(self.0); }
        }
        for prefix in [false, true] {
            let _reset = PrefixReset(super::super::utils::set_guidance_prefix_override_for_test(Some(prefix)));
            for metrics in [false, true] {
                for limit in [4096, 8192, 16384] {
                    let (_temp, mut ctx, path, _) = fixture(5, 20, 8);
                    ctx.metrics = metrics;
                    with_transport_limit(limit, || {
                        let result = dispatch_tool(&ctx, "xray_definitions", &json!({
                            "file": [path], "name": ["Comp"], "excludeDir": ["excluded"], "maxBodyLines": 3,
                        }));
                        assert!(!result.is_error, "{}", result.content[0].text);
                        let output = payload(&result, limit);
                        if metrics {
                            assert_eq!(output["summary"]["responseBytes"].as_u64().unwrap() as usize, result.content[0].text.len());
                        }
                        let hints = output.get("recommendedNextQueries").and_then(Value::as_array);
                        if limit >= 8192 { assert!(hints.is_some(), "{output}"); }
                        if let Some(hints) = hints {
                            assert!(hints.len() <= 3);
                            for hint in hints {
                                let args = &hint["args"];
                                super::super::arg_validation::assert_generated_query("xray_definitions", args);
                                let read = dispatch_tool(&ctx, "xray_definitions", args);
                                assert!(!read.is_error, "{}", read.content[0].text);
                                let read = payload(&read, limit);
                                assert_eq!(read["definitions"].as_array().unwrap().len(), 1);
                                assert_eq!(read["definitions"][0]["lines"], format!("{}-{}", args["bodyTarget"]["startLine"], args["bodyTarget"]["endLine"]));
                            }
                        }
                    });
                }
            }
        }
    }

    #[test]
    fn executable_hints_cross_tool_selected_file_dispatch() {
        let (_temp, mut ctx, path, _) = fixture(1, 10, 1);
        let root = std::path::Path::new(&path).parent().unwrap();
        ctx.index_base = root.join(".index");
        let content = crate::build_content_index(&crate::ContentIndexArgs {
            dir: root.to_string_lossy().to_string(), ext: "rs".to_string(), threads: 1, ..Default::default()
        }).unwrap();
        ctx.index = Arc::new(RwLock::new(content));
        for (tool, args) in [
            ("xray_grep", json!({"terms": ["line"], "dir": path, "excludeDir": ["excluded"]})),
            ("xray_fast", json!({"pattern": ["sample.rs"], "dir": root.to_string_lossy(), "ext": ["rs"]})),
        ] {
            let result = dispatch_tool(&ctx, tool, &args);
            assert!(!result.is_error, "{}", result.content[0].text);
            let output = payload(&result, 16384);
            let next = &output["summary"]["nextStepQuery"];
            assert_eq!(next["args"]["file"], json!([path]), "{output}");
            assert_eq!(next["semanticsChanged"], true);
            super::super::arg_validation::assert_generated_query("xray_definitions", &next["args"]);
            let found = dispatch_tool(&ctx, "xray_definitions", &next["args"]);
            assert!(!found.is_error, "{}", found.content[0].text);
            let found = payload(&found, 16384);
            let read_args = &found["recommendedNextQueries"][0]["args"];
            super::super::arg_validation::assert_generated_query("xray_definitions", read_args);
            let read = dispatch_tool(&ctx, "xray_definitions", read_args);
            assert!(!read.is_error, "{}", read.content[0].text);
            let read = payload(&read, 16384);
            assert_eq!(read["definitions"].as_array().unwrap().len(), 1);
            assert_eq!(read["definitions"][0]["name"], "Compute");
            assert_eq!(read["definitions"][0]["file"], path);
        }
    }

    #[test]
    fn executable_hints_caller_read_scope_drops_root_page_limit() {
        let expected = json!({
            "excludeDir": ["excluded"], "maxBodyLines": 12, "maxTotalBodyLines": 24,
            "includeDocComments": true, "bodyLineStart": 2, "bodyLineEnd": 10,
        });
        for root_limit in [0, 1, 100] {
            let mut original = expected.clone();
            original["method"] = json!(["Compute"]);
            original["maxResults"] = json!(root_limit);
            let scope = super::super::definitions::caller_read_scope(&original).unwrap();
            assert_eq!(scope, expected);
            assert!(scope.get("maxResults").is_none());
        }
    }

    #[test]
    fn executable_hints_metadata_source_limit_preserves_results() {
        let (_temp, ctx, path, _) = fixture(1, 1, 1);
        let limit = super::super::definitions::MAX_HINT_SOURCE_BYTES;
        assert_eq!(limit, 1_048_576);
        let request = json!({"file": [path], "name": ["Compute"]});
        let expected = payload(&dispatch_tool(&ctx, "xray_definitions", &request), 16384)["definitions"].clone();
        for size in [limit - 1, limit, limit + 1, 5 * limit] {
            let mut bytes = vec![b'x'; size];
            bytes[1] = b'\n';
            std::fs::write(&path, bytes).unwrap();
            let result = dispatch_tool(&ctx, "xray_definitions", &request);
            assert!(!result.is_error, "{}", result.content[0].text);
            let output = payload(&result, 16384);
            assert_eq!(output["definitions"], expected, "size={size}");
            assert_eq!(output["summary"]["returned"], 1);
            if size <= limit {
                assert_eq!(output["recommendedNextQueries"].as_array().unwrap().len(), 1);
                assert!(output.get("nextQueryUnavailable").is_none());
            } else {
                assert!(output.get("recommendedNextQueries").is_none());
                let reason = output["nextQueryUnavailable"].as_str().unwrap();
                assert!(reason.contains("1 MiB (1048576 bytes) optional hint source limit"), "{reason}");
            }
            let mut read_args = request.clone();
            read_args["includeBody"] = json!(true);
            let read = dispatch_tool(&ctx, "xray_definitions", &read_args);
            assert!(!read.is_error, "{}", read.content[0].text);
            assert_eq!(payload(&read, 16384)["definitions"][0]["body"], json!(["x"]));
        }
    }

    #[test]
    fn executable_hints_metadata_source_limit_reuses_file_cache() {
        let (_temp, ctx, path, _) = fixture(2, 1, 1);
        let index = ctx.def_index.as_ref().unwrap().read().unwrap();
        let mut cache = std::collections::HashMap::new();
        let first = super::super::definitions::selected_definition_read(
            &json!({}), &index, &index.definitions[0], &mut cache,
        );
        assert!(first.is_some());
        std::fs::remove_file(&path).unwrap();
        let second = super::super::definitions::selected_definition_read(
            &json!({}), &index, &index.definitions[1], &mut cache,
        );
        assert!(second.is_some());
        assert_eq!(cache.len(), 1);

        std::fs::write(&path, vec![b'x'; super::super::definitions::MAX_HINT_SOURCE_BYTES + 1]).unwrap();
        cache.clear();
        assert!(super::super::definitions::selected_definition_read(
            &json!({}), &index, &index.definitions[0], &mut cache,
        ).is_none());
        std::fs::write(&path, "first\nsecond\n").unwrap();
        assert!(super::super::definitions::selected_definition_read(
            &json!({}), &index, &index.definitions[1], &mut cache,
        ).is_none());
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert!(super::super::definitions::selected_definition_read(
            &json!({}), &index, &index.definitions[1], &mut cache,
        ).is_some());
    }

    #[test]
    fn executable_hints_metadata_snapshot_change_and_missing_source() {
        let (_temp, ctx, path, _) = fixture(1, 10, 1);
        let request = json!({"file": [path], "name": ["Compute"]});
        let output = payload(&dispatch_tool(&ctx, "xray_definitions", &request), 16384);
        let next = &output["recommendedNextQueries"][0]["args"];
        std::fs::write(&path, "// changed\n".repeat(10)).unwrap();
        let read = dispatch_tool(&ctx, "xray_definitions", next);
        assert!(read.is_error);
        assert_eq!(payload(&read, 16384)["error"]["code"], "body_source_changed");
        std::fs::remove_file(&path).unwrap();
        let output = payload(&dispatch_tool(&ctx, "xray_definitions", &request), 16384);
        assert!(output.get("recommendedNextQueries").is_none());
        assert!(output.get("nextQueryUnavailable").is_some());
    }

    fn payload(result: &ToolCallResult, limit: usize) -> Value {
        assert!(result.content.iter().map(|content| content.text.len()).sum::<usize>() <= limit);
        let text = &result.content[0].text;
        let json = text.split_once("\n\n").map(|(_, suffix)| suffix).unwrap_or(text);
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn body_delivery_reads_all_lines_after_byte_fit() {
        let (_temp, ctx, path, expected) = fixture(1, 160, 30);
        with_transport_limit(8192, || {
            let mut args = json!({"file": [path], "name": ["Compute"], "exactNameOnly": true,
                "includeBody": true, "maxBodyLines": 0, "maxTotalBodyLines": 0, "excludeDir": ["excluded"]});
            let mut actual = Vec::new();
            for _page in 0..160 {
                let result = dispatch_tool(&ctx, "xray_definitions", &args);
                assert!(!result.is_error, "{}", result.content[0].text);
                let output = payload(&result, 8192);
                let definition = &output["definitions"][0];
                assert_eq!(definition["bodyStartLine"].as_u64().unwrap(), actual.len() as u64 + 1);
                let body = definition["body"].as_array().unwrap();
                assert!(!body.is_empty());
                actual.extend(body.iter().map(|line| line.as_str().unwrap().to_string()));
                let Some(next) = definition.pointer("/bodyContinuation/nextArgs") else { break; };
                assert_eq!(next["excludeDir"], args["excludeDir"]);
                assert!(super::super::arg_validation::check_unknown_args("xray_definitions", next).is_none());
                args = next.clone();
            }
            assert_eq!(actual, expected);
        });
    }

    #[test]
    fn body_delivery_anchor_survives_byte_fit_and_covers_both_sides() {
        let (_temp, ctx, path, expected) = fixture(1, 100, 40);
        with_transport_limit(6000, || {
            let args = json!({"file": [path], "containsLine": 50, "includeBody": true, "maxBodyLines": 8});
            let result = dispatch_tool(&ctx, "xray_definitions", &args);
            assert!(!result.is_error, "{}", result.content[0].text);
            let output = payload(&result, 6000);
            let definition = &output["containingDefinitions"][0];
            assert_eq!(definition["bodyAnchorLine"], 50);
            assert_eq!(definition["bodyAnchorVisible"], true);
            assert_eq!(definition["bodyComplete"], false);
            let mut covered = std::collections::BTreeMap::new();
            let mut pending = vec![definition.clone()];
            let mut requests = 0;
            while let Some(fragment) = pending.pop() {
                let start = fragment["bodyStartLine"].as_u64().unwrap();
                for (offset, line) in fragment["body"].as_array().unwrap().iter().enumerate() {
                    assert!(covered.insert(start + offset as u64, line.as_str().unwrap().to_string()).is_none());
                }
                for key in ["beforeArgs", "nextArgs"] {
                    if let Some(next) = fragment["bodyContinuation"].get(key) {
                        requests += 1;
                        assert!(requests < 100);
                        let result = dispatch_tool(&ctx, "xray_definitions", next);
                        assert!(!result.is_error, "{}", result.content[0].text);
                        pending.push(payload(&result, 6000)["definitions"][0].clone());
                    }
                }
            }
            assert_eq!(covered.into_values().collect::<Vec<_>>(), expected);
        });
    }

    #[test]
    fn body_delivery_detects_changed_source_and_keeps_overload_identity() {
        let (_temp, ctx, path, _) = fixture(2, 50, 4);
        with_transport_limit(16384, || {
            let first = dispatch_tool(&ctx, "xray_definitions", &json!({
                "file": [path], "containsLine": 75, "includeBody": true, "maxBodyLines": 5
            }));
            assert!(!first.is_error, "{}", first.content[0].text);
            let output = payload(&first, 16384);
            let next = &output["containingDefinitions"][0]["bodyContinuation"]["nextArgs"];
            let result = dispatch_tool(&ctx, "xray_definitions", next);
            assert!(!result.is_error, "{}", result.content[0].text);
            let output = payload(&result, 16384);
            assert_eq!(output["definitions"].as_array().unwrap().len(), 1);
            assert_eq!(next["bodyTarget"]["startLine"], 51);
            assert_eq!(output["definitions"][0]["lines"], "51-100");
            assert_eq!(output["definitions"][0]["bodyStartLine"], next["bodyLineStart"]);
            std::fs::write(&path, "// changed\n".repeat(100)).unwrap();
            let changed = dispatch_tool(&ctx, "xray_definitions", next);
            assert!(changed.is_error);
            assert_eq!(payload(&changed, 16384)["error"]["code"], "body_source_changed");
        });
    }

    #[test]
    fn body_delivery_page_args_follow_final_byte_fitted_cursor() {
        let (_temp, ctx, path, _) = fixture(60, 2, 1);
        with_transport_limit(6000, || {
            let mut args = json!({"file": [path], "name": ["Compute"], "exactNameOnly": true,
                "excludeDir": ["excluded"], "maxResults": 0});
            let mut ranges = std::collections::HashSet::new();
            for _page in 0..60 {
                let result = dispatch_tool(&ctx, "xray_definitions", &args);
                assert!(!result.is_error, "{}", result.content[0].text);
                let output = payload(&result, 6000);
                for definition in output["definitions"].as_array().unwrap() {
                    assert!(ranges.insert(definition["lines"].as_str().unwrap().to_string()));
                }
                let page = &output["resultStatus"]["page"];
                let Some(next) = page.get("nextArgs") else { assert!(page.get("continuationToken").is_none()); break; };
                assert_eq!(next["continuationToken"], page["continuationToken"]);
                assert_eq!(next["excludeDir"], args["excludeDir"]);
                assert!(next.get("offset").is_none());
                let mut invalid = next.clone();
                invalid["excludeDir"] = json!([]);
                assert!(dispatch_tool(&ctx, "xray_definitions", &invalid).is_error);
                args = next.clone();
            }
            assert_eq!(ranges.len(), 60);
        });
    }

    #[test]
    fn body_delivery_small_body_and_unfit_line() {
        let (_temp, ctx, path, expected) = fixture(1, 2, 1);
        with_transport_limit(16384, || {
            let result = dispatch_tool(&ctx, "xray_definitions", &json!({"file": [path], "name": ["Compute"], "includeBody": true}));
            assert!(!result.is_error);
            let output = payload(&result, 16384);
            let definition = &output["definitions"][0];
            assert_eq!(definition["bodyComplete"], true);
            assert_eq!(definition["body"], json!(expected));
            assert!(definition.get("bodyContinuation").is_none());
        });
        let (_temp, ctx, path, _) = fixture(1, 1, 10000);
        with_transport_limit(4096, || {
            let result = dispatch_tool(&ctx, "xray_definitions", &json!({"file": [path], "name": ["Compute"], "includeBody": true}));
            assert!(result.is_error, "{}", result.content[0].text);
            payload(&result, 4096);
        });
    }

    #[test]
    fn body_delivery_prefix_and_metrics_matrix() {
        struct PrefixReset(Option<bool>);
        impl Drop for PrefixReset {
            fn drop(&mut self) { super::super::utils::set_guidance_prefix_override_for_test(self.0); }
        }
        for prefix in [false, true] {
            let _reset = PrefixReset(super::super::utils::set_guidance_prefix_override_for_test(Some(prefix)));
            for metrics in [false, true] {
                let (_temp, mut ctx, path, _) = fixture(1, 100, 40);
                ctx.metrics = metrics;
                with_transport_limit(8192, || {
                    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
                        "file": [path], "containsLine": 50, "includeBody": true, "maxBodyLines": 100
                    }));
                    assert!(!result.is_error, "{}", result.content[0].text);
                    let output = payload(&result, 8192);
                    let definition = &output["containingDefinitions"][0];
                    assert_eq!(definition["bodyAnchorVisible"], true);
                    assert!(definition.get("bodyRead").is_none());
                    if metrics {
                        assert_eq!(output["summary"]["responseBytes"].as_u64().unwrap() as usize, result.content[0].text.len());
                    }
                    for key in ["beforeArgs", "nextArgs"] {
                        if let Some(next) = definition["bodyContinuation"].get(key) {
                            for field in ["file", "name", "kind", "parent", "excludeDir"] {
                                if let Some(value) = next.get(field) {
                                    assert!(value.as_array().is_some_and(|items| items.iter().all(Value::is_string)));
                                }
                            }
                            let followup = dispatch_tool(&ctx, "xray_definitions", next);
                            assert!(!followup.is_error, "{}", followup.content[0].text);
                            payload(&followup, 8192);
                        }
                    }
                });
            }
        }
    }

    #[cfg(feature = "lang-xml")]
    #[test]
    fn body_delivery_xml_target_and_anchor() {
        let (_temp, ctx, path, _) = fixture(1, 1, 1);
        let file = std::path::Path::new(&path).with_file_name("sample.xml");
        let source = format!("<Root>\n<Section>\n{}\n</Section>\n</Root>",
            (1..=50).map(|line| format!("<Item>{line}</Item>")).collect::<Vec<_>>().join("\n"));
        std::fs::write(&file, &source).unwrap();
        with_transport_limit(8192, || {
            let result = dispatch_tool(&ctx, "xray_definitions", &json!({
                "file": [file.to_string_lossy()], "containsLine": 25, "includeBody": true, "maxBodyLines": 8
            }));
            assert!(!result.is_error, "{}", result.content[0].text);
            let output = payload(&result, 8192);
            let definition = &output["definitions"][0];
            assert_eq!(definition["name"], "Section");
            assert_eq!(definition["bodyAnchorVisible"], true);
            let next = &definition["bodyContinuation"]["nextArgs"];
            let followup = dispatch_tool(&ctx, "xray_definitions", next);
            assert!(!followup.is_error, "{}", followup.content[0].text);
            assert_eq!(payload(&followup, 8192)["definitions"][0]["bodyStartLine"], next["bodyLineStart"]);
            std::fs::write(&file, source.replace("<Item>1</Item>", "<Item>changed</Item>")).unwrap();
            let changed = dispatch_tool(&ctx, "xray_definitions", next);
            assert!(changed.is_error);
            assert_eq!(payload(&changed, 8192)["error"]["code"], "body_source_changed");
        });
    }

    #[test]
    fn body_delivery_callers_batch_obeys_cap() {
        let (_temp, ctx, _path, _) = fixture(2, 80, 80);
        with_transport_limit(16384, || {
            let result = dispatch_tool(&ctx, "xray_callers", &json!({
                "method": ["Compute", "Missing"], "includeBody": true, "maxBodyLines": 0,
                "maxTotalBodyLines": 0, "excludeDir": ["excluded"]
            }));
            assert!(!result.is_error, "{}", result.content[0].text);
            let mut output = payload(&result, 16384);
            let mut continuations = Vec::new();
            visit_entries(&mut output, &mut |entry| {
                if let Some(next) = entry.pointer("/bodyContinuation/nextArgs") {
                    continuations.push(next.clone());
                }
            });
            assert!(!continuations.is_empty(), "{}", result.content[0].text);
            for next in continuations {
                assert_eq!(next["excludeDir"], json!(["excluded"]));
                let followup = dispatch_tool(&ctx, "xray_definitions", &next);
                assert!(!followup.is_error, "{}", followup.content[0].text);
                payload(&followup, 16384);
            }
        });
    }


    #[test]
    fn body_delivery_size_matches_finalized_metadata() {
        for output in [
            json!({"summary": {"count": 1}, "definitions": [{"name": "Compute"}]}),
            json!({"resultStatus": {"page": {"queryArgs": {"name": ["Compute"]}}}}),
            json!({"resultStatus": {"page": {"queryArgs": null}}}),
            json!({"definitions": [{"bodySourceHash": "hash", "bodyRead": {"args": {}}}]}),
            json!({"results": [{"rootMethod": {
                "bodySourceHash": "hash", "bodyRead": {"args": {}}
            }}]}),
        ] {
            let mut finalized = output.clone();
            finalize_metadata(&mut finalized);
            assert_eq!(delivered_size(&output), super::super::utils::json_to_string(&finalized).len());
        }
    }

    #[test]
    fn body_delivery_small_bodies_do_not_pay_for_internal_metadata() {
        let (_temp, ctx, path, _) = fixture(24, 2, 1);
        with_transport_limit(16384, || {
            let result = dispatch_tool(&ctx, "xray_definitions", &json!({
                "file": [path], "name": ["Compute"], "includeBody": true,
                "maxBodyLines": 0, "maxTotalBodyLines": 0
            }));
            assert!(!result.is_error, "{}", result.content[0].text);
            let output = payload(&result, 16384);
            let entries = output["definitions"].as_array().unwrap();
            assert_eq!(entries.len(), 24);
            for entry in entries {
                assert_eq!(entry["bodyComplete"], true);
                assert_eq!(entry["bodyRangeComplete"], true);
                assert_eq!(entry["body"].as_array().unwrap().len(), 2);
                for key in ["bodyRead", "bodySourceHash", "bodyFile", "bodyDefinitionStartLine",
                    "bodyDefinitionEndLine", "bodyAvailableStartLine", "bodyRequestedStartLine",
                    "bodyRequestedEndLine", "bodyExplicitRange"]
                {
                    assert!(entry.get(key).is_none(), "{key}: {entry}");
                }
            }
            assert!(output["summary"].get("responseTruncated").is_none());
        });
        let mut internal = json!({"definitions": [{"body": ["small"], "bodyStartLine": 1,
            "bodyEndLine": 1, "bodyComplete": true, "bodySourceHash": "hash",
            "bodyRead": {"args": "x".repeat(20000)}}]});
        let expected_size = delivered_size(&internal);
        internal = super::super::utils::truncate_large_response(internal, expected_size);
        assert_eq!(internal["definitions"][0]["body"], json!(["small"]));
        assert!(internal.get("summary").is_none());
        finalize_metadata(&mut internal);
        assert_eq!(super::super::utils::json_to_string(&internal).len(), expected_size);
    }

    #[test]
    fn body_delivery_restricted_callers_keep_fragments() {
        let (_temp, ctx, _path, expected) = fixture(1, 80, 80);
        for restriction in [json!({"productionOnly": true}), json!({"ext": ["rs"]}),
            json!({"excludeFile": ["absent"]}), json!({"unknownDeliveryArg": true})]
        {
            with_transport_limit(16384, || {
                let mut args = json!({"method": ["Compute"], "includeBody": true,
                    "maxBodyLines": 0, "maxTotalBodyLines": 0});
                args.as_object_mut().unwrap().extend(restriction.as_object().unwrap().clone());
                let result = dispatch_tool(&ctx, "xray_callers", &args);
                assert!(!result.is_error, "{}", result.content[0].text);
                let mut output = payload(&result, 16384);
                let unknown = restriction.get("unknownDeliveryArg").is_some();
                if unknown { assert!(output.get("continuationUnavailable").is_some()); }
                let mut fragments = 0;
                visit_entries(&mut output, &mut |entry| {
                    if let Some(body) = entry.get("body").and_then(Value::as_array) {
                        assert!(!body.is_empty());
                        assert!(body.len() < expected.len());
                        assert_eq!(entry["bodyComplete"], false);
                        let start = entry["bodyStartLine"].as_u64().unwrap() as usize;
                        let end = entry["bodyEndLine"].as_u64().unwrap() as usize;
                        assert_eq!(body, json!(expected[start - 1..end]).as_array().unwrap());
                        assert!(entry.get("bodyContinuation").is_none());
                        if !unknown { assert!(entry.get("bodyContinuationUnavailable").is_some()); }
                        fragments += 1;
                    }
                });
                assert!(fragments > 0, "{}", result.content[0].text);
            });
        }
    }

    #[test]
    fn body_delivery_unknown_args_keep_anchor_without_replay() {
        let (_temp, ctx, path, expected) = fixture(1, 80, 80);
        with_transport_limit(16384, || {
            let result = dispatch_tool(&ctx, "xray_definitions", &json!({
                "file": [path], "containsLine": 40, "includeBody": true,
                "maxBodyLines": 0, "maxTotalBodyLines": 0, "unknownDeliveryArg": true
            }));
            assert!(!result.is_error, "{}", result.content[0].text);
            let output = payload(&result, 16384);
            let entry = &output["containingDefinitions"][0];
            assert_eq!(entry["bodyAnchorVisible"], true);
            assert_eq!(entry["bodyComplete"], false);
            assert_eq!(entry["bodyRangeComplete"], false);
            assert!(output.get("continuationUnavailable").is_some());
            assert!(entry.get("bodyContinuation").is_none());
            let start = entry["bodyStartLine"].as_u64().unwrap() as usize;
            let end = entry["bodyEndLine"].as_u64().unwrap() as usize;
            assert!(start <= 40 && end >= 40);
            assert_eq!(entry["body"], json!(expected[start - 1..end]));
        });
    }

    #[test]
    fn body_delivery_transport_limit_parser() {
        assert_eq!(parse_transport_limit(Err(std::env::VarError::NotPresent)).unwrap(), None);
        for limit in [512, 4096, usize::MAX] {
            assert_eq!(parse_transport_limit(Ok(limit.to_string())).unwrap(), Some(limit));
        }
        for value in ["", "0", "511", "-1", "1.5", "abc", " 512", "512 ", "18446744073709551616"] {
            let result = parse_transport_limit(Ok(value.to_string())).unwrap_err();
            assert!(result.is_error);
            let output = payload(&result, 512);
            assert_eq!(output["error"]["code"], "invalid_transport_max_bytes");
            assert_eq!(output["error"]["parameter"], "XRAY_TRANSPORT_MAX_BYTES");
            assert!(output["error"]["message"].as_str().unwrap().contains("at least 512"));
            assert_eq!(output["resultStatus"]["complete"], false);
        }
        #[cfg(windows)]
        let invalid = {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&[0xd800])
        };
        #[cfg(not(windows))]
        let invalid = {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(vec![0xff])
        };
        let result = parse_transport_limit(Err(std::env::VarError::NotUnicode(invalid))).unwrap_err();
        assert!(result.is_error);
        let output = payload(&result, 512);
        assert_eq!(output["error"]["code"], "invalid_transport_max_bytes");
        assert!(output["error"]["message"].as_str().unwrap().contains("not valid Unicode"));
    }

    #[test]
    fn body_delivery_help_and_errors_obey_transport_limit() {
        let ctx = HandlerContext::default();
        for limit in [512, 4096, 16384] {
            with_transport_limit(limit, || {
                for tool in ["xray_help", "unknown_tool"] {
                    payload(&dispatch_tool(&ctx, tool, &json!({})), limit);
                }
            });
        }
    }
}
