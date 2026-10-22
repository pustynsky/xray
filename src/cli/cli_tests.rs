use super::*;
use crate::index::load_content_index;
use std::collections::HashMap;
use crate::{ContentIndex, Posting, TrigramIndex};

fn save_grep_test_index(root: &Path, cache: &Path, ext: &str) -> ContentIndex {
    let index = build_content_index(&ContentIndexArgs {
        dir: root.to_string_lossy().to_string(),
        ext: ext.to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    save_content_index(&index, cache).unwrap();
    index
}

#[test]
fn test_502_normalize_query_extensions_preserves_invalid_entries() {
    assert_eq!(normalize_grep_extensions(&None), None);
    assert_eq!(normalize_grep_extensions(&Some(" RS, cs ,RS ".into())),
        Some(vec!["rs".into(), "cs".into(), "rs".into()]));
    assert_eq!(normalize_grep_extensions(&Some(".RS".into())), Some(vec![".rs".into()]));
    for ext in ["", " ", "\t", ",", ",rs", "rs,", "rs,,cs", "rs, ,cs"] {
        let normalized = normalize_grep_extensions(&Some(ext.into())).unwrap();
        assert!(normalized.iter().any(String::is_empty), "{ext:?}");
        assert!(matches!(crate::index::validate_content_extensions(&normalized), Err(SearchError::InvalidArgs(_))));
    }
}

#[test]
fn test_502_normalized_extensions_across_search_modes_preserve_cache() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("systemtemp");
    fs::create_dir_all(&base).unwrap();
    let fixture = tempfile::Builder::new().prefix("grep502-").tempdir_in(&base).unwrap();
    let fixture_root = fs::canonicalize(fixture.path()).unwrap();
    let root = fixture_root.join("sources");
    let cache = fixture_root.join("cache");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&cache).unwrap();
    let names = ["probe.RS", "probe.Cs", "probe.txt"];
    for name in names {
        fs::write(root.join(name), "scope_not_found scope_not_found alpha beta\nscope_not_found alpha beta\n").unwrap();
    }
    let original = save_grep_test_index(&root, &cache, "rs,cs,txt");
    let path = content_index_path_for(&original.root, "rs,cs,txt", &cache);
    let meta_path = path.with_extension("word-search.meta");
    let bytes = fs::read(&path).unwrap();
    let meta = fs::read(&meta_path).unwrap();
    let cache_entries = fs::read_dir(&cache).unwrap().count();
    let phrase_re = Regex::new(r"(?i)scope_not_found\s+alpha").unwrap();
    let nonmatching_phrases = [
        (vec!["alpha".into(), "scope_not_found".into()], Regex::new(r"(?i)alpha\s+scope_not_found").unwrap()),
        (vec!["scope_not_found".into(), "beta".into()], Regex::new(r"(?i)scope_not_found\s+beta").unwrap()),
    ];
    for (ext, expected_names) in [
        (Some("rs"), vec![names[0]]), (Some("cs"), vec![names[1]]),
        (Some(" RS "), vec![names[0]]), (Some("rs,cs"), vec![names[0], names[1]]),
        (Some(" CS, rS,RS "), vec![names[0], names[1]]), (None, names.to_vec()),
    ] {
        let ext = normalize_grep_extensions(&ext.map(str::to_string));
        for auto_reindex in [false, true] {
            let (index, _) = load_grep_index(&original.root, &ext, auto_reindex, false, &cache).unwrap();
            assert_eq!(index.extensions, ["rs", "cs", "txt"]);
            for (pattern, substring, regex) in [
                ("scope_not_found", false, false), ("scope", true, false), ("^scope_not_found$", false, true),
            ] {
                let terms = expand_grep_terms(&[pattern.into()], &index, substring, regex).unwrap();
                let results = score_grep_results(&index, &terms, &ext, &[], &[], grep_score_options(false, 1, false));
                let mut filenames: Vec<_> = results.iter().map(|r| Path::new(&r.file_path).file_name().unwrap().to_str().unwrap()).collect();
                filenames.sort();
                let mut expected = expected_names.clone();
                expected.sort();
                assert_eq!(filenames, expected, "{pattern}: {ext:?}");
                assert_eq!(results.iter().map(|r| r.lines.len()).sum::<usize>(), expected.len() * 2);
                assert!(results.iter().all(|r| r.lines == [1, 2] && r.occurrences == 3));
                let absent = expand_grep_terms(&["genuinely_absent".into()], &index, substring, regex).unwrap();
                assert!(score_grep_results(&index, &absent, &ext, &[], &[], grep_score_options(false, 1, false)).is_empty());
            }
            let candidates = find_phrase_candidates(&index, &["scope_not_found".into(), "alpha".into()], &ext, &[], &[]);
            assert_eq!(candidates.len(), expected_names.len());
            let (results, _) = verify_phrase_matches(&index, &candidates, &phrase_re);
            assert_eq!(results.len(), expected_names.len());
            assert!(results.iter().all(|r| r.lines == [1, 2]));
            assert!(find_phrase_candidates(&index, &["genuinely_absent".into(), "alpha".into()], &ext, &[], &[]).is_empty());
            for (tokens, regex) in &nonmatching_phrases {
                let candidates = find_phrase_candidates(&index, tokens, &ext, &[], &[]);
                assert_eq!(candidates.len(), expected_names.len());
                assert!(verify_phrase_matches(&index, &candidates, regex).0.is_empty());
            }
        }
    }
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::read(&meta_path).unwrap(), meta);
    assert_eq!(fs::read_dir(&cache).unwrap().count(), cache_entries);
}

#[test]
fn test_502_invalid_query_extensions_never_load_or_build() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("systemtemp");
    fs::create_dir_all(&base).unwrap();
    let fixture = tempfile::Builder::new().prefix("invalid502-").tempdir_in(&base).unwrap();
    let root = fs::canonicalize(fixture.path()).unwrap();
    let cache = root.join("cache");
    fs::create_dir_all(&cache).unwrap();
    let dir = root.to_string_lossy();
    for corrupt_cache in [false, true] {
        let path = content_index_path_for(&dir, "rs,cs", &cache);
        if corrupt_cache {
            fs::write(&path, b"corrupt cache").unwrap();
        }
        for ext in ["", " ", "\t", ",", ",rs", "rs,", "rs,,cs", "rs, ,cs"] {
            for auto in [false, true] {
                let normalized = normalize_grep_extensions(&Some(ext.into()));
                let err = load_grep_index(&dir, &normalized, auto, false, &cache).unwrap_err();
                assert!(matches!(err, SearchError::InvalidArgs(_)), "{ext:?}: {err}");
            }
        }
        assert_eq!(fs::read_dir(&cache).unwrap().count(), usize::from(corrupt_cache));
        if corrupt_cache {
            assert_eq!(fs::read(path).unwrap(), b"corrupt cache");
        }
    }
}

#[test]
fn test_load_grep_index_without_ext_reuses_cache_repeatedly() {
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    fs::write(root.path().join("probe.rs"), "scope_not_found\nscope_not_found\n").unwrap();
    let original = save_grep_test_index(root.path(), cache.path(), "rs");
    let path = content_index_path_for(&original.root, "rs", cache.path());
    let meta_path = path.with_extension("word-search.meta");
    let original_bytes = fs::read(&path).unwrap();
    let original_meta = fs::read(&meta_path).unwrap();
    let terms = vec!["scope_not_found".to_string()];

    for auto_reindex in [true, false] {
        for _ in 0..2 {
            let (index, _) = load_grep_index(&original.root, &None, auto_reindex, false, cache.path()).unwrap();
            assert_eq!(index.extensions, ["rs"]);
            assert_eq!(index.files.len(), 1);
            let results = score_grep_results(&index, &terms, &None, &[], &[], grep_score_options(false, 1, false));
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].occurrences, 2);
            assert_eq!(results[0].lines, [1, 2]);
        }
    }

    assert_eq!(fs::read(&path).unwrap(), original_bytes);
    assert_eq!(fs::read(&meta_path).unwrap(), original_meta);
    assert!(!content_index_path_for(&original.root, "", cache.path()).exists());
    let (explicit, _) = load_grep_index(&original.root, &Some(vec!["rs".into()]), true, false, cache.path()).unwrap();
    assert_eq!(explicit.files, original.files);
    let absent = vec!["missing_term".to_string()];
    assert!(score_grep_results(&explicit, &absent, &None, &[], &[], grep_score_options(false, 1, false)).is_empty());
}

#[test]
fn test_load_grep_index_without_ext_ignores_legacy_empty_cache() {
    for with_meta in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        fs::write(root.path().join("probe.rs"), "scope_not_found\n").unwrap();
        let original = save_grep_test_index(root.path(), cache.path(), "rs");
        let empty = ContentIndex {
            root: original.root.clone(),
            format_version: code_xray::CONTENT_INDEX_VERSION,
            extensions: vec![String::new()],
            ..Default::default()
        };
        save_content_index(&empty, cache.path()).unwrap();
        if !with_meta {
            let path = content_index_path_for(&original.root, "", cache.path());
            fs::remove_file(path.with_extension("word-search.meta")).unwrap();
        }

        for _ in 0..2 {
            let (index, _) = load_grep_index(&original.root, &None, true, false, cache.path()).unwrap();
            assert_eq!(index.extensions, ["rs"]);
            assert_eq!(index.files, original.files);
        }
    }
}

#[test]
fn test_load_grep_index_without_ext_requires_usable_cache() {
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let dir = root.path().to_string_lossy().to_string();
    for auto_reindex in [true, false] {
        let err = load_grep_index(&dir, &None, auto_reindex, false, cache.path()).unwrap_err();
        assert!(matches!(err, SearchError::IndexNotFound { .. }));
        assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
    }

    for extensions in [vec![], vec![String::new()], vec![" ".into()], vec!["rs".into(), String::new()]] {
        let empty = ContentIndex {
            root: crate::clean_path(&fs::canonicalize(root.path()).unwrap().to_string_lossy()),
            format_version: code_xray::CONTENT_INDEX_VERSION,
            extensions,
            ..Default::default()
        };
        save_content_index(&empty, cache.path()).unwrap();
        let err = load_grep_index(&dir, &None, true, false, cache.path()).unwrap_err();
        assert!(matches!(err, SearchError::IndexNotFound { .. }));
    }
}

#[test]
fn test_load_grep_index_superset_keeps_query_filter_separate() {
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    fs::write(root.path().join("probe.rs"), "scope_not_found\n").unwrap();
    fs::write(root.path().join("probe.cs"), "scope_not_found\n").unwrap();
    let original = save_grep_test_index(root.path(), cache.path(), "rs,cs");
    let query_ext = Some(vec!["rs".to_string()]);
    for auto_reindex in [true, false] {
        let (index, _) = load_grep_index(&original.root, &query_ext, auto_reindex, false, cache.path()).unwrap();
        assert_eq!(index.extensions, ["rs", "cs"]);
        assert_eq!(index.files.len(), 2);
        let terms = vec!["scope_not_found".to_string()];
        let results = score_grep_results(&index, &terms, &query_ext, &[], &[], grep_score_options(false, 1, false));
        assert_eq!(results.len(), 1);
        assert!(results[0].file_path.ends_with("probe.rs"));
        assert!(!content_index_path_for(&original.root, "rs", cache.path()).exists());
    }
}

#[test]
fn test_load_grep_index_rebuilds_selected_stale_cache_with_original_extensions() {
    for ext in [None, Some(vec!["rs".to_string()])] {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        fs::write(root.path().join("probe.rs"), "scope_not_found\n").unwrap();
        let mut original = save_grep_test_index(root.path(), cache.path(), "rs,cs");
        original.created_at = 1;
        original.max_age_secs = 3600;
        original.respect_git_exclude = true;
        save_content_index(&original, cache.path()).unwrap();
        fs::write(root.path().join("added.cs"), "scope_not_found\n").unwrap();

        let (stale, _) = load_grep_index(&original.root, &ext, false, false, cache.path()).unwrap();
        assert!(stale.is_stale());
        assert_eq!(stale.files.len(), 1);
        let (rebuilt, _) = load_grep_index(&original.root, &ext, true, false, cache.path()).unwrap();
        assert_eq!(rebuilt.extensions, ["rs", "cs"]);
        assert_eq!(rebuilt.files.len(), 2);
        assert_eq!(rebuilt.max_age_secs, 3600);
        assert!(rebuilt.respect_git_exclude);
        assert!(!rebuilt.is_stale());
        let persisted = load_content_index(&original.root, "rs,cs", cache.path()).unwrap();
        assert_eq!(persisted.files, rebuilt.files);
        assert!(!content_index_path_for(&original.root, "", cache.path()).exists());
        assert!(!content_index_path_for(&original.root, "rs", cache.path()).exists());
    }

}

#[test]
fn test_504_hidden_scope_survives_expiry() {
    fs::create_dir_all("target/systemtemp").unwrap();
    let fixture = tempfile::tempdir_in("target/systemtemp").unwrap();
    let root = fs::canonicalize(fixture.path()).unwrap();
    let cache = root.join("cache");
    let sources = root.join("sources");
    fs::create_dir_all(sources.join(".hidden")).unwrap();
    fs::create_dir_all(&cache).unwrap();
    fs::write(sources.join(".hidden").join("probe.rs"), "scope_not_found\n").unwrap();
    let dir = sources.to_string_lossy().to_string();
    let mut original = build_content_index(&ContentIndexArgs {
        dir: dir.clone(), ext: "rs".into(), max_age_hours: 1,
        hidden: true, no_ignore: true, respect_git_exclude: false,
        threads: 1, min_token_len: DEFAULT_MIN_TOKEN_LEN,
    }).unwrap();
    assert_eq!(original.files.len(), 1);
    original.created_at = 1;
    save_content_index(&original, &cache).unwrap();
    let (rebuilt, _) = load_grep_index(&dir, &Some(vec!["rs".into()]), true, false, &cache).unwrap();
    assert_eq!(rebuilt.files.len(), 1, "expiry must not discard hidden scope");
}

fn traversal_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    fs::create_dir_all("target/systemtemp").unwrap();
    let fixture = tempfile::Builder::new().prefix("scope504-").tempdir_in("target/systemtemp").unwrap();
    let base = fs::canonicalize(fixture.path()).unwrap();
    let sources = base.join("sources");
    let cache = base.join("cache");
    fs::create_dir_all(&sources).unwrap();
    fs::create_dir_all(&cache).unwrap();
    let git = crate::background_command("git")
        .args(["-c", "init.defaultBranch=main", "init", "--quiet"])
        .arg(&sources).output().unwrap();
    assert!(git.status.success(), "{}", String::from_utf8_lossy(&git.stderr));
    fs::write(sources.join(".gitignore"), "ignored/\n").unwrap();
    fs::write(sources.join(".git").join("info").join("exclude"), "excluded/\n").unwrap();
    (fixture, sources, cache)
}

fn write_traversal_probes(root: &Path, prefix: &str) {
    for scope in ["ordinary", ".hidden", "ignored", "excluded"] {
        fs::create_dir_all(root.join(scope)).unwrap();
        for ext in ["rs", "cs"] {
            fs::write(root.join(scope).join(format!("{prefix}_probe.{ext}")), "scope_not_found\n").unwrap();
        }
    }
}

fn probe_paths<'a>(paths: impl Iterator<Item = &'a str>) -> std::collections::BTreeSet<String> {
    paths.filter(|path| path.contains("_probe."))
        .map(|path| path.replace('\\', "/")).collect()
}

fn expected_probe_paths(root: &Path, prefixes: &[&str], hidden: bool, no_ignore: bool, respect: bool) -> std::collections::BTreeSet<String> {
    let mut expected = std::collections::BTreeSet::new();
    for (scope, included) in [("ordinary", true), (".hidden", hidden), ("ignored", no_ignore), ("excluded", !respect)] {
        if included {
            for prefix in prefixes {
                for ext in ["rs", "cs"] {
                    expected.insert(crate::clean_path(&root.join(scope).join(format!("{prefix}_probe.{ext}")).to_string_lossy()));
                }
            }
        }
    }
    expected
}

#[test]
fn test_504_traversal_options_matrix_file_and_sharded_content() {
    for hidden in [false, true] {
        for no_ignore in [false, true] {
            for respect in [false, true] {
                let (_fixture, root, cache) = traversal_fixture();
                let dir = root.to_string_lossy().to_string();
                write_traversal_probes(&root, "initial");
                let mut file = build_index(&IndexArgs {
                    dir: dir.clone(), max_age_hours: 1, hidden, no_ignore,
                    respect_git_exclude: respect, threads: 1,
                }).unwrap();
                let mut content = build_content_index(&ContentIndexArgs {
                    dir: dir.clone(), ext: "rs,cs".into(), max_age_hours: 1,
                    hidden, no_ignore, respect_git_exclude: respect,
                    threads: 1, min_token_len: DEFAULT_MIN_TOKEN_LEN,
                }).unwrap();
                let before = expected_probe_paths(&root, &["initial"], hidden, no_ignore, respect);
                assert_eq!(probe_paths(file.entries.iter().map(|entry| entry.path.as_str())), before);
                assert_eq!(probe_paths(content.files.iter().map(String::as_str)), before);
                assert_eq!((file.hidden, file.no_ignore, file.respect_git_exclude), (hidden, no_ignore, respect));
                assert_eq!((content.hidden, content.no_ignore, content.respect_git_exclude), (hidden, no_ignore, respect));
                let cloned = content.clone();
                let head = content.build_head();
                assert_eq!((cloned.hidden, cloned.no_ignore, cloned.respect_git_exclude), (hidden, no_ignore, respect));
                assert_eq!((head.hidden, head.no_ignore, head.respect_git_exclude), (hidden, no_ignore, respect));
                let binary: FileIndex = bincode::deserialize(&bincode::serialize(&file).unwrap()).unwrap();
                assert_eq!((binary.hidden, binary.no_ignore, binary.respect_git_exclude), (hidden, no_ignore, respect));
                file.created_at = 1;
                content.created_at = 1;
                save_index(&file, &cache).unwrap();
                save_content_index(&content, &cache).unwrap();
                let content_path = content_index_path_for(&dir, "rs,cs", &cache);
                assert_eq!(&fs::read(&content_path).unwrap()[..4], crate::index::SHARD_MAGIC);
                assert_eq!(crate::index::read_format_version_from_index_file(&content_path), Some(5));
                assert_eq!(crate::index::read_format_version_from_index_file(&index_path_for(&dir, &cache)), Some(3));
                fs::remove_file(sidecar_path(&content_path)).unwrap();
                fs::remove_file(sidecar_path(&index_path_for(&dir, &cache))).unwrap();
                write_traversal_probes(&root, "added");
                let stale_file = load_fast_index(&dir, false, !respect, &cache).unwrap();
                let (stale_content, _) = load_grep_index(&dir, &Some(vec!["rs".into()]), false, !respect, &cache).unwrap();
                assert!(stale_file.is_stale() && stale_content.is_stale());
                assert_eq!(probe_paths(stale_file.entries.iter().map(|entry| entry.path.as_str())), before);
                assert_eq!(probe_paths(stale_content.files.iter().map(String::as_str)), before);
                let after = expected_probe_paths(&root, &["initial", "added"], hidden, no_ignore, respect);
                for query_ext in [normalize_grep_extensions(&Some(" RS, rs ".into())), None] {
                    let rebuilt_file = load_fast_index(&dir, true, !respect, &cache).unwrap();
                    let (rebuilt_content, _) = load_grep_index(&dir, &query_ext, true, !respect, &cache).unwrap();
                    let disk_file = load_index(&dir, &cache).unwrap();
                    let disk_content = load_content_index(&dir, "rs,cs", &cache).unwrap();
                    for selected in [&rebuilt_file, &disk_file] {
                        assert_eq!((selected.hidden, selected.no_ignore, selected.respect_git_exclude), (hidden, no_ignore, respect));
                        assert_eq!(selected.max_age_secs, 3600);
                        assert!(!selected.is_stale());
                        assert_eq!(probe_paths(selected.entries.iter().map(|entry| entry.path.as_str())), after);
                    }
                    for selected in [&rebuilt_content, &disk_content] {
                        assert_eq!((selected.hidden, selected.no_ignore, selected.respect_git_exclude), (hidden, no_ignore, respect));
                        assert_eq!(selected.extensions, ["rs", "cs"]);
                        assert_eq!(selected.max_age_secs, 3600);
                        assert!(!selected.is_stale());
                        assert_eq!(probe_paths(selected.files.iter().map(String::as_str)), after);
                        assert_eq!(selected.index["scope_not_found"].len(), after.len());
                        let results = score_grep_results(
                            selected, &["scope_not_found".into()], &query_ext, &[], &[],
                            grep_score_options(false, 1, false),
                        );
                        assert_eq!(results.len(), if query_ext.is_some() { after.len() / 2 } else { after.len() });
                        if query_ext.is_some() {
                            assert!(results.iter().all(|result| result.file_path.ends_with(".rs")));
                        }
                    }
                    assert!(!content_index_path_for(&dir, "rs", &cache).exists());
                }
            }
        }
    }
}

#[test]
fn test_504_true_missing_and_valid_empty_scopes() {
    let (_fixture, root, cache) = traversal_fixture();
    let dir = root.to_string_lossy().to_string();
    assert!(load_fast_index(&dir, true, false, &cache).is_ok());
    assert!(matches!(load_grep_index(&dir, &None, true, false, &cache), Err(SearchError::IndexNotFound { .. })));
    let (content, _) = load_grep_index(&dir, &Some(vec!["rs".into()]), true, false, &cache).unwrap();
    assert!(content.files.is_empty());
    for auto in [false, true] {
        let (content, _) = load_grep_index(&dir, &None, auto, false, &cache).unwrap();
        assert!(content.files.is_empty());
    }
    for ext in ["", ",", "rs,"] {
        assert!(load_grep_index(&dir, &normalize_grep_extensions(&Some(ext.into())), true, false, &cache).is_err());
    }
}

fn sidecar_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".meta");
    name.into()
}

#[test]
fn test_504_existing_legacy_and_corrupt_caches_are_not_overwritten() {
    for legacy in [false, true] {
        for sidecars in [false, true] {
            let (_fixture, root, cache) = traversal_fixture();
            let dir = root.to_string_lossy().to_string();
            let file = build_index(&IndexArgs {
                dir: dir.clone(), max_age_hours: 1, hidden: true, no_ignore: true,
                respect_git_exclude: true, threads: 1,
            }).unwrap();
            let content = save_grep_test_index(&root, &cache, "rs,cs");
            save_index(&file, &cache).unwrap();
            let file_path = index_path_for(&dir, &cache);
            let content_path = content_index_path_for(&dir, "rs,cs", &cache);
            if legacy {
                // Real v2 file layout and v4 production sharded head, without
                // the new traversal fields (not merely new structs restamped).
                crate::index::save_compressed(&file_path, &(
                    &file.root, 2u32, file.created_at, file.max_age_secs,
                    &file.entries, file.respect_git_exclude,
                ), "legacy-file").unwrap();
                let head = (
                    &content.root, 4u32, content.created_at, content.max_age_secs,
                    &content.files, content.total_tokens, &content.extensions,
                    &content.file_token_counts, &content.trigram, content.trigram_dirty,
                    &content.path_to_id, content.read_errors, content.lossy_file_count,
                    content.worker_panics, content.respect_git_exclude,
                );
                crate::index::save_sharded(&content_path, &head, Vec::<(String, Vec<Posting>)>::new(), "legacy-content").unwrap();
                assert_eq!(crate::index::read_format_version_from_index_file(&file_path), Some(2));
                assert_eq!(crate::index::read_format_version_from_index_file(&content_path), Some(4));
            } else {
                fs::write(&file_path, b"corrupt").unwrap();
                // Retain root/version attribution, but leave the payload corrupt.
                fs::write(&content_path, bincode::serialize(&(&content.root, code_xray::CONTENT_INDEX_VERSION)).unwrap()).unwrap();
            }
            if !sidecars {
                fs::remove_file(sidecar_path(&file_path)).unwrap();
                fs::remove_file(sidecar_path(&content_path)).unwrap();
            }
            let paths = [&file_path, &content_path];
            let evidence: Vec<_> = paths.iter().map(|path|
                (fs::read(path).unwrap(), fs::read(sidecar_path(path)).ok())
            ).collect();
            for auto in [false, true] {
                let file_err = load_fast_index(&dir, auto, false, &cache).unwrap_err().to_string();
                assert!(file_err.contains("xray index") && file_err.contains("--hidden") && file_err.contains("--no-ignore"));
                for ext in [None, Some(vec!["rs".into()]), Some(vec!["rs".into(), "cs".into()])] {
                    let err = load_grep_index(&dir, &ext, auto, false, &cache).unwrap_err().to_string();
                    assert!(err.contains("xray content-index") && err.contains("--hidden") && err.contains("--no-ignore"), "{err}");
                }
            }
            for (path, (bytes, meta)) in paths.into_iter().zip(evidence) {
                assert_eq!(fs::read(path).unwrap(), bytes);
                assert_eq!(fs::read(sidecar_path(path)).ok(), meta);
            }
        }
    }
}

#[test]
fn test_504_bad_exact_good_superset_and_unattributable_alternates() {
    for sidecars in [false, true] {
        let (_fixture, root, cache) = traversal_fixture();
        let dir = root.to_string_lossy().to_string();
        write_traversal_probes(&root, "initial");
        let exact = save_grep_test_index(&root, &cache, "rs");
        let superset = save_grep_test_index(&root, &cache, "rs,cs");
        let exact_path = content_index_path_for(&dir, "rs", &cache);
        let superset_path = content_index_path_for(&dir, "rs,cs", &cache);
        fs::write(&exact_path, b"unreadable root and payload").unwrap();
        if !sidecars {
            fs::remove_file(sidecar_path(&exact_path)).unwrap();
            fs::remove_file(sidecar_path(&superset_path)).unwrap();
        }
        let bad = fs::read(&exact_path).unwrap();
        let (selected, _) = load_grep_index(&dir, &Some(vec!["rs".into()]), true, false, &cache).unwrap();
        assert_eq!(selected.extensions, superset.extensions);
        assert_eq!(fs::read(&exact_path).unwrap(), bad);
        let (selected, _) = load_grep_index(&dir, &None, true, false, &cache).unwrap();
        assert_eq!(selected.extensions, ["rs", "cs"]);
        fs::remove_file(&superset_path).unwrap();
        if sidecars {
            fs::remove_file(sidecar_path(&superset_path)).unwrap();
        }
        let err = load_grep_index(&dir, &Some(vec!["rs".into()]), true, false, &cache).unwrap_err().to_string();
        assert!(err.contains(&exact_path.display().to_string()), "{err}");
        let no_ext = load_grep_index(&dir, &None, true, false, &cache).unwrap_err();
        if sidecars {
            assert!(no_ext.to_string().contains("Refusing to replace"));
        } else {
            assert!(matches!(no_ext, SearchError::IndexNotFound { .. }));
        }
        assert_eq!(exact.extensions, ["rs"]);
        fs::write(&exact_path, bincode::serialize(&("a different root", code_xray::CONTENT_INDEX_VERSION)).unwrap()).unwrap();
        let wrong_root = fs::read(&exact_path).unwrap();
        let err = load_grep_index(&dir, &Some(vec!["rs".into()]), true, false, &cache).unwrap_err().to_string();
        assert!(err.contains("exact cache key has a different root"), "{err}");
        assert_eq!(fs::read(&exact_path).unwrap(), wrong_root);

        // Unreadable alternates without attribution do not belong to this
        // root. An unrelated root with a readable legacy header also cannot
        // poison it.
        fs::remove_file(&exact_path).unwrap();
        if sidecars {
            fs::remove_file(sidecar_path(&exact_path)).unwrap();
        }
        fs::write(cache.join("unknown.word-search"), b"rootless").unwrap();
        let foreign = root.parent().unwrap().join("other-root");
        let foreign_path = content_index_path_for(&foreign.to_string_lossy(), "rs", &cache);
        fs::write(&foreign_path, bincode::serialize(&(foreign.to_string_lossy().to_string(), 4u32)).unwrap()).unwrap();
        assert!(find_content_index_for_dir(&dir, &cache, &[]).unwrap().is_none());
        let (fresh, _) = load_grep_index(&dir, &Some(vec!["rs".into()]), true, false, &cache).unwrap();
        assert!(!fresh.files.is_empty());
    }
}

#[test]
fn test_load_grep_index_builds_only_with_explicit_valid_extensions() {
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let dir = root.path().to_string_lossy().to_string();
    fs::write(root.path().join("probe.rs"), "scope_not_found\n").unwrap();
    for ext in ["", " ", ",", "rs,", ",rs", "rs, ,cs"] {
        let err = load_grep_index(&dir, &normalize_grep_extensions(&Some(ext.into())), true, false, cache.path()).unwrap_err();
        assert!(matches!(err, SearchError::InvalidArgs(_)), "{ext:?}: {err}");
    }
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
    let (index, _) = load_grep_index(&dir, &Some(vec!["rs".into()]), true, false, cache.path()).unwrap();
    assert_eq!(index.files.len(), 1);
    assert_eq!(load_content_index(&dir, "rs", cache.path()).unwrap().files, index.files);
}

#[test]
fn test_load_grep_index_does_not_use_unrelated_extensions() {
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    fs::write(root.path().join("probe.cs"), "scope_not_found\n").unwrap();
    let original = save_grep_test_index(root.path(), cache.path(), "cs");
    let ext = Some(vec!["rs".into()]);
    assert!(load_grep_index(&original.root, &ext, false, false, cache.path()).is_err());
    let (empty, _) = load_grep_index(&original.root, &ext, true, false, cache.path()).unwrap();
    assert_eq!(empty.extensions, ["rs"]);
    assert!(empty.files.is_empty());
    let (persisted, _) = load_grep_index(&original.root, &ext, true, false, cache.path()).unwrap();
    assert!(persisted.files.is_empty());
}

// ─── Helper: build a minimal ContentIndex from known data ────────

/// Build a test ContentIndex with given files, tokens→postings, and token counts.
/// Also builds a valid TrigramIndex from the token set.
fn make_test_index(
    files: Vec<&str>,
    postings: Vec<(&str, Vec<Posting>)>,
    file_token_counts: Vec<u32>,
) -> ContentIndex {
    let files: Vec<String> = files.into_iter().map(|s| s.to_string()).collect();
    let mut index: HashMap<String, Vec<Posting>> = HashMap::new();
    for (token, posts) in postings {
        index.insert(token.to_string(), posts);
    }

    // Build trigram index from the token keys
    let mut tokens: Vec<String> = index.keys().cloned().collect();
    tokens.sort();
    let mut trigram_map: HashMap<String, Vec<u32>> = HashMap::new();
    for (idx, token) in tokens.iter().enumerate() {
        for tri in code_xray::generate_trigrams(token) {
            trigram_map.entry(tri).or_default().push(idx as u32);
        }
    }

    ContentIndex {
        root: ".".to_string(),
        files,
        index,
        file_token_counts,
        trigram: TrigramIndex { tokens, trigram_map },
        ..Default::default()
    }
}

// ═══════════════════════════════════════════════════════════════════
//  file_matches_filters() tests — pure predicate, no index needed
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_file_matches_filters_no_filters() {
    // No filters → everything passes
    assert!(file_matches_filters("src/main.cs", &None, &[], &[]));
}

#[test]
fn test_file_matches_filters_ext_match() {
    let ext = Some(vec!["cs".to_string()]);
    assert!(file_matches_filters("src/Service.cs", &ext, &[], &[]));
}

#[test]
fn test_file_matches_filters_ext_no_match() {
    let ext = Some(vec!["cs".to_string()]);
    assert!(!file_matches_filters("src/main.rs", &ext, &[], &[]));
}

#[test]
fn test_file_matches_filters_ext_case_insensitive() {
    let ext = Some(vec!["cs".to_string()]);
    assert!(file_matches_filters("src/Service.CS", &ext, &[], &[]));
    assert!(file_matches_filters("src/Service.Cs", &ext, &[], &[]));
}

#[test]
fn test_file_matches_filters_exclude_dir() {
    let exclude_dir = vec!["tests".to_string()];
    assert!(!file_matches_filters("src/tests/unit.cs", &None, &exclude_dir, &[]));
    assert!(file_matches_filters("src/main.cs", &None, &exclude_dir, &[]));
}

#[test]
fn test_file_matches_filters_exclude_dir_case_insensitive() {
    let exclude_dir = vec!["Tests".to_string()];
    assert!(!file_matches_filters("src/tests/unit.cs", &None, &exclude_dir, &[]));
    assert!(!file_matches_filters("src/TESTS/unit.cs", &None, &exclude_dir, &[]));
}

#[test]
fn test_file_matches_filters_exclude_pattern() {
    let exclude = vec!["mock".to_string()];
    assert!(!file_matches_filters("src/ServiceMock.cs", &None, &[], &exclude));
    assert!(file_matches_filters("src/Service.cs", &None, &[], &exclude));
}

#[test]
fn test_file_matches_filters_exclude_pattern_case_insensitive() {
    let exclude = vec!["Mock".to_string()];
    assert!(!file_matches_filters("src/servicemock.cs", &None, &[], &exclude));
}

#[test]
fn test_file_matches_filters_multiple_excludes() {
    let exclude_dir = vec!["tests".to_string()];
    let exclude = vec!["mock".to_string()];
    // Excluded by dir
    assert!(!file_matches_filters("tests/unit.cs", &None, &exclude_dir, &exclude));
    // Excluded by pattern
    assert!(!file_matches_filters("src/ServiceMock.cs", &None, &exclude_dir, &exclude));
    // Passes both
    assert!(file_matches_filters("src/Service.cs", &None, &exclude_dir, &exclude));
}

#[test]
fn test_file_matches_filters_combined_ext_and_exclude() {
    let ext = Some(vec!["cs".to_string()]);
    let exclude_dir = vec!["tests".to_string()];
    // Wrong extension
    assert!(!file_matches_filters("src/main.rs", &ext, &exclude_dir, &[]));
    // Right extension but excluded dir
    assert!(!file_matches_filters("tests/unit.cs", &ext, &exclude_dir, &[]));
    // Passes both
    assert!(file_matches_filters("src/Service.cs", &ext, &exclude_dir, &[]));
}

#[test]
fn test_file_matches_filters_no_extension_file() {
    let ext = Some(vec!["cs".to_string()]);
    // File with no extension should not match ext filter
    assert!(!file_matches_filters("Makefile", &ext, &[], &[]));
}

#[test]
fn test_grep_path_separators_across_scoring_and_phrase_candidates() {
    for prefix in ["C:/workspace/src/", r"C:\workspace\src\", r"C:\workspace/src\"] {
        let excluded = format!("{prefix}handlers/Target.cs");
        let kept = format!("{prefix}Keep.cs");
        let index = make_test_index(
            vec![&excluded, &kept],
            vec![("pathseparatorneedle", vec![
                Posting { file_id: 0, lines: vec![1] },
                Posting { file_id: 1, lines: vec![1] },
            ])],
            vec![1, 1],
        );
        let terms = vec!["pathseparatorneedle".to_string()];
        for filter in [
            "src/handlers", r"src\handlers", r"src/handlers\Target.cs",
            "C:/workspace/src/handlers/Target.cs",
            r"C:\workspace\src\handlers\Target.cs",
            r"C:\workspace/src\handlers/Target.cs",
        ] {
            let filters = vec!["missing/path".to_string(), filter.to_uppercase()];
            for (exclude_dir, exclude) in [
                (filters.as_slice(), &[][..]),
                (&[][..], filters.as_slice()),
            ] {
                let paths: Vec<_> = index.files.iter()
                    .filter(|path| file_matches_filters(path, &None, exclude_dir, exclude))
                    .collect();
                assert_eq!(paths, vec![&kept], "prefix={prefix}, filter={filter}");
                let scored = score_grep_results(
                    &index, &terms, &None, exclude_dir, exclude,
                    grep_score_options(false, 1, false),
                );
                assert_eq!(
                    scored.iter().map(|entry| &entry.file_path).collect::<Vec<_>>(),
                    vec![&kept],
                    "prefix={prefix}, filter={filter}",
                );
                let candidates = find_phrase_candidates(
                    &index, &terms, &None, exclude_dir, exclude,
                );
                assert_eq!(candidates, std::collections::HashSet::from([1]));
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
//  expand_regex_terms() tests
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_expand_regex_terms_matches_tokens() {
    let mut index_keys: HashMap<String, Vec<Posting>> = HashMap::new();
    index_keys.insert("httpclient".to_string(), vec![]);
    index_keys.insert("httphandler".to_string(), vec![]);
    index_keys.insert("ilogger".to_string(), vec![]);
    index_keys.insert("icache".to_string(), vec![]);

    let terms = vec!["http.*".to_string()];
    let result = expand_regex_terms(&terms, &index_keys).unwrap();
    assert_eq!(result.len(), 2);
    let mut sorted = result.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["httpclient", "httphandler"]);
}

#[test]
fn test_expand_regex_terms_no_match() {
    let mut index_keys: HashMap<String, Vec<Posting>> = HashMap::new();
    index_keys.insert("httpclient".to_string(), vec![]);

    let terms = vec!["zzzznonexistent".to_string()];
    let result = expand_regex_terms(&terms, &index_keys).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_expand_regex_terms_invalid_regex() {
    let index_keys: HashMap<String, Vec<Posting>> = HashMap::new();
    let terms = vec!["[invalid".to_string()];
    let result = expand_regex_terms(&terms, &index_keys);
    assert!(result.is_err());
}

#[test]
fn test_expand_regex_terms_multiple_patterns() {
    let mut index_keys: HashMap<String, Vec<Posting>> = HashMap::new();
    index_keys.insert("httpclient".to_string(), vec![]);
    index_keys.insert("ilogger".to_string(), vec![]);
    index_keys.insert("icache".to_string(), vec![]);

    let terms = vec!["http.*".to_string(), "i.*".to_string()];
    let result = expand_regex_terms(&terms, &index_keys).unwrap();
    // http.* matches httpclient; i.* matches ilogger, icache
    assert_eq!(result.len(), 3);
}

#[test]
fn test_expand_regex_terms_preserves_overlapping_pattern_duplicates() {
    let mut index_keys: HashMap<String, Vec<Posting>> = HashMap::new();
    index_keys.insert("userservice".to_string(), vec![]);

    let terms = vec!["user.*".to_string(), ".*service".to_string()];
    let result = expand_regex_terms(&terms, &index_keys).unwrap();

    assert_eq!(result, vec!["userservice", "userservice"]);
}

#[test]
fn test_expand_regex_terms_case_insensitive() {
    let mut index_keys: HashMap<String, Vec<Posting>> = HashMap::new();
    index_keys.insert("httpclient".to_string(), vec![]);

    // Pattern is uppercase but matching is case-insensitive ((?i) is added)
    let terms = vec!["HTTPCLIENT".to_string()];
    let result = expand_regex_terms(&terms, &index_keys).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "httpclient");
}

// ═══════════════════════════════════════════════════════════════════
//  expand_substring_terms() tests
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_expand_substring_terms_matches_via_trigram() {
    // Build a TrigramIndex with known tokens
    let tokens = vec![
        "httpclient".to_string(),
        "httphandler".to_string(),
        "ilogger".to_string(),
    ];
    let mut trigram_map: HashMap<String, Vec<u32>> = HashMap::new();
    for (idx, token) in tokens.iter().enumerate() {
        for tri in code_xray::generate_trigrams(token) {
            trigram_map.entry(tri).or_default().push(idx as u32);
        }
    }
    let trigram_idx = TrigramIndex { tokens, trigram_map };

    // "client" is a substring of "httpclient" only
    let terms = vec!["client".to_string()];
    let result = expand_substring_terms(&terms, &trigram_idx);
    assert_eq!(result, vec!["httpclient"]);
}

#[test]
fn test_expand_substring_terms_matches_multiple() {
    let tokens = vec![
        "httpclient".to_string(),
        "httphandler".to_string(),
        "ilogger".to_string(),
    ];
    let mut trigram_map: HashMap<String, Vec<u32>> = HashMap::new();
    for (idx, token) in tokens.iter().enumerate() {
        for tri in code_xray::generate_trigrams(token) {
            trigram_map.entry(tri).or_default().push(idx as u32);
        }
    }
    let trigram_idx = TrigramIndex { tokens, trigram_map };

    // "http" is a substring of both httpclient and httphandler
    let terms = vec!["http".to_string()];
    let result = expand_substring_terms(&terms, &trigram_idx);
    let mut sorted = result.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["httpclient", "httphandler"]);
}

#[test]
fn test_expand_substring_terms_no_match() {
    let tokens = vec!["httpclient".to_string()];
    let mut trigram_map: HashMap<String, Vec<u32>> = HashMap::new();
    for (idx, token) in tokens.iter().enumerate() {
        for tri in code_xray::generate_trigrams(token) {
            trigram_map.entry(tri).or_default().push(idx as u32);
        }
    }
    let trigram_idx = TrigramIndex { tokens, trigram_map };

    let terms = vec!["zzzzz".to_string()];
    let result = expand_substring_terms(&terms, &trigram_idx);
    assert!(result.is_empty());
}

#[test]
fn test_expand_substring_terms_short_term_linear_scan() {
    // Terms shorter than 3 chars use linear scan (no trigrams)
    let tokens = vec![
        "ax".to_string(),
        "bx".to_string(),
        "abc".to_string(),
    ];
    let trigram_map: HashMap<String, Vec<u32>> = HashMap::new(); // empty — short terms don't use it
    let trigram_idx = TrigramIndex { tokens, trigram_map };

    // "ax" should match "ax" via linear scan
    let terms = vec!["ax".to_string()];
    let result = expand_substring_terms(&terms, &trigram_idx);
    assert_eq!(result, vec!["ax"]);
}

#[test]
fn test_expand_substring_terms_unicode_short_term_linear_scan() {
    let tokens = vec!["中".to_string(), "中文".to_string(), "abc".to_string()];
    let trigram_idx = TrigramIndex { tokens, trigram_map: HashMap::new() };

    let result = expand_substring_terms(&["中".to_string()], &trigram_idx);

    assert_eq!(result, vec!["中", "中文"]);
}

// ═══════════════════════════════════════════════════════════════════
//  expand_grep_terms() tests — routing logic
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_expand_grep_terms_exact_mode() {
    let index = make_test_index(
        vec!["file.cs"],
        vec![("httpclient", vec![Posting { file_id: 0, lines: vec![1] }])],
        vec![10],
    );
    // Exact mode preserves first-seen order without duplicate weighting.
    let raw = vec![
        "myterm".to_string(),
        "myterm".to_string(),
        "other".to_string(),
    ];
    let result = expand_grep_terms(&raw, &index, false, false).unwrap();
    assert_eq!(result, vec!["myterm", "other"]);
}

#[test]
fn test_expand_grep_terms_substring_mode() {
    let index = make_test_index(
        vec!["file.cs"],
        vec![
            ("httpclient", vec![Posting { file_id: 0, lines: vec![1] }]),
            ("httphandler", vec![Posting { file_id: 0, lines: vec![2] }]),
        ],
        vec![10],
    );
    // Substring mode: "http" should expand to matching tokens
    let raw = vec!["http".to_string()];
    let result = expand_grep_terms(&raw, &index, true, false).unwrap();
    let mut sorted = result.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["httpclient", "httphandler"]);
}

#[test]
fn test_expand_grep_terms_regex_mode() {
    let index = make_test_index(
        vec!["file.cs"],
        vec![
            ("httpclient", vec![Posting { file_id: 0, lines: vec![1] }]),
            ("ilogger", vec![Posting { file_id: 0, lines: vec![2] }]),
        ],
        vec![10],
    );
    // Regex mode: "http.*" should match httpclient
    let raw = vec!["http.*".to_string()];
    let result = expand_grep_terms(&raw, &index, false, true).unwrap();
    assert_eq!(result, vec!["httpclient"]);
}

#[test]
fn test_expand_grep_terms_regex_invalid() {
    let index = make_test_index(vec!["file.cs"], vec![], vec![10]);
    let raw = vec!["[invalid".to_string()];
    let result = expand_grep_terms(&raw, &index, false, true);
    assert!(result.is_err());
}

// ═══════════════════════════════════════════════════════════════════
//  find_phrase_candidates() tests
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_find_phrase_candidates_all_tokens_found() {
    let index = make_test_index(
        vec!["src/a.cs", "src/b.cs"],
        vec![
            ("new", vec![
                Posting { file_id: 0, lines: vec![1] },
                Posting { file_id: 1, lines: vec![1] },
            ]),
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1] },
            ]),
        ],
        vec![10, 10],
    );
    // "new" is in files 0,1. "httpclient" is in file 0 only.
    // Intersection = {0}
    let tokens = vec!["new".to_string(), "httpclient".to_string()];
    let result = find_phrase_candidates(&index, &tokens, &None, &[], &[]);
    assert_eq!(result.len(), 1);
    assert!(result.contains(&0));
}

#[test]
fn test_find_phrase_candidates_requires_tokens_on_same_line() {
    let index = make_test_index(
        vec!["src/a.cs"],
        vec![
            ("new", vec![Posting { file_id: 0, lines: vec![1] }]),
            ("httpclient", vec![Posting { file_id: 0, lines: vec![2] }]),
        ],
        vec![10],
    );
    let tokens = vec!["new".to_string(), "httpclient".to_string()];
    let result = find_phrase_candidates(&index, &tokens, &None, &[], &[]);
    assert!(result.is_empty());
}

#[test]
fn test_find_phrase_candidates_includes_token_prefix_matches() {
    let index = make_test_index(
        vec!["src/a.cs"],
        vec![
            ("public", vec![Posting { file_id: 0, lines: vec![1] }]),
            ("classbuilder", vec![Posting { file_id: 0, lines: vec![1] }]),
        ],
        vec![10],
    );
    let tokens = vec!["public".to_string(), "class".to_string()];
    let result = find_phrase_candidates(&index, &tokens, &None, &[], &[]);
    assert!(result.contains(&0));
}

#[test]
fn test_find_phrase_candidates_token_missing() {
    let index = make_test_index(
        vec!["src/a.cs"],
        vec![
            ("new", vec![Posting { file_id: 0, lines: vec![1] }]),
        ],
        vec![10],
    );
    // "httpclient" is not in the index → empty result
    let tokens = vec!["new".to_string(), "httpclient".to_string()];
    let result = find_phrase_candidates(&index, &tokens, &None, &[], &[]);
    assert!(result.is_empty());
}

#[test]
fn test_find_phrase_candidates_with_ext_filter() {
    let index = make_test_index(
        vec!["src/a.cs", "src/b.rs"],
        vec![
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1] },
                Posting { file_id: 1, lines: vec![1] },
            ]),
        ],
        vec![10, 10],
    );
    // Filter to .cs only
    let ext = Some(vec!["cs".to_string()]);
    let tokens = vec!["httpclient".to_string()];
    let result = find_phrase_candidates(&index, &tokens, &ext, &[], &[]);
    assert_eq!(result.len(), 1);
    assert!(result.contains(&0));
}

#[test]
fn test_find_phrase_candidates_with_exclude_dir() {
    let index = make_test_index(
        vec!["src/Service.cs", "tests/ServiceTest.cs"],
        vec![
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1] },
                Posting { file_id: 1, lines: vec![1] },
            ]),
        ],
        vec![10, 10],
    );
    let exclude_dir = vec!["tests".to_string()];
    let tokens = vec!["httpclient".to_string()];
    let result = find_phrase_candidates(&index, &tokens, &None, &exclude_dir, &[]);
    assert_eq!(result.len(), 1);
    assert!(result.contains(&0)); // only src/Service.cs
}

#[test]
fn test_find_phrase_candidates_empty_tokens() {
    let index = make_test_index(
        vec!["src/a.cs"],
        vec![("httpclient", vec![Posting { file_id: 0, lines: vec![1] }])],
        vec![10],
    );
    let tokens: Vec<String> = vec![];
    let result = find_phrase_candidates(&index, &tokens, &None, &[], &[]);
    // No tokens → no candidates (unwrap_or_default returns empty set)
    assert!(result.is_empty());
}

// ═══════════════════════════════════════════════════════════════════
//  score_grep_results() tests
// ═══════════════════════════════════════════════════════════════════

fn grep_score_options(
    require_all: bool,
    raw_term_count: usize,
    boost_exact_file_stem: bool,
) -> GrepScoreOptions {
    GrepScoreOptions { require_all, raw_term_count, boost_exact_file_stem }
}

#[test]
fn test_exact_file_stem_boost_enabled_only_for_exact_non_regex() {
    assert!(exact_file_stem_boost_enabled(true, false));
    assert!(!exact_file_stem_boost_enabled(false, false));
    assert!(!exact_file_stem_boost_enabled(false, true));
    assert!(!exact_file_stem_boost_enabled(true, true));
}

#[test]
fn test_score_grep_results_single_term() {
    // Need 3+ files so IDF > 0 (term appears in 2 of 3 files → IDF = ln(3/2) > 0)
    let index = make_test_index(
        vec!["src/a.cs", "src/b.cs", "src/c.cs"],
        vec![
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1, 5] },
                Posting { file_id: 1, lines: vec![3] },
            ]),
            ("othertoken", vec![
                Posting { file_id: 2, lines: vec![1] },
            ]),
        ],
        vec![10, 20, 10],
    );
    let terms = vec!["httpclient".to_string()];
    let results = score_grep_results(&index, &terms, &None, &[], &[], grep_score_options(false, 1, false));
    assert_eq!(results.len(), 2);
    // File 0 has 2 occurrences in 10 tokens (higher TF) → should rank higher
    assert_eq!(results[0].file_path, "src/a.cs");
    assert_eq!(results[0].occurrences, 2);
    assert_eq!(results[1].file_path, "src/b.cs");
    assert_eq!(results[1].occurrences, 1);
}

#[test]
fn test_score_grep_results_require_all_filters() {
    let index = make_test_index(
        vec!["src/both.cs", "src/only_one.cs"],
        vec![
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1] },
                Posting { file_id: 1, lines: vec![1] },
            ]),
            ("ilogger", vec![
                Posting { file_id: 0, lines: vec![2] },
            ]),
        ],
        vec![10, 10],
    );
    let terms = vec!["httpclient".to_string(), "ilogger".to_string()];

    // require_all = true, raw_term_count = 2 → only files with both terms
    let results = score_grep_results(&index, &terms, &None, &[], &[], grep_score_options(true, 2, false));
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].file_path, "src/both.cs");
    assert_eq!(results[0].terms_matched, 2);
}

#[test]
fn test_score_grep_results_require_all_false() {
    let index = make_test_index(
        vec!["src/both.cs", "src/only_one.cs"],
        vec![
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1] },
                Posting { file_id: 1, lines: vec![1] },
            ]),
            ("ilogger", vec![
                Posting { file_id: 0, lines: vec![2] },
            ]),
        ],
        vec![10, 10],
    );
    let terms = vec!["httpclient".to_string(), "ilogger".to_string()];

    // require_all = false → both files returned (OR)
    let results = score_grep_results(&index, &terms, &None, &[], &[], grep_score_options(false, 2, false));
    assert_eq!(results.len(), 2);
}

#[test]
fn test_score_grep_results_with_ext_filter() {
    let index = make_test_index(
        vec!["src/a.cs", "src/b.rs"],
        vec![
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1] },
                Posting { file_id: 1, lines: vec![1] },
            ]),
        ],
        vec![10, 10],
    );
    let ext = Some(vec!["cs".to_string()]);
    let terms = vec!["httpclient".to_string()];
    let results = score_grep_results(&index, &terms, &ext, &[], &[], grep_score_options(false, 1, false));
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].file_path, "src/a.cs");
}

#[test]
fn test_score_grep_results_with_exclude() {
    let index = make_test_index(
        vec!["src/Service.cs", "src/ServiceMock.cs"],
        vec![
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1] },
                Posting { file_id: 1, lines: vec![1] },
            ]),
        ],
        vec![10, 10],
    );
    let exclude = vec!["mock".to_string()];
    let terms = vec!["httpclient".to_string()];
    let results = score_grep_results(&index, &terms, &None, &[], &exclude, grep_score_options(false, 1, false));
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].file_path, "src/Service.cs");
}

#[test]
fn test_score_grep_results_empty_terms() {
    let index = make_test_index(
        vec!["src/a.cs"],
        vec![("httpclient", vec![Posting { file_id: 0, lines: vec![1] }])],
        vec![10],
    );
    let terms: Vec<String> = vec![];
    let results = score_grep_results(&index, &terms, &None, &[], &[], grep_score_options(false, 0, false));
    assert!(results.is_empty());
}

#[test]
fn test_score_grep_results_lines_deduped_and_sorted() {
    let index = make_test_index(
        vec!["src/a.cs"],
        vec![
            // Two terms both appear on line 5 → should be deduped
            ("termx", vec![Posting { file_id: 0, lines: vec![5, 3, 5] }]),
            ("termy", vec![Posting { file_id: 0, lines: vec![3, 7] }]),
        ],
        vec![10],
    );
    let terms = vec!["termx".to_string(), "termy".to_string()];
    let results = score_grep_results(&index, &terms, &None, &[], &[], grep_score_options(false, 2, false));
    assert_eq!(results.len(), 1);
    // Lines should be sorted and deduped: [3, 5, 7]
    assert_eq!(results[0].lines, vec![3, 5, 7]);
}

#[test]
fn test_score_grep_results_boosts_normalized_exact_file_stem() {
    let index = make_test_index(
        vec!["src/widget_service.rs", "test/WidgetServiceTests.rs", "src/other.rs"],
        vec![("widgetservice", vec![
            Posting { file_id: 0, lines: vec![1] },
            Posting { file_id: 1, lines: vec![1] },
        ])],
        vec![1000, 100, 100],
    );
    let terms = vec!["widgetservice".to_string()];
    let unboosted = score_grep_results(
        &index,
        &terms,
        &None,
        &[],
        &[],
        grep_score_options(false, 1, false),
    );
    let boosted = score_grep_results(
        &index,
        &terms,
        &None,
        &[],
        &[],
        grep_score_options(false, 1, true),
    );

    assert_eq!(unboosted[0].file_path, "test/WidgetServiceTests.rs");
    assert_eq!(boosted[0].file_path, "src/widget_service.rs");
    let expected_bonus = 0.02 * (3.0_f64 / 2.0).ln();
    assert!(
        (boosted[0].tf_idf - unboosted[1].tf_idf - expected_bonus).abs() < 1e-12
    );

    let multi_terms = vec!["widgetservice".to_string(), "missing".to_string()];
    let multi_term = score_grep_results(
        &index,
        &multi_terms,
        &None,
        &[],
        &[],
        grep_score_options(false, 2, true),
    );
    assert_eq!(multi_term[0].file_path, "test/WidgetServiceTests.rs");
}

#[test]
fn test_score_grep_results_tf_idf_ranking() {
    // File A: small file with 2 occurrences of term (high TF)
    // File B: large file with 1 occurrence of term (low TF)
    // File C: doesn't have the term (so IDF = ln(3/2) > 0, not ln(1) = 0)
    let index = make_test_index(
        vec!["small.cs", "large.cs", "unrelated.cs"],
        vec![
            ("targetterm", vec![
                Posting { file_id: 0, lines: vec![1, 2] },
                Posting { file_id: 1, lines: vec![50] },
            ]),
            ("othertoken", vec![
                Posting { file_id: 2, lines: vec![1] },
            ]),
        ],
        vec![5, 500, 10], // small file has 5 tokens, large has 500, unrelated has 10
    );
    let terms = vec!["targetterm".to_string()];
    let results = score_grep_results(&index, &terms, &None, &[], &[], grep_score_options(false, 1, false));
    assert_eq!(results.len(), 2);
    // Small file should rank first (higher TF-IDF)
    assert_eq!(results[0].file_path, "small.cs");
    assert!(results[0].tf_idf > results[1].tf_idf);
}

#[test]
fn test_score_grep_results_nonexistent_term() {
    let index = make_test_index(
        vec!["src/a.cs"],
        vec![("httpclient", vec![Posting { file_id: 0, lines: vec![1] }])],
        vec![10],
    );
    let terms = vec!["nonexistent".to_string()];
    let results = score_grep_results(&index, &terms, &None, &[], &[], grep_score_options(false, 1, false));
    assert!(results.is_empty());
}

// ─── respect_git_exclude propagation (Phase 1 regression tests) ──────────
//
// These tests ensure the --respect-git-exclude CLI flag is accepted by the
// FastArgs and GrepArgs parsers and that the parsed value lands on the struct
// field. Propagation into build_index / build_content_index is covered by
// existing tests in index_tests.rs; here we only guard the CLI surface.

#[test]
fn test_fast_args_respect_git_exclude_default_false() {
    use clap::Parser;
    let args = super::args::FastArgs::try_parse_from(["xray", "pattern"]).unwrap();
    assert!(!args.respect_git_exclude, "respect_git_exclude should default to false");
}

#[test]
fn test_fast_args_respect_git_exclude_flag_set() {
    use clap::Parser;
    let args = super::args::FastArgs::try_parse_from(["xray", "pattern", "--respect-git-exclude"]).unwrap();
    assert!(args.respect_git_exclude, "--respect-git-exclude should set the flag to true");
}

#[test]
fn test_grep_args_respect_git_exclude_default_false() {
    use clap::Parser;
    let args = super::args::GrepArgs::try_parse_from(["xray", "pattern"]).unwrap();
    assert!(!args.respect_git_exclude, "respect_git_exclude should default to false");
}

#[test]
fn test_grep_args_respect_git_exclude_flag_set() {
    use clap::Parser;
    let args = super::args::GrepArgs::try_parse_from(["xray", "pattern", "--respect-git-exclude"]).unwrap();
    assert!(args.respect_git_exclude, "--respect-git-exclude should set the flag to true");
}

#[test]
fn test_serve_args_respect_git_exclude_default_false() {
    use clap::Parser;
    let args = super::args::ServeArgs::try_parse_from(["xray"]).unwrap();
    assert!(!args.respect_git_exclude, "respect_git_exclude should default to false");
}

#[test]
fn test_serve_args_respect_git_exclude_flag_set() {
    use clap::Parser;
    let args = super::args::ServeArgs::try_parse_from(["xray", "--respect-git-exclude"]).unwrap();
    assert!(args.respect_git_exclude, "--respect-git-exclude should set the flag to true");
}
