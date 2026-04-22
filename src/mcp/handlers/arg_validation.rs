//! Args validation: detect unknown/aliased keys before dispatching to handlers.
//!
//! ## Rationale
//!
//! Every handler parses args via `args.get("key").and_then(...)`, so misspelled
//! or alien-named keys (e.g., `includePattern`, `isRegexp` from VS Code's
//! `grep_search`; `query`, `path`, `since` from generic LLM guesses) are silently
//! ignored, leading to surprising results: filters not applied, regex flag does
//! nothing, search runs over the whole repo. This module surfaces unknown keys
//! as `unknownArgsWarning` in `summary` (or hard errors when the env var
//! `XRAY_STRICT_ARGS=1` is set).
//!
//! ## Source of truth
//!
//! Allowed arg names per tool are extracted from `tool_definitions(&[])`
//! (`inputSchema.properties` keys) — no per-handler hardcoding.
//!
//! ## Aliases
//!
//! A small static table maps the most common LLM/agent aliases to the correct
//! xray parameter, so the warning can say `Use 'regex' instead.` rather than
//! the generic `Did you mean 'regex'? (similarity 71%)`.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use serde_json::{json, Value};

use super::utils::{json_to_string, name_similarity};
use crate::mcp::protocol::ToolCallResult;

/// Threshold for nearest-key suggestion via Jaro-Winkler similarity.
const NEAREST_KEY_THRESHOLD: f64 = 0.80;

/// Per-tool set of allowed arg names, derived from `tool_definitions(&[])`.
fn known_args() -> &'static HashMap<String, HashSet<String>> {
    static MAP: OnceLock<HashMap<String, HashSet<String>>> = OnceLock::new();
    MAP.get_or_init(build_known_args_map)
}

fn build_known_args_map() -> HashMap<String, HashSet<String>> {
    // `def_extensions=&[]` is fine: extensions only affect description text,
    // not the property names in inputSchema.
    let tools = super::tool_definitions(&[]);
    let mut map: HashMap<String, HashSet<String>> = HashMap::with_capacity(tools.len());
    for tool in tools {
        let props = tool
            .input_schema
            .get("properties")
            .and_then(|v| v.as_object());
        let names: HashSet<String> = match props {
            Some(obj) => obj.keys().cloned().collect(),
            None => HashSet::new(),
        };
        map.insert(tool.name, names);
    }
    map
}

/// Alien-arg → preferred-arg mapping with optional explanatory note.
struct Alias {
    correct: &'static str,
    note: Option<&'static str>,
}

fn alias_table() -> &'static HashMap<&'static str, Alias> {
    static TABLE: OnceLock<HashMap<&'static str, Alias>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut m: HashMap<&'static str, Alias> = HashMap::new();
        // VS Code grep_search aliases (most common offenders)
        m.insert("isRegexp", Alias { correct: "regex", note: None });
        m.insert("useRegex", Alias { correct: "regex", note: None });
        m.insert("is_regex", Alias { correct: "regex", note: None });
        m.insert(
            "includePattern",
            Alias {
                correct: "file",
                note: Some("xray_grep `file` is substring + comma-OR (e.g., 'Service,Client'), not a glob"),
            },
        );
        m.insert(
            "excludePattern",
            Alias {
                correct: "excludeDir",
                note: Some("xray uses `excludeDir` for directory-name exclusion (array)"),
            },
        );
        m.insert(
            "glob",
            Alias {
                correct: "pattern",
                note: Some("xray_fast `pattern` auto-detects glob chars (* ?); plain substring also works"),
            },
        );
        // Generic LLM guesses
        m.insert("query", Alias { correct: "terms", note: Some("xray_grep uses `terms`; xray_definitions uses `name`") });
        m.insert("search", Alias { correct: "terms", note: None });
        m.insert("path", Alias { correct: "file", note: Some("for a single file use `file`; for a directory use `dir`") });
        m.insert("filePath", Alias { correct: "file", note: None });
        m.insert("file_path", Alias { correct: "file", note: None });
        m.insert("directory", Alias { correct: "dir", note: None });
        m.insert("limit", Alias { correct: "maxResults", note: None });
        m.insert("max", Alias { correct: "maxResults", note: None });
        m.insert("count", Alias { correct: "maxResults", note: None });
        // xray_callers
        m.insert("function", Alias { correct: "method", note: None });
        m.insert("func", Alias { correct: "method", note: None });
        m.insert("methodName", Alias { correct: "method", note: None });
        m.insert("caller", Alias { correct: "direction", note: Some("use direction='up' for callers") });
        m.insert("callee", Alias { correct: "direction", note: Some("use direction='down' for callees") });
        // xray_edit
        m.insert("preview", Alias { correct: "dryRun", note: None });
        m.insert("dry_run", Alias { correct: "dryRun", note: None });
        m.insert("dryrun", Alias { correct: "dryRun", note: None });
        m.insert("find", Alias { correct: "edits", note: Some("text-match mode: edits=[{search:'old', replace:'new'}]") });
        m.insert("oldText", Alias { correct: "edits", note: Some("use edits=[{search:'old', replace:'new'}]") });
        m.insert("oldString", Alias { correct: "edits", note: Some("use edits=[{search:'old', replace:'new'}]") });
        m.insert("newText", Alias { correct: "edits", note: Some("use edits=[{search:'old', replace:'new'}]") });
        m.insert("newString", Alias { correct: "edits", note: Some("use edits=[{search:'old', replace:'new'}]") });
        // git tools
        m.insert("since", Alias { correct: "from", note: None });
        m.insert("until", Alias { correct: "to", note: None });
        m.insert("repository", Alias { correct: "repo", note: None });
        m.insert("repo_path", Alias { correct: "repo", note: None });
        m.insert("repoPath", Alias { correct: "repo", note: None });
        m
    })
}

/// One unknown arg with a human-readable hint.
#[derive(Debug, Clone)]
pub(crate) struct UnknownArg {
    pub key: String,
    pub hint: String,
}

/// Result of validating a tool-call's args against the schema.
#[derive(Debug, Clone, Default)]
pub(crate) struct UnknownArgsReport {
    pub unknown: Vec<UnknownArg>,
}

impl UnknownArgsReport {
    pub fn is_empty(&self) -> bool {
        self.unknown.is_empty()
    }
}

/// Returns `Some(report)` if `args` contains keys not in the tool's schema.
/// Returns `None` if everything is valid (or if the tool has no schema entry,
/// which means we don't validate it — defensive default).
pub(crate) fn check_unknown_args(tool_name: &str, args: &Value) -> Option<UnknownArgsReport> {
    let allowed = known_args().get(tool_name)?;
    let obj = args.as_object()?;

    let mut report = UnknownArgsReport::default();
    for key in obj.keys() {
        if allowed.contains(key) {
            continue;
        }
        let hint = build_hint(key, allowed);
        report.unknown.push(UnknownArg {
            key: key.clone(),
            hint,
        });
    }

    if report.is_empty() {
        None
    } else {
        Some(report)
    }
}

fn build_hint(unknown_key: &str, allowed: &HashSet<String>) -> String {
    // 1. Alias table — prefer explicit hint.
    if let Some(alias) = alias_table().get(unknown_key)
        && allowed.contains(alias.correct)
    {
        return match alias.note {
            Some(note) => format!("Use '{}' instead. {}", alias.correct, note),
            None => format!("Use '{}' instead.", alias.correct),
        };
    }

    // 2. Jaro-Winkler nearest match within this tool's known args.
    let mut best: Option<(&String, f64)> = None;
    for known in allowed {
        let score = name_similarity(unknown_key, known);
        if score >= NEAREST_KEY_THRESHOLD
            && best.as_ref().is_none_or(|(_, s)| score > *s)
        {
            best = Some((known, score));
        }
    }
    if let Some((name, score)) = best {
        return format!(
            "Did you mean '{}'? (similarity {}%)",
            name,
            (score * 100.0).round() as u32
        );
    }

    // 3. Generic fallback.
    "Unknown argument; no close match in the tool's schema.".to_string()
}

/// Compose a one-line warning suitable for `summary.unknownArgsWarning`.
pub(crate) fn warning_text(report: &UnknownArgsReport) -> String {
    let parts: Vec<String> = report
        .unknown
        .iter()
        .map(|u| format!("'{}': {}", u.key, u.hint))
        .collect();
    format!(
        "Unknown args silently ignored ({}): {}. Set XRAY_STRICT_ARGS=1 to make this an error.",
        report.unknown.len(),
        parts.join("; ")
    )
}

/// True when `XRAY_STRICT_ARGS=1` (or `true`/`yes`/`on`) is set in the environment.
pub(crate) fn strict_args_enabled() -> bool {
    std::env::var("XRAY_STRICT_ARGS")
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Build a strict-mode error response (returned before handler dispatch when
/// `XRAY_STRICT_ARGS=1`).
pub(crate) fn strict_error_response(tool_name: &str, report: &UnknownArgsReport) -> ToolCallResult {
    let body = json!({
        "error": "UNKNOWN_ARGS",
        "tool": tool_name,
        "message": warning_text(report),
        "unknownArgs": report.unknown.iter().map(|u| json!({
            "key": u.key,
            "hint": u.hint,
        })).collect::<Vec<_>>(),
        "envHint": "Unset XRAY_STRICT_ARGS or set it to '0' to downgrade these to warnings."
    });
    ToolCallResult::error(json_to_string(&body))
}

/// Inject `unknownArgsWarning` into `summary` of a handler's result.
/// If the result text isn't valid JSON or has no object root, returns the
/// result unchanged (defensive — never break a successful response).
pub(crate) fn inject_warning(result: ToolCallResult, report: &UnknownArgsReport) -> ToolCallResult {
    let was_error = result.is_error;
    let text = match result.content.first() {
        Some(c) => &c.text,
        None => return result,
    };

    let mut output = match serde_json::from_str::<Value>(text) {
        Ok(v) => v,
        Err(_) => return result,
    };

    let Some(obj) = output.as_object_mut() else {
        return result;
    };

    if !obj.contains_key("summary") {
        obj.insert("summary".to_string(), json!({}));
    }
    if let Some(summary) = obj.get_mut("summary").and_then(|v| v.as_object_mut()) {
        summary.insert(
            "unknownArgsWarning".to_string(),
            json!(warning_text(report)),
        );
        summary.insert(
            "unknownArgs".to_string(),
            json!(report
                .unknown
                .iter()
                .map(|u| json!({"key": u.key, "hint": u.hint}))
                .collect::<Vec<_>>()),
        );
    }

    let new_result = ToolCallResult::success(json_to_string(&output));
    if was_error {
        ToolCallResult { is_error: true, ..new_result }
    } else {
        new_result
    }
}


/// Serializes tests that mutate `XRAY_STRICT_ARGS` (process-wide env var).
/// Shared with `handlers_tests.rs` so both modules grab the same lock.
#[cfg(test)]
pub(crate) static STRICT_ARGS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ─── known_args() — schema extraction ───────────────────────────

    #[test]
    fn known_args_includes_grep_terms_and_file() {
        let map = known_args();
        let grep = map.get("xray_grep").expect("xray_grep should have a schema entry");
        assert!(grep.contains("terms"), "xray_grep should accept `terms`");
        assert!(grep.contains("file"), "xray_grep should accept `file`");
        assert!(grep.contains("regex"), "xray_grep should accept `regex`");
        assert!(grep.contains("excludeDir"), "xray_grep should accept `excludeDir`");
    }

    #[test]
    fn known_args_covers_all_expected_tools() {
        let map = known_args();
        for name in [
            "xray_grep",
            "xray_fast",
            "xray_definitions",
            "xray_callers",
            "xray_edit",
            "xray_info",
            "xray_help",
            "xray_reindex",
            "xray_reindex_definitions",
            "xray_git_history",
            "xray_git_authors",
            "xray_git_activity",
            "xray_git_blame",
            "xray_branch_status",
        ] {
            assert!(
                map.contains_key(name),
                "known_args() should include schema for `{}`",
                name
            );
        }
    }

    // ─── check_unknown_args() — happy path ───────────────────────────

    #[test]
    fn check_unknown_args_returns_none_for_valid_keys() {
        let args = json!({"terms": "Foo", "regex": true, "file": "bar.rs"});
        assert!(check_unknown_args("xray_grep", &args).is_none());
    }

    #[test]
    fn check_unknown_args_returns_none_for_empty_args() {
        let args = json!({});
        assert!(check_unknown_args("xray_grep", &args).is_none());
    }

    #[test]
    fn check_unknown_args_returns_none_for_unknown_tool() {
        // Defensive: tools without a schema entry are not validated.
        let args = json!({"foo": "bar"});
        assert!(check_unknown_args("definitely_not_a_tool", &args).is_none());
    }

    // ─── alias hints — VS Code grep_search names ───────────────────────────

    #[test]
    fn alias_hint_isregexp_to_regex() {
        let args = json!({"terms": "Foo", "isRegexp": true});
        let report = check_unknown_args("xray_grep", &args).expect("isRegexp should be flagged");
        assert_eq!(report.unknown.len(), 1);
        assert_eq!(report.unknown[0].key, "isRegexp");
        assert!(
            report.unknown[0].hint.contains("Use 'regex' instead"),
            "hint should suggest 'regex', got: {}",
            report.unknown[0].hint
        );
    }

    #[test]
    fn alias_hint_includepattern_to_file() {
        let args = json!({"terms": "Foo", "includePattern": "src/**/*.rs"});
        let report = check_unknown_args("xray_grep", &args).expect("includePattern should be flagged");
        let hint = &report.unknown[0].hint;
        assert!(hint.contains("Use 'file' instead"), "hint should suggest 'file', got: {hint}");
        assert!(
            hint.contains("substring + comma-OR"),
            "hint should explain semantics, got: {hint}"
        );
    }

    #[test]
    fn alias_hint_function_to_method_for_callers() {
        let args = json!({"function": "GetUser", "class": "UserService"});
        let report = check_unknown_args("xray_callers", &args).expect("`function` should be flagged");
        assert!(report.unknown[0].hint.contains("Use 'method' instead"));
    }

    #[test]
    fn alias_hint_path_to_file_for_edit() {
        let args = json!({"path": "foo.rs", "edits": []});
        let report = check_unknown_args("xray_edit", &args);
        // `path` IS a valid xray_edit param, so it should NOT be flagged.
        assert!(report.is_none(), "xray_edit accepts `path`; should not warn");
    }

    #[test]
    fn alias_hint_since_to_from_for_git() {
        let args = json!({"since": "2025-01-01", "file": "src/main.rs"});
        let report = check_unknown_args("xray_git_history", &args)
            .expect("`since` should be flagged for git tools");
        assert!(report.unknown[0].hint.contains("Use 'from' instead"));
    }

    // ─── nearest-match (Jaro-Winkler) fallback ───────────────────────────

    #[test]
    fn nearest_match_suggests_close_typos() {
        // 'maxResult' (missing 's') should suggest 'maxResults'
        let args = json!({"terms": "Foo", "maxResult": 10});
        let report = check_unknown_args("xray_grep", &args).unwrap();
        let hint = &report.unknown[0].hint;
        assert!(
            hint.contains("Did you mean 'maxResults'?"),
            "hint should suggest nearest match, got: {hint}"
        );
    }

    #[test]
    fn nearest_match_falls_back_when_no_close_key() {
        let args = json!({"terms": "Foo", "qwertyuiop": 1});
        let report = check_unknown_args("xray_grep", &args).unwrap();
        let hint = &report.unknown[0].hint;
        assert!(
            hint.contains("no close match"),
            "hint should be the generic fallback, got: {hint}"
        );
    }

    // ─── multiple unknowns reported together ───────────────────────────

    #[test]
    fn multiple_unknown_args_collected_in_one_report() {
        let args = json!({
            "terms": "Foo",
            "isRegexp": true,
            "includePattern": "src",
            "qwertyuiop": 1
        });
        let report = check_unknown_args("xray_grep", &args).unwrap();
        assert_eq!(report.unknown.len(), 3);
        let keys: Vec<&str> = report.unknown.iter().map(|u| u.key.as_str()).collect();
        assert!(keys.contains(&"isRegexp"));
        assert!(keys.contains(&"includePattern"));
        assert!(keys.contains(&"qwertyuiop"));
    }

    // ─── warning_text() format ───────────────────────────

    #[test]
    fn warning_text_mentions_count_and_strict_env() {
        let report = UnknownArgsReport {
            unknown: vec![UnknownArg {
                key: "isRegexp".to_string(),
                hint: "Use 'regex' instead.".to_string(),
            }],
        };
        let text = warning_text(&report);
        assert!(text.contains("Unknown args silently ignored (1)"));
        assert!(text.contains("'isRegexp'"));
        assert!(text.contains("XRAY_STRICT_ARGS=1"));
    }

    // ─── strict_args_enabled() — env parsing ───────────────────────────

    #[test]
    fn strict_args_enabled_recognises_truthy_values() {
        let _guard = STRICT_ARGS_ENV_LOCK.lock().unwrap();
        // SAFETY: Setting/removing env vars is `unsafe` since Rust 2024 because it can
        // race with other threads reading env. Serialized via STRICT_ARGS_ENV_LOCK.
        for truthy in ["1", "true", "yes", "on", "TRUE", "On"] {
            unsafe { std::env::set_var("XRAY_STRICT_ARGS", truthy) };
            assert!(strict_args_enabled(), "value `{truthy}` should be truthy");
        }
        for falsy in ["0", "false", "no", "off", ""] {
            unsafe { std::env::set_var("XRAY_STRICT_ARGS", falsy) };
            assert!(!strict_args_enabled(), "value `{falsy}` should be falsy");
        }
        unsafe { std::env::remove_var("XRAY_STRICT_ARGS") };
        assert!(!strict_args_enabled(), "unset should be falsy");
    }

    // ─── inject_warning() — summary integration ───────────────────────────

    #[test]
    fn inject_warning_adds_summary_field_to_existing_summary() {
        let body = json!({
            "matches": [],
            "summary": {"totalFiles": 0}
        });
        let result = ToolCallResult::success(body.to_string());
        let report = UnknownArgsReport {
            unknown: vec![UnknownArg {
                key: "foo".to_string(),
                hint: "bar".to_string(),
            }],
        };
        let injected = inject_warning(result, &report);
        let parsed: Value = serde_json::from_str(&injected.content[0].text).unwrap();
        let summary = parsed.get("summary").and_then(|v| v.as_object()).unwrap();
        assert!(summary.contains_key("unknownArgsWarning"));
        assert!(summary.contains_key("unknownArgs"));
        // Pre-existing fields must be preserved.
        assert_eq!(summary.get("totalFiles"), Some(&json!(0)));
    }

    #[test]
    fn inject_warning_creates_summary_when_missing() {
        let body = json!({"matches": []});
        let result = ToolCallResult::success(body.to_string());
        let report = UnknownArgsReport {
            unknown: vec![UnknownArg {
                key: "foo".to_string(),
                hint: "bar".to_string(),
            }],
        };
        let injected = inject_warning(result, &report);
        let parsed: Value = serde_json::from_str(&injected.content[0].text).unwrap();
        assert!(parsed.get("summary").is_some());
    }

    #[test]
    fn inject_warning_preserves_is_error_flag() {
        let body = json!({"error": "boom"});
        let result = ToolCallResult::error(body.to_string());
        let report = UnknownArgsReport {
            unknown: vec![UnknownArg {
                key: "foo".to_string(),
                hint: "bar".to_string(),
            }],
        };
        let injected = inject_warning(result, &report);
        assert!(injected.is_error, "is_error must be preserved across injection");
    }

    #[test]
    fn inject_warning_no_op_for_non_json_body() {
        let result = ToolCallResult::success("not json at all".to_string());
        let report = UnknownArgsReport {
            unknown: vec![UnknownArg {
                key: "foo".to_string(),
                hint: "bar".to_string(),
            }],
        };
        let injected = inject_warning(result, &report);
        // Defensive: never break a response over a warning failure.
        assert_eq!(injected.content[0].text, "not json at all");
    }
}

