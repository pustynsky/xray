//! MCP tool handler for `xray_edit` — reliable file editing with two modes:
//! - Mode A (operations): line-range splice, applied bottom-up to avoid offset cascade
//! - Mode B (edits): text find-replace, literal or regex, with insert after/before support

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{json, Value};

use crate::mcp::protocol::ToolCallResult;
use super::utils::json_to_string;
use super::HandlerContext;

/// Edit mode: either line-range operations (Mode A) or text-match edits (Mode B).
/// Using an enum makes invalid states (both None or both Some) unrepresentable at the type level.
enum EditMode<'a> {
    /// Mode A: line-range splice operations, applied bottom-up.
    Operations(&'a [Value]),
    /// Mode B: text find-replace or insert after/before, applied sequentially.
    Edits(&'a [Value]),
}

/// Maximum number of files for multi-file edit (protection against abuse).
const MAX_MULTI_FILE_PATHS: usize = 20;

/// Maximum file size (in bytes) for nearest-match hint computation.
/// Files larger than this skip the hint to avoid performance impact.
const NEAREST_MATCH_MAX_FILE_SIZE: usize = 512_000; // 500 KB

/// Minimum similarity ratio (0.0–1.0) for a nearest match to be reported.
/// Below this threshold the hint is suppressed as unhelpful.
const NEAREST_MATCH_MIN_SIMILARITY: f64 = 0.4;

/// Maximum length of search/match text shown in hint messages.
const NEAREST_MATCH_MAX_DISPLAY_LEN: usize = 150;

/// Minimum similarity ratio for emitting a byte-level diff hint alongside the
/// `Nearest match` line. Was 0.99, but multi-line whitespace / blank-line drift
/// drops similarity to ~0.85–0.95 — right when the byte hint is most needed
/// (after Step 2/3 silent retries were removed).
const NEAREST_MATCH_BYTE_DIFF_THRESHOLD: f32 = 0.80;

/// Handle `xray_edit` tool call.
pub(crate) fn handle_xray_edit(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    // ── Reject unknown top-level parameters ──
    // Without this guard, common mis-spellings (`files`, `batch`, `targets`)
    // are silently dropped and the caller is left wondering why their batch
    // was treated as a single-file edit. Surface the typo with a concrete hint
    // pointing at `path` (single file) or `paths` (batch — same edits to all).
    if let Some(obj) = args.as_object()
        && let Some(unknown_msg) = check_unknown_top_level_params(obj)
    {
        return ToolCallResult::error(unknown_msg);
    }

    // ── Reject wrong-type values for canonical top-level params ──
    // Caller passing the right key with the wrong JSON type (e.g. `path: 123`,
    // `regex: "true"`) would otherwise silently fall through to None / default
    // and surface a misleading downstream error. Run AFTER the unknown-key
    // check so a typo'd key gets the alias hint, not a generic type error.
    if let Some(obj) = args.as_object()
        && let Some(type_msg) = check_top_level_param_types(obj)
    {
        return ToolCallResult::error(type_msg);
    }



    // ── Parse path/paths ──
    let single_path = args.get("path").and_then(|v| v.as_str());
    let multi_paths = args.get("paths").and_then(|v| v.as_array());

    // Validate: path XOR paths
    match (single_path, multi_paths) {
        (Some(_), Some(_)) => {
            return ToolCallResult::error(
                "Specify 'path' (single file) or 'paths' (multiple files), not both.".to_string(),
            );
        }
        (None, None) => {
            return ToolCallResult::error(
                missing_path_error_message(args),
            );
        }
        _ => {}
    }

    // ── Parse common arguments ──
    let operations = args.get("operations").and_then(|v| v.as_array());
    let edits = args.get("edits").and_then(|v| v.as_array());
    let is_regex = args.get("regex").and_then(|v| v.as_bool()).unwrap_or(false);
    let dry_run = args.get("dryRun").and_then(|v| v.as_bool()).unwrap_or(false);
    let expected_line_count = args.get("expectedLineCount").and_then(|v| v.as_u64()).map(|v| v as usize);

    // ── Validate mode and construct EditMode ──
    let mode = match (operations, edits) {
        (None, None) => {
            return ToolCallResult::error(
                "Specify 'operations' (line-range) or 'edits' (text-match), not neither.".to_string(),
            );
        }
        (Some(_), Some(_)) => {
            return ToolCallResult::error(
                "Specify 'operations' or 'edits', not both.".to_string(),
            );
        }
        (Some(ops), None) => EditMode::Operations(ops),
        (None, Some(eds)) => EditMode::Edits(eds),
    };

    // ── Dispatch single vs multi-file ──
    if let Some(paths_array) = multi_paths {
        handle_multi_file_edit(ctx, paths_array, &mode, is_regex, dry_run, expected_line_count)
    } else {
        let path_str = single_path.unwrap(); // validated above
        handle_single_file_edit(ctx, path_str, &mode, is_regex, dry_run, expected_line_count)
    }
}

/// Set of all top-level parameters accepted by `xray_edit`. Used by
/// `check_unknown_top_level_params` to flag typos / made-up wrappers
/// (`files`, `batch`, `targets`, …) that would otherwise be silently dropped.
const KNOWN_EDIT_PARAMS: &[&str] = &[
    "path",
    "paths",
    "operations",
    "edits",
    "regex",
    "dryRun",
    "expectedLineCount",
];

/// Per-edit fields accepted inside `edits[]` / `operations[]` (used both for
/// the "did you mean" hint when `path` is missing and for unknown-field
/// rejection inside `parse_text_edits`).
const KNOWN_EDIT_OBJECT_FIELDS: &[&str] = &[
    "search",
    "replace",
    "occurrence",
    "insertAfter",
    "insertBefore",
    "content",
    "startLine",
    "endLine",
    "expectedContext",
    "skipIfNotFound",
];

/// Common alias names callers reach for inside `edits[]` items (carried over
/// from Anthropic's text-editor tool, the VS Code `replace_string_in_file`
/// tool, sed-style "find/with", or just intuitive naming). Each entry maps
/// `(alias, canonical)`. We do NOT silently accept these — the goal is to
/// produce an actionable error pointing at the canonical name on the very
/// first failed attempt, so the caller does not iterate through schema
/// guesses or fall back to a built-in tool.
const EDIT_FIELD_SYNONYMS: &[(&str, &str)] = &[
    // search
    ("oldText", "search"),
    ("old_str", "search"),
    ("oldString", "search"),
    ("old", "search"),
    ("find", "search"),
    ("pattern", "search"),
    ("searchText", "search"),
    ("from", "search"),
    // replace
    ("newText", "replace"),
    ("new_str", "replace"),
    ("newString", "replace"),
    ("new", "replace"),
    ("with", "replace"),
    ("replaceWith", "replace"),
    ("replaceText", "replace"),
    ("to", "replace"),
    ("replacement", "replace"),
    // insertAfter / insertBefore
    ("after", "insertAfter"),
    ("insert_after", "insertAfter"),
    ("before", "insertBefore"),
    ("insert_before", "insertBefore"),
    // content
    ("text", "content"),
    ("value", "content"),
    ("body", "content"),
    // misc
    ("expected_context", "expectedContext"),
    ("context", "expectedContext"),
    ("skip_if_not_found", "skipIfNotFound"),
    ("optional", "skipIfNotFound"),
    ("nth", "occurrence"),
    ("index", "occurrence"),
];

/// Inline form-menu used in `edits[]` item error messages. Kept short — the
/// caller already has the JSON they sent in their context, so the goal is to
/// remind them which canonical fields exist, not to teach the full schema.
const EDIT_FORM_MENU: &str = "Each edit item must use ONE of three forms: \
    (a) {\"search\": \"old\", \"replace\": \"new\"} — text replacement; \
    (b) {\"insertAfter\": \"anchor\", \"content\": \"...\"} — insert after anchor; \
    (c) {\"insertBefore\": \"anchor\", \"content\": \"...\"} — insert before anchor. \
    Optional fields: occurrence, expectedContext, skipIfNotFound. \
    For Mode A (line-range) use the top-level 'operations' parameter instead.";

/// Canonical examples block, prefixed for inline appending into error messages.
/// Defined in `tips.rs` as the single source of truth so error hints emitted by
/// `xray_edit` and the per-tool `xray_help tool="xray_edit"` payload cannot drift apart.
use crate::tips::{CANONICAL_MODE_A_EXAMPLE, CANONICAL_MODE_B_EXAMPLE};

/// Common invented top-level wrappers callers reach for from other code-mod
/// tool families (Anthropic text-editor `files`/`changes`, sed-style `patches`,
/// diff-tools `hunks`/`diff`, generic `batch`/`targets`). Listed here so the
/// rejection hint can call them out by name and steer the caller to `path` /
/// `paths` instead of producing a generic "unknown parameter" message.
const INVENTED_TOP_LEVEL_WRAPPERS: &[&str] =
    &["files", "batch", "targets", "changes", "hunks", "patches", "diff"];

/// Append both canonical examples to an error message. Single source of truth
/// — `tips::tool_help("xray_edit")` consumes the same constants so the help
/// payload and the error hints cannot drift apart.
fn canonical_examples_block() -> String {
    format!(
        " Canonical examples — Mode A (line-range): {} | Mode B (text-match): {}",
        CANONICAL_MODE_A_EXAMPLE, CANONICAL_MODE_B_EXAMPLE
    )
}

/// Look up a synonym in `EDIT_FIELD_SYNONYMS`. Case-sensitive on purpose —
/// callers consistently use a single casing per attempt and we want to tell
/// them the exact canonical name.
fn lookup_edit_field_synonym(key: &str) -> Option<&'static str> {
    EDIT_FIELD_SYNONYMS
        .iter()
        .find(|(alias, _)| *alias == key)
        .map(|(_, canonical)| *canonical)
}

/// Validate the keys of a single `edits[]` item. Returns `Some(error)` when
/// any unknown / aliased / mis-typed field is present, `None` when every key
/// is recognised. This runs BEFORE per-mode validation so that the caller
/// gets a synonym/typo hint instead of the misleading "missing 'search'"
/// message that the search/replace fall-through used to produce.
///
/// Mode-A line-range fields (`startLine`/`endLine`) are not valid here
/// (`edits[]` is Mode B only), so they are reported with a hint to switch to
/// the `operations` parameter.
fn check_unknown_edit_object_fields(
    obj: &serde_json::Map<String, Value>,
    i: usize,
) -> Option<String> {
    // Fields that ARE valid inside an `edits[]` item (Mode B). Excludes the
    // Mode-A-only line-range fields.
    const VALID_EDITS_ITEM_FIELDS: &[&str] = &[
        "search",
        "replace",
        "occurrence",
        "insertAfter",
        "insertBefore",
        "content",
        "expectedContext",
        "skipIfNotFound",
    ];

    for key in obj.keys() {
        if VALID_EDITS_ITEM_FIELDS.contains(&key.as_str()) {
            continue;
        }
        // Surface alias → canonical mapping directly. Most common case in
        // practice (oldText/newText, find/with, pattern/with, …).
        if let Some(canonical) = lookup_edit_field_synonym(key) {
            return Some(format!(
                "edits[{i}]: unknown field '{key}'. Did you mean '{canonical}'? \
                 xray_edit uses '{canonical}', not '{key}'. {menu}",
                i = i, key = key, canonical = canonical, menu = EDIT_FORM_MENU
            ));
        }
        // Mode-A line-range fields used inside edits[] — wrong nesting level.
        if key == "startLine" || key == "endLine" {
            return Some(format!(
                "edits[{i}]: '{key}' is a Mode A (line-range) field and is not valid \
                 inside 'edits[]' (Mode B is text-match only). For line-range edits, \
                 use the top-level 'operations' parameter: \
                 {{\"path\": \"...\", \"operations\": [{{\"startLine\": 5, \"endLine\": 5, \"content\": \"...\"}}]}}.",
                i = i, key = key
            ));
        }
        // Generic typo → did_you_mean against the canonical field set.
        let suggestion = did_you_mean(key, VALID_EDITS_ITEM_FIELDS);
        let mut msg = format!("edits[{}]: unknown field '{}'.", i, key);
        if let Some(s) = suggestion {
            msg.push_str(&format!(" Did you mean '{}'?", s));
        }
        msg.push(' ');
        msg.push_str(EDIT_FORM_MENU);
        return Some(msg);
    }
    None
}

/// Build the error message for an `edits[]` item that contains no canonical
/// field at all (after `check_unknown_edit_object_fields` has cleared all
/// keys — i.e. the item is empty `{}` or contains only meta-fields like
/// `occurrence`/`expectedContext`/`skipIfNotFound`).
fn missing_edit_form_error_message(edit: &Value, i: usize) -> String {
    let keys: Vec<String> = edit
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    let got = if keys.is_empty() {
        "{} (empty object)".to_string()
    } else {
        format!("keys [{}]", keys.join(", "))
    };
    format!(
        "edits[{i}]: edit item is missing a primary action field (search/insertAfter/insertBefore). \
         Got {got}. {menu}",
        i = i, got = got, menu = EDIT_FORM_MENU
    )
}

/// Detect unknown top-level parameters and return an actionable error message,
/// or `None` if every key is recognised. Suggests the most-likely correct key
/// via `did_you_mean`, with a special case for the common `files: [...]`
/// wrapper invented by callers expecting per-file operations.
///
/// Every rejection path appends `canonical_examples_block()` so the very first
/// failed attempt teaches the schema (Mode A + Mode B) without forcing a
/// follow-up `xray_help` round-trip.
fn check_unknown_top_level_params(obj: &serde_json::Map<String, Value>) -> Option<String> {
    for key in obj.keys() {
        if KNOWN_EDIT_PARAMS.contains(&key.as_str()) {
            continue;
        }
        // Special case: `files` is the most common invented wrapper. Callers
        // expecting per-file `operations` (different ops per file) reach for
        // this shape because their mental model differs from `paths` (which
        // applies the SAME operations to every file). Surface the mismatch
        // explicitly so the caller does not waste turns probing variants.
        if key == "files" {
            let mut msg = String::from(
                "Unknown parameter 'files'. xray_edit does NOT take a per-file batch wrapper. \
                 Use 'paths' (array of file paths — the SAME 'edits'/'operations' are applied to ALL files) \
                 or call xray_edit once per file. Example batch: \
                 { \"paths\": [\"a.ts\", \"b.ts\"], \"edits\": [{\"search\": \"foo\", \"replace\": \"bar\"}] }.",
            );
            msg.push_str(&canonical_examples_block());
            return Some(msg);
        }
        // Other invented wrappers from sibling tool families. Same pattern:
        // call them out by name, steer to path/paths.
        if INVENTED_TOP_LEVEL_WRAPPERS.contains(&key.as_str()) {
            let mut msg = format!(
                "Unknown parameter '{}'. xray_edit does NOT use this wrapper — \
                 it is invented by other code-mod tool families. \
                 Use 'path' (single file) or 'paths' (array, same edits applied to all). \
                 Allowed top-level parameters: {}.",
                key,
                KNOWN_EDIT_PARAMS.join(", ")
            );
            msg.push_str(&canonical_examples_block());
            return Some(msg);
        }
        let suggestion = did_you_mean(key, KNOWN_EDIT_PARAMS);
        let mut msg = format!("Unknown parameter '{}'.", key);
        if let Some(s) = suggestion {
            msg.push_str(&format!(" Did you mean '{}'?", s));
        }
        msg.push_str(&format!(
            " Allowed top-level parameters: {}.",
            KNOWN_EDIT_PARAMS.join(", ")
        ));
        msg.push_str(&canonical_examples_block());
        return Some(msg);
    }
    None
}

/// Validate the JSON type of every recognised top-level parameter. Without
/// this gate, callers passing the right key with the wrong type — e.g.
/// `path: 123`, `regex: "true"`, `paths: "a.ts"`, `expectedLineCount: "10"` —
/// silently fall into `.and_then(|v| v.as_str())` / `.as_bool()` / `.as_u64()`
/// branches that drop to None or default-false and produce misleading
/// downstream errors ("missing path", "regex disabled", count mismatch).
fn check_top_level_param_types(obj: &serde_json::Map<String, Value>) -> Option<String> {
    fn err(key: &str, expected: &str, actual: &Value) -> String {
        format!(
            "Parameter '{}' must be {}, got {}.",
            key,
            expected,
            json_type_name(actual)
        )
    }

    if let Some(v) = obj.get("path")
        && !v.is_string()
    {
        return Some(err("path", "a string", v));
    }
    if let Some(v) = obj.get("paths") {
        match v {
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    if !item.is_string() {
                        return Some(format!(
                            "Parameter 'paths[{}]' must be a string, got {}.",
                            i,
                            json_type_name(item)
                        ));
                    }
                }
            }
            other => return Some(err("paths", "an array of strings", other)),
        }
    }
    if let Some(v) = obj.get("operations")
        && !v.is_array()
    {
        return Some(err("operations", "an array", v));
    }
    if let Some(v) = obj.get("edits")
        && !v.is_array()
    {
        return Some(err("edits", "an array", v));
    }
    if let Some(v) = obj.get("regex")
        && !v.is_boolean()
    {
        return Some(err("regex", "a boolean (true/false)", v));
    }
    if let Some(v) = obj.get("dryRun")
        && !v.is_boolean()
    {
        return Some(err("dryRun", "a boolean (true/false)", v));
    }
    if let Some(v) = obj.get("expectedLineCount")
        && v.as_u64().is_none()
    {
        return Some(err("expectedLineCount", "a non-negative integer", v));
    }
    None
}

/// Construct the "missing required parameter" error for the no-`path`/no-`paths` case,
/// with a concrete example for both single-file and batch forms. If a `path`
/// field appears inside an `edits[]` / `operations[]` object, surface that as
/// a structural hint (caller put `path` at the wrong nesting level).
fn missing_path_error_message(args: &Value) -> String {

    // Detect path-nested-inside-edit-object as a structural hint.
    let nested_hint = args.get("edits")
        .or_else(|| args.get("operations"))
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.iter().find(|item| {
            item.as_object().is_some_and(|o| o.contains_key("path"))
                && item.as_object().is_some_and(|o| {
                    o.keys().any(|k| KNOWN_EDIT_OBJECT_FIELDS.contains(&k.as_str()))
                })
        }))
        .map(|_| {
            " Note: 'path' must be a top-level parameter, not nested inside an 'edits' or 'operations' item."
        })
        .unwrap_or("");

    format!(
        "Missing required parameter: 'path' (single file) or 'paths' (array of files).{} \
         Single: {{ \"path\": \"a.ts\", \"edits\": [...] }}. \
         Batch (same edits to all files): {{ \"paths\": [\"a.ts\", \"b.ts\"], \"edits\": [...] }}.",
        nested_hint
    )
}

/// Levenshtein-style "did you mean" — returns the closest candidate within
/// edit distance 2, or None if no candidate is close enough.
fn did_you_mean<'a>(input: &str, candidates: &'a [&'a str]) -> Option<&'a str> {
    let input_lower = input.to_ascii_lowercase();
    let mut best: Option<(&str, usize)> = None;
    for cand in candidates {
        let dist = levenshtein(&input_lower, &cand.to_ascii_lowercase());
        if dist <= 2 && best.is_none_or(|(_, d)| dist < d) {
            best = Some((cand, dist));
        }
    }
    best.map(|(c, _)| c)
}

/// Standard iterative Levenshtein distance. Small inputs (parameter names),
/// so the O(n*m) table is negligible.
fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let (m, n) = (a_chars.len(), b_chars.len());
    if m == 0 { return n; }
    if n == 0 { return m; }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] { 0 } else { 1 };
            curr[j] = (curr[j - 1] + 1)
                .min(prev[j] + 1)
                .min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

/// Detail about a single skipped edit (when `skipIfNotFound=true`).
struct SkippedEditDetail {
    /// 0-based index of the edit in the edits array.
    edit_index: usize,
    /// The search/anchor text that was not found (truncated for display).
    search_text: String,
    /// Human-readable reason why the edit was skipped.
    reason: String,
}

/// Result of editing a single file's content (in-memory, before writing).
struct EditResult {
    modified_content: String,
    applied: usize,
    total_replacements: usize,
    skipped_details: Vec<SkippedEditDetail>,
    warnings: Vec<String>,
    diff: String,
    lines_added: i64,
    lines_removed: i64,
    new_line_count: usize,
    /// Post-write structural sanity warnings about asymmetric brace deltas.
    /// One entry per disagreeing pair class (`{}`, `()`, `[]`). Empty in the
    /// common case. See `brace_balance_warnings` for the heuristic.
    brace_balance_warnings: Vec<String>,
}

/// Read and validate a file, returning its content and line ending style.
/// Returns `(resolved_path, normalized_content, line_ending, file_existed_before_edit)`.
///
/// `file_existed_before_edit` is computed BEFORE any directory creation or write —
/// this lets the caller distinguish "edited an existing file" from "created a new file"
/// reliably (without relying on `normalized.is_empty()` which is also true for
/// existing-but-empty files).
///
/// `dry_run` skips parent-directory creation for non-existent files. Without this,
/// a `dryRun=true` preview against a non-existent path leaves empty parent
/// directories on disk — violating the "preview without writing" contract (EDIT-003).
fn read_and_validate_file(server_dir: &str, path_str: &str, dry_run: bool) -> Result<(PathBuf, String, &'static str, bool), String> {
    let resolved = resolve_path(server_dir, path_str);
    let file_existed = resolved.exists();
    if !file_existed {
        // File doesn't exist — treat as empty (allows creation via insert operations).
        // EDIT-003: only create parent dirs on a real write. dryRun must be a pure preview.
        if !dry_run
            && let Some(parent) = resolved.parent()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directories for '{}': {}", path_str, e))?;
        }
        return Ok((resolved, String::new(), "\n", false));
    }
    if resolved.is_dir() {
        return Err(format!("Path is a directory, not a file: {}", path_str));
    }

    let raw_bytes = std::fs::read(&resolved)
        .map_err(|e| format!("Failed to read file '{}': {}", path_str, e))?;

    // Binary detection: check for null bytes in first 8KB
    let check_len = raw_bytes.len().min(8192);
    if raw_bytes[..check_len].contains(&0) {
        return Err(format!("Binary file detected, not editable: {}", path_str));
    }

    // EDIT-004: strict UTF-8. Lossy fallback (replacing invalid bytes with U+FFFD)
    // would silently corrupt non-UTF-8 source files (Windows-1251, Shift-JIS,
    // GB2312, Latin-1) on the next write. Refuse to edit instead — preserves
    // original bytes and surfaces the encoding mismatch to the caller.
    let content = match std::str::from_utf8(&raw_bytes) {
        Ok(s) => s.to_string(),
        Err(e) => {
            return Err(format!(
                "File '{}' is not valid UTF-8 (invalid byte at offset {}): refuse to edit to avoid silent corruption.",
                path_str, e.valid_up_to()
            ));
        }
    };

    let line_ending = detect_line_ending(&content);

    // Normalize to LF for processing
    let normalized = if line_ending == "\r\n" {
        content.replace("\r\n", "\n")
    } else {
        content
    };

    Ok((resolved, normalized, line_ending, true))
}

/// Sync-reindex eligibility check used after a successful edit/write.
///
/// Returns `Some("<reason>")` if the file should NOT be sync-reindexed, with a
/// human-readable reason that becomes `skippedReason` in the response. Returns
/// `None` if the file is eligible for sync reindex.
///
/// Decision logic (mirrors what the FS watcher does):
///   1. Outside server `--dir` → skip (not in our index scope).
///   2. Extension not in server `--ext` → skip (matches watcher filter).
///   3. Inside any `.git/` directory → skip (matches watcher filter).
fn classify_for_sync_reindex(
    canonical_server_dir: &str,
    server_extensions: &[String],
    resolved: &Path,
) -> Option<&'static str> {
    // 1. Outside server_dir — most common skip reason for cross-project edits.
    //    Uses `code_xray::is_path_within`, which performs a logical-path comparison
    //    first (matching what the indexer sees via `WalkBuilder::follow_links`).
    //    Without this, files in a symlinked subdirectory like `docs/personal`
    //    (target outside the workspace) would be wrongly classified as
    //    `outsideServerDir`, because plain `canonicalize()` resolves the symlink.
    let resolved_str = resolved.to_string_lossy();
    if !crate::is_path_within(&resolved_str, canonical_server_dir) {
        return Some("outsideServerDir");
    }
    // 2. Extension filter — server only indexes a subset of extensions.
    if !crate::mcp::watcher::matches_extensions(resolved, server_extensions) {
        return Some("extensionNotIndexed");
    }
    // 3. .git internals — never indexed.
    if crate::mcp::watcher::is_inside_git_dir(resolved) {
        return Some("insideGitDir");
    }
    None
}

/// Count contentful lines in `s` using the same convention editors and
/// `xray_definitions`/`xray_grep` use for 1-based line numbers:
///   - empty file → 0
///   - trailing `\n` is a line terminator, not an extra empty line
///
/// Examples: `""` → 0, `"a"` → 1, `"a\n"` → 1, `"a\nb"` → 2, `"a\nb\n"` → 2.
///
/// This matches the maximum line number an LLM can ever observe via xray
/// read tools, so values it passes back as `expectedLineCount` line up.
fn count_lines(s: &str) -> usize {
    if s.is_empty() {
        0
    } else {
        s.split('\n').count() - usize::from(s.ends_with('\n'))
    }
}

/// Apply edits/operations to file content and return results.
///
/// `file_existed` distinguishes "this is a freshly-created file" from
/// "this is an edit on an existing (possibly empty) file". The flag is
/// only consumed by the brace-balance sanity check, which must fire on
/// existing-but-empty files (an unbalanced INSERT into a 0-byte file is
/// just as broken as into a non-empty one) but stay silent on auto-create
/// (the whole content is "new", delta has no meaning there).
fn apply_edits_to_content(
    path_str: &str,
    normalized: &str,
    mode: &EditMode<'_>,
    is_regex: bool,
    expected_line_count: Option<usize>,
    file_existed: bool,
) -> Result<EditResult, String> {
    // expectedLineCount safety check — applies to BOTH modes.
    // Previously this lived inside the Mode A (Operations) arm only, so
    // for Mode B (text-match edits) the parameter was silently ignored.
    if let Some(expected) = expected_line_count {
        let actual = count_lines(normalized);
        if actual != expected {
            return Err(format!(
                "Expected {} lines, file has {}. File may have changed.",
                expected, actual
            ));
        }
    }

    let (modified_content, applied, total_replacements, skipped_details, warnings) = match mode {
        EditMode::Operations(ops_array) => {
            // Mode A: Line-range operations
            let ops = parse_line_operations(ops_array)?;

            let lines: Vec<&str> = normalized.split('\n').collect();

            let (new_lines, applied_count) = apply_line_operations(&lines, ops)?;
            (new_lines.join("\n"), applied_count, 0, Vec::new(), Vec::new())
        }
        EditMode::Edits(edits_array) => {
            // Mode B: Text-match edits
            let text_edits = parse_text_edits(edits_array)?;

            let (new_content, replacements, skipped, edit_warnings) = apply_text_edits(normalized, &text_edits, is_regex)?;
            // `applied` must exclude edits that were skipped via `skipIfNotFound`
            // or deduplicated via idempotency check. Previously this counted every
            // entry in the edits array, which made the response claim success for
            // edits that never touched the file.
            let applied_count = text_edits.len().saturating_sub(skipped.len());
            (new_content, applied_count, replacements, skipped, edit_warnings)
        }
    };

    // Generate unified diff
    let diff = generate_unified_diff(path_str, normalized, &modified_content);

    // Count changes — use the same human-line semantics as xray read tools so
    // `new_line_count` in the response can be reused as `expectedLineCount`
    // for the next edit without an off-by-one.
    let original_line_count = count_lines(normalized);
    let new_line_count = count_lines(&modified_content);
    let lines_delta = new_line_count as i64 - original_line_count as i64;
    let lines_removed = if lines_delta < 0 { -lines_delta } else { 0 };
    let lines_added = if lines_delta > 0 { lines_delta } else { 0 };

    // Post-write structural sanity: asymmetric brace deltas surface the
    // "search included a closer that replace omitted" failure class
    // (commit a823c57 had exactly this bug — caught only by `cargo build`
    // 30s after the write). Skip ONLY on auto-create: a freshly-minted
    // file has no "original" to delta against. Existing-but-empty files
    // still get the check — an unbalanced INSERT into a 0-byte file is
    // just as broken as one into a non-empty file.
    let brace_balance_warnings = if file_existed && !is_prose_extension(path_str) {
        brace_balance_warnings(normalized, &modified_content)
    } else {
        Vec::new()
    };

    Ok(EditResult {
        modified_content,
        applied,
        total_replacements,
        skipped_details,
        warnings,
        diff,
        lines_added,
        lines_removed,
        new_line_count,
        brace_balance_warnings,
    })
}

/// Write modified content back to file, restoring original line endings.
fn write_file_with_endings(resolved: &Path, content: &str, line_ending: &str) -> Result<(), String> {
    use std::io::Write;

    let output = if line_ending == "\r\n" {
        content.replace('\n', "\r\n")
    } else {
        content.to_string()
    };

    // Atomic write: stage into a sibling temp file, fsync, then rename over the target.
    // Plain `std::fs::write` does open(O_TRUNC) + write_all, leaving the file empty/partial
    // if the process is killed between those two steps. The rename below is atomic on POSIX
    // and best-effort-atomic on Windows (see `rename_replace` for the remove+rename fallback).
    let tmp = temp_path_for(resolved);

    let staged = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(output.as_bytes())?;
        f.sync_all()?;
        Ok(())
    })();

    if let Err(e) = staged {
        let _ = std::fs::remove_file(&tmp); // best-effort cleanup
        return Err(format!("Failed to write file: {}", e));
    }

    if let Err(e) = rename_replace(&tmp, resolved) {
        let _ = std::fs::remove_file(&tmp); // best-effort cleanup
        return Err(e);
    }

    // Crash-durability: fsync the parent directory so the rename itself is
    // persisted before we report success. Without this, a power loss between
    // the rename and the next implicit metadata flush can leave the directory
    // entry pointing at the old inode (or no entry at all on a fresh create).
    // POSIX-only — Windows has no equivalent operation, and `std::fs::File`
    // refuses to open directories there. Best-effort: a failed parent fsync
    // does not invalidate the write itself, so we swallow the error.
    #[cfg(unix)]
    {
        if let Some(parent) = resolved.parent()
            && let Ok(dir) = std::fs::File::open(parent)
        {
            let _ = dir.sync_all();
        }
    }

    Ok(())
}

/// Re-read the file after a write and verify its bytes match what `write_file_with_endings`
/// would have produced from (`expected_lf_content`, `line_ending`). Returns Ok on match,
/// Err with a diagnostic on mismatch. This guards the tool's core contract: the `diff`
/// returned in the response must correspond to bytes actually on disk. Any divergence
/// (rare: concurrent writer, antivirus truncation, filesystem quirk) is surfaced rather
/// than silently hidden under a misleading "applied: N" response.
fn verify_written_file(resolved: &Path, expected_lf_content: &str, line_ending: &str) -> Result<(), String> {
    let expected_bytes = if line_ending == "\r\n" {
        expected_lf_content.replace('\n', "\r\n").into_bytes()
    } else {
        expected_lf_content.as_bytes().to_vec()
    };
    let actual = std::fs::read(resolved)
        .map_err(|e| format!("Post-write verification: cannot re-read file: {}", e))?;
    if actual == expected_bytes {
        return Ok(());
    }
    // Find first differing byte for a useful diagnostic.
    let mismatch_at = actual.iter()
        .zip(expected_bytes.iter())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| actual.len().min(expected_bytes.len()));
    Err(format!(
        "Post-write verification failed: on-disk bytes do not match the computed diff. \
         File length expected {} bytes, got {} bytes; first difference at byte offset {}.",
        expected_bytes.len(), actual.len(), mismatch_at
    ))
}

/// Generate a per-call unique temp file path in the same directory as `target`.
///
/// EDIT-005: a deterministic name (e.g. `.{name}.xray_tmp`) collides between
/// concurrent `xray_edit` calls on the same file (LLM parallel tool-calls, two
/// MCP servers, agent-vs-formatter). Both `File::create` succeed (truncate),
/// both write, both rename → second rename overwrites first → silent lost write.
/// Including PID + nanosecond timestamp + atomic counter makes practical
/// collision impossible.
fn temp_path_for(target: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let file_name = target.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    target.with_file_name(format!(".{}.xray_tmp.{}.{}.{}", file_name, pid, nanos, counter))
}

/// Backup file path used by `rename_replace` to protect against the Windows
/// remove-then-rename data-loss window (EDIT-007).
fn backup_path_for(target: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let file_name = target.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    target.with_file_name(format!(".{}.xray_backup.{}.{}.{}", file_name, pid, nanos, counter))
}

/// Rename `src` to `dst`, replacing `dst` if it exists.
///
/// On Windows, `std::fs::rename` may fail when `dst` exists, so we fall back to
/// `remove(dst)` + `rename(src, dst)`. The naive fallback has a data-loss
/// window (EDIT-007): if `remove` succeeds but `rename` then fails (antivirus
/// holding a handle, OneDrive sync, transient I/O), the target is gone forever.
/// We mitigate by copying the original to a sibling backup first, restoring it
/// on rename failure. The backup is removed on success.
fn rename_replace(src: &Path, dst: &Path) -> Result<(), String> {
    // Try direct rename first (atomic on POSIX, sometimes works on Windows).
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) => {
            if !dst.exists() {
                return Err(format!("Cannot rename temp to '{}': {}", dst.display(), e));
            }
            // EDIT-007: stage a backup of the original before remove+rename.
            let backup = backup_path_for(dst);
            if let Err(e2) = std::fs::copy(dst, &backup) {
                return Err(format!(
                    "Cannot stage backup for '{}': {} (original error: {})",
                    dst.display(), e2, e
                ));
            }
            if let Err(e2) = std::fs::remove_file(dst) {
                let _ = std::fs::remove_file(&backup); // best-effort cleanup
                return Err(format!("Cannot remove original '{}': {}", dst.display(), e2));
            }
            match std::fs::rename(src, dst) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&backup); // best-effort cleanup
                    Ok(())
                }
                Err(e2) => {
                    // Restore the backup so the target is not lost.
                    if let Err(e3) = std::fs::rename(&backup, dst) {
                        // Last resort: copy back, then remove backup.
                        if let Err(e4) = std::fs::copy(&backup, dst) {
                            return Err(format!(
                                "Cannot rename temp to '{}': {} (original error: {}); \
                                 also failed to restore backup '{}': rename={}, copy={}",
                                dst.display(), e2, e, backup.display(), e3, e4
                            ));
                        }
                        let _ = std::fs::remove_file(&backup);
                    }
                    Err(format!(
                        "Cannot rename temp to '{}': {} (original error: {}); \
                         original file restored from backup.",
                        dst.display(), e2, e
                    ))
                }
            }
        }
    }
}

/// Handle single-file edit (original behavior).
fn handle_single_file_edit(
    ctx: &HandlerContext,
    path_str: &str,
    mode: &EditMode<'_>,
    is_regex: bool,
    dry_run: bool,
    expected_line_count: Option<usize>,
) -> ToolCallResult {
    // Read and validate. `file_existed` is captured BEFORE the write so we can
    // accurately set `fileCreated` and `fileListInvalidated` in the response.
    // Pass dry_run so a preview against a non-existent path doesn't create empty
    // parent directories (EDIT-003).
    let (resolved, normalized, line_ending, file_existed) = match read_and_validate_file(&ctx.server_dir(), path_str, dry_run) {
        Ok(r) => r,
        Err(e) => return ToolCallResult::error(e),
    };
    let file_created = !file_existed;

    // Apply edits
    let edit_result = match apply_edits_to_content(path_str, &normalized, mode, is_regex, expected_line_count, file_existed) {
        Ok(r) => r,
        Err(e) => return ToolCallResult::error(e),
    };

    // Write file (unless dryRun)
    if !dry_run
        && let Err(e) = write_file_with_endings(&resolved, &edit_result.modified_content, line_ending) {
            return ToolCallResult::error(e);
        }

    // Post-write verification: re-read the file and confirm its bytes match what
    // we intended to write. This enforces the tool's "diff ↔ content" guarantee.
    if !dry_run
        && let Err(e) = verify_written_file(&resolved, &edit_result.modified_content, line_ending) {
            return ToolCallResult::error(e);
        }

    // Build response
    let mut response = json!({
        "path": path_str,
        "applied": edit_result.applied,
        "linesAdded": edit_result.lines_added,
        "linesRemoved": edit_result.lines_removed,
        "newLineCount": edit_result.new_line_count,
        "dryRun": dry_run,
        // Fix 4: expose line ending so clients can reconcile tool-diff (LF) with
        // on-disk bytes (LF or CRLF). Prevents "diff disagrees with git diff" confusion.
        "lineEnding": if line_ending == "\r\n" { "CRLF" } else { "LF" },
        // INSERT-after-EOF idiom hint: agents can read these values directly
        // from a previous response instead of guessing from stale state.
        "appendIdiom": {
            "startLine": edit_result.new_line_count + 1,
            "endLine": edit_result.new_line_count,
        },
    });

    if edit_result.total_replacements > 0 {
        response["totalReplacements"] = json!(edit_result.total_replacements);
    }

    if !edit_result.skipped_details.is_empty() {
        response["skippedEdits"] = json!(edit_result.skipped_details.len());
        response["skippedDetails"] = json!(edit_result.skipped_details.iter().map(|s| {
            json!({
                "editIndex": s.edit_index,
                "search": s.search_text,
                "reason": s.reason,
            })
        }).collect::<Vec<_>>());
    }

    if !edit_result.warnings.is_empty() {
        response["warnings"] = json!(edit_result.warnings);
    }

    if !edit_result.brace_balance_warnings.is_empty() {
        response["braceBalanceWarnings"] = json!(edit_result.brace_balance_warnings);
    }

    if !edit_result.diff.is_empty() {
        response["diff"] = json!(edit_result.diff);
    } else {
        response["diff"] = json!("(no changes)");
    }

    if file_created {
        response["fileCreated"] = json!(true);
    }

    // ── Synchronous reindex (only on real writes, never on dryRun) ──
    // Eliminates the 500ms FS-watcher debounce window so a follow-up xray_grep
    // or xray_definitions sees the new content immediately.
    if !dry_run {
        let server_extensions: Vec<String> = ctx.server_ext.split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        match classify_for_sync_reindex(&ctx.canonical_server_dir(), &server_extensions, &resolved) {
            Some(reason) => {
                response["contentIndexUpdated"] = json!(false);
                response["defIndexUpdated"] = json!(false);
                response["fileListInvalidated"] = json!(false);
                response["skippedReason"] = json!(reason);
            }
            None => {
                let stats = crate::mcp::watcher::reindex_paths_sync(
                    &ctx.index,
                    &ctx.def_index,
                    std::slice::from_ref(&resolved),
                    &[],
                    &server_extensions,
                );
                response["contentIndexUpdated"] = json!(stats.content_updated > 0);
                response["defIndexUpdated"] = json!(stats.def_updated > 0);
                response["reindexElapsedMs"] = json!(format!("{:.2}", stats.elapsed_ms));
                if stats.content_lock_poisoned || stats.def_lock_poisoned {
                    response["reindexWarning"] = json!(
                        "Index lock was poisoned — sync reindex partially failed; FS watcher will reconcile within 500ms."
                    );
                }
                // New file → invalidate file-list cache (xray_fast).
                if file_created {
                    ctx.file_index_dirty.store(true, std::sync::atomic::Ordering::Relaxed);
                    response["fileListInvalidated"] = json!(true);
                } else {
                    response["fileListInvalidated"] = json!(false);
                }
            }
        }
    }

    ToolCallResult::success(json_to_string(&response))
}

/// Handle multi-file edit with transactional semantics (all-or-nothing).
fn handle_multi_file_edit(
    ctx: &HandlerContext,
    paths_array: &[Value],
    mode: &EditMode<'_>,
    is_regex: bool,
    dry_run: bool,
    expected_line_count: Option<usize>,
) -> ToolCallResult {
    // Validate paths array
    if paths_array.is_empty() {
        return ToolCallResult::error("'paths' array must not be empty.".to_string());
    }
    if paths_array.len() > MAX_MULTI_FILE_PATHS {
        return ToolCallResult::error(format!(
            "'paths' array has {} entries, maximum is {}.",
            paths_array.len(), MAX_MULTI_FILE_PATHS
        ));
    }

    // Parse path strings
    let path_strings: Vec<&str> = match paths_array.iter()
        .enumerate()
        .map(|(i, v)| v.as_str().ok_or_else(|| format!("paths[{}]: expected string", i)))
        .collect::<Result<Vec<&str>, String>>() {
        Ok(ps) => ps,
        Err(e) => return ToolCallResult::error(e),
    };

    // Phase 1: Read all files (with duplicate path detection).
    // We carry `file_existed` through the pipeline so each per-file response
    // can correctly report `fileCreated` and `fileListInvalidated`.
    let mut file_data: Vec<(&str, PathBuf, String, &'static str, bool)> = Vec::with_capacity(path_strings.len());
    let mut seen_paths: HashSet<PathBuf> = HashSet::with_capacity(path_strings.len());
    for path_str in &path_strings {
        match read_and_validate_file(&ctx.server_dir(), path_str, dry_run) {
            Ok((resolved, normalized, line_ending, file_existed)) => {
                // Normalize path to handle ./file.txt vs file.txt
                let normalized_path: PathBuf = resolved.components().collect();
                if !seen_paths.insert(normalized_path.clone()) {
                    // Find the original path string that resolved to the same file
                    let original = file_data.iter()
                        .find(|(_, r, _, _, _)| {
                            let nr: PathBuf = r.components().collect();
                            nr == normalized_path
                        })
                        .map(|(p, _, _, _, _)| *p)
                        .unwrap_or("?");
                    return ToolCallResult::error(format!(
                        "Duplicate path: '{}' and '{}' resolve to the same file",
                        original, path_str
                    ));
                }
                file_data.push((path_str, resolved, normalized, line_ending, file_existed));
            }
            Err(e) => return ToolCallResult::error(format!("File '{}': {}", path_str, e)),
        }
    }

    // Phase 2: Apply edits to all (in memory)
    let mut edit_results: Vec<(&str, PathBuf, EditResult, &'static str, bool)> = Vec::with_capacity(file_data.len());
    for (path_str, resolved, normalized, line_ending, file_existed) in file_data {
        match apply_edits_to_content(path_str, &normalized, mode, is_regex, expected_line_count, file_existed) {
            Ok(result) => {
                edit_results.push((path_str, resolved, result, line_ending, file_existed));
            }
            Err(e) => return ToolCallResult::error(format!("File '{}': {}", path_str, e)),
        }
    }

    // Phase 3: Write all (only if !dry_run) — atomic multi-file via temp+rename
    if !dry_run {
        // Phase 3a: Write to temp files (validates I/O before touching originals)
        let mut temp_files: Vec<(&str, PathBuf, PathBuf)> = Vec::with_capacity(edit_results.len());
        for (path_str, resolved, result, line_ending, _file_existed) in &edit_results {
            let temp = temp_path_for(resolved);
            if let Err(e) = write_file_with_endings(&temp, &result.modified_content, line_ending) {
                // Clean up temp files already written
                for (_, _, tp) in &temp_files {
                    let _ = std::fs::remove_file(tp);
                }
                return ToolCallResult::error(format!("File '{}': {}", path_str, e));
            }
            temp_files.push((path_str, resolved.clone(), temp));
        }

        // Phase 3b: Rename temp files to targets (fast, unlikely to fail)
        for (renamed, (path_str, resolved, temp)) in temp_files.iter().enumerate() {
            if let Err(e) = rename_replace(temp, resolved) {
                // Best-effort cleanup of remaining temp files
                for (_, _, tp) in &temp_files[renamed..] {
                    let _ = std::fs::remove_file(tp);
                }
                // Build the list of files that already committed (renames
                // succeeded for indices 0..renamed). These cannot be rolled back
                // by this tool — surface them so the caller can `git restore`.
                let committed_files: Vec<String> = temp_files[..renamed]
                    .iter()
                    .map(|(p, _, _)| (*p).to_string())
                    .collect();
                let committed_json = serde_json::to_string(&committed_files)
                    .unwrap_or_else(|_| "[]".to_string());
                return ToolCallResult::error(format!(
                    "File '{}': rename failed after {} of {} files committed: {}. \
                     Already-committed files cannot be rolled back. committedFiles: {}",
                    path_str, renamed, temp_files.len(), e, committed_json
                ));
            }
        }

        // Phase 3c-pre: Post-write verification across the whole batch.
        // A failure here means an on-disk file diverged from the diff we are about
        // to return — surface it before the response goes out.
        for (path_str, resolved, result, line_ending, _) in &edit_results {
            if let Err(e) = verify_written_file(resolved, &result.modified_content, line_ending) {
                return ToolCallResult::error(format!("File '{}': {}", path_str, e));
            }
        }
    }

    // Phase 3c: Sync reindex of all written files (only on real writes).
    // Batched into ONE call so the inverted-index write lock is held for ~1ms
    // for the whole batch, not N times. Outside-server-dir / wrong-ext / .git
    // files are filtered per-file and reported via per-file `skippedReason`.
    let server_extensions: Vec<String> = ctx.server_ext.split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let canonical_root = ctx.canonical_server_dir();
    let mut per_file_skip: Vec<Option<&'static str>> = Vec::with_capacity(edit_results.len());
    let mut eligible_paths: Vec<PathBuf> = Vec::new();
    let mut any_file_created_eligible = false;
    if !dry_run {
        for (_, resolved, _, _, file_existed) in &edit_results {
            let skip = classify_for_sync_reindex(&canonical_root, &server_extensions, resolved);
            if skip.is_none() {
                eligible_paths.push(resolved.clone());
                if !*file_existed { any_file_created_eligible = true; }
            }
            per_file_skip.push(skip);
        }
    } else {
        for _ in &edit_results { per_file_skip.push(None); }
    }
    let batch_stats = if !dry_run && !eligible_paths.is_empty() {
        Some(crate::mcp::watcher::reindex_paths_sync(
            &ctx.index,
            &ctx.def_index,
            &eligible_paths,
            &[],
            &server_extensions,
        ))
    } else {
        None
    };
    if !dry_run && any_file_created_eligible {
        ctx.file_index_dirty.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    // Phase 4: Build response with per-file results
    let mut total_applied: usize = 0;
    let mut results_array = Vec::new();
    for (i, (path_str, _, result, line_ending, file_existed)) in edit_results.iter().enumerate() {
        total_applied += result.applied;
        let mut file_result = json!({
            "path": path_str,
            "applied": result.applied,
            "linesAdded": result.lines_added,
            "linesRemoved": result.lines_removed,
            "newLineCount": result.new_line_count,
            "lineEnding": if *line_ending == "\r\n" { "CRLF" } else { "LF" },
            // INSERT-after-EOF idiom hint: agents can read these values directly
            // from a previous response instead of guessing from stale state.
            "appendIdiom": {
                "startLine": result.new_line_count + 1,
                "endLine": result.new_line_count,
            },
        });
        if result.total_replacements > 0 {
            file_result["totalReplacements"] = json!(result.total_replacements);
        }
        if !result.skipped_details.is_empty() {
            file_result["skippedEdits"] = json!(result.skipped_details.len());
            file_result["skippedDetails"] = json!(result.skipped_details.iter().map(|s| {
                json!({
                    "editIndex": s.edit_index,
                    "search": s.search_text,
                    "reason": s.reason,
                })
            }).collect::<Vec<_>>());
        }
        if !result.warnings.is_empty() {
            file_result["warnings"] = json!(result.warnings);
        }
        if !result.brace_balance_warnings.is_empty() {
            file_result["braceBalanceWarnings"] = json!(result.brace_balance_warnings);
        }
        if !result.diff.is_empty() {
            file_result["diff"] = json!(result.diff);
        } else {
            file_result["diff"] = json!("(no changes)");
        }
        if !*file_existed {
            file_result["fileCreated"] = json!(true);
        }
        // Per-file sync-reindex outcome.
        if !dry_run {
            match per_file_skip[i] {
                Some(reason) => {
                    file_result["contentIndexUpdated"] = json!(false);
                    file_result["defIndexUpdated"] = json!(false);
                    file_result["fileListInvalidated"] = json!(false);
                    file_result["skippedReason"] = json!(reason);
                }
                None => {
                    // Mirror the single-file path: derive `contentIndexUpdated`
                    // from the actual batch outcome, NOT from "we tried". When
                    // the content-index lock was poisoned, `reindex_paths_sync`
                    // returns `content_updated == 0` and sets
                    // `content_lock_poisoned = true` (surfaced via the
                    // batch-level `summary.reindexWarning`). Reporting `true`
                    // here would tell the caller "your edit landed in the
                    // index" while the warning says the opposite — caller-side
                    // staleness checks (e.g. follow-up `xray_grep` looking for
                    // the new symbol) would then be ignored.
                    file_result["contentIndexUpdated"] = json!(
                        batch_stats.as_ref().is_some_and(|s| s.content_updated > 0)
                    );
                    file_result["defIndexUpdated"] = json!(
                        ctx.def_index.is_some()
                            && batch_stats.as_ref().is_some_and(|s| s.def_updated > 0)
                    );
                    file_result["fileListInvalidated"] = json!(!*file_existed);
                }
            }
        }
        results_array.push(file_result);
    }

    let mut summary = json!({
        "filesEdited": edit_results.len(),
        "totalApplied": total_applied,
        "dryRun": dry_run,
    });
    if let Some(stats) = batch_stats {
        summary["reindexElapsedMs"] = json!(format!("{:.2}", stats.elapsed_ms));
        if stats.content_lock_poisoned || stats.def_lock_poisoned {
            summary["reindexWarning"] = json!(
                "Index lock was poisoned — sync reindex partially failed; FS watcher will reconcile within 500ms."
            );
        }
    }

    let response = json!({
        "results": results_array,
        "summary": summary,
    });

    ToolCallResult::success(json_to_string(&response))
}

// ─── Path resolution ─────────────────────────────────────────────────

/// Resolve a file path: if absolute, use as-is; if relative, resolve from server_dir.
fn resolve_path(server_dir: &str, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        Path::new(server_dir).join(p)
    }
}

// ─── Line ending detection ───────────────────────────────────────────

/// Detect whether the file uses CRLF or LF line endings.
/// Returns "\r\n" for CRLF, "\n" for LF (default).
fn detect_line_ending(content: &str) -> &'static str {
    // Count CRLF vs bare LF
    let crlf_count = content.matches("\r\n").count();
    let lf_count = content.matches('\n').count();
    // bare LF = total LF - CRLF (each \r\n contains one \n)
    let bare_lf_count = lf_count - crlf_count;

    if crlf_count > bare_lf_count {
        "\r\n"
    } else {
        "\n"
    }
}

// ─── Mode A: Line-range operations ───────────────────────────────────

struct LineOperation {
    start_line: usize, // 1-based
    end_line: usize,   // 1-based, inclusive
    content: String,
}

fn parse_line_operations(ops_array: &[Value]) -> Result<Vec<LineOperation>, String> {
    let mut ops = Vec::with_capacity(ops_array.len());
    for (i, op) in ops_array.iter().enumerate() {
        let start_line = op.get("startLine")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("operations[{}]: missing or invalid 'startLine'", i))? as usize;
        let end_line = op.get("endLine")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("operations[{}]: missing or invalid 'endLine'", i))? as usize;
        let content = op.get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("operations[{}]: missing or invalid 'content'", i))?
            .to_string();
        let content = normalize_crlf(&content);

        if start_line == 0 {
            return Err(format!("operations[{}]: startLine must be >= 1", i));
        }

        ops.push(LineOperation { start_line, end_line, content });
    }
    Ok(ops)
}

/// Apply line-range operations bottom-up. Returns (new_lines, applied_count).
fn apply_line_operations(lines: &[&str], ops: Vec<LineOperation>) -> Result<(Vec<String>, usize), String> {
    // Mode A addressable line count must match `count_lines()` / human-editor
    // semantics so it agrees with `expectedLineCount` and `newLineCount`. The
    // raw `lines.len()` from `split('\n')` over-counts the trailing-newline
    // sentinel: `""` splits to `[""]` (len 1), `"\n"` splits to `["", ""]`
    // (len 2), `"a\n"` splits to `["a", ""]` (len 2). In every case where the
    // last element is empty AND the file is non-empty, that empty element is
    // the sentinel after the final `\n`, not an addressable line.
    //
    // Without this normalization a blank-line-only file (`"\n"`/`"\r\n"`) is
    // reported as `originalLineCount: 1` but `apply_line_operations` would
    // treat it as 2 addressable lines — REPLACE 1..2 / DELETE 2 would silently
    // succeed against a phantom second line. The earlier carve-out for `[""]`
    // closed only the empty-file case; this generalization closes the
    // blank-line-only case (and all higher line counts have always been
    // off-by-one in the same direction).
    //
    // INSERT mode (`startLine: 1, endLine: 0`) remains the canonical form for
    // writing into an empty/auto-created file and is unaffected.
    let line_count = if lines.last() == Some(&"") {
        lines.len().saturating_sub(1)
    } else {
        lines.len()
    };

    // Validate ranges
    for op in &ops {
        // For insert mode (endLine < startLine), startLine can be line_count + 1 (append after last line)
        if op.end_line >= op.start_line {
            // Replace/delete mode
            if line_count == 0 {
                return Err(format!(
                    "Cannot {} lines in empty file (0 lines). \
                     To write content into an empty (or auto-created) file, use INSERT mode: \
                     startLine: 1, endLine: 0, content: '...'",
                    if op.content.is_empty() { "delete" } else { "replace" }
                ));
            }
            if op.start_line > line_count {
                return Err(format!(
                    "startLine {} out of range (file has {} lines). \
                     To append after the last line, pass startLine: {}, endLine: {} \
                     (INSERT mode at line N+1). \
                     To replace the last line, pass startLine: {}, endLine: {}.",
                    op.start_line,
                    line_count,
                    line_count + 1,
                    line_count,
                    line_count,
                    line_count,
                ));
            }
            if op.end_line > line_count {
                return Err(format!(
                    "endLine {} out of range (file has {} lines). \
                     To append after the last line, pass startLine: {}, endLine: {} \
                     (INSERT mode at line N+1).",
                    op.end_line,
                    line_count,
                    line_count + 1,
                    line_count,
                ));
            }
        } else {
            // Insert mode: startLine can be 1..=line_count+1
            if op.start_line > line_count + 1 {
                return Err(format!(
                    "startLine {} out of range for insert (file has {} lines, max insert position is {}). \
                     To append to end of file, pass startLine: {}, endLine: {} (INSERT mode at line N+1)",
                    op.start_line,
                    line_count,
                    line_count + 1,
                    line_count + 1,
                    line_count
                ));
            }
        }
    }

    // Sort by startLine descending (bottom-up)
    let mut sorted_ops: Vec<&LineOperation> = ops.iter().collect();
    sorted_ops.sort_by_key(|b| std::cmp::Reverse(b.start_line));

    // Check overlaps (after sorting descending)
    // sorted_ops[0] has highest startLine, sorted_ops[last] has lowest
    for i in 0..sorted_ops.len().saturating_sub(1) {
        let higher = sorted_ops[i];   // higher startLine
        let lower = sorted_ops[i + 1]; // lower startLine

        // Skip overlap check for insert operations (endLine < startLine)
        if higher.end_line < higher.start_line || lower.end_line < lower.start_line {
            continue;
        }

        // Check: the lower operation's endLine must be < higher operation's startLine
        if lower.end_line >= higher.start_line {
            return Err(format!(
                "Operations overlap at lines {}-{}",
                lower.start_line, higher.end_line
            ));
        }
    }

    let mut result: Vec<String> = lines.iter().map(|s| s.to_string()).collect();

    for op in &sorted_ops {
        let start = op.start_line - 1; // 0-based

        if op.end_line < op.start_line {
            // INSERT mode: insert content before startLine
            if op.content.is_empty() {
                continue; // empty insert = no-op
            }
            let new_lines: Vec<String> = op.content.split('\n').map(String::from).collect();
            for (i, line) in new_lines.iter().enumerate() {
                result.insert(start + i, line.clone());
            }
        } else if op.content.is_empty() {
            // DELETE mode: remove lines startLine..=endLine
            let end = op.end_line; // exclusive end for drain = endLine (1-based = 0-based + 1)
            result.drain(start..end);
        } else {
            // REPLACE mode: replace lines startLine..=endLine with content
            let end = op.end_line; // exclusive end for splice
            let new_lines: Vec<String> = op.content.split('\n').map(String::from).collect();
            result.splice(start..end, new_lines);
        }
    }

    Ok((result, ops.len()))
}

// ─── Mode B: Text-match edits ────────────────────────────────────────

/// Represents a single text edit operation.
/// Supports two modes:
/// - Search/replace: find `search` text and replace with `replace`
/// - Insert after/before: find anchor text and insert `content` after/before it
struct TextEdit {
    /// Text to search for (literal or regex). Used in search/replace mode.
    search: Option<String>,
    /// Replacement text. Used in search/replace mode.
    replace: Option<String>,
    /// Which occurrence to target. 0 = all occurrences.
    occurrence: usize,
    /// Anchor text to insert AFTER. Mutually exclusive with search/replace.
    insert_after: Option<String>,
    /// Anchor text to insert BEFORE. Mutually exclusive with search/replace.
    insert_before: Option<String>,
    /// Content to insert (used with insert_after/insert_before).
    content: Option<String>,
    /// Expected context near the search/anchor text (±5 lines). Safety check.
    expected_context: Option<String>,
    /// If true, skip this edit silently when search/anchor text is not found (instead of returning error).
    /// Useful with multi-file `paths` where not all files contain the target text.
    skip_if_not_found: bool,
}

/// Normalize CRLF line endings to LF in a string.
/// This ensures search text from JSON input matches LF-normalized file content.
fn normalize_crlf(s: &str) -> String {
    if s.contains("\r\n") {
        s.replace("\r\n", "\n")
    } else {
        s.to_string()
    }
}

/// Collapse runs of horizontal whitespace (spaces/tabs) to a single space per line.
/// Used for flexible whitespace comparison in expectedContext checks.
fn collapse_spaces(s: &str) -> String {
    s.lines()
        .map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            parts.join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Convert a literal search string to a flex-space regex pattern.
/// Each whitespace gap between non-whitespace tokens becomes `[ \t]+`,
/// and leading/trailing whitespace per line becomes `[ \t]*`.
/// This allows matching text with different amounts of horizontal whitespace.
///
/// Markdown table separator lines (e.g., `|---|---|`) are detected and handled
/// specially: dash counts become flexible while preserving column structure.
///
/// Example: `"| Issue | Count |"` becomes a pattern matching `"| Issue       | Count     |"`
/// Example: `"|---|---|"` matches `"|---------|-------------|"` (any dash count)
fn search_to_flex_pattern(search: &str) -> Option<String> {
    let lines: Vec<&str> = search.split('\n').collect();
    let mut pattern_parts: Vec<String> = Vec::new();
    let mut has_content = false;

    for line in &lines {
        if is_markdown_table_separator(line) {
            // Markdown table separator: flex dash/colon counts, preserve pipe structure
            has_content = true;
            let sep_pattern = flex_pattern_for_separator(line);
            pattern_parts.push(format!("[ \\t]*{}[ \\t]*", sep_pattern));
        } else {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.is_empty() {
                // Empty line: match zero or more horizontal whitespace
                pattern_parts.push("[ \\t]*".to_string());
            } else {
                has_content = true;
                let escaped_parts: Vec<String> = parts.iter()
                    .map(|p| regex::escape(p))
                    .collect();
                let flexed = escaped_parts.join("[ \\t]+");
                pattern_parts.push(format!("[ \\t]*{}[ \\t]*", flexed));
            }
        }
    }

    if !has_content {
        return None; // All-whitespace search — don't flex-match
    }

    Some(pattern_parts.join("\n"))
}

/// Check if a line is a markdown table separator (e.g., `|---|---|`, `|:---:|---:|`).
/// Returns true if all characters are from `{|, -, –, —, :, space, tab}` and the line
/// contains both at least one `|` and at least one dash-like character.
/// Recognizes en dash (–, U+2013) and em dash (—, U+2014) in addition to
/// hyphen-minus (-) to handle LLM/auto-formatting substitutions.
fn is_markdown_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let has_pipe = trimmed.contains('|');
    let has_dash = trimmed.chars().any(|c| matches!(c, '-' | '\u{2013}' | '\u{2014}'));
    let all_valid = trimmed.chars().all(|c| matches!(c, '|' | '-' | '\u{2013}' | '\u{2014}' | ':' | ' ' | '\t'));
    has_pipe && has_dash && all_valid
}

/// Generate a flex regex pattern for a markdown table separator line.
/// Preserves column count (number of `|` separators) but allows variable
/// dash/colon/space counts between pipes.
/// The character class includes en dash (–) and em dash (—) to handle
/// LLM/auto-formatting substitutions.
///
/// Example: `"|---------|-------------|"` → `"\\|[-–—: \\t]+\\|[-–—: \\t]+\\""`
fn flex_pattern_for_separator(line: &str) -> String {
    // Character class: hyphen-minus, en dash, em dash, colon, space, tab
    const DASH_CLASS: &str = "[-\\u{2013}\\u{2014}: \\t]+";

    let mut result = String::new();
    let mut in_dash_run = false;

    for ch in line.chars() {
        if ch == '|' {
            if in_dash_run {
                result.push_str(DASH_CLASS);
                in_dash_run = false;
            }
            result.push_str("\\|");
        } else if matches!(ch, '-' | '\u{2013}' | '\u{2014}' | ':' | ' ' | '\t') {
            in_dash_run = true;
            // Accumulate — will emit dash class when we hit next pipe or end
        }
    }
    // Handle trailing dash run (for lines like "---|---" without trailing pipe)
    if in_dash_run {
        result.push_str(DASH_CLASS);
    }

    result
}

/// Describe a byte for diagnostic messages (hex + human-readable name).
fn describe_byte(b: u8) -> String {
    match b {
        b' ' => format!("0x{:02X} (space)", b),
        b'\t' => format!("0x{:02X} (tab)", b),
        b'\n' => format!("0x{:02X} (newline)", b),
        b'\r' => format!("0x{:02X} (carriage return)", b),
        0xC2 => format!("0x{:02X} (possible non-breaking space start)", b),
        b if b.is_ascii_graphic() => format!("0x{:02X} ('{}')", b, b as char),
        b => format!("0x{:02X}", b),
    }
}

/// Type name of a JSON value for diagnostic messages — `"number"`, `"string"`,
/// `"boolean"`, `"null"`, `"array"`, `"object"`.
fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Read a string field from an `edits[]` item. Returns `Ok(None)` if absent,
/// `Ok(Some(s))` if present and a string, `Err` if present but a different
/// type. Distinguishing "missing" from "wrong type" prevents the misleading
/// `'replace' provided without 'search'` message when the caller passed e.g.
/// `{"search": 123, "replace": "x"}`.
fn expect_string_field(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    i: usize,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(format!(
            "edits[{}]: '{}' must be a string, got {}",
            i,
            key,
            json_type_name(other)
        )),
    }
}

/// Same as [`expect_string_field`] for boolean fields (e.g. `skipIfNotFound`).
/// Critical: `.unwrap_or(false)` on a wrong-type bool would silently change
/// edit semantics (caller thinks `skipIfNotFound=true` is honoured, batch
/// fails atomically instead).
fn expect_bool_field(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    i: usize,
) -> Result<Option<bool>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(other) => Err(format!(
            "edits[{}]: '{}' must be a boolean (true/false), got {}",
            i,
            key,
            json_type_name(other)
        )),
    }
}

/// Same as [`expect_string_field`] for `u64` fields (e.g. `occurrence`).
fn expect_u64_field(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    i: usize,
) -> Result<Option<u64>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Number(n)) => match n.as_u64() {
            Some(u) => Ok(Some(u)),
            None => Err(format!(
                "edits[{}]: '{}' must be a non-negative integer, got {}",
                i, key, n
            )),
        },
        Some(other) => Err(format!(
            "edits[{}]: '{}' must be a non-negative integer, got {}",
            i,
            key,
            json_type_name(other)
        )),
    }
}

fn parse_text_edits(edits_array: &[Value]) -> Result<Vec<TextEdit>, String> {
    let mut edits = Vec::with_capacity(edits_array.len());
    for (i, edit) in edits_array.iter().enumerate() {
        // Structural shape: each item must be a JSON object. Without this
        // up-front check, `edit.get(...)` returns None for every field on a
        // scalar/null/array payload and the caller falls into the misleading
        // "missing primary action field" branch — far from the real cause.
        let Some(obj) = edit.as_object() else {
            return Err(format!(
                "edits[{}]: each edit item must be a JSON object, got {}. \
                 Example: {{\"search\": \"old\", \"replace\": \"new\"}}",
                i,
                json_type_name(edit)
            ));
        };

        // Validate edit-object keys: reject unknown / aliased fields with an
        // actionable hint BEFORE per-mode validation. This catches the common
        // case where the caller used an alias (oldText/newText, find/with,
        // pattern/with, etc.) — instead of the misleading "missing or invalid
        // 'search'" message, surface the alias → canonical mapping directly.
        if let Some(msg) = check_unknown_edit_object_fields(obj, i) {
            return Err(msg);
        }

        // Part A: Normalize CRLF in all text fields to match LF-normalized
        // file content. Type-strict accessors distinguish "missing" from
        // "present but wrong type" so the caller sees a targeted message
        // instead of the silent drop / misleading downstream error.
        let search = expect_string_field(obj, "search", i)?.map(|s| normalize_crlf(&s));
        let replace = expect_string_field(obj, "replace", i)?.map(|s| normalize_crlf(&s));
        let insert_after = expect_string_field(obj, "insertAfter", i)?.map(|s| normalize_crlf(&s));
        let insert_before = expect_string_field(obj, "insertBefore", i)?.map(|s| normalize_crlf(&s));
        let content = expect_string_field(obj, "content", i)?.map(|s| normalize_crlf(&s));
        let occurrence = expect_u64_field(obj, "occurrence", i)?.unwrap_or(0) as usize;
        let expected_context = expect_string_field(obj, "expectedContext", i)?.map(|s| normalize_crlf(&s));
        let skip_if_not_found = expect_bool_field(obj, "skipIfNotFound", i)?.unwrap_or(false);

        let has_search_replace = search.is_some() || replace.is_some();
        let has_insert = insert_after.is_some() || insert_before.is_some();

        // Validate mutual exclusivity
        if has_search_replace && has_insert {
            return Err(format!(
                "edits[{}]: 'search'/'replace' and 'insertAfter'/'insertBefore' are mutually exclusive",
                i
            ));
        }

        if has_insert {
            // Insert mode validation
            if insert_after.is_some() && insert_before.is_some() {
                return Err(format!(
                    "edits[{}]: 'insertAfter' and 'insertBefore' are mutually exclusive",
                    i
                ));
            }
            if content.is_none() {
                return Err(format!(
                    "edits[{}]: 'content' is required when using 'insertAfter' or 'insertBefore'",
                    i
                ));
            }
            let anchor = insert_after.as_deref().or(insert_before.as_deref()).unwrap();
            if anchor.is_empty() {
                return Err(format!(
                    "edits[{}]: anchor text must not be empty",
                    i
                ));
            }
        } else if has_search_replace {
            // Search/replace mode validation — at least one of search/replace
            // is set, so the caller clearly intended this mode. Validate
            // both halves are present and non-empty.
            let search_str = search.as_deref()
                .ok_or_else(|| format!(
                    "edits[{}]: 'replace' provided without 'search'. Both fields are required for text replacement. \
                     Example: {{\"search\": \"old\", \"replace\": \"new\"}}",
                    i
                ))?;
            if replace.is_none() {
                return Err(format!(
                    "edits[{}]: 'search' provided without 'replace'. Both fields are required for text replacement. \
                     Example: {{\"search\": \"old\", \"replace\": \"new\"}}",
                    i
                ));
            }
            if search_str.is_empty() {
                return Err(format!("edits[{}]: 'search' must not be empty", i));
            }
        } else {
            // Neither mode signalled. After the unknown-field check above, this
            // means the edit-object is empty (or contained only meta-fields like
            // 'occurrence' / 'expectedContext'). Surface the full menu of forms
            // so the caller can pick one without round-tripping to xray_help.
            return Err(missing_edit_form_error_message(edit, i));
        }

        edits.push(TextEdit {
            search,
            replace,
            occurrence,
            insert_after,
            insert_before,
            content,
            expected_context,
            skip_if_not_found,
        });
    }
    Ok(edits)
}


/// Suffix added to occurrence errors when the edit is not the first in the batch.
/// Explains that previous edits may have changed the content, reducing occurrence counts.
const SEQUENTIAL_EDIT_HINT: &str = ". Note: edits are applied sequentially — previous edits in the same request may have modified the content, reducing the occurrence count";

/// Result of the auto-retry anchor/search cascade (exact → strip WS → trim blanks → flex-space).
struct RetrySearchResult {
    /// Start positions of all matches found.
    positions: Vec<usize>,
    /// Match length for non-flex matches (same for all positions).
    match_len: usize,
    /// Per-match lengths when flex-space matching was used (lengths may differ).
    flex_match_lens: Option<Vec<usize>>,
    /// The search text that actually matched (may differ from original after trimming).
    effective_search: String,
    /// Compiled flex regex, if flex-space matching was used.
    flex_re: Option<Regex>,
    /// Warnings generated during the retry cascade.
    warnings: Vec<String>,
}

/// 2-step search cascade: exact match, then optional flex-space regex.
/// Used by both insert-anchor and literal search/replace modes.
///
/// `allow_flex_whitespace`: when `false`, step 2 (regex-based whitespace-collapsing match)
/// is skipped. Flex-space matching can silently match a semantically-different block
/// elsewhere in the file, so it is opt-in: enabled only when the caller supplies an
/// `expectedContext`, which validates ±5 lines around the match and rejects misfires.
///
/// Note: prior versions also retried with trailing-whitespace stripping (Step 2) and
/// blank-line trimming (Step 3). Those were removed in favour of explicit, diagnose-first
/// behaviour — silent retries masked semantic mismatches and led callers to write the
/// wrong bytes. Whitespace and blank-line drift now surfaces as a `Text not found`
/// error with a categorised `Nearest match` hint.
fn find_with_retry(
    content: &str,
    search_text: &str,
    edit_index: usize,
    label: &str,
    allow_flex_whitespace: bool,
) -> RetrySearchResult {
    let mut warnings = Vec::new();

    // Step 1: Exact match
    let mut positions = find_all_occurrences(content, search_text);
    let match_len = search_text.len();
    let effective_search = search_text.to_string();
    let mut flex_match_lens: Option<Vec<usize>> = None;
    let mut flex_re: Option<Regex> = None;

    // Step 2: Flex-space matching (collapse whitespace to regex).
    // Opt-in: only runs when caller passed allow_flex_whitespace=true (i.e. an
    // expectedContext is present to validate the match). Without that guard, the
    // regex can silently match a similar-looking block elsewhere in the file.
    if positions.is_empty()
        && allow_flex_whitespace
        && let Some(pattern) = search_to_flex_pattern(search_text)
            && let Ok(re) = Regex::new(&pattern) {
                let flex_results: Vec<(usize, usize)> = re.find_iter(content)
                    .map(|m| (m.start(), m.end() - m.start()))
                    .collect();
                if !flex_results.is_empty() {
                    warnings.push(format!(
                        "edits[{}]: {} matched with flexible whitespace (spaces collapsed) [fallbackApplied:flexWhitespace]",
                        edit_index, label
                    ));
                    positions = flex_results.iter().map(|&(s, _)| s).collect();
                    flex_match_lens = Some(flex_results.iter().map(|&(_, l)| l).collect());
                    flex_re = Some(re);
                }
            }

    RetrySearchResult {
        positions,
        match_len,
        flex_match_lens,
        effective_search,
        flex_re,
        warnings,
    }
}

/// Apply an insert-after or insert-before edit.
fn apply_insert(
    result: &mut String,
    edit: &TextEdit,
    edit_index: usize,
) -> Result<(usize, Option<SkippedEditDetail>, Vec<String>), String> {
    let anchor = edit.insert_after.as_deref()
        .or(edit.insert_before.as_deref())
        .unwrap();
    let insert_content = edit.content.as_deref().unwrap(); // validated in parse
    let is_after = edit.insert_after.is_some();

    let allow_flex = edit.expected_context.is_some();
    let search_result = find_with_retry(result, anchor, edit_index, "anchor", allow_flex);
    let warnings = search_result.warnings;

    if search_result.positions.is_empty() {
        if edit.skip_if_not_found {
            return Ok((0, Some(SkippedEditDetail {
                edit_index,
                search_text: truncate_for_display(anchor),
                reason: "anchor text not found".to_string(),
            }), warnings));
        }
        let hint = nearest_match_hint(result, anchor);
        let flex_hint = smart_search_not_found_hint(
            anchor, &hint, edit.expected_context.is_some(),
        );
        return Err(format!("Anchor text not found: \"{}\"{}{}", truncate_for_display(anchor), hint, flex_hint));
    }

    // Determine which occurrence to use
    let target_pos = match edit.occurrence {
        0 => search_result.positions[0],
        n => {
            if n > search_result.positions.len() {
                let hint = if edit_index > 0 { SEQUENTIAL_EDIT_HINT } else { "" };
                return Err(format!(
                    "Occurrence {} requested but anchor \"{}\" found only {} time(s){}",
                    n, search_result.effective_search, search_result.positions.len(), hint
                ));
            }
            search_result.positions[n - 1]
        }
    };

    // Compute actual match length for this occurrence (may differ with flex-space)
    let selected_idx = match edit.occurrence { 0 => 0, n => n - 1 };
    let selected_match_len = if let Some(ref lens) = search_result.flex_match_lens {
        lens[selected_idx]
    } else {
        search_result.match_len
    };

    // Check expectedContext if present
    if let Some(ref ctx_text) = edit.expected_context {
        check_expected_context(result, target_pos, selected_match_len, ctx_text)?;
    }

    let anchor_end = target_pos + selected_match_len;

    // Idempotency check: if the content we are about to insert already exists
    // adjacent to the anchor (same side we would insert on), skip the edit
    // instead of producing a duplicate. Protects against the common case where
    // an agent retries a multi-edit call after a partial success.
    if is_after {
        // Insert after: find end of the line containing the anchor, insert on next line
        let line_end = result[anchor_end..].find('\n')
            .map(|p| anchor_end + p)
            .unwrap_or(result.len());
        // Compare: would-be-inserted text starts with '\n' + insert_content.
        // If `result[line_end..]` already begins with that sequence, it's a duplicate.
        let insert_text = format!("\n{}", insert_content);
        if result[line_end..].starts_with(&insert_text) {
            return Ok((0, Some(SkippedEditDetail {
                edit_index,
                search_text: truncate_for_display(anchor),
                reason: "alreadyApplied: content already present after anchor".to_string(),
            }), warnings));
        }
        result.insert_str(line_end, &insert_text);
    } else {
        // Insert before: find start of the line containing the anchor, insert before it
        let line_start = result[..target_pos].rfind('\n')
            .map(|p| p + 1)
            .unwrap_or(0);
        let insert_text = format!("{}\n", insert_content);
        if result[..line_start].ends_with(&insert_text) {
            return Ok((0, Some(SkippedEditDetail {
                edit_index,
                search_text: truncate_for_display(anchor),
                reason: "alreadyApplied: content already present before anchor".to_string(),
            }), warnings));
        }
        result.insert_str(line_start, &insert_text);
    }

    Ok((1, None, warnings))
}

/// Apply a regex-based search/replace edit.
fn apply_regex_replace(
    result: &mut String,
    edit: &TextEdit,
    edit_index: usize,
) -> Result<(usize, Option<SkippedEditDetail>, Vec<String>), String> {
    let search = edit.search.as_deref().unwrap();
    let replace = edit.replace.as_deref().unwrap();

    let re = Regex::new(search)
        .map_err(|e| format!("Invalid regex '{}': {}", search, e))?;
    let count = re.find_iter(result.as_str()).count();

    if count == 0 {
        if edit.skip_if_not_found {
            return Ok((0, Some(SkippedEditDetail {
                edit_index,
                search_text: truncate_for_display(search),
                reason: "regex pattern not found".to_string(),
            }), Vec::new()));
        }
        let hint = nearest_match_hint(result, search);
        return Err(format!("Pattern not found: \"{}\"{}", truncate_for_display(search), hint));
    }

    // Check expectedContext on first match
    if let Some(ref ctx_text) = edit.expected_context
        && let Some(m) = re.find(result.as_str()) {
            check_expected_context(result, m.start(), m.len(), ctx_text)?;
        }

    let replacements = match edit.occurrence {
        0 => {
            let new_content = re.replace_all(result.as_str(), replace).to_string();
            *result = new_content;
            count
        }
        n => {
            if n > count {
                let hint = if edit_index > 0 { SEQUENTIAL_EDIT_HINT } else { "" };
                return Err(format!(
                    "Occurrence {} requested but pattern \"{}\" found only {} time(s){}",
                    n, search, count, hint
                ));
            }
            let mut current = 0usize;
            let replace_str = replace.to_string();
            let new_content = re.replace_all(result.as_str(), |caps: &regex::Captures| {
                current += 1;
                if current == n {
                    // Use caps.expand() to avoid cascade bug where
                    // $0 expansion containing "$1" gets double-substituted
                    let mut out = String::new();
                    caps.expand(&replace_str, &mut out);
                    out
                } else {
                    caps[0].to_string()
                }
            }).to_string();
            *result = new_content;
            1
        }
    };

    Ok((replacements, None, Vec::new()))
}

/// Apply a literal (non-regex) search/replace edit with auto-retry cascade.
fn apply_literal_replace(
    result: &mut String,
    edit: &TextEdit,
    edit_index: usize,
) -> Result<(usize, Option<SkippedEditDetail>, Vec<String>), String> {
    let search = edit.search.as_deref().unwrap();
    let replace = edit.replace.as_deref().unwrap();

    let allow_flex = edit.expected_context.is_some();
    let search_result = find_with_retry(result, search, edit_index, "text", allow_flex);
    let warnings = search_result.warnings;
    let count = search_result.positions.len();

    if count == 0 {
        if edit.skip_if_not_found {
            return Ok((0, Some(SkippedEditDetail {
                edit_index,
                search_text: truncate_for_display(search),
                reason: "text not found".to_string(),
            }), warnings));
        }
        let hint = nearest_match_hint(result, search);
        let flex_hint = smart_search_not_found_hint(
            search, &hint, edit.expected_context.is_some(),
        );
        return Err(format!("Text not found: \"{}\"{}{}", truncate_for_display(search), hint, flex_hint));
    }

    // Validate occurrence range BEFORE evaluating `expectedContext`. The
    // canonical "Occurrence N requested but text … found only M time(s)"
    // diagnostic is the more fundamental error: a caller who asked for
    // `occurrence: 99` against a text that only matches twice has the wrong
    // mental model of the file, and that signal must not be masked by an
    // `expectedContext` mismatch on the (clamped) first match.
    //
    // Earlier this check lived AFTER `check_expected_context`, with
    // `target_idx` clamped to 0 just to avoid an out-of-range panic — but if
    // the first match's surroundings happened to disagree with the supplied
    // context, the user got `Expected context …` instead of the canonical
    // occurrence error. The inline comment in the clamp explicitly promised
    // the canonical error would fire below; that promise is now actually kept.
    if edit.occurrence > count {
        let hint = if edit_index > 0 { SEQUENTIAL_EDIT_HINT } else { "" };
        let effective_search = if search_result.flex_re.is_some() {
            search.to_string()
        } else {
            search_result.effective_search.clone()
        };
        return Err(format!(
            "Occurrence {} requested but text \"{}\" found only {} time(s){}",
            edit.occurrence, effective_search, count, hint
        ));
    }

    // Check expectedContext against the position that will actually be
    // replaced. For `occurrence: 0` (replace-all) we still validate the first
    // match — the contract is "the user-supplied context must surround at
    // least one of the matches we're about to touch", and matching against
    // the first preserves the historical error message. For `occurrence: N`
    // (1-indexed nth-match), validate THAT match: otherwise a caller who
    // explicitly targets the second `Foo` with a context that disambiguates
    // it from the first `Foo` would see the gate fail on the first match's
    // surroundings and never reach their intended replacement.
    if let Some(ref ctx_text) = edit.expected_context {
        // Safe by construction: the `occurrence > count` guard above ensures
        // `target_idx` is in range. `occurrence == 0` (replace-all) maps to
        // the first match.
        let target_idx = if edit.occurrence == 0 { 0 } else { edit.occurrence - 1 };
        let target_len = if let Some(ref lens) = search_result.flex_match_lens {
            lens[target_idx]
        } else {
            search_result.match_len
        };
        check_expected_context(result, search_result.positions[target_idx], target_len, ctx_text)?;
    }

    // Apply replacement
    let replacements = if let Some(ref re) = search_result.flex_re {
        // Flex-space: use regex replacement with NoExpand (literal replacement)
        match edit.occurrence {
            0 => {
                let new_content = re.replace_all(result.as_str(), regex::NoExpand(replace)).to_string();
                *result = new_content;
                count
            }
            n => {
                // Range already validated above; n is in `1..=count` here.
                let mut current = 0usize;
                let replace_owned = replace.to_string();
                let new_content = re.replace_all(result.as_str(), |caps: &regex::Captures| {
                    current += 1;
                    if current == n {
                        replace_owned.clone()
                    } else {
                        caps[0].to_string()
                    }
                }).to_string();
                *result = new_content;
                1
            }
        }
    } else {
        // Literal replacement (steps 1-3)
        let effective_search = &search_result.effective_search;
        match edit.occurrence {
            0 => {
                let new_content = result.replace(effective_search.as_str(), replace);
                *result = new_content;
                count
            }
            n => {
                // Range already validated above; n is in `1..=count` here.
                let mut current = 0usize;
                let mut new_result = String::new();
                let mut remaining = result.as_str();
                while let Some(pos) = remaining.find(effective_search.as_str()) {
                    current += 1;
                    new_result.push_str(&remaining[..pos]);
                    if current == n {
                        new_result.push_str(replace);
                    } else {
                        new_result.push_str(effective_search);
                    }
                    remaining = &remaining[pos + effective_search.len()..];
                }
                new_result.push_str(remaining);
                *result = new_result;
                1
            }
        }
    };

    Ok((replacements, None, warnings))
}

/// Apply text edits sequentially. Returns (new_content, total_replacements, skipped_details, warnings).
fn apply_text_edits(content: &str, edits: &[TextEdit], is_regex: bool) -> Result<(String, usize, Vec<SkippedEditDetail>, Vec<String>), String> {
    let mut result = content.to_string();
    let mut total_replacements = 0;
    let mut skipped_details: Vec<SkippedEditDetail> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    for (edit_index, edit) in edits.iter().enumerate() {
        let (reps, skipped, mut edit_warnings) = if edit.insert_after.is_some() || edit.insert_before.is_some() {
            apply_insert(&mut result, edit, edit_index)?
        } else if is_regex {
            apply_regex_replace(&mut result, edit, edit_index)?
        } else {
            apply_literal_replace(&mut result, edit, edit_index)?
        };

        total_replacements += reps;
        if let Some(s) = skipped {
            skipped_details.push(s);
        }
        warnings.append(&mut edit_warnings);
    }

    Ok((result, total_replacements, skipped_details, warnings))
}

/// Find all occurrences of a literal string, returning their start positions.
fn find_all_occurrences(haystack: &str, needle: &str) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        positions.push(start + pos);
        start += pos + needle.len();
    }
    positions
}

/// Check that expectedContext text exists within ±5 lines of the match position.
fn check_expected_context(content: &str, match_pos: usize, _match_len: usize, expected: &str) -> Result<(), String> {
    // Find line number of the match
    let match_line = content[..match_pos].matches('\n').count();

    // Collect all lines
    let lines: Vec<&str> = content.split('\n').collect();

    // Define context window: ±5 lines around the match
    let start_line = match_line.saturating_sub(5);
    let end_line = (match_line + 5).min(lines.len().saturating_sub(1));

    // Build context string from the window
    let context_window: String = lines[start_line..=end_line].join("\n");

    if !context_window.contains(expected) {
        // Flex-space fallback: collapse whitespace in both and retry
        let collapsed_window = collapse_spaces(&context_window);
        let collapsed_expected = collapse_spaces(expected);
        if !collapsed_window.contains(&collapsed_expected) {
            return Err(format!(
                "Expected context \"{}\" not found near match at line {} (checked lines {}-{})",
                expected, match_line + 1, start_line + 1, end_line + 1
            ));
        }
    }

    Ok(())
}

// ─── Nearest match hint ──────────────────────────────────────────────

/// Truncate a string for display in error messages.
fn truncate_for_display(s: &str) -> String {
    if s.len() <= NEAREST_MATCH_MAX_DISPLAY_LEN {
        s.to_string()
    } else {
        format!("{}…", &s[..s.floor_char_boundary(NEAREST_MATCH_MAX_DISPLAY_LEN)])
    }
}

/// Find the nearest matching line/window in `content` for the given `search_text`.
/// Returns a hint string to append to the error message, or empty string if no good match.
///
/// Algorithm:
/// - For single-line search: compare each line with char-level similarity
/// - For multi-line search: use sliding window of N lines, join and compare
/// - Uses `similar::TextDiff::ratio()` for similarity scoring
/// - Skips files > 500KB for performance
fn nearest_match_hint(content: &str, search_text: &str) -> String {
    // Skip for large files
    if content.len() > NEAREST_MATCH_MAX_FILE_SIZE {
        return String::new();
    }

    let search_lines: Vec<&str> = search_text.split('\n').collect();
    let search_line_count = search_lines.len();
    let content_lines: Vec<&str> = content.split('\n').collect();

    if content_lines.is_empty() || search_text.is_empty() {
        return String::new();
    }

    let mut best_similarity: f32 = 0.0;
    let mut best_line_num: usize = 0; // 1-based
    let mut best_text = String::new();

    if search_line_count <= 1 {
        // Single-line search: compare against each line
        for (i, line) in content_lines.iter().enumerate() {
            let ratio = similar::TextDiff::from_chars(search_text, line).ratio();
            if ratio > best_similarity {
                best_similarity = ratio;
                best_line_num = i + 1;
                best_text = line.to_string();
            }
        }
    } else {
        // Multi-line search: sliding window of search_line_count lines
        if content_lines.len() >= search_line_count {
            for i in 0..=(content_lines.len() - search_line_count) {
                let window = content_lines[i..i + search_line_count].join("\n");
                let ratio = similar::TextDiff::from_chars(search_text, &window).ratio();
                if ratio > best_similarity {
                    best_similarity = ratio;
                    best_line_num = i + 1;
                    best_text = window;
                }
            }
        }
    }

    if best_similarity < NEAREST_MATCH_MIN_SIMILARITY as f32 {
        return String::new();
    }

    let pct = (best_similarity * 100.0).round() as u32;
    let display_text = truncate_for_display(&best_text);

    // Part C: When similarity is high enough (≥ NEAREST_MATCH_BYTE_DIFF_THRESHOLD),
    // add byte-level diff diagnostic plus a category tag for common drift classes.
    let byte_diff_hint = if best_similarity >= NEAREST_MATCH_BYTE_DIFF_THRESHOLD {
        let byte_hint = byte_level_diff_hint(search_text, &best_text);
        let category = detect_diff_category(search_text, &best_text);
        if !category.is_empty() && !byte_hint.is_empty() {
            format!("{} (category: {})", byte_hint, category)
        } else if !category.is_empty() {
            format!(". Diff category: {}", category)
        } else {
            byte_hint
        }
    } else {
        String::new()
    };

    // Part D: When similarity is very high (≥ 90%), append the FULL actual
    // text from the file (up to 800 bytes). This lets the LLM copy-paste
    // the exact content into its corrected `search` without having to
    // re-read the file or guess at whitespace/punctuation differences.
    // Gate on raw float (not rounded pct) to avoid the 89.5%→90% rounding band.
    let actual_content_hint = if best_similarity >= 0.90 {
        let cap = 800;
        if best_text.len() <= cap {
            format!(" Actual content (copy-pastable): `{}`", best_text)
        } else {
            format!(
                " Actual content (truncated to {} bytes): `{}`",
                cap,
                &best_text[..best_text.floor_char_boundary(cap)]
            )
        }
    } else {
        String::new()
    };

    format!(
        ". Nearest match at line {} (similarity {}%): \"{}\"{}{}",
        best_line_num, pct, display_text, byte_diff_hint, actual_content_hint
    )
}


/// Pick the most useful hint to append to a `Text not found` /
/// `Anchor text not found` error, given the user's `search`, the formatted
/// `nearest_match_hint`, and whether `expectedContext` was supplied.
///
/// Priority order (highest-confidence first):
/// 1. `search` contains a Rust/JSON-style escape literal (`\u{...}` or
///    `\xNN`). The contract is that `search` bytes are taken verbatim, so
///    these escapes never get interpreted. The legacy `expectedContext`
///    hint is unrelated and misleads callers into pasting the same broken
///    escape with one extra parameter.
/// 2. `near_hint` shows `First difference at byte 0:` AND `search` starts
///    with leading whitespace (space or tab). This catches the
///    "copied an indented block one column off" failure where the user's
///    boundary line has whitespace the file at that offset does not.
///    `expectedContext` cannot fix this either — it's a ±5-line safety
///    check, not a fuzzy matcher.
/// 3. Fallback: the existing `expectedContext` hint, only if the caller
///    didn't already pass one.
///
/// Returns an empty string when no hint should be appended.
fn smart_search_not_found_hint(
    search: &str,
    near_hint: &str,
    has_expected_context: bool,
) -> &'static str {
    // (1) Literal escape sequences. Cheap textual check — no parsing.
    // `\u{` is unambiguous: it is the only escape form that can encode
    // non-ASCII characters in Rust source, so its appearance in a
    // verbatim `search` string is almost certainly a misuse. We do NOT
    // detect `\xNN` here — Rust's `\xNN` only encodes ASCII bytes
    // (0x00..0x7F), so it is never the right tool for the typographic
    // (em-dash etc.) case the hint targets, and detecting it produced
    // false positives on legitimate path-like input (`C:\x86\toolchain`).
    if search.contains("\\u{") {
        return ". Tip: 'search' is taken verbatim — \\u{...} is NOT \
                interpreted. Pass actual UTF-8 characters instead \
                (e.g., the literal '—' for U+2014).";
    }
    // (2) Boundary-whitespace mismatch at byte 0. The nearest_match_hint
    // already computed the byte-diff position; we re-use its formatted
    // marker text so we don't need to refactor the return type. This is a
    // narrow heuristic — it only fires when there's a high-similarity
    // nearest match AND the user's search starts with whitespace AND the
    // diff is at the very first byte.
    if near_hint.contains("First difference at byte 0:")
        && (search.starts_with(' ') || search.starts_with('\t'))
    {
        return ". Tip: leading whitespace in 'search' doesn't match the file at \
                byte 0 — trim the first line of your `search`, or use \
                insertAfter/insertBefore with a unique anchor.";
    }
    // (3) Legacy fallback.
    if !has_expected_context {
        return ". Hint: pass `expectedContext` to enable flexible-whitespace fallback matching";
    }
    ""
}

/// Generate a byte-level diff hint showing the first difference between two strings.
/// Used when similarity is ≥99% to help identify invisible whitespace differences.
fn byte_level_diff_hint(search: &str, found: &str) -> String {
    let search_bytes = search.as_bytes();
    let found_bytes = found.as_bytes();

    // Find first different byte
    for (i, (s, f)) in search_bytes.iter().zip(found_bytes.iter()).enumerate() {
        if s != f {
            return format!(
                ". First difference at byte {}: search has {}, file has {}",
                i, describe_byte(*s), describe_byte(*f)
            );
        }
    }

    // If one is a prefix of the other
    if search_bytes.len() != found_bytes.len() {
        let shorter = search_bytes.len().min(found_bytes.len());
        if search_bytes.len() > found_bytes.len() {
            let extra_byte = search_bytes[shorter];
            return format!(
                ". Search text is {} byte(s) longer than file text. Extra content starts with {}",
                search_bytes.len() - found_bytes.len(), describe_byte(extra_byte)
            );
        } else {
            let extra_byte = found_bytes[shorter];
            return format!(
                ". File text is {} byte(s) longer than search text. Extra content starts with {}",
                found_bytes.len() - search_bytes.len(), describe_byte(extra_byte)
            );
        }
    }

    // Identical bytes — shouldn't happen if we got here, but be safe
    String::new()
}

/// Categorise the difference between `search` (what the caller passed) and `found`
/// (the nearest match in the file). Returns one of:
///   - `"crlfVsLf"`               — line-ending normalisation would make them equal
///   - `"leadingOrTrailingBlankLines"` — surrounding `\n` padding differs
///   - `"trailingWhitespace"`       — only trailing spaces/tabs per line differ
///   - `"unicodeConfusable"`        — only NBSP, en/em-dash, or smart quotes differ
///   - `""`                        — no recognised category (or strings identical)
///
/// The first matching category wins. The detector is conservative: any byte
/// difference outside the relevant equivalence class disqualifies the category.
fn detect_diff_category(search: &str, found: &str) -> &'static str {
    if search == found {
        return "";
    }

    // 1. CRLF vs LF: stripping all '\r' from both makes them equal,
    //    and at least one side actually has a '\r'.
    let s_no_cr: String = search.chars().filter(|&c| c != '\r').collect();
    let f_no_cr: String = found.chars().filter(|&c| c != '\r').collect();
    if s_no_cr == f_no_cr && (search.contains('\r') || found.contains('\r')) {
        return "crlfVsLf";
    }

    // 2. Leading/trailing blank lines: trimming '\n' and '\r' on both ends
    //    makes them equal. Stripping both characters keeps this branch line-
    //    ending agnostic so CRLF drift like "\r\nfoo\r\n" vs "foo\r\n" is
    //    classified correctly. Pure LF↔CRLF mismatch is already handled by the
    //    crlfVsLf branch above and therefore never reaches this point.
    if search.trim_matches(|c: char| c == '\n' || c == '\r')
        == found.trim_matches(|c: char| c == '\n' || c == '\r')
    {
        return "leadingOrTrailingBlankLines";
    }

    // 3. Trailing whitespace per line: rstrip [ \t] from each line on both sides
    //    makes them equal.
    let rstrip_per_line = |s: &str| -> String {
        s.split('\n')
            .map(|line| line.trim_end_matches([' ', '\t']))
            .collect::<Vec<_>>()
            .join("\n")
    };
    if rstrip_per_line(search) == rstrip_per_line(found) {
        return "trailingWhitespace";
    }

    // 4. Unicode confusables: NBSP, en/em-dash, smart quotes.
    //    If folding all confusables to their ASCII counterparts on BOTH sides
    //    makes the strings equal, classify accordingly.
    let fold_confusables = |s: &str| -> String {
        s.chars()
            .map(|c| match c {
                '\u{00A0}' => ' ',     // NBSP
                '\u{2013}' | '\u{2014}' => '-', // en-dash, em-dash
                '\u{2018}' | '\u{2019}' => '\'', // single smart quotes
                '\u{201C}' | '\u{201D}' => '"', // double smart quotes
                other => other,
            })
            .collect()
    };
    if fold_confusables(search) == fold_confusables(found) {
        return "unicodeConfusable";
    }

    ""
}

// ─── Diff generation ─────────────────────────────────────────────────

/// Compute brace-balance delta warnings between original and modified
/// content. Returns one warning per disagreeing pair class (`{}`, `()`,
/// `[]`). Empty Vec when all three pairs are balanced (the common case).
///
/// The check is intentionally crude: it counts raw bytes, ignoring
/// strings, comments, and char literals. That makes it correct for
/// **delta** comparison (a brace that exists identically in both
/// pre- and post-edit cancels out) and dirt-cheap to compute.
///
/// Caveat: an edit that adds AND removes the same number of
/// counter-balanced braces (e.g. moves a block to a different scope)
/// has delta = 0 and won't fire. That is the deliberate trade-off —
/// false positives on Python/Markdown/SQL would be unacceptable.
fn brace_balance_warnings(original: &str, modified: &str) -> Vec<String> {
    let pairs: [(u8, u8, &str); 3] = [
        (b'{', b'}', "curly"),
        (b'(', b')', "round"),
        (b'[', b']', "square"),
    ];
    let mut out = Vec::new();
    for &(open, close, name) in &pairs {
        let count_b = |s: &str, b: u8| s.as_bytes().iter().filter(|&&c| c == b).count() as isize;
        let d_open = count_b(modified, open) - count_b(original, open);
        let d_close = count_b(modified, close) - count_b(original, close);
        if d_open != d_close {
            out.push(format!(
                "{} brace count drifted asymmetrically: \
                 '{}' delta = {:+}, '{}' delta = {:+}. \
                 If this was unintended (most common cause: search \
                 included a closer that replace omitted), the file may \
                 no longer compile — verify with cargo build / linter.",
                name,
                open as char, d_open,
                close as char, d_close,
            ));
        }
    }
    out
}

/// Prose / markup file extensions where round/square/curly braces in the
/// content are NOT structural — parens in English text ("(see X)", "e.g. ..."),
/// markdown emphasis, etc. — and `brace_balance_warnings` would only generate
/// false positives. Keep `.html`/`.xml` OUT of this list: they routinely host
/// inline JS/templates where braces ARE structural.
fn is_prose_extension(path_str: &str) -> bool {
    const PROSE_EXTS: &[&str] = &["md", "markdown", "txt", "rst", "adoc", "asciidoc"];
    std::path::Path::new(path_str)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let lower = e.to_ascii_lowercase();
            PROSE_EXTS.contains(&lower.as_str())
        })
        .unwrap_or(false)
}

/// Generate a unified diff between original and modified content.
fn generate_unified_diff(path: &str, original: &str, modified: &str) -> String {
    if original == modified {
        return String::new();
    }

    similar::TextDiff::from_lines(original, modified)
        .unified_diff()
        .header(&format!("a/{}", path), &format!("b/{}", path))
        .to_string()
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;