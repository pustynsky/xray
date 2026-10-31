use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub(super) const DEFAULT_REPOSITORY: &str = "pustynsky/xray";

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct Repository(String);

impl Default for Repository {
    fn default() -> Self { Self(DEFAULT_REPOSITORY.into()) }
}

impl TryFrom<String> for Repository {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid = value.split_once('/').is_some_and(|(owner, repo)| {
            !owner.is_empty() && owner.len() <= 39 && !repo.is_empty() && repo.len() <= 100
                && owner.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                && owner.as_bytes()[0].is_ascii_alphanumeric()
                && repo.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
                && !matches!(repo, "." | "..")
        });
        if valid { Ok(Self(value)) } else { Err("XRAY_UPDATE_REPOSITORY must be a GitHub owner/repo") }
    }
}

impl From<Repository> for String {
    fn from(repository: Repository) -> Self { repository.0 }
}

impl Repository {
    pub fn name(&self) -> &str { &self.0 }
    pub fn endpoint(&self) -> String { format!("https://api.github.com/repos/{}/releases/latest", self.0) }
    pub fn release_url(&self, tag: &str) -> String { format!("https://github.com/{}/releases/tag/{tag}", self.0) }
    pub fn download_url(&self, tag: &str) -> String { format!("https://github.com/{}/releases/download/{tag}/xray.exe", self.0) }
    pub fn update_script_url(&self, tag: &str) -> String {
        format!("https://raw.githubusercontent.com/{}/{tag}/scripts/setup-xray.ps1", self.0)
    }

    pub fn cache_dir(&self, base: &Path) -> PathBuf {
        if self.0 == DEFAULT_REPOSITORY { base.to_path_buf() } else {
            base.join("release-sources").join(format!("{:x}", Sha256::digest(self.0.as_bytes())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_setting_constructs_only_github_urls() {
        let source = Repository::try_from("new-owner/new_repo".to_string()).unwrap();
        assert_eq!(source.endpoint(), "https://api.github.com/repos/new-owner/new_repo/releases/latest");
        assert_eq!(source.download_url("v1.0.0"), "https://github.com/new-owner/new_repo/releases/download/v1.0.0/xray.exe");
        assert_eq!(source.update_script_url("v1.0.0"), "https://raw.githubusercontent.com/new-owner/new_repo/v1.0.0/scripts/setup-xray.ps1");
        for invalid in ["", "../repo", "owner/..", "https://evil/repo", "a/b/c", "owner/repo?query", "owner/repo#fragment", " a/b", "owner/repo\n"] {
            assert!(Repository::try_from(invalid.to_string()).is_err());
        }
    }

    #[test]
    fn cache_locations_are_separate_for_different_sources() {
        let base = Path::new("cache");
        assert_eq!(Repository::default().cache_dir(base), base);
        let first = Repository::try_from("first/repo".to_string()).unwrap().cache_dir(base);
        let second = Repository::try_from("second/repo".to_string()).unwrap().cache_dir(base);
        assert_ne!(first, second);
        assert!(first.starts_with(base));
    }
}
