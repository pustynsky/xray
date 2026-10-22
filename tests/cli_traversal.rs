#![cfg(windows)]

use code_xray::{clean_path, extract_semantic_prefix, stable_hash, ContentIndex, FileIndex};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct CacheLinks(Vec<PathBuf>);

impl CacheLinks {
    fn add(&mut self, target: &Path, path: PathBuf) {
        assert!(fs::symlink_metadata(&path).is_err(), "cache path must be unused");
        std::os::windows::fs::symlink_file(target, &path).unwrap();
        self.0.push(path);
    }
}

impl Drop for CacheLinks {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

fn run(root: &Path, mode: &str, extensions: bool) -> Output {
    use std::os::windows::process::CommandExt;
    #[allow(clippy::disallowed_methods, reason = "Integration crate cannot access the binary-private background_command; applies CREATE_NO_WINDOW here")]
    let mut command = Command::new(env!("CARGO_BIN_EXE_xray"));
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    command.arg(mode).arg("scope_not_found").arg("-d").arg(root).arg("--count");
    if extensions {
        command.args(["-e", "rs"]);
    }
    command.output().unwrap()
}

fn run_grep(root: &Path, pattern: &str, mode: &str, ext: Option<&str>, flags: &[&str]) -> Output {
    use std::os::windows::process::CommandExt;
    #[allow(clippy::disallowed_methods, reason = "Integration crate cannot access the binary-private background_command; applies CREATE_NO_WINDOW here")]
    let mut command = Command::new(env!("CARGO_BIN_EXE_xray"));
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    command.arg("grep").arg(pattern).arg("-d").arg(root).args(flags);
    if !mode.is_empty() {
        command.arg(mode);
    }
    if let Some(ext) = ext {
        command.arg("-e").arg(ext);
    }
    command.output().unwrap()
}

#[test]
#[ignore = "requires Windows file-symlink privilege; run explicitly for real CLI smoke"]
fn grep_normalized_extensions_cli_modes() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("systemtemp");
    fs::create_dir_all(&base).unwrap();
    let fixture = tempfile::Builder::new().prefix("cli502-").tempdir_in(&base).unwrap();
    let root = fs::canonicalize(fixture.path()).unwrap();
    let canonical = root.to_string_lossy();
    let cache_dir = dirs::data_local_dir().unwrap().join("xray");
    fs::create_dir_all(&cache_dir).unwrap();
    let prefix = extract_semantic_prefix(&root);
    let content_hash = stable_hash(&[canonical.as_bytes(), b"rs,cs,txt"]) as u32;
    let content_path = cache_dir.join(format!("{prefix}_{content_hash:08x}.word-search"));
    let content_target = root.join("cache.word-search");
    let meta_target = root.join("content.meta");
    let names = ["probe.RS", "probe.Cs", "probe.txt"];
    let mut content = ContentIndex {
        root: clean_path(&canonical), format_version: code_xray::CONTENT_INDEX_VERSION,
        created_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        max_age_secs: 86400, extensions: vec!["rs".into(), "cs".into(), "txt".into()],
        file_token_counts: vec![7; 3], total_tokens: 21,
        ..Default::default()
    };
    for name in names {
        let path = root.join(name);
        fs::write(&path, "scope_not_found scope_not_found alpha beta\nscope_not_found alpha beta\n").unwrap();
        content.files.push(path.to_string_lossy().into_owned());
    }
    for token in ["alpha", "beta", "scope_not_found"] {
        content.index.insert(token.into(), (0..3).map(|file_id| code_xray::Posting {
            file_id, lines: if token == "scope_not_found" { vec![1, 1, 2] } else { vec![1, 2] },
        }).collect());
    }
    // Two-character substring queries scan the sorted token list directly.
    content.trigram.tokens = vec!["alpha".into(), "beta".into(), "scope_not_found".into()];
    let original_bytes = bincode::serialize(&content).unwrap();
    let original_meta = br#"{"fixture":"502; fresh queries must not rewrite this"}"#;
    fs::write(&content_target, &original_bytes).unwrap();
    fs::write(&meta_target, original_meta).unwrap();
    let mut links = CacheLinks(Vec::new());
    links.add(&content_target, content_path.clone());
    links.add(&meta_target, PathBuf::from(format!("{}.meta", content_path.display())));
    let cache_paths = || {
        let mut paths: Vec<_> = fs::read_dir(&cache_dir).unwrap().map(|entry| entry.unwrap().path())
            .filter(|path| path.file_name().unwrap().to_string_lossy().starts_with(&format!("{prefix}_")))
            .collect();
        paths.sort();
        paths
    };
    let original_paths = cache_paths();
    assert_eq!(original_paths.len(), 2);

    for (mode, pattern, absent) in [
        ("--exact", "scope_not_found", "genuinely_absent"),
        ("", "sc", "zz"),
        ("--regex", "^scope_not_found$", "^genuinely_absent$"),
        ("--phrase", "scope_not_found alpha", "genuinely_absent alpha"),
    ] {
        for (ext, expected) in [
            (Some("rs"), 1), (Some("cs"), 1), (Some("RS"), 1), (Some(" CS "), 1),
            (Some(" rs "), 1), (Some("rs,cs"), 2), (Some(" CS, rS "), 2),
            (Some("rs,RS,cs,CS"), 2), (None, 3),
        ] {
            for _ in 0..2 {
                for count in [false, true] {
                    let flags: &[&str] = if count { &["--count"] } else { &[] };
                    let output = run_grep(&root, pattern, mode, ext, flags);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let unit = if mode == "--phrase" { "lines" } else { "occurrences" };
                    assert_eq!(output.status.code(), Some(0), "{mode} {ext:?}: {stderr}");
                    assert!(stderr.contains(&format!("{expected} files, {} {unit}", expected * 2)),
                        "{mode} {ext:?}: {stderr}");
                    assert!(!stderr.contains("rebuild"), "{stderr}");
                    if count {
                        assert!(stdout.is_empty(), "{stdout}");
                    } else {
                        assert_eq!(stdout.lines().count(), expected, "{stdout}");
                        let single_cs = ext.is_some_and(|e| e.trim().eq_ignore_ascii_case("cs"));
                        assert!(stdout.contains(if single_cs {
                            names[1]
                        } else { names[0] }), "{stdout}");
                        assert_eq!(stdout.contains(names[1]), expected > 1 || single_cs, "{stdout}");
                        assert_eq!(stdout.contains(names[2]), ext.is_none(), "{stdout}");
                        assert!(stdout.lines().all(|line| line.ends_with("lines: 1, 2)")), "{stdout}");
                    }
                }
            }
        }
        let missing_patterns = if mode == "--phrase" {
            vec![absent, "alpha scope_not_found", "scope_not_found beta"]
        } else { vec![absent] };
        for missing in missing_patterns {
            let output = run_grep(&root, missing, mode, Some(" rs, CS "), &["--count"]);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(output.status.code(), Some(0), "{stderr}");
            assert!(output.stdout.is_empty());
            assert!(stderr.contains("0 files, 0 "), "{stderr}");
        }
        let output = run_grep(&root, pattern, mode, Some(" RS, cs "), &["--max-results", "1"]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{stderr}");
        assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 1);
        assert!(stderr.contains("2 files, 4 "), "{stderr}");
    }
    assert_eq!(fs::read(&content_target).unwrap(), original_bytes);
    assert_eq!(fs::read(&meta_target).unwrap(), original_meta);
    assert_eq!(cache_paths(), original_paths);

    // Invalid query scopes must fail before even trying to load a corrupt cache.
    fs::write(&content_target, b"corrupt cache").unwrap();
    for ext in ["", " ", "\t", ",", ",rs", "rs,", "rs,,cs", "rs, ,cs"] {
        let output = run_grep(&root, "scope_not_found", "--exact", Some(ext), &["--count"]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{ext:?}: {stderr}");
        assert!(output.stdout.is_empty());
        assert!(stderr.contains("Content index extensions must be non-empty"), "{ext:?}: {stderr}");
        assert!(!stderr.contains("rebuild") && !stderr.contains("Refusing to replace"), "{stderr}");
    }
    assert_eq!(fs::read(&content_target).unwrap(), b"corrupt cache");
    assert_eq!(fs::read(&meta_target).unwrap(), original_meta);
    assert_eq!(cache_paths(), original_paths);
}

#[test]
#[ignore = "requires Windows file-symlink privilege; run explicitly for real CLI smoke"]
fn existing_cache_refusal_and_valid_empty_cli_output() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("systemtemp");
    fs::create_dir_all(&base).unwrap();
    let fixture = tempfile::Builder::new().prefix("cli504-").tempdir_in(&base).unwrap();
    let root = fs::canonicalize(fixture.path()).unwrap();
    let canonical = root.to_string_lossy();
    let clean = clean_path(&canonical);
    let cache_dir = dirs::data_local_dir().unwrap().join("xray");
    fs::create_dir_all(&cache_dir).unwrap();
    let prefix = extract_semantic_prefix(&root);
    let file_hash = stable_hash(&[canonical.as_bytes()]) as u32;
    let content_hash = stable_hash(&[canonical.as_bytes(), b"rs"]) as u32;
    let file_path = cache_dir.join(format!("{prefix}_{file_hash:08x}.file-list"));
    let content_path = cache_dir.join(format!("{prefix}_{content_hash:08x}.word-search"));
    let file_target = root.join("cache.file-list");
    let content_target = root.join("cache.word-search");
    let file_meta_target = root.join("file.meta");
    let content_meta_target = root.join("content.meta");
    let meta_bytes = br#"{"deliberately":"not authoritative traversal settings"}"#;
    fs::write(&file_meta_target, meta_bytes).unwrap();
    fs::write(&content_meta_target, meta_bytes).unwrap();

    let file = FileIndex {
        root: clean.clone(), format_version: code_xray::FILE_INDEX_VERSION,
        created_at: 1, max_age_secs: 3600, entries: Vec::new(),
        respect_git_exclude: true, hidden: true, no_ignore: true,
    };
    let mut content = ContentIndex {
        root: clean, format_version: code_xray::CONTENT_INDEX_VERSION,
        created_at: 1, max_age_secs: 3600, extensions: vec!["rs".into()],
        respect_git_exclude: true, hidden: true, no_ignore: true,
        ..Default::default()
    };
    fs::write(&file_target, bincode::serialize(&file).unwrap()).unwrap();
    fs::write(&content_target, bincode::serialize(&content).unwrap()).unwrap();
    // Only unique lookup links live in the normal CLI cache directory. All
    // fixture/cache payload bytes remain under target/systemtemp.
    let mut links = CacheLinks(Vec::new());
    links.add(&file_target, file_path.clone());
    links.add(&content_target, content_path.clone());
    links.add(&file_meta_target, PathBuf::from(format!("{}.meta", file_path.display())));
    links.add(&content_meta_target, PathBuf::from(format!("{}.meta", content_path.display())));

    for legacy in [true, false] {
        if legacy {
            fs::write(&file_target, bincode::serialize(&(
                &file.root, 2u32, file.created_at, file.max_age_secs,
                &file.entries, file.respect_git_exclude,
            )).unwrap()).unwrap();
            fs::write(&content_target, bincode::serialize(&(
                &content.root, 4u32, content.created_at, content.max_age_secs,
                &content.files, &content.index, content.total_tokens,
                &content.extensions, &content.file_token_counts,
                &content.trigram, content.trigram_dirty, &content.path_to_id,
                content.read_errors, content.lossy_file_count, content.worker_panics,
                content.respect_git_exclude,
            )).unwrap()).unwrap();
        } else {
            fs::write(&file_target, b"corrupt cache").unwrap();
            fs::write(&content_target, bincode::serialize(&(
                &content.root, code_xray::CONTENT_INDEX_VERSION,
            )).unwrap()).unwrap();
        }
        let file_bytes = fs::read(&file_target).unwrap();
        let content_bytes = fs::read(&content_target).unwrap();
        for (mode, ext, command) in [("fast", false, "xray index"), ("grep", true, "xray content-index"), ("grep", false, "xray content-index")] {
            let output = run(&root, mode, ext);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(output.status.code(), Some(1), "{stderr}");
            assert!(output.stdout.is_empty(), "{:?}", output.stdout);
            assert!(stderr.contains("Refusing to replace") && stderr.contains(command), "{stderr}");
            assert!(stderr.contains("--hidden") && stderr.contains("--no-ignore"), "{stderr}");
        }
        assert_eq!(fs::read(&file_target).unwrap(), file_bytes);
        assert_eq!(fs::read(&content_target).unwrap(), content_bytes);
        assert_eq!(fs::read(&file_meta_target).unwrap(), meta_bytes);
        assert_eq!(fs::read(&content_meta_target).unwrap(), meta_bytes);
    }

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let file = FileIndex { created_at: now, ..file };
    content.created_at = now;
    fs::write(&file_target, bincode::serialize(&file).unwrap()).unwrap();
    fs::write(&content_target, bincode::serialize(&content).unwrap()).unwrap();
    for (mode, ext, summary) in [("fast", false, "0 matches found among 0 indexed entries"), ("grep", false, "0 files, 0 occurrences")] {
        let output = run(&root, mode, ext);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{stderr}");
        assert!(output.stdout.is_empty());
        assert!(stderr.contains(summary), "{stderr}");
    }
    drop(links);
    let missing = run(&root, "grep", false);
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("No content index found"));
}
