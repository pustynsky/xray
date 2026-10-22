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
