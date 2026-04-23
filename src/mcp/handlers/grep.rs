//! xray_grep handler: token search, substring search, phrase search.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde_json::{json, Value};
use tracing::debug;

use crate::mcp::protocol::ToolCallResult;
use crate::{read_file_lossy, tokenize, ContentIndex};
use crate::index::build_trigram_index;
use code_xray::generate_trigrams;

#[allow(unused_imports)] // `self` needed by test submodules for utils::ExcludePatterns
use super::utils::{self,
    build_line_content_from_matches, inject_branch_warning, is_under_dir, json_to_string,
    matches_ext_filter, sorted_intersect, validate_search_dir,
};
use super::HandlerContext;

/// Shared parameters for substring and phrase search modes.
/// Eliminates 10+ positional parameters from handle_substring_search and handle_phrase_search.
pub(crate) struct GrepSearchParams<'a> {
    pub ext_filter: &'a Option<String>,
    pub show_lines: bool,
    pub context_lines: usize,
    pub max_results: usize,
    pub mode_and: bool,
    pub count_only: bool,
    pub search_start: Instant,
    pub dir_filter: &'a Option<String>,
    /// Optional file path/name substring filter (lowercase match against full file path).
    /// Supports comma-separated terms (OR semantics) — any term matching accepts the file.
    pub file_filter: &'a Option<String>,
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

    // File name/path substring filter (comma-separated OR semantics).
    // Match is case-insensitive and checks both the full path and the basename
    // so `file='CHANGELOG.md'` works regardless of the caller's path style.
    if let Some(file_sub) = params.file_filter {
        let fp_lower = file_path.to_lowercase().replace('\\', "/");
        let basename_lower = std::path::Path::new(file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        let any_match = file_sub.split(',')
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .any(|needle| {
                fp_lower.contains(&needle) || basename_lower.contains(&needle)
            });
        if !any_match { return false; }
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

/// Parsed arguments for the grep handler. Extracts all parameter parsing
/// from the main handler to reduce its cognitive complexity.
#[derive(Debug)]
struct ParsedGrepArgs {
    terms_str: String,
    dir_filter: Option<String>,
    ext_filter: Option<String>,
    file_filter: Option<String>,
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
    let terms_str = match args.get("terms").and_then(|v| v.as_str()) {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        Some(_) => return Err(ToolCallResult::error(
            "Parameter 'terms' must not be empty. Provide one or more search terms (comma-separated for multi-term).".to_string(),
        )),
        None => return Err(ToolCallResult::error("Missing required parameter: terms".to_string())),
    };

    // Explicit `file` filter (user-provided). Takes precedence over dir-autoconvert filename.
    let mut file_filter: Option<String> = args.get("file")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let mut dir_auto_converted_note: Option<String> = None;

    let dir_filter: Option<String> = if let Some(dir) = args.get("dir").and_then(|v| v.as_str()) {
        match validate_search_dir(dir, server_dir) {
            Ok(filter) => {
                // Detect file paths passed as dir= and auto-convert to parent-dir + file filter.
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
                        // Explicit `file=` wins; otherwise auto-populate from the filename.
                        if file_filter.is_none() && !filename.is_empty() {
                            file_filter = Some(filename.clone());
                        }
                        dir_auto_converted_note = Some(format!(
                            "dir='{}' looked like a file path — auto-converted to dir='{}' file='{}'. \
                             Next time pass file='<name>' (or dir=<parent>) directly to avoid this conversion.",
                            dir, parent, filename
                        ));
                        // Re-validate the parent dir against server_dir scope.
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

    let ext_filter = args.get("ext").and_then(|v| v.as_str()).map(|s| s.to_string());
    let mode_and = args.get("mode").and_then(|v| v.as_str()) == Some("and");
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
        terms_str,
        dir_filter,
        ext_filter,
        file_filter,
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
        let mut summary = build_grep_base_summary(
            total_files, total_occurrences, &json!(terms), search_mode,
            index, search_elapsed, ctx, true,
        );
        apply_dir_auto_converted_note(&mut summary, params);
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

    let mut summary = build_grep_base_summary(
        total_files, total_occurrences, &json!(terms), search_mode,
        index, search_elapsed, ctx, true,
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
    let parsed = match parse_grep_args(args, &ctx.server_dir()) {
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

    let exclude_patterns = super::utils::ExcludePatterns::from_dirs(&parsed.exclude_dir);
    let exclude_lower: Vec<String> = parsed.exclude.iter()
        .map(|s| s.to_lowercase())
        .collect();
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
        exclude_patterns,
        exclude_lower,
        dir_auto_converted_note: parsed.dir_auto_converted_note.clone(),
        auto_balance: parsed.auto_balance,
        max_occurrences_per_term: parsed.max_occurrences_per_term,
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

    // --- Line-based regex mode (supports `^`, `$`, whitespace, non-token chars)
    if parsed.use_line_regex {
        let mut result = handle_line_regex_search(ctx, &index, &parsed.terms_str, &grep_params);
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

    // Warn when regex=true and pattern uses constructs incompatible with token-based regex.
    // Token regex matches against individual index tokens (alphanumeric+underscore, no whitespace),
    // so anchors `^`/`$` (anchored to token boundaries, not lines) and spaces never match.
    let pattern_has_spaces = parsed.terms_str.contains(' ');
    let pattern_has_anchors = parsed.terms_str.contains('^') || parsed.terms_str.contains('$');
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
                let note = if has_punctuation {
                    format!("{} — auto-switched to phrase search (~100x slower). \
                     Tip: use last segment only for faster substring search \
                     (e.g., 'SqlClient' instead of 'System.Data.SqlClient', \
                     'Blobs' instead of 'Azure.Storage.Blobs')", reason)
                } else {
                    format!("{} — auto-switched to phrase search \
                     (substring mode operates on individual tokens which only contain \
                     alphanumeric characters and underscores)", reason)
                };
                summary["searchModeNote"] = serde_json::Value::String(note);
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

    if params.count_only {
        let mut summary = build_grep_base_summary(
            total_files, total_occurrences, &json!(raw_terms),
            &format!("substring-{}", search_mode), index, search_start.elapsed(), ctx, false,
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
    if let Some(ab) = auto_balance_info {
        inject_auto_balance(&mut summary, ab);
    }
    apply_dir_auto_converted_note(&mut summary, params);
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
    let mut results = match collect_phrase_matches(index, phrase, params) {
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

    if count_only {
        let mut summary = build_grep_base_summary(
            total_files, total_occurrences, &json!([phrase]), "phrase",
            index, search_elapsed, ctx, true,
        );
        apply_dir_auto_converted_note(&mut summary, params);
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

    let mut summary = build_grep_base_summary(
        total_files, total_occurrences, &json!([phrase]), "phrase",
        index, search_elapsed, ctx, true,
    );
    apply_dir_auto_converted_note(&mut summary, params);
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
    let mut per_token_file_lines: Vec<HashMap<u32, Vec<u32>>> =
        Vec::with_capacity(phrase_tokens.len());
    for token in &phrase_tokens {
        let postings = match index.index.get(token.as_str()) {
            Some(p) => p,
            // A token has no postings at all -> intersection is empty -> no matches.
            None => return Ok(Vec::new()),
        };
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
        }
        if map.is_empty() {
            return Ok(Vec::new());
        }
        per_token_file_lines.push(map);
    }

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

    Ok(results)
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

    if count_only {
        let mut summary = build_grep_base_summary(
            total_files, total_occurrences, &json!(searched_terms), search_mode,
            index, search_elapsed, ctx, true,
        );
        apply_dir_auto_converted_note(&mut summary, params);
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

    let mut summary = build_grep_base_summary(
        total_files, total_occurrences, &json!(searched_terms), search_mode,
        index, search_elapsed, ctx, true,
    );
    apply_dir_auto_converted_note(&mut summary, params);
    let output = json!({
        "files": files_json,
        "summary": summary
    });

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
    terms_str: &str,
    params: &GrepSearchParams,
) -> ToolCallResult {
    // Parse comma-separated patterns. Unlike token regex, we do NOT lowercase —
    // user-supplied regex flags (e.g., `(?i)`) control case sensitivity. We also
    // do NOT trim each pattern, because whitespace inside a regex is significant
    // (e.g., `^## ` matches markdown level-2 headings only, NOT `^##` which would
    // also match `^### `). Users wanting comma-with-padding (`a, b`) should not
    // include leading spaces — or should escape them via `\s`/`[ ]`.
    let patterns: Vec<String> = terms_str
        .split(',')
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if patterns.is_empty() {
        return ToolCallResult::error("No search patterns provided".to_string());
    }

    // Compile all patterns up-front with multi_line=true so `^` and `$` anchor
    // to line boundaries (not input boundaries). Without this, `^foo` would only
    // match at the very start of the file content, breaking any anchor-based
    // search on multi-line files. user-supplied flags like `(?m)`/`(?s)` still
    // override our defaults.
    let mut compiled: Vec<regex::Regex> = Vec::with_capacity(patterns.len());
    for pat in &patterns {
        match regex::RegexBuilder::new(pat).multi_line(true).build() {
            Ok(re) => compiled.push(re),
            Err(e) => return ToolCallResult::error(format!("Invalid regex '{}': {}", pat, e)),
        }
    }

    // Iterate ALL indexed files, apply file/ext/dir filters, then run line regex.
    // No token pre-filter: regex with anchors/whitespace cannot be reduced to a
    // safe literal substring without a regex AST analyzer (would risk false
    // negatives). Filters keep the candidate set manageable in practice.
    let mut per_pattern_matches: Vec<HashMap<String, Vec<u32>>> = vec![HashMap::new(); patterns.len()];
    let mut content_cache: HashMap<String, String> = HashMap::new();

    // MINOR-23: cap total bytes held in `content_cache` so a single broad query
    // (e.g. lineRegex='.*' with showLines=true on a large repo) cannot OOM the
    // server. When the cap is exceeded, matches are still recorded — only the
    // `lineContent` previews for files inserted past the cap are dropped, and
    // the response surfaces a `lineContentTruncated` hint in the summary.
    // The cap is lowered for `cfg(test)` so a regression test can exercise the
    // truncation branch without allocating real megabytes of fixtures.
    #[cfg(not(test))]
    const MAX_CONTENT_CACHE_BYTES: usize = 256 * 1024 * 1024; // 256 MiB
    #[cfg(test)]
    const MAX_CONTENT_CACHE_BYTES: usize = 4 * 1024; // 4 KiB
    let mut cache_bytes_used: usize = 0;
    let mut line_content_truncated = false;

    for file_path in &index.files {
        if !passes_file_filters(file_path, params) {
            continue;
        }

        // Read file once per candidate; cache for show_lines reuse.
        let content = match read_file_lossy(std::path::Path::new(file_path)) {
            Ok((c, _lossy)) => c,
            Err(_) => continue,
        };

        // Pre-check: does ANY pattern match anywhere in the file?
        // This is a fast-rejection optimization for files that don't match any pattern.
        let any_pattern_matches = compiled.iter().any(|re| re.is_match(&content));
        if !any_pattern_matches {
            continue;
        }

        let mut matched_any_pattern = false;
        for (pat_idx, re) in compiled.iter().enumerate() {
            let mut matching_lines: Vec<u32> = Vec::new();
            for (line_num, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    matching_lines.push((line_num + 1) as u32);
                }
            }
            if !matching_lines.is_empty() {
                per_pattern_matches[pat_idx].insert(file_path.clone(), matching_lines);
                matched_any_pattern = true;
            }
        }

        if matched_any_pattern && params.show_lines {
            // Reserve cache budget before insertion. If this file would push us
            // past the cap, skip caching its content — matched line numbers are
            // still emitted, only the source preview is dropped (and we set the
            // `lineContentTruncated` flag so the client knows previews are
            // partial). Files already cached stay cached; cap is monotone.
            if cache_bytes_used.saturating_add(content.len()) > MAX_CONTENT_CACHE_BYTES {
                line_content_truncated = true;
            } else {
                cache_bytes_used = cache_bytes_used.saturating_add(content.len());
                content_cache.insert(file_path.clone(), content);
            }
        }
    }

    // Merge per-pattern matches with OR or AND semantics.
    let merged_files: HashMap<String, Vec<u32>> = if params.mode_and {
        // Files appearing in ALL pattern result sets.
        let mut common: Option<HashSet<String>> = None;
        for pm in &per_pattern_matches {
            let files: HashSet<String> = pm.keys().cloned().collect();
            common = Some(match common {
                None => files,
                Some(prev) => prev.intersection(&files).cloned().collect(),
            });
        }
        let common = common.unwrap_or_default();
        let mut merged: HashMap<String, Vec<u32>> = HashMap::new();
        for pm in &per_pattern_matches {
            for (path, lines) in pm {
                if common.contains(path) {
                    merged.entry(path.clone()).or_default().extend_from_slice(lines);
                }
            }
        }
        merged
    } else {
        // OR: union of all files.
        let mut merged: HashMap<String, Vec<u32>> = HashMap::new();
        for pm in &per_pattern_matches {
            for (path, lines) in pm {
                merged.entry(path.clone()).or_default().extend_from_slice(lines);
            }
        }
        merged
    };

    // Sort/dedup line numbers per file.
    let mut results: Vec<PhraseFileMatch> = merged_files.into_iter()
        .map(|(file_path, mut lines)| {
            lines.sort();
            lines.dedup();
            let content = content_cache.remove(&file_path);
            PhraseFileMatch { file_path, lines, content }
        })
        .collect();

    let total_files = results.len();
    let total_occurrences: usize = results.iter().map(|r| r.lines.len()).sum();

    // Sort by occurrences descending (most matches first), like phrase search.
    // Tie-break by file path ascending for deterministic truncated output.
    results.sort_by(|a, b| {
        b.lines.len()
            .cmp(&a.lines.len())
            .then_with(|| a.file_path.cmp(&b.file_path))
    });

    if params.max_results > 0 {
        results.truncate(params.max_results);
    }

    let search_elapsed = params.search_start.elapsed();
    let search_mode = if params.mode_and { "lineRegex-and" } else { "lineRegex" };

    if params.count_only {
        let mut summary = build_grep_base_summary(
            total_files, total_occurrences, &json!(patterns), search_mode,
            index, search_elapsed, ctx, true,
        );
        apply_dir_auto_converted_note(&mut summary, params);
        let output = json!({ "summary": summary });
        return ToolCallResult::success(json_to_string(&output));
    }

    let files_json: Vec<Value> = results.iter().map(|r| {
        let mut file_obj = json!({
            "path": r.file_path,
            "occurrences": r.lines.len(),
            "lines": r.lines,
        });
        if params.show_lines
            && let Some(ref content) = r.content {
                file_obj["lineContent"] = build_line_content_from_matches(content, &r.lines, params.context_lines);
            }
        file_obj
    }).collect();

    let mut summary = build_grep_base_summary(
        total_files, total_occurrences, &json!(patterns), search_mode,
        index, search_elapsed, ctx, true,
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
                    MAX_CONTENT_CACHE_BYTES / (1024 * 1024)
                )),
            );
        }
    }
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
