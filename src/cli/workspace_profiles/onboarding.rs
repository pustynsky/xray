use std::{collections::BTreeMap, fs::{self, OpenOptions}, io::{self, Write}, path::{Path, PathBuf}};

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{ServeArgs, comparison_path, invalid, resolve};

const KNOWN: &[&str] = &[
    "cs", "csx", "csproj", "vb", "vbproj", "fsproj", "vcxproj", "sln", "props", "targets",
    "xaml", "razor", "cshtml", "ts", "tsx", "js", "jsx", "mjs", "cjs", "vue", "svelte",
    "html", "htm", "css", "scss", "less", "sass", "rs", "toml", "py", "pyi", "pyx", "go", "mod",
    "java", "kt", "kts", "gradle", "c", "cpp", "cc", "cxx", "h", "hpp", "hxx",
    "rb", "php", "pl", "pm", "swift", "m", "mm", "ps1", "psm1", "psd1", "sh", "bash", "zsh",
    "xml", "resx", "json", "jsonc", "yaml", "yml", "config", "ini", "env",
    "manifestxml", "appxmanifest", "vsixmanifest", "md", "txt", "rst", "sql",
    "scala", "clj", "ex", "erl", "dart", "lua", "r", "rmd", "tf", "hcl",
];
const STRUCTURAL: &[&str] = &[
    "md", "sln", "csproj", "vbproj", "fsproj", "vcxproj", "props", "targets",
    "xaml", "razor", "cshtml", "xml", "config", "resx", "manifestxml", "appxmanifest", "vsixmanifest",
];
const SKIP: &[&str] = &[
    ".git", "node_modules", "bin", "obj", "target", "dist", "build", ".vs", ".vscode", ".roo",
    ".idea", "__pycache__", "packages", ".nuget", "vendor", ".next", ".output", "coverage",
    "Debug", "Release", "x64", "x86", ".playwright-mcp", "TestResults", "testbin",
    "shared.obj", "shared.obj.x64Debug", "shared.obj.x86Debug",
];

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Options {
    #[serde(default)]
    pub confirm: bool,
    proposal_id: Option<String>,
    extensions: Option<Vec<String>>,
}

pub(super) fn tool() -> crate::mcp::protocol::ToolDefinition {
    crate::mcp::protocol::ToolDefinition {
        name: "xray_setup".into(),
        description: "Set up Xray for this unregistered workspace. First call with no arguments to scan file names and propose extensions. Show the workspace path and extensions to the user and ask for explicit approval; NEVER approve on their behalf. Only after approval call with confirm=true and the returned proposalId, optionally with the user's chosen detected extensions. No profile or indexes are created before confirmation. After setup, wait for xray_info indexes to become ready before searching.".into(),
        input_schema: json!({
            "type":"object", "additionalProperties":false,
            "properties":{
                "confirm":{"type":"boolean","default":false,"description":"True only after explicit user approval of this proposal."},
                "proposalId":{"type":"string","description":"The ID returned by the preceding proposal, required for confirmation."},
                "extensions":{"type":"array","items":{"type":"string"},"minItems":1,"description":"User-approved detected extensions. Omit to accept suggestedExtensions."}
            }
        }),
    }
}

pub(super) fn git_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors().find(|path| path.join(".git").is_dir() || path.join(".git").is_file()).map(Path::to_path_buf)
}

pub(super) struct Proposal {
    root: PathBuf,
    canonical_root: PathBuf,
    counts: BTreeMap<String, usize>,
    suggested: Vec<String>,
    threshold: usize,
    id: String,
}

impl Proposal {
    pub(super) fn scan(root: &Path) -> io::Result<Self> {
        let root = installer_root(root)?;
        let canonical_root = fs::canonicalize(&root)?;
        let mut counts = BTreeMap::new();
        let mut stack = vec![canonical_root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).map_err(|error| io::Error::new(error.kind(), format!(
                "Cannot scan {}: {error}. No workspace profile was created.", dir.display()
            )))? {
                let entry = entry?;
                let kind = entry.file_type()?;
                if is_link(&entry)? { continue; }
                if kind.is_dir() {
                    if !SKIP.iter().any(|skip| entry.file_name().to_string_lossy().eq_ignore_ascii_case(skip)) {
                        stack.push(entry.path());
                    }
                } else if kind.is_file()
                    && let Some((_, ext)) = entry.file_name().to_str().and_then(|name| name.rsplit_once('.')) {
                        let ext = ext.to_lowercase();
                        if KNOWN.contains(&ext.as_str()) { *counts.entry(ext).or_insert(0) += 1; }
                    }
            }
        }
        if counts.is_empty() { return Err(invalid("No recognized code/text files were found. Configure this workspace with setup-xray.ps1 -UserMcp -Extensions <extensions>.")); }
        let total: usize = counts.values().sum();
        let threshold = 5.max(total.div_ceil(200));
        let suggested = counts.iter().filter(|(ext, count)| **count >= threshold || STRUCTURAL.contains(&ext.as_str()))
            .map(|(ext, _)| ext.clone()).collect();
        let id = format!("{:x}", Sha256::digest(serde_json::to_vec(&json!({"root":root,"counts":counts}))?));
        Ok(Self { root, canonical_root, counts, suggested, threshold, id })
    }

    pub(super) fn body(&self) -> Value {
        json!({
            "status":"confirmation_required", "workspaceRoot":portable_root(&self.root), "proposalId":self.id,
            "detectedExtensions":self.counts, "suggestedExtensions":self.suggested,
            "totalDetectedFiles":self.counts.values().sum::<usize>(), "threshold":self.threshold,
            "hint":"Show this workspace path and proposed extensions to the user and ask for explicit approval. Do not create a profile or index without approval. After approval call xray_setup with confirm=true and this proposalId. If suggestedExtensions is empty, ask the user to choose from detectedExtensions and pass extensions explicitly. Counts are filename-based estimates; indexing applies its normal ignore rules."
        })
    }

    pub(super) fn install(&self, profiles: &Path, options: Options) -> io::Result<ServeArgs> {
        if options.proposal_id.as_deref() != Some(&self.id) {
            return Err(invalid("Missing or stale proposalId. Request a proposal and obtain user approval before confirming."));
        }
        if comparison_path(&fs::canonicalize(&self.root)?) != comparison_path(&self.canonical_root) {
            return Err(invalid("Workspace root changed since the proposal. Reconnect and obtain approval for its current location."));
        }
        if Self::scan(&self.root)?.id != self.id {
            return Err(invalid("Workspace files changed since the proposal. Request a fresh proposal and confirmation."));
        }
        let mut extensions = options.extensions.unwrap_or_else(|| self.suggested.clone());
        for ext in &mut extensions {
            *ext = ext.trim().trim_start_matches('.').to_lowercase();
            if !self.counts.contains_key(ext) { return Err(invalid(format!("Extension '{ext}' was not detected in this workspace."))); }
        }
        extensions.sort();
        extensions.dedup();
        if extensions.is_empty() { return Err(invalid("No extensions selected. Ask the user to choose detectedExtensions and pass extensions explicitly.")); }
        let profile_root = resolved_destination(profiles)?;
        if comparison_path(&profile_root).starts_with(comparison_path(&self.canonical_root)) {
            return Err(invalid("Workspace profiles must be stored outside the repository. No profile was created."));
        }
        match resolve(profiles, &self.root) {
            Ok(_) => return Err(invalid("A workspace profile was installed during confirmation. Reconnect to use it; no existing settings were changed.")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let directory = profiles.join(profile_id(&self.root));
        fs::create_dir_all(&directory)?;
        let file = directory.join("config.json");
        let bytes = serde_json::to_vec_pretty(&json!({
            "version":1, "workspaceRoot":portable_root(&self.root),
            "args":["--ext", extensions.join(","), "--watch", "--definitions", "--debug-log"],
        }))?;
        publish_profile(&file, &bytes)?;
        resolve(profiles, &self.root)
    }
}

fn installer_root(root: &Path) -> io::Result<PathBuf> {
    let root = portable_root(&std::path::absolute(root)?);
    if git_root(&root).is_none() { return Ok(root); }
    let output = crate::background_command("git").arg("-C").arg(&root)
        .args(["rev-parse", "--show-toplevel"]).output()?;
    if !output.status.success() {
        return Err(invalid(format!("Cannot determine Git workspace root for {} ({}). No profile was created.", root.display(), output.status)));
    }
    let text = std::str::from_utf8(&output.stdout).map_err(|error| invalid(error.to_string()))?.trim();
    let top = portable_root(&std::path::absolute(Path::new(text))?);
    if text.is_empty() || !comparison_path(&fs::canonicalize(&root)?).starts_with(comparison_path(&fs::canonicalize(&top)?)) {
        return Err(invalid("Git workspace root is empty or outside the selected workspace ancestry. No profile was created."));
    }
    Ok(top)
}

fn publish_profile(file: &Path, bytes: &[u8]) -> io::Result<()> {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(io::Error::other)?.as_nanos();
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary = file.with_file_name(format!(".config-{}-{timestamp}-{sequence}.pending", std::process::id()));
    let mut staged = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
    let result = (|| {
        staged.write_all(bytes)?;
        staged.sync_all()?;
        drop(staged);
        // A new hard link atomically publishes complete bytes and never overwrites a competing installer.
        fs::hard_link(&temporary, file)
    })();
    if let Err(error) = fs::remove_file(&temporary) {
        eprintln!("Xray: cannot remove staged workspace profile {}: {error}", temporary.display());
        if result.is_ok() { return Err(error); }
    }
    result.map_err(|error| io::Error::new(error.kind(), format!(
        "Cannot publish workspace profile {}: {error}. No existing profile was overwritten.", file.display()
    )))
}

fn resolved_destination(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut ancestor = absolute.as_path();
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or_else(|| invalid("Cannot resolve workspace profile directory"))?;
    }
    let mut canonical = fs::canonicalize(ancestor)?;
    for component in absolute.strip_prefix(ancestor).map_err(|error| invalid(error.to_string()))?.components() {
        match component {
            std::path::Component::ParentDir => { canonical.pop(); }
            std::path::Component::CurDir => {}
            component => canonical.push(component),
        }
    }
    Ok(canonical)
}

fn profile_id(root: &Path) -> String {
    let root = portable_root(root);
    let key = root.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
    let hash = format!("{:x}", Sha256::digest(key.as_bytes()));
    let mut label = String::new();
    for ch in key.chars() {
        if ch.is_ascii_alphanumeric() { label.push(ch); }
        else if !label.ends_with('_') { label.push('_'); }
    }
    let label = label.trim_matches('_');
    let label = &label[..label.len().min(64)];
    let label = label.trim_end_matches('_');
    format!("{}-{}", if label.is_empty() { "workspace" } else { label }, &hash[..12])
}

fn portable_root(root: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let path = root.to_string_lossy();
        if let Some(unc) = path.strip_prefix(r"\\?\UNC\") { return PathBuf::from(format!(r"\\{unc}")); }
        PathBuf::from(path.strip_prefix(r"\\?\").unwrap_or(&path))
    }
    #[cfg(not(windows))]
    { root.to_path_buf() }
}

fn is_link(entry: &fs::DirEntry) -> io::Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(entry.metadata()?.file_attributes() & 0x400 != 0)
    }
    #[cfg(not(windows))]
    { Ok(entry.file_type()?.is_symlink()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(proposal: &Proposal, extensions: Option<Vec<String>>) -> Options {
        Options { confirm: true, proposal_id: Some(proposal.id.clone()), extensions }
    }

    #[test]
    fn detection_rules_match_installer() {
        let script = include_str!("../../../scripts/setup-xray.ps1");
        let quoted = regex::Regex::new("'([^']*)'").unwrap();
        let known = script.split("$KnownCodeExtensions = @{").nth(1).unwrap().split("\n}").next().unwrap();
        let keys = regex::Regex::new("'([^']*)'\\s*=").unwrap();
        let mut actual: Vec<_> = keys.captures_iter(known).map(|cap| cap[1].to_string()).collect();
        actual.sort();
        let mut expected: Vec<_> = KNOWN.iter().map(|ext| ext.to_string()).collect();
        expected.sort();
        assert_eq!(actual, expected);
        for (name, expected) in [("$AlwaysIncludeIfDetected", STRUCTURAL), ("$SkipDirs", SKIP)] {
            let section = script.split(&format!("{name} = @(")).nth(1).unwrap().split("\n)").next().unwrap();
            let actual: Vec<_> = quoted.captures_iter(section).map(|cap| cap[1].to_string()).collect();
            assert_eq!(actual, expected);
        }
        assert!(script.contains("[Math]::Max(5, [Math]::Ceiling($totalFiles * 0.005))"));
    }

    #[test]
    fn detection_preserves_threshold_structural_files_and_directory_pruning() {
        let temp = tempfile::tempdir().unwrap();
        for index in 0..1000 {
            fs::write(temp.path().join(format!("{index}.RS")), "").unwrap();
        }
        for index in 0..5 {
            fs::write(temp.path().join(format!("{index}.ts")), "").unwrap();
        }
        fs::write(temp.path().join("project.csproj"), "").unwrap();
        fs::write(temp.path().join(".env"), "").unwrap();
        fs::create_dir(temp.path().join("TARGET")).unwrap();
        fs::write(temp.path().join("TARGET/generated.py"), "").unwrap();
        fs::write(temp.path().join("ignored.exe"), "").unwrap();
        let detected = Proposal::scan(temp.path()).unwrap();
        assert_eq!(detected.threshold, 6);
        assert_eq!(detected.suggested, ["csproj", "rs"]);
        assert_eq!(detected.counts["ts"], 5);
        assert_eq!(detected.counts["env"], 1);
        assert!(!detected.counts.contains_key("py"));
        assert!(!detected.counts.contains_key("exe"));
    }

    #[test]
    fn tiny_workspaces_need_explicit_selection_and_reject_stale_proposals() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let profiles = temp.path().join("profiles");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("source.rs"), "pub fn source() {}").unwrap();
        let root = fs::canonicalize(root).unwrap();
        let detected = Proposal::scan(&root).unwrap();
        assert!(detected.suggested.is_empty());
        assert!(detected.install(&profiles, options(&detected, None)).unwrap_err().to_string().contains("No extensions"));
        assert!(!profiles.exists());
        fs::write(root.join("README.md"), "").unwrap();
        assert!(detected.install(&profiles, options(&detected, Some(vec!["rs".into()]))).unwrap_err().to_string().contains("changed"));
        assert!(!profiles.exists());
        let fresh = Proposal::scan(&root).unwrap();
        let serve = fresh.install(&profiles, options(&fresh, Some(vec![" .RS ".into(), "rs".into()]))).unwrap();
        assert_eq!(serve.ext, ["rs"]);
        let file = profiles.join(profile_id(&root)).join("config.json");
        let before = fs::read(&file).unwrap();
        assert!(fresh.install(&profiles, options(&fresh, None)).is_err());
        assert_eq!(fs::read(file).unwrap(), before);
    }

    #[test]
    fn confirmation_refuses_profile_directories_inside_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        fs::write(root.join("README.md"), "").unwrap();
        let detected = Proposal::scan(&root).unwrap();
        let profiles = root.join("new").join("..").join("profiles");
        assert!(detected.install(&profiles, options(&detected, None)).unwrap_err().to_string().contains("outside"));
        assert!(!root.join("new").exists());
        assert!(!root.join("profiles").exists());
    }

    #[test]
    fn concurrent_confirmation_publishes_one_complete_profile_without_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let profiles = temp.path().join("profiles");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("README.md"), "").unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = (0..2).map(|_| {
            let root = root.clone();
            let profiles = profiles.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let detected = Proposal::scan(&root).unwrap();
                barrier.wait();
                detected.install(&profiles, options(&detected, None))
            })
        }).collect();
        let results: Vec<_> = threads.into_iter().map(|thread| thread.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let serve = resolve(&profiles, &root).unwrap();
        assert_eq!(serve.ext, ["md"]);
        let file = profiles.join(profile_id(&root)).join("config.json");
        let value: Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["args"], json!(["--ext", "md", "--watch", "--definitions", "--debug-log"]));
    }

    #[test]
    fn atomic_publication_refuses_an_existing_destination_and_cleans_its_stage() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("config.json");
        fs::write(&file, "existing profile").unwrap();
        assert!(publish_profile(&file, b"replacement profile").is_err());
        assert_eq!(fs::read_to_string(file).unwrap(), "existing profile");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn junction_workspace_uses_the_installers_git_root_and_profile_name() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        let alias = temp.path().join("alias");
        let profiles = temp.path().join("profiles");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("README.md"), "").unwrap();
        assert!(crate::background_command("git").args(["init", "--quiet"]).arg(&real).status().unwrap().success());
        let escape = |path: &Path| path.to_string_lossy().replace('\'', "''");
        let installer = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts").join("setup-xray.ps1");
        let script = format!(
            "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path '{}' -Target '{}' | Out-Null; \
            $errors=$null; $ast=[Management.Automation.Language.Parser]::ParseFile('{}',[ref]$null,[ref]$errors); \
            $f=$ast.Find({{param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Get-WorkspaceProfilePath'}},$true); \
            Invoke-Expression $f.Extent.Text; $root=(git -C '{}' rev-parse --show-toplevel); if ($LASTEXITCODE -ne 0) {{ throw 'Git root failed' }}; \
            $root=(Resolve-Path -LiteralPath $root).ProviderPath; \
            @{{root=$root; profile=(Get-WorkspaceProfilePath -Profiles '{}' -RepoRoot $root)}} | ConvertTo-Json -Compress",
            escape(&alias), escape(&real), escape(&installer), escape(&alias), escape(&profiles)
        );
        let output = crate::background_command("powershell.exe")
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", &script]).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let expected: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_ne!(portable_root(&fs::canonicalize(&alias).unwrap()), alias);
        let detected = Proposal::scan(&alias).unwrap();
        assert_eq!(detected.root, Path::new(expected["root"].as_str().unwrap()));
        detected.install(&profiles, options(&detected, None)).unwrap();
        let file = Path::new(expected["profile"].as_str().unwrap());
        let value: Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
        assert_eq!(value["workspaceRoot"], expected["root"]);
        assert_eq!(resolve(&profiles, &real).unwrap().ext, ["md"]);
    }

    #[cfg(windows)]
    #[test]
    fn profile_names_and_roots_are_compatible_with_windows_installer() {
        assert_eq!(profile_id(Path::new(r"C:\Repos\MyProject")), "c_repos_myproject-22bf3ee8f773");
        assert_eq!(profile_id(Path::new(r"\\?\C:\Repos\MyProject")), profile_id(Path::new(r"C:\Repos\MyProject")));
        assert_eq!(profile_id(Path::new(r"\\?\UNC\server\share\repo")), profile_id(Path::new(r"\\server\share\repo")));
        assert_eq!(portable_root(Path::new(r"\\?\C:\Repos\MyProject")), Path::new(r"C:\Repos\MyProject"));
    }

    #[cfg(windows)]
    #[test]
    fn detection_does_not_follow_directory_junctions() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("README.md"), "").unwrap();
        fs::write(temp.path().join("outside.rs"), "").unwrap();
        let path = root.join("cycle").to_string_lossy().replace('\'', "''");
        let target = temp.path().to_string_lossy().replace('\'', "''");
        let output = crate::background_command("powershell.exe")
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command",
                &format!("$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path '{path}' -Target '{target}' | Out-Null")])
            .output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(Proposal::scan(&root).unwrap().counts.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn detection_does_not_follow_directory_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("README.md"), "").unwrap();
        std::os::unix::fs::symlink(temp.path(), root.join("cycle")).unwrap();
        fs::write(temp.path().join("outside.rs"), "").unwrap();
        assert_eq!(Proposal::scan(&root).unwrap().counts.len(), 1);
    }
}
