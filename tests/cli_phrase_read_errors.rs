use code_xray::{ContentIndex, Posting, clean_path, extract_semantic_prefix, stable_hash};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Default)]
struct CacheFiles(Vec<PathBuf>);

impl CacheFiles {
    fn create(&mut self, path: PathBuf, bytes: &[u8]) {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap();
        self.0.push(path);
        file.write_all(bytes).unwrap();
    }
}

impl Drop for CacheFiles {
    fn drop(&mut self) {
        for path in &self.0 {
            if let Err(error) = fs::remove_file(path) {
                eprintln!("Cannot clean test cache {}: {error}", path.display());
            }
        }
    }
}

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    cache: CacheFiles,
    bytes: Vec<u8>,
    meta: Vec<u8>,
}

impl Fixture {
    fn indexed_encoding(bytes: &[u8]) -> Self {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("systemtemp");
        fs::create_dir_all(&base).unwrap();
        let temporary = tempfile::Builder::new().prefix("cli508-encoding-").tempdir_in(&base).unwrap();
        let root = fs::canonicalize(temporary.path()).unwrap();
        fs::write(root.join("probe.rs"), bytes).unwrap();
        let cache_dir = dirs::data_local_dir().unwrap().join("xray");
        fs::create_dir_all(&cache_dir).unwrap();
        let prefix = extract_semantic_prefix(&root);
        let hash = stable_hash(&[root.to_string_lossy().as_bytes(), b"rs,txt"]) as u32;
        let path = cache_dir.join(format!("{prefix}_{hash:08x}.word-search"));
        let meta_path = PathBuf::from(format!("{}.meta", path.display()));
        assert!(!path.exists() && !meta_path.exists(), "never overwrite an existing cache");
        let cache = CacheFiles(vec![path.clone(), meta_path.clone()]);
        let output = xray_command().arg("content-index").arg("--dir").arg(&root)
            .args(["--ext", "rs,txt", "--threads", "1", "--hidden", "--no-ignore", "--respect-git-exclude"])
            .output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
        let bytes = fs::read(path).unwrap();
        let meta = fs::read(meta_path).unwrap();
        let fixture = Self { _temporary: temporary, root, cache, bytes, meta };
        for token in ["alpha", "beta"] {
            let output = fixture.grep(token, &["--exact"]);
            let stderr = String::from_utf8(output.stderr).unwrap();
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert_eq!(output.status.code(), Some(0), "{stderr}");
            assert!(stderr.contains("1 files, 2 occurrences"), "{stderr}");
            assert!(stdout.contains("lines: 2, 3"), "real index candidate for {token}: {stdout}");
        }
        fixture
    }

    fn new(names: &[&str]) -> Self {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("systemtemp");
        fs::create_dir_all(&base).unwrap();
        let temporary = tempfile::Builder::new().prefix("cli508-").tempdir_in(&base).unwrap();
        let root = fs::canonicalize(temporary.path()).unwrap();
        let mut index = ContentIndex {
            root: clean_path(&root.to_string_lossy()),
            format_version: code_xray::CONTENT_INDEX_VERSION,
            created_at: code_xray::current_unix_secs().unwrap(),
            max_age_secs: 86400,
            extensions: vec!["rs".into(), "txt".into()],
            file_token_counts: vec![4; names.len()],
            total_tokens: (4 * names.len()) as u64,
            hidden: true,
            no_ignore: true,
            respect_git_exclude: true,
            ..Default::default()
        };
        for name in names {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "alpha beta\nalpha   beta\n").unwrap();
            index.files.push(path.to_string_lossy().into_owned());
        }
        for token in ["alpha", "beta"] {
            index.index.insert(token.into(), (0..names.len() as u32).map(|file_id| Posting {
                file_id, lines: vec![1, 2],
            }).collect());
        }
        let bytes = bincode::serialize(&index).unwrap();
        let meta = br#"{"fixture":"508; query must preserve cache and traversal settings"}"#.to_vec();
        let mut fixture = Self { _temporary: temporary, root, cache: CacheFiles::default(), bytes, meta };
        let cache_dir = dirs::data_local_dir().unwrap().join("xray");
        fs::create_dir_all(&cache_dir).unwrap();
        let prefix = extract_semantic_prefix(&fixture.root);
        let hash = stable_hash(&[fixture.root.to_string_lossy().as_bytes(), b"rs,txt"]) as u32;
        let path = cache_dir.join(format!("{prefix}_{hash:08x}.word-search"));
        let meta_path = PathBuf::from(format!("{}.meta", path.display()));
        fixture.cache.create(path, &fixture.bytes);
        fixture.cache.create(meta_path, &fixture.meta);
        fixture
    }

    fn grep(&self, pattern: &str, flags: &[&str]) -> Output {
        let mut command = xray_command();
        let output = command.arg("grep").arg(pattern).arg("--dir").arg(&self.root)
            .args(flags).output().unwrap();
        assert_eq!(fs::read(&self.cache.0[0]).unwrap(), self.bytes);
        assert_eq!(fs::read(&self.cache.0[1]).unwrap(), self.meta);
        output
    }

}

fn xray_command() -> Command {
    #[allow(clippy::disallowed_methods, reason = "Integration harness cannot access binary-private background_command")]
    let command = Command::new(env!("CARGO_BIN_EXE_xray"));
    #[cfg(windows)]
    let command = {
        use std::os::windows::process::CommandExt;
        let mut command = command;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        command
    };
    command
}

fn assert_supported_encoding_output(bytes: &[u8], decoded_suffix: &str) {
    let fixture = Fixture::indexed_encoding(bytes);
    for flags in [
        vec!["--phrase", "--max-results", "1"],
        vec!["--phrase", "--count"],
        vec!["--phrase", "--show-lines", "--context", "1"],
        vec!["--phrase", "--count", "--show-lines"],
    ] {
        let output = fixture.grep("alpha beta", &flags);
        let stderr = String::from_utf8(output.stderr).unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(output.status.code(), Some(0), "{stderr}");
        assert!(stderr.contains("1 files, 2 lines matching phrase"), "{stderr}");
        assert!(!stderr.contains("Error:"), "{stderr}");
        if flags.contains(&"--count") {
            assert!(stdout.is_empty(), "{stdout}");
        } else if flags.contains(&"--show-lines") {
            let path = clean_path(&fixture.root.join("probe.rs").to_string_lossy());
            assert_eq!(stdout, format!(
                " {path}:1: before {decoded_suffix}\n>{path}:2: alpha beta {decoded_suffix}\n>{path}:3: alpha   beta {decoded_suffix}\n {path}:4: after {decoded_suffix}\n\n"
            ));
        } else {
            assert_eq!(stdout.lines().count(), 1, "{stdout}");
            assert!(stdout.contains("(2 matches, lines: 2, 3)"), "{stdout}");
        }
    }
    let output = fixture.grep("alpha", &["--exact", "--show-lines"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(&format!(":2: alpha beta {decoded_suffix}")), "{stdout}");
    assert!(stdout.contains(&format!(":3: alpha   beta {decoded_suffix}")), "{stdout}");
}

#[test]
fn phrase_supported_encoding_process_utf16le() {
    let bytes: Vec<_> = [0xff, 0xfe].into_iter()
        .chain("before café\nalpha beta café\nalpha   beta café\nafter café\n".encode_utf16().flat_map(u16::to_le_bytes)).collect();
    assert_supported_encoding_output(&bytes, "café");
}

#[test]
fn phrase_supported_encoding_process_utf16be() {
    let bytes: Vec<_> = [0xfe, 0xff].into_iter()
        .chain("before café\nalpha beta café\nalpha   beta café\nafter café\n".encode_utf16().flat_map(u16::to_be_bytes)).collect();
    assert_supported_encoding_output(&bytes, "café");
}

#[test]
fn phrase_supported_encoding_process_lossy_windows1252() {
    assert_supported_encoding_output(
        b"before caf\xe9\nalpha beta caf\xe9\nalpha   beta caf\xe9\nafter caf\xe9\n", "caf\u{fffd}",
    );
}

impl Fixture {
    fn assert_read_failure(&self, flags: &[&str], path: &Path, original: &std::io::Error) {
        let mut args = vec!["--phrase", "--max-results", "1"];
        args.extend_from_slice(flags);
        let output = self.grep("alpha beta", &args);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(1), "{stderr}");
        assert!(output.stdout.is_empty(), "{:?}", output.stdout);
        assert!(stderr.contains("Found 2 candidate files"), "{stderr}");
        assert!(stderr.contains("Error: I/O error: Cannot verify phrase in"), "{stderr}");
        assert!(stderr.contains(path.to_str().unwrap()), "{stderr}");
        assert!(stderr.contains(&original.to_string()), "{stderr}");
        assert!(!stderr.contains("lines matching phrase"), "{stderr}");
        assert!(!stderr.contains("Index load:"), "{stderr}");
        assert!(!stderr.contains("Phrase detail:"), "{stderr}");
    }
}

#[test]
fn phrase_read_errors_process_contract() {
    let fixture = Fixture::new(&["good.rs", "bad.rs"]);
    for flags in [
        vec!["--phrase", "--max-results", "1"],
        vec!["--phrase", "--count"],
        vec!["--phrase", "--show-lines"],
        vec!["--phrase", "--count", "--show-lines"],
    ] {
        let output = fixture.grep("alpha beta", &flags);
        let stderr = String::from_utf8(output.stderr).unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(output.status.code(), Some(0), "{stderr}");
        assert!(stderr.contains("2 files, 4 lines matching phrase"), "{stderr}");
        if flags.contains(&"--count") {
            assert!(stdout.is_empty(), "{stdout}");
        } else if flags.contains(&"--show-lines") {
            assert!(stdout.contains(":1: alpha beta"), "{stdout}");
            assert!(stdout.contains(":2: alpha   beta"), "{stdout}");
        } else {
            assert_eq!(stdout.lines().count(), 1, "{stdout}");
            assert!(stdout.contains("(2 matches, lines: 1, 2)"), "{stdout}");
        }
    }
    for pattern in ["absent beta", "beta alpha"] {
        let output = fixture.grep(pattern, &["--phrase", "--show-lines"]);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(0), "{stderr}");
        assert!(output.stdout.is_empty());
        assert!(stderr.contains("0 files, 0 lines matching phrase"), "{stderr}");
        let candidates = if pattern == "beta alpha" { 2 } else { 0 };
        assert!(stderr.contains(&format!("Found {candidates} candidate files")), "{stderr}");
    }
    let bad = fixture.root.join("bad.rs");
    fs::remove_file(&bad).unwrap();
    let original = fs::read(&bad).unwrap_err();
    for flags in [vec![], vec!["--count"], vec!["--show-lines"], vec!["--count", "--show-lines"]] {
        fixture.assert_read_failure(&flags, &bad, &original);
    }
    let output = fixture.grep("alpha", &["--exact"]);
    let stderr = String::from_utf8(output.stderr).unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(stdout.contains("bad.rs"), "{stdout}");
    assert!(stderr.contains("2 files, 4 occurrences"), "{stderr}");
    let output = fixture.grep("alpha", &["--exact", "--show-lines"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8(output.stdout).unwrap().contains("good.rs:1: alpha beta"));
}

#[test]
fn phrase_read_errors_process_filters_before_reads() {
    let excluded = Path::new("excluded").join("bad.rs");
    let fixture = Fixture::new(&["good.rs", "bad.txt", excluded.to_str().unwrap()]);
    fs::remove_file(fixture.root.join("bad.txt")).unwrap();
    fs::remove_file(fixture.root.join(excluded)).unwrap();
    for flags in [
        vec!["--phrase", "--count", "--exclude", "bad"],
        vec!["--phrase", "--show-lines", "--ext", "rs", "--exclude-dir", "excluded"],
    ] {
        let output = fixture.grep("alpha beta", &flags);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(0), "{stderr}");
        assert!(stderr.contains("1 files, 2 lines matching phrase"), "{stderr}");
    }
}

#[cfg(windows)]
#[test]
fn phrase_read_errors_process_windows_lock_and_recovery() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new(&["good.rs", "bad.rs"]);
    let bad = fixture.root.join("bad.rs");
    let lock = fs::OpenOptions::new().read(true).share_mode(0).open(&bad).unwrap();
    let original = fs::read_to_string(&bad).unwrap_err();
    assert_eq!(original.raw_os_error(), Some(32));
    for flags in [vec![], vec!["--count"], vec!["--show-lines"], vec!["--count", "--show-lines"]] {
        fixture.assert_read_failure(&flags, &bad, &original);
    }
    let token = fixture.grep("alpha", &["--exact", "--count", "--show-lines"]);
    let stderr = String::from_utf8(token.stderr).unwrap();
    assert_eq!(token.status.code(), Some(0), "{stderr}");
    assert!(token.stdout.is_empty());
    assert!(stderr.contains("2 files, 4 occurrences"), "{stderr}");
    drop(lock);
    let recovered = fixture.grep("alpha beta", &["--phrase", "--show-lines"]);
    assert_eq!(recovered.status.code(), Some(0));
    assert!(String::from_utf8(recovered.stdout).unwrap().contains("bad.rs:1: alpha beta"));
    assert!(String::from_utf8(recovered.stderr).unwrap().contains("2 files, 4 lines matching phrase"));
}
