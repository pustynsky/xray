use code_xray::{ContentIndex, Posting, clean_path, extract_semantic_prefix, stable_hash};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
    bytes: Vec<u8>,
}

impl Fixture {
    fn new(corpus: &[(&str, &[&str])]) -> Self {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("systemtemp");
        fs::create_dir_all(&base).unwrap();
        let temporary = tempfile::Builder::new().prefix("regex-queue-").tempdir_in(base).unwrap();
        let root = fs::canonicalize(temporary.path()).unwrap();
        let mut index = ContentIndex {
            root: clean_path(&root.to_string_lossy()),
            format_version: code_xray::CONTENT_INDEX_VERSION,
            created_at: code_xray::current_unix_secs().unwrap(),
            max_age_secs: 86400,
            extensions: vec!["txt".into()],
            file_token_counts: corpus.iter().map(|(_, tokens)| tokens.len() as u32).collect(),
            total_tokens: corpus.iter().map(|(_, tokens)| tokens.len() as u64).sum(),
            ..Default::default()
        };
        for (file_id, (name, tokens)) in corpus.iter().enumerate() {
            let path = root.join(name);
            fs::write(&path, tokens.join(" ")).unwrap();
            index.files.push(path.to_string_lossy().into_owned());
            for token in *tokens {
                index.index.entry(token.to_string()).or_default().push(Posting {
                    file_id: file_id as u32, lines: vec![1],
                });
            }
        }
        index.trigram.tokens = index.index.keys().cloned().collect();
        index.trigram.tokens.sort();
        for (id, token) in index.trigram.tokens.iter().enumerate() {
            let chars: Vec<_> = token.chars().collect();
            for window in chars.windows(3) {
                let gram: String = window.iter().collect();
                let postings = index.trigram.trigram_map.entry(gram).or_default();
                if postings.last() != Some(&(id as u32)) {
                    postings.push(id as u32);
                }
            }
        }
        let bytes = bincode::serialize(&index).unwrap();
        let cache_dir = dirs::data_local_dir().unwrap().join("xray");
        fs::create_dir_all(&cache_dir).unwrap();
        let prefix = extract_semantic_prefix(&root);
        let hash = stable_hash(&[root.to_string_lossy().as_bytes(), b"txt"]) as u32;
        let cache = cache_dir.join(format!("{prefix}_{hash:08x}.word-search"));
        assert!(!cache.exists(), "fixture must not overwrite another cache");
        fs::write(&cache, &bytes).unwrap();
        Self { _temporary: temporary, root, cache, bytes }
    }

    fn grep(&self, pattern: &str, flags: &[&str]) -> Output {
        #[allow(clippy::disallowed_methods, reason = "Integration harness cannot access binary-private background_command")]
        let mut command = Command::new(env!("CARGO_BIN_EXE_xray"));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        }
        let output = command.arg("grep").arg(pattern).arg("--dir").arg(&self.root)
            .args(["--ext", "txt"]).args(flags).output().unwrap();
        assert_eq!(fs::read(&self.cache).unwrap(), self.bytes, "query must not rewrite its cache");
        output
    }

    fn assert_paths(&self, pattern: &str, flags: &[&str], expected: &[&str]) {
        let output = self.grep(pattern, flags);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(0), "{pattern}: {stderr}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        let mut actual = Vec::new();
        for line in stdout.lines() {
            let record = line.split_once("] ").map_or(line, |(_, record)| record);
            let path = record.split_once(" (").expect("CLI result path and statistics").0;
            actual.push(Path::new(path).file_name().unwrap().to_str().unwrap().to_string());
        }
        actual.sort();
        assert_eq!(actual, expected, "{pattern}: {stderr}");
        assert!(stderr.contains(pattern), "{stderr}");
        if flags.contains(&"--regex") && !pattern.contains(',') {
            assert!(stderr.contains(&format!("'{pattern}' matched")), "{stderr}");
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_file(&self.cache).expect("remove owned regex fixture cache");
    }
}

#[test]
fn regex_queue_470_cli_source_and_controls() {
    let fixture = Fixture::new(&[("letters.txt", &["httpclient"]), ("digits.txt", &["12345"])]);
    fixture.assert_paths(r"\D+", &["--regex"], &["letters.txt"]);
    fixture.assert_paths(r"\S+", &["--regex"], &["digits.txt", "letters.txt"]);
    fixture.assert_paths(r"\d+,\D+", &["--regex"], &["digits.txt", "letters.txt"]);
    fixture.assert_paths(r"\d+", &["--regex"], &["digits.txt"]);
    fixture.assert_paths(r"\s+", &["--regex"], &[]);
    fixture.assert_paths("HTTPCLIENT", &["--regex"], &["letters.txt"]);
    fixture.assert_paths("(?-i)HTTPCLIENT", &["--regex"], &[]);
    fixture.assert_paths("HTTPCLIENT", &["--exact"], &["letters.txt"]);
    fixture.assert_paths("CLIENT", &[], &["letters.txt"]);
    fixture.assert_paths("HTTPCLIENT", &["--phrase"], &["letters.txt"]);
}

#[test]
fn regex_queue_470_cli_and_and_identical_duplicates() {
    let fixture = Fixture::new(&[
        ("letters.txt", &["alpha"]), ("digits.txt", &["12345"]), ("mixed.txt", &["alpha", "12345"]),
    ]);
    fixture.assert_paths(r"\d+,\D+", &["--regex", "--all"], &["mixed.txt"]);
    fixture.assert_paths(r" \D+, \D+, ", &["--regex", "--all"], &["letters.txt", "mixed.txt"]);
}

#[test]
fn regex_queue_473_cli_whole_alternation() {
    let fixture = Fixture::new(&[
        ("alpha.txt", &["alpha"]), ("beta.txt", &["beta"]),
        ("alphabet.txt", &["alphabet"]), ("prebeta.txt", &["prebeta"]),
    ]);
    for pattern in ["alpha|beta", "(?:alpha|beta)", "(alpha)|(beta)", "^alpha$|^beta$", "alpha|beta|"] {
        fixture.assert_paths(pattern, &["--regex"], &["alpha.txt", "beta.txt"]);
    }
    fixture.assert_paths("(?x)alpha # trailing comment", &["--regex"], &["alpha.txt"]);
}

#[test]
fn regex_queue_cli_invalid_source() {
    let fixture = Fixture::new(&[("alpha.txt", &["alpha"])]);
    for pattern in ["[", "\\", r"\q", "foo)|(?:bar", "(?x)alpha) # trailing comment"] {
        let output = fixture.grep(pattern, &["--regex"]);
        assert_eq!(output.status.code(), Some(1), "{pattern}");
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(pattern), "{stderr}");
    }
}
