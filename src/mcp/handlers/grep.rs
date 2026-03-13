//! xray_grep handler: token search, substring search, phrase search.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde_json::{json, Value};
use tracing::debug;

use crate::mcp::protocol::ToolCallResult;
use crate::{read_file_lossy, tokenize, ContentIndex};
use crate::index::build_trigram_index;
use code_xray::generate_trigrams;

use super::utils::{
    build_line_content_from_matches, inject_branch_warning, is_under_dir, json_to_string,
    matches_ext_filter, sorted_intersect, validate_search_dir,
};
use super::HandlerContext;

/// Shared parameters for substring and phrase search modes.
/// Eliminates 10+ positional parameters from handle_substring_search and handle_phrase_search.
pub(crate) struct GrepSearchParams<'a> {
    pub ext_filter: &'a Option<String>,
    pub exclude_dir: &'a [String],
    pub exclude: &'a [String],
    pub show_lines: bool,
    pub context_lines: usize,
    pub max_results: usize,
    pub mode_and: bool,
    pub count_only: bool,
    pub search_start: Instant,
    pub dir_filter: &'a Option<String>,
}

pub(crate) struct FileScoreEntry {
    pub file_path: String,
    pub lines: Vec<u32>,
    pub tf_idf: f64,
    pub occurrences: usize,
    pub terms_matched: usize,
}

/// A single file match from phrase search, with matched lines and optionally cached content.
pub(crate) struct PhraseFileMatch {
    pub file_path: String,
    pub lines: Vec<u32>,
    pub content: Option<String>,
}

/// Build the common grep summary JSON with readErrors, lossyUtf8Files, and branchWarning.
/// When `include_index_stats` is true, adds indexFiles, indexTokens, searchTimeMs, indexLoadTimeMs.
fn build_grep_base_summary(
    total_files: usize,
    total_occurrences: usize,
    terms: &Value,
    search_mode: &str,
    index: &ContentIndex,
    search_elapsed: std::time::Duration,
    ctx: &HandlerContext,
    include_index_stats: bool,
) -> Value {
    let mut summary = json!({
        "totalFiles": total_files,
        "totalOccurrences": total_occurrences,
        "termsSearched": terms,
        "searchMode": search_mode,
    });
    if include_index_stats {
        summary["indexFiles"] = json!(index.files.len());
        summary["indexTokens"] = json!(index.index.len());
        summary["searchTimeMs"] = json!(search_elapsed.as_secs_f64() * 1000.0);
        summary["indexLoadTimeMs"] = json!(0.0);
    }
    if index.read_errors > 0 {
        summary["readErrors"] = json!(index.read_errors);
    }
    if index.lossy_file_count > 0 {
        summary["lossyUtf8Files"] = json!(index.lossy_file_count);
    }
    inject_branch_warning(&mut summary, ctx);
    summary
}

/// Finalize grep results: filter by AND mode, sort/dedup lines, sort by TF-IDF descending.
/// Returns (sorted_results, total_files_before_truncation, total_occurrences).
fn finalize_grep_results(
    file_scores: HashMap<u32, FileScoreEntry>,
    mode_and: bool,
    term_count: usize,
) -> (Vec<FileScoreEntry>, usize, usize) {
    let mut results: Vec<FileScoreEntry> = file_scores
        .into_values()
        .filter(|fs| !mode_and || fs.terms_matched >= term_count)
        .collect();

    // Sort/dedup lines
    for result in &mut results {
        result.lines.sort();
        result.lines.dedup();
    }

    // Sort by TF-IDF descending
    results.sort_by(|a, b| b.tf_idf.partial_cmp(&a.tf_idf).unwrap_or(std::cmp::Ordering::Equal));

    let total_files = results.len();
    let total_occurrences: usize = results.iter().map(|r| r.occurrences).sum();

    (results, total_files, total_occurrences)
}

/// Ensure the trigram index is up-to-date. Called before substring search.
/// If `trigram_dirty` is set, rebuilds the trigram index with minimal write-lock time.
fn ensure_trigram_index(ctx: &HandlerContext) {
    let trigram_check_start = Instant::now();
    let needs_rebuild = ctx.index.read().map(|idx| idx.trigram_dirty).unwrap_or(false);
    if needs_rebuild {
        debug!("[substring-trace] Trigram dirty, rebuilding...");
        let rebuild_start = Instant::now();
        // Build trigram index under READ lock (doesn't block other readers)
        let new_trigram = ctx.index.read().ok().and_then(|idx| {
            if idx.trigram_dirty {
                Some(build_trigram_index(&idx.index))
            } else {
                None
            }
        });
        // Swap in under brief WRITE lock (microseconds, not ~200ms)
        if let Some(trigram) = new_trigram
            && let Ok(mut idx) = ctx.index.write()
                && idx.trigram_dirty {  // double-check after acquiring write lock
                    debug!("[substring] Rebuilt trigram index: {} tokens, {} trigrams",
                        trigram.tokens.len(), trigram.trigram_map.len());
                    idx.trigram = trigram;
                    idx.trigram_dirty = false;
                }
        debug!("[substring-trace] Trigram rebuild: {:.3}ms", rebuild_start.elapsed().as_secs_f64() * 1000.0);
    } else {
        debug!("[substring-trace] Trigram dirty check: clean in {:.3}ms", trigram_check_start.elapsed().as_secs_f64() * 1000.0);
    }
}

/// Check if a file passes all grep filters (dir, ext, excludeDir, exclude).
/// Returns true if the file should be included in results.
fn passes_file_filters(file_path: &str, params: &GrepSearchParams) -> bool {
    // Dir prefix filter (subdirectory search)
    if let Some(prefix) = params.dir_filter
        && !is_under_dir(file_path, prefix) { return false; }

    // Extension filter (supports comma-separated extensions)
    if let Some(ext) = params.ext_filter
        && !matches_ext_filter(file_path, ext) { return false; }

    // Exclude dir filter
    if params.exclude_dir.iter().any(|excl| {
        file_path.to_lowercase().contains(&excl.to_lowercase())
    }) { return false; }

    // Exclude pattern filter
    if params.exclude.iter().any(|excl| {
        file_path.to_lowercase().contains(&excl.to_lowercase())
    }) { return false; }

    true
}

/// Parsed arguments for the grep handler. Extracts all parameter parsing
/// from the main handler to reduce its cognitive complexity.
#[derive(Debug)]
struct ParsedGrepArgs {
    terms_str: String,
    dir_filter: Option<String>,
    ext_filter: Option<String>,
    mode_and: bool,
    use_regex: bool,
    use_phrase: bool,
    use_substring: bool,
    context_lines: usize,
    show_lines: bool,
    max_results: usize,
    count_only: bool,
    exclude_dir: Vec<String>,
    exclude: Vec<String>,
}

/// Parse and validate all grep parameters from JSON args.
/// Returns `Ok(ParsedGrepArgs)` on success, `Err(ToolCallResult)` on validation error.
fn parse_grep_args(args: &Value, server_dir: &str) -> Result<ParsedGrepArgs, ToolCallResult> {
    let terms_str = match args.get("terms").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => return Err(ToolCallResult::error("Missing required parameter: terms".to_string())),
    };

    let dir_filter: Option<String> = if let Some(dir) = args.get("dir").and_then(|v| v.as_str()) {
        match validate_search_dir(dir, server_dir) {
            Ok(filter) => {
                // Detect file paths passed as dir= and reject with helpful hint
                if let Some(ref resolved) = filter {
                    let path = std::path::Path::new(resolved);
                    if path.is_file() || super::utils::looks_like_file_path(resolved) {
                        let parent = path.parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|| server_dir.to_string());
                        let filename = path.file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_default();
                        return Err(ToolCallResult::error(format!(
                            "dir='{}' is a file path, not a directory. xray_grep dir= accepts directories only. \
                             Try: dir='{}' to search the parent directory, \
                             or use xray_definitions file='{}' for AST-based search in a specific file.",
                            dir, parent, filename
                        )));
                    }
                }
                filter
            },
            Err(msg) => return Err(ToolCallResult::error(msg)),
        }
    } else {
        None
    };

    let ext_filter = args.get("ext").and_then(|v| v.as_str()).map(|s| s.to_string());
    let mode_and = args.get("mode").and_then(|v| v.as_str()) == Some("and");
    let use_regex = args.get("regex").and_then(|v| v.as_bool()).unwrap_or(false);
    let use_phrase = args.get("phrase").and_then(|v| v.as_bool()).unwrap_or(false);

    // Default to substring=true so compound C# identifiers are always found.
    // Auto-disable when regex/phrase is used.
    let use_substring = if use_regex || use_phrase {
        if args.get("substring").and_then(|v| v.as_bool()) == Some(true) {
            return Err(ToolCallResult::error(
                "substring is mutually exclusive with regex and phrase".to_string(),
            ));
        }
        false
    } else {
        args.get("substring").and_then(|v| v.as_bool()).unwrap_or(true)
    };

    let context_lines = args.get("contextLines").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    // Auto-enable showLines when contextLines > 0
    let show_lines = args.get("showLines").and_then(|v| v.as_bool()).unwrap_or(false)
        || context_lines > 0;
    let max_results = args.get("maxResults").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
    let count_only = args.get("countOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    let exclude_dir: Vec<String> = args.get("excludeDir")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let exclude: Vec<String> = args.get("exclude")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    Ok(ParsedGrepArgs {
        terms_str,
        dir_filter,
        ext_filter,
        mode_and,
        use_regex,
        use_phrase,
        use_substring,
        context_lines,
        show_lines,
        max_results,
        count_only,
        exclude_dir,
        exclude,
    })
}

/// Expand regex patterns against index keys. Returns expanded terms or error.
fn expand_regex_terms(
    raw_terms: &[String],
    index: &ContentIndex,
) -> Result<Vec<String>, ToolCallResult> {
    let mut expanded = Vec::new();
    for pat in raw_terms {
        match regex::Regex::new(&format!("(?i)^{}$", pat)) {
            Ok(re) => {
                let matching: Vec<String> = index.index.keys()
                    .filter(|k| re.is_match(k))
                    .cloned()
                    .collect();
                expanded.extend(matching);
            }
            Err(e) => return Err(ToolCallResult::error(format!("Invalid regex '{}': {}", pat, e))),
        }
    }
    Ok(expanded)
}

/// Score files for normal (non-substring, non-phrase) token search.
/// Iterates over terms, looks up postings, computes TF-IDF, accumulates file scores.
fn score_normal_token_search(
    terms: &[String],
    index: &ContentIndex,
    params: &GrepSearchParams,
) -> HashMap<u32, FileScoreEntry> {
    let total_docs = index.files.len() as f64;
    let mut file_scores: HashMap<u32, FileScoreEntry> = HashMap::new();

    for term in terms {
        if let Some(postings) = index.index.get(term.as_str()) {
            let doc_freq = postings.len() as f64;
            let idf = (total_docs / doc_freq).ln();

            for posting in postings {
                let file_path = match index.files.get(posting.file_id as usize) {
                    Some(p) => p,
                    None => continue,
                };

                if !passes_file_filters(file_path, params) { continue; }

                let occurrences = posting.lines.len();
                let file_total = if (posting.file_id as usize) < index.file_token_counts.len() {
                    index.file_token_counts[posting.file_id as usize] as f64
                } else {
                    1.0
                };
                let tf = occurrences as f64 / file_total;
                let tf_idf = tf * idf;

                let entry = file_scores.entry(posting.file_id).or_insert(FileScoreEntry {
                    file_path: file_path.clone(),
                    lines: Vec::new(),
                    tf_idf: 0.0,
                    occurrences: 0,
                    terms_matched: 0,
                });
                entry.tf_idf += tf_idf;
                entry.occurrences += occurrences;
                entry.lines.extend_from_slice(&posting.lines);
                entry.terms_matched += 1;
            }
        }
    }

    file_scores
}

/// Build the final JSON response for grep results (normal and substring modes).
fn build_grep_response(
    results: &[FileScoreEntry],
    terms: &[String],
    total_files: usize,
    total_occurrences: usize,
    search_mode: &str,
    index: &ContentIndex,
    ctx: &HandlerContext,
    params: &GrepSearchParams,
) -> ToolCallResult {
    let search_elapsed = params.search_start.elapsed();

    if params.count_only {
        let summary = build_grep_base_summary(
            total_files, total_occurrences, &json!(terms), search_mode,
            index, search_elapsed, ctx, true,
        );
        let output = json!({ "summary": summary });
        return ToolCallResult::success(json_to_string(&output));
    }

    let files_json: Vec<Value> = results.iter().map(|r| {
        let mut file_obj = json!({
            "path": r.file_path,
            "score": (r.tf_idf * 10000.0).round() / 10000.0,
            "occurrences": r.occurrences,
            "termsMatched": format!("{}/{}", r.terms_matched, terms.len()),
            "lines": r.lines,
        });

        if params.show_lines
            && let Ok((content, _lossy)) = read_file_lossy(std::path::Path::new(&r.file_path)) {
                file_obj["lineContent"] = build_line_content_from_matches(&content, &r.lines, params.context_lines);
            }

        file_obj
    }).collect();

    let summary = build_grep_base_summary(
        total_files, total_occurrences, &json!(terms), search_mode,
        index, search_elapsed, ctx, true,
    );
    let output = json!({
        "files": files_json,
        "summary": summary
    });

    ToolCallResult::success(json_to_string(&output))
}

/// When grep returns 0 results and `ext` filter targets a non-indexed extension,
/// inject a hint explaining why no results were found.
/// Only fires when ext filter is explicitly set — avoids noise on generic searches.
fn inject_grep_ext_hint(
    result: &mut ToolCallResult,
    ext_filter: &Option<String>,
    ctx: &HandlerContext,
) {
    // Only hint when ext filter is explicitly set
    let ext_str = match ext_filter {
        Some(e) => e,
        None => return,
    };

    let text = match result.content.first() {
        Some(c) => &c.text,
        None => return,
    };

    let mut output: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Check if totalFiles == 0
    let total_files = output.pointer("/summary/totalFiles")
        .and_then(|v| v.as_u64()).unwrap_or(1); // default to 1 = no hint
    if total_files > 0 { return; }

    // Parse server extensions
    let server_exts: Vec<&str> = ctx.server_ext
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect();

    // Find non-indexed extensions in the filter
    let non_indexed: Vec<&str> = ext_str.split(',')
        .map(|s| s.trim())
        .filter(|e| !e.is_empty() && !server_exts.iter().any(|s| s.eq_ignore_ascii_case(e)))
        .collect();

    if non_indexed.is_empty() { return; }

    let hint = format!(
        "Extension(s) '{}' not in content index (indexed: {}). \
         Use read_file for these file types.",
        non_indexed.join(", "),
        ctx.server_ext,
    );

    if let Some(summary) = output.get_mut("summary").and_then(|v| v.as_object_mut()) {
        summary.insert("hint".to_string(), json!(hint));
    }

    result.content[0].text = json_to_string(&output);
}


pub(crate) fn handle_xray_grep(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    let parsed = match parse_grep_args(args, &ctx.server_dir) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let search_start = Instant::now();

    if parsed.use_substring {
        ensure_trigram_index(ctx);
    }

    let index = match ctx.index.read() {
        Ok(idx) => idx,
        Err(e) => return ToolCallResult::error(format!("Failed to acquire index lock: {}", e)),
    };

    let grep_params = GrepSearchParams {
        ext_filter: &parsed.ext_filter,
        exclude_dir: &parsed.exclude_dir,
        exclude: &parsed.exclude,
        show_lines: parsed.show_lines,
        context_lines: parsed.context_lines,
        max_results: parsed.max_results,
        mode_and: parsed.mode_and,
        count_only: parsed.count_only,
        search_start,
        dir_filter: &parsed.dir_filter,
    };

    // --- Substring search mode
    if parsed.use_substring {
        let mut result = handle_substring_search(ctx, &index, &parsed.terms_str, &grep_params);
        inject_grep_ext_hint(&mut result, &parsed.ext_filter, ctx);
        return result;
    }

    // --- Phrase search mode
    if parsed.use_phrase {
        let phrases: Vec<String> = parsed.terms_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if phrases.is_empty() {
            return ToolCallResult::error("No search terms provided".to_string());
        }
        let mut result = handle_multi_phrase_search(ctx, &index, &phrases, &grep_params);
        inject_grep_ext_hint(&mut result, &parsed.ext_filter, ctx);
        return result;
    }

    // --- Normal token search
    let raw_terms: Vec<String> = parsed.terms_str
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    if raw_terms.is_empty() {
        return ToolCallResult::error("No search terms provided".to_string());
    }

    let terms: Vec<String> = if parsed.use_regex {
        match expand_regex_terms(&raw_terms, &index) {
            Ok(t) => t,
            Err(e) => return e,
        }
    } else {
        raw_terms.clone()
    };

    let search_mode = if parsed.use_regex { "regex" } else if parsed.mode_and { "and" } else { "or" };
    let term_count_for_all = if parsed.use_regex { raw_terms.len() } else { terms.len() };

    let file_scores = score_normal_token_search(&terms, &index, &grep_params);

    let (mut results, total_files, total_occurrences) =
        finalize_grep_results(file_scores, parsed.mode_and, term_count_for_all);

    if parsed.max_results > 0 {
        results.truncate(parsed.max_results);
    }

    let mut result = build_grep_response(
        &results, &terms, total_files, total_occurrences,
        search_mode, &index, ctx, &grep_params,
    );

    // Warn when regex=true and terms contain spaces (tokens never have spaces)
    if parsed.use_regex && parsed.terms_str.contains(' ') {
        if let Some(text) = result.content.first_mut().map(|c| &mut c.text)
            && let Ok(mut output) = serde_json::from_str::<serde_json::Value>(text) {
                if let Some(summary) = output.get_mut("summary") {
                    summary["searchModeNote"] = serde_json::Value::String(
                        "Regex operates on individual index tokens which never contain spaces. \
                         Multi-word regex patterns like 'private.*double' will not match across tokens. \
                         For multi-word search use phrase=true, or search individual terms separately with regex=true.".to_string(),
                    );
                }
                *text = json_to_string(&output);
            }
    }

    inject_grep_ext_hint(&mut result, &parsed.ext_filter, ctx);

    result
}

/// Check if a search term contains characters that the tokenizer strips.
/// The tokenizer (`tokenize()`) splits on `!c.is_alphanumeric() && c != '_'`,
/// so any character that is not alphanumeric and not `_` is a separator.
/// If a term contains such characters (e.g., `#[cfg(test)]`, `<summary>`,
/// `@Attribute`), substring search will never find it because no indexed
/// token contains those characters.
fn has_non_token_chars(term: &str) -> bool {
    term.chars().any(|c| !c.is_alphanumeric() && c != '_')
}

/// Auto-switch to phrase mode when substring terms contain characters that
/// the tokenizer strips (spaces, punctuation, brackets, etc.).
///
/// Substring search operates on individual indexed tokens, which only contain
/// alphanumeric characters and underscores. Queries like `#[cfg(test)]` or
/// `CREATE PROCEDURE` would always return 0 results in substring mode because
/// no token contains `#`, `[`, `(`, `)`, `]`, or spaces.
///
/// Phrase search with punctuation does raw substring matching on file content,
/// which correctly handles these patterns.
///
/// Returns `Some(result)` if auto-switched, `None` if normal processing should continue.
fn auto_switch_to_phrase_if_needed(
    ctx: &HandlerContext,
    index: &ContentIndex,
    terms_str: &str,
    raw_terms: &[String],
    params: &GrepSearchParams,
) -> Option<ToolCallResult> {
    let has_spaces = raw_terms.iter().any(|t| t.contains(' '));
    let has_punctuation = raw_terms.iter().any(|t| has_non_token_chars(t));

    if !has_spaces && !has_punctuation {
        return None;
    }

    let reason = if has_spaces && has_punctuation {
        "Terms contain spaces and non-token characters (punctuation/brackets)"
    } else if has_spaces {
        "Terms contain spaces"
    } else {
        "Terms contain non-token characters (punctuation/brackets) that the tokenizer strips"
    };

    debug!("[substring-trace] {} — auto-switching to phrase mode", reason);
    let phrases: Vec<String> = terms_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut result = handle_multi_phrase_search(ctx, index, &phrases, params);
    // Inject a note explaining the auto-switch
    if let Some(text) = result.content.first_mut().map(|c| &mut c.text)
        && let Ok(mut output) = serde_json::from_str::<serde_json::Value>(text) {
            if let Some(summary) = output.get_mut("summary") {
                summary["searchModeNote"] = serde_json::Value::String(
                    format!("{} — auto-switched to phrase search \
                     (substring mode operates on individual tokens which only contain \
                     alphanumeric characters and underscores)", reason),
                );
            }
            *text = json_to_string(&output);
        }
    Some(result)
}

/// Find token indices matching a term as a substring via the trigram index.
/// For short terms (<3 chars), falls back to linear scan.
/// Returns indices into the trigram token list.
fn find_matching_tokens_for_term(
    term: &str,
    trigram_idx: &crate::TrigramIndex,
) -> Vec<u32> {
    if term.len() < 3 {
        // Linear scan for very short terms (no trigrams possible)
        return trigram_idx.tokens.iter().enumerate()
            .filter(|(_, tok)| tok.contains(term))
            .map(|(i, _)| i as u32)
            .collect();
    }

    let trigrams = generate_trigrams(term);
    if trigrams.is_empty() {
        return Vec::new();
    }

    // Intersect trigram posting lists to find candidate token indices
    let mut candidates: Option<Vec<u32>> = None;
    for tri in &trigrams {
        if let Some(posting_list) = trigram_idx.trigram_map.get(tri) {
            candidates = Some(match candidates {
                None => posting_list.clone(),
                Some(prev) => sorted_intersect(&prev, posting_list),
            });
        } else {
            return Vec::new(); // Trigram not found → no candidates
        }
    }

    let candidate_indices = candidates.unwrap_or_default();

    // Verify candidates actually contain the term (.contains() check)
    let verify_start = Instant::now();
    let verified: Vec<u32> = candidate_indices.into_iter()
        .filter(|&idx| {
            trigram_idx.tokens.get(idx as usize)
                .is_some_and(|tok| tok.contains(term))
        })
        .collect();
    debug!("[substring-trace] Token verification for '{}': {} verified from candidates in {:.3}ms",
        term, verified.len(), verify_start.elapsed().as_secs_f64() * 1000.0);
    verified
}

/// Score matched tokens against the main inverted index for a single term.
/// Applies file filters, computes TF-IDF, and accumulates into shared structures.
fn score_token_postings(
    matched_tokens: &[String],
    term_idx: usize,
    index: &ContentIndex,
    params: &GrepSearchParams,
    total_docs: f64,
    tokens_with_hits: &mut HashSet<String>,
    file_scores: &mut HashMap<u32, FileScoreEntry>,
    file_matched_terms: &mut HashMap<u32, HashSet<usize>>,
) {
    let lookup_start = Instant::now();
    let mut term_postings_checked: usize = 0;
    let mut term_files_passed: usize = 0;

    for token in matched_tokens {
        if let Some(postings) = index.index.get(token.as_str()) {
            let doc_freq = postings.len() as f64;
            let idf = if doc_freq > 0.0 { (total_docs / doc_freq).ln() } else { 0.0 };

            for posting in postings {
                term_postings_checked += 1;
                let file_path = match index.files.get(posting.file_id as usize) {
                    Some(p) => p,
                    None => continue,
                };

                if !passes_file_filters(file_path, params) { continue; }

                term_files_passed += 1;
                tokens_with_hits.insert(token.clone());

                let occurrences = posting.lines.len();
                let file_total = if (posting.file_id as usize) < index.file_token_counts.len() {
                    index.file_token_counts[posting.file_id as usize] as f64
                } else {
                    1.0
                };
                let tf = occurrences as f64 / file_total;
                let tf_idf = tf * idf;

                let entry = file_scores.entry(posting.file_id).or_insert(FileScoreEntry {
                    file_path: file_path.clone(),
                    lines: Vec::new(),
                    tf_idf: 0.0,
                    occurrences: 0,
                    terms_matched: 0,
                });
                entry.tf_idf += tf_idf;
                entry.occurrences += occurrences;
                entry.lines.extend_from_slice(&posting.lines);
                file_matched_terms.entry(posting.file_id).or_default().insert(term_idx);
            }
        }
    }

    debug!("[substring-trace] Main index lookup: {} tokens, {} postings checked, {} files passed in {:.3}ms",
        matched_tokens.len(), term_postings_checked, term_files_passed,
        lookup_start.elapsed().as_secs_f64() * 1000.0);
}

/// Build the final substring search response (JSON with files, summary, warnings, matchedTokens).
fn build_substring_response(
    results: &[FileScoreEntry],
    raw_terms: &[String],
    all_matched_tokens: &[String],
    warnings: &[String],
    total_files: usize,
    total_occurrences: usize,
    search_mode: &str,
    index: &ContentIndex,
    ctx: &HandlerContext,
    params: &GrepSearchParams,
) -> ToolCallResult {
    let search_start = params.search_start;

    if params.count_only {
        let mut summary = build_grep_base_summary(
            total_files, total_occurrences, &json!(raw_terms),
            &format!("substring-{}", search_mode), index, search_start.elapsed(), ctx, false,
        );
        summary["matchedTokens"] = json!(all_matched_tokens);
        if !warnings.is_empty() {
            summary["warnings"] = json!(warnings);
        }
        let output = json!({ "summary": summary });
        debug!("[substring-trace] Total: {:.3}ms (count_only)", search_start.elapsed().as_secs_f64() * 1000.0);
        return ToolCallResult::success(output.to_string());
    }

    let json_start = Instant::now();
    let files_json: Vec<Value> = results.iter().map(|r| {
        let mut file_obj = json!({
            "path": r.file_path,
            "score": (r.tf_idf * 10000.0).round() / 10000.0,
            "occurrences": r.occurrences,
            "lines": r.lines,
        });

        if params.show_lines
            && let Ok((content, _lossy)) = read_file_lossy(std::path::Path::new(&r.file_path)) {
                file_obj["lineContent"] = build_line_content_from_matches(&content, &r.lines, params.context_lines);
            }

        file_obj
    }).collect();

    let mut summary = build_grep_base_summary(
        total_files, total_occurrences, &json!(raw_terms),
        &format!("substring-{}", search_mode), index, search_start.elapsed(), ctx, false,
    );
    summary["matchedTokens"] = json!(all_matched_tokens);
    if !warnings.is_empty() {
        summary["warnings"] = json!(warnings);
    }
    let output = json!({
        "files": files_json,
        "summary": summary
    });
    debug!("[substring-trace] Response JSON: {:.3}ms", json_start.elapsed().as_secs_f64() * 1000.0);
    debug!("[substring-trace] Total: {:.3}ms ({} files, {} tokens matched)",
        search_start.elapsed().as_secs_f64() * 1000.0, total_files, all_matched_tokens.len());

    ToolCallResult::success(output.to_string())
}

/// Substring search using the trigram index.
fn handle_substring_search(
    ctx: &HandlerContext,
    index: &ContentIndex,
    terms_str: &str,
    params: &GrepSearchParams,
) -> ToolCallResult {
    // Stage 1: Terms parsing
    let stage1 = Instant::now();
    let raw_terms: Vec<String> = terms_str
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    debug!("[substring-trace] Terms parsed: {:?} in {:.3}ms", raw_terms, stage1.elapsed().as_secs_f64() * 1000.0);

    if raw_terms.is_empty() {
        return ToolCallResult::error("No search terms provided".to_string());
    }

    // Auto-switch to phrase mode when terms contain spaces or non-token characters
    if let Some(result) = auto_switch_to_phrase_if_needed(ctx, index, terms_str, &raw_terms, params) {
        return result;
    }

    let trigram_idx = &index.trigram;
    let total_docs = index.files.len() as f64;
    let search_mode = if params.mode_and { "and" } else { "or" };

    let mut warnings: Vec<String> = Vec::new();
    if raw_terms.iter().any(|t| t.len() < 4) {
        warnings.push("Short substring query (<4 chars) may return broad results".to_string());
    }

    debug!("[substring-trace] Trigram index: {} tokens, {} trigrams",
        trigram_idx.tokens.len(), trigram_idx.trigram_map.len());

    let mut tokens_with_hits: HashSet<String> = HashSet::new();
    let mut file_scores: HashMap<u32, FileScoreEntry> = HashMap::new();
    let term_count = raw_terms.len();
    let mut file_matched_terms: HashMap<u32, HashSet<usize>> = HashMap::new();

    for (term_idx, term) in raw_terms.iter().enumerate() {
        let trigram_start = Instant::now();
        let matched_token_indices = find_matching_tokens_for_term(term, trigram_idx);

        debug!("[substring-trace] Trigram intersection for '{}': {} candidates in {:.3}ms",
            term, matched_token_indices.len(), trigram_start.elapsed().as_secs_f64() * 1000.0);

        let matched_tokens: Vec<String> = matched_token_indices.iter()
            .filter_map(|&idx| trigram_idx.tokens.get(idx as usize).cloned())
            .collect();

        score_token_postings(
            &matched_tokens, term_idx, index, params, total_docs,
            &mut tokens_with_hits, &mut file_scores, &mut file_matched_terms,
        );
    }

    let mut all_matched_tokens: Vec<String> = tokens_with_hits.into_iter().collect();
    all_matched_tokens.sort();

    // Set terms_matched from the distinct matched term indices
    for (file_id, entry) in &mut file_scores {
        if let Some(matched) = file_matched_terms.get(file_id) {
            entry.terms_matched = matched.len();
        }
    }

    let (mut results, total_files, total_occurrences) =
        finalize_grep_results(file_scores, params.mode_and, term_count);

    if params.max_results > 0 {
        results.truncate(params.max_results);
    }

    build_substring_response(
        &results, &raw_terms, &all_matched_tokens, &warnings,
        total_files, total_occurrences, search_mode, index, ctx, params,
    )
}


fn handle_phrase_search(
    ctx: &HandlerContext,
    index: &ContentIndex,
    phrase: &str,
    params: &GrepSearchParams,
) -> ToolCallResult {
    let show_lines = params.show_lines;
    let context_lines = params.context_lines;
    let max_results = params.max_results;
    let count_only = params.count_only;
    let search_start = params.search_start;
    let phrase_lower = phrase.to_lowercase();
    let phrase_tokens = tokenize(&phrase_lower, 2);

    if phrase_tokens.is_empty() {
        return ToolCallResult::error(format!(
            "Phrase '{}' has no indexable tokens (min length 2)", phrase
        ));
    }

    let phrase_regex_pattern = phrase_tokens.iter()
        .map(|t| regex::escape(t))
        .collect::<Vec<_>>()
        .join(r"\s+");
    let phrase_re = match regex::Regex::new(&format!("(?i){}", phrase_regex_pattern)) {
        Ok(r) => r,
        Err(e) => return ToolCallResult::error(format!("Failed to build phrase regex: {}", e)),
    };

    // Step 1: Find candidate files via AND search
    let mut candidate_file_ids: Option<std::collections::HashSet<u32>> = None;
    for token in &phrase_tokens {
        if let Some(postings) = index.index.get(token.as_str()) {
            let file_ids: std::collections::HashSet<u32> = postings.iter()
                .filter(|p| {
                    let path = match index.files.get(p.file_id as usize) {
                        Some(p) => p,
                        None => return false,
                    };
                    passes_file_filters(path, params)
                })
                .map(|p| p.file_id)
                .collect();
            candidate_file_ids = Some(match candidate_file_ids {
                Some(existing) => existing.intersection(&file_ids).cloned().collect(),
                None => file_ids,
            });
        } else {
            candidate_file_ids = Some(std::collections::HashSet::new());
            break;
        }
    }

    let candidates = candidate_file_ids.unwrap_or_default();

    // Step 2: Verify phrase match in raw file content.
    //
    // When the original phrase contains non-alphanumeric characters (XML tags,
    // angle brackets, etc.), the tokenizer strips them, causing false positives.
    // In that case, we match using the original phrase as a case-insensitive
    // substring against raw file content instead of the tokenized regex.
    // This eliminates false positives from tokenization stripping punctuation.
    let phrase_has_punctuation = phrase.chars().any(|c| !c.is_alphanumeric() && !c.is_whitespace());

    struct PhraseMatch {
        file_path: String,
        lines: Vec<u32>,
        content: Option<String>, // cached for show_lines to avoid re-reading
    }
    let mut results: Vec<PhraseMatch> = Vec::new();

    for &file_id in &candidates {
        let file_path = &index.files[file_id as usize];
        if let Ok((content, _lossy)) = read_file_lossy(std::path::Path::new(file_path)) {
            let mut matching_lines = Vec::new();
            if phrase_has_punctuation {
                // Use raw phrase substring match (case-insensitive) to avoid
                // false positives from tokenizer stripping punctuation
                for (line_num, line) in content.lines().enumerate() {
                    if line.to_lowercase().contains(&phrase_lower) {
                        matching_lines.push((line_num + 1) as u32);
                    }
                }
            } else if phrase_re.is_match(&content) {
                // Use tokenized phrase regex (no punctuation → no false positives)
                for (line_num, line) in content.lines().enumerate() {
                    if phrase_re.is_match(line) {
                        matching_lines.push((line_num + 1) as u32);
                    }
                }
            }
            if !matching_lines.is_empty() {
                results.push(PhraseMatch {
                    file_path: file_path.clone(),
                    lines: matching_lines,
                    content: if show_lines { Some(content) } else { None },
                });
            }
        }
    }

    let total_files = results.len();
    let total_occurrences: usize = results.iter().map(|r| r.lines.len()).sum();

    // Sort by number of occurrences descending (most matches first)
    results.sort_by(|a, b| b.lines.len().cmp(&a.lines.len()));

    if max_results > 0 {
        results.truncate(max_results);
    }

    let search_elapsed = search_start.elapsed();

    if count_only {
        let summary = build_grep_base_summary(
            total_files, total_occurrences, &json!([phrase]), "phrase",
            index, search_elapsed, ctx, true,
        );
        let output = json!({ "summary": summary });
        return ToolCallResult::success(json_to_string(&output));
    }

    let files_json: Vec<Value> = results.iter().map(|r| {
        let mut file_obj = json!({
            "path": r.file_path,
            "occurrences": r.lines.len(),
            "lines": r.lines,
        });

        if show_lines {
            // Use cached content from phrase verification (no second read)
            if let Some(ref content) = r.content {
                file_obj["lineContent"] = build_line_content_from_matches(content, &r.lines, context_lines);
            }
        }

        file_obj
    }).collect();

    let summary = build_grep_base_summary(
        total_files, total_occurrences, &json!([phrase]), "phrase",
        index, search_elapsed, ctx, true,
    );
    let output = json!({
        "files": files_json,
        "summary": summary
    });

    ToolCallResult::success(json_to_string(&output))
}

/// Core phrase-matching logic: finds files containing the given phrase.
/// Extracted to allow reuse by both single-phrase and multi-phrase search.
fn collect_phrase_matches(
    index: &ContentIndex,
    phrase: &str,
    params: &GrepSearchParams,
) -> Result<Vec<PhraseFileMatch>, String> {
    let show_lines = params.show_lines;

    let phrase_lower = phrase.to_lowercase();
    let phrase_tokens = tokenize(&phrase_lower, 2);

    if phrase_tokens.is_empty() {
        return Err(format!(
            "Phrase '{}' has no indexable tokens (min length 2)", phrase
        ));
    }

    let phrase_regex_pattern = phrase_tokens.iter()
        .map(|t| regex::escape(t))
        .collect::<Vec<_>>()
        .join(r"\s+");
    let phrase_re = match regex::Regex::new(&format!("(?i){}", phrase_regex_pattern)) {
        Ok(r) => r,
        Err(e) => return Err(format!("Failed to build phrase regex: {}", e)),
    };

    // Find candidate files via AND search on phrase tokens
    let mut candidate_file_ids: Option<HashSet<u32>> = None;
    for token in &phrase_tokens {
        if let Some(postings) = index.index.get(token.as_str()) {
            let file_ids: HashSet<u32> = postings.iter()
                .filter(|p| {
                    let path = match index.files.get(p.file_id as usize) {
                        Some(p) => p,
                        None => return false,
                    };
                    passes_file_filters(path, params)
                })
                .map(|p| p.file_id)
                .collect();
            candidate_file_ids = Some(match candidate_file_ids {
                Some(existing) => existing.intersection(&file_ids).cloned().collect(),
                None => file_ids,
            });
        } else {
            candidate_file_ids = Some(HashSet::new());
            break;
        }
    }

    let candidates = candidate_file_ids.unwrap_or_default();

    // Verify phrase match in raw file content.
    // When phrase contains punctuation, use raw substring match to avoid
    // false positives from tokenizer stripping non-alphanumeric characters.
    let phrase_has_punctuation = phrase.chars().any(|c| !c.is_alphanumeric() && !c.is_whitespace());

    let mut results: Vec<PhraseFileMatch> = Vec::new();

    for &file_id in &candidates {
        let file_path = &index.files[file_id as usize];
        if let Ok((content, _lossy)) = read_file_lossy(std::path::Path::new(file_path)) {
            let mut matching_lines = Vec::new();
            if phrase_has_punctuation {
                for (line_num, line) in content.lines().enumerate() {
                    if line.to_lowercase().contains(&phrase_lower) {
                        matching_lines.push((line_num + 1) as u32);
                    }
                }
            } else if phrase_re.is_match(&content) {
                for (line_num, line) in content.lines().enumerate() {
                    if phrase_re.is_match(line) {
                        matching_lines.push((line_num + 1) as u32);
                    }
                }
            }
            if !matching_lines.is_empty() {
                results.push(PhraseFileMatch {
                    file_path: file_path.clone(),
                    lines: matching_lines,
                    content: if show_lines { Some(content) } else { None },
                });
            }
        }
    }

    Ok(results)
}

/// Multi-phrase search: searches each phrase independently, merges with OR/AND semantics.
/// When only one phrase is provided, delegates to the existing single-phrase handler.
fn handle_multi_phrase_search(
    ctx: &HandlerContext,
    index: &ContentIndex,
    phrases: &[String],
    params: &GrepSearchParams,
) -> ToolCallResult {
    // Single phrase → delegate to existing handler
    if phrases.len() == 1 {
        return handle_phrase_search(ctx, index, &phrases[0], params);
    }

    let max_results = params.max_results;
    let count_only = params.count_only;
    let search_start = params.search_start;
    let show_lines = params.show_lines;
    let context_lines = params.context_lines;
    let mode_and = params.mode_and;

    // Collect matches for each phrase independently
    let mut per_phrase_results: Vec<Vec<PhraseFileMatch>> = Vec::new();
    let mut searched_terms: Vec<&str> = Vec::new();

    for phrase in phrases {
        match collect_phrase_matches(index, phrase, params) {
            Ok(matches) => {
                per_phrase_results.push(matches);
                searched_terms.push(phrase);
            }
            Err(e) => return ToolCallResult::error(e),
        }
    }

    // Merge results with OR or AND semantics
    let merged = if mode_and {
        merge_phrase_results_and(per_phrase_results)
    } else {
        merge_phrase_results_or(per_phrase_results)
    };

    let total_files = merged.len();
    let total_occurrences: usize = merged.iter().map(|r| r.lines.len()).sum();

    // Sort by occurrences descending
    let mut results = merged;
    results.sort_by(|a, b| b.lines.len().cmp(&a.lines.len()));

    if max_results > 0 {
        results.truncate(max_results);
    }

    let search_elapsed = search_start.elapsed();
    let search_mode = if mode_and { "phrase-and" } else { "phrase-or" };

    if count_only {
        let summary = build_grep_base_summary(
            total_files, total_occurrences, &json!(searched_terms), search_mode,
            index, search_elapsed, ctx, true,
        );
        let output = json!({ "summary": summary });
        return ToolCallResult::success(json_to_string(&output));
    }

    let files_json: Vec<Value> = results.iter().map(|r| {
        let mut file_obj = json!({
            "path": r.file_path,
            "occurrences": r.lines.len(),
            "lines": r.lines,
        });
        if show_lines
            && let Some(ref content) = r.content {
                file_obj["lineContent"] = build_line_content_from_matches(content, &r.lines, context_lines);
            }
        file_obj
    }).collect();

    let summary = build_grep_base_summary(
        total_files, total_occurrences, &json!(searched_terms), search_mode,
        index, search_elapsed, ctx, true,
    );
    let output = json!({
        "files": files_json,
        "summary": summary
    });

    ToolCallResult::success(json_to_string(&output))
}

/// Merge phrase results with OR semantics: union of all files.
/// If the same file appears in multiple phrase results, lines are merged and deduplicated.
fn merge_phrase_results_or(per_phrase: Vec<Vec<PhraseFileMatch>>) -> Vec<PhraseFileMatch> {
    let mut file_map: HashMap<String, PhraseFileMatch> = HashMap::new();
    for results in per_phrase {
        for m in results {
            let entry = file_map.entry(m.file_path.clone()).or_insert(PhraseFileMatch {
                file_path: m.file_path.clone(),
                lines: Vec::new(),
                content: None,
            });
            entry.lines.extend_from_slice(&m.lines);
            // Keep content if available (for show_lines)
            if entry.content.is_none() && m.content.is_some() {
                entry.content = m.content;
            }
        }
    }
    for entry in file_map.values_mut() {
        entry.lines.sort();
        entry.lines.dedup();
    }
    file_map.into_values().collect()
}

/// Merge phrase results with AND semantics: only files appearing in ALL phrase results.
fn merge_phrase_results_and(per_phrase: Vec<Vec<PhraseFileMatch>>) -> Vec<PhraseFileMatch> {
    if per_phrase.is_empty() {
        return Vec::new();
    }
    // Intersect file paths across all phrase results
    let mut common_files: HashSet<String> = per_phrase[0].iter()
        .map(|m| m.file_path.clone())
        .collect();
    for results in &per_phrase[1..] {
        let phrase_files: HashSet<String> = results.iter()
            .map(|m| m.file_path.clone())
            .collect();
        common_files = common_files.intersection(&phrase_files).cloned().collect();
    }
    // Build merged results for common files only
    let mut file_map: HashMap<String, PhraseFileMatch> = HashMap::new();
    for results in per_phrase {
        for m in results {
            if common_files.contains(&m.file_path) {
                let entry = file_map.entry(m.file_path.clone()).or_insert(PhraseFileMatch {
                    file_path: m.file_path.clone(),
                    lines: Vec::new(),
                    content: None,
                });
                entry.lines.extend_from_slice(&m.lines);
                if entry.content.is_none() && m.content.is_some() {
                    entry.content = m.content;
                }
            }
        }
    }
    for entry in file_map.values_mut() {
        entry.lines.sort();
        entry.lines.dedup();
    }
    file_map.into_values().collect()
}

#[cfg(test)]
#[path = "grep_tests.rs"]
mod grep_extracted_tests;

#[cfg(test)]
#[path = "grep_tests_additional.rs"]
mod grep_additional_tests;
