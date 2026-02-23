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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use ignore::WalkBuilder;
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
    name = "search-index",
    version,
    author = "Sergey Pustynsky",
    long_version = concat!(
        env!("CARGO_PKG_VERSION"), " (built ", env!("BUILD_DATETIME"), ")\n",
        "Author: Sergey Pustynsky\n",
        "License: MIT OR Apache-2.0"
    ),
    about,
    after_help = "\
Run 'search-index <COMMAND> --help' for detailed options and examples.\n\
Common options: -d <DIR> (directory), -e <EXT> (extension filter), -c (count only)"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    /// Search for files (live filesystem walk)
    Find(FindArgs),

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
}

// ─── Main entry point ───────────────────────────────────────────────

pub fn run() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Find(args) => cmd_find(args),
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
        Commands::Tips => { print!("{}", crate::tips::render_cli()); Ok(()) },
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

// ─── Small commands ─────────────────────────────────────────────────

fn cmd_index(args: IndexArgs) -> Result<(), SearchError> {
    let idx_base = index_dir();
    let index = build_index(&args);
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
    let index = build_content_index(&args);
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
            eprintln!("[def-audit] No definition index found for dir='{}' ext='{}'. Run 'search-index def-index' first.", args.dir, args.ext);
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
        eprintln!("  Re-run: search-index def-index --dir {} --ext {} 2>&1 | findstr /i \"lossy\"", args.dir, args.ext);
    }

    Ok(())
}

// ─── cmd_find ───────────────────────────────────────────────────────

fn cmd_find(args: FindArgs) -> Result<(), SearchError> {
    let start = Instant::now();

    let pattern = if args.ignore_case {
        args.pattern.to_lowercase()
    } else {
        args.pattern.clone()
    };

    let re = if args.regex {
        let pat = if args.ignore_case {
            format!("(?i){}", &args.pattern)
        } else {
            args.pattern.clone()
        };
        match Regex::new(&pat) {
            Ok(r) => Some(r),
            Err(e) => {
                return Err(SearchError::InvalidRegex { pattern: pat, source: e });
            }
        }
    } else {
        None
    };

    let root = Path::new(&args.dir);
    if !root.exists() {
        return Err(SearchError::DirNotFound(args.dir.clone()));
    }

    let match_count = AtomicUsize::new(0);
    let file_count = AtomicUsize::new(0);

    let mut builder = WalkBuilder::new(root);
    builder.hidden(!args.hidden);
    builder.git_ignore(!args.no_ignore);
    builder.git_global(!args.no_ignore);
    builder.git_exclude(!args.no_ignore);

    if args.max_depth > 0 {
        builder.max_depth(Some(args.max_depth));
    }

    let thread_count = if args.threads == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    } else {
        args.threads
    };
    builder.threads(thread_count);

    if args.contents {
        builder.build_parallel().run(|| {
            let pattern = pattern.clone();
            let re = re.clone();
            let ignore_case = args.ignore_case;
            let count_only = args.count;
            let ext_filter = args.ext.clone();
            let match_count = &match_count;
            let file_count = &file_count;

            Box::new(move |entry| {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => return ignore::WalkState::Continue,
                };
                if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                    return ignore::WalkState::Continue;
                }
                if let Some(ref ext) = ext_filter {
                    let matches_ext = entry.path().extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case(ext));
                    if !matches_ext { return ignore::WalkState::Continue; }
                }
                file_count.fetch_add(1, Ordering::Relaxed);
                let content = match fs::read_to_string(entry.path()) {
                    Ok(c) => c,
                    Err(_) => return ignore::WalkState::Continue,
                };
                let matched = if let Some(ref re) = re {
                    re.is_match(&content)
                } else if ignore_case {
                    content.to_lowercase().contains(&pattern)
                } else {
                    content.contains(&pattern)
                };
                if matched {
                    match_count.fetch_add(1, Ordering::Relaxed);
                    if !count_only {
                        for (line_num, line) in content.lines().enumerate() {
                            let line_matched = if let Some(ref re) = re {
                                re.is_match(line)
                            } else if ignore_case {
                                line.to_lowercase().contains(&pattern)
                            } else {
                                line.contains(&pattern)
                            };
                            if line_matched {
                                println!("{}:{}: {}", entry.path().display(), line_num + 1, line.trim());
                            }
                        }
                    }
                }
                ignore::WalkState::Continue
            })
        });
    } else {
        builder.build_parallel().run(|| {
            let pattern = pattern.clone();
            let re = re.clone();
            let ignore_case = args.ignore_case;
            let count_only = args.count;
            let ext_filter = args.ext.clone();
            let match_count = &match_count;
            let file_count = &file_count;

            Box::new(move |entry| {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => return ignore::WalkState::Continue,
                };
                file_count.fetch_add(1, Ordering::Relaxed);
                let name = match entry.file_name().to_str() {
                    Some(n) => n.to_string(),
                    None => return ignore::WalkState::Continue,
                };
                if let Some(ref ext) = ext_filter {
                    let matches_ext = entry.path().extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case(ext));
                    if !matches_ext { return ignore::WalkState::Continue; }
                }
                let search_name = if ignore_case { name.to_lowercase() } else { name.clone() };
                let matched = if let Some(ref re) = re {
                    re.is_match(&search_name)
                } else {
                    search_name.contains(&pattern)
                };
                if matched {
                    match_count.fetch_add(1, Ordering::Relaxed);
                    if !count_only {
                        println!("{}", entry.path().display());
                    }
                }
                ignore::WalkState::Continue
            })
        });
    }

    let elapsed = start.elapsed();
    let matches = match_count.load(Ordering::Relaxed);
    let files = file_count.load(Ordering::Relaxed);
    eprintln!("\n{} matches found among {} entries in {:.3}s ({} threads)",
        matches, files, elapsed.as_secs_f64(), thread_count);
    Ok(())
}

// ─── cmd_fast ───────────────────────────────────────────────────────

fn cmd_fast(args: FastArgs) -> Result<(), SearchError> {
    let start = Instant::now();
    let idx_base = index_dir();

    let index = match load_index(&args.dir, &idx_base) {
        Ok(idx) => {
            if idx.is_stale() && args.auto_reindex {
                eprintln!("Index is stale, rebuilding...");
                let new_index = build_index(&IndexArgs {
                    dir: args.dir.clone(), max_age_hours: idx.max_age_secs / 3600,
                    hidden: false, no_ignore: false, threads: 0,
                });
                if let Err(e) = save_index(&new_index, &idx_base) {
                    eprintln!("Warning: failed to save updated index: {}", e);
                }
                new_index
            } else {
                if idx.is_stale() {
                    eprintln!("Warning: index is stale (use 'search-index index -d {}' to rebuild)", args.dir);
                }
                idx
            }
        }
        Err(_) => {
            eprintln!("No index found for '{}'. Building one now...", args.dir);
            let new_index = build_index(&IndexArgs {
                dir: args.dir.clone(), max_age_hours: 24,
                hidden: false, no_ignore: false, threads: 0,
            });
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

// ─── cmd_grep ───────────────────────────────────────────────────────

fn cmd_grep(args: GrepArgs) -> Result<(), SearchError> {
    let start = Instant::now();
    let idx_base = index_dir();
    let exts_for_load = args.ext.clone().unwrap_or_default();

    let index = match load_content_index(&args.dir, &exts_for_load, &idx_base) {
        Ok(idx) => {
            if idx.is_stale() && args.auto_reindex {
                eprintln!("Content index is stale, rebuilding...");
                let ext_str = idx.extensions.join(",");
                let new_idx = build_content_index(&ContentIndexArgs {
                    dir: args.dir.clone(), ext: ext_str, max_age_hours: idx.max_age_secs / 3600,
                    hidden: false, no_ignore: false, threads: 0, min_token_len: DEFAULT_MIN_TOKEN_LEN,
                });
                let _ = save_content_index(&new_idx, &idx_base);
                new_idx
            } else {
                if idx.is_stale() { eprintln!("Warning: content index is stale"); }
                idx
            }
        }
        Err(_) => {
            match find_content_index_for_dir(&args.dir, &idx_base) {
                Some(idx) => idx,
                None => return Err(SearchError::IndexNotFound { dir: args.dir.clone() }),
            }
        }
    };

    let load_elapsed = start.elapsed();
    let search_start = Instant::now();

    // ─── Determine search mode ──────────────────────────────
    // Default: substring search (like MCP). Disabled by --exact, --regex, or --phrase.
    let use_substring = !args.exact && !args.regex && !args.phrase;

    // ─── Phrase search mode ─────────────────────────────────
    if args.phrase {
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

        let mut candidate_file_ids: Option<std::collections::HashSet<u32>> = None;
        for token in &phrase_tokens {
            if let Some(postings) = index.index.get(token.as_str()) {
                let file_ids: std::collections::HashSet<u32> = postings.iter()
                    .filter(|p| {
                        let path = match index.files.get(p.file_id as usize) {
                            Some(p) => p,
                            None => return false,
                        };
                        if let Some(ref ext) = args.ext {
                            let m = Path::new(path).extension().and_then(|e| e.to_str())
                                .is_some_and(|e| e.eq_ignore_ascii_case(ext));
                            if !m { return false; }
                        }
                        if args.exclude_dir.iter().any(|excl| path.to_lowercase().contains(&excl.to_lowercase())) { return false; }
                        if args.exclude.iter().any(|excl| path.to_lowercase().contains(&excl.to_lowercase())) { return false; }
                        true
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

        let candidates = candidate_file_ids.unwrap_or_default();
        eprintln!("Found {} candidate files containing all tokens", candidates.len());

        struct PhraseMatch { file_path: String, lines: Vec<u32> }
        let mut results: Vec<PhraseMatch> = Vec::new();

        for &file_id in &candidates {
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

        let search_elapsed = search_start.elapsed();
        let total_elapsed = start.elapsed();
        let match_count = results.len();
        let line_count: usize = results.iter().map(|r| r.lines.len()).sum();

        let display_results = if args.max_results > 0 { &results[..results.len().min(args.max_results)] } else { &results };
        let ctx_before = if args.context > 0 { args.context } else { args.before };
        let ctx_after = if args.context > 0 { args.context } else { args.after };

        if !args.count {
            for result in display_results {
                if args.show_lines {
                    if let Ok(content) = fs::read_to_string(&result.file_path) {
                        let lines_vec: Vec<&str> = content.lines().collect();
                        let total_lines = lines_vec.len();
                        let mut lines_to_show: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
                        let mut match_lines_set: std::collections::HashSet<usize> = std::collections::HashSet::new();
                        for &ln in &result.lines {
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
                            println!("{}{}:{}: {}", marker, result.file_path, idx + 1, lines_vec[idx]);
                            prev = Some(idx);
                        }
                        if !lines_to_show.is_empty() { println!(); }
                    }
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
        return Ok(());
    }

    // ─── Normal token search ────────────────────────────────
    let raw_terms: Vec<String> = args.pattern.split(',')
        .map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();

    let terms: Vec<String> = if use_substring {
        // Expand terms using trigram index: find all tokens containing each term as a substring
        let trigram_idx = &index.trigram;
        let mut expanded = Vec::new();
        for term in &raw_terms {
            let matched_tokens: Vec<String> = if term.len() < 3 {
                // Linear scan for very short terms (no trigrams possible)
                trigram_idx.tokens.iter()
                    .filter(|tok| tok.contains(term.as_str()))
                    .cloned()
                    .collect()
            } else {
                let trigrams = search_index::generate_trigrams(term);
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
    } else if args.regex {
        let mut expanded = Vec::new();
        for pat in &raw_terms {
            match Regex::new(&format!("(?i)^{}$", pat)) {
                Ok(re) => {
                    let matching: Vec<String> = index.index.keys().filter(|k| re.is_match(k)).cloned().collect();
                    if matching.is_empty() { eprintln!("Warning: regex '{}' matched 0 tokens", pat); }
                    else { eprintln!("Regex '{}' matched {} tokens", pat, matching.len()); }
                    expanded.extend(matching);
                }
                Err(e) => return Err(SearchError::InvalidRegex { pattern: pat.clone(), source: e }),
            }
        }
        expanded
    } else {
        raw_terms.clone()
    };

    let total_docs = index.files.len() as f64;
    let mode_str = if use_substring { if args.all { "SUBSTRING-AND" } else { "SUBSTRING-OR" } }
        else if args.regex { "REGEX" } else if args.all { "AND" } else { "OR" };

    struct FileScore { file_path: String, lines: Vec<u32>, tf_idf: f64, occurrences: usize, terms_matched: usize }
    let mut file_scores: HashMap<u32, FileScore> = HashMap::new();
    let term_count_for_all = if args.regex || use_substring { raw_terms.len() } else { terms.len() };

    for term in &terms {
        if let Some(postings) = index.index.get(term.as_str()) {
            let doc_freq = postings.len() as f64;
            let idf = (total_docs / doc_freq).ln();
            for posting in postings {
                let file_path = match index.files.get(posting.file_id as usize) {
                    Some(p) => p,
                    None => continue,
                };
                if let Some(ref ext) = args.ext {
                    let matches = Path::new(file_path).extension().and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case(ext));
                    if !matches { continue; }
                }
                if args.exclude_dir.iter().any(|excl| file_path.to_lowercase().contains(&excl.to_lowercase())) { continue; }
                if args.exclude.iter().any(|excl| file_path.to_lowercase().contains(&excl.to_lowercase())) { continue; }

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
        .filter(|fs| !args.all || fs.terms_matched >= term_count_for_all).collect();

    for result in &mut results { result.lines.sort(); result.lines.dedup(); }
    results.sort_by(|a, b| b.tf_idf.partial_cmp(&a.tf_idf).unwrap_or(std::cmp::Ordering::Equal));

    let match_count = results.len();
    let line_count: usize = results.iter().map(|r| r.lines.len()).sum();
    let display_results = if args.max_results > 0 { &results[..results.len().min(args.max_results)] } else { &results };

    let ctx_before = if args.context > 0 { args.context } else { args.before };
    let ctx_after = if args.context > 0 { args.context } else { args.after };

    if !args.count {
        for result in display_results {
            if args.show_lines {
                if let Ok(content) = fs::read_to_string(&result.file_path) {
                    let lines_vec: Vec<&str> = content.lines().collect();
                    let total_lines = lines_vec.len();
                    let mut lines_to_show: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
                    let mut match_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
                    for &line_num in &result.lines {
                        let idx = (line_num as usize).saturating_sub(1);
                        if idx < total_lines {
                            match_lines.insert(idx);
                            let start = idx.saturating_sub(ctx_before);
                            let end = (idx + ctx_after).min(total_lines - 1);
                            for i in start..=end { lines_to_show.insert(i); }
                        }
                    }
                    let mut prev_idx: Option<usize> = None;
                    for &idx in &lines_to_show {
                        if let Some(prev) = prev_idx && idx > prev + 1 { println!("--"); }
                        let marker = if match_lines.contains(&idx) { ">" } else { " " };
                        println!("{}{}:{}: {}", marker, result.file_path, idx + 1, lines_vec[idx]);
                        prev_idx = Some(idx);
                    }
                    if !lines_to_show.is_empty() { println!(); }
                }
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