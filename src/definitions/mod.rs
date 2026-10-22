//! Definition index: AST-based code structure extraction using tree-sitter.

/// PARSE-002: hard cap shared by source parsers and Angular template extraction.
/// Tree-sitter parsers use it to bound parse-tree memory; template enrichment uses the
/// same 4 MiB limit for consistent source coverage and to bound file reads per worker.
pub(crate) const MAX_PARSE_SOURCE_BYTES: usize = 4 * 1024 * 1024;

mod types;
mod csharp_semantics;
#[cfg(any(feature = "lang-csharp", feature = "lang-typescript", feature = "lang-rust", feature = "lang-xml"))]
mod tree_sitter_utils;
#[cfg(feature = "lang-csharp")]
mod parser_csharp;
#[cfg(feature = "lang-typescript")]
mod parser_typescript;
// SQL parser is always compiled in — it is a pure regex-based parser with no
// tree-sitter dependency, so there is no cost to keeping it unconditional.
// The former `lang-sql` feature was removed because the T-SQL tree-sitter
// grammar is incompatible with tree-sitter 0.24 (see Cargo.toml).
mod parser_sql;
#[cfg(feature = "lang-rust")]
mod parser_rust;
#[cfg(feature = "lang-xml")]
pub(crate) mod parser_xml;
mod storage;
mod incremental;

// Re-export all public types and functions
pub use types::*;
pub use csharp_semantics::*;
pub use storage::*;
pub use incremental::*;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sha2::{Digest, Sha256};

use ignore::WalkBuilder;

use crate::{clean_path, is_inside_git_dir};

fn definition_metadata_revision(metadata: &std::fs::Metadata) -> (u64, u128) {
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos());
    (metadata.len(), modified_nanos)
}

pub(crate) fn definition_input_revision_from_metadata(
    metadata: &std::fs::Metadata,
) -> DefinitionInputRevision {
    let (size, modified_nanos) = definition_metadata_revision(metadata);
    DefinitionInputRevision {
        size,
        modified_nanos,
    }
}

pub(crate) fn definition_input_revision(
    path: &Path,
) -> Option<DefinitionInputRevision> {
    std::fs::metadata(path)
        .ok()
        .as_ref()
        .map(definition_input_revision_from_metadata)
}

fn definition_input_fingerprint(
    metadata: &std::fs::Metadata,
    content: &[u8],
) -> DefinitionInputFingerprint {
    DefinitionInputFingerprint {
        size: metadata.len(),
        modified_nanos: definition_metadata_revision(metadata).1,
        content_hash: Sha256::digest(content).into(),
    }
}

pub(crate) fn definition_input_key(path: &Path) -> String {
    crate::clean_path(&crate::path_identity_key(path).to_string_lossy())
}

pub(crate) fn replace_live_definition_index(
    current: &mut DefinitionIndex,
    mut replacement: DefinitionIndex,
) {
    replacement.definition_generation = current
        .definition_generation
        .max(replacement.definition_generation)
        .saturating_add(1);
    *current = replacement;
}

pub(crate) fn definition_fingerprint_conflicts(
    index: &DefinitionIndex,
    expected: &HashMap<String, Option<DefinitionInputFingerprint>>,
) -> HashSet<String> {
    expected
        .iter()
        .filter(|(key, fingerprint)| index.input_fingerprints.get(*key) != fingerprint.as_ref())
        .map(|(key, _)| key.clone())
        .collect()
}

#[cfg(test)]
pub(crate) fn definition_fingerprints_match(
    index: &DefinitionIndex,
    expected: &HashMap<String, Option<DefinitionInputFingerprint>>,
) -> bool {
    definition_fingerprint_conflicts(index, expected).is_empty()
}

#[cfg(test)]
static DEFINITION_SOURCE_READ_ERRORS: std::sync::LazyLock<
    std::sync::Mutex<HashMap<PathBuf, std::io::ErrorKind>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

#[cfg(test)]
pub(crate) fn install_definition_source_read_error(
    path: &Path,
    kind: std::io::ErrorKind,
) {
    DEFINITION_SOURCE_READ_ERRORS
        .lock()
        .unwrap()
        .insert(crate::path_identity_key(path), kind);
}

#[cfg(test)]
pub(crate) fn remove_definition_source_read_error(path: &Path) {
    DEFINITION_SOURCE_READ_ERRORS
        .lock()
        .unwrap()
        .remove(&crate::path_identity_key(path));
}


pub(crate) fn read_definition_source_snapshot(
    path: &Path,
) -> std::io::Result<(String, bool, DefinitionInputFingerprint)> {
    #[cfg(test)]
    if let Some(kind) = DEFINITION_SOURCE_READ_ERRORS
        .lock()
        .unwrap()
        .get(&crate::path_identity_key(path))
        .copied()
    {
        return Err(std::io::Error::from(kind));
    }

    for _ in 0..2 {
        let before = std::fs::metadata(path)?;
        let before_revision = definition_metadata_revision(&before);
        let (content, was_lossy) = crate::read_file_lossy(path)?;
        let after = std::fs::metadata(path)?;
        if before_revision == definition_metadata_revision(&after) {
            let fingerprint = definition_input_fingerprint(&after, content.as_bytes());
            return Ok((content, was_lossy, fingerprint));
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        format!("definition input changed while reading: {}", path.display()),
    ))
}


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
    respect_git_exclude: bool,
) -> Vec<String> {
    let mut walker = WalkBuilder::new(dir);
    walker.follow_links(true).hidden(false).git_ignore(true).git_exclude(respect_git_exclude);

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
            if is_inside_git_dir(path) {
                return ignore::WalkState::Continue;
            }
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
#[cfg(test)]
pub(crate) fn index_file_defs(
    index: &mut DefinitionIndex,
    file_id: u32,
    file_defs: Vec<DefinitionEntry>,
    file_calls: Vec<(usize, Vec<CallSite>)>,
    file_stats: Vec<(usize, CodeStats)>,
) -> usize {
    index_file_defs_with_semantics(
        index,
        file_id,
        file_defs,
        file_calls,
        file_stats,
        CSharpFileContribution::default(),
    )
}

pub(crate) fn index_file_defs_with_semantics(
    index: &mut DefinitionIndex,
    file_id: u32,
    file_defs: Vec<DefinitionEntry>,
    file_calls: Vec<(usize, Vec<CallSite>)>,
    file_stats: Vec<(usize, CodeStats)>,
    file_csharp_semantics: CSharpFileContribution,
) -> usize {
    let base_def_idx = index.definitions.len() as u32;
    let definition_count = file_defs.len();
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
        if calls.is_empty() {
            continue;
        }
        if let Some(existing) = index.method_calls.get_mut(&global_idx) {
            tracing::warn!(
                target: "xray::definitions",
                definition_index = global_idx,
                "duplicate call-site owner; merging batches"
            );
            let mut seen: HashSet<CallSite> = existing.iter().cloned().collect();
            for call in calls {
                if seen.insert(call.clone()) {
                    existing.push(call);
                    call_sites_added += 1;
                }
            }
        } else {
            call_sites_added += calls.len();
            index.method_calls.insert(global_idx, calls);
        }
    }

    // Map local code stats indices to global def indices
    for (local_idx, stats) in file_stats {
        let global_idx = base_def_idx + local_idx as u32;
        index.code_stats.insert(global_idx, stats);
    }

    index.csharp_semantics.apply_file_contribution(
        base_def_idx,
        file_id,
        definition_count,
        file_csharp_semantics,
    );

    call_sites_added
}

#[cfg(feature = "lang-typescript")]
enum AngularTemplateRead {
    Content {
        content: String,
        fingerprint: DefinitionInputFingerprint,
    },
    TooLarge { observed_size: u64 },
}

#[cfg(feature = "lang-typescript")]
fn read_angular_template(path: &Path) -> std::io::Result<AngularTemplateRead> {
    use std::io::Read as _;

    for _ in 0..2 {
        let before = std::fs::metadata(path)?;
        let before_revision = definition_metadata_revision(&before);
        if before.len() > MAX_PARSE_SOURCE_BYTES as u64 {
            return Ok(AngularTemplateRead::TooLarge {
                observed_size: before.len(),
            });
        }

        let file = std::fs::File::open(path)?;
        let mut reader = file.take((MAX_PARSE_SOURCE_BYTES + 1) as u64);
        let mut bytes = Vec::with_capacity(before.len() as usize);
        reader.read_to_end(&mut bytes)?;
        if bytes.len() > MAX_PARSE_SOURCE_BYTES {
            return Ok(AngularTemplateRead::TooLarge {
                observed_size: bytes.len() as u64,
            });
        }

        let after = std::fs::metadata(path)?;
        if before_revision != definition_metadata_revision(&after) {
            continue;
        }
        let fingerprint = definition_input_fingerprint(&after, &bytes);
        let content = String::from_utf8(bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        return Ok(AngularTemplateRead::Content {
            content,
            fingerprint,
        });
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        format!("Angular template changed while reading: {}", path.display()),
    ))
}

#[cfg(feature = "lang-typescript")]
fn lexical_normalize_path(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            std::path::Component::Normal(segment) => normalized.push(segment),
        }
    }
    Some(normalized)
}

#[cfg(feature = "lang-typescript")]
fn resolve_angular_template_source_path(
    workspace_root: &str,
    source_path: &Path,
    relative_path: &str,
) -> Option<(PathBuf, String)> {
    let template_path = lexical_normalize_path(&source_path.parent()?.join(relative_path))?;
    let workspace_path = lexical_normalize_path(Path::new(workspace_root))?;
    let template_key = crate::path_identity_key(&template_path);
    let workspace_key = crate::path_identity_key(&workspace_path);
    if !template_key.starts_with(&workspace_key) {
        return None;
    }
    let owner_key = crate::clean_path(&template_key.to_string_lossy());
    Some((template_path, owner_key))
}

#[cfg(feature = "lang-typescript")]
fn resolve_angular_template_path(
    workspace_root: &str,
    files: &[String],
    file_id: u32,
    relative_path: &str,
) -> Option<(PathBuf, String)> {
    let source_path = Path::new(files.get(file_id as usize)?);
    resolve_angular_template_source_path(workspace_root, source_path, relative_path)
}

#[cfg(feature = "lang-typescript")]
pub(crate) fn prepare_angular_template_updates(
    workspace_root: &str,
    parsed_results: &mut [ParsedFileResult],
    changed_paths: &[PathBuf],
    existing_owner_keys: &HashSet<String>,
) -> Vec<PreparedAngularTemplateUpdate> {
    let mut candidates: HashMap<String, (PathBuf, bool, HashSet<PathBuf>)> = HashMap::new();
    for path in changed_paths {
        let owner_key = crate::clean_path(
            &crate::path_identity_key(path).to_string_lossy(),
        );
        if existing_owner_keys.contains(&owner_key) {
            candidates.insert(owner_key, (path.clone(), true, HashSet::new()));
        }
    }

    for result in parsed_results {
        for record in &mut result.angular_components {
            let relative_path = match &record.component.template {
                AngularTemplateSource::External { relative_path }
                | AngularTemplateSource::UnavailableExternal { relative_path, .. } => {
                    relative_path.clone()
                }
                _ => continue,
            };
            let Some((template_path, owner_key)) = resolve_angular_template_source_path(
                workspace_root,
                &result.path,
                &relative_path,
            ) else {
                record.component.template = AngularTemplateSource::UnavailableExternal {
                    relative_path,
                    reason: "external template path is outside the workspace".to_string(),
                };
                continue;
            };
            candidates
                .entry(owner_key)
                .or_insert_with(|| (template_path, false, HashSet::new()))
                .2
                .insert(result.path.clone());
        }
    }

    let mut candidates: Vec<_> = candidates.into_iter().collect();
    candidates.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    candidates.into_iter().map(|(owner_key, (template_path, triggered_by_change, source_paths))| {
        let mut dependent_source_paths: Vec<_> = source_paths.into_iter().collect();
        dependent_source_paths.sort_unstable();
        match read_angular_template(&template_path) {
            Ok(AngularTemplateRead::Content {
                content,
                fingerprint,
            }) => PreparedAngularTemplateUpdate {
                path: template_path.clone(),
                owner_key,
                dependent_source_paths,
                template_children: extract_custom_elements(&content),
                unavailable_reason: None,
                triggered_by_change,
                fingerprint: Some(fingerprint),
            },
            Ok(AngularTemplateRead::TooLarge { observed_size }) => {
                tracing::warn!(
                    target: "xray::parse",
                    file = %template_path.display(),
                    size = observed_size,
                    limit = MAX_PARSE_SOURCE_BYTES,
                    "skipping oversized Angular template"
                );
                PreparedAngularTemplateUpdate {
                    path: template_path.clone(),
                    owner_key,
                    dependent_source_paths,
                    template_children: Vec::new(),
                    unavailable_reason: Some(
                        "external template exceeds the source size limit".to_string(),
                    ),
                    triggered_by_change,
                    fingerprint: None,
                }
            }
            Err(_) => PreparedAngularTemplateUpdate {
                path: template_path,
                owner_key,
                dependent_source_paths,
                template_children: Vec::new(),
                unavailable_reason: Some("external template could not be read".to_string()),
                triggered_by_change,
                fingerprint: None,
            },
        }
    }).collect()
}

pub(crate) fn retain_stable_prepared_definition_inputs(
    parsed_results: &mut Vec<ParsedFileResult>,
    template_updates: &mut Vec<PreparedAngularTemplateUpdate>,
    mut rejected_keys: HashSet<String>,
) -> HashSet<String> {
    for update in template_updates.iter() {
        if rejected_keys.contains(&update.owner_key) {
            rejected_keys.extend(
                update
                    .dependent_source_paths
                    .iter()
                    .map(|path| definition_input_key(path)),
            );
        }
    }

    parsed_results.retain(|result| {
        !rejected_keys.contains(&definition_input_key(&result.path))
    });
    template_updates.retain(|update| {
        !rejected_keys.contains(&update.owner_key)
            && (update.triggered_by_change
                || update.dependent_source_paths.iter().any(|path| {
                    !rejected_keys.contains(&definition_input_key(path))
                }))
    });
    rejected_keys
}

pub(crate) fn defer_rejected_definition_paths(
    paths: &mut Vec<PathBuf>,
    rejected_keys: &HashSet<String>,
    deferred_paths: &mut HashSet<PathBuf>,
) {
    paths.retain(|path| {
        if rejected_keys.contains(&definition_input_key(path)) {
            deferred_paths.insert(path.clone());
            false
        } else {
            true
        }
    });
}

pub(crate) fn mark_pending_definition_conflicts(
    index: &mut DefinitionIndex,
    paths: &HashSet<PathBuf>,
) {
    for path in paths {
        index.pending_definition_inputs.insert(
            crate::path_identity_key(path),
            PendingDefinitionInput {
                attempts: 0,
                observed_revision: definition_input_revision(path),
            },
        );
    }
}


pub(crate) fn validate_prepared_definition_inputs(
    parsed_results: &[ParsedFileResult],
    template_updates: &[PreparedAngularTemplateUpdate],
) -> Vec<PathBuf> {
    fn unstable_source_path(result: &ParsedFileResult) -> Option<PathBuf> {
        match read_definition_source_snapshot(&result.path) {
            Ok((_, _, fingerprint)) if fingerprint == result.fingerprint => None,
            _ => Some(result.path.clone()),
        }
    }

    let unstable_sources = if parsed_results.len() <= 1 {
        parsed_results
            .iter()
            .filter_map(unstable_source_path)
            .collect::<HashSet<_>>()
    } else {
        let num_threads = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(4);
        let chunk_size = parsed_results.len().div_ceil(num_threads).max(1);
        std::thread::scope(|scope| {
            let handles: Vec<_> = parsed_results
                .chunks(chunk_size)
                .map(|chunk| {
                    scope.spawn(move || {
                        chunk
                            .iter()
                            .filter_map(unstable_source_path)
                            .collect::<HashSet<_>>()
                    })
                })
                .collect();
            let mut unstable = HashSet::new();
            let mut worker_panicked = false;
            for handle in handles {
                match handle.join() {
                    Ok(paths) => unstable.extend(paths),
                    Err(_) => worker_panicked = true,
                }
            }
            if worker_panicked {
                tracing::warn!(
                    "Definition snapshot validation worker panicked; deferring parsed sources"
                );
                parsed_results
                    .iter()
                    .map(|result| result.path.clone())
                    .collect()
            } else {
                unstable
            }
        })
    };

    #[cfg(not(feature = "lang-typescript"))]
    let unstable = unstable_sources;
    #[cfg(feature = "lang-typescript")]
    let mut unstable = unstable_sources;

    #[cfg(not(feature = "lang-typescript"))]
    let _ = template_updates;

    #[cfg(feature = "lang-typescript")]
    for update in template_updates {
        let current = match read_angular_template(&update.path) {
            Ok(AngularTemplateRead::Content { fingerprint, .. }) => {
                (Some(fingerprint), None)
            }
            Ok(AngularTemplateRead::TooLarge { .. }) => (
                None,
                Some("external template exceeds the source size limit".to_string()),
            ),
            Err(_) => (
                None,
                Some("external template could not be read".to_string()),
            ),
        };
        if current.0 != update.fingerprint || current.1 != update.unavailable_reason {
            unstable.insert(update.path.clone());
        }
    }

    let mut unstable: Vec<_> = unstable.into_iter().collect();
    unstable.sort_unstable();
    unstable
}


#[cfg(feature = "lang-typescript")]
pub(crate) fn angular_template_snapshot_matches(
    path: &Path,
    expected_fingerprint: Option<&DefinitionInputFingerprint>,
    expected_unavailable_reason: Option<&str>,
) -> bool {
    let metadata = std::fs::metadata(path);
    if let (Some(expected), Ok(metadata)) = (expected_fingerprint, &metadata)
        && definition_metadata_revision(metadata)
            == (expected.size, expected.modified_nanos)
    {
        return true;
    }
    if expected_fingerprint.is_none() {
        match (expected_unavailable_reason, &metadata) {
            (
                Some("external template exceeds the source size limit"),
                Ok(metadata),
            ) if metadata.len() > MAX_PARSE_SOURCE_BYTES as u64 => return true,
            (Some("external template could not be read"), Err(_)) => return true,
            _ => {}
        }
    }

    let current = match read_angular_template(path) {
        Ok(AngularTemplateRead::Content { fingerprint, .. }) => {
            (Some(fingerprint), None)
        }
        Ok(AngularTemplateRead::TooLarge { .. }) => (
            None,
            Some("external template exceeds the source size limit"),
        ),
        Err(_) => (None, Some("external template could not be read")),
    };
    current.0.as_ref() == expected_fingerprint
        && current.1 == expected_unavailable_reason
}

#[cfg(not(feature = "lang-typescript"))]
pub(crate) fn angular_template_snapshot_matches(
    _path: &Path,
    _expected_fingerprint: Option<&DefinitionInputFingerprint>,
    _expected_unavailable_reason: Option<&str>,
) -> bool {
    true
}


#[cfg(not(feature = "lang-typescript"))]
pub(crate) fn prepare_angular_template_updates(
    _workspace_root: &str,
    _parsed_results: &mut [ParsedFileResult],
    _changed_paths: &[PathBuf],
    _existing_owner_keys: &HashSet<String>,
) -> Vec<PreparedAngularTemplateUpdate> {
    Vec::new()
}

fn remove_angular_template_edges(index: &mut DefinitionIndex, definition_index: u32) {
    let Some(children) = index.template_children.remove(&definition_index) else {
        return;
    };
    for child in children {
        if let Some(parents) = index.template_parents.get_mut(&child) {
            parents.retain(|parent| *parent != definition_index);
            if parents.is_empty() {
                index.template_parents.remove(&child);
            }
        }
    }
}

pub(crate) fn apply_prepared_angular_template_updates(
    index: &mut DefinitionIndex,
    updates: Vec<PreparedAngularTemplateUpdate>,
) {
    for update in updates {
        let owners = index
            .template_owners
            .get(&update.owner_key)
            .cloned()
            .unwrap_or_default();
        if owners.is_empty() {
            index.input_fingerprints.remove(&update.owner_key);
            continue;
        }
        match &update.fingerprint {
            Some(fingerprint) => {
                index
                    .input_fingerprints
                    .insert(update.owner_key.clone(), fingerprint.clone());
            }
            None => {
                index.input_fingerprints.remove(&update.owner_key);
            }
        }
        for definition_index in owners {
            let relative_path = match index.angular_components.get(&definition_index) {
                Some(AngularComponentRecord {
                    template: AngularTemplateSource::External { relative_path }
                        | AngularTemplateSource::UnavailableExternal { relative_path, .. },
                    ..
                }) => relative_path.clone(),
                _ => continue,
            };

            remove_angular_template_edges(index, definition_index);
            if let Some(component) = index.angular_components.get_mut(&definition_index) {
                component.template = match &update.unavailable_reason {
                    Some(reason) => AngularTemplateSource::UnavailableExternal {
                        relative_path,
                        reason: reason.clone(),
                    },
                    None => AngularTemplateSource::External { relative_path },
                };
            }

            for child in &update.template_children {
                let parents = index.template_parents.entry(child.clone()).or_default();
                if !parents.contains(&definition_index) {
                    parents.push(definition_index);
                }
            }
            if !update.template_children.is_empty() {
                index
                    .template_children
                    .insert(definition_index, update.template_children.clone());
            }
        }
    }
}


#[cfg(feature = "lang-typescript")]
fn mark_angular_template_unavailable(
    index: &mut DefinitionIndex,
    definition_index: u32,
    relative_path: String,
    reason: &str,
) {
    if let Some(component) = index.angular_components.get_mut(&definition_index) {
        component.template = AngularTemplateSource::UnavailableExternal {
            relative_path,
            reason: reason.to_string(),
        };
    }
}


pub(crate) fn index_parsed_angular_components(
    index: &mut DefinitionIndex,
    base_def_idx: u32,
    definition_count: usize,
    records: Vec<ParsedAngularComponentRecord>,
) {
    for record in records {
        if record.local_def_index >= definition_count {
            tracing::warn!(
                target: "xray::definitions",
                local_definition_index = record.local_def_index,
                definition_count,
                "dropping Angular component record with invalid local definition index"
            );
            continue;
        }

        let definition_index = base_def_idx + record.local_def_index as u32;
        let component = record.component;
        index.angular_components.insert(definition_index, component.clone());

        let Some(definition) = index.definitions.get(definition_index as usize) else {
            continue;
        };
        if definition.kind != DefinitionKind::Class {
            continue;
        }
        #[cfg(feature = "lang-typescript")]
        let file_id = definition.file_id;

        if let StaticValue::Static(selector) = &component.selector {
            let selector = selector.to_lowercase();
            let definitions = index.name_index.entry(selector.clone()).or_default();
            if !definitions.contains(&definition_index) {
                definitions.push(definition_index);
            }
            let selectors = index.selector_index.entry(selector).or_default();
            if !selectors.contains(&definition_index) {
                selectors.push(definition_index);
            }
        }

        #[cfg(feature = "lang-typescript")]
        if let AngularTemplateSource::External { relative_path }
            | AngularTemplateSource::UnavailableExternal { relative_path, .. } = &component.template
            && let Some((_, owner_key)) = resolve_angular_template_path(
                &index.root,
                &index.files,
                file_id,
                relative_path,
            )
        {
            index.input_fingerprints.remove(&owner_key);
            let owners = index.template_owners.entry(owner_key).or_default();
            if !owners.contains(&definition_index) {
                owners.push(definition_index);
            }
        }

        for child in &record.template_children {
            let parents = index.template_parents.entry(child.clone()).or_default();
            if !parents.contains(&definition_index) {
                parents.push(definition_index);
            }
        }
        if !record.template_children.is_empty() {
            index
                .template_children
                .insert(definition_index, record.template_children);
        }
    }
}

/// Scan Angular @Component definitions for selectors and template children.
/// Populates selector_index and template_children from HTML templates.
#[cfg(feature = "lang-typescript")]
pub(crate) fn enrich_angular_templates(index: &mut DefinitionIndex) {
    let template_start = Instant::now();
    let mut templates_processed = 0usize;
    let mut templates_oversized = 0usize;
    let mut templates_failed = 0usize;

    index.selector_index.clear();
    index.template_children.clear();
    index.template_owners.clear();
    index.template_parents.clear();

    let mut components: Vec<_> = index
        .angular_components
        .iter()
        .map(|(&definition_index, component)| (definition_index, component.clone()))
        .collect();
    components.sort_unstable_by_key(|(definition_index, _)| *definition_index);

    for (definition_index, component) in components {
        let Some(definition) = index.definitions.get(definition_index as usize) else {
            continue;
        };
        let active = index
            .file_index
            .get(&definition.file_id)
            .is_some_and(|definitions| definitions.binary_search(&definition_index).is_ok());
        if !active || definition.kind != DefinitionKind::Class {
            continue;
        }
        let file_id = definition.file_id;

        if let StaticValue::Static(selector) = &component.selector {
            let selector = selector.to_lowercase();
            let definitions = index.name_index.entry(selector.clone()).or_default();
            if !definitions.contains(&definition_index) {
                definitions.push(definition_index);
            }
            index
                .selector_index
                .entry(selector)
                .or_default()
                .push(definition_index);
        }

        let template_content = match component.template {
            AngularTemplateSource::Inline { content } => Some(content),
            AngularTemplateSource::External { relative_path } => {
                let Some((template_path, owner_key)) = resolve_angular_template_path(
                    &index.root,
                    &index.files,
                    file_id,
                    &relative_path,
                ) else {
                    templates_failed += 1;
                    mark_angular_template_unavailable(
                        index,
                        definition_index,
                        relative_path,
                        "external template path is outside the workspace",
                    );
                    continue;
                };
                index
                    .template_owners
                    .entry(owner_key.clone())
                    .or_default()
                    .push(definition_index);
                index.input_fingerprints.remove(&owner_key);
                match read_angular_template(&template_path) {
                    Ok(AngularTemplateRead::Content {
                        content,
                        fingerprint,
                    }) => {
                        index.input_fingerprints.insert(owner_key, fingerprint);
                        Some(content)
                    }
                    Ok(AngularTemplateRead::TooLarge { observed_size }) => {
                        templates_oversized += 1;
                        tracing::warn!(
                            target: "xray::parse",
                            file = %template_path.display(),
                            size = observed_size,
                            limit = MAX_PARSE_SOURCE_BYTES,
                            "skipping oversized Angular template"
                        );
                        mark_angular_template_unavailable(
                            index,
                            definition_index,
                            relative_path,
                            "external template exceeds the source size limit",
                        );
                        None
                    }
                    Err(_) => {
                        templates_failed += 1;
                        mark_angular_template_unavailable(
                            index,
                            definition_index,
                            relative_path,
                            "external template could not be read",
                        );
                        None
                    }
                }
            }
            AngularTemplateSource::UnavailableExternal { .. }
            | AngularTemplateSource::Dynamic { .. }
            | AngularTemplateSource::Missing => None,
        };

        if let Some(content) = template_content {
            let children = extract_custom_elements(&content);
            for child in &children {
                index
                    .template_parents
                    .entry(child.clone())
                    .or_default()
                    .push(definition_index);
            }
            if !children.is_empty() {
                index.template_children.insert(definition_index, children);
            }
            templates_processed += 1;
        }
    }

    for values in index.selector_index.values_mut() {
        values.sort_unstable();
        values.dedup();
    }
    for values in index.template_owners.values_mut() {
        values.sort_unstable();
        values.dedup();
    }
    for values in index.template_parents.values_mut() {
        values.sort_unstable();
        values.dedup();
    }

    if templates_processed > 0 || templates_oversized > 0 || templates_failed > 0 {
        eprintln!(
            "[def-index] Angular templates: {} enriched, {} oversized, {} unavailable ({:.1}ms)",
            templates_processed,
            templates_oversized,
            templates_failed,
            template_start.elapsed().as_secs_f64() * 1000.0
        );
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
) -> usize {
    index.parse_errors += errors;
    for f in &lossy_files {
        eprintln!("[def-index] WARNING: file contains non-UTF8 bytes (lossy conversion applied): {}", f);
    }
    index.lossy_file_count += lossy_files.len();
    index.empty_file_ids.extend(empty_files);

    let mut call_sites = 0usize;
    for (
        file_id,
        file_defs,
        file_calls,
        file_stats,
        file_csharp_semantics,
        file_angular_components,
        file_fingerprint,
    ) in chunk_defs
    {
        let base_def_idx = index.definitions.len() as u32;
        let definition_count = file_defs.len();
        call_sites += index_file_defs_with_semantics(
            index,
            file_id,
            file_defs,
            file_calls,
            file_stats,
            file_csharp_semantics,
        );
        index_parsed_angular_components(
            index,
            base_def_idx,
            definition_count,
            file_angular_components,
        );
        if let Some(path) = index.files.get(file_id as usize) {
            let input_key = definition_input_key(Path::new(path));
            index.input_fingerprints.insert(input_key, file_fingerprint);
        }
    }

    index.extension_methods = index.csharp_semantics.merged_extension_methods();

    call_sites
}

#[must_use]
pub fn build_definition_index(args: &DefIndexArgs) -> DefinitionIndex {
    let dir = std::fs::canonicalize(&args.dir)
        .unwrap_or_else(|_| PathBuf::from(&args.dir));
    #[cfg(test)]
    crate::index::build_counts::record(&dir, true);
    let dir_str = clean_path(&dir.to_string_lossy());

    let extensions: Vec<String> = args.ext.split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    let start = Instant::now();

    // ─── Collect all matching source files ─────────────────────
    let collect_start = Instant::now();
    let files = collect_source_files(&dir, &extensions, args.threads, args.respect_git_exclude);
    let collect_elapsed = collect_start.elapsed();
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

    let bytes_parsed = std::sync::atomic::AtomicU64::new(0);
    let read_nanos = std::sync::atomic::AtomicU64::new(0);
    let parse_nanos = std::sync::atomic::AtomicU64::new(0);

    crate::index::log_phase("definitionsCollectFiles", &[
        ("definitionsCollectFilesMs", crate::index::format_duration_ms(collect_elapsed)),
        ("definitionsFilesDiscovered", total_files.to_string()),
        ("definitionsThreadCount", num_threads.to_string()),
    ]);

    // ─── Initialize index BEFORE chunked parsing ─────────────
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::with_capacity(files.len());
    for (file_id, file_path) in files.iter().enumerate() {
        path_to_id.insert(crate::path_identity_key(std::path::Path::new(file_path)), file_id as u32);
    }

    let mut index = DefinitionIndex {
        root: dir_str,
        format_version: types::DEFINITION_INDEX_VERSION,
        extensions,
        files,
        path_to_id,
        respect_git_exclude: args.respect_git_exclude,
        ..Default::default()
    };

    let mut total_call_sites = 0usize;
    let mut merge_elapsed = Duration::ZERO;

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
            let bytes_parsed = &bytes_parsed;
            let read_nanos = &read_nanos;
            let parse_nanos = &parse_nanos;
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
                    let mut errors = 0usize;
                    let mut lossy_files: Vec<String> = Vec::new();
                    let mut empty_files: Vec<(u32, u64)> = Vec::new();

                    for (file_id, file_path) in sub_chunk {
                        let read_start = Instant::now();
                        let (content, was_lossy, file_fingerprint) =
                            match read_definition_source_snapshot(Path::new(file_path)) {
                            Ok(r) => {
                                read_nanos.fetch_add(read_start.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
                                r
                            }
                            Err(_) => {
                                read_nanos.fetch_add(read_start.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
                                errors += 1;
                                continue;
                            }
                        };
                        if was_lossy {
                            lossy_files.push(file_path.clone());
                        }

                        let content_len = content.len() as u64;
                        bytes_parsed.fetch_add(content_len, std::sync::atomic::Ordering::Relaxed);

                        let ext = Path::new(file_path.as_str())
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("");

                        let parse_start = Instant::now();
                        #[cfg_attr(not(feature = "lang-csharp"), allow(unused_mut))]
                        let mut file_csharp_semantics = CSharpFileContribution::default();
                        #[cfg_attr(not(feature = "lang-typescript"), allow(unused_mut))]
                        let mut file_angular_components = Vec::new();
                        let (file_defs, file_calls, file_stats) = match ext.to_lowercase().as_str() {
                            #[cfg(feature = "lang-csharp")]
                            "cs" => {
                                let (defs, calls, stats, _, semantics) = parser_csharp::parse_csharp_definitions_with_semantics(&mut cs_parser, &content, *file_id);
                                file_csharp_semantics = semantics;
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
                                let (parsed, components) =
                                    parser_typescript::parse_typescript_definitions_with_components(
                                        parser,
                                        &content,
                                        *file_id,
                                    );
                                file_angular_components = components;
                                parsed
                            }
                            #[cfg(feature = "lang-typescript")]
                            "tsx" if need_tsx => {
                                let parser = tsx_parser.get_or_insert_with(|| {
                                    let mut p = tree_sitter::Parser::new();
                                    p.set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())
                                        .expect("Error loading TSX grammar");
                                    p
                                });
                                let (parsed, components) =
                                    parser_typescript::parse_typescript_definitions_with_components(
                                        parser,
                                        &content,
                                        *file_id,
                                    );
                                file_angular_components = components;
                                parsed
                            }
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
                        parse_nanos.fetch_add(parse_start.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);

                        if !file_defs.is_empty() {
                            chunk_defs.push((
                                *file_id,
                                file_defs,
                                file_calls,
                                file_stats,
                                file_csharp_semantics,
                                file_angular_components,
                                file_fingerprint,
                            ));
                        } else {
                            empty_files.push((*file_id, content_len));
                        }
                    }

                    (chunk_defs, errors, lossy_files, empty_files)
                })
            }).collect();

            // ─── Join-based streaming merge ─────────────────────
            for (sub_idx, handle) in handles.into_iter().enumerate() {
                let (chunk_defs, errors, lossy_files, empty_files) =
                    handle.join().unwrap_or_else(|_| {
                        eprintln!("[WARN] Worker thread panicked during definition index building");
                        index.worker_panics += 1;
                        (Vec::new(), 0, Vec::new(), Vec::new())
                    });

                let merge_start = Instant::now();
                let chunk_call_sites = merge_chunk_result(
                    &mut index, chunk_defs, errors, lossy_files, empty_files,
                );
                merge_elapsed = merge_elapsed.saturating_add(merge_start.elapsed());
                total_call_sites += chunk_call_sites;

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
    let enrich_start = Instant::now();
    #[cfg(feature = "lang-typescript")]
    enrich_angular_templates(&mut index);
    let enrich_elapsed = enrich_start.elapsed();

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
    let files_parsed = total_files.saturating_sub(index.parse_errors);
    let bytes_parsed = bytes_parsed.load(std::sync::atomic::Ordering::Relaxed);
    let read_elapsed = Duration::from_nanos(read_nanos.load(std::sync::atomic::Ordering::Relaxed));
    let parse_elapsed = Duration::from_nanos(parse_nanos.load(std::sync::atomic::Ordering::Relaxed));
    crate::index::log_phase("definitionsBuildComplete", &[
        ("definitionsCollectFilesMs", crate::index::format_duration_ms(collect_elapsed)),
        ("definitionsFilesDiscovered", total_files.to_string()),
        ("definitionsFilesParsed", files_parsed.to_string()),
        ("definitionsBytesParsed", bytes_parsed.to_string()),
        ("definitionsReadMs", crate::index::format_duration_ms(read_elapsed)),
        ("definitionsParseExtractMs", crate::index::format_duration_ms(parse_elapsed)),
        ("definitionsMergeMs", crate::index::format_duration_ms(merge_elapsed)),
        ("definitionsEnrichMs", crate::index::format_duration_ms(enrich_elapsed)),
        ("definitionsTotalBuildMs", crate::index::format_duration_ms(elapsed)),
        ("definitionsThreadCount", num_threads.to_string()),
        ("definitionsParseErrors", index.parse_errors.to_string()),
        ("definitionsWorkerPanics", index.worker_panics.to_string()),
        ("definitionsExtracted", index.definitions.len().to_string()),
        ("definitionsCallSites", total_call_sites.to_string()),
        ("definitionsCodeStats", index.code_stats.len().to_string()),
    ]);
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
    index.definition_generation = 1;

    index
}

/// Extract custom element tag names from HTML content.
#[cfg_attr(not(feature = "lang-typescript"), allow(dead_code))]
/// Extracts complete custom-element start tags from active HTML data context.
/// Comments, declarations, CDATA, quoted attributes, and script/style raw text are skipped.
/// Malformed inactive contexts consume the remaining input, and templates above the parser
/// source-size limit are ignored rather than producing a partial graph.
/// Returns deduplicated lowercase names in lexicographic order, excluding Angular `ng-*` tags.
pub(crate) fn extract_custom_elements(html_content: &str) -> Vec<String> {
    if html_content.len() > MAX_PARSE_SOURCE_BYTES {
        tracing::warn!(
            target: "xray::parse",
            size = html_content.len(),
            limit = MAX_PARSE_SOURCE_BYTES,
            "skipping oversized Angular template content"
        );
        return Vec::new();
    }

    #[derive(Clone, Copy)]
    enum RawTextElement {
        Script,
        Style,
    }

    #[derive(Clone, Copy)]
    enum HtmlState {
        Data,
        TagOpen {
            closing: bool,
        },
        TagName {
            start: usize,
            closing: bool,
        },
        TagBody {
            custom_tag: Option<(usize, usize)>,
            raw_text: Option<RawTextElement>,
            last_non_whitespace: Option<u8>,
        },
        SingleQuotedAttribute {
            custom_tag: Option<(usize, usize)>,
            raw_text: Option<RawTextElement>,
            last_non_whitespace: Option<u8>,
        },
        DoubleQuotedAttribute {
            custom_tag: Option<(usize, usize)>,
            raw_text: Option<RawTextElement>,
            last_non_whitespace: Option<u8>,
        },
        Comment,
        Declaration {
            quote: Option<u8>,
        },
        Cdata,
        RawText(RawTextElement),
    }

    fn raw_text_name(element: RawTextElement) -> &'static [u8] {
        match element {
            RawTextElement::Script => b"script",
            RawTextElement::Style => b"style",
        }
    }

    let mut elements: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let bytes = html_content.as_bytes();
    let len = bytes.len();
    let mut state = HtmlState::Data;
    let mut i = 0;

    while i < len {
        match state {
            HtmlState::Data => {
                if bytes[i] == b'<' {
                    state = HtmlState::TagOpen { closing: false };
                }
                i += 1;
            }
            HtmlState::TagOpen { closing } => {
                if bytes[i..].starts_with(b"!--") {
                    state = HtmlState::Comment;
                    i += 3;
                } else if bytes[i..].starts_with(b"![CDATA[") {
                    state = HtmlState::Cdata;
                    i += 8;
                } else if bytes[i] == b'!' || bytes[i] == b'?' {
                    state = HtmlState::Declaration { quote: None };
                    i += 1;
                } else if !closing && bytes[i] == b'/' {
                    state = HtmlState::TagOpen { closing: true };
                    i += 1;
                } else if bytes[i].is_ascii_alphabetic() {
                    state = HtmlState::TagName { start: i, closing };
                } else {
                    state = HtmlState::Data;
                }
            }
            HtmlState::TagName { start, closing } => {
                while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
                    i += 1;
                }

                let tag_name = &bytes[start..i];
                let custom_tag = (!closing
                    && tag_name.contains(&b'-')
                    && !tag_name
                        .get(..3)
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"ng-")))
                .then_some((start, i));
                let raw_text = if !closing && tag_name.eq_ignore_ascii_case(b"script") {
                    Some(RawTextElement::Script)
                } else if !closing && tag_name.eq_ignore_ascii_case(b"style") {
                    Some(RawTextElement::Style)
                } else {
                    None
                };

                state = HtmlState::TagBody {
                    custom_tag,
                    raw_text,
                    last_non_whitespace: tag_name.last().copied(),
                };
            }
            HtmlState::TagBody {
                custom_tag,
                raw_text,
                last_non_whitespace,
            } => match bytes[i] {
                b'<' => {
                    state = HtmlState::TagOpen { closing: false };
                    i += 1;
                }
                b'\'' => {
                    state = HtmlState::SingleQuotedAttribute {
                        custom_tag,
                        raw_text,
                        last_non_whitespace,
                    };
                    i += 1;
                }
                b'"' => {
                    state = HtmlState::DoubleQuotedAttribute {
                        custom_tag,
                        raw_text,
                        last_non_whitespace,
                    };
                    i += 1;
                }
                b'>' => {
                    if let Some((start, end)) = custom_tag {
                        let tag_lower = html_content[start..end].to_ascii_lowercase();
                        if seen.insert(tag_lower.clone()) {
                            elements.push(tag_lower);
                        }
                    }
                    state = match raw_text {
                        Some(element) if last_non_whitespace != Some(b'/') => {
                            HtmlState::RawText(element)
                        }
                        _ => HtmlState::Data,
                    };
                    i += 1;
                }
                byte if !byte.is_ascii_whitespace() => {
                    state = HtmlState::TagBody {
                        custom_tag,
                        raw_text,
                        last_non_whitespace: Some(byte),
                    };
                    i += 1;
                }
                _ => i += 1,
            },
            HtmlState::SingleQuotedAttribute {
                custom_tag,
                raw_text,
                last_non_whitespace,
            } => {
                if bytes[i] == b'\'' {
                    state = HtmlState::TagBody {
                        custom_tag,
                        raw_text,
                        last_non_whitespace,
                    };
                }
                i += 1;
            }
            HtmlState::DoubleQuotedAttribute {
                custom_tag,
                raw_text,
                last_non_whitespace,
            } => {
                if bytes[i] == b'"' {
                    state = HtmlState::TagBody {
                        custom_tag,
                        raw_text,
                        last_non_whitespace,
                    };
                }
                i += 1;
            }
            HtmlState::Comment => {
                if bytes[i..].starts_with(b"-->") {
                    state = HtmlState::Data;
                    i += 3;
                } else {
                    i += 1;
                }
            }
            HtmlState::Declaration { quote } => match quote {
                Some(delimiter) => {
                    if bytes[i] == delimiter {
                        state = HtmlState::Declaration { quote: None };
                    }
                    i += 1;
                }
                None => match bytes[i] {
                    b'\'' | b'"' => {
                        state = HtmlState::Declaration {
                            quote: Some(bytes[i]),
                        };
                        i += 1;
                    }
                    b'>' => {
                        state = HtmlState::Data;
                        i += 1;
                    }
                    _ => i += 1,
                },
            },
            HtmlState::Cdata => {
                if bytes[i..].starts_with(b"]]>") {
                    state = HtmlState::Data;
                    i += 3;
                } else {
                    i += 1;
                }
            }
            HtmlState::RawText(element) => {
                let name = raw_text_name(element);
                let name_start = i + 2;
                let name_end = name_start + name.len();
                let closes_raw_text = bytes[i] == b'<'
                    && bytes.get(i + 1) == Some(&b'/')
                    && bytes
                        .get(name_start..name_end)
                        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name))
                    && bytes.get(name_end).is_some_and(|boundary| {
                        boundary.is_ascii_whitespace() || *boundary == b'>' || *boundary == b'/'
                    });

                if closes_raw_text {
                    state = HtmlState::TagBody {
                        custom_tag: None,
                        raw_text: None,
                        last_non_whitespace: name.last().copied(),
                    };
                    i = name_end;
                } else {
                    i += 1;
                }
            }
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

#[cfg(test)]
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

