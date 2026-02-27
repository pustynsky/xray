use super::*;

#[test]
fn test_io_error_display() {
    let err = SearchError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "file not found",
    ));
    assert!(err.to_string().contains("I/O error"));
    assert!(err.to_string().contains("file not found"));
}

#[test]
fn test_dir_not_found_display() {
    let err = SearchError::DirNotFound("/nonexistent".to_string());
    assert!(err.to_string().contains("/nonexistent"));
}

#[test]
fn test_index_not_found_display() {
    let err = SearchError::IndexNotFound {
        dir: "C:\\Projects".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("C:\\Projects"));
    assert!(msg.contains("content-index"));
}

#[test]
fn test_invalid_regex_display() {
    let regex_err = regex::Regex::new("[invalid").unwrap_err();
    let err = SearchError::InvalidRegex {
        pattern: "[invalid".to_string(),
        source: regex_err,
    };
    assert!(err.to_string().contains("[invalid"));
}

#[test]
fn test_empty_phrase_display() {
    let err = SearchError::EmptyPhrase {
        phrase: "a b".to_string(),
    };
    assert!(err.to_string().contains("a b"));
    assert!(err.to_string().contains("no indexable tokens"));
}

#[test]
fn test_io_error_from_conversion() {
    let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
    let search_err: SearchError = io_err.into();
    assert!(matches!(search_err, SearchError::Io(_)));
}

#[test]
fn test_stale_index_display() {
    let err = SearchError::StaleIndex { age_secs: 7200, max_secs: 3600 };
    let msg = err.to_string();
    assert!(msg.contains("stale"), "StaleIndex should mention 'stale': {}", msg);
    assert!(msg.contains("7200"), "StaleIndex should contain age: {}", msg);
    assert!(msg.contains("3600"), "StaleIndex should contain max: {}", msg);
}

#[test]
fn test_index_load_display() {
    let err = SearchError::IndexLoad {
        path: "C:\\data\\index.bin".to_string(),
        message: "corrupted header".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("C:\\data\\index.bin"), "IndexLoad should contain path: {}", msg);
    assert!(msg.contains("corrupted header"), "IndexLoad should contain message: {}", msg);
}

#[test]
fn test_lock_poisoned_display() {
    let err = SearchError::LockPoisoned("content index mutex".to_string());
    let msg = err.to_string();
    assert!(msg.contains("content index mutex"), "LockPoisoned should contain label: {}", msg);
    assert!(msg.to_lowercase().contains("poison"), "LockPoisoned should mention 'poison': {}", msg);
}

#[test]
fn test_save_failed_display() {
    let err = SearchError::SaveFailed("disk full".to_string());
    let msg = err.to_string();
    assert!(msg.contains("disk full"), "SaveFailed should contain reason: {}", msg);
}

#[test]
fn test_invalid_args_display() {
    let err = SearchError::InvalidArgs("--regex and --phrase are mutually exclusive".to_string());
    let msg = err.to_string();
    assert!(msg.contains("mutually exclusive"), "InvalidArgs should contain message: {}", msg);
}
