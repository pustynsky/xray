//! xray_grep handler: token search, substring search, phrase search.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tracing::{debug, info, warn};

use crate::mcp::protocol::ToolCallResult;
use crate::{read_file_lossy, tokenize, ContentIndex};
use crate::index::build_trigram_index_from_tokens;
use code_xray::generate_trigrams;

#[allow(unused_imports)] // `self` needed by test submodules for utils::ExcludePatterns
use super::utils::{self,
    build_line_content_from_matches, inject_branch_warning, is_under_dir, json_to_string,
    matches_ext_filter, read_enum_string_with_default, sorted_intersect, validate_search_dir,
};
use super::HandlerContext;

#[path = "grep_literal_extract.rs"]
mod grep_literal_extract;

/// Closed enum of accepted `mode` values for `xray_grep`.
///
/// Drift-guard: `test_all_grep_modes_drift_guard` pins the slice; any change
/// here must be paired with a downstream branch update in the term-combining
/// logic that consumes `mode_and`.
pub(crate) const ALL_GREP_MODES: &[&str] = &["or", "and"];

/// Shared parameters for substring and phrase search modes.
/// Eliminates 10+ positional parameters from handle_substring_search and handle_phrase_search.
pub(crate) struct GrepSearchParams<'a> {
    /// File extension filter. Empty = no filter; otherwise each entry is one
    /// extension (no leading dot, case-insensitive). Migrated from
    /// `Option<String>` (comma-split) to a slice in 2026-04-25.
    pub ext_filter: &'a [String],
    pub show_lines: bool,
    pub context_lines: usize,
    pub max_results: usize,
    pub mode_and: bool,
    pub count_only: bool,
    pub search_start: Instant,
    pub dir_filter: &'a Option<String>,
    /// Optional file path/name substring filter. Empty = no filter; otherwise
    /// each entry is one substring (case-insensitive, OR semantics) matched
    /// against both the full file path and the basename.
    /// Ignored when `exact_file_path` is `Some(_)` — that mode supersedes
    /// substring scoping.
    pub file_filter: &'a [String],
    /// When `Some(path)`, the file's full normalized path must equal this value
    /// exactly (case-insensitive, `\` normalized to `/`). Set ONLY by the
    /// `dir=<file>` auto-convert branch in `parse_grep_args`. The previous
    /// basename-only check let `subdir/Service.cs` leak when the user pointed
    /// `dir=<root>/Service.cs`; full-path equality closes that hole. User-provided
    /// `file=` keeps substring/comma-OR semantics (this stays `None`).
    pub exact_file_path: &'a Option<String>,
    /// Optional canonical form of `exact_file_path`, populated ONLY when the
    /// canonical file is still inside the workspace. This is a narrow fallback
    /// for Windows short/long path-form mismatches (`RUNNER~1` vs `runneradmin`)
    /// while preserving logical-path semantics for symlinked workspace paths.
    pub exact_file_path_canonical: &'a Option<String>,
    /// Pre-computed exclude dir patterns (avoids per-file allocations)
    pub exclude_patterns: super::utils::ExcludePatterns,
    /// Pre-lowercased exclude path substrings
    pub exclude_lower: Vec<String>,
    /// Optional note to include in response summary when `dir=` was auto-converted
    /// from a file path to parent dir + file filter.
    pub dir_auto_converted_note: Option<String>,
    /// When true (default) and the query is multi-term substring-OR, post-process
    /// results so a dominant common term cannot starve rare-term matches out of
    /// the response. See [`apply_auto_balance`].
    pub auto_balance: bool,
    /// Optional explicit cap (in files) for the dominant-only group when
    /// auto-balance triggers. `None` lets [`apply_auto_balance`] derive it from
    /// `2 * second_max` clamped to `[20, 100]`.
    pub max_occurrences_per_term: Option<usize>,
    /// Time spent waiting for read lock on content index (ms).
    /// Non-zero indicates lock contention from concurrent writers.
    pub lock_wait_ms: f64,
    /// When true, trigram rebuild was skipped (scope narrow or auto-phrase).
    /// Trigram-backed prefilters must be disabled to avoid false negatives
    /// from stale trigram data.
    pub trigram_stale: bool,
    /// User-visible mode requested before any internal auto-switch.
    pub requested_mode: &'a str,
}

pub(crate) struct FileScoreEntry {
    pub file_path: String,
    pub lines: Vec<u32>,
    pub tf_idf: f64,
    pub occurrences: usize,
    pub terms_matched: usize,
    /// Per-term occurrence counts, indexed by term position in the parsed
    /// `terms_str` (grown lazily during scoring). Required by
    /// [`apply_auto_balance`] to detect a single dominant term and trim
    /// dominant-only files when one term swamps the rest.
    pub per_term_occurrences: Vec<usize>,
}

/// A single file match from phrase search, with matched lines and optionally cached content.
pub(crate) struct PhraseFileMatch {
    pub file_path: String,
    pub lines: Vec<u32>,
    pub content: Option<String>,
}

/// Outcome of [`apply_auto_balance`]. Surfaced in the response as
/// `summary.autoBalance` so callers can tell that the result set was trimmed
/// (and which term was dominant) rather than silently see fewer rows.
#[derive(Debug, Clone)]
pub(crate) struct AutoBalanceInfo {
    pub dominant_term: String,
    pub dominant_occurrences: usize,
    pub second_max_occurrences: usize,
    pub min_nonzero_occurrences: usize,
    pub ratio: f64,
    pub cap: usize,
    pub dropped_files: usize,
}

/// Trim dominant-only files when ONE term contributes >10x more occurrences
/// than the rarest matched term. Without this, mixed queries like
/// `terms="TODO, clearTimeout, localStorage"` are dominated by `localStorage`
/// (~2k matches) and the rare TODO/clearTimeout matches get pushed off the
/// `maxResults` window — the user sees a noisy list of `localStorage`-only
/// files and concludes the rare terms don't exist.
///
/// Strategy: keep every file matched by ≥2 distinct terms (cross-term
/// relevance is the high-signal case). Among files matched ONLY by the
/// dominant term, keep the top `cap` by `tf_idf` and drop the rest. Returns
/// `None` when no balancing is needed (single-matched-term query, ratio
/// below threshold, or no dominant-only files to drop).
///
/// Cap derivation: `user_cap` if provided, else `2 * second_max_occurrences`
/// clamped to `[20, 100]` — small enough to keep the response focused, large
/// enough that the dominant term's strongest hits still surface.
pub(crate) fn apply_auto_balance(
    results: &mut Vec<FileScoreEntry>,
    term_count: usize,
    raw_terms: &[String],
    user_cap: Option<usize>,
) -> Option<AutoBalanceInfo> {
    if term_count < 2 || results.is_empty() {
        return None;
    }

    // Aggregate per-term occurrences across the *full* result set (this runs
    // before max_results truncation, so the imbalance signal is accurate).
    let mut per_term_occ = vec![0usize; term_count];
    for r in results.iter() {
        for (i, &occ) in r.per_term_occurrences.iter().enumerate() {
            if i < term_count {
                per_term_occ[i] += occ;
            }
        }
    }

    let nonzero: Vec<usize> = per_term_occ.iter().copied().filter(|&v| v > 0).collect();
    if nonzero.len() < 2 {
        return None;
    }
    let max_occ = *nonzero.iter().max().unwrap();
    let min_occ = *nonzero.iter().min().unwrap();
    if max_occ < min_occ.saturating_mul(10) {
        return None;
    }

    let dominant_idx = per_term_occ
        .iter()
        .enumerate()
        .max_by_key(|&(_, &v)| v)
        .map(|(i, _)| i)?;
    let mut sorted = per_term_occ.clone();
    sorted.sort_unstable();
    let second_max = sorted.iter().rev().nth(1).copied().unwrap_or(0);

    let cap = user_cap.unwrap_or_else(|| {
        let derived = second_max.saturating_mul(2);
        derived.clamp(20, 100)
    });

    // Sort by tf_idf descending to keep the strongest dominant-only files.
    // Stable indices via enumerate so we can mirror the "keep" decision
    // back into the original `results` order at the end.
    let mut indexed: Vec<(usize, f64, bool)> = results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let dom_only = r
                .per_term_occurrences
                .get(dominant_idx)
                .copied()
                .unwrap_or(0)
                > 0
                && r.per_term_occurrences
                    .iter()
                    .enumerate()
                    .all(|(j, &occ)| j == dominant_idx || occ == 0);
            (i, r.tf_idf, dom_only)
        })
        .collect();
    indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut keep = vec![true; results.len()];
    let mut kept_dominant_only = 0usize;
    let mut dropped = 0usize;
    for (orig_idx, _tf, dom_only) in &indexed {
        if *dom_only {
            if kept_dominant_only < cap {
                kept_dominant_only += 1;
            } else {
                keep[*orig_idx] = false;
                dropped += 1;
            }
        }
    }

    if dropped == 0 {
        return None;
    }

    let mut idx = 0usize;
    results.retain(|_| {
        let k = keep[idx];
        idx += 1;
        k
    });

    Some(AutoBalanceInfo {
        dominant_term: raw_terms.get(dominant_idx).cloned().unwrap_or_default(),
        dominant_occurrences: max_occ,
        second_max_occurrences: second_max,
        min_nonzero_occurrences: min_occ,
        ratio: max_occ as f64 / min_occ.max(1) as f64,
        cap,
        dropped_files: dropped,
    })
}

/// Render an [`AutoBalanceInfo`] into the response summary so the caller can
/// see (a) which term was dominant, (b) how many files were dropped, and
/// (c) the opt-out instructions.
fn inject_auto_balance(summary: &mut Value, info: &AutoBalanceInfo) {
    let warning = format!(
        "Auto-balanced: '{}' had {} occurrences ({:.0}× more than the rarest matched term: {}). \
         {} dominant-only file(s) trimmed beyond cap={} to keep rare-term matches visible. \
         Pass autoBalance=false to disable, or maxOccurrencesPerTerm=N to set an explicit cap.",
        info.dominant_term,
        info.dominant_occurrences,
        info.ratio,
        info.min_nonzero_occurrences,
        info.dropped_files,
        info.cap,
    );
    summary["autoBalance"] = json!({
        "dominantTerm": info.dominant_term,
        "dominantOccurrences": info.dominant_occurrences,
        "secondMaxOccurrences": info.second_max_occurrences,
        "minNonzeroOccurrences": info.min_nonzero_occurrences,
        "ratio": (info.ratio * 100.0).round() / 100.0,
        "cap": info.cap,
        "droppedFiles": info.dropped_files,
        "hint": warning.clone(),
    });
    // Append to existing warnings array (or create one) so warning-aware
    // clients see it without having to look at a new field.
    let warnings_entry = summary.as_object_mut().and_then(|m| {
        m.entry("warnings".to_string()).or_insert_with(|| json!([])).as_array_mut()
    });
    if let Some(arr) = warnings_entry {
        arr.push(json!(warning));
    }
}

/// Build the common grep summary JSON with readErrors, lossyUtf8Files, and branchWarning.
/// When `include_index_stats` is true, adds indexFiles, indexTokens, searchTimeMs, indexLoadTimeMs.
#[allow(clippy::too_many_arguments)]
fn build_grep_base_summary(
    total_files: usize,
    total_occurrences: usize,
    terms: &Value,
    search_mode: &str,
    index: &ContentIndex,
    search_elapsed: std::time::Duration,
    ctx: &HandlerContext,
    include_index_stats: bool,
    lock_wait_ms: f64,
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
        if lock_wait_ms > 0.5 {
            summary["lockWaitMs"] = json!(format!("{:.1}", lock_wait_ms));
        }
    }
    if index.read_errors > 0 {
        summary["readErrors"] = json!(index.read_errors);
    }
    if index.lossy_file_count > 0 {
        summary["lossyUtf8Files"] = json!(index.lossy_file_count);
    }
    inject_branch_warning(&mut summary, ctx);
    if let Some(hint) = line_regex_perf_hint(
        search_mode,
        search_elapsed.as_millis() as u64,
        index.files.len(),
        false, // default copy assumes no prefilter; lineRegex callers may
               // override via `apply_literal_prefilter_summary` when the
               // prefilter actually ran.
        None,
        None,
    ) {
        // Use a dedicated `perfHint` field so a later truncation pass
        // (`truncate_large_response`, which writes `summary["hint"]`) cannot
        // overwrite this guidance. A slow lineRegex scan over a large repo is
        // exactly the kind of response most likely to trip the byte cap, so
        // overloading the same key would silently swallow the perf hint.
        summary["perfHint"] = json!(hint);
    }
    summary
}

fn grep_requested_mode(parsed: &ParsedGrepArgs) -> &'static str {
    if parsed.use_line_regex {
        "lineRegex"
    } else if parsed.use_phrase {
        "phrase"
    } else if parsed.use_regex {
        "regex"
    } else if parsed.use_substring {
        "substring"
    } else {
        "token"
    }
}

fn effective_mode_family(effective_mode: &str) -> &str {
    if effective_mode.starts_with("substring") {
        "substring"
    } else if effective_mode.starts_with("phrase") {
        "phrase"
    } else if effective_mode.starts_with("lineRegex") {
        "lineRegex"
    } else if matches!(effective_mode, "or" | "and") {
        "token"
    } else {
        effective_mode
    }
}

fn grep_cost_class(effective_mode: &str, mode_changed: bool) -> &'static str {
    if mode_changed || matches!(effective_mode_family(effective_mode), "phrase" | "lineRegex") {
        "expensive"
    } else {
        "normal"
    }
}

fn build_grep_execution(
    params: &GrepSearchParams,
    effective_mode: &str,
    reason: Option<&str>,
    prefilter_used: bool,
    candidate_files: Option<usize>,
    files_scanned: Option<usize>,
) -> Value {
    let effective_family = effective_mode_family(effective_mode);
    let mode_changed = params.requested_mode != effective_family;
    let effective_reason = reason.or_else(|| {
        if params.requested_mode == "substring" && effective_family == "phrase" {
            Some("term contains punctuation or whitespace that tokenizer strips")
        } else if params.requested_mode == "lineRegex" && !prefilter_used {
            Some("lineRegex ran without a literal prefilter")
        } else {
            None
        }
    });
    let mut execution = json!({
        "requestedMode": params.requested_mode,
        "effectiveMode": effective_family,
        "modeChanged": mode_changed,
        "costClass": grep_cost_class(effective_mode, mode_changed),
        "prefilterUsed": prefilter_used,
    });
    if let Some(reason) = effective_reason {
        execution["reason"] = json!(reason);
    }
    if let Some(candidate_files) = candidate_files {
        execution["candidateFiles"] = json!(candidate_files);
    }
    if let Some(files_scanned) = files_scanned {
        execution["filesScanned"] = json!(files_scanned);
    }
    execution
}

fn build_grep_result_status(
    returned_files: usize,
    returned_occurrences: usize,
    total_files: usize,
    total_occurrences: usize,
    count_only: bool,
) -> Value {
    if count_only {
        let mut status = super::utils::build_result_status(
            "counts_only",
            true,
            true,
            false,
            "counts_only",
            Vec::new(),
        );
        status["total"] = json!({ "files": total_files, "occurrences": total_occurrences });
        status["totalKnown"] = json!(true);
        return status;
    }

    let not_found = total_files == 0 && total_occurrences == 0;
    let partial = returned_files < total_files;
    let mut status = super::utils::build_result_status(
        if not_found { "not_found" } else if partial { "partial" } else { "complete" },
        !partial,
        !partial,
        false,
        "snippet",
        if not_found {
            vec!["no_matches".to_string()]
        } else if partial {
            vec!["max_results".to_string()]
        } else {
            Vec::new()
        },
    );
    super::utils::add_collection_accounting(
        &mut status,
        json!({ "files": returned_files, "occurrences": returned_occurrences }),
        json!({ "files": total_files, "occurrences": total_occurrences }),
    );
    status
}

fn add_grep_next_queries(output: &mut Value, terms: &Value, params: &GrepSearchParams) {
    let is_partial = output
        .get("resultStatus")
        .and_then(|status| status.get("status"))
        .and_then(Value::as_str)
        == Some("partial");
    let is_expensive = output
        .get("execution")
        .and_then(|execution| execution.get("costClass"))
        .and_then(Value::as_str)
        == Some("expensive");
    if !is_partial && !is_expensive {
        return;
    }

    let mut suggestions = Vec::new();
    if is_partial {
        suggestions.push(json!({
            "tool": "xray_grep",
            "args": { "terms": terms, "countOnly": true },
            "reason": "get exhaustive counts before claiming completeness",
        }));
    }
    if is_expensive && params.requested_mode == "substring" {
        suggestions.push(json!({
            "tool": "xray_grep",
            "args": { "terms": terms, "lineRegex": true },
            "reason": "make raw-line matching explicit when punctuation or anchors matter",
        }));
    }
    if !suggestions.is_empty() {
        output["recommendedNextQueries"] = json!(suggestions);
    }
}

fn finalize_grep_output(
    mut output: Value,
    result_status: Value,
    execution: Value,
    terms: &Value,
    params: &GrepSearchParams,
) -> Value {
    let mut ordered = serde_json::Map::new();
    ordered.insert("execution".to_string(), execution);
    if let Value::Object(map) = output {
        for (key, value) in map {
            if key != "execution" && key != "resultStatus" {
                ordered.insert(key, value);
            }
        }
    }
    output = Value::Object(ordered);
    output = super::utils::attach_result_status(output, result_status);
    add_grep_next_queries(&mut output, terms, params);
    output
}


/// Threshold (milliseconds) above which a `lineRegex` scan is considered slow
/// enough to warrant a performance hint in the response. Patterns that reduce
/// to a fixed substring should run via `terms=[...]` (trigram-prefiltered) and
/// finish in well under 10 ms even on large repos.
const LINE_REGEX_SLOW_MS: u64 = 2000;

/// Lower bound on indexed file count for the slow-scan hint. On tiny repos a
/// 2 s lineRegex scan is not actionable advice (the user already has fast
/// feedback) — only surface the hint when the cost actually scales with
/// repo size.
const LINE_REGEX_LARGE_INDEX_FILES: usize = 1000;

const LINE_REGEX_DOMINANT_PHASE_MIN_MS: u64 = 100;
const LINE_REGEX_PARALLEL_SCAN_MIN_FILES: usize = 512;
const LINE_REGEX_PARALLEL_SCAN_MAX_THREADS: usize = 8;

#[cfg(not(test))]
const LINE_REGEX_MAX_CONTENT_CACHE_BYTES: usize = 256 * 1024 * 1024; // 256 MiB
#[cfg(test)]
const LINE_REGEX_MAX_CONTENT_CACHE_BYTES: usize = 4 * 1024; // 4 KiB

const LINE_REGEX_RESPONSE_FINALIZE_PLACEHOLDER_MS: u64 = 9_999_999_999_999;
const LINE_REGEX_PERF_HINT_PLACEHOLDER: &str = "__xray_line_regex_perf_hint_placeholder__";

/// AC-1: returns a human-readable performance hint when a `lineRegex` query
/// triggered a costly full-file scan. Returns `None` for non-lineRegex modes,
/// fast scans, or small indexes — i.e. when the hint would be noise.
///
/// Pure function: thresholds are module-level `const`s so the helper is unit
/// testable without spinning up a real index. Callers inject the returned
/// string into `summary["perfHint"]` (a dedicated key, distinct from the
/// generic `summary["hint"]` written by `truncate_large_response`, so the
/// two pieces of guidance can coexist on a truncated response).
///
/// `index_files` is the size of the indexed corpus (upper bound on the scan
/// set). The actual count of files read depends on file/ext/dir filters and
/// is not threaded through here, so the message uses upper-bound phrasing
/// ("index of N files") rather than implying every file was read.
/// Look up the file_id set whose tokens contain `term` as a substring.
/// Wraps the existing trigram-intersection helper with the file_id
/// projection that [`compute_literal_prefilter`] needs.
fn files_containing_substring(
    term: &str,
    trigram_idx: &crate::TrigramIndex,
    inverted: &HashMap<String, Vec<crate::Posting>>,
) -> HashSet<u32> {
    let token_ids = find_matching_tokens_for_term(term, trigram_idx);
    let mut out = HashSet::new();
    for tok_id in token_ids {
        if let Some(token) = trigram_idx.tokens.get(tok_id as usize)
            && let Some(postings) = inverted.get(token)
        {
            for p in postings {
                out.insert(p.file_id);
            }
        }
    }
    out
}


/// Information about an attempted literal-trigram prefilter pass for
/// `lineRegex` mode. Populated by [`compute_literal_prefilter`] and exposed
/// by [`apply_literal_prefilter_summary`] as `summary.literalPrefilter` so
/// clients can see whether/why the prefilter narrowed the search.
#[derive(Debug, Clone, Default)]
struct LiteralPrefilterInfo {
    /// Whether the prefilter actually narrowed the iteration. `false` when
    /// extraction failed for any required pattern, when the OR-mode contract
    /// admitted an unprefilterable pattern, or when the candidate ratio
    /// exceeded [`LITERAL_PREFILTER_MAX_RATIO`] (short-circuit fallback).
    used: bool,
    /// Number of files in the candidate set when `used == true`. When
    /// `short_circuited` is set, this is the candidate count that *would* have
    /// been used had the ratio guard not tripped (informational).
    candidate_files: usize,
    /// `index.files.len()` snapshot at the time the prefilter ran.
    total_files: usize,
    /// Word-shaped literal *fragments* fed into the trigram lookup, after
    /// extraction + non-word splitting + lowercasing + dedup. NOT identical
    /// to the raw literals returned by `regex-syntax` (e.g. `"pub fn"` becomes
    /// the single fragment `["pub"]` because `"fn"` falls below the trigram
    /// floor). Capped at 5 entries when serialised; full list kept here for
    /// debug logging.
    extracted_fragments: Vec<String>,
    /// True iff the candidate ratio exceeded
    /// [`LITERAL_PREFILTER_MAX_RATIO`] and we fell back to the full scan.
    short_circuited: bool,
    /// Human-readable explanation when `used == false`. Surfaced in the
    /// summary so clients understand why the prefilter was skipped.
    reason: Option<String>,
    /// Number of files in `index.files` that survive `passes_file_filters`
    /// (i.e. respect `dir`/`file`/`ext`/`exclude` scope), regardless of the
    /// trigram prefilter. `None` when no scope filter is set — in that case
    /// the unscoped `total_files` already answers the question. Surfaced as
    /// `summary.literalPrefilter.totalFilesAfterScope` so clients can tell
    /// "prefilter narrowed" apart from "scope narrowed" (the cross-validation
    /// finding that motivated the alternation-split advisory revert).
    total_files_after_scope: Option<usize>,
    /// Intersection of the candidate set with the scope-filtered file set.
    /// `None` when no scope filter is set OR when no candidate set was
    /// produced (`used == false`). Surfaced as
    /// `summary.literalPrefilter.candidateFilesAfterScope`.
    candidate_files_after_scope: Option<usize>,
}

#[derive(Debug, Clone, Default)]
struct LineRegexScanTelemetry {
    compile_duration: Duration,
    literal_prefilter_duration: Duration,
    scope_count_duration: Duration,
    scan_duration: Duration,
    candidate_filter_duration: Duration,
    scope_filter_duration: Duration,
    read_duration: Duration,
    whole_file_precheck_duration: Duration,
    line_eval_duration: Duration,
    match_bookkeeping_duration: Duration,
    merge_duration: Duration,
    sort_dedup_duration: Duration,
    rank_truncate_duration: Duration,
    response_build_duration: Duration,
    response_finalize_duration: Duration,
    parallel_scan: bool,
    worker_threads: usize,
    files_visited: usize,
    files_skipped_by_prefilter: usize,
    files_skipped_by_scope: usize,
    files_read: usize,
    bytes_read: usize,
    whole_file_precheck_files: usize,
    whole_file_precheck_matched_files: usize,
    line_eval_files: usize,
    line_eval_lines: usize,
    matched_files: usize,
}

#[derive(Debug, Clone, Default)]
struct LineRegexFileCounters {
    files_visited: usize,
    files_skipped_by_prefilter: usize,
    files_skipped_by_scope: usize,
    files_read: usize,
    bytes_read: usize,
    whole_file_precheck_files: usize,
    whole_file_precheck_matched_files: usize,
    line_eval_files: usize,
    line_eval_lines: usize,
}

#[derive(Debug, Clone, Default)]
struct LineRegexFileTimings {
    candidate_filter_duration: Duration,
    scope_filter_duration: Duration,
    read_duration: Duration,
    whole_file_precheck_duration: Duration,
    line_eval_duration: Duration,
    match_bookkeeping_duration: Duration,
}

#[derive(Debug, Clone, Default)]
struct LineRegexFileScanOutput {
    path: Option<String>,
    matched_lines_by_pattern: Vec<(usize, Vec<u32>)>,
    counters: LineRegexFileCounters,
    timings: LineRegexFileTimings,
}

impl LineRegexScanTelemetry {
    fn add_file_scan_output(&mut self, output: &LineRegexFileScanOutput) {
        self.candidate_filter_duration += output.timings.candidate_filter_duration;
        self.scope_filter_duration += output.timings.scope_filter_duration;
        self.read_duration += output.timings.read_duration;
        self.whole_file_precheck_duration += output.timings.whole_file_precheck_duration;
        self.line_eval_duration += output.timings.line_eval_duration;
        self.match_bookkeeping_duration += output.timings.match_bookkeeping_duration;
        self.files_visited = self.files_visited.saturating_add(output.counters.files_visited);
        self.files_skipped_by_prefilter = self.files_skipped_by_prefilter
            .saturating_add(output.counters.files_skipped_by_prefilter);
        self.files_skipped_by_scope = self.files_skipped_by_scope
            .saturating_add(output.counters.files_skipped_by_scope);
        self.files_read = self.files_read.saturating_add(output.counters.files_read);
        self.bytes_read = self.bytes_read.saturating_add(output.counters.bytes_read);
        self.whole_file_precheck_files = self.whole_file_precheck_files
            .saturating_add(output.counters.whole_file_precheck_files);
        self.whole_file_precheck_matched_files = self.whole_file_precheck_matched_files
            .saturating_add(output.counters.whole_file_precheck_matched_files);
        self.line_eval_files = self.line_eval_files.saturating_add(output.counters.line_eval_files);
        self.line_eval_lines = self.line_eval_lines.saturating_add(output.counters.line_eval_lines);
    }

    fn compile_ms(&self) -> u64 {
        duration_ms_u64(self.compile_duration)
    }

    fn literal_prefilter_ms(&self) -> u64 {
        duration_ms_u64(self.literal_prefilter_duration)
    }

    fn scope_count_ms(&self) -> u64 {
        duration_ms_u64(self.scope_count_duration)
    }

    fn scan_ms(&self) -> u64 {
        duration_ms_u64(self.scan_duration)
    }

    fn candidate_filter_ms(&self) -> u64 {
        duration_ms_u64(self.candidate_filter_duration)
    }

    fn scope_filter_ms(&self) -> u64 {
        duration_ms_u64(self.scope_filter_duration)
    }

    fn read_ms(&self) -> u64 {
        duration_ms_u64(self.read_duration)
    }

    fn whole_file_precheck_ms(&self) -> u64 {
        duration_ms_u64(self.whole_file_precheck_duration)
    }

    fn line_eval_ms(&self) -> u64 {
        duration_ms_u64(self.line_eval_duration)
    }

    fn match_bookkeeping_ms(&self) -> u64 {
        duration_ms_u64(self.match_bookkeeping_duration)
    }

    fn merge_ms(&self) -> u64 {
        duration_ms_u64(self.merge_duration)
    }

    fn sort_dedup_ms(&self) -> u64 {
        duration_ms_u64(self.sort_dedup_duration)
    }

    fn rank_truncate_ms(&self) -> u64 {
        duration_ms_u64(self.rank_truncate_duration)
    }

    fn response_build_ms(&self) -> u64 {
        duration_ms_u64(self.response_build_duration)
    }

    fn response_finalize_ms(&self) -> u64 {
        duration_ms_u64(self.response_finalize_duration)
    }

    fn measured_scan_phase_ms(&self) -> u64 {
        self.candidate_filter_ms()
            .saturating_add(self.scope_filter_ms())
            .saturating_add(self.read_ms())
            .saturating_add(self.whole_file_precheck_ms())
            .saturating_add(self.line_eval_ms())
            .saturating_add(self.match_bookkeeping_ms())
    }

    fn scan_bottleneck_ms(&self, phase_ms: u64) -> u64 {
        let scan_wall_ms = self.scan_ms();
        let measured_scan_ms = self.measured_scan_phase_ms();
        if !self.parallel_scan || scan_wall_ms == 0 || measured_scan_ms <= scan_wall_ms {
            return phase_ms;
        }
        let scaled = u128::from(phase_ms)
            .saturating_mul(u128::from(scan_wall_ms))
            / u128::from(measured_scan_ms);
        scaled.min(u128::from(u64::MAX)) as u64
    }

    fn candidate_filter_bottleneck_ms(&self) -> u64 {
        self.scan_bottleneck_ms(self.candidate_filter_ms())
    }

    fn scope_filter_bottleneck_ms(&self) -> u64 {
        self.scan_bottleneck_ms(self.scope_filter_ms())
    }

    fn read_bottleneck_ms(&self) -> u64 {
        self.scan_bottleneck_ms(self.read_ms())
    }

    fn whole_file_precheck_bottleneck_ms(&self) -> u64 {
        self.scan_bottleneck_ms(self.whole_file_precheck_ms())
    }

    fn line_eval_bottleneck_ms(&self) -> u64 {
        self.scan_bottleneck_ms(self.line_eval_ms())
    }

    fn match_bookkeeping_bottleneck_ms(&self) -> u64 {
        self.scan_bottleneck_ms(self.match_bookkeeping_ms())
    }

    fn observed_total_ms(&self) -> u64 {
        self.scan_ms()
            .saturating_add(self.merge_ms())
            .saturating_add(self.sort_dedup_ms())
            .saturating_add(self.rank_truncate_ms())
            .saturating_add(self.response_build_ms())
            .saturating_add(self.response_finalize_ms())
    }

    fn scan_residual_ms(&self) -> u64 {
        self.scan_ms().saturating_sub(self.measured_scan_phase_ms())
    }

    fn to_json(&self) -> Value {
        json!({
            "compileMs": self.compile_ms(),
            "literalPrefilterMs": self.literal_prefilter_ms(),
            "scopeCountMs": self.scope_count_ms(),
            "scanMs": self.scan_ms(),
            "parallelScan": self.parallel_scan,
            "workerThreads": self.worker_threads,
            "scanWallMs": self.scan_ms(),
            "readMs": self.read_bottleneck_ms(),
            "readSumMs": self.read_ms(),
            "wholeFilePrecheckMs": self.whole_file_precheck_bottleneck_ms(),
            "wholeFilePrecheckSumMs": self.whole_file_precheck_ms(),
            "lineEvalMs": self.line_eval_bottleneck_ms(),
            "lineEvalSumMs": self.line_eval_ms(),
            "candidateFilterMs": self.candidate_filter_bottleneck_ms(),
            "candidateFilterSumMs": self.candidate_filter_ms(),
            "scopeFilterMs": self.scope_filter_bottleneck_ms(),
            "scopeFilterSumMs": self.scope_filter_ms(),
            "matchBookkeepingMs": self.match_bookkeeping_bottleneck_ms(),
            "matchBookkeepingSumMs": self.match_bookkeeping_ms(),
            "mergeMs": self.merge_ms(),
            "sortDedupMs": self.sort_dedup_ms(),
            "rankTruncateMs": self.rank_truncate_ms(),
            "responseBuildMs": self.response_build_ms(),
            "responseFinalizeMs": self.response_finalize_ms(),
            "scanResidualMs": self.scan_residual_ms(),
            "filesVisited": self.files_visited,
            "filesSkippedByPrefilter": self.files_skipped_by_prefilter,
            "filesSkippedByScope": self.files_skipped_by_scope,
            "filesRead": self.files_read,
            "bytesRead": self.bytes_read,
            "wholeFilePrecheckFiles": self.whole_file_precheck_files,
            "wholeFilePrecheckMatchedFiles": self.whole_file_precheck_matched_files,
            "lineEvalFiles": self.line_eval_files,
            "lineEvalLines": self.line_eval_lines,
            "matchedFiles": self.matched_files,
        })
    }
}

fn duration_ms_u64(duration: Duration) -> u64 {
    let millis = duration.as_millis();
    millis.min(u128::from(u64::MAX)) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineRegexBottleneck {
    FileRead,
    WholeFilePrecheck,
    LineEvaluation,
    Filtering,
    MatchBookkeeping,
    MergeSortOrResponse,
    ScanResidual,
    NoClearDominantPhase,
}

fn classify_line_regex_bottleneck(telemetry: &LineRegexScanTelemetry) -> LineRegexBottleneck {
    let merge_sort_or_response_ms = telemetry
        .merge_ms()
        .saturating_add(telemetry.sort_dedup_ms())
        .saturating_add(telemetry.rank_truncate_ms())
        .saturating_add(telemetry.response_build_ms())
        .saturating_add(telemetry.response_finalize_ms());
    let filtering_ms = telemetry
        .candidate_filter_bottleneck_ms()
        .saturating_add(telemetry.scope_filter_bottleneck_ms());
    let phases = [
        (LineRegexBottleneck::FileRead, telemetry.read_bottleneck_ms()),
        (
            LineRegexBottleneck::WholeFilePrecheck,
            telemetry.whole_file_precheck_bottleneck_ms(),
        ),
        (LineRegexBottleneck::LineEvaluation, telemetry.line_eval_bottleneck_ms()),
        (LineRegexBottleneck::Filtering, filtering_ms),
        (
            LineRegexBottleneck::MatchBookkeeping,
            telemetry.match_bookkeeping_bottleneck_ms(),
        ),
        (LineRegexBottleneck::MergeSortOrResponse, merge_sort_or_response_ms),
        (LineRegexBottleneck::ScanResidual, telemetry.scan_residual_ms()),
    ];
    let (phase, phase_ms) = phases
        .into_iter()
        .max_by_key(|(_, phase_ms)| *phase_ms)
        .unwrap_or((LineRegexBottleneck::NoClearDominantPhase, 0));
    if phase_ms < LINE_REGEX_DOMINANT_PHASE_MIN_MS
        || phase_ms.saturating_mul(4) < telemetry.observed_total_ms()
    {
        LineRegexBottleneck::NoClearDominantPhase
    } else {
        phase
    }
}

fn line_regex_phase_hint(
    telemetry: &LineRegexScanTelemetry,
    search_elapsed_ms: u64,
    index_files: usize,
    prefilter_used: bool,
    prefilter_reason: Option<&str>,
) -> String {
    let prefilter_context = if prefilter_used {
        let candidate_files = telemetry
            .files_visited
            .saturating_sub(telemetry.files_skipped_by_prefilter);
        format!(
            "The literal-trigram prefilter narrowed the scan to {} candidate files.",
            candidate_files
        )
    } else if let Some(reason) = prefilter_reason {
        format!(
            "The literal-trigram prefilter was attempted but did not narrow the search \
             (literalPrefilter.reason: \"{}\").",
            reason
        )
    } else {
        "The literal-trigram prefilter could not narrow the search.".to_string()
    };

    let phase_context = match classify_line_regex_bottleneck(telemetry) {
        LineRegexBottleneck::FileRead => "Measured telemetry shows file reads dominate the scan; narrow scope with dir=/file=/ext= or use a literal terms=[\"...\"] query when possible.",
        LineRegexBottleneck::WholeFilePrecheck => "Measured telemetry shows whole-file regex prechecks dominate; simplify the regex or add a more selective literal prefix if possible.",
        LineRegexBottleneck::LineEvaluation => "Measured telemetry shows per-line regex evaluation is the dominant phase; simplify lookarounds, unicode classes, and nested quantifiers.",
        LineRegexBottleneck::Filtering => "Measured telemetry shows candidate/scope filtering dominates; narrow dir=/file=/ext= filters or use more selective file filters.",
        LineRegexBottleneck::MatchBookkeeping => "Measured telemetry shows match bookkeeping dominates; reduce showLines/context work or narrow the match set.",
        LineRegexBottleneck::MergeSortOrResponse => "Measured telemetry shows merge/sort/truncation/response building dominates; reduce maxResults/showLines/contextLines or narrow the match set.",
        LineRegexBottleneck::ScanResidual => "Measured telemetry shows significant residual scan-loop overhead outside named phases; inspect summary.lineRegexScan before choosing the next optimization.",
        LineRegexBottleneck::NoClearDominantPhase => "No single measured phase clearly dominates; inspect summary.lineRegexScan for the phase breakdown before choosing a mitigation.",
    };

    format!(
        "lineRegex took {}ms over an index of {} files. {} {} See `xray_help tool=\"xray_grep\"` for full guidance.",
        search_elapsed_ms, index_files, prefilter_context, phase_context
    )
}

fn apply_line_regex_scan_summary(summary: &mut Value, telemetry: &LineRegexScanTelemetry) {
    if let Some(obj) = summary.as_object_mut() {
        obj.insert("lineRegexScan".to_string(), telemetry.to_json());
    }
}

fn finalize_line_regex_summary(
    summary: &mut Value,
    prefilter_info: &LiteralPrefilterInfo,
    scan_telemetry: &mut LineRegexScanTelemetry,
    total_elapsed_ms: u64,
    search_mode: &str,
) -> String {
    apply_literal_prefilter_summary(
        summary,
        prefilter_info,
        scan_telemetry,
        total_elapsed_ms,
        search_mode,
    );
    if search_mode.starts_with("lineRegex")
        && prefilter_info.total_files >= LINE_REGEX_LARGE_INDEX_FILES
        && let Some(obj) = summary.as_object_mut()
    {
        obj.insert(
            "perfHint".to_string(),
            json!(LINE_REGEX_PERF_HINT_PLACEHOLDER),
        );
    }
    scan_telemetry.response_finalize_duration =
        Duration::from_millis(LINE_REGEX_RESPONSE_FINALIZE_PLACEHOLDER_MS);
    apply_line_regex_scan_summary(summary, scan_telemetry);
    scan_telemetry.response_finalize_duration = Duration::default();
    json_to_string(summary)
}

fn replace_response_finalize_placeholder(response: String, response_finalize_ms: u64) -> String {
    response.replacen(
        &format!(
            "\"responseFinalizeMs\":{}",
            LINE_REGEX_RESPONSE_FINALIZE_PLACEHOLDER_MS
        ),
        &format!("\"responseFinalizeMs\":{}", response_finalize_ms),
        1,
    )
}

fn replace_perf_hint_placeholder(response: String, hint: Option<String>) -> String {
    let placeholder_json = json_to_string(&json!(LINE_REGEX_PERF_HINT_PLACEHOLDER));
    let placeholder_field = format!("\"perfHint\":{}", placeholder_json);
    if let Some(hint) = hint {
        return response.replacen(
            &placeholder_field,
            &format!("\"perfHint\":{}", json_to_string(&json!(hint))),
            1,
        );
    }
    for pattern in [
        format!(",{}", placeholder_field),
        format!("{},", placeholder_field),
        placeholder_field,
    ] {
        if response.contains(&pattern) {
            return response.replacen(&pattern, "", 1);
        }
    }
    response
}

fn final_line_regex_perf_hint(
    search_mode: &str,
    total_elapsed_ms: u64,
    prefilter_info: &LiteralPrefilterInfo,
    scan_telemetry: &LineRegexScanTelemetry,
) -> Option<String> {
    line_regex_perf_hint(
        search_mode,
        total_elapsed_ms,
        prefilter_info.total_files,
        prefilter_info.used,
        Some(scan_telemetry),
        prefilter_info.reason.as_deref(),
    )
}


/// Maximum ratio of `candidate_files / total_files` at which the literal
/// prefilter is considered worth applying. Above this threshold the trigram
/// intersection cost is comparable to (or worse than) just reading every
/// surviving file, so we fall back to the original full-scan path. 0.5 is
/// an empirically chosen midpoint; refine after AC-4 measurements land.
const LITERAL_PREFILTER_MAX_RATIO: f64 = 0.5;

/// Compute the candidate file set for `handle_line_regex_search` using the
/// required-prefix literals extracted from each compiled regex pattern.
///
/// Returns `(Some(set), info)` when the prefilter narrowed the search and
/// `(None, info)` when callers must fall back to scanning every file.
/// Fallback triggers: the index has no files, every pattern is
/// unprefilterable, an OR-mode batch contains an unprefilterable pattern
/// (because OR with a missing constraint is unconstrained), or the candidate
/// ratio exceeded [`LITERAL_PREFILTER_MAX_RATIO`].
///
/// Correctness: the extractor returns `Kind::Prefix` literals — every regex
/// match starts with one of them. Literals are lowercased and looked up in
/// the (lowercased) trigram index, so the prefilter overestimates candidates
/// (case-folded match) but never underestimates. The per-line regex remains
/// the final arbiter on every surviving file, so false positives are dropped
/// silently and case-sensitive patterns stay correct.
fn compute_literal_prefilter(
    index: &ContentIndex,
    patterns: &[String],
    mode_and: bool,
) -> (Option<HashSet<u32>>, LiteralPrefilterInfo) {
    let total_files = index.files.len();
    let mut info = LiteralPrefilterInfo {
        total_files,
        ..LiteralPrefilterInfo::default()
    };

    if patterns.is_empty() || total_files == 0 {
        info.reason = Some("empty pattern list or empty index".into());
        return (None, info);
    }

    // Phase 1: per-pattern literal extraction. `None` = unprefilterable.
    let per_pattern_literals: Vec<Option<Vec<String>>> = patterns
        .iter()
        .map(|p| {
            grep_literal_extract::extract_required_literals(p)
                .filter(|e| e.usable)
                .map(|e| e.literals)
        })
        .collect();

    let any_unprefilterable = per_pattern_literals.iter().any(|p| p.is_none());
    let all_unprefilterable = per_pattern_literals.iter().all(|p| p.is_none());

    if all_unprefilterable {
        info.reason = Some("no pattern has extractable literals".into());
        return (None, info);
    }
    if !mode_and && any_unprefilterable {
        // OR with any unprefilterable term = unconstrained candidate set.
        info.reason =
            Some("OR mode contains an unprefilterable pattern".into());
        return (None, info);
    }

    // Phase 2: per-pattern candidate file_id sets. Each Kind::Prefix literal
    // is split into word-shaped fragments (alphanumeric/underscore runs of
    // length ≥ MIN_LITERAL_LEN) because the trigram index is built from
    // tokenised content — a literal containing whitespace or punctuation
    // (`"## "`, `"pub fn"`, `"GET /api"`) cannot be looked up directly.
    // Per-literal: INTERSECT fragment file sets (every fragment must be
    // present in any file matching the regex via that alternative). If a
    // literal yields zero usable fragments (e.g. "## " alone), the whole
    // pattern becomes unprefilterable — we cannot rule out files via that
    // alternative, so the OR-with-unprefilterable rule kicks in. Per-pattern:
    // UNION across literal alternatives. Per-batch: AND/OR via `mode_and`.
    let mut combined: Option<HashSet<u32>> = None;
    let mut all_fragments_seen: Vec<String> = Vec::new();

    for pat_literals in &per_pattern_literals {
        let pat_set: Option<HashSet<u32>> = match pat_literals {
            None => None, // unprefilterable from extraction
            Some(literals) => {
                let mut union: HashSet<u32> = HashSet::new();
                let mut all_alternatives_fragmentable = true;
                for lit in literals {
                    let fragments: Vec<&str> = lit
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        // Length in CHARACTERS (Unicode scalars), not bytes.
                        // `generate_trigrams` slides over chars for non-ASCII
                        // input, so a 2-char Cyrillic fragment like "яб"
                        // produces zero trigrams — lookup would silently
                        // return an empty file set (false negative).
                        .filter(|s| s.chars().count() >= grep_literal_extract::MIN_LITERAL_LEN)
                        .collect();
                    if fragments.is_empty() {
                        // This alternative cannot be constrained via the
                        // trigram index; the whole literal-disjunction is
                        // therefore unconstrained.
                        all_alternatives_fragmentable = false;
                        break;
                    }
                    // Per-literal: INTERSECT file sets across fragments
                    // (regex match implies every fragment present).
                    let mut alt_set: Option<HashSet<u32>> = None;
                    for frag in &fragments {
                        all_fragments_seen.push((*frag).to_string());
                        let frag_files = files_containing_substring(
                            frag,
                            &index.trigram,
                            &index.index,
                        );
                        alt_set = Some(match alt_set.take() {
                            None => frag_files,
                            Some(prev) => prev
                                .intersection(&frag_files)
                                .copied()
                                .collect(),
                        });
                    }
                    if let Some(s) = alt_set {
                        union.extend(s);
                    }
                }
                if all_alternatives_fragmentable {
                    Some(union)
                } else {
                    None
                }
            }
        };

        // Re-evaluate the AND/OR composition rules after potentially
        // demoting a pattern to unprefilterable above. OR with any
        // unprefilterable pattern means the candidate set is unconstrained.
        if !mode_and && pat_set.is_none() {
            info.reason = Some(
                "OR mode contains a literal whose word fragments are too short"
                    .into(),
            );
            return (None, info);
        }

        combined = match (combined.take(), pat_set, mode_and) {
            // First constrained pattern seeds the candidate set.
            (None, Some(set), _) => Some(set),
            // Leading unprefilterable patterns: stay unconstrained for now.
            (None, None, _) => None,
            // AND: intersect when both sides are constrained.
            (Some(prev), Some(set), true) => {
                Some(prev.intersection(&set).copied().collect())
            }
            // AND with an unprefilterable later pattern: that pattern adds
            // no constraint, so the existing prev set still over-approximates
            // matching files.
            (Some(prev), None, true) => Some(prev),
            // OR with both constrained: union the file sets.
            (Some(prev), Some(set), false) => {
                Some(prev.union(&set).copied().collect())
            }
            // OR with unprefilterable already bailed at the guard above.
            (Some(_), None, false) => unreachable!(
                "OR with unprefilterable pattern should have bailed earlier"
            ),
        };
    }

    all_fragments_seen.sort();
    all_fragments_seen.dedup();
    info.extracted_fragments = all_fragments_seen;

    let candidate_set = match combined {
        Some(s) => s,
        None => {
            // Reachable in AND mode when every pattern's literal-disjunction
            // turned out to have only unfragmentable alternatives — then
            // every pat_set above was None and `combined` never seeded.
            info.reason =
                Some("all patterns lacked word-shaped literal fragments".into());
            return (None, info);
        }
    };

    let candidate_count = candidate_set.len();
    info.candidate_files = candidate_count;

    // Short-circuit: if the prefilter does not eliminate at least half the
    // index, the per-file regex pre-check is cheaper than maintaining the
    // candidate hashset.
    let ratio = candidate_count as f64 / total_files as f64;
    if ratio > LITERAL_PREFILTER_MAX_RATIO {
        info.short_circuited = true;
        info.reason = Some(format!(
            "candidate set covers {}/{} files (>{:.0}% threshold)",
            candidate_count,
            total_files,
            LITERAL_PREFILTER_MAX_RATIO * 100.0
        ));
        return (None, info);
    }

    info.used = true;
    debug!(
        "[lineRegex-prefilter] extracted {} fragment(s), candidates {}/{} ({:.1}%)",
        info.extracted_fragments.len(),
        candidate_count,
        total_files,
        ratio * 100.0
    );
    (Some(candidate_set), info)
}

fn line_regex_perf_hint(
    search_mode: &str,
    search_elapsed_ms: u64,
    index_files: usize,
    prefilter_used: bool,
    telemetry: Option<&LineRegexScanTelemetry>,
    prefilter_reason: Option<&str>,
) -> Option<String> {
    if !search_mode.starts_with("lineRegex") {
        return None;
    }
    if search_elapsed_ms < LINE_REGEX_SLOW_MS {
        return None;
    }
    if index_files < LINE_REGEX_LARGE_INDEX_FILES {
        return None;
    }
    if let Some(telemetry) = telemetry {
        return Some(line_regex_phase_hint(
            telemetry,
            search_elapsed_ms,
            index_files,
            prefilter_used,
            prefilter_reason,
        ));
    }
    if prefilter_used {
        return Some(format!(
            "lineRegex took {}ms over an index of {} files even with the literal-trigram prefilter applied. \
             Inspect `summary.lineRegexScan` for the measured phase breakdown, then narrow scope with dir=/file=/ext= \
             or simplify the regex. See `xray_help tool=\"xray_grep\"` for full guidance.",
            search_elapsed_ms, index_files
        ));
    }
    Some(format!(
        "lineRegex took {}ms over an index of {} files (literal-trigram prefilter could not narrow the search). \
         Inspect `summary.lineRegexScan` for readMs/wholeFilePrecheckMs/lineEvalMs and related phase timings before choosing a mitigation. \
         If the pattern reduces to a fixed substring, drop lineRegex and use terms=[\"...\"] (~1000x faster). \
         To stay in regex: narrow scope with dir=/file=/ext=, anchor with ^ or $ + a literal prefix (e.g. `^pub fn`), \
         or split into a substring prefilter plus a regex client-side filter. See `xray_help tool=\"xray_grep\"` for full guidance.",
        search_elapsed_ms, index_files
    ))
}

/// Cap on `extracted_fragments` entries in the serialised summary. Keeps
/// `summary.literalPrefilter` compact for clients while debug logs retain
/// the full list. Five is empirically enough to diagnose typical mismatches
/// ("why didn't fragment X reach the trigram lookup?").
const LITERAL_PREFILTER_FRAGMENT_PREVIEW: usize = 5;

/// Inject `summary.literalPrefilter` describing how the AC-4 prefilter
/// behaved for this `lineRegex` request. When the prefilter ran AND
/// `search_elapsed_ms` is still slow, also REPLACES `summary.perfHint`
/// with a prefilter-aware variant (the default hint emitted by
/// [`build_grep_base_summary`] assumes no prefilter ran and tells users
/// to do things the prefilter already did, which would mislead).
fn apply_literal_prefilter_summary(
    summary: &mut Value,
    info: &LiteralPrefilterInfo,
    telemetry: &LineRegexScanTelemetry,
    search_elapsed_ms: u64,
    search_mode: &str,
) {
    let Some(obj) = summary.as_object_mut() else {
        return;
    };

    // Cap fragment list for serialisation; record overflow as a separate
    // counter so clients know when the preview was truncated without us
    // changing the array shape.
    let fragments_total = info.extracted_fragments.len();
    let fragments_preview: Vec<&String> = info
        .extracted_fragments
        .iter()
        .take(LITERAL_PREFILTER_FRAGMENT_PREVIEW)
        .collect();

    let mut prefilter = json!({
        "used": info.used,
        "candidateFiles": info.candidate_files,
        "totalFiles": info.total_files,
        "extractedFragments": fragments_preview,
    });
    if fragments_total > LITERAL_PREFILTER_FRAGMENT_PREVIEW {
        prefilter["extractedFragmentsTruncated"] =
            json!(fragments_total - LITERAL_PREFILTER_FRAGMENT_PREVIEW);
    }
    if info.short_circuited {
        prefilter["shortCircuited"] = json!(true);
    }
    if let Some(ref reason) = info.reason {
        prefilter["reason"] = json!(reason);
    }
    if let Some(t) = info.total_files_after_scope {
        prefilter["totalFilesAfterScope"] = json!(t);
    }
    if let Some(c) = info.candidate_files_after_scope {
        prefilter["candidateFilesAfterScope"] = json!(c);
    }
    obj.insert("literalPrefilter".into(), prefilter);

    // Override the perfHint installed by `build_grep_base_summary` when the
    // prefilter ran — the default copy says "no prefilter" and is
    // wrong here. We only override on slow runs (the helper itself returns
    // None for fast runs, so there's nothing to overwrite). Three perfHint
    // states are distinguished:
    //   1. `info.used == true`            → prefilter-applied copy.
    //   2. `info.used == false` AND we have a `reason`  → attempted-but-
    //      discarded copy that points the user at `summary.literalPrefilter`
    //      so they understand why narrowing failed (short-circuit, OR-bail,
    //      fragments-too-short). Without this override, the default copy
    //      installed by `build_grep_base_summary` would falsely claim the
    //      regex has no extractable required-substring prefix.
    //   3. `info.used == false` AND no `reason`  → by construction the
    //      prefilter wasn't even attempted (non-lineRegex caller); leave
    //      the default copy untouched.
    let slow_enough = search_elapsed_ms >= LINE_REGEX_SLOW_MS
        && info.total_files >= LINE_REGEX_LARGE_INDEX_FILES
        && search_mode.starts_with("lineRegex");
    let perf_hint = if info.used || (slow_enough && info.reason.is_some()) {
        line_regex_perf_hint(
            search_mode,
            search_elapsed_ms,
            info.total_files,
            info.used,
            Some(telemetry),
            info.reason.as_deref(),
        )
    } else {
        None
    };
    if let Some(hint) = perf_hint {
        obj.insert("perfHint".into(), json!(hint));
    }
}

/// Attach the `dirAutoConverted` hint to summary when dir= was a file path.
/// Called from every response builder so the note reaches the LLM regardless of search mode.
fn apply_dir_auto_converted_note(summary: &mut Value, params: &GrepSearchParams) {
    if let Some(ref note) = params.dir_auto_converted_note {
        summary["dirAutoConverted"] = json!(note);
    }
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
    rebuild_trigram_index_singleflight(&ctx.index, &ctx.trigram_build_gate, true);
}

pub(crate) fn schedule_trigram_rebuild_after_edit(ctx: &HandlerContext) {
    let index = Arc::clone(&ctx.index);
    let gate = Arc::clone(&ctx.trigram_build_gate);

    if let Err(error) = std::thread::Builder::new()
        .name("xray-trigram-rebuild".to_string())
        .spawn(move || rebuild_trigram_index_singleflight(&index, &gate, false))
    {
        warn!(error = %error, "[grep-trace] failed to spawn background trigram rebuild");
    }
}

/// Startup trigram warm-up that also clears `trigram_dirty` if needed.
///
/// Plain `ContentIndex::warm_up()` only faults pages in via `black_box`; if
/// the persisted index was saved with `trigram_dirty=true` (a previous
/// session ended with pending edits before a wide grep ran), warm-up leaves
/// the dirty flag set, and the first wide `xray_grep` after startup ends up
/// paying the full ~3 s rebuild + transient peak RSS spike (~+800 MB on
/// Shared). This helper does the rebuild ahead of time, gated by the same
/// single-flight `TrigramRebuildGate` that the edit/grep handlers share, so
/// a watcher edit landing during warm-up cannot race the rebuild.
///
/// Returns `(trigram_count, token_count)` — the same shape as `warm_up`
/// so the caller can preserve the `trigramWarmupReady` log fields.
pub(crate) fn warm_trigram_index(
    index: &Arc<RwLock<ContentIndex>>,
    gate: &Arc<utils::TrigramRebuildGate>,
) -> (usize, usize) {
    // Cheap read-locked dirty check. We don't hold the lock across the
    // rebuild — `rebuild_trigram_index_singleflight` re-checks under the
    // write lock for race correctness.
    let needs_rebuild = match index.read() {
        Ok(idx) => idx.trigram_dirty,
        Err(poisoned) => poisoned.into_inner().trigram_dirty,
    };
    if needs_rebuild {
        rebuild_trigram_index_singleflight(index, gate, false);
    }
    // Always page-in: after a fresh rebuild the new trigram pages are hot,
    // but the surrounding inverted-index keys / postings still benefit from
    // the explicit fault-in pass that `warm_up()` performs.
    let idx = index.read().unwrap_or_else(|e| e.into_inner());
    idx.warm_up()
}

/// Eager-startup variant of [`warm_trigram_index`] that performs phase-1
/// (snapshot tokens + clear dirty) **synchronously on the caller thread**,
/// then spawns a background thread for the offline build + swap + page-in.
///
/// Why split: on a cache-load startup, `cmd_serve` spawns this warm-up
/// alongside `schedule_rebuild_file_tokens`. The latter holds the
/// `ContentIndex` write lock for ~3 s on a 76K-file workspace. If the
/// warm-up's first `index.write()` lands AFTER the file_tokens write lock
/// has latched, the warm-up serializes behind it (~+3 s on
/// `trigramWarmupReady`). Performing phase-1 synchronously on the caller
/// thread before the file_tokens spawn guarantees the trigram snapshot
/// wins the race for the write lock; the long offline build then runs in
/// parallel with the file_tokens rebuild.
///
/// The single-flight gate is held across the whole operation via a
/// `TrigramBuildGuard` carried into the background thread, so concurrent
/// `xray_edit`-driven rebuilds wait via the existing condvar path.
pub(crate) fn start_warm_trigram_index(
    index: &Arc<RwLock<ContentIndex>>,
    gate: &Arc<utils::TrigramRebuildGate>,
) {
    let warmup_start = Instant::now();

    // Try to acquire the single-flight gate. If a rebuild is already in
    // flight (e.g. an `xray_edit` fired between content-publish and here),
    // fall back to page-in only — that other rebuild will populate the
    // trigram for us.
    {
        let mut building = gate.building.lock().unwrap_or_else(|e| e.into_inner());
        if *building {
            drop(building);
            spawn_warmup_page_in_only(Arc::clone(index), warmup_start);
            return;
        }
        *building = true;
    }

    // Phase 1 (sync, on caller thread): under write lock, check the dirty
    // bit and snapshot tokens. Brief — a `Vec::clone` over HashMap keys
    // (~3.8 M entries on Shared, ~50 ms) versus the multi-second
    // file_tokens hold we are racing.
    let initial_snapshot: Option<Vec<String>> = {
        let mut idx = index.write().unwrap_or_else(|e| e.into_inner());
        if idx.trigram_dirty {
            let tokens: Vec<String> = idx.index.keys().cloned().collect();
            idx.trigram_dirty = false;
            Some(tokens)
        } else {
            None
        }
    };

    let Some(initial_tokens) = initial_snapshot else {
        // Trigram already clean — release gate and run page-in only.
        {
            let mut building = gate.building.lock().unwrap_or_else(|e| e.into_inner());
            *building = false;
            gate.done.notify_all();
        }
        spawn_warmup_page_in_only(Arc::clone(index), warmup_start);
        return;
    };

    // Phase 2/3 (background): offline build + swap + re-loop on race + page-in.
    let bg_index = Arc::clone(index);
    let bg_gate = Arc::clone(gate);
    let spawn_result = std::thread::Builder::new()
        .name("xray-trigram-warmup".to_string())
        .spawn(move || {
            crate::index::log_phase("trigramWarmupStarted", &[]);
            let _guard = TrigramBuildGuard { gate: bg_gate };

            let mut tokens = initial_tokens;
            loop {
                let trigram = build_trigram_index_from_tokens(tokens, 0);

                let mut idx = bg_index.write().unwrap_or_else(|e| e.into_inner());
                idx.trigram = trigram;
                let dirty_after_swap = idx.trigram_dirty;
                if dirty_after_swap {
                    tokens = idx.index.keys().cloned().collect();
                    idx.trigram_dirty = false;
                } else {
                    tokens = Vec::new();
                }
                drop(idx);

                if !dirty_after_swap {
                    break;
                }
                info!("[grep-trace] trigram dirtied during warm-up rebuild; rebuilding again");
            }

            // Page-in pass.
            let idx = bg_index.read().unwrap_or_else(|e| e.into_inner());
            let (trigrams, tkns) = idx.warm_up();
            crate::index::log_phase("trigramWarmupReady", &[
                ("trigramWarmupMs", crate::index::format_duration_ms(warmup_start.elapsed())),
                ("trigrams", trigrams.to_string()),
                ("tokens", tkns.to_string()),
            ]);
            crate::index::log_memory("serve: trigram warm-up done");
        });

    // If the bg thread failed to spawn, the closure was dropped: the
    // `TrigramBuildGuard` was never constructed, so the gate would stay
    // latched forever, AND we already cleared `trigram_dirty` above. A
    // later wide grep would then either skip the rebuild (dirty=false →
    // stale trigram) or wait forever on the stuck gate. Restore both
    // pieces of state so the next grep can re-trigger a rebuild via
    // `rebuild_trigram_index_singleflight`.
    if let Err(err) = spawn_result {
        warn!(
            "[grep-trace] failed to spawn trigram warm-up thread; restoring trigram_dirty and releasing gate: {}",
            err
        );
        {
            let mut idx = index.write().unwrap_or_else(|e| e.into_inner());
            idx.trigram_dirty = true;
        }
        let mut building = gate.building.lock().unwrap_or_else(|e| e.into_inner());
        *building = false;
        gate.done.notify_all();
    }
}

fn spawn_warmup_page_in_only(index: Arc<RwLock<ContentIndex>>, warmup_start: Instant) {
    let _ = std::thread::Builder::new()
        .name("xray-trigram-warmup".to_string())
        .spawn(move || {
            crate::index::log_phase("trigramWarmupStarted", &[]);
            let idx = index.read().unwrap_or_else(|e| e.into_inner());
            let (trigrams, tokens) = idx.warm_up();
            crate::index::log_phase("trigramWarmupReady", &[
                ("trigramWarmupMs", crate::index::format_duration_ms(warmup_start.elapsed())),
                ("trigrams", trigrams.to_string()),
                ("tokens", tokens.to_string()),
            ]);
            crate::index::log_memory("serve: trigram warm-up done");
        });
}

fn rebuild_trigram_index_singleflight(
    index: &Arc<RwLock<ContentIndex>>,
    gate: &Arc<utils::TrigramRebuildGate>,
    wait_for_inflight: bool,
) {
    let wait_start = Instant::now();
    let mut building = gate.building.lock().unwrap_or_else(|e| e.into_inner());
    while *building {
        if !wait_for_inflight {
            debug!("[grep-trace] trigram rebuild already in progress; background duplicate skipped");
            return;
        }
        info!("[grep-trace] waiting for in-flight trigram rebuild before wide trigram-dependent search");
        building = gate.done.wait(building).unwrap_or_else(|e| e.into_inner());
    }
    *building = true;
    drop(building);

    let _guard = TrigramBuildGuard { gate: Arc::clone(gate) };

    loop {
        let trigram_check_start = Instant::now();
        let mut idx = index.write().unwrap_or_else(|e| e.into_inner());
        let dirty_check_ms = trigram_check_start.elapsed().as_secs_f64() * 1000.0;
        if !idx.trigram_dirty {
            debug!(
                wait_ms = format_args!("{:.3}", wait_start.elapsed().as_secs_f64() * 1000.0),
                dirty_check_ms = format_args!("{:.3}", dirty_check_ms),
                "[substring-trace] trigram dirty check clean"
            );
            return;
        }

        let tokens: Vec<String> = idx.index.keys().cloned().collect();
        idx.trigram_dirty = false;
        drop(idx);

        let rebuild_start = Instant::now();
        info!(
            dirty_check_ms = format_args!("{:.3}", dirty_check_ms),
            token_count = tokens.len(),
            "[grep-trace] trigram dirty; rebuilding before trigram-dependent search"
        );

        let trigram = build_trigram_index_from_tokens(tokens, 0);
        let token_count = trigram.tokens.len();
        let trigram_count = trigram.trigram_map.len();
        let build_ms = rebuild_start.elapsed().as_secs_f64() * 1000.0;
        let swap_start = Instant::now();

        let mut idx = index.write().unwrap_or_else(|e| e.into_inner());
        idx.trigram = trigram;
        let dirty_after_swap = idx.trigram_dirty;
        drop(idx);

        let swap_ms = swap_start.elapsed().as_secs_f64() * 1000.0;
        let total_ms = rebuild_start.elapsed().as_secs_f64() * 1000.0;
        info!(
            token_count,
            trigram_count,
            build_ms = format_args!("{:.3}", build_ms),
            swap_ms = format_args!("{:.3}", swap_ms),
            total_ms = format_args!("{:.3}", total_ms),
            dirty_after_swap,
            "[grep-trace] trigram rebuild finished"
        );

        if !dirty_after_swap {
            return;
        }
        info!("[grep-trace] trigram dirtied during rebuild; rebuilding again before releasing gate");
    }
}

struct TrigramBuildGuard {
    gate: Arc<utils::TrigramRebuildGate>,
}

impl Drop for TrigramBuildGuard {
    fn drop(&mut self) {
        let mut building = self.gate.building.lock().unwrap_or_else(|e| e.into_inner());
        *building = false;
        self.gate.done.notify_all();
    }
}

/// Check if a file passes all grep filters (dir, ext, excludeDir, exclude).
/// Returns true if the file should be included in results.
fn passes_file_filters(file_path: &str, params: &GrepSearchParams) -> bool {
    // Exact-file mode (set ONLY by the `dir=<file>` auto-convert branch).
    // Supersedes dir/file_filter scoping — the user's intent was unambiguously
    // "this exact file", so we full-path equality-check and short-circuit.
    // Without this, the recursive prefix `dir_filter=<parent>` would still
    // accept `<parent>/sub/Service.cs` and the basename match would let it
    // through (the gap the reviewer flagged).
    if let Some(target) = params.exact_file_path {
        let fp_norm = file_path.to_lowercase().replace('\\', "/");
        let target_norm = target.to_lowercase().replace('\\', "/");
        if fp_norm != target_norm {
            // Preserve logical-path semantics first. Only if the logical paths
            // differ do we attempt the narrow canonical fallback used for
            // Windows 8.3 short/long path aliases. We do NOT canonicalize the
            // requested target unconditionally, because that would resolve
            // symlinked workspace paths to their external targets and break the
            // exact-file contract for logical paths like `root/personal/note.md`.
            let Some(target_canonical) = params.exact_file_path_canonical else {
                return false;
            };
            let Ok(fp_canonical) = std::fs::canonicalize(file_path) else {
                return false;
            };
            let fp_canonical_norm = crate::clean_path(&fp_canonical.to_string_lossy())
                .to_lowercase();
            let target_canonical_norm = target_canonical.to_lowercase().replace('\\', "/");
            if fp_canonical_norm != target_canonical_norm {
                return false;
            }
        }
        // Still apply ext / exclude filters below (they're cheap and harmless
        // for a single file; they also keep behavior consistent if the caller
        // ever adds an explicit `ext` that contradicts the auto-converted path).
    } else {
        // Dir prefix filter (subdirectory search) — only meaningful in scoped mode.
        if let Some(prefix) = params.dir_filter
            && !is_under_dir(file_path, prefix) { return false; }

        // File name/path filter (substring OR over the user-supplied array):
        // each entry in `file_filter` is one substring; the file passes if any
        // entry hits either the full path or the basename (case-insensitive).
        // Empty Vec means "no filter".
        if !params.file_filter.is_empty() {
            let basename_lower = std::path::Path::new(file_path)
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            let fp_lower = file_path.to_lowercase().replace('\\', "/");
            let any_match = params.file_filter.iter()
                .map(|t| t.to_lowercase())
                .any(|needle| {
                    fp_lower.contains(&needle) || basename_lower.contains(&needle)
                });
            if !any_match { return false; }
        }
    }

    // Extension filter (array form). Empty Vec = no filter; otherwise the
    // file's extension must match any entry (case-insensitive). We keep
    // `matches_ext_filter` accepting a comma-joined string for now — file
    // extensions never contain `,`, so the round-trip is lossless.
    if !params.ext_filter.is_empty() {
        let joined = params.ext_filter.join(",");
        if !matches_ext_filter(file_path, &joined) { return false; }
    }

    // Pre-compute lowercased + normalized path once for all exclude checks
    let needs_lower = !params.exclude_patterns.is_empty() || !params.exclude_lower.is_empty();
    let path_lower = if needs_lower {
        file_path.to_lowercase().replace('\\', "/")
    } else {
        String::new()
    };

    // Exclude dir filter — use pre-computed patterns (zero per-file allocations for patterns)
    if !params.exclude_patterns.is_empty()
        && params.exclude_patterns.matches(&path_lower) { return false; }

    // Exclude pattern filter — use pre-lowercased excludes
    if params.exclude_lower.iter().any(|excl| {
        path_lower.contains(excl.as_str())
    }) { return false; }

    true
}

/// Returns true when any scoping filter (`dir`/`file`/`exact_file_path`/`ext`/
/// `excludeDir`/`exclude`) is non-default, i.e. `passes_file_filters` may
/// reject some indexed files. Used by the `lineRegex` flow to decide whether
/// to emit `summary.literalPrefilter.{candidate,total}FilesAfterScope` — the
/// counters are only informative when scope actually narrows the corpus.
fn params_have_scope_filter(params: &GrepSearchParams) -> bool {
    params.dir_filter.is_some()
        || params.exact_file_path.is_some()
        || !params.file_filter.is_empty()
        || !params.ext_filter.is_empty()
        || !params.exclude_patterns.is_empty()
        || !params.exclude_lower.is_empty()
}

/// Parsed arguments for the grep handler. Extracts all parameter parsing
/// from the main handler to reduce its cognitive complexity.
#[derive(Debug)]
struct ParsedGrepArgs {
    /// Search terms, post-validation. Each entry is one term taken verbatim
    /// from the `terms` array (trimmed; empty entries dropped). Empty Vec
    /// means no terms supplied — only valid when `lineRegex=true` (which
    /// drives off this same array).
    terms: Vec<String>,
    dir_filter: Option<String>,
    /// File extension filter. Empty = no filter; otherwise each entry is one
    /// extension (no leading dot).
    ext_filter: Vec<String>,
    /// File path/basename substring filter (case-insensitive OR). Empty = no
    /// filter. Each entry is one substring; literal `,` inside an entry is
    /// preserved verbatim.
    file_filter: Vec<String>,
    /// When `Some(path)`, the request was `dir=<file>` (auto-converted) and
    /// only that exact file (full normalized LOGICAL path) should match.
    /// Closes the nested-basename leak (`<parent>/sub/Service.cs` was previously
    /// accepted) without losing symlink-path semantics.
    exact_file_path: Option<String>,
    /// Narrow fallback for Windows short/long path aliases when the canonical
    /// file still lives inside the workspace. Not used for symlink targets
    /// outside the workspace.
    exact_file_path_canonical: Option<String>,
    mode_and: bool,
    use_regex: bool,
    use_phrase: bool,
    use_substring: bool,
    /// Line-based regex mode. When true, applies the user-supplied regex
    /// pattern to each line of candidate files (slower than token-based
    /// regex, but supports line anchors `^`/`$`, whitespace, and arbitrary
    /// non-token characters). Mutually exclusive with `phrase`. Implies
    /// `regex=true` (validated in `parse_grep_args`). Auto-disables `substring`.
    use_line_regex: bool,
    context_lines: usize,
    show_lines: bool,
    max_results: usize,
    count_only: bool,
    exclude_dir: Vec<String>,
    exclude: Vec<String>,
    /// Set when user passed a file path in `dir=` — we auto-convert it to
    /// parent dir + file filter and surface this note in the response summary.
    dir_auto_converted_note: Option<String>,
    /// Auto-balance multi-term substring-OR results so a dominant common
    /// term cannot starve rare-term matches. Default `true`.
    auto_balance: bool,
    /// Optional explicit per-term file cap for the dominant-only group when
    /// auto-balance triggers. `None` = derived from `2 * second_max`.
    max_occurrences_per_term: Option<usize>,
}

/// Parse and validate all grep parameters from JSON args.
/// Returns `Ok(ParsedGrepArgs)` on success, `Err(ToolCallResult)` on validation error.
fn parse_grep_args(args: &Value, server_dir: &str) -> Result<ParsedGrepArgs, ToolCallResult> {
    // GREP-015: reject empty/whitespace-only `terms` here instead of letting
    // it propagate into per-mode handlers, where it produces inconsistent
    // failure modes (regex compiles `""` into a match-everything pattern,
    // substring path returns silently empty results, etc.). LLM clients
    // interpret "empty result" as "this code does not exist" — a misleading
    // signal for what is really a malformed query.
    //
    // `terms` is required unless `lineRegex=true` is also set; the line-regex
    // mode can drive entirely off non-token patterns where any element of
    // the array is a valid regex (and an empty array is rejected below in the
    // `lineRegex` branch).
    let terms: Vec<String> = match utils::read_string_array(args, "terms") {
        Ok(v) => v,
        Err(e) => return Err(ToolCallResult::error(e)),
    };
    let use_line_regex_peek = args.get("lineRegex").and_then(|v| v.as_bool()).unwrap_or(false);
    if terms.is_empty() && !use_line_regex_peek {
        return Err(ToolCallResult::error(
            "Parameter 'terms' must contain at least one entry. Pass [\"a\",\"b\"] for multi-term search.".to_string(),
        ));
    }

    // Explicit `file` filter (user-provided). Takes precedence over dir-autoconvert filename.
    let file_filter: Vec<String> = match utils::read_string_array(args, "file") {
        Ok(v) => v,
        Err(e) => return Err(ToolCallResult::error(e)),
    };
    // Set ONLY when the `dir=<file>` auto-convert path fires. Carries the FULL
    // resolved path of the targeted file so `passes_file_filters` can do exact
    // path equality (basename-only would let `<parent>/sub/<name>` leak).
    let mut exact_file_path: Option<String> = None;
    let mut exact_file_path_canonical: Option<String> = None;

    let mut dir_auto_converted_note: Option<String> = None;

    let dir_filter: Option<String> = if let Some(dir) = args.get("dir").and_then(|v| v.as_str()) {
        match validate_search_dir(dir, server_dir) {
            Ok(filter) => {
                // Detect file paths passed as dir= and auto-convert to parent-dir + exact-file scope.
                // Historically this returned an error; now we accept it, surface a note in summary,
                // and teach the LLM the correct pattern for next time.
                if let Some(ref resolved) = filter {
                    let path = std::path::Path::new(resolved);
                    if path.is_file() || super::utils::looks_like_file_path(resolved) {
                        let parent = path.parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| server_dir.to_string());
                        let filename = path.file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_default();
                        // Pin to the exact resolved path so siblings AND nested
                        // duplicates of the same basename (`<parent>/sub/<name>`)
                        // can't sneak in via the recursive prefix `dir_filter`.
                        // Explicit user `file=` (substring-OR) is left untouched —
                        // we only set `exact_file_path` when explicitly auto-converted.
                        if !filename.is_empty() {
                            // Keep the LOGICAL path as the primary exact-file
                            // filter so symlinked workspace paths continue to
                            // match the logical paths recorded by the indexer.
                            exact_file_path = Some(resolved.clone());

                            // Narrow fallback for Windows 8.3 short/long path
                            // aliases: if the canonicalized file still lives
                            // inside the workspace boundary, keep that form too
                            // so `passes_file_filters` can recover when the
                            // logical path differs only in root representation
                            // (`RUNNER~1` vs `runneradmin`). We intentionally do
                            // NOT keep canonical paths that point outside the
                            // workspace (symlink/junction targets), because that
                            // would break the logical-path contract.
                            if let Ok(canonical) = std::fs::canonicalize(path) {
                                let canonical = crate::clean_path(&canonical.to_string_lossy());
                                if canonical != resolved.as_str()
                                    && code_xray::is_path_within(&canonical, server_dir)
                                {
                                    exact_file_path_canonical = Some(canonical);
                                }
                            }
                        }
                        dir_auto_converted_note = Some(format!(
                            "dir='{}' looked like a file path — auto-converted to scope=exactly that one file ({}). \
                             To search the WHOLE folder instead, pass dir='{}'. \
                             Note: explicit file='<substring>' uses substring + comma-OR semantics — \
                             it would also match siblings like 'My{}', 'Old{}'.",
                            dir, resolved, parent, filename, filename
                        ));
                        // Re-validate the parent dir against server_dir scope (kept as a
                        // cheap pre-filter; the exact-path check is the real gate).
                        validate_search_dir(&parent, server_dir).unwrap_or_default()
                    } else {
                        filter
                    }
                } else {
                    filter
                }
            },
            Err(msg) => return Err(ToolCallResult::error(msg)),
        }
    } else {
        None
    };

    let ext_filter: Vec<String> = match utils::read_string_array(args, "ext") {
        Ok(v) => v,
        Err(e) => return Err(ToolCallResult::error(e)),
    };
    let mode_and = match read_enum_string_with_default(args, "mode", ALL_GREP_MODES, "or") {
        Ok(m) => m == "and",
        Err(e) => return Err(ToolCallResult::error(e)),
    };
    let mut use_regex = args.get("regex").and_then(|v| v.as_bool()).unwrap_or(false);
    let use_phrase = args.get("phrase").and_then(|v| v.as_bool()).unwrap_or(false);
    let use_line_regex = args.get("lineRegex").and_then(|v| v.as_bool()).unwrap_or(false);

    // Validate lineRegex compatibility:
    // - lineRegex implies regex=true (auto-promoted with a note: explicit error
    //   would be hostile to LLMs that forget the implication).
    // - lineRegex is mutually exclusive with phrase=true (different semantics).
    if use_line_regex {
        if use_phrase {
            return Err(ToolCallResult::error(
                "lineRegex is mutually exclusive with phrase. Use one or the other.".to_string(),
            ));
        }
        // Auto-enable regex when lineRegex is requested.
        use_regex = true;
    }

    // Default to substring=true so compound C# identifiers are always found.
    // Auto-disable when regex/phrase/lineRegex is used.
    let use_substring = if use_regex || use_phrase || use_line_regex {
        if args.get("substring").and_then(|v| v.as_bool()) == Some(true) {
            return Err(ToolCallResult::error(
                "substring is mutually exclusive with regex, phrase, and lineRegex".to_string(),
            ));
        }
        false
    } else {
        args.get("substring").and_then(|v| v.as_bool()).unwrap_or(true)
    };

    // GREP-007: bound user-supplied integers instead of `as usize` truncation.
    // Without these caps a hostile/buggy client can request `maxResults=10_000_000`
    // (response builder OOMs while serializing JSON) or `contextLines=1_000_000`
    // (every matched file's IO + memory blows up by 1 MLOC).
    fn parse_bounded_usize(args: &Value, key: &str, default: usize, max: usize) -> Result<usize, String> {
        match args.get(key).and_then(|v| v.as_u64()) {
            Some(v) => {
                let v_usize = usize::try_from(v)
                    .map_err(|_| format!("{key} must be 0..={} (got {v})", max))?;
                if v_usize > max {
                    return Err(format!("{key} must be 0..={} (got {v})", max));
                }
                Ok(v_usize)
            }
            None => Ok(default),
        }
    }
    let context_lines = match parse_bounded_usize(args, "contextLines", 0, 50) {
        Ok(n) => n,
        Err(e) => return Err(ToolCallResult::error(e)),
    };
    // Auto-enable showLines when contextLines > 0
    let show_lines = args.get("showLines").and_then(|v| v.as_bool()).unwrap_or(false)
        || context_lines > 0;
    let max_results = match parse_bounded_usize(args, "maxResults", 50, 10_000) {
        Ok(n) => n,
        Err(e) => return Err(ToolCallResult::error(e)),
    };
    let count_only = args.get("countOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    let exclude_dir: Vec<String> = args.get("excludeDir")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let exclude: Vec<String> = args.get("exclude")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    // Auto-balance is opt-out: defaults to true. Only fires for multi-term
    // substring-OR queries (see apply_auto_balance for full preconditions).
    let auto_balance = args.get("autoBalance").and_then(|v| v.as_bool()).unwrap_or(true);
    // When provided, overrides the derived `2 * second_max` cap. Bounded to
    // avoid OOM via the same pattern as maxResults / contextLines.
    let max_occurrences_per_term = match parse_bounded_usize(args, "maxOccurrencesPerTerm", 0, 10_000) {
        Ok(0) => None,
        Ok(n) => Some(n),
        Err(e) => return Err(ToolCallResult::error(e)),
    };

    Ok(ParsedGrepArgs {
        terms,
        dir_filter,
        ext_filter,
        file_filter,
        exact_file_path,
        exact_file_path_canonical,
        mode_and,
        use_regex,
        use_phrase,
        use_substring,
        use_line_regex,
        context_lines,
        show_lines,
        max_results,
        count_only,
        exclude_dir,
        exclude,
        dir_auto_converted_note,
        auto_balance,
        max_occurrences_per_term,
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
    // GREP-014: dedupe expanded terms across patterns. Two patterns that
    // overlap on the same token (e.g. `User.*,.*Service` both hit
    // `UserService`) would otherwise have that token contribute its
    // TF-IDF score twice in `score_normal_token_search`, silently skewing
    // file ranking toward documents that match multiple input patterns.
    expanded.sort();
    expanded.dedup();
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
            if doc_freq == 0.0 { continue; }
            let idf = (total_docs / doc_freq).ln();

            for posting in postings {
                let file_path = match index.files.get(posting.file_id as usize) {
                    Some(p) => p,
                    None => continue,
                };

                if !passes_file_filters(file_path, params) { continue; }

                let occurrences = posting.lines.len();
                let file_total = if (posting.file_id as usize) < index.file_token_counts.len() {
                    let count = index.file_token_counts[posting.file_id as usize] as f64;
                    if count == 0.0 { 1.0 } else { count }
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
                    per_term_occurrences: Vec::new(),
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
#[allow(clippy::too_many_arguments)]
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
        let terms_value = json!(terms);
        let mut summary = build_grep_base_summary(
            total_files, total_occurrences, &terms_value, search_mode,
            index, search_elapsed, ctx, true, params.lock_wait_ms,
        );
        apply_dir_auto_converted_note(&mut summary, params);
        let result_status = build_grep_result_status(0, 0, total_files, total_occurrences, true);
        let execution = build_grep_execution(params, search_mode, None, false, Some(total_files), None);
        let output = finalize_grep_output(
            json!({ "summary": summary }),
            result_status,
            execution,
            &terms_value,
            params,
        );
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

    let terms_value = json!(terms);
    let mut summary = build_grep_base_summary(
        total_files, total_occurrences, &terms_value, search_mode,
        index, search_elapsed, ctx, true, params.lock_wait_ms,
    );
    apply_dir_auto_converted_note(&mut summary, params);

    // XML structural context hint: if results contain XML files, suggest containsLine
    #[cfg(feature = "lang-xml")]
    {
        use crate::definitions::parser_xml::is_xml_extension;
        let has_xml = results.iter().any(|r| {
            std::path::Path::new(&r.file_path)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(is_xml_extension)
        });
        if has_xml {
            summary["xmlHint"] = json!(
                "XML matches found. Use xray_definitions file='<path>' containsLine=<N> includeBody=true for structural context (returns enclosing XML block with siblings)."
            );
        }
    }

    let returned_occurrences = results.iter().map(|result| result.occurrences).sum();
    let result_status = build_grep_result_status(
        results.len(),
        returned_occurrences,
        total_files,
        total_occurrences,
        false,
    );
    let execution = build_grep_execution(params, search_mode, None, false, Some(total_files), None);
    let output = finalize_grep_output(
        json!({
            "files": files_json,
            "summary": summary
        }),
        result_status,
        execution,
        &terms_value,
        params,
    );

    ToolCallResult::success(json_to_string(&output))
}

/// When grep returns 0 results and `ext` filter targets a non-indexed extension,
/// inject a hint explaining why no results were found.
/// Only fires when ext filter is explicitly set — avoids noise on generic searches.
fn inject_grep_ext_hint(
    result: &mut ToolCallResult,
    ext_filter: &[String],
    ctx: &HandlerContext,
) {
    // Only hint when ext filter is explicitly set
    if ext_filter.is_empty() {
        return;
    }

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
    let non_indexed: Vec<&str> = ext_filter.iter()
        .map(|s| s.as_str())
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
    let parsed = match parse_grep_args(args, &ctx.server_dir()) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let search_start = Instant::now();

    // Log entry for dispatch-level timing correlation
    info!(mode = if parsed.use_phrase { "phrase" } else if parsed.use_substring { "substring" }
        else if parsed.use_line_regex { "lineRegex" } else { "token" },
        terms_count = parsed.terms.len(),
        "[grep-trace] handle_xray_grep entered");

    // Skip trigram rebuild when ALL terms contain non-token chars (spaces,
    // punctuation) — substring search will return 0 results and auto-switch
    // to phrase, which doesn't use trigrams. Avoids a ~40s full trigram
    // rebuild on large repos after xray_edit sets trigram_dirty=true.
    let will_auto_switch_to_phrase = parsed.use_substring
        && parsed.terms.iter().all(|t| has_non_token_chars(t));
    // Also skip trigram rebuild when scope is already narrow (file= or
    // exact_file_path set) — the literal prefilter would just confirm what
    // the file filter already guarantees. Rebuilding 3.8M-token trigrams
    // for a 2-file scope is pure waste (~40-70s on large repos).
    let scope_already_narrow = !parsed.file_filter.is_empty()
        || parsed.exact_file_path.is_some();
    let trigram_skipped = will_auto_switch_to_phrase || scope_already_narrow;
    if (parsed.use_substring || parsed.use_line_regex)
        && !will_auto_switch_to_phrase
        && !scope_already_narrow
    {
        // lineRegex shares the substring branch's trigram dependency: AC-4
        // adds a literal-trigram prefilter (`compute_literal_prefilter`) that
        // reads `index.trigram` while the caller already holds a read lock
        // below — so the trigram index must be made clean *before* that read
        // lock is acquired (rebuilding from inside the read lock would need a
        // write lock and deadlock).
        ensure_trigram_index(ctx);
    }

    let lock_wait_start = Instant::now();
    let index = match ctx.index.read() {
        Ok(idx) => idx,
        Err(e) => return ToolCallResult::error(format!("Failed to acquire index lock: {}", e)),
    };
    let lock_wait_ms = lock_wait_start.elapsed().as_secs_f64() * 1000.0;
    if lock_wait_ms > 100.0 {
        info!(lock_wait_ms = format_args!("{:.1}", lock_wait_ms),
            "[grep-trace] slow read-lock acquisition on content index");
    }
    let trigram_rebuild_in_flight = ctx.trigram_build_gate.is_building();
    let trigram_stale = trigram_skipped && (index.trigram_dirty || trigram_rebuild_in_flight);
    let stale_safe_search_enabled = trigram_stale && !will_auto_switch_to_phrase;
    if stale_safe_search_enabled {
        info!(
            scope_already_narrow,
            file_filter_count = parsed.file_filter.len(),
            exact_file_path = parsed.exact_file_path.is_some(),
            "[grep-trace] trigram rebuild skipped; stale-safe search path enabled"
        );
    }

    let exclude_patterns = super::utils::ExcludePatterns::from_dirs(&parsed.exclude_dir);
    let exclude_lower: Vec<String> = parsed.exclude.iter()
        .map(|s| s.to_lowercase())
        .collect();
    let requested_mode = grep_requested_mode(&parsed);

    let grep_params = GrepSearchParams {
        ext_filter: &parsed.ext_filter,
        show_lines: parsed.show_lines,
        context_lines: parsed.context_lines,
        max_results: parsed.max_results,
        mode_and: parsed.mode_and,
        count_only: parsed.count_only,
        search_start,
        dir_filter: &parsed.dir_filter,
        file_filter: &parsed.file_filter,
        exact_file_path: &parsed.exact_file_path,
        exact_file_path_canonical: &parsed.exact_file_path_canonical,
        exclude_patterns,
        exclude_lower,
        dir_auto_converted_note: parsed.dir_auto_converted_note.clone(),
        auto_balance: parsed.auto_balance,
        max_occurrences_per_term: parsed.max_occurrences_per_term,
        lock_wait_ms,
        trigram_stale,
        requested_mode,
    };


    // --- Substring search mode
    if parsed.use_substring {
        let mut result = handle_substring_search(ctx, &index, &parsed.terms, &grep_params);
        inject_grep_ext_hint(&mut result, &parsed.ext_filter, ctx);
        return result;
    }

    // --- Phrase search mode
    if parsed.use_phrase {
        // Each `terms` entry is one phrase, taken verbatim (no comma-split).
        let phrases: Vec<String> = parsed.terms.clone();
        if phrases.is_empty() {
            return ToolCallResult::error("No search terms provided".to_string());
        }
        let mut result = handle_multi_phrase_search(ctx, &index, &phrases, &grep_params);
        inject_grep_ext_hint(&mut result, &parsed.ext_filter, ctx);
        return result;
    }

    // --- Line-based regex mode (supports `^`, `$`, whitespace, non-token chars)
    if parsed.use_line_regex {
        // Each `terms` entry is one regex pattern, taken verbatim. Literal
        // `,` inside an entry is preserved (e.g. CSV-shape regex
        // `^[^,]+,[^,]+$`, log prefix `^ERROR,WARN:`).
        let patterns: Vec<String> = parsed.terms.clone();
        let mut result = handle_line_regex_search(ctx, &index, patterns, &grep_params);
        inject_grep_ext_hint(&mut result, &parsed.ext_filter, ctx);
        return result;
    }

    // --- Normal token search
    // Each `terms` entry is one search term, taken verbatim from the array
    // (already trimmed, empty entries dropped). Lowercase here for the
    // tokenizer; user-supplied case is preserved upstream for phrase mode.
    let raw_terms: Vec<String> = parsed.terms.iter()
        .map(|s| s.to_lowercase())
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

    // Warn when regex=true and pattern uses constructs incompatible with token-based regex.
    // Token regex matches against individual index tokens (alphanumeric+underscore, no whitespace),
    // so anchors `^`/`$` (anchored to token boundaries, not lines) and spaces never match.
    let pattern_has_spaces = parsed.terms.iter().any(|t| t.contains(' '));
    let pattern_has_anchors = parsed.terms.iter().any(|t| t.contains('^') || t.contains('$'));
    if parsed.use_regex && (pattern_has_spaces || pattern_has_anchors)
        && let Some(text) = result.content.first_mut().map(|c| &mut c.text)
            && let Ok(mut output) = serde_json::from_str::<serde_json::Value>(text) {
                if let Some(summary) = output.get_mut("summary") {
                    let reason = match (pattern_has_spaces, pattern_has_anchors) {
                        (true, true) => "Pattern contains spaces and line anchors (`^`/`$`)",
                        (true, false) => "Pattern contains spaces",
                        (false, true) => "Pattern contains line anchors (`^`/`$`)",
                        _ => unreachable!(),
                    };
                    summary["searchModeNote"] = serde_json::Value::String(format!(
                        "{} — token-based regex cannot match these (operates on alphanumeric+underscore tokens, not whole lines). \
                         For line-based regex with anchor/whitespace support, set lineRegex=true (slower but accurate). \
                         For multi-word substring search without regex, use phrase=true. \
                         Example: terms='^## ' regex=true lineRegex=true file='X.md' — finds markdown headings.",
                        reason
                    ));
                }
                *text = json_to_string(&output);
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
    terms: &[String],
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
    // Each `terms` entry is one phrase, taken verbatim (literal-comma-safe).
    let phrases: Vec<String> = terms.to_vec();
    let mut result = handle_multi_phrase_search(ctx, index, &phrases, params);
    // Inject a note explaining the auto-switch
    if let Some(text) = result.content.first_mut().map(|c| &mut c.text)
        && let Ok(mut output) = serde_json::from_str::<serde_json::Value>(text) {
            if let Some(summary) = output.get_mut("summary") {
                let note = if has_punctuation {
                    // Surface the actual offending characters from raw_terms so
                    // the hint is context-specific instead of always pointing at
                    // .NET-namespace examples. Alphanumerics and underscores are
                    // kept by tokenize(); spaces are reported via the has_spaces
                    // channel, so we exclude both here. BTreeSet gives stable,
                    // deterministic ordering for tests and human readers.
                    let offenders: std::collections::BTreeSet<char> = raw_terms
                        .iter()
                        .flat_map(|t| t.chars())
                        .filter(|c| !c.is_alphanumeric() && *c != '_' && *c != ' ')
                        .collect();
                    let chars: String = offenders.iter().collect();
                    format!(
                        "{} ({}) — auto-switched to phrase search (~100x slower). \
                         Tip: drop the punctuation for a fast substring match, \
                         or pass lineRegex=true to match the literal pattern on raw lines.",
                        reason, chars
                    )
                } else {
                    format!("{} — auto-switched to phrase search \
                     (substring mode operates on individual tokens which only contain \
                     alphanumeric characters and underscores)", reason)
                };
                append_search_mode_note(summary, note);
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
#[allow(clippy::too_many_arguments)]
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
                    let count = index.file_token_counts[posting.file_id as usize] as f64;
                    if count == 0.0 { 1.0 } else { count }
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
                    per_term_occurrences: Vec::new(),
                });
                entry.tf_idf += tf_idf;
                entry.occurrences += occurrences;
                entry.lines.extend_from_slice(&posting.lines);
                if entry.per_term_occurrences.len() <= term_idx {
                    entry.per_term_occurrences.resize(term_idx + 1, 0);
                }
                entry.per_term_occurrences[term_idx] += occurrences;
                file_matched_terms.entry(posting.file_id).or_default().insert(term_idx);
            }
        }
    }

    debug!("[substring-trace] Main index lookup: {} tokens, {} postings checked, {} files passed in {:.3}ms",
        matched_tokens.len(), term_postings_checked, term_files_passed,
        lookup_start.elapsed().as_secs_f64() * 1000.0);
}

/// Build the final substring search response (JSON with files, summary, warnings, matchedTokens).
#[allow(clippy::too_many_arguments)]
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
    auto_balance_info: Option<&AutoBalanceInfo>,
) -> ToolCallResult {
    let search_start = params.search_start;
    let search_mode_label = format!("substring-{}", search_mode);
    let terms_value = json!(raw_terms);

    if params.count_only {
        let mut summary = build_grep_base_summary(
            total_files, total_occurrences, &terms_value,
            &search_mode_label, index, search_start.elapsed(), ctx, false, params.lock_wait_ms,
        );
        // Don't include matchedTokens in countOnly mode — the caller only needs
        // totalFiles/totalOccurrences. Including tokens wastes bytes and can trigger
        // false truncation ("capped matchedTokens to 20") that confuses LLMs.
        if !warnings.is_empty() {
            summary["warnings"] = json!(warnings);
        }
        if let Some(ab) = auto_balance_info {
            inject_auto_balance(&mut summary, ab);
        }
        apply_dir_auto_converted_note(&mut summary, params);
        let result_status = build_grep_result_status(0, 0, total_files, total_occurrences, true);
        let execution = build_grep_execution(params, &search_mode_label, None, false, Some(total_files), None);
        let output = finalize_grep_output(
            json!({ "summary": summary }),
            result_status,
            execution,
            &terms_value,
            params,
        );
        debug!("[substring-trace] Total: {:.3}ms (count_only)", search_start.elapsed().as_secs_f64() * 1000.0);
        return ToolCallResult::success(json_to_string(&output));
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
        total_files, total_occurrences, &terms_value,
        &search_mode_label, index, search_start.elapsed(), ctx, false, params.lock_wait_ms,
    );
    summary["matchedTokens"] = json!(all_matched_tokens);
    if !warnings.is_empty() {
        summary["warnings"] = json!(warnings);
    }
    if let Some(ab) = auto_balance_info {
        inject_auto_balance(&mut summary, ab);
    }
    apply_dir_auto_converted_note(&mut summary, params);
    let returned_occurrences = results.iter().map(|result| result.occurrences).sum();
    let result_status = build_grep_result_status(
        results.len(),
        returned_occurrences,
        total_files,
        total_occurrences,
        false,
    );
    let execution = build_grep_execution(params, &search_mode_label, None, false, Some(total_files), None);
    let output = finalize_grep_output(
        json!({
            "files": files_json,
            "summary": summary
        }),
        result_status,
        execution,
        &terms_value,
        params,
    );
    debug!("[substring-trace] Response JSON: {:.3}ms", json_start.elapsed().as_secs_f64() * 1000.0);
    debug!("[substring-trace] Total: {:.3}ms ({} files, {} tokens matched)",
        search_start.elapsed().as_secs_f64() * 1000.0, total_files, all_matched_tokens.len());

    ToolCallResult::success(json_to_string(&output))
}

/// Substring search using the trigram index.
fn handle_substring_search(
    ctx: &HandlerContext,
    index: &ContentIndex,
    terms: &[String],
    params: &GrepSearchParams,
) -> ToolCallResult {
    // Stage 1: Terms parsing — lowercase the user-supplied entries (already
    // trimmed and de-empty'd by `read_string_array`). Substring search runs
    // against the trigram index whose tokens are lowercased, so we mirror.
    let stage1 = Instant::now();
    let raw_terms: Vec<String> = terms.iter()
        .map(|s| s.to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    debug!("[substring-trace] Terms parsed: {:?} in {:.3}ms", raw_terms, stage1.elapsed().as_secs_f64() * 1000.0);

    if raw_terms.is_empty() {
        return ToolCallResult::error("No search terms provided".to_string());
    }

    // Auto-switch to phrase mode when terms contain spaces or non-token characters
    if let Some(result) = auto_switch_to_phrase_if_needed(ctx, index, terms, &raw_terms, params) {
        return result;
    }

    let trigram_idx = &index.trigram;
    let total_docs = index.files.len() as f64;
    let search_mode = if params.mode_and { "and" } else { "or" };
    if params.trigram_stale {
        debug!(
            terms_count = raw_terms.len(),
            indexed_tokens = index.index.len(),
            "[substring-trace] using direct token scan because trigram index is stale"
        );
    }

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
        // When trigram is stale (rebuild skipped for narrow scope), fall back
        // to brute-force token matching to avoid false negatives from missing
        // trigram entries for newly-added tokens.
        let matched_tokens: Vec<String> = if params.trigram_stale {
            index.index.keys()
                .filter(|k| k.contains(term.as_str()))
                .cloned()
                .collect()
        } else {
            let matched_token_indices = find_matching_tokens_for_term(term, trigram_idx);
            matched_token_indices.iter()
                .filter_map(|&idx| trigram_idx.tokens.get(idx as usize).cloned())
                .collect()
        };

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

    // Trim a single dominant common term so it cannot starve rare-term matches
    // off the response. Only fires for multi-term substring-OR; AND mode is
    // skipped because it already requires every term to match per file. Runs
    // BEFORE max_results truncation so the cap operates on the full result
    // set, not the already-truncated head.
    let auto_balance_info = if params.auto_balance && !params.mode_and && term_count >= 2 {
        apply_auto_balance(&mut results, term_count, &raw_terms, params.max_occurrences_per_term)
    } else {
        None
    };

    if params.max_results > 0 {
        results.truncate(params.max_results);
    }

    build_substring_response(
        &results, &raw_terms, &all_matched_tokens, &warnings,
        total_files, total_occurrences, search_mode, index, ctx, params,
        auto_balance_info.as_ref(),
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

    // C1 refactor: Delegate tokenization, candidate search, and phrase verification
    // to collect_phrase_matches() — eliminating ~85 lines of duplicated logic.
    let (mut results, diag) = match collect_phrase_matches(index, phrase, params) {
        Ok(r) => r,
        Err(e) => return ToolCallResult::error(e),
    };

    let total_files = results.len();
    let total_occurrences: usize = results.iter().map(|r| r.lines.len()).sum();

    // Sort by number of occurrences descending (most matches first).
    // Tie-break by file path ascending so that, when occurrences are equal,
    // the truncated `max_results` slice is deterministic across runs (the
    // tier-A parallel candidate verification means worker-thread completion
    // order is non-deterministic, so without a secondary key the tail of the
    // result set could shuffle between identical queries).
    results.sort_by(|a, b| {
        b.lines.len()
            .cmp(&a.lines.len())
            .then_with(|| a.file_path.cmp(&b.file_path))
    });

    if max_results > 0 {
        results.truncate(max_results);
    }

    let search_elapsed = search_start.elapsed();
    let search_mode = "phrase";
    let terms_value = json!([phrase]);

    if count_only {
        let mut summary = build_grep_base_summary(
            total_files, total_occurrences, &terms_value, search_mode,
            index, search_elapsed, ctx, true, params.lock_wait_ms,
        );
        apply_dir_auto_converted_note(&mut summary, params);
        summary["phraseDetail"] = diag.to_json();
        let result_status = build_grep_result_status(0, 0, total_files, total_occurrences, true);
        let execution = build_grep_execution(params, search_mode, None, false, Some(total_files), None);
        inject_phrase_diagnostic_note(&mut summary, &diag);
        let output = finalize_grep_output(
            json!({ "summary": summary }),
            result_status,
            execution,
            &terms_value,
            params,
        );
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

    let mut summary = build_grep_base_summary(
        total_files, total_occurrences, &terms_value, search_mode,
        index, search_elapsed, ctx, true, params.lock_wait_ms,
    );
    apply_dir_auto_converted_note(&mut summary, params);
    summary["phraseDetail"] = diag.to_json();
    let returned_occurrences = results.iter().map(|result| result.lines.len()).sum();
    let result_status = build_grep_result_status(
        results.len(),
        returned_occurrences,
        total_files,
        total_occurrences,
        false,
    );
    let execution = build_grep_execution(params, search_mode, None, false, Some(total_files), None);
    inject_phrase_diagnostic_note(&mut summary, &diag);
    let output = finalize_grep_output(
        json!({
            "files": files_json,
            "summary": summary
        }),
        result_status,
        execution,
        &terms_value,
        params,
    );

    ToolCallResult::success(json_to_string(&output))
}

/// Diagnostic counters for phrase search sub-timings.
const PHRASE_WARNINGS_MAX: usize = 20;
const PHRASE_WARNING_MISSING_TOKENS_MAX: usize = 3;

#[derive(Debug, Default, Clone)]
pub(crate) struct PhraseSearchDiag {
    pub token_count: usize,
    pub per_token: Vec<(String, usize, usize, f64)>,
    pub missing_tokens: Vec<String>,
    pub posting_scan_ms: f64,
    pub intersection_ms: f64,
    pub candidates_after_intersection: usize,
    pub file_verify_ms: f64,
    pub files_read: usize,
    pub result_count: usize,
}

impl PhraseSearchDiag {
    fn to_json(&self) -> Value {
        let mut value = json!({
            "tokenCount": self.token_count,
            "postingScanMs": format!("{:.1}", self.posting_scan_ms),
            "intersectionMs": format!("{:.1}", self.intersection_ms),
            "candidatesAfterIntersection": self.candidates_after_intersection,
            "fileVerifyMs": format!("{:.1}", self.file_verify_ms),
            "filesRead": self.files_read,
            "perToken": self.per_token.iter().map(|(t, postings, passed, ms)| json!({
                "token": t, "postings": postings, "passed": passed, "ms": format!("{:.1}", ms)
            })).collect::<Vec<_>>(),
        });
        if !self.missing_tokens.is_empty() {
            value["missingTokens"] = json!(self.missing_tokens);
        }
        value
    }

    fn has_missing_tokens(&self) -> bool {
        !self.missing_tokens.is_empty()
    }

    fn missing_tokens_note(&self) -> Option<String> {
        if self.missing_tokens.is_empty() {
            return None;
        }
        let mut preview = self.missing_tokens.iter()
            .take(PHRASE_WARNING_MISSING_TOKENS_MAX)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        if self.missing_tokens.len() > PHRASE_WARNING_MISSING_TOKENS_MAX {
            preview.push_str(&format!(
                " ... (+{} more)",
                self.missing_tokens.len() - PHRASE_WARNING_MISSING_TOKENS_MAX,
            ));
        }
        Some(format!(
            "Phrase search stopped before reading files because {} token(s) had no postings in the index: {}. Try substring=true for token-substring search, or lineRegex=true for raw literal matching.",
            self.missing_tokens.len(), preview
        ))
    }
}

fn append_search_mode_note(summary: &mut Value, note: String) {
    let next = match summary.get("searchModeNote").and_then(Value::as_str) {
        Some(existing) if existing == note => return,
        Some(existing) if !existing.is_empty() => format!("{} {}", existing, note),
        _ => note,
    };
    summary["searchModeNote"] = Value::String(next);
}

fn inject_phrase_diagnostic_note(summary: &mut Value, diag: &PhraseSearchDiag) {
    if let Some(note) = diag.missing_tokens_note() {
        append_search_mode_note(summary, note);
    }
}

fn phrase_warning_json(phrase: &str, diag: &PhraseSearchDiag) -> Value {
    let missing_tokens = diag.missing_tokens
        .iter()
        .take(PHRASE_WARNING_MISSING_TOKENS_MAX)
        .cloned()
        .collect::<Vec<_>>();
    let mut warning = json!({
        "phrase": phrase,
        "tokenCount": diag.token_count,
        "missingTokenCount": diag.missing_tokens.len(),
        "missingTokens": missing_tokens,
    });
    if diag.missing_tokens.len() > PHRASE_WARNING_MISSING_TOKENS_MAX {
        warning["missingTokensOmitted"] = json!(diag.missing_tokens.len() - PHRASE_WARNING_MISSING_TOKENS_MAX);
    }
    warning
}

fn inject_multi_phrase_warnings(summary: &mut Value, phrase_warnings: &[Value], omitted: usize) {
    if phrase_warnings.is_empty() && omitted == 0 {
        return;
    }
    if !phrase_warnings.is_empty() {
        summary["phraseWarnings"] = json!(phrase_warnings);
    }
    if omitted > 0 {
        summary["phraseWarningsOmitted"] = json!(omitted);
    }
    append_search_mode_note(
        summary,
        "One or more phrase terms stopped before reading files because an index token had no postings; see summary.phraseWarnings. Try substring=true for token-substring search, or lineRegex=true for raw literal matching.".to_string(),
    );
}

/// Core phrase-matching logic: finds files containing the given phrase.
/// Extracted to allow reuse by both single-phrase and multi-phrase search.
fn collect_phrase_matches(
    index: &ContentIndex,
    phrase: &str,
    params: &GrepSearchParams,
) -> Result<(Vec<PhraseFileMatch>, PhraseSearchDiag), String> {
    let show_lines = params.show_lines;
    let mut diag = PhraseSearchDiag::default();

    let phrase_lower = phrase.to_lowercase();
    let phrase_tokens = tokenize(&phrase_lower, 2);

    if phrase_tokens.is_empty() {
        return Err(format!(
            "Phrase '{}' has no indexable tokens (min length 2). \
             To search for punctuation/operators, use lineRegex=true \
             \u{2014} it bypasses the token index and matches raw line content. \
             Example: terms=[\"{}\"], lineRegex=true.",
            phrase, regex::escape(phrase)
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

    // PERF (tier-B): use per-token line lists from the inverted index to
    // intersect at the LINE level, not just the file level. The existing
    // `Posting { file_id, lines: Vec<u32> }` already records which lines a
    // token appears on inside each file; the previous implementation threw
    // that information away (only kept `file_id`) and re-scanned every
    // candidate file's bytes through a regex. The new flow:
    //   1. For each phrase token, collect (file_id -> sorted-unique line set)
    //      from its postings, applying file filters once per posting.
    //   2. Intersect file ids AND line numbers across all tokens. A file
    //      where token A appears only on line 5 and token B appears only on
    //      line 12 is dropped without ever opening the file -- the phrase
    //      cannot fit on a single line in that file.
    //   3. Read only the surviving candidate files and verify the phrase
    //      regex/substring on the small set of candidate lines.
    // For phrases like "foo bar" on a large repo the candidate
    // file count typically drops from hundreds to ~the result count itself,
    // eliminating most of the disk I/O that dominated phrase search runtime.
    // Backward compatible: no index format change.
    diag.token_count = phrase_tokens.len();
    let posting_scan_start = Instant::now();
    let mut per_token_file_lines: Vec<HashMap<u32, Vec<u32>>> =
        Vec::with_capacity(phrase_tokens.len());
    for token in &phrase_tokens {
        let token_start = Instant::now();
        let postings = match index.index.get(token.as_str()) {
            Some(p) => p,
            None => {
                diag.missing_tokens.push(token.clone());
                diag.per_token.push((
                    token.clone(),
                    0,
                    0,
                    token_start.elapsed().as_secs_f64() * 1000.0,
                ));
                diag.posting_scan_ms = posting_scan_start.elapsed().as_secs_f64() * 1000.0;
                return Ok((Vec::new(), diag));
            }
        };
        let posting_count = postings.len();
        let mut pass_count = 0usize;
        let mut map: HashMap<u32, Vec<u32>> = HashMap::with_capacity(postings.len());
        for p in postings {
            let path = match index.files.get(p.file_id as usize) {
                Some(p) => p,
                None => continue,
            };
            if !passes_file_filters(path, params) {
                continue;
            }
            // Postings record a line number once per occurrence; dedup so
            // intersection works on sets, not multisets.
            let mut lines = p.lines.clone();
            lines.sort_unstable();
            lines.dedup();
            map.insert(p.file_id, lines);
            pass_count += 1;
        }
        diag.per_token.push((token.clone(), posting_count, pass_count,
            token_start.elapsed().as_secs_f64() * 1000.0));
        if map.is_empty() {
            diag.posting_scan_ms = posting_scan_start.elapsed().as_secs_f64() * 1000.0;
            return Ok((Vec::new(), diag));
        }
        per_token_file_lines.push(map);
    }
    diag.posting_scan_ms = posting_scan_start.elapsed().as_secs_f64() * 1000.0;

    // Start the intersection from the smallest per-token map -- minimises
    // outer-loop iterations and the size of `current_lines` we carry forward.
    let smallest_idx = per_token_file_lines
        .iter()
        .enumerate()
        .min_by_key(|(_, m)| m.len())
        .map(|(i, _)| i)
        .unwrap_or(0);
    let smallest = per_token_file_lines.swap_remove(smallest_idx);
    let other_maps: &[HashMap<u32, Vec<u32>>] = &per_token_file_lines;
    let intersection_start = Instant::now();

    let mut candidates: Vec<(u32, Vec<u32>)> = Vec::new();
    for (file_id, mut current_lines) in smallest {
        let mut keep = true;
        for other in other_maps {
            match other.get(&file_id) {
                None => {
                    keep = false;
                    break;
                }
                Some(other_lines) => {
                    current_lines = intersect_sorted_unique(&current_lines, other_lines);
                    if current_lines.is_empty() {
                        keep = false;
                        break;
                    }
                }
            }
        }
        if keep && !current_lines.is_empty() {
            candidates.push((file_id, current_lines));
        }
    }

    // Verify phrase match in raw file content.
    // When phrase contains punctuation, use raw substring match to avoid
    // false positives from tokenizer stripping non-alphanumeric characters.
    let phrase_has_punctuation = phrase.chars().any(|c| !c.is_alphanumeric() && !c.is_whitespace());
    diag.intersection_ms = intersection_start.elapsed().as_secs_f64() * 1000.0;
    diag.candidates_after_intersection = candidates.len();

    // PERF (tier-A): parallelize file I/O + per-file scan across the
    // surviving (post-line-intersection) candidates. Even after tier-B's
    // file-skip, the remaining candidates each require one disk read +
    // one full content.lines() walk; spreading the work across worker
    // threads via std::thread::scope (no new deps) keeps the wall-clock
    // bounded by the slowest read on multi-core boxes.
    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    let chunk_size = candidates.len().div_ceil(num_threads).max(1);
    let file_verify_start = Instant::now();

    let results: Vec<PhraseFileMatch> = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for chunk in candidates.chunks(chunk_size) {
            let phrase_re_ref = &phrase_re;
            let phrase_lower_ref = phrase_lower.as_str();
            let index_ref = index;
            let chunk_owned: Vec<(u32, Vec<u32>)> = chunk.to_vec();
            let handle = scope.spawn(move || {
                let mut local: Vec<PhraseFileMatch> = Vec::with_capacity(chunk_owned.len());
                for (file_id, candidate_lines) in chunk_owned {
                    let file_path = &index_ref.files[file_id as usize];
                    let (content, _lossy) = match read_file_lossy(std::path::Path::new(file_path)) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };
                    let candidate_set: HashSet<u32> = candidate_lines.into_iter().collect();
                    let mut matching_lines = Vec::new();
                    for (line_num, line) in content.lines().enumerate() {
                        let line_no = (line_num + 1) as u32;
                        // Tier-B: only verify lines that the index says contain ALL
                        // phrase tokens. Other lines cannot satisfy the phrase regex
                        // (which requires every token, separated by whitespace, on
                        // a single line) nor the lowercase-substring punctuation
                        // path (which also matches single-line content).
                        if !candidate_set.contains(&line_no) {
                            continue;
                        }
                        let hit = if phrase_has_punctuation {
                            line.to_lowercase().contains(phrase_lower_ref)
                        } else {
                            phrase_re_ref.is_match(line)
                        };
                        if hit {
                            matching_lines.push(line_no);
                        }
                    }
                    if !matching_lines.is_empty() {
                        local.push(PhraseFileMatch {
                            file_path: file_path.clone(),
                            lines: matching_lines,
                            content: if show_lines { Some(content) } else { None },
                        });
                    }
                }
                local
            });
            handles.push(handle);
        }
        let mut all = Vec::new();
        let mut worker_panics: usize = 0;
        for h in handles {
            match h.join() {
                Ok(local) => all.extend(local),
                // Bug 9 (consolidated plan 2026-04-23): a panic in a phrase-
                // verification worker silently dropped that chunk's results.
                // Now we count panics and emit a `tracing::warn!` so the issue
                // surfaces in logs / metrics instead of returning quietly
                // with fewer matches than the user expects.
                Err(_) => worker_panics += 1,
            }
        }
        if worker_panics > 0 {
            tracing::warn!(
                worker_panics = worker_panics,
                "phrase verification worker(s) panicked; result set may be incomplete"
            );
        }
        all
    });

    diag.file_verify_ms = file_verify_start.elapsed().as_secs_f64() * 1000.0;
    diag.files_read = results.len();
    diag.result_count = results.iter().map(|r| r.lines.len()).sum();

    info!(
        phrase = %phrase,
        tokens = diag.token_count,
        posting_scan_ms = format_args!("{:.1}", diag.posting_scan_ms),
        intersection_ms = format_args!("{:.1}", diag.intersection_ms),
        candidates = diag.candidates_after_intersection,
        file_verify_ms = format_args!("{:.1}", diag.file_verify_ms),
        files_read = diag.files_read,
        results = diag.result_count,
        "[phrase-trace] collect_phrase_matches complete"
    );
    for (token, postings, passed, ms) in &diag.per_token {
        info!(
            token = %token,
            postings = postings,
            passed = passed,
            ms = format_args!("{:.1}", ms),
            "[phrase-trace] per-token posting scan"
        );
    }

    Ok((results, diag))
}

/// Intersection of two sorted-unique `Vec<u32>` lists, in O(n + m).
/// Both inputs MUST be sorted ascending and free of duplicates; the result
/// preserves both invariants.
fn intersect_sorted_unique(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len().min(b.len()));
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    out
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
    let mut last_diag = PhraseSearchDiag::default();
    let mut phrase_warnings: Vec<Value> = Vec::new();
    let mut phrase_warnings_omitted = 0usize;

    for phrase in phrases {
        match collect_phrase_matches(index, phrase, params) {
            Ok((matches, diag)) => {
                if diag.has_missing_tokens() {
                    if phrase_warnings.len() < PHRASE_WARNINGS_MAX {
                        phrase_warnings.push(phrase_warning_json(phrase, &diag));
                    } else {
                        phrase_warnings_omitted += 1;
                    }
                }
                last_diag = diag;
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

    // Sort by occurrences descending, tie-break by file path ascending for
    // deterministic ordering after truncation (see collect_phrase_matches).
    let mut results = merged;
    results.sort_by(|a, b| {
        b.lines.len()
            .cmp(&a.lines.len())
            .then_with(|| a.file_path.cmp(&b.file_path))
    });

    if max_results > 0 {
        results.truncate(max_results);
    }

    let search_elapsed = search_start.elapsed();
    let search_mode = if mode_and { "phrase-and" } else { "phrase-or" };
    let terms_value = json!(searched_terms);

    if count_only {
        let mut summary = build_grep_base_summary(
            total_files, total_occurrences, &terms_value, search_mode,
            index, search_elapsed, ctx, true, params.lock_wait_ms,
        );
        apply_dir_auto_converted_note(&mut summary, params);
        summary["phraseDetail"] = last_diag.to_json();
        let result_status = build_grep_result_status(0, 0, total_files, total_occurrences, true);
        let execution = build_grep_execution(params, search_mode, None, false, Some(total_files), None);
        inject_multi_phrase_warnings(&mut summary, &phrase_warnings, phrase_warnings_omitted);
        let output = finalize_grep_output(
            json!({ "summary": summary }),
            result_status,
            execution,
            &terms_value,
            params,
        );
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

    let mut summary = build_grep_base_summary(
        total_files, total_occurrences, &terms_value, search_mode,
        index, search_elapsed, ctx, true, params.lock_wait_ms,
    );
    apply_dir_auto_converted_note(&mut summary, params);
    summary["phraseDetail"] = last_diag.to_json();
    let returned_occurrences = results.iter().map(|result| result.lines.len()).sum();
    let result_status = build_grep_result_status(
        results.len(),
        returned_occurrences,
        total_files,
        total_occurrences,
        false,
    );
    let execution = build_grep_execution(params, search_mode, None, false, Some(total_files), None);
    inject_multi_phrase_warnings(&mut summary, &phrase_warnings, phrase_warnings_omitted);
    let output = finalize_grep_output(
        json!({
            "files": files_json,
            "summary": summary
        }),
        result_status,
        execution,
        &terms_value,
        params,
    );

    ToolCallResult::success(json_to_string(&output))
}

/// Line-based regex search: applies the user-supplied regex to each line of
/// candidate files. Supports line anchors `^`/`$`, whitespace, and arbitrary
/// non-token characters that token-based regex cannot match.
///
/// Performance: this mode is intentionally slower than token-based regex
/// (~10-100× depending on candidate file count and file sizes). Use file
/// scope filters (`ext`, `dir`, `file`) to keep the candidate set small.
/// Without filters, it scans every indexed file — still fast enough for
/// typical projects (<10K files), but pay attention to scope.
///
/// Multi-pattern support: comma-separated patterns are searched independently
/// with OR semantics (a file matches if ANY pattern hits at least one line).
/// `mode=and` switches to AND (file must contain at least one match for EVERY
/// pattern). Each pattern is compiled once and applied to every line of every
/// candidate file.
fn handle_line_regex_search(
    ctx: &HandlerContext,
    index: &ContentIndex,
    patterns: Vec<String>,
    params: &GrepSearchParams,
) -> ToolCallResult {
    // AC-4 differential-test toggle: production code always passes `true`,
    // but the `cfg(test)` build honours a thread-local override so a
    // differential test can drive the SAME entry point with the prefilter
    // disabled and assert files+lines parity. The override is OFF by default
    // in tests too, so existing tests keep the production-equivalent path.
    #[cfg(test)]
    let prefilter_enabled = !line_regex_prefilter_disabled_for_test();
    #[cfg(not(test))]
    let prefilter_enabled = !params.trigram_stale;
    handle_line_regex_search_inner(ctx, index, patterns, params, prefilter_enabled)
}

#[cfg(test)]
thread_local! {
    /// Per-thread switch read by [`handle_line_regex_search`] in test builds.
    /// Set via [`set_line_regex_prefilter_disabled_for_test`] for the
    /// duration of one differential-test scope. Defaults to `false` so
    /// untouched tests behave exactly like production.
    static LINE_REGEX_PREFILTER_DISABLED_FOR_TEST: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn line_regex_prefilter_disabled_for_test() -> bool {
    LINE_REGEX_PREFILTER_DISABLED_FOR_TEST.with(|c| c.get())
}

#[cfg(test)]
pub(crate) fn set_line_regex_prefilter_disabled_for_test(disabled: bool) {
    LINE_REGEX_PREFILTER_DISABLED_FOR_TEST.with(|c| c.set(disabled));
}

/// Inner implementation of [`handle_line_regex_search`]. The
/// `prefilter_enabled` flag will be wired in step 3 of AC-4 to gate the
/// literal-trigram prefilter (kept here in step 2 only to enable the
/// `#[cfg(test)]` differential check added in step 5). For now both call
/// sites pass `true` and the flag is intentionally unused.
fn line_regex_candidate_count(
    index: &ContentIndex,
    candidate_file_ids: Option<&HashSet<u32>>,
) -> usize {
    candidate_file_ids.map_or(index.files.len(), HashSet::len)
}

fn line_regex_scan_worker_threads(candidate_count: usize) -> usize {
    if candidate_count < LINE_REGEX_PARALLEL_SCAN_MIN_FILES {
        return 1;
    }
    let available_threads = std::thread::available_parallelism()
        .map(|threads| threads.get())
        .unwrap_or(1);
    available_threads.min(LINE_REGEX_PARALLEL_SCAN_MAX_THREADS)
}

fn scan_line_regex_file(
    file_id: usize,
    file_path: &str,
    candidate_file_ids: Option<&HashSet<u32>>,
    compiled: &[regex::Regex],
    params: &GrepSearchParams,
) -> LineRegexFileScanOutput {
    let mut output = LineRegexFileScanOutput::default();
    output.counters.files_visited = 1;

    let candidate_filter_start = Instant::now();
    if let Some(candidates) = candidate_file_ids
        && !candidates.contains(&(file_id as u32))
    {
        output.timings.candidate_filter_duration = candidate_filter_start.elapsed();
        output.counters.files_skipped_by_prefilter = 1;
        return output;
    }
    output.timings.candidate_filter_duration = candidate_filter_start.elapsed();

    let scope_filter_start = Instant::now();
    if !passes_file_filters(file_path, params) {
        output.timings.scope_filter_duration = scope_filter_start.elapsed();
        output.counters.files_skipped_by_scope = 1;
        return output;
    }
    output.timings.scope_filter_duration = scope_filter_start.elapsed();

    let read_start = Instant::now();
    let content = match read_file_lossy(std::path::Path::new(file_path)) {
        Ok((content, _lossy)) => content,
        Err(_) => return output,
    };
    output.timings.read_duration = read_start.elapsed();
    output.counters.files_read = 1;
    output.counters.bytes_read = content.len();

    output.counters.whole_file_precheck_files = 1;
    let precheck_start = Instant::now();
    let any_pattern_matches = compiled.iter().any(|regex| regex.is_match(&content));
    output.timings.whole_file_precheck_duration = precheck_start.elapsed();
    if !any_pattern_matches {
        return output;
    }
    output.counters.whole_file_precheck_matched_files = 1;
    output.counters.line_eval_files = 1;

    for (pattern_index, regex) in compiled.iter().enumerate() {
        let line_eval_start = Instant::now();
        let mut matching_lines: Vec<u32> = Vec::new();
        for (line_number, line) in content.lines().enumerate() {
            output.counters.line_eval_lines = output.counters.line_eval_lines.saturating_add(1);
            if regex.is_match(line) {
                matching_lines.push((line_number + 1) as u32);
            }
        }
        output.timings.line_eval_duration += line_eval_start.elapsed();

        let match_bookkeeping_start = Instant::now();
        if !matching_lines.is_empty() {
            output.matched_lines_by_pattern.push((pattern_index, matching_lines));
        }
        output.timings.match_bookkeeping_duration += match_bookkeeping_start.elapsed();
    }

    if !output.matched_lines_by_pattern.is_empty() {
        output.path = Some(file_path.to_string());
    }

    output
}

fn collect_line_regex_scan_outputs(
    index: &ContentIndex,
    candidate_file_ids: Option<&HashSet<u32>>,
    compiled: &[regex::Regex],
    params: &GrepSearchParams,
) -> Result<(Vec<LineRegexFileScanOutput>, bool, usize, Duration), String> {
    let candidate_count = line_regex_candidate_count(index, candidate_file_ids);
    let worker_threads = line_regex_scan_worker_threads(candidate_count);
    let parallel_scan = worker_threads > 1;
    let scan_start = Instant::now();

    if !parallel_scan {
        let outputs = index.files.iter().enumerate()
            .map(|(file_id, file_path)| {
                scan_line_regex_file(
                    file_id,
                    file_path,
                    candidate_file_ids,
                    compiled,
                    params,
                )
            })
            .collect();
        return Ok((outputs, false, 1, scan_start.elapsed()));
    }

    let chunk_size = index.files.len().div_ceil(worker_threads);
    let mut outputs: Vec<LineRegexFileScanOutput> = Vec::with_capacity(index.files.len());
    let mut worker_panicked = false;

    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (chunk_index, file_chunk) in index.files.chunks(chunk_size).enumerate() {
            let first_file_id = chunk_index * chunk_size;
            handles.push(scope.spawn(move || {
                file_chunk.iter().enumerate()
                    .map(|(offset, file_path)| {
                        scan_line_regex_file(
                            first_file_id + offset,
                            file_path,
                            candidate_file_ids,
                            compiled,
                            params,
                        )
                    })
                    .collect::<Vec<_>>()
            }));
        }

        for handle in handles {
            match handle.join() {
                Ok(mut chunk_outputs) => outputs.append(&mut chunk_outputs),
                Err(_) => worker_panicked = true,
            }
        }
    });

    if worker_panicked {
        return Err("lineRegex parallel scan worker panicked".to_string());
    }

    Ok((outputs, true, worker_threads, scan_start.elapsed()))
}

fn aggregate_line_regex_scan_outputs(
    scan_outputs: Vec<LineRegexFileScanOutput>,
    pattern_count: usize,
    telemetry: &mut LineRegexScanTelemetry,
) -> Vec<HashMap<String, Vec<u32>>> {
    let mut per_pattern_matches: Vec<HashMap<String, Vec<u32>>> = vec![HashMap::new(); pattern_count];
    for output in scan_outputs {
        telemetry.add_file_scan_output(&output);
        let Some(file_path) = output.path else {
            continue;
        };
        for (pattern_index, matching_lines) in output.matched_lines_by_pattern {
            per_pattern_matches[pattern_index].insert(file_path.clone(), matching_lines);
        }
    }
    per_pattern_matches
}

fn attach_line_regex_content_previews(results: &mut [PhraseFileMatch]) -> bool {
    let mut cache_bytes_used: usize = 0;
    let mut line_content_truncated = false;
    for result in results.iter_mut() {
        let content = match read_file_lossy(std::path::Path::new(&result.file_path)) {
            Ok((content, _lossy)) => content,
            Err(_) => continue,
        };
        if cache_bytes_used.saturating_add(content.len()) > LINE_REGEX_MAX_CONTENT_CACHE_BYTES {
            line_content_truncated = true;
            continue;
        }
        cache_bytes_used = cache_bytes_used.saturating_add(content.len());
        result.content = Some(content);
    }
    line_content_truncated
}

fn handle_line_regex_search_inner(
    ctx: &HandlerContext,
    index: &ContentIndex,
    patterns: Vec<String>,
    params: &GrepSearchParams,
    prefilter_enabled: bool,
) -> ToolCallResult {
    // `patterns` is supplied by the caller as one regex per array entry
    // (taken verbatim from the `terms` array — literal `,` inside an entry is
    // preserved). Unlike token regex, we do NOT lowercase — user-supplied
    // regex flags (e.g., `(?i)`) control case sensitivity. We also do NOT
    // trim each pattern, because whitespace inside a regex is significant
    // (e.g., `^## ` matches markdown level-2 headings only, NOT `^##` which would
    // also match `^### `).
    let patterns: Vec<String> = patterns
        .into_iter()
        .filter(|pattern| !pattern.is_empty())
        .collect();

    if patterns.is_empty() {
        return ToolCallResult::error("No search patterns provided".to_string());
    }

    // Compile all patterns up-front with multi_line=true so `^` and `$` anchor
    // to line boundaries (not input boundaries). Without this, `^foo` would only
    // match at the very start of the file content, breaking any anchor-based
    // search on multi-line files. user-supplied flags like `(?m)`/`(?s)` still
    // override our defaults.
    //
    // We ALSO enable `crlf(true)`. Without it, `$` in multi-line mode matches
    // only the position before `\n`, NOT before `\r\n` — so the whole-content
    // pre-check (`re.is_match(&content)`) silently rejects CRLF files whose
    // matches end with `}\r\n` and the per-line scan that would have matched
    // never runs. With `crlf(true)`, both `^` and `$` treat `\r\n` as a single
    // line terminator, matching how `str::lines()` splits content for the
    // per-line scan below. Regression test:
    // `tests_line_regex::line_regex_dollar_anchor_crlf_file_matches_closing_brace`.
    let mut scan_telemetry = LineRegexScanTelemetry::default();

    let compile_start = Instant::now();
    let mut compiled: Vec<regex::Regex> = Vec::with_capacity(patterns.len());
    for pattern in &patterns {
        match regex::RegexBuilder::new(pattern)
            .multi_line(true)
            .crlf(true)
            .build()
        {
            Ok(regex) => compiled.push(regex),
            Err(error) => return ToolCallResult::error(format!("Invalid regex '{}': {}", pattern, error)),
        }
    }
    scan_telemetry.compile_duration = compile_start.elapsed();

    // AC-4: literal-trigram prefilter. When enabled and the regex pattern
    // exposes a fixed substring prefix (e.g. `App\s*=\s*\d+` -> `app`), shrink
    // the file iteration to the trigram-derived candidate set. `None` means
    // "no prefilter" (extraction failed, OR-mode unprefilterable, or short-
    // circuit) — fall back to scanning every file. `_prefilter_info` is
    // captured here for the summary observability added in step 4.
    let literal_prefilter_start = Instant::now();
    let (candidate_file_ids, mut prefilter_info): (Option<HashSet<u32>>, LiteralPrefilterInfo) =
        if prefilter_enabled {
            compute_literal_prefilter(index, &patterns, params.mode_and)
        } else {
            (
                None,
                LiteralPrefilterInfo {
                    total_files: index.files.len(),
                    reason: Some("prefilter disabled by caller".into()),
                    ..LiteralPrefilterInfo::default()
                },
            )
        };
    scan_telemetry.literal_prefilter_duration = literal_prefilter_start.elapsed();

    // Scope-aware counters: when any `dir`/`file`/`ext`/`exclude*` filter is
    // set, do a single pass over `index.files` and count (a) how many files
    // survive scope without the prefilter, and (b) how many candidate-set
    // files survive scope. Both numbers are emitted as
    // `summary.literalPrefilter.{total,candidate}FilesAfterScope` so callers
    // can distinguish "prefilter narrowed" from "scope narrowed". Without
    // this, scoped queries on a 60k-file index always reported the same
    // global counts, which is the cognitive trap that motivated the
    // alternation-split advisory revert.
    if params_have_scope_filter(params) {
        let scope_count_start = Instant::now();
        let mut total_after = 0usize;
        let mut cand_after = 0usize;
        for (file_id, file_path) in index.files.iter().enumerate() {
            if !passes_file_filters(file_path, params) {
                continue;
            }
            total_after += 1;
            if let Some(ref candidates) = candidate_file_ids
                && candidates.contains(&(file_id as u32))
            {
                cand_after += 1;
            }
        }
        prefilter_info.total_files_after_scope = Some(total_after);
        if candidate_file_ids.is_some() {
            prefilter_info.candidate_files_after_scope = Some(cand_after);
        }
        scan_telemetry.scope_count_duration = scope_count_start.elapsed();
    }

    let (scan_outputs, parallel_scan, worker_threads, scan_duration) = match collect_line_regex_scan_outputs(
        index,
        candidate_file_ids.as_ref(),
        &compiled,
        params,
    ) {
        Ok(scan_result) => scan_result,
        Err(error) => return ToolCallResult::error(error),
    };
    scan_telemetry.parallel_scan = parallel_scan;
    scan_telemetry.worker_threads = worker_threads;
    scan_telemetry.scan_duration = scan_duration;

    let per_pattern_matches = aggregate_line_regex_scan_outputs(
        scan_outputs,
        patterns.len(),
        &mut scan_telemetry,
    );

    // Merge per-pattern matches with OR or AND semantics.
    let merge_start = Instant::now();
    let merged_files: HashMap<String, Vec<u32>> = if params.mode_and {
        // Files appearing in ALL pattern result sets.
        let mut common: Option<HashSet<String>> = None;
        for pattern_matches in &per_pattern_matches {
            let files: HashSet<String> = pattern_matches.keys().cloned().collect();
            common = Some(match common {
                None => files,
                Some(prev) => prev.intersection(&files).cloned().collect(),
            });
        }
        let common = common.unwrap_or_default();
        let mut merged: HashMap<String, Vec<u32>> = HashMap::new();
        for pattern_matches in &per_pattern_matches {
            for (path, lines) in pattern_matches {
                if common.contains(path) {
                    merged.entry(path.clone()).or_default().extend_from_slice(lines);
                }
            }
        }
        merged
    } else {
        // OR: union of all files.
        let mut merged: HashMap<String, Vec<u32>> = HashMap::new();
        for pattern_matches in &per_pattern_matches {
            for (path, lines) in pattern_matches {
                merged.entry(path.clone()).or_default().extend_from_slice(lines);
            }
        }
        merged
    };

    scan_telemetry.merge_duration = merge_start.elapsed();

    // Sort/dedup line numbers per file.
    let sort_dedup_start = Instant::now();
    let mut results: Vec<PhraseFileMatch> = merged_files.into_iter()
        .map(|(file_path, mut lines)| {
            lines.sort();
            lines.dedup();
            PhraseFileMatch { file_path, lines, content: None }
        })
        .collect();
    scan_telemetry.sort_dedup_duration = sort_dedup_start.elapsed();

    let rank_truncate_start = Instant::now();
    let total_files = results.len();
    scan_telemetry.matched_files = total_files;
    let total_occurrences: usize = results.iter().map(|result| result.lines.len()).sum();

    // Sort by occurrences descending (most matches first), like phrase search.
    // Tie-break by file path ascending for deterministic truncated output.
    results.sort_by(|left, right| {
        right.lines.len()
            .cmp(&left.lines.len())
            .then_with(|| left.file_path.cmp(&right.file_path))
    });

    if params.max_results > 0 {
        results.truncate(params.max_results);
    }
    scan_telemetry.rank_truncate_duration = rank_truncate_start.elapsed();

    let search_elapsed = params.search_start.elapsed();
    let search_mode = if params.mode_and { "lineRegex-and" } else { "lineRegex" };

    let terms_value = json!(patterns);

    if params.count_only {
        let response_build_start = Instant::now();
        let mut summary = build_grep_base_summary(
            total_files, total_occurrences, &terms_value, search_mode,
            index, search_elapsed, ctx, true, params.lock_wait_ms,
        );
        apply_dir_auto_converted_note(&mut summary, params);
        scan_telemetry.response_build_duration = response_build_start.elapsed();
        let response_finalize_start = Instant::now();
        let total_elapsed_ms = params.search_start.elapsed().as_millis() as u64;
        let summary_json = finalize_line_regex_summary(
            &mut summary,
            &prefilter_info,
            &mut scan_telemetry,
            total_elapsed_ms,
            search_mode,
        );
        let summary_value = serde_json::from_str::<Value>(&summary_json).unwrap_or(summary);
        let result_status = build_grep_result_status(0, 0, total_files, total_occurrences, true);
        let candidate_files = prefilter_info
            .candidate_files_after_scope
            .or(if prefilter_info.used { Some(prefilter_info.candidate_files) } else { prefilter_info.total_files_after_scope });
        let execution = build_grep_execution(
            params,
            search_mode,
            prefilter_info.reason.as_deref(),
            prefilter_info.used,
            candidate_files,
            Some(scan_telemetry.files_visited),
        );
        let output = finalize_grep_output(
            json!({ "summary": summary_value }),
            result_status,
            execution,
            &terms_value,
            params,
        );
        scan_telemetry.response_finalize_duration = response_finalize_start.elapsed();
        let response = replace_response_finalize_placeholder(
            json_to_string(&output),
            scan_telemetry.response_finalize_ms(),
        );
        let total_elapsed_ms = params.search_start.elapsed().as_millis() as u64;
        let response = replace_perf_hint_placeholder(
            response,
            final_line_regex_perf_hint(
                search_mode,
                total_elapsed_ms,
                &prefilter_info,
                &scan_telemetry,
            ),
        );
        return ToolCallResult::success(response);
    }

    let response_build_start = Instant::now();
    let line_content_truncated = if params.show_lines {
        attach_line_regex_content_previews(&mut results)
    } else {
        false
    };
    let files_json: Vec<Value> = results.iter().map(|result| {
        let mut file_obj = json!({
            "path": result.file_path,
            "occurrences": result.lines.len(),
            "lines": result.lines,
        });
        if params.show_lines
            && let Some(ref content) = result.content {
                file_obj["lineContent"] = build_line_content_from_matches(content, &result.lines, params.context_lines);
            }
        file_obj
    }).collect();

    let mut summary = build_grep_base_summary(
        total_files, total_occurrences, &terms_value, search_mode,
        index, search_elapsed, ctx, true, params.lock_wait_ms,
    );
    apply_dir_auto_converted_note(&mut summary, params);
    if line_content_truncated {
        // MINOR-23: tell the client that lineContent previews are partial.
        // The matched line *numbers* are complete; only `lineContent` arrays
        // for some files are absent because the cache budget was exceeded.
        if let Some(obj) = summary.as_object_mut() {
            obj.insert(
                "lineContentTruncated".into(),
                json!(true),
            );
            obj.insert(
                "lineContentTruncationReason".into(),
                json!(format!(
                    "showLines content cache exceeded {} MiB cap; line numbers are complete but some files lack `lineContent` previews.",
                    LINE_REGEX_MAX_CONTENT_CACHE_BYTES / (1024 * 1024)
                )),
            );
        }
    }
    let returned_occurrences = results.iter().map(|result| result.lines.len()).sum();
    scan_telemetry.response_build_duration = response_build_start.elapsed();
    let response_finalize_start = Instant::now();
    let total_elapsed_ms = params.search_start.elapsed().as_millis() as u64;
    let summary_json = finalize_line_regex_summary(
        &mut summary,
        &prefilter_info,
        &mut scan_telemetry,
        total_elapsed_ms,
        search_mode,
    );
    let summary_value = serde_json::from_str::<Value>(&summary_json).unwrap_or(summary);
    let result_status = build_grep_result_status(
        results.len(),
        returned_occurrences,
        total_files,
        total_occurrences,
        false,
    );
    let candidate_files = prefilter_info
        .candidate_files_after_scope
        .or(if prefilter_info.used { Some(prefilter_info.candidate_files) } else { prefilter_info.total_files_after_scope });
    let execution = build_grep_execution(
        params,
        search_mode,
        prefilter_info.reason.as_deref(),
        prefilter_info.used,
        candidate_files,
        Some(scan_telemetry.files_visited),
    );
    let output = finalize_grep_output(
        json!({
            "files": files_json,
            "summary": summary_value,
        }),
        result_status,
        execution,
        &terms_value,
        params,
    );
    scan_telemetry.response_finalize_duration = response_finalize_start.elapsed();
    let response = replace_response_finalize_placeholder(
        json_to_string(&output),
        scan_telemetry.response_finalize_ms(),
    );
    let total_elapsed_ms = params.search_start.elapsed().as_millis() as u64;
    let response = replace_perf_hint_placeholder(
        response,
        final_line_regex_perf_hint(
            search_mode,
            total_elapsed_ms,
            &prefilter_info,
            &scan_telemetry,
        ),
    );

    ToolCallResult::success(response)
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
