//! CLI layer: argument parsing, command dispatch, and subcommand implementations.

pub mod args;
mod info;
mod serve;

pub use args::*;
pub use info::cmd_info_json;

use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

use regex::Regex;

use crate::{
    build_content_index, build_index, cleanup_indexes_for_dir, cleanup_orphaned_indexes,
    content_index_path_for, find_content_index_for_dir,
    index_dir, index_path_for, load_content_index, load_index,
    save_content_index, save_index, tokenize,
    SearchError, DEFAULT_MIN_TOKEN_LEN,
};
use crate::definitions;

// ─── CLI ─────────────────────────────────────────────────────────────

/// High-performance code search engine with inverted indexing and AST-based code intelligence
#[derive(Parser, Debug)]
#[command(
    name = "xray",
    version,
    author = "Sergey Pustynsky",
    long_version = concat!(
        env!("CARGO_PKG_VERSION"), " (built ", env!("BUILD_DATETIME"), ")\n",
        "Author: Sergey Pustynsky\n",
        "License: MIT OR Apache-2.0"
    ),
    about,
    after_help = "\
Run 'xray <COMMAND> --help' for detailed options and examples.\n\
Common options: -d <DIR> (directory), -e <EXT> (extension filter), -c (count only)"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    /// Build a file index for a directory
    Index(IndexArgs),

    /// Search using a pre-built index (instant results)
    Fast(FastArgs),

    /// Show index info or list indexed directories
    Info,

    /// Build an inverted (content) index for text/code files
    ContentIndex(ContentIndexArgs),

    /// Search file contents using inverted index (instant grep).
    Grep(GrepArgs),

    /// Start MCP (Model Context Protocol) server over stdio.
    Serve(ServeArgs),

    /// Build a code definition index (classes, methods, interfaces, etc.)
    DefIndex(definitions::DefIndexArgs),

    /// Audit definition index coverage (load from disk, no rebuild)
    DefAudit(definitions::DefAuditArgs),

    /// Remove orphaned index files, or indexes for a specific directory.
    Cleanup(CleanupArgs),

    /// Show best practices and tips.
    Tips,

    /// Create a stale content index with format_version=0 (for E2E testing).
    #[command(hide = true)]
    TestCreateStaleIndex(TestCreateStaleIndexArgs),
}

// ─── Main entry point ───────────────────────────────────────────────

pub fn run() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Index(args) => cmd_index(args),
        Commands::Fast(args) => cmd_fast(args),
        Commands::Info => { info::cmd_info(); Ok(()) },
        Commands::ContentIndex(args) => cmd_content_index(args),
        Commands::Grep(args) => cmd_grep(args),
        Commands::Serve(args) => { serve::cmd_serve(args); Ok(()) },
        Commands::DefIndex(args) => cmd_def_index(args),
        Commands::DefAudit(args) => cmd_def_audit(args),
        Commands::Cleanup(args) => {
            let idx_base = index_dir();
            if let Some(ref dir) = args.dir {
                eprintln!("Removing indexes for directory '{}' from {}...", dir, idx_base.display());
                let removed = cleanup_indexes_for_dir(dir, &idx_base);
                if removed == 0 {
                    eprintln!("No indexes found for '{}'.", dir);
                } else {
                    eprintln!("Removed {} index file(s) for '{}'.", removed, dir);
                }
            } else {
                eprintln!("Scanning for orphaned indexes in {}...", idx_base.display());
                let removed = cleanup_orphaned_indexes(&idx_base);
                if removed == 0 {
                    eprintln!("No orphaned indexes found.");
                } else {
                    eprintln!("Removed {} orphaned index file(s).", removed);
                }
            }
            Ok(())
        },
        Commands::Tips => {
            let all_exts: Vec<String> = crate::definitions::definition_extensions()
                .iter().map(|s| s.to_string()).collect();
            print!("{}", crate::tips::render_cli(&all_exts));
            Ok(())
        },
        Commands::TestCreateStaleIndex(args) => cmd_test_create_stale_index(args),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

// ─── Small commands ─────────────────────────────────────────────────

/// Hidden test helper: build a content index then overwrite format_version.
fn cmd_test_create_stale_index(args: TestCreateStaleIndexArgs) -> Result<(), SearchError> {
    let idx_base = index_dir();
    // Build a real content index first
    let mut idx = build_content_index(&ContentIndexArgs {
        dir: args.dir.clone(),
        ext: args.ext.clone(),
        max_age_hours: 24,
        hidden: false,
        no_ignore: false,
        threads: 0,
        min_token_len: 2,
    })?;
    // Override format_version to simulate stale/old index
    idx.format_version = args.version;
    save_content_index(&idx, &idx_base)?;
    let exts_str = idx.extensions.join(",");
    let path = content_index_path_for(&idx.root, &exts_str, &idx_base);
    eprintln!("Created stale content index (version={}) at {}", args.version, path.display());
    Ok(())
}

fn cmd_index(args: IndexArgs) -> Result<(), SearchError> {
    let idx_base = index_dir();
    let index = build_index(&args)?;
    save_index(&index, &idx_base)?;
    let path = index_path_for(&args.dir, &idx_base);
    let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "Index saved to {} ({:.1} MB)",
        path.display(),
        size as f64 / 1_048_576.0
    );
    Ok(())
}

fn cmd_content_index(args: ContentIndexArgs) -> Result<(), SearchError> {
    let idx_base = index_dir();
    let exts_str = args.ext.clone();
    let index = build_content_index(&args)?;
    save_content_index(&index, &idx_base)?;
    let path = content_index_path_for(&args.dir, &exts_str, &idx_base);
    let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "Content index saved to {} ({:.1} MB)",
        path.display(),
        size as f64 / 1_048_576.0
    );
    Ok(())
}

fn cmd_def_index(args: definitions::DefIndexArgs) -> Result<(), SearchError> {
    let idx_base = index_dir();
    let index = definitions::build_definition_index(&args);
    definitions::save_definition_index(&index, &idx_base)?;
    eprintln!("[def-index] Done! {} definitions from {} files",
        index.definitions.len(), index.files.len());
    Ok(())
}

fn cmd_def_audit(args: definitions::DefAuditArgs) -> Result<(), SearchError> {
    let idx_base = index_dir();
    let exts = args.ext.split(',').map(|s| s.trim().to_lowercase()).collect::<Vec<_>>().join(",");

    let index = match definitions::load_definition_index(&args.dir, &exts, &idx_base) {
        Ok(idx) => idx,
        Err(_) => {
            eprintln!("[def-audit] No definition index found for dir='{}' ext='{}'. Run 'xray def-index' first.", args.dir, args.ext);
            return Ok(());
        }
    };

    let files_with_defs = index.file_index.len();
    let total_files = index.files.len();
    let files_without_defs = index.empty_file_ids.len();

    eprintln!("[def-audit] Index: {} total files, {} with definitions, {} without definitions",
        total_files, files_with_defs, files_without_defs);
    eprintln!("[def-audit] {} definitions, {} read errors, {} lossy-UTF8 files",
        index.definitions.len(), index.parse_errors, index.lossy_file_count);

    let suspicious: Vec<_> = index.empty_file_ids.iter()
        .filter(|(_, size)| *size > args.min_bytes)
        .collect();

    if suspicious.is_empty() {
        eprintln!("[def-audit] No suspicious files (all files >{}B have definitions). ✓", args.min_bytes);
    } else {
        eprintln!("[def-audit] {} suspicious files (>{}B with 0 definitions):",
            suspicious.len(), args.min_bytes);
        for (fid, size) in &suspicious {
            let path = index.files.get(*fid as usize).map(|s| s.as_str()).unwrap_or("?");
            eprintln!("  {} ({} bytes)", path, size);
        }
    }

    if args.show_lossy && index.lossy_file_count > 0 {
        eprintln!("\n[def-audit] Files with lossy UTF-8 conversion:");
        // We don't store the list of lossy paths in the index (only count),
        // so we can't enumerate them here. Re-run def-index to see warnings.
        eprintln!("  (lossy file paths are logged during def-index build, not stored in index)");
        eprintln!("  Re-run: xray def-index --dir {} --ext {} 2>&1 | findstr /i \"lossy\"", args.dir, args.ext);
    }

    Ok(())
}





// ─── cmd_fast ───────────────────────────────────────────────────────

fn cmd_fast(args: FastArgs) -> Result<(), SearchError> {
    if args.pattern.trim().is_empty() {
        return Err(SearchError::InvalidArgs("Pattern must not be empty.".to_string()));
    }

    let start = Instant::now();
    let idx_base = index_dir();

    let index = match load_index(&args.dir, &idx_base) {
        Ok(idx) => {
            if idx.is_stale() && args.auto_reindex {
                eprintln!("Index is stale, rebuilding...");
                let new_index = build_index(&IndexArgs {
                    dir: args.dir.clone(), max_age_hours: idx.max_age_secs / 3600,
                    hidden: false, no_ignore: false, threads: 0,
                })?;
                if let Err(e) = save_index(&new_index, &idx_base) {
                    eprintln!("Warning: failed to save updated index: {}", e);
                }
                new_index
            } else {
                if idx.is_stale() {
                    eprintln!("Warning: index is stale (use 'xray index -d {}' to rebuild)", args.dir);
                }
                idx
            }
        }
        Err(_) => {
            eprintln!("No index found for '{}'. Building one now...", args.dir);
            let new_index = build_index(&IndexArgs {
                dir: args.dir.clone(), max_age_hours: 24,
                hidden: false, no_ignore: false, threads: 0,
            })?;
            if let Err(e) = save_index(&new_index, &idx_base) {
                eprintln!("Warning: failed to save index: {}", e);
            }
            new_index
        }
    };

    let load_elapsed = start.elapsed();

    let pattern = if args.ignore_case { args.pattern.to_lowercase() } else { args.pattern.clone() };

    let re = if args.regex {
        let pat = if args.ignore_case { format!("(?i){}", &args.pattern) } else { args.pattern.clone() };
        match Regex::new(&pat) {
            Ok(r) => Some(r),
            Err(e) => return Err(SearchError::InvalidRegex { pattern: pat, source: e }),
        }
    } else {
        None
    };

    let search_start = Instant::now();
    let mut match_count = 0usize;

    for entry in &index.entries {
        if args.dirs_only && !entry.is_dir { continue; }
        if args.files_only && entry.is_dir { continue; }
        if let Some(min) = args.min_size && entry.size < min { continue; }
        if let Some(max) = args.max_size && entry.size > max { continue; }
        if let Some(ref ext) = args.ext {
            let matches_ext = Path::new(&entry.path).extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case(ext));
            if !matches_ext { continue; }
        }

        let name = Path::new(&entry.path).file_name().and_then(|n| n.to_str()).unwrap_or("");
        let search_name = if args.ignore_case { name.to_lowercase() } else { name.to_string() };

        let matched = if let Some(ref re) = re { re.is_match(&search_name) } else { search_name.contains(&pattern) };
        if matched {
            match_count += 1;
            if !args.count {
                if entry.is_dir { println!("[DIR]  {}", entry.path); }
                else { println!("       {}", entry.path); }
            }
        }
    }

    let search_elapsed = search_start.elapsed();
    let total_elapsed = start.elapsed();
    eprintln!("\n{} matches found among {} indexed entries", match_count, index.entries.len());
    eprintln!("Index load: {:.3}s | Search: {:.6}s | Total: {:.3}s",
        load_elapsed.as_secs_f64(), search_elapsed.as_secs_f64(), total_elapsed.as_secs_f64());
    Ok(())
}

// ─── cmd_grep helpers ───────────────────────────────────────────────

use crate::ContentIndex;
use std::time::Duration;

/// Check if a file path passes extension + exclude_dir + exclude filters.
fn file_matches_filters(
    file_path: &str,
    ext: &Option<String>,
    exclude_dir: &[String],
    exclude: &[String],
) -> bool {
    if let Some(ext) = ext {
        let m = Path::new(file_path).extension().and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case(ext));
        if !m { return false; }
    }
    if exclude_dir.iter().any(|excl| file_path.to_lowercase().contains(&excl.to_lowercase())) { return false; }
    if exclude.iter().any(|excl| file_path.to_lowercase().contains(&excl.to_lowercase())) { return false; }
    true
}

/// Display a single file's matching lines with context (shared by phrase and token search).
fn display_lines_with_context(
    file_path: &str,
    match_line_numbers: &[u32],
    ctx_before: usize,
    ctx_after: usize,
) {
    if let Ok(content) = fs::read_to_string(file_path) {
        let lines_vec: Vec<&str> = content.lines().collect();
        let total_lines = lines_vec.len();
        let mut lines_to_show: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        let mut match_lines_set: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for &ln in match_line_numbers {
            let idx = (ln as usize).saturating_sub(1);
            if idx < total_lines {
                match_lines_set.insert(idx);
                let s = idx.saturating_sub(ctx_before);
                let e = (idx + ctx_after).min(total_lines - 1);
                for i in s..=e { lines_to_show.insert(i); }
            }
        }
        let mut prev: Option<usize> = None;
        for &idx in &lines_to_show {
            if let Some(p) = prev && idx > p + 1 { println!("--"); }
            let marker = if match_lines_set.contains(&idx) { ">" } else { " " };
            println!("{}{}:{}: {}", marker, file_path, idx + 1, lines_vec[idx]);
            prev = Some(idx);
        }
        if !lines_to_show.is_empty() { println!(); }
    }
}

/// Load content index, handle stale/missing/auto-reindex.
fn load_grep_index(
    dir: &str,
    ext: &Option<String>,
    auto_reindex: bool,
) -> Result<(ContentIndex, Duration), SearchError> {
    let start = Instant::now();
    let idx_base = index_dir();
    let exts_for_load = ext.clone().unwrap_or_default();

    let index = match load_content_index(dir, &exts_for_load, &idx_base) {
        Ok(idx) => {
            if idx.is_stale() && auto_reindex {
                eprintln!("Content index is stale, rebuilding...");
                let ext_str = idx.extensions.join(",");
                let new_idx = build_content_index(&ContentIndexArgs {
                    dir: dir.to_string(), ext: ext_str, max_age_hours: idx.max_age_secs / 3600,
                    hidden: false, no_ignore: false, threads: 0, min_token_len: DEFAULT_MIN_TOKEN_LEN,
                })?;
                let _ = save_content_index(&new_idx, &idx_base);
                new_idx
            } else {
                if idx.is_stale() { eprintln!("Warning: content index is stale"); }
                idx
            }
        }
        Err(_) => {
            match find_content_index_for_dir(dir, &idx_base, &[]) {
                Some(idx) => idx,
                None => return Err(SearchError::IndexNotFound { dir: dir.to_string() }),
            }
        }
    };

    let load_elapsed = start.elapsed();
    Ok((index, load_elapsed))
}

/// Find file IDs that contain all phrase tokens via inverted index intersection.
fn find_phrase_candidates(
    index: &ContentIndex,
    phrase_tokens: &[String],
    ext: &Option<String>,
    exclude_dir: &[String],
    exclude: &[String],
) -> std::collections::HashSet<u32> {
    let mut candidate_file_ids: Option<std::collections::HashSet<u32>> = None;
    for token in phrase_tokens {
        if let Some(postings) = index.index.get(token.as_str()) {
            let file_ids: std::collections::HashSet<u32> = postings.iter()
                .filter(|p| {
                    let path = match index.files.get(p.file_id as usize) {
                        Some(p) => p,
                        None => return false,
                    };
                    file_matches_filters(path, ext, exclude_dir, exclude)
                })
                .map(|p| p.file_id).collect();
            candidate_file_ids = Some(match candidate_file_ids {
                Some(existing) => existing.intersection(&file_ids).cloned().collect(),
                None => file_ids,
            });
        } else {
            candidate_file_ids = Some(std::collections::HashSet::new());
            break;
        }
    }
    candidate_file_ids.unwrap_or_default()
}

struct PhraseMatch { file_path: String, lines: Vec<u32> }

/// Read candidate files from disk and verify phrase regex matches.
fn verify_phrase_matches(
    index: &ContentIndex,
    candidates: &std::collections::HashSet<u32>,
    phrase_re: &Regex,
) -> Vec<PhraseMatch> {
    let mut results: Vec<PhraseMatch> = Vec::new();
    for &file_id in candidates {
        let file_path = match index.files.get(file_id as usize) {
            Some(p) => p,
            None => continue,
        };
        if let Ok(content) = fs::read_to_string(file_path) && phrase_re.is_match(&content) {
            let mut matching_lines = Vec::new();
            for (line_num, line) in content.lines().enumerate() {
                if phrase_re.is_match(line) { matching_lines.push((line_num + 1) as u32); }
            }
            if !matching_lines.is_empty() {
                results.push(PhraseMatch { file_path: file_path.clone(), lines: matching_lines });
            }
        }
    }
    results
}

/// Handle the entire phrase search branch (early return path).
fn cmd_grep_phrase(
    index: &ContentIndex,
    args: &GrepArgs,
    load_elapsed: Duration,
) -> Result<(), SearchError> {
    let search_start = Instant::now();

    let phrase = &args.pattern;
    let phrase_lower = phrase.to_lowercase();
    let phrase_tokens = tokenize(&phrase_lower, DEFAULT_MIN_TOKEN_LEN);
    if phrase_tokens.is_empty() {
        return Err(SearchError::EmptyPhrase { phrase: phrase.to_string() });
    }

    let phrase_regex_pattern = phrase_tokens.iter()
        .map(|t| regex::escape(t)).collect::<Vec<_>>().join(r"\s+");
    let phrase_re_pat = format!("(?i){}", phrase_regex_pattern);
    let phrase_re = match Regex::new(&phrase_re_pat) {
        Ok(r) => r,
        Err(e) => return Err(SearchError::InvalidRegex { pattern: phrase_re_pat, source: e }),
    };

    eprintln!("Phrase search: '{}' -> tokens: {:?} -> regex: {}", phrase, phrase_tokens, phrase_regex_pattern);

    let candidates = find_phrase_candidates(index, &phrase_tokens, &args.ext, &args.exclude_dir, &args.exclude);
    eprintln!("Found {} candidate files containing all tokens", candidates.len());

    let results = verify_phrase_matches(index, &candidates, &phrase_re);

    let search_elapsed = search_start.elapsed();
    let total_elapsed = load_elapsed + search_elapsed;
    let match_count = results.len();
    let line_count: usize = results.iter().map(|r| r.lines.len()).sum();

    let display_results = if args.max_results > 0 { &results[..results.len().min(args.max_results)] } else { &results };
    let ctx_before = if args.context > 0 { args.context } else { args.before };
    let ctx_after = if args.context > 0 { args.context } else { args.after };

    if !args.count {
        for result in display_results {
            if args.show_lines {
                display_lines_with_context(&result.file_path, &result.lines, ctx_before, ctx_after);
            } else {
                println!("{} ({} matches, lines: {})", result.file_path, result.lines.len(),
                    result.lines.iter().take(10).map(|n| n.to_string()).collect::<Vec<_>>().join(", "));
            }
        }
    }

    eprintln!("\n{} files, {} lines matching phrase '{}' (candidates: {}, index: {} files)",
        match_count, line_count, phrase, candidates.len(), index.files.len());
    eprintln!("Index load: {:.3}s | Search+Verify: {:.6}s | Total: {:.3}s",
        load_elapsed.as_secs_f64(), search_elapsed.as_secs_f64(), total_elapsed.as_secs_f64());
    Ok(())
}

/// Expand terms using trigram index for substring matching.
fn expand_substring_terms(
    raw_terms: &[String],
    trigram_idx: &crate::TrigramIndex,
) -> Vec<String> {
    let mut expanded = Vec::new();
    for term in raw_terms {
        let matched_tokens: Vec<String> = if term.len() < 3 {
            // Linear scan for very short terms (no trigrams possible)
            trigram_idx.tokens.iter()
                .filter(|tok| tok.contains(term.as_str()))
                .cloned()
                .collect()
        } else {
            let trigrams = code_xray::generate_trigrams(term);
            if trigrams.is_empty() {
                Vec::new()
            } else {
                let mut candidates: Option<Vec<u32>> = None;
                for tri in &trigrams {
                    if let Some(posting_list) = trigram_idx.trigram_map.get(tri) {
                        candidates = Some(match candidates {
                            None => posting_list.clone(),
                            Some(prev) => crate::mcp::handlers::utils::sorted_intersect(&prev, posting_list),
                        });
                    } else {
                        candidates = Some(Vec::new());
                        break;
                    }
                }
                candidates.unwrap_or_default().into_iter()
                    .filter_map(|idx| trigram_idx.tokens.get(idx as usize))
                    .filter(|tok| tok.contains(term.as_str()))
                    .cloned()
                    .collect()
            }
        };
        if matched_tokens.is_empty() {
            eprintln!("Warning: substring '{}' matched 0 tokens", term);
        } else {
            eprintln!("Substring '{}' matched {} tokens: {}", term, matched_tokens.len(),
                matched_tokens.iter().take(10).cloned().collect::<Vec<_>>().join(", "));
        }
        expanded.extend(matched_tokens);
    }
    expanded
}

/// Expand terms using regex pattern matching against all indexed tokens.
fn expand_regex_terms(
    raw_terms: &[String],
    index_keys: &HashMap<String, Vec<crate::Posting>>,
) -> Result<Vec<String>, SearchError> {
    let mut expanded = Vec::new();
    for pat in raw_terms {
        match Regex::new(&format!("(?i)^{}$", pat)) {
            Ok(re) => {
                let matching: Vec<String> = index_keys.keys().filter(|k| re.is_match(k)).cloned().collect();
                if matching.is_empty() { eprintln!("Warning: regex '{}' matched 0 tokens", pat); }
                else { eprintln!("Regex '{}' matched {} tokens", pat, matching.len()); }
                expanded.extend(matching);
            }
            Err(e) => return Err(SearchError::InvalidRegex { pattern: pat.clone(), source: e }),
        }
    }
    Ok(expanded)
}

/// Expand raw comma-separated search terms based on search mode (substring/regex/exact).
fn expand_grep_terms(
    raw_terms: &[String],
    index: &ContentIndex,
    use_substring: bool,
    use_regex: bool,
) -> Result<Vec<String>, SearchError> {
    if use_substring {
        Ok(expand_substring_terms(raw_terms, &index.trigram))
    } else if use_regex {
        expand_regex_terms(raw_terms, &index.index)
    } else {
        Ok(raw_terms.to_vec())
    }
}

struct FileScore { file_path: String, lines: Vec<u32>, tf_idf: f64, occurrences: usize, terms_matched: usize }

/// Compute TF-IDF scores per file for expanded terms.
fn score_grep_results(
    index: &ContentIndex,
    terms: &[String],
    ext: &Option<String>,
    exclude_dir: &[String],
    exclude: &[String],
    require_all: bool,
    raw_term_count: usize,
) -> Vec<FileScore> {
    let total_docs = index.files.len() as f64;
    let mut file_scores: HashMap<u32, FileScore> = HashMap::new();

    for term in terms {
        if let Some(postings) = index.index.get(term.as_str()) {
            let doc_freq = postings.len() as f64;
            let idf = (total_docs / doc_freq).ln();
            for posting in postings {
                let file_path = match index.files.get(posting.file_id as usize) {
                    Some(p) => p,
                    None => continue,
                };
                if !file_matches_filters(file_path, ext, exclude_dir, exclude) { continue; }

                let occurrences = posting.lines.len();
                let file_total = if (posting.file_id as usize) < index.file_token_counts.len() {
                    index.file_token_counts[posting.file_id as usize] as f64
                } else { 1.0 };
                let tf = occurrences as f64 / file_total;
                let tf_idf = tf * idf;

                let entry = file_scores.entry(posting.file_id).or_insert(FileScore {
                    file_path: file_path.clone(), lines: Vec::new(), tf_idf: 0.0, occurrences: 0, terms_matched: 0,
                });
                entry.tf_idf += tf_idf;
                entry.occurrences += occurrences;
                entry.lines.extend_from_slice(&posting.lines);
                entry.terms_matched += 1;
            }
        }
    }

    let mut results: Vec<FileScore> = file_scores.into_values()
        .filter(|fs| !require_all || fs.terms_matched >= raw_term_count).collect();

    for result in &mut results { result.lines.sort(); result.lines.dedup(); }
    results.sort_by(|a, b| b.tf_idf.partial_cmp(&a.tf_idf).unwrap_or(std::cmp::Ordering::Equal));

    results
}

// ─── cmd_grep ───────────────────────────────────────────────────────

fn cmd_grep(args: GrepArgs) -> Result<(), SearchError> {
    let start = Instant::now();
    let (index, load_elapsed) = load_grep_index(&args.dir, &args.ext, args.auto_reindex)?;
    let search_start = Instant::now();

    // Default: substring search (like MCP). Disabled by --exact, --regex, or --phrase.
    let use_substring = !args.exact && !args.regex && !args.phrase;

    // Phrase search: separate code path with early return
    if args.phrase {
        return cmd_grep_phrase(&index, &args, load_elapsed);
    }

    // ─── Normal token search ────────────────────────────────
    let raw_terms: Vec<String> = args.pattern.split(',')
        .map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();

    let terms = expand_grep_terms(&raw_terms, &index, use_substring, args.regex)?;

    let mode_str = if use_substring { if args.all { "SUBSTRING-AND" } else { "SUBSTRING-OR" } }
        else if args.regex { "REGEX" } else if args.all { "AND" } else { "OR" };

    let raw_term_count = if args.regex || use_substring { raw_terms.len() } else { terms.len() };
    let results = score_grep_results(
        &index, &terms, &args.ext, &args.exclude_dir, &args.exclude,
        args.all, raw_term_count,
    );

    let match_count = results.len();
    let line_count: usize = results.iter().map(|r| r.lines.len()).sum();
    let max = args.max_results;
    let display_results = if max > 0 { &results[..results.len().min(max)] } else { &results };

    let ctx_before = if args.context > 0 { args.context } else { args.before };
    let ctx_after = if args.context > 0 { args.context } else { args.after };

    if !args.count {
        for result in display_results {
            if args.show_lines {
                display_lines_with_context(&result.file_path, &result.lines, ctx_before, ctx_after);
            } else {
                println!("[{:.4}] {} ({} occurrences, {}/{} terms, lines: {})",
                    result.tf_idf, result.file_path, result.occurrences,
                    result.terms_matched, terms.len(),
                    result.lines.iter().take(10).map(|n| n.to_string()).collect::<Vec<_>>().join(", "));
            }
        }
    }

    let search_elapsed = search_start.elapsed();
    let total_elapsed = start.elapsed();
    eprintln!("\n{} files, {} occurrences matching {} terms [{}]: '{}' (index: {} files, {} unique tokens)",
        match_count, line_count, terms.len(), mode_str, args.pattern, index.files.len(), index.index.len());
    eprintln!("Index load: {:.3}s | Search+Rank: {:.6}s | Total: {:.3}s",
        load_elapsed.as_secs_f64(), search_elapsed.as_secs_f64(), total_elapsed.as_secs_f64());
    Ok(())
}

// ─── Tests for extracted cmd_grep helpers ────────────────────────────

#[cfg(test)]
#[path = "cli_tests.rs"]
mod grep_helper_tests;
