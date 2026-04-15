//! Definition index: AST-based code structure extraction using tree-sitter.

mod types;
mod tree_sitter_utils;
#[cfg(feature = "lang-csharp")]
mod parser_csharp;
#[cfg(feature = "lang-typescript")]
mod parser_typescript;
#[cfg(feature = "lang-sql")]
mod parser_sql;
#[cfg(feature = "lang-rust")]
mod parser_rust;
#[cfg(feature = "lang-xml")]
pub(crate) mod parser_xml;
mod storage;
mod incremental;

// Re-export all public types and functions
pub use types::*;
pub use storage::*;
pub use incremental::*;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;

use crate::{clean_path, read_file_lossy};
#[cfg(feature = "lang-typescript")]
use parser_typescript::extract_component_metadata;

/// File extensions that have definition parser support (tree-sitter or regex).
/// Used to dynamically generate MCP instructions about which files can be read
/// via xray_definitions instead of direct file reads.
///
/// Returns extensions based on which language features are compiled in.
pub fn definition_extensions() -> &'static [&'static str] {
    // When all default features are enabled, return the same list as the old const.
    // For non-default feature combos, cfg picks the right subset.
    // Build at compile time based on enabled features.
    const EXTS: &[&str] = &[
        #[cfg(feature = "lang-csharp")]
        "cs",
        #[cfg(feature = "lang-typescript")]
        "ts",
        #[cfg(feature = "lang-typescript")]
        "tsx",
        #[cfg(feature = "lang-sql")]
        "sql",
        #[cfg(feature = "lang-rust")]
        "rs",
    ];
    EXTS
}

// ─── Extracted helpers ───────────────────────────────────────────────

/// Walk directory tree and collect all source files matching the given extensions.
/// Returns cleaned file paths. Uses parallel walker with .gitignore support.
pub(crate) fn collect_source_files(
    dir: &Path,
    extensions: &[String],
    threads: usize,
) -> Vec<String> {
    let mut walker = WalkBuilder::new(dir);
    walker.hidden(false).git_ignore(true);
    if threads > 0 {
        walker.threads(threads);
    }

    let all_files: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    walker.build_parallel().run(|| {
        Box::new(|entry| {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => return ignore::WalkState::Continue,
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }
            let path = entry.path();
            let ext_match = path.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| extensions.iter().any(|x| x.eq_ignore_ascii_case(e)));
            if !ext_match {
                return ignore::WalkState::Continue;
            }
            let clean = clean_path(&path.to_string_lossy());
            all_files.lock().unwrap_or_else(|e| e.into_inner()).push(clean);
            ignore::WalkState::Continue
        })
    });

    crate::index::recover_mutex(all_files, "def-index")
}

/// Index a single file's parsed definitions into the DefinitionIndex.
///
/// Populates: name_index, kind_index, attribute_index, base_type_index,
/// file_index, method_calls, and code_stats.
///
/// Returns the number of call sites added.
///
/// Used by both `build_definition_index()` (bulk build) and
/// `update_file_definitions()` (incremental update) to eliminate duplication.
pub(crate) fn index_file_defs(
    index: &mut DefinitionIndex,
    file_id: u32,
    file_defs: Vec<DefinitionEntry>,
    file_calls: Vec<(usize, Vec<CallSite>)>,
    file_stats: Vec<(usize, CodeStats)>,
) -> usize {
    let base_def_idx = index.definitions.len() as u32;
    let mut call_sites_added = 0usize;

    for def in file_defs {
        let def_idx = index.definitions.len() as u32;

        index.name_index.entry(def.name.to_lowercase())
            .or_default()
            .push(def_idx);

        index.kind_index.entry(def.kind)
            .or_default()
            .push(def_idx);

        {
            let mut seen_attrs = std::collections::HashSet::new();
            for attr in &def.attributes {
                let attr_name = attr.split('(').next().unwrap_or(attr).trim().to_lowercase();
                if seen_attrs.insert(attr_name.clone()) {
                    index.attribute_index.entry(attr_name)
                        .or_default()
                        .push(def_idx);
                }
            }
        }

        for bt in &def.base_types {
            index.base_type_index.entry(bt.to_lowercase())
                .or_default()
                .push(def_idx);
        }

        index.file_index.entry(file_id)
            .or_default()
            .push(def_idx);

        index.definitions.push(def);
    }

    // Map local call site indices to global def indices
    for (local_idx, calls) in file_calls {
        let global_idx = base_def_idx + local_idx as u32;
        if !calls.is_empty() {
            call_sites_added += calls.len();
            index.method_calls.insert(global_idx, calls);
        }
    }

    // Map local code stats indices to global def indices
    for (local_idx, stats) in file_stats {
        let global_idx = base_def_idx + local_idx as u32;
        index.code_stats.insert(global_idx, stats);
    }

    call_sites_added
}

/// Scan Angular @Component definitions for selectors and template children.
/// Populates selector_index and template_children from HTML templates.
#[cfg(feature = "lang-typescript")]
pub(crate) fn enrich_angular_templates(
    definitions: &[DefinitionEntry],
    files: &[String],
    name_index: &mut HashMap<String, Vec<u32>>,
    selector_index: &mut HashMap<String, Vec<u32>>,
    template_children: &mut HashMap<u32, Vec<String>>,
) {
    let template_start = Instant::now();
    let mut templates_processed = 0usize;
    let mut templates_failed = 0usize;

    for (def_idx, def) in definitions.iter().enumerate() {
        if def.kind != DefinitionKind::Class { continue; }
        let component_attr = match def.attributes.iter().find(|a| a.starts_with("Component(")) {
            Some(a) => a,
            None => continue,
        };
        let (selector, template_url) = match extract_component_metadata(component_attr) {
            Some(meta) => meta,
            None => continue,
        };
        // Add selector to name_index for discoverability
        let sel_lower = selector.to_lowercase();
        name_index.entry(sel_lower).or_default().push(def_idx as u32);

        selector_index.entry(selector).or_default().push(def_idx as u32);

        if let Some(ref tpl_url) = template_url {
            let ts_file_path = match files.get(def.file_id as usize) {
                Some(p) => p,
                None => continue,
            };
            let html_path = match std::path::Path::new(ts_file_path).parent() {
                Some(dir) => dir.join(tpl_url.strip_prefix("./").unwrap_or(tpl_url)),
                None => std::path::PathBuf::from(tpl_url),
            };
            match std::fs::read_to_string(&html_path) {
                Ok(html_content) => {
                    let children = extract_custom_elements(&html_content);
                    if !children.is_empty() {
                        template_children.insert(def_idx as u32, children);
                        templates_processed += 1;
                    }
                }
                Err(_) => { templates_failed += 1; }
            }
        }
    }

    if templates_processed > 0 || templates_failed > 0 {
        eprintln!("[def-index] Angular templates: {} enriched, {} read errors ({:.1}ms)",
            templates_processed, templates_failed, template_start.elapsed().as_secs_f64() * 1000.0);
    }
}

// ─── Index Build ─────────────────────────────────────────────────────

/// Merge a single chunk's parse results into the DefinitionIndex.
/// Returns the number of call sites added.
///
/// Extracted as a helper to support join-based streaming merge,
/// where each chunk is merged and freed immediately after its thread completes.
fn merge_chunk_result(
    index: &mut DefinitionIndex,
    chunk_defs: Vec<DefChunk>,
    errors: usize,
    lossy_files: Vec<String>,
    empty_files: Vec<(u32, u64)>,
    chunk_ext_methods: HashMap<String, Vec<String>>,
) -> usize {
    index.parse_errors += errors;
    for f in &lossy_files {
        eprintln!("[def-index] WARNING: file contains non-UTF8 bytes (lossy conversion applied): {}", f);
    }
    index.lossy_file_count += lossy_files.len();
    index.empty_file_ids.extend(empty_files);

    let mut call_sites = 0usize;
    for (file_id, file_defs, file_calls, file_stats) in chunk_defs {
        call_sites += index_file_defs(index, file_id, file_defs, file_calls, file_stats);
    }

    for (method_name, classes) in chunk_ext_methods {
        index.extension_methods.entry(method_name).or_default().extend(classes);
    }

    call_sites
}

#[must_use]
pub fn build_definition_index(args: &DefIndexArgs) -> DefinitionIndex {
    let dir = std::fs::canonicalize(&args.dir)
        .unwrap_or_else(|_| PathBuf::from(&args.dir));
    let dir_str = clean_path(&dir.to_string_lossy());

    let extensions: Vec<String> = args.ext.split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    let start = Instant::now();

    // ─── Collect all matching source files ─────────────────────
    let files = collect_source_files(&dir, &extensions, args.threads);
    let total_files = files.len();
    eprintln!("[def-index] Found {} files to parse", total_files);
    crate::index::log_memory(&format!("def-build: after file walk ({} files)", total_files));

    // ─── Parallel parsing ─────────────────────────────────────
    let num_threads = if args.threads > 0 {
        args.threads
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    };
    #[cfg(feature = "lang-typescript")]
    let need_ts = extensions.iter().any(|e| e == "ts");
    #[cfg(feature = "lang-typescript")]
    let need_tsx = extensions.iter().any(|e| e == "tsx");
    #[cfg(feature = "lang-rust")]
    let need_rs = extensions.iter().any(|e| e == "rs");

    // ─── Initialize index BEFORE chunked parsing ─────────────
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::with_capacity(files.len());
    for (file_id, file_path) in files.iter().enumerate() {
        path_to_id.insert(PathBuf::from(file_path), file_id as u32);
    }

    let mut index = DefinitionIndex {
        root: dir_str,
        format_version: types::DEFINITION_INDEX_VERSION,
        extensions,
        files,
        path_to_id,
        ..Default::default()
    };

    let mut total_call_sites = 0usize;

    // ─── Chunked parallel parsing + streaming merge ──────────
    // Outer loop splits files into macro-chunks of 4096.
    // Each macro-chunk is parsed by num_threads threads in parallel.
    // After each macro-chunk, results are merged and freed, and
    // mimalloc is asked to return memory to OS. This reduces peak
    // memory by ~350 MB for def-build (only 1 macro-chunk's parse
    // results live at a time instead of ALL files' results).
    const MACRO_CHUNK_SIZE: usize = 4096;

    let file_entries: Vec<(u32, String)> = index.files.iter().enumerate()
        .map(|(i, f)| (i as u32, f.clone()))
        .collect();

    let total_macro_chunks = file_entries.len().div_ceil(MACRO_CHUNK_SIZE).max(1);

    eprintln!("[def-index] Parsing with {} threads, {} macro-chunks of up to {} files",
        num_threads, total_macro_chunks, MACRO_CHUNK_SIZE);

    for (macro_chunk_idx, macro_chunk) in file_entries.chunks(MACRO_CHUNK_SIZE).enumerate() {
        let sub_chunk_size = macro_chunk.len().div_ceil(num_threads).max(1);
        let sub_chunks: Vec<&[(u32, String)]> = macro_chunk.chunks(sub_chunk_size).collect();
        let num_sub_chunks = sub_chunks.len();

        std::thread::scope(|s| {
            let handles: Vec<_> = sub_chunks.into_iter().map(|sub_chunk| {
                s.spawn(move || {
                    #[cfg(feature = "lang-csharp")]
                    let mut cs_parser = {
                        let mut p = tree_sitter::Parser::new();
                        p.set_language(&tree_sitter_c_sharp::LANGUAGE.into())
                            .expect("Error loading C# grammar");
                        p
                    };

                    #[cfg(feature = "lang-typescript")]
                    let mut ts_parser: Option<tree_sitter::Parser> = None;
                    #[cfg(feature = "lang-typescript")]
                    let mut tsx_parser: Option<tree_sitter::Parser> = None;
                    #[cfg(feature = "lang-rust")]
                    let mut rs_parser: Option<tree_sitter::Parser> = None;

                    let mut chunk_defs: Vec<DefChunk> = Vec::new();
                    #[cfg(feature = "lang-csharp")]
                    let mut chunk_ext_methods: HashMap<String, Vec<String>> = HashMap::new();
                    let mut errors = 0usize;
                    let mut lossy_files: Vec<String> = Vec::new();
                    let mut empty_files: Vec<(u32, u64)> = Vec::new();

                    for (file_id, file_path) in sub_chunk {
                        let (content, was_lossy) = match read_file_lossy(Path::new(file_path)) {
                            Ok(r) => r,
                            Err(_) => { errors += 1; continue; }
                        };
                        if was_lossy {
                            lossy_files.push(file_path.clone());
                        }

                        let content_len = content.len() as u64;

                        let ext = Path::new(file_path.as_str())
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("");

                        let (file_defs, file_calls, file_stats) = match ext.to_lowercase().as_str() {
                            #[cfg(feature = "lang-csharp")]
                            "cs" => {
                                let (defs, calls, stats, ext_methods) = parser_csharp::parse_csharp_definitions(&mut cs_parser, &content, *file_id);
                                for (method_name, classes) in ext_methods {
                                    chunk_ext_methods.entry(method_name).or_default().extend(classes);
                                }
                                (defs, calls, stats)
                            }
                            #[cfg(feature = "lang-typescript")]
                            "ts" if need_ts => {
                                let parser = ts_parser.get_or_insert_with(|| {
                                    let mut p = tree_sitter::Parser::new();
                                    p.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
                                        .expect("Error loading TypeScript grammar");
                                    p
                                });
                                parser_typescript::parse_typescript_definitions(parser, &content, *file_id)
                            }
                            #[cfg(feature = "lang-typescript")]
                            "tsx" if need_tsx => {
                                let parser = tsx_parser.get_or_insert_with(|| {
                                    let mut p = tree_sitter::Parser::new();
                                    p.set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())
                                        .expect("Error loading TSX grammar");
                                    p
                                });
                                parser_typescript::parse_typescript_definitions(parser, &content, *file_id)
                            }
                            #[cfg(feature = "lang-sql")]
                            "sql" => {
                                let (defs, calls, stats) = parser_sql::parse_sql_definitions(&content, *file_id);
                                (defs, calls, stats)
                            }
                            #[cfg(feature = "lang-rust")]
                            "rs" if need_rs => {
                                let parser = rs_parser.get_or_insert_with(|| {
                                    let mut p = tree_sitter::Parser::new();
                                    p.set_language(&tree_sitter_rust::LANGUAGE.into())
                                        .expect("Error loading Rust grammar");
                                    p
                                });
                                parser_rust::parse_rust_definitions(parser, &content, *file_id)
                            }
                            _ => (Vec::new(), Vec::new(), Vec::new()),
                        };

                        if !file_defs.is_empty() {
                            chunk_defs.push((*file_id, file_defs, file_calls, file_stats));
                        } else {
                            empty_files.push((*file_id, content_len));
                        }
                    }

                    #[cfg(feature = "lang-csharp")]
                    { (chunk_defs, errors, lossy_files, empty_files, chunk_ext_methods) }
                    #[cfg(not(feature = "lang-csharp"))]
                    { (chunk_defs, errors, lossy_files, empty_files, HashMap::<String, Vec<String>>::new()) }
                })
            }).collect();

            // ─── Join-based streaming merge ─────────────────────
            for (sub_idx, handle) in handles.into_iter().enumerate() {
                let (chunk_defs, errors, lossy_files, empty_files, chunk_ext_methods) =
                    handle.join().unwrap_or_else(|_| {
                        eprintln!("[WARN] Worker thread panicked during definition index building");
                        (Vec::new(), 0, Vec::new(), Vec::new(), HashMap::new())
                    });

                total_call_sites += merge_chunk_result(
                    &mut index, chunk_defs, errors, lossy_files, empty_files, chunk_ext_methods,
                );

                crate::index::log_memory(&format!(
                    "def-build: merged sub-chunk {}/{} of macro-chunk {}/{} ({} defs so far)",
                    sub_idx + 1, num_sub_chunks,
                    macro_chunk_idx + 1, total_macro_chunks,
                    index.definitions.len()
                ));
            }
        });
        // All sub-chunk parse results are dropped here

        crate::index::log_memory(&format!(
            "def-build: macro-chunk {}/{} complete ({} defs so far)",
            macro_chunk_idx + 1, total_macro_chunks, index.definitions.len()
        ));
        crate::index::force_mimalloc_collect();
    }

    // ─── Angular template enrichment ──────────────────────────
    #[cfg(feature = "lang-typescript")]
    enrich_angular_templates(
        &index.definitions, &index.files,
        &mut index.name_index, &mut index.selector_index, &mut index.template_children,
    );

    // ─── Report and finalize ──────────────────────────────────
    let suspicious_threshold = 500u64;
    let suspicious_count = index.empty_file_ids.iter()
        .filter(|(_, size)| *size > suspicious_threshold)
        .count();
    if suspicious_count > 0 {
        eprintln!("[def-index] WARNING: {} files with >{}B but 0 definitions. Run 'xray def-audit' to see full list.",
            suspicious_count, suspicious_threshold);
    }

    crate::index::log_memory(&format!("def-build: parsing complete ({} defs, {} calls)", index.definitions.len(), total_call_sites));

    let elapsed = start.elapsed();
    let files_with_defs = total_files - index.empty_file_ids.len() - index.parse_errors;
    eprintln!(
        "[def-index] Parsed {} files in {:.1}s: {} with definitions, {} empty, {} read errors, {} lossy-utf8, {} threads",
        total_files,
        elapsed.as_secs_f64(),
        files_with_defs,
        index.empty_file_ids.len(),
        index.parse_errors,
        index.lossy_file_count,
        num_threads
    );
    eprintln!(
        "[def-index] Extracted {} definitions, {} call sites, {} code stats entries",
        index.definitions.len(),
        total_call_sites,
        index.code_stats.len(),
    );

    index.created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();

    index
}

/// Extract custom element tag names from HTML content.
#[cfg_attr(not(feature = "lang-typescript"), allow(dead_code))]
/// Custom elements are identified by a hyphen in the tag name (HTML spec, web components).
/// Excludes Angular built-ins: ng-container, ng-content, ng-template.
/// Returns a deduplicated, sorted list in lowercase.
pub(crate) fn extract_custom_elements(html_content: &str) -> Vec<String> {
    let mut elements: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let bytes = html_content.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        if bytes[i] == b'<' && i + 1 < len && bytes[i + 1].is_ascii_alphabetic() {
            let start = i + 1;
            let mut end = start;
            while end < len && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-') {
                end += 1;
            }
            let tag_name = &html_content[start..end];
            if tag_name.contains('-') {
                let tag_lower = tag_name.to_lowercase();
                if !tag_lower.starts_with("ng-") && seen.insert(tag_lower.clone()) {
                    elements.push(tag_lower);
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
    elements.sort();
    elements
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "definitions_tests.rs"]
mod tests;

#[cfg(all(test, feature = "lang-csharp"))]
#[path = "definitions_tests_csharp.rs"]
mod tests_csharp;

#[cfg(all(test, feature = "lang-typescript"))]
#[path = "definitions_tests_typescript.rs"]
mod tests_typescript;

#[cfg(all(test, feature = "lang-sql"))]
#[path = "definitions_tests_sql.rs"]
mod tests_sql;

#[cfg(all(test, feature = "lang-rust"))]
#[path = "definitions_tests_rust.rs"]
mod tests_rust;

#[cfg(all(test, feature = "lang-xml"))]
#[path = "definitions_tests_xml.rs"]
mod tests_xml;

#[cfg(all(test, feature = "lang-csharp", feature = "lang-typescript"))]
#[path = "audit_tests.rs"]
mod audit_tests;

