//! Tests for grep/substring/phrase/truncation -- extracted from handlers_tests.rs.

use super::*;
use super::grep::handle_xray_grep;
use super::handlers_test_utils::{cleanup_tmp, make_empty_ctx};
use crate::index::build_trigram_index;
use crate::Posting;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
// --- Substring search handler integration tests ---

fn make_substring_ctx(tokens_to_files: Vec<(&str, u32, Vec<u32>)>, files: Vec<&str>) -> HandlerContext {
    let mut index_map: HashMap<String, Vec<Posting>> = HashMap::new();
    for (token, file_id, lines) in &tokens_to_files {
        index_map.entry(token.to_string()).or_default().push(Posting { file_id: *file_id, lines: lines.clone() });
    }
    let file_token_counts: Vec<u32> = {
        let mut counts = vec![0u32; files.len()];
        for (_, file_id, lines) in &tokens_to_files {
            if (*file_id as usize) < counts.len() { counts[*file_id as usize] += lines.len() as u32; }
        }
        counts
    };
    let total_tokens: u64 = file_token_counts.iter().map(|&c| c as u64).sum();
    let trigram = build_trigram_index(&index_map);
    let content_index = ContentIndex {
        root: ".".to_string(),
        files: files.iter().map(|s| s.to_string()).collect(), index: index_map,
        total_tokens, extensions: vec!["cs".to_string()], file_token_counts,
        trigram, ..Default::default()
    };
    HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        ..Default::default()
    }
}

#[test] fn test_substring_xray_finds_partial_match() {
    let ctx = make_substring_ctx(vec![("databaseconnectionfactory", 0, vec![10])], vec!["C:\\test\\Activity.cs"]);
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "databaseconn", "substring": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 1);
}

#[test] fn test_substring_search_no_match() {
    let ctx = make_substring_ctx(vec![("httpclient", 0, vec![5])], vec!["C:\\test\\Program.cs"]);
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "xyznonexistent", "substring": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 0);
}

#[test] fn test_substring_search_full_token_match() {
    let ctx = make_substring_ctx(vec![("httpclient", 0, vec![5, 12])], vec!["C:\\test\\Program.cs"]);
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "httpclient", "substring": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 1);
}

#[test] fn test_substring_search_case_insensitive() {
    let ctx = make_substring_ctx(vec![("httpclient", 0, vec![5])], vec!["C:\\test\\Program.cs"]);
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "HttpCli", "substring": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 1);
}

#[test] fn test_substring_search_short_query_warning() {
    let ctx = make_substring_ctx(vec![("ab_something", 0, vec![1])], vec!["C:\\test\\File.cs"]);
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "ab", "substring": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["warnings"].is_array(),
        "Expected 'warnings' array in summary, got: {}", output["summary"]);
}

#[test] fn test_substring_search_mutually_exclusive_with_regex() {
    let ctx = make_substring_ctx(vec![("httpclient", 0, vec![5])], vec!["C:\\test\\Program.cs"]);
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "http", "substring": true, "regex": true}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("mutually exclusive"));
}

#[test] fn test_substring_search_mutually_exclusive_with_phrase() {
    let ctx = make_substring_ctx(vec![("httpclient", 0, vec![5])], vec!["C:\\test\\Program.cs"]);
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "http", "substring": true, "phrase": true}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("mutually exclusive"));
}

#[test] fn test_substring_search_multi_term_or() {
    let ctx = make_substring_ctx(vec![("httpclient", 0, vec![5]), ("grpchandler", 1, vec![10])], vec!["C:\\test\\Http.cs", "C:\\test\\Grpc.cs"]);
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "httpcli,grpchan", "substring": true, "mode": "or"}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 2);
}

#[test] fn test_substring_search_multi_term_and() {
    let ctx = make_substring_ctx(vec![("httpclient", 0, vec![5]), ("grpchandler", 0, vec![10]), ("grpchandler", 1, vec![20])], vec!["C:\\test\\Both.cs", "C:\\test\\GrpcOnly.cs"]);
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "httpcli,grpchan", "substring": true, "mode": "and"}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 1);
}

#[test] fn test_substring_and_mode_no_false_positive_from_multi_token_match() {
    let ctx = make_substring_ctx(
        vec![
            ("userservice", 0, vec![10]),
            ("servicehelper", 0, vec![20]),
            ("servicemanager", 0, vec![30]),
        ],
        vec!["C:\\test\\ServiceFile.cs"],
    );
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "service,handler",
        "substring": true,
        "mode": "and"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 0,
        "AND mode should require ALL terms to match, not count per-token. Got: {}", output);
}

#[test] fn test_substring_and_mode_correct_when_both_terms_match() {
    let ctx = make_substring_ctx(
        vec![
            ("userservice", 0, vec![10]),
            ("servicehelper", 0, vec![20]),
            ("requesthandler", 0, vec![30]),
        ],
        vec!["C:\\test\\ServiceFile.cs"],
    );
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "service,handler",
        "substring": true,
        "mode": "and"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 1,
        "AND mode should pass when all terms match. Got: {}", output);
}

#[test] fn test_substring_search_count_only() {
    let ctx = make_substring_ctx(vec![("httpclient", 0, vec![5, 12]), ("httphandler", 1, vec![3])], vec!["C:\\test\\Client.cs", "C:\\test\\Handler.cs"]);
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "http", "substring": true, "countOnly": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 2);
    assert!(output.get("files").is_none());
}

#[test]
fn test_substring_search_trigram_dirty_triggers_rebuild() {
    let mut index_map: HashMap<String, Vec<Posting>> = HashMap::new();
    index_map.insert("httpclient".to_string(), vec![Posting { file_id: 0, lines: vec![5] }]);
    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec!["C:\\test\\Program.cs".to_string()], index: index_map,
        total_tokens: 1, extensions: vec!["cs".to_string()], file_token_counts: vec![1],
        trigram_dirty: true,
        ..Default::default()
    };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "httpcli", "substring": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 1);
    let idx = ctx.index.read().unwrap();
    assert!(!idx.trigram_dirty);
    assert!(!idx.trigram.tokens.is_empty());
}

// --- E2E tests ---

fn make_e2e_substring_ctx() -> (HandlerContext, std::path::PathBuf) {
    use std::io::Write;
    static E2E_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = E2E_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_e2e_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    { let mut f = std::fs::File::create(tmp_dir.join("Service.cs")).unwrap();
      writeln!(f, "using System;").unwrap(); writeln!(f, "namespace MyApp {{").unwrap();
      writeln!(f, "    public class DatabaseConnectionFactory {{").unwrap();
      writeln!(f, "        private HttpClientHandler _handler;").unwrap();
      writeln!(f, "        public void Execute() {{").unwrap();
      writeln!(f, "            var provider = new GrpcServiceProvider();").unwrap();
      writeln!(f, "            _handler.Send();").unwrap();
      writeln!(f, "        }}").unwrap(); writeln!(f, "    }}").unwrap(); writeln!(f, "}}").unwrap(); }
    { let mut f = std::fs::File::create(tmp_dir.join("Controller.cs")).unwrap();
      writeln!(f, "using System;").unwrap(); writeln!(f, "namespace MyApp {{").unwrap();
      writeln!(f, "    public class UserController {{").unwrap();
      writeln!(f, "        private readonly HttpClientHandler _client;").unwrap();
      writeln!(f, "        public async Task<IActionResult> GetAsync() {{").unwrap();
      writeln!(f, "            return Ok();").unwrap();
      writeln!(f, "        }}").unwrap(); writeln!(f, "    }}").unwrap(); writeln!(f, "}}").unwrap(); }
    { let mut f = std::fs::File::create(tmp_dir.join("Util.cs")).unwrap();
      writeln!(f, "public static class CacheManagerHelper {{").unwrap();
      writeln!(f, "    public static void ClearAll() {{ }}").unwrap();
      writeln!(f, "}}").unwrap(); }

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp_dir.to_string_lossy().to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp_dir.to_string_lossy().to_string()))),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };
    (ctx, tmp_dir)
}

#[test] fn e2e_substring_search_full_pipeline() {
    let (ctx, tmp_dir) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "databaseconn", "substring": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["totalFiles"].as_u64().unwrap() >= 1);
    let matched = output["summary"]["matchedTokens"].as_array().unwrap();
    assert!(matched.iter().any(|t| t.as_str().unwrap() == "databaseconnectionfactory"));
    cleanup_tmp(&tmp_dir);
}

#[test] fn e2e_substring_search_with_show_lines() {
    let (ctx, tmp_dir) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "grpcservice", "substring": true, "showLines": true, "contextLines": 1}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["totalFiles"].as_u64().unwrap() >= 1);
    let files = output["files"].as_array().unwrap();
    assert!(!files.is_empty());
    assert!(files[0]["lineContent"].is_array());
    cleanup_tmp(&tmp_dir);
}

#[test] fn e2e_reindex_rebuilds_trigram() {
    use std::io::Write;
    let (ctx, tmp_dir) = make_e2e_substring_ctx();
    let r1 = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "cachemanager", "substring": true}));
    let o1: Value = serde_json::from_str(&r1.content[0].text).unwrap();
    assert!(o1["summary"]["totalFiles"].as_u64().unwrap() >= 1);
    std::fs::remove_file(tmp_dir.join("Util.cs")).unwrap();
    { let mut f = std::fs::File::create(tmp_dir.join("NewFile.cs")).unwrap(); writeln!(f, "public class DatabaseConnectionPoolManager {{}}").unwrap(); }
    let _ = dispatch_tool(&ctx, "xray_reindex", &json!({}));
    let r2 = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "cachemanager", "substring": true}));
    let o2: Value = serde_json::from_str(&r2.content[0].text).unwrap();
    assert_eq!(o2["summary"]["totalFiles"], 0);
    let r3 = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "connectionpool", "substring": true}));
    let o3: Value = serde_json::from_str(&r3.content[0].text).unwrap();
    assert!(o3["summary"]["totalFiles"].as_u64().unwrap() >= 1);
    cleanup_tmp(&tmp_dir);
}

#[test] fn e2e_watcher_trigram_dirty_lazy_rebuild() {
    use std::io::Write;
    let (ctx, tmp_dir) = make_e2e_substring_ctx();
    { let mut idx = ctx.index.write().unwrap();
      let new_file_id = idx.files.len() as u32;
      let new_path = tmp_dir.join("Dynamic.cs");
      { let mut f = std::fs::File::create(&new_path).unwrap(); writeln!(f, "public class AsyncBlobStorageProcessor {{}}").unwrap(); }
      idx.files.push(clean_path(&new_path.to_string_lossy()));
      idx.file_token_counts.push(1);
      idx.index.entry("asyncblobstorageprocessor".to_string()).or_default().push(Posting { file_id: new_file_id, lines: vec![1] });
      idx.total_tokens += 1;
      idx.trigram_dirty = true;
    }
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "blobstorage", "substring": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["totalFiles"].as_u64().unwrap() >= 1);
    assert!(!ctx.index.read().unwrap().trigram_dirty);
    cleanup_tmp(&tmp_dir);
}

#[test] fn e2e_index_serialization_roundtrip_with_trigram() {
    let (ctx, tmp_dir) = make_e2e_substring_ctx();
    let original = ctx.index.read().unwrap();
    let orig_files = original.files.len();
    let orig_tokens = original.index.len();
    let orig_trigrams = original.trigram.trigram_map.len();
    let idx_base = tmp_dir.join(".index");
    crate::save_content_index(&original, &idx_base).unwrap();
    let root = original.root.clone(); let exts = original.extensions.join(",");
    drop(original);
    let loaded = crate::load_content_index(&root, &exts, &idx_base).expect("load should succeed");
    assert_eq!(loaded.files.len(), orig_files);
    assert_eq!(loaded.index.len(), orig_tokens);
    assert_eq!(loaded.trigram.trigram_map.len(), orig_trigrams);
    let loaded_ctx = HandlerContext { index: Arc::new(RwLock::new(loaded)), workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(root.to_string()))), ..Default::default() };
    let result = dispatch_tool(&loaded_ctx, "xray_grep", &json!({"terms": "databaseconn", "substring": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["totalFiles"].as_u64().unwrap() >= 1);
    cleanup_tmp(&tmp_dir);
}

#[test] fn e2e_substring_search_multi_term_and() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "httpclient,grpcservice", "substring": true, "mode": "and"}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["totalFiles"].as_u64().unwrap() >= 1);
    cleanup_tmp(&tmp);
}

#[test] fn e2e_substring_search_count_only() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "httpclient", "substring": true, "countOnly": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output.get("files").is_none());
    assert!(output["summary"]["totalFiles"].as_u64().unwrap() >= 2);
    cleanup_tmp(&tmp);
}

#[test] fn e2e_substring_search_with_excludes() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "httpclient", "substring": true, "exclude": ["Controller"]}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();
    for file in files { assert!(!file["path"].as_str().unwrap().contains("Controller")); }
    cleanup_tmp(&tmp);
}

#[test] fn e2e_substring_search_max_results() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "public", "substring": true, "maxResults": 1}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["files"].as_array().unwrap().len() <= 1);
    cleanup_tmp(&tmp);
}

#[test] fn e2e_substring_search_short_query_warning() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "ok", "substring": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["warnings"].is_array(),
        "Expected 'warnings' array in summary, got: {}", output["summary"]);
    cleanup_tmp(&tmp);
}

#[test] fn e2e_substring_mutually_exclusive_with_regex() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "test", "substring": true, "regex": true}));
    assert!(result.is_error);
    cleanup_tmp(&tmp);
}

#[test] fn e2e_substring_mutually_exclusive_with_phrase() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "test", "substring": true, "phrase": true}));
    assert!(result.is_error);
    cleanup_tmp(&tmp);
}

#[test] fn e2e_substring_search_has_scores() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "httpclient", "substring": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();
    for file in files { assert!(file["score"].is_number()); }
    cleanup_tmp(&tmp);
}
// --- Substring-by-default tests (E2E baseline comparison fix) ---

#[test] fn test_substring_default_finds_compound_identifiers() {
    let ctx = make_substring_ctx(
        vec![
            ("storageindexmanager", 0, vec![39]),
            ("istorageindexmanager", 1, vec![5]),
            ("m_storageindexmanager", 2, vec![12]),
        ],
        vec!["C:\\test\\StorageIndexManager.cs", "C:\\test\\IStorageIndexManager.cs", "C:\\test\\Controller.cs"],
    );
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "StorageIndexManager"}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 3,
        "Default substring=true should find compound identifiers. Got: {}", output);
    let mode = output["summary"]["searchMode"].as_str().unwrap();
    assert!(mode.starts_with("substring"), "Expected substring search mode, got: {}", mode);
}

#[test] fn test_substring_false_misses_compound_identifiers() {
    let ctx = make_substring_ctx(
        vec![
            ("storageindexmanager", 0, vec![39]),
            ("istorageindexmanager", 1, vec![5]),
            ("m_storageindexmanager", 2, vec![12]),
        ],
        vec!["C:\\test\\StorageIndexManager.cs", "C:\\test\\IStorageIndexManager.cs", "C:\\test\\Controller.cs"],
    );
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "storageindexmanager", "substring": false}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 1,
        "substring=false should only find exact token match. Got: {}", output);
}

#[test] fn test_regex_auto_disables_substring() {
    let ctx = make_substring_ctx(
        vec![("httpclient", 0, vec![5])],
        vec!["C:\\test\\Program.cs"],
    );
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "http.*", "regex": true}));
    assert!(!result.is_error, "regex=true should auto-disable substring, not error");
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 1);
}

#[test] fn test_phrase_auto_disables_substring() {
    let ctx = make_substring_ctx(
        vec![("new", 0, vec![5]), ("httpclient", 0, vec![5])],
        vec!["C:\\test\\Program.cs"],
    );
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "new httpclient", "phrase": true}));
    assert!(!result.is_error, "phrase=true should auto-disable substring, not error");
}

// --- Phrase post-filter tests (raw content matching) ---

/// Helper: create a temp dir with test files for phrase post-filter tests.
/// Returns (HandlerContext, temp_dir_path).
fn make_phrase_postfilter_ctx() -> (HandlerContext, std::path::PathBuf) {
    use std::io::Write;
    static PHRASE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = PHRASE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_phrase_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    { let mut f = std::fs::File::create(tmp_dir.join("manifest.xml")).unwrap();
      writeln!(f, "<Root>").unwrap();
      writeln!(f, "  <Property Name=\"A\">value</Property> </Property>").unwrap();
      writeln!(f, "  <Other>text</Other>").unwrap();
      writeln!(f, "</Root>").unwrap(); }

    { let mut f = std::fs::File::create(tmp_dir.join("Service.xml")).unwrap();
      writeln!(f, "<Root>").unwrap();
      writeln!(f, "  <Property Name=\"X\">").unwrap();
      writeln!(f, "    <Property Name=\"Y\">inner</Property>").unwrap();
      writeln!(f, "  </Property>").unwrap();
      writeln!(f, "</Root>").unwrap(); }

    { let mut f = std::fs::File::create(tmp_dir.join("Logger.xml")).unwrap();
      writeln!(f, "<Config>").unwrap();
      writeln!(f, "  <Type>ILogger<string></Type>").unwrap();
      writeln!(f, "</Config>").unwrap(); }

    { let mut f = std::fs::File::create(tmp_dir.join("Other.xml")).unwrap();
      writeln!(f, "<Config>").unwrap();
      writeln!(f, "  <Type>ILogger string adapter</Type>").unwrap();
      writeln!(f, "</Config>").unwrap(); }

    { let mut f = std::fs::File::create(tmp_dir.join("AppConfig.xml")).unwrap();
      writeln!(f, "<AppSettings>").unwrap();
      writeln!(f, "  <MaxRetries>1</MaxRetries>").unwrap();
      writeln!(f, "  <TimeoutSeconds>3</TimeoutSeconds>").unwrap();
      writeln!(f, "</AppSettings>").unwrap(); }

    { let mut f = std::fs::File::create(tmp_dir.join("Code.xml")).unwrap();
      writeln!(f, "<Code>").unwrap();
      writeln!(f, "  pub fn main() {{}}").unwrap();
      writeln!(f, "  pub fn helper() {{}}").unwrap();
      writeln!(f, "</Code>").unwrap(); }

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp_dir.to_string_lossy().to_string(),
        ext: "xml".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp_dir.to_string_lossy().to_string()))),
        server_ext: "xml".to_string(),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };
    (ctx, tmp_dir)
}

#[test] fn test_phrase_postfilter_xml_literal_match() {
    let (ctx, tmp) = make_phrase_postfilter_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "</Property> </Property>",
        "phrase": true
    }));
    assert!(!result.is_error, "Phrase search should succeed: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert_eq!(total, 1, "Should find exactly 1 file with literal '</Property> </Property>', got {}", total);
    let files = output["files"].as_array().unwrap();
    let path = files[0]["path"].as_str().unwrap();
    assert!(path.contains("manifest.xml"), "Should match manifest.xml, got {}", path);
    cleanup_tmp(&tmp);
}

#[test] fn test_phrase_postfilter_no_punctuation_uses_regex() {
    let (ctx, tmp) = make_phrase_postfilter_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "pub fn",
        "phrase": true
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert!(total >= 1, "Should find at least 1 file for 'pub fn' phrase (regex mode)");
    let files = output["files"].as_array().unwrap();
    let path = files[0]["path"].as_str().unwrap();
    assert!(path.contains("Code.xml"), "Should match Code.xml, got {}", path);
    cleanup_tmp(&tmp);
}

#[test] fn test_phrase_postfilter_xml_full_tag() {
    let (ctx, tmp) = make_phrase_postfilter_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "<MaxRetries>1</MaxRetries>",
        "phrase": true
    }));
    assert!(!result.is_error, "Phrase search should succeed: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert_eq!(total, 1, "Should find exactly 1 file with literal '<MaxRetries>1</MaxRetries>', got {}", total);
    let files = output["files"].as_array().unwrap();
    let path = files[0]["path"].as_str().unwrap();
    assert!(path.contains("AppConfig.xml"), "Should match AppConfig.xml, got {}", path);
    cleanup_tmp(&tmp);
}

#[test] fn test_phrase_postfilter_angle_brackets() {
    let (ctx, tmp) = make_phrase_postfilter_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "ILogger<string>",
        "phrase": true
    }));
    assert!(!result.is_error, "Phrase search should succeed: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert_eq!(total, 1, "Should find exactly 1 file with literal 'ILogger<string>', got {}", total);
    let files = output["files"].as_array().unwrap();
    let path = files[0]["path"].as_str().unwrap();
    assert!(path.contains("Logger.xml"), "Should match Logger.xml, got {}", path);
    cleanup_tmp(&tmp);
}

#[test] fn test_explicit_substring_true_with_regex_errors() {
    let ctx = make_substring_ctx(
        vec![("httpclient", 0, vec![5])],
        vec!["C:\\test\\Program.cs"],
    );
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "http", "substring": true, "regex": true}));
    assert!(result.is_error, "Explicit substring=true + regex=true should error");
}
#[test] fn test_grep_with_subdir_filter() {
    let tmp_holder = tempfile::tempdir().unwrap();
    let tmp_buf = crate::canonicalize_test_root(tmp_holder.path());
    let tmp = tmp_buf.as_path();
    let sub_a = tmp.join("subA"); let sub_b = tmp.join("subB");
    std::fs::create_dir_all(&sub_a).unwrap(); std::fs::create_dir_all(&sub_b).unwrap();
    std::fs::write(sub_a.join("hello.txt"), "ProductCatalog usage here").unwrap();
    std::fs::write(sub_b.join("other.txt"), "ProductCatalog other usage").unwrap();
    let index = crate::build_content_index(&crate::ContentIndexArgs { dir: tmp.to_string_lossy().to_string(), ext: "txt".to_string(), threads: 1, ..Default::default() }).unwrap();
    let ctx = HandlerContext { index: Arc::new(RwLock::new(index)), workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp.to_string_lossy().to_string()))), server_ext: "txt".to_string(), index_base: tmp.to_path_buf(), ..Default::default() };
    let r_all = handle_xray_grep(&ctx, &json!({"terms": "productcatalog"}));
    let o_all: Value = serde_json::from_str(&r_all.content[0].text).unwrap();
    assert_eq!(o_all["summary"]["totalFiles"], 2);
    let r_sub = handle_xray_grep(&ctx, &json!({"terms": "productcatalog", "dir": sub_a.to_string_lossy().to_string()}));
    assert!(!r_sub.is_error);
    let o_sub: Value = serde_json::from_str(&r_sub.content[0].text).unwrap();
    assert_eq!(o_sub["summary"]["totalFiles"], 1);
}

#[cfg(windows)]
#[test] fn test_grep_rejects_outside_dir() {
    let tmp_holder = tempfile::tempdir().unwrap();
    let tmp = tmp_holder.path();
    let index = ContentIndex { root: tmp.to_string_lossy().to_string(), ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(index)), workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp.to_string_lossy().to_string()))), index_base: tmp.to_path_buf(), ..Default::default() };
    // Drive-letter path Z:\… is only treated as absolute on Windows; on Unix
    // it would be interpreted as a relative path and accepted.
    let result = handle_xray_grep(&ctx, &json!({"terms": "test", "dir": r"Z:\some\other\path"}));
    assert!(result.is_error);
}

// --- Response truncation integration tests ---

#[test]
fn test_response_truncation_triggers_on_large_result() {
    let mut idx = HashMap::new();
    let mut files = Vec::new();
    let mut file_token_counts = Vec::new();

    for i in 0..500 {
        let path = format!(
            "C:\\Projects\\Enterprise\\Solution\\src\\Features\\Module_{:03}\\SubModule\\Implementations\\Component_{:03}Service.cs",
            i / 10, i
        );
        files.push(path);
        file_token_counts.push(1000u32);

        let lines: Vec<u32> = (1..=100).collect();
        idx.entry("common".to_string())
            .or_insert_with(Vec::new)
            .push(Posting { file_id: i as u32, lines });
    }

    let index = ContentIndex {
        root: ".".to_string(),
        files,
        index: idx,
        total_tokens: 500_000,
        extensions: vec!["cs".to_string()],
        file_token_counts,
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(index)),
        metrics: true,
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "common",
        "maxResults": 0,
        "substring": false
    }));

    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    assert_eq!(output["summary"]["totalFiles"], 500);

    assert_eq!(output["summary"]["responseTruncated"], true,
        "Expected responseTruncated=true for 500-file response");
    assert!(output["summary"]["truncationReason"].as_str().is_some(),
        "Expected truncationReason in summary");
    assert!(output["summary"]["hint"].as_str().is_some(),
        "Expected hint in summary");

    let files_arr = output["files"].as_array().unwrap();
    assert!(files_arr.len() < 500,
        "Expected files array to be truncated from 500, got {}", files_arr.len());

    let response_bytes = output["summary"]["responseBytes"].as_u64().unwrap();
    assert!(response_bytes < 20_000,
        "Expected responseBytes < 20000, got {}", response_bytes);
}

#[test]
fn test_response_truncation_does_not_trigger_on_small_result() {
    let mut idx = HashMap::new();
    idx.insert("mytoken".to_string(), vec![Posting { file_id: 0, lines: vec![10, 20] }]);

    let index = ContentIndex {
        root: ".".to_string(),
        files: vec!["C:\\test\\Small.cs".to_string()],
        index: idx,
        total_tokens: 50,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50],
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(index)),
        metrics: true,
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "mytoken", "substring": false}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    assert!(output["summary"].get("responseTruncated").is_none(),
        "Small response should not have responseTruncated");
    assert_eq!(output["summary"]["totalFiles"], 1);
    assert_eq!(output["files"].as_array().unwrap().len(), 1);
}
// ─── Response truncation via small budget ──────────────────────────

#[test]
fn test_xray_grep_response_truncation_via_small_budget() {
    let mut idx = HashMap::new();
    let mut files = Vec::new();
    let mut file_token_counts = Vec::new();

    for i in 0..100 {
        let path = format!(
            "C:\\Projects\\Module_{:03}\\Component_{:03}Service.cs",
            i / 10, i
        );
        files.push(path);
        file_token_counts.push(100u32);
        let lines: Vec<u32> = (1..=20).collect();
        idx.entry("targettoken".to_string())
            .or_insert_with(Vec::new)
            .push(Posting { file_id: i as u32, lines });
    }

    let index = ContentIndex {
        root: ".".to_string(),
        files,
        index: idx,
        total_tokens: 10_000,
        extensions: vec!["cs".to_string()],
        file_token_counts,
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(index)),
        metrics: true,
        max_response_bytes: 2_000,
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "targettoken",
        "maxResults": 0,
        "substring": false
    }));

    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    assert_eq!(output["summary"]["totalFiles"], 100);

    assert_eq!(output["summary"]["responseTruncated"], true,
        "Expected responseTruncated=true for 100-file response with 2KB budget");
    assert!(output["summary"]["truncationReason"].as_str().is_some(),
        "Expected truncationReason in summary");

    let files_arr = output["files"].as_array().unwrap();
    assert!(files_arr.len() < 100,
        "Expected files array to be truncated from 100, got {}", files_arr.len());
}
// ═══════════════════════════════════════════════════════════════════════
// Batch 3 tests — Nice-to-have edge cases
// ═══════════════════════════════════════════════════════════════════════

/// T39 — xray_grep SQL extension filter.
#[test]
fn test_xray_grep_sql_extension_filter() {
    let mut idx = HashMap::new();
    idx.insert("createtable".to_string(), vec![
        Posting { file_id: 0, lines: vec![5] },
        Posting { file_id: 1, lines: vec![10] },
        Posting { file_id: 2, lines: vec![3] },
    ]);

    let index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "C:\\src\\Schema.sql".to_string(),
            "C:\\src\\Service.cs".to_string(),
            "C:\\src\\Migration.sql".to_string(),
        ],
        index: idx,
        total_tokens: 100,
        extensions: vec!["cs".to_string(), "sql".to_string()],
        file_token_counts: vec![50, 50, 50],
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(index)),
        server_ext: "cs,sql".to_string(),
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "createtable",
        "ext": "sql",
        "substring": false
    }));
    assert!(!result.is_error, "grep with ext=sql should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 2,
        "Should find exactly 2 .sql files, got: {}", output["summary"]["totalFiles"]);
    let files = output["files"].as_array().unwrap();
    for file in files {
        let path = file["path"].as_str().unwrap();
        assert!(path.ends_with(".sql"),
            "All results should be .sql files, but found: {}", path);
    }

    let result_all = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "createtable",
        "substring": false
    }));
    assert!(!result_all.is_error);
    let output_all: Value = serde_json::from_str(&result_all.content[0].text).unwrap();
    assert_eq!(output_all["summary"]["totalFiles"], 3,
        "Without ext filter should find all 3 files");
}

/// T71 — xray_grep SQL phrase search with showLines.
#[test]
fn test_xray_grep_phrase_search_with_show_lines() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_phrase_sql_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    {
        let mut f = std::fs::File::create(tmp_dir.join("schema.sql")).unwrap();
        writeln!(f, "-- Database schema").unwrap();
        writeln!(f, "CREATE TABLE Users (").unwrap();
        writeln!(f, "    Id INT PRIMARY KEY,").unwrap();
        writeln!(f, "    Name NVARCHAR(100)").unwrap();
        writeln!(f, ");").unwrap();
        writeln!(f, "CREATE TABLE Orders (").unwrap();
        writeln!(f, "    OrderId INT PRIMARY KEY").unwrap();
        writeln!(f, ");").unwrap();
    }
    {
        let mut f = std::fs::File::create(tmp_dir.join("other.sql")).unwrap();
        writeln!(f, "-- No create table here").unwrap();
        writeln!(f, "SELECT * FROM Users;").unwrap();
    }

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp_dir.to_string_lossy().to_string(),
        ext: "sql".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp_dir.to_string_lossy().to_string()))),
        server_ext: "sql".to_string(),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "CREATE TABLE",
        "phrase": true,
        "showLines": true
    }));
    assert!(!result.is_error, "Phrase search should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert!(total >= 1, "Should find at least 1 file with 'CREATE TABLE' phrase, got {}", total);

    let files = output["files"].as_array().unwrap();
    assert!(!files.is_empty(), "Files array should not be empty");
    let first_file = &files[0];
    assert!(first_file["lineContent"].is_array(),
        "showLines=true should produce lineContent array");
    let line_content = first_file["lineContent"].as_array().unwrap();
    assert!(!line_content.is_empty(), "lineContent should have entries");

    cleanup_tmp(&tmp_dir);
}
/// T82 — xray_grep maxResults=0 semantics.
#[test]
fn test_xray_grep_max_results_zero_means_unlimited() {
    let mut idx = HashMap::new();
    let mut files = Vec::new();
    let mut file_token_counts = Vec::new();

    for i in 0..25 {
        let path = format!("C:\\src\\Module_{:02}\\Service.cs", i);
        files.push(path);
        file_token_counts.push(50u32);
        idx.entry("commontoken".to_string())
            .or_insert_with(Vec::new)
            .push(Posting { file_id: i as u32, lines: vec![10] });
    }

    let index = ContentIndex {
        root: ".".to_string(),
        files,
        index: idx,
        total_tokens: 1000,
        extensions: vec!["cs".to_string()],
        file_token_counts,
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(index)),
        def_index: None,
        max_response_bytes: 0,
        ..Default::default()
    };

    let result_unlimited = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "commontoken",
        "maxResults": 0,
        "substring": false
    }));
    assert!(!result_unlimited.is_error);
    let output_unlimited: Value = serde_json::from_str(&result_unlimited.content[0].text).unwrap();
    assert_eq!(output_unlimited["summary"]["totalFiles"], 25);
    let files_unlimited = output_unlimited["files"].as_array().unwrap();
    assert_eq!(files_unlimited.len(), 25,
        "maxResults=0 should return all 25 files (unlimited), got {}", files_unlimited.len());

    let result_capped = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "commontoken",
        "maxResults": 5,
        "substring": false
    }));
    assert!(!result_capped.is_error);
    let output_capped: Value = serde_json::from_str(&result_capped.content[0].text).unwrap();
    assert_eq!(output_capped["summary"]["totalFiles"], 25,
        "totalFiles in summary should reflect full count (25)");
    let files_capped = output_capped["files"].as_array().unwrap();
    assert_eq!(files_capped.len(), 5,
        "maxResults=5 should return exactly 5 files, got {}", files_capped.len());

    let result_default = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "commontoken",
        "substring": false
    }));
    assert!(!result_default.is_error);
    let output_default: Value = serde_json::from_str(&result_default.content[0].text).unwrap();
    let files_default = output_default["files"].as_array().unwrap();
    assert_eq!(files_default.len(), 25,
        "Default maxResults=50 should return all 25 files when total < 50, got {}", files_default.len());
}
/// xray_grep phrase mode: results sorted by number of occurrences descending.
#[test]
fn test_xray_grep_phrase_sort_by_occurrences() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_grep_phrase_rank_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // File with 1 occurrence
    {
        let mut f = std::fs::File::create(tmp_dir.join("one.cs")).unwrap();
        writeln!(f, "// some code").unwrap();
        writeln!(f, "var result = user service call;").unwrap();
        writeln!(f, "// end").unwrap();
    }
    // File with 3 occurrences
    {
        let mut f = std::fs::File::create(tmp_dir.join("three.cs")).unwrap();
        writeln!(f, "var a = user service one;").unwrap();
        writeln!(f, "var b = user service two;").unwrap();
        writeln!(f, "// middle").unwrap();
        writeln!(f, "var c = user service three;").unwrap();
    }
    // File with 2 occurrences
    {
        let mut f = std::fs::File::create(tmp_dir.join("two.cs")).unwrap();
        writeln!(f, "var x = user service alpha;").unwrap();
        writeln!(f, "// gap").unwrap();
        writeln!(f, "var y = user service beta;").unwrap();
    }

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp_dir.to_string_lossy().to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp_dir.to_string_lossy().to_string()))),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "user service",
        "phrase": true
    }));
    assert!(!result.is_error, "Phrase search should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();

    assert!(files.len() >= 2, "Should find at least 2 files with 'user service' phrase, got {}", files.len());

    // Verify descending order by occurrences
    for i in 0..files.len() - 1 {
        let occ_a = files[i]["occurrences"].as_u64().unwrap();
        let occ_b = files[i + 1]["occurrences"].as_u64().unwrap();
        assert!(occ_a >= occ_b,
            "Phrase results should be sorted by occurrences descending: file at pos {} has {} occurrences, file at pos {} has {}",
            i, occ_a, i + 1, occ_b);
    }

    cleanup_tmp(&tmp_dir);
}

/// xray_grep phrase mode: tie-break by file path (lexicographic ascending) when
/// occurrences are equal. Without a deterministic secondary key, the parallel
/// candidate verification (tier-A) leaves equal-occurrences files in
/// worker-completion order, so the truncated `max_results` slice is unstable
/// across runs of the same query.
#[test]
fn test_xray_grep_phrase_sort_tie_break_by_path() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_grep_phrase_tiebreak_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Create 6 files, each with exactly 1 occurrence of the same phrase.
    // Names chosen so that lexicographic order is unambiguous and not the
    // same as filesystem-creation order.
    let names = ["zzz.cs", "aaa.cs", "mmm.cs", "bbb.cs", "yyy.cs", "ccc.cs"];
    for name in &names {
        let mut f = std::fs::File::create(tmp_dir.join(name)).unwrap();
        writeln!(f, "var x = user service one;").unwrap();
    }

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp_dir.to_string_lossy().to_string(),
        threads: 4, // exercise parallel candidate verification (tier-A)
        ..Default::default()
    }).unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp_dir.to_string_lossy().to_string()))),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };

    // Run the same query several times; the result order must be stable AND
    // ascending by file path among the (all-equal) occurrence groups.
    let mut prev_paths: Option<Vec<String>> = None;
    for run in 0..5 {
        let result = dispatch_tool(&ctx, "xray_grep", &json!({
            "terms": "user service",
            "phrase": true,
        }));
        assert!(!result.is_error, "Run {}: phrase search should not error: {}", run, result.content[0].text);
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        let files = output["files"].as_array().unwrap();
        assert_eq!(files.len(), names.len(), "Run {}: expected all {} files", run, names.len());

        let paths: Vec<String> = files.iter()
            .map(|f| f["path"].as_str().unwrap().to_string())
            .collect();

        // All occurrences are equal (1) → file paths must be sorted ascending.
        for i in 0..paths.len() - 1 {
            assert!(paths[i] <= paths[i + 1],
                "Run {}: tie-broken paths must be ascending; got {:?} before {:?}",
                run, paths[i], paths[i + 1]);
        }

        // And identical between runs.
        if let Some(ref prev) = prev_paths {
            assert_eq!(prev, &paths,
                "Run {}: result order is not deterministic between identical queries", run);
        }
        prev_paths = Some(paths);
    }

    cleanup_tmp(&tmp_dir);
}

/// Tier-A regression (sibling of `test_xray_grep_phrase_sort_tie_break_by_path`):
/// `handle_multi_phrase_search` had the same single-key sort that left
/// equal-occurrences files in worker-completion order. Now that the secondary
/// path key is in place, multi-phrase results must be deterministic across
/// runs and ascending-by-path within an equal-occurrences group.
#[test]
fn test_xray_grep_multi_phrase_sort_tie_break_by_path() {

    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_grep_mphrase_tiebreak_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let names = ["zzz.cs", "aaa.cs", "mmm.cs", "bbb.cs", "yyy.cs", "ccc.cs"];
    for name in &names {
        // Every file contains BOTH phrases exactly once → phrase-or count is 2
        // for every file, so any non-deterministic order will surface here.
        let mut f = std::fs::File::create(tmp_dir.join(name)).unwrap();
        writeln!(f, "alpha foo bar baz").unwrap();
        writeln!(f, "qux quux corge").unwrap();
    }

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp_dir.to_string_lossy().to_string(),
        threads: 4,
        ..Default::default()
    }).unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp_dir.to_string_lossy().to_string()))),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };

    let mut prev_paths: Option<Vec<String>> = None;
    for run in 0..5 {
        let result = dispatch_tool(&ctx, "xray_grep", &json!({
            "terms": "foo bar,qux quux",
            "phrase": true,
        }));
        assert!(!result.is_error, "Run {}: multi-phrase search should not error: {}", run, result.content[0].text);
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        let mode = output["summary"]["searchMode"].as_str().unwrap_or("");
        assert_eq!(mode, "phrase-or", "Run {}: expected phrase-or mode, got {}", run, mode);
        let files = output["files"].as_array().unwrap();
        assert_eq!(files.len(), names.len(), "Run {}: expected all {} files", run, names.len());

        let paths: Vec<String> = files.iter()
            .map(|f| f["path"].as_str().unwrap().to_string())
            .collect();
        for i in 0..paths.len() - 1 {
            assert!(paths[i] <= paths[i + 1],
                "Run {}: tie-broken paths must be ascending; got {:?} before {:?}",
                run, paths[i], paths[i + 1]);
        }
        if let Some(ref prev) = prev_paths {
            assert_eq!(prev, &paths,
                "Run {}: multi-phrase order is not deterministic between identical queries", run);
        }
        prev_paths = Some(paths);
    }

    cleanup_tmp(&tmp_dir);
}

/// Tier-A regression (sibling of `test_xray_grep_phrase_sort_tie_break_by_path`):
/// `handle_line_regex_search` had the same single-key sort. lineRegex mode
/// also benefits from the secondary path key for stable result truncation.
#[test]
fn test_xray_grep_line_regex_sort_tie_break_by_path() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_grep_lineregex_tiebreak_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let names = ["zzz.cs", "aaa.cs", "mmm.cs", "bbb.cs", "yyy.cs", "ccc.cs"];
    for name in &names {
        // Every file has exactly one line that the line-regex matches —
        // occurrence count is 1 for every file. Use a class-declaration line
        // anchored at start to exercise lineRegex (`^...`) semantics.
        let mut f = std::fs::File::create(tmp_dir.join(name)).unwrap();
        writeln!(f, "public class HeadingLine {{}}").unwrap();
        writeln!(f, "some body text").unwrap();
    }


    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp_dir.to_string_lossy().to_string(),
        threads: 4,
        ..Default::default()
    }).unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp_dir.to_string_lossy().to_string()))),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };

    let mut prev_paths: Option<Vec<String>> = None;
    for run in 0..5 {
        let result = dispatch_tool(&ctx, "xray_grep", &json!({
            "terms": "^public class",
            "regex": true,
            "lineRegex": true,
        }));
        assert!(!result.is_error, "Run {}: lineRegex search should not error: {}", run, result.content[0].text);
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        let mode = output["summary"]["searchMode"].as_str().unwrap_or("");
        assert!(mode.starts_with("lineRegex"), "Run {}: expected lineRegex mode, got {}", run, mode);
        let files = output["files"].as_array().unwrap();
        assert_eq!(files.len(), names.len(), "Run {}: expected all {} files", run, names.len());

        let paths: Vec<String> = files.iter()

            .map(|f| f["path"].as_str().unwrap().to_string())
            .collect();
        for i in 0..paths.len() - 1 {
            assert!(paths[i] <= paths[i + 1],
                "Run {}: tie-broken paths must be ascending; got {:?} before {:?}",
                run, paths[i], paths[i + 1]);
        }
        if let Some(ref prev) = prev_paths {
            assert_eq!(prev, &paths,
                "Run {}: lineRegex order is not deterministic between identical queries", run);
        }
        prev_paths = Some(paths);
    }

    cleanup_tmp(&tmp_dir);
}

/// Tier-B regression (end-to-end pruning): the `intersect_sorted_unique` helper
/// is unit-tested directly, but no test exercised the full handler path that
/// drives line-level posting intersection. The phrase contract is
/// "all tokens on a single line" — a file with token A on line 1 and token B
/// on line 2 must NOT match, while a file with both tokens on the same line
/// must match. Without the line-level pruning the false-match file would be
/// opened and verified by the per-line scan and correctly rejected; with
/// pruning it is dropped from the candidate list before any read. Both paths
/// should produce the same final result — this test pins that contract.
#[test]
fn test_xray_grep_phrase_line_level_pruning_same_line_vs_split_lines() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_grep_phrase_pruning_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // matching.cs: both tokens on one line — must be returned.
    let mut f = std::fs::File::create(tmp_dir.join("matching.cs")).unwrap();
    writeln!(f, "public class CustomerService {{}}").unwrap();
    writeln!(f, "unrelated noise").unwrap();

    // split.cs: tokens present in the file but on DIFFERENT lines — file-level
    // intersection passes (both file_ids appear in both posting lists), but
    // line-level intersection must drop this file.
    let mut f = std::fs::File::create(tmp_dir.join("split.cs")).unwrap();
    writeln!(f, "public interface IFoo {{}}").unwrap();
    writeln!(f, "class CustomerRepository {{}}").unwrap();

    // unrelated.cs: only one token — must be excluded by file-level intersection.
    let mut f = std::fs::File::create(tmp_dir.join("unrelated.cs")).unwrap();
    writeln!(f, "public class Foo {{}}").unwrap();

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp_dir.to_string_lossy().to_string(),
        threads: 4,
        ..Default::default()
    }).unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp_dir.to_string_lossy().to_string()))),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "public class",
        "phrase": true,
    }));
    assert!(!result.is_error, "phrase search should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();
    let paths: Vec<String> = files.iter()
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();

    assert!(paths.iter().any(|p| p.ends_with("matching.cs")),
        "matching.cs (both tokens on the same line) must be in results: {:?}", paths);
    assert!(paths.iter().any(|p| p.ends_with("unrelated.cs")),
        "unrelated.cs (single-token line) must match phrase 'public class' (it literally contains it): {:?}", paths);
    // Crucial: split.cs has tokens on different lines — must be excluded.
    assert!(!paths.iter().any(|p| p.ends_with("split.cs")),
        "split.cs (tokens on different lines) must be pruned out, but appeared in: {:?}", paths);

    cleanup_tmp(&tmp_dir);
}

/// BUG-6: xray_grep with contextLines>0 should auto-enable showLines.
#[test]
fn test_xray_grep_context_lines_auto_enables_show_lines() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    // contextLines=3 without explicit showLines=true
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "httpclient",
        "contextLines": 3
    }));
    assert!(!result.is_error, "contextLines without showLines should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();
    if !files.is_empty() {
        assert!(files[0].get("lineContent").is_some(),
            "contextLines>0 should auto-enable showLines, but lineContent is missing");
    }
    cleanup_tmp(&tmp);
}
/// Regression: readErrors/lossyUtf8Files should appear in ALL grep summary modes.
/// Self-review (2026-02-26) found they were only in normal token mode, missing from
/// 5 other summary builders (substring, phrase, countOnly variants).
#[test]
fn test_read_errors_in_substring_summary() {
    let mut idx = HashMap::new();
    idx.insert("httpclient".to_string(), vec![Posting { file_id: 0, lines: vec![5] }]);
    let trigram = build_trigram_index(&idx);
    let index = ContentIndex {
        root: ".".to_string(),
        files: vec!["C:\\test\\Program.cs".to_string()], index: idx,
        total_tokens: 1, extensions: vec!["cs".to_string()], file_token_counts: vec![1],
        trigram,
        read_errors: 3, lossy_file_count: 2,
        ..Default::default()
    };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(index)),
        ..Default::default()
    };
    // Substring mode
    let result = handle_xray_grep(&ctx, &json!({"terms": "httpcli", "substring": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["readErrors"], 3,
        "readErrors should appear in substring summary");
    assert_eq!(output["summary"]["lossyUtf8Files"], 2,
        "lossyUtf8Files should appear in substring summary");

    // CountOnly mode
    let result2 = handle_xray_grep(&ctx, &json!({"terms": "httpcli", "substring": true, "countOnly": true}));
    assert!(!result2.is_error);
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    assert_eq!(output2["summary"]["readErrors"], 3,
        "readErrors should appear in countOnly substring summary");
}

/// BUG-7: xray_grep substring matchedTokens should be filtered by dir/ext/exclude.
///
/// Windows-only: hard-coded `C:\\project\\…` paths and a `C:\\project\\dir_a`
/// dir filter — on Unix the prefix match doesn't fire.
#[cfg(windows)]
#[test]
fn test_substring_matched_tokens_filtered_by_dir() {
    // Two files in different directories, each with a unique token containing "service"
    let ctx = make_substring_ctx(
        vec![
            ("userservice", 0, vec![10]),       // in dir_a
            ("servicehelper", 1, vec![20]),      // in dir_b
            ("orderservice", 0, vec![30]),       // in dir_a
        ],
        vec![
            "C:\\project\\dir_a\\FileA.cs",
            "C:\\project\\dir_b\\FileB.cs",
        ],
    );
    // Override server_dir to match the file paths
    let ctx = HandlerContext {
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned("C:\\project".to_string()))),
        ..ctx
    };

    // Search with dir filter restricting to dir_a only
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "service",
        "substring": true,
        "dir": "C:\\project\\dir_a"
    }));
    assert!(!result.is_error, "Grep should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    // Should find files only in dir_a
    assert_eq!(output["summary"]["totalFiles"], 1,
        "Should find 1 file in dir_a, got: {}", output["summary"]["totalFiles"]);

    // matchedTokens should NOT contain "servicehelper" (only in dir_b)
    let matched_tokens = output["summary"]["matchedTokens"].as_array().unwrap();
    let token_names: Vec<&str> = matched_tokens.iter()
        .filter_map(|t| t.as_str())
        .collect();

    assert!(token_names.contains(&"userservice"),
        "matchedTokens should contain 'userservice' (in dir_a). Got: {:?}", token_names);
    assert!(token_names.contains(&"orderservice"),
        "matchedTokens should contain 'orderservice' (in dir_a). Got: {:?}", token_names);
    assert!(!token_names.contains(&"servicehelper"),
        "BUG-7: matchedTokens should NOT contain 'servicehelper' (only in dir_b). Got: {:?}", token_names);
}

/// BUG-7: matchedTokens filtered by ext filter.
#[test]
fn test_substring_matched_tokens_filtered_by_ext() {
    let ctx = make_substring_ctx(
        vec![
            ("userservice", 0, vec![10]),       // .cs file
            ("serviceconfig", 1, vec![20]),     // .xml file
        ],
        vec![
            "C:\\project\\Service.cs",
            "C:\\project\\Config.xml",
        ],
    );

    // Search with ext filter restricting to .cs only
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "service",
        "substring": true,
        "ext": "cs"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    assert_eq!(output["summary"]["totalFiles"], 1);

    let matched_tokens = output["summary"]["matchedTokens"].as_array().unwrap();
    let token_names: Vec<&str> = matched_tokens.iter()
        .filter_map(|t| t.as_str())
        .collect();

    assert!(token_names.contains(&"userservice"),
        "matchedTokens should contain 'userservice' (.cs file). Got: {:?}", token_names);
    assert!(!token_names.contains(&"serviceconfig"),
        "BUG-7: matchedTokens should NOT contain 'serviceconfig' (.xml file). Got: {:?}", token_names);
}

/// BUG-7: matchedTokens filtered by exclude filter.
#[test]
fn test_substring_matched_tokens_filtered_by_exclude() {
    let ctx = make_substring_ctx(
        vec![
            ("userservice", 0, vec![10]),       // production file
            ("servicemock", 1, vec![20]),        // mock file
        ],
        vec![
            "C:\\project\\Service.cs",
            "C:\\project\\ServiceMock.cs",
        ],
    );

    // Search with exclude filter removing mock files
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "service",
        "substring": true,
        "exclude": ["Mock"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    assert_eq!(output["summary"]["totalFiles"], 1);

    let matched_tokens = output["summary"]["matchedTokens"].as_array().unwrap();
    let token_names: Vec<&str> = matched_tokens.iter()
        .filter_map(|t| t.as_str())
        .collect();

    assert!(token_names.contains(&"userservice"),
        "matchedTokens should contain 'userservice'. Got: {:?}", token_names);
    assert!(!token_names.contains(&"servicemock"),
        "BUG-7: matchedTokens should NOT contain 'servicemock' (excluded). Got: {:?}", token_names);
}

/// BUG-7: matchedTokens empty when no files match (countOnly mode).
#[test]
fn test_substring_matched_tokens_empty_when_no_files_match() {
    let ctx = make_substring_ctx(
        vec![
            ("servicehelper", 0, vec![10]),
        ],
        vec![
            "C:\\project\\dir_b\\FileB.cs",
        ],
    );
    let ctx = HandlerContext {
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned("C:\\project".to_string()))),
        ..ctx
    };

    // Search in dir_a (no files there)
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "service",
        "substring": true,
        "dir": "C:\\project\\dir_a",
        "countOnly": true
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    assert_eq!(output["summary"]["totalFiles"], 0);
    // With countOnly=true, matchedTokens should be absent from the response
    // (Block A fix: suppress matchedTokens in countOnly mode to avoid false truncation).
    assert!(output["summary"].get("matchedTokens").is_none(),
        "matchedTokens should be absent in countOnly mode. Got: {:?}",
        output["summary"]["matchedTokens"]);
}
// ─── Substring auto-switch to phrase for spaced terms tests ─────────

// ─── US-16: Substring auto-switch to phrase for spaced terms ────────

/// US-16: xray_grep with default substring mode and spaced terms auto-switches to phrase.
#[test]
fn test_substring_space_in_terms_auto_switches_to_phrase() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    // "public class" contains a space — should auto-switch to phrase mode
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "public class"
    }));
    assert!(!result.is_error, "Spaced terms should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    // Should find files (phrase mode finds "public class" in the test files)
    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert!(total >= 1, "Should find at least 1 file with 'public class', got 0 (before fix this was 0)");

    // searchMode should indicate phrase
    let mode = output["summary"]["searchMode"].as_str().unwrap_or("");
    assert_eq!(mode, "phrase", "Should auto-switch to phrase mode, got: {}", mode);

    // searchModeNote should explain the auto-switch
    let note = output["summary"]["searchModeNote"].as_str();
    assert!(note.is_some(), "Should have searchModeNote explaining auto-switch");
    assert!(note.unwrap().contains("spaces"), "Note should mention spaces: {}", note.unwrap());

    cleanup_tmp(&tmp);
}

/// US-16: Spaced terms with countOnly=true also auto-switches to phrase.
#[test]
fn test_substring_space_in_terms_count_only() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "public class",
        "countOnly": true
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert!(total >= 1, "countOnly with spaced terms should still find files");
    assert!(output.get("files").is_none(), "countOnly should not have files array");
    cleanup_tmp(&tmp);
}

/// US-16: Non-spaced terms still use substring mode (no auto-switch).
#[test]
fn test_substring_no_space_stays_substring() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "httpclient"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let mode = output["summary"]["searchMode"].as_str().unwrap_or("");
    assert!(mode.starts_with("substring"), "Non-spaced terms should stay in substring mode, got: {}", mode);
    assert!(output["summary"].get("searchModeNote").is_none(),
        "Non-spaced terms should NOT have searchModeNote");
    cleanup_tmp(&tmp);
}

/// US-16: E2E with SQL files — "CREATE TABLE" should find results via auto-switch.
#[test]
fn test_substring_space_sql_create_table() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_space_sql_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    {
        let mut f = std::fs::File::create(tmp_dir.join("schema.sql")).unwrap();
        writeln!(f, "CREATE TABLE Users (Id INT PRIMARY KEY);").unwrap();
        writeln!(f, "CREATE TABLE Orders (OrderId INT);").unwrap();
    }
    {
        let mut f = std::fs::File::create(tmp_dir.join("sproc.sql")).unwrap();
        writeln!(f, "CREATE PROCEDURE dsp_GetUsers AS SELECT * FROM Users;").unwrap();
    }

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp_dir.to_string_lossy().to_string(),
        ext: "sql".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp_dir.to_string_lossy().to_string()))),
        server_ext: "sql".to_string(),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };

    // "CREATE TABLE" with default substring mode — should auto-switch to phrase
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "CREATE TABLE"
    }));
    assert!(!result.is_error, "CREATE TABLE search should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert_eq!(total, 1, "Should find exactly 1 file with 'CREATE TABLE', got {}", total);
    assert_eq!(output["summary"]["searchMode"], "phrase");
    assert!(output["summary"]["searchModeNote"].as_str().is_some());

    // "CREATE PROCEDURE" — should also find via auto-switch
    let result2 = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "CREATE PROCEDURE"
    }));
    assert!(!result2.is_error);
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    assert_eq!(output2["summary"]["totalFiles"], 1);

    cleanup_tmp(&tmp_dir);
}

// ═══════════════════════════════════════════════════════════════════════
// Multi-phrase OR/AND search tests (bug fix: comma-separated phrases)
// ═══════════════════════════════════════════════════════════════════════

/// Multi-phrase OR: auto-switch from substring mode when terms have spaces.
/// Each comma-separated term with space should be searched as a separate phrase.
#[test]
fn test_multi_phrase_or_auto_switch() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    // "public class" and "private readonly" both exist in test files
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "public class,private readonly"
    }));
    assert!(!result.is_error, "Multi-phrase OR should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert!(total >= 2, "Should find at least 2 files (Service.cs has 'public class', Controller.cs has 'private readonly'), got {}", total);

    // searchMode should reflect phrase-or (via auto-switch)
    let mode = output["summary"]["searchMode"].as_str().unwrap_or("");
    assert!(mode == "phrase-or" || mode == "phrase",
        "Expected phrase-or or phrase mode, got: {}", mode);

    // termsSearched should be individual phrases, not the whole string
    let terms = output["summary"]["termsSearched"].as_array().unwrap();
    assert!(terms.len() >= 2, "termsSearched should have at least 2 entries, got: {:?}", terms);

    cleanup_tmp(&tmp);
}

/// Multi-phrase OR: explicit phrase:true with comma-separated terms.
#[test]
fn test_multi_phrase_or_explicit_phrase() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "public class,private readonly",
        "phrase": true
    }));
    assert!(!result.is_error, "Multi-phrase explicit should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert!(total >= 2, "Should find at least 2 files via explicit phrase:true multi-phrase, got {}", total);

    let mode = output["summary"]["searchMode"].as_str().unwrap_or("");
    assert_eq!(mode, "phrase-or", "Expected phrase-or mode for explicit multi-phrase, got: {}", mode);

    let terms = output["summary"]["termsSearched"].as_array().unwrap();
    assert_eq!(terms.len(), 2, "termsSearched should have exactly 2 entries, got: {:?}", terms);

    cleanup_tmp(&tmp);
}

/// Multi-phrase AND: only files containing ALL phrases.
#[test]
fn test_multi_phrase_and_explicit_phrase() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    // "public class" exists in all 3 test files, "private readonly" only in Controller.cs
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "public class,private readonly",
        "phrase": true,
        "mode": "and"
    }));
    assert!(!result.is_error, "Multi-phrase AND should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    // Only Controller.cs has both "public class" and "private readonly"
    assert!(total >= 1, "AND mode should find at least 1 file with both phrases, got {}", total);
    // Make sure AND is stricter than OR
    assert!(total <= 2, "AND mode should find fewer files than OR mode, got {}", total);

    let mode = output["summary"]["searchMode"].as_str().unwrap_or("");
    assert_eq!(mode, "phrase-and", "Expected phrase-and mode, got: {}", mode);

    cleanup_tmp(&tmp);
}

/// Regression: single phrase with spaces still works (no comma → not multi-phrase).
#[test]
fn test_single_phrase_regression_no_comma() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "public class"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert!(total >= 1, "Single phrase should still find files, got 0");

    let mode = output["summary"]["searchMode"].as_str().unwrap_or("");
    assert_eq!(mode, "phrase", "Single phrase should use 'phrase' mode (not phrase-or), got: {}", mode);

    // termsSearched should be a single entry
    let terms = output["summary"]["termsSearched"].as_array().unwrap();
    assert_eq!(terms.len(), 1, "Single phrase should have 1 entry in termsSearched, got: {:?}", terms);

    cleanup_tmp(&tmp);
}

/// Regression: tokens without spaces + explicit phrase:false → still uses substring mode.
#[test]
fn test_tokens_no_spaces_stays_substring() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "httpclient,grpcservice"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    let mode = output["summary"]["searchMode"].as_str().unwrap_or("");
    assert!(mode.starts_with("substring"), "Non-spaced terms should stay in substring mode, got: {}", mode);

    cleanup_tmp(&tmp);
}

/// Multi-phrase countOnly works correctly.
#[test]
fn test_multi_phrase_count_only() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "public class,private readonly",
        "countOnly": true
    }));
    assert!(!result.is_error, "Multi-phrase countOnly should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert!(total >= 2, "countOnly multi-phrase should find at least 2 files, got {}", total);
    assert!(output.get("files").is_none(), "countOnly should not have files array");

    cleanup_tmp(&tmp);
}

/// Multi-phrase with explicit phrase:true and countOnly.
#[test]
fn test_multi_phrase_explicit_count_only() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "public class,private readonly",
        "phrase": true,
        "countOnly": true
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert!(total >= 2, "explicit phrase countOnly should find at least 2 files, got {}", total);
    assert!(output.get("files").is_none());

    let mode = output["summary"]["searchMode"].as_str().unwrap_or("");
    assert_eq!(mode, "phrase-or", "Expected phrase-or mode, got: {}", mode);

    cleanup_tmp(&tmp);
}

/// Bug scenario from user story: "fn handle_xray_definitions,fn build_caller_tree"
/// should find files with either function.
#[test]
fn test_multi_phrase_fn_signatures() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_multi_phrase_fn_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    {
        let mut f = std::fs::File::create(tmp_dir.join("definitions.rs")).unwrap();
        writeln!(f, "pub fn handle_xray_definitions(ctx: &Context) -> Result {{").unwrap();
        writeln!(f, "    // implementation").unwrap();
        writeln!(f, "}}").unwrap();
    }
    {
        let mut f = std::fs::File::create(tmp_dir.join("callers.rs")).unwrap();
        writeln!(f, "pub fn build_caller_tree(method: &str) -> Tree {{").unwrap();
        writeln!(f, "    // implementation").unwrap();
        writeln!(f, "}}").unwrap();
    }
    {
        let mut f = std::fs::File::create(tmp_dir.join("utils.rs")).unwrap();
        writeln!(f, "pub fn helper() {{ }}").unwrap();
    }

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp_dir.to_string_lossy().to_string(),
        ext: "rs".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp_dir.to_string_lossy().to_string()))),
        server_ext: "rs".to_string(),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };

    // This was the bug scenario: comma-separated function signatures
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "fn handle_xray_definitions,fn build_caller_tree"
    }));
    assert!(!result.is_error, "Multi-phrase fn search should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert_eq!(total, 2, "Should find exactly 2 files (definitions.rs and callers.rs), got {}", total);

    // utils.rs should NOT be in results
    if let Some(files) = output["files"].as_array() {
        for file in files {
            let path = file["path"].as_str().unwrap();
            assert!(!path.contains("utils.rs"),
                "utils.rs should not be in results (has no matching phrases)");
        }
    }

    // termsSearched should show individual phrases
    let terms = output["summary"]["termsSearched"].as_array().unwrap();
    assert_eq!(terms.len(), 2, "Should have 2 searched terms, got: {:?}", terms);

    cleanup_tmp(&tmp_dir);
}
/// Report gap 45.1: Unicode search terms should return 0 results, no panic.
/// LLM agents may pass non-ASCII terms when working with multilingual codebases.
#[test]
fn test_grep_unicode_search_terms_no_crash() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": "数据库连接", "countOnly": true}));
    assert!(!result.is_error, "Unicode search terms should not crash: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 0,
        "Unicode terms in ASCII codebase should return 0 files");
}

/// Report gap 49.3: Single-character grep (broadest possible query) should not OOM.
#[test]
fn test_grep_single_char_exact_no_oom() {
    let ctx = make_substring_ctx(
        vec![("httpclient", 0, vec![5]), ("abc", 1, vec![10])],
        vec!["C:\\test\\Program.cs", "C:\\test\\Other.cs"],
    );
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "a",
        "substring": false,
        "countOnly": true
    }));
    assert!(!result.is_error, "Single-char grep should not crash: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    // "a" as exact token is unlikely to exist in tokenized codebase (min_token_len=2)
    // The key assertion is no panic/OOM, not the result count
    assert!(output["summary"]["totalFiles"].as_u64().is_some(),
        "Should return a valid totalFiles count");
}

/// BUG-8: xray_grep with dir= pointing to a file should return error with hint.
#[test]
fn test_grep_dir_as_file_auto_converts_to_parent_plus_file_filter() {
    // Previously this returned an error. Now the tool auto-converts
    // dir=<file> into dir=<parent> + file=<basename> and surfaces a
    // `dirAutoConverted` note in the response summary so the LLM can learn
    // the correct pattern for next time.
    let (ctx, tmp) = make_e2e_substring_ctx();
    let file_path = tmp.join("Service.cs");
    assert!(file_path.exists(), "Test setup: Service.cs should exist");

    let result = handle_xray_grep(&ctx, &json!({
        "terms": "httpclient",
        "dir": file_path.to_string_lossy().to_string()
    }));
    assert!(!result.is_error,
        "dir= pointing to a file should now auto-convert, not error. Got: {}",
        result.content[0].text);
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let note = output["summary"]["dirAutoConverted"].as_str()
        .expect("summary.dirAutoConverted note should be present after auto-conversion");
    assert!(note.contains("auto-converted"),
        "note should mention auto-conversion, got: {}", note);
    assert!(note.contains("Service.cs"),
        "note should mention the filename, got: {}", note);
    assert!(note.contains("file='"),
        "note should show the correct next-time pattern with file=, got: {}", note);
    cleanup_tmp(&tmp);
}

#[test]
fn test_grep_explicit_file_filter_narrows_results() {
    // Explicit file= parameter restricts results to paths matching the substring.
    let (ctx, tmp) = make_e2e_substring_ctx();

    // Baseline: httpclient should appear in multiple files in the fixture.
    let baseline = handle_xray_grep(&ctx, &json!({ "terms": "httpclient" }));
    assert!(!baseline.is_error);
    let baseline_out: serde_json::Value = serde_json::from_str(&baseline.content[0].text).unwrap();
    let baseline_total = baseline_out["summary"]["totalFiles"].as_u64().unwrap_or(0);

    // Narrowed: only files whose path contains "Service".
    let narrowed = handle_xray_grep(&ctx, &json!({
        "terms": "httpclient",
        "file": "Service"
    }));
    assert!(!narrowed.is_error);
    let narrowed_out: serde_json::Value = serde_json::from_str(&narrowed.content[0].text).unwrap();
    let narrowed_total = narrowed_out["summary"]["totalFiles"].as_u64().unwrap_or(0);
    assert!(narrowed_total <= baseline_total,
        "file= filter should never INCREASE result count (baseline={}, narrowed={})",
        baseline_total, narrowed_total);
    // Every returned file must contain the filter substring in its path.
    if let Some(files) = narrowed_out["files"].as_array() {
        for f in files {
            let path = f["path"].as_str().unwrap_or("");
            assert!(path.to_lowercase().contains("service"),
                "file= filter violated: path '{}' does not contain 'service'", path);
        }
    }
    cleanup_tmp(&tmp);
}

#[test]
fn test_grep_file_filter_substring_basename_matches_includes_siblings() {
    // file='Service.cs' is a SUBSTRING filter (not exact). The basename
    // substring "service.cs" appears in `Service.cs`, `MyService.cs`,
    // `OldService.cs`, etc., so all of them are accepted — this is
    // documented contract, not a bug. Use the dir=<file> auto-convert
    // form (covered separately) when you want exact-file scoping.
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "httpclient",
        "file": "Service.cs"
    }));
    assert!(!result.is_error);
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    if let Some(files) = output["files"].as_array() {
        for f in files {
            let path = f["path"].as_str().unwrap_or("").to_lowercase();
            // Substring oracle: full path or basename must contain "service.cs".
            // Stronger than the previous `ends_with("service.cs")` which only
            // checked the basename suffix and accidentally read like an
            // exact-basename oracle (it isn't — substring is the contract).
            assert!(
                path.contains("service.cs"),
                "file= substring filter violated: path '{}' does not contain 'service.cs'",
                path
            );
        }
    }
    cleanup_tmp(&tmp);
}

#[test]
fn test_grep_file_filter_comma_separated_or_semantics() {
    // Comma-separated values in file= should OR-match across multiple names.
    let (ctx, tmp) = make_e2e_substring_ctx();
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "httpclient",
        "file": "Service,Client"
    }));
    assert!(!result.is_error);
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    if let Some(files) = output["files"].as_array() {
        for f in files {
            let path = f["path"].as_str().unwrap_or("").to_lowercase();
            assert!(path.contains("service") || path.contains("client"),
                "OR semantics violated: path '{}' matches neither 'service' nor 'client'", path);
        }
    }
    cleanup_tmp(&tmp);
}


// ─── Grep with relative dir ─────────────────────────────────────────────

#[test]
fn test_grep_dir_as_file_auto_convert_uses_exact_basename_not_substring_siblings() {
    // Regression: dir=<file> auto-convert previously populated `file=<basename>`
    // and let it propagate through `passes_file_filters` with substring semantics,
    // so a query intended as "scope to this one file" would also match siblings
    // whose basename CONTAINS the target as a substring (`MyService.cs`,
    // `Service.cs.bak`, `OldService.cs.disabled`, etc.). Fix: when the auto-convert
    // path populates `file_filter`, set `file_filter_exact = true`, switching
    // `passes_file_filters` to exact-basename matching for that filter.
    use std::io::Write;
    static C: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = C.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp = std::env::temp_dir().join(format!("grep_exact_basename_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // Marker token `xrayExactBasenameMarker` appears in ALL three files so the only
    // thing that can vary across the test variants is the file-scoping filter.
    // All three are .cs (indexed extension) so the index sees them; their basenames
    // share `Service.cs`/`Service` as a substring, the exact case the bug widened.
    for name in ["Service.cs", "MyService.cs", "OldService.cs"] {
        let mut f = std::fs::File::create(tmp.join(name)).unwrap();
        writeln!(f, "// xrayExactBasenameMarker token {}", name).unwrap();
        writeln!(f, "public class C {{ }}").unwrap();
    }

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp.to_string_lossy().to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp.to_string_lossy().to_string()))),
        index_base: tmp.join(".index"),
        ..Default::default()
    };

    // Baseline: token search without any file scoping must find all three files
    // (sanity check that the fixture indexed correctly).
    let baseline = handle_xray_grep(&ctx, &json!({ "terms": "xrayExactBasenameMarker" }));
    assert!(!baseline.is_error, "baseline must succeed: {}", baseline.content[0].text);
    let baseline_out: Value = serde_json::from_str(&baseline.content[0].text).unwrap();
    let baseline_paths: Vec<String> = baseline_out["files"].as_array().unwrap().iter()
        .map(|f| f["path"].as_str().unwrap().to_string()).collect();
    assert!(baseline_paths.iter().any(|p| p.ends_with("/Service.cs") || p.ends_with("\\Service.cs")),
        "baseline must include Service.cs: {:?}", baseline_paths);
    assert!(baseline_paths.iter().any(|p| p.ends_with("MyService.cs")),
        "baseline must include MyService.cs: {:?}", baseline_paths);
    assert!(baseline_paths.iter().any(|p| p.ends_with("OldService.cs")),
        "baseline must include OldService.cs: {:?}", baseline_paths);

    // Auto-convert path: dir= points at the literal file Service.cs.
    let scoped = handle_xray_grep(&ctx, &json!({
        "terms": "xrayExactBasenameMarker",
        "dir": tmp.join("Service.cs").to_string_lossy().to_string()
    }));
    assert!(!scoped.is_error, "auto-convert query must succeed: {}", scoped.content[0].text);
    let scoped_out: Value = serde_json::from_str(&scoped.content[0].text).unwrap();

    // Note must still be present for LLM ergonomics (the fix only changed scoping,
    // not the surfaced hint).
    assert!(scoped_out["summary"]["dirAutoConverted"].as_str().is_some(),
        "dirAutoConverted note must still be present after the fix: {}",
        serde_json::to_string_pretty(&scoped_out).unwrap());

    let scoped_paths: Vec<String> = scoped_out["files"].as_array().unwrap().iter()
        .map(|f| f["path"].as_str().unwrap().to_string()).collect();

    // Exactly one file: the literal Service.cs the user pointed at.
    assert_eq!(scoped_paths.len(), 1,
        "BUG REGRESSION: dir=<file> must scope to exactly that one file (got: {:?})",
        scoped_paths);
    let only = &scoped_paths[0];
    assert!(
        (only.ends_with("/Service.cs") || only.ends_with("\\Service.cs"))
            && !only.ends_with("MyService.cs")
            && !only.ends_with("OldService.cs"),
        "BUG REGRESSION: scoped result must be the literal Service.cs basename, got: {}",
        only);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_grep_dir_as_file_auto_convert_does_not_match_nested_same_basename() {
    // Reviewer-flagged gap: an earlier fix used basename-only equality, but
    // `dir_filter` is a recursive prefix match, so a nested duplicate
    // (`<parent>/sub/Service.cs`) would still match when the user pointed
    // `dir=<parent>/Service.cs`. The full-path equality check in
    // `passes_file_filters` (set via `exact_file_path`) closes this gap.
    use std::io::Write;
    static C: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = C.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp = std::env::temp_dir().join(format!("grep_exact_nested_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let nested = tmp.join("sub");
    std::fs::create_dir_all(&nested).unwrap();

    // Both files share the basename `Service.cs` and contain the marker.
    // With basename-only equality the nested copy would leak into the result.
    for path in [tmp.join("Service.cs"), nested.join("Service.cs")] {
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "// xrayNestedBasenameMarker token at {}", path.display()).unwrap();
        writeln!(f, "public class C {{ }}").unwrap();
    }

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp.to_string_lossy().to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp.to_string_lossy().to_string()))),
        index_base: tmp.join(".index"),
        ..Default::default()
    };

    // Baseline: token search without scoping must find BOTH copies (proves
    // the fixture indexed the nested duplicate; otherwise the negative
    // assertion below would be vacuously true).
    let baseline = handle_xray_grep(&ctx, &json!({ "terms": "xrayNestedBasenameMarker" }));
    assert!(!baseline.is_error, "baseline must succeed: {}", baseline.content[0].text);
    let baseline_out: Value = serde_json::from_str(&baseline.content[0].text).unwrap();
    let baseline_paths: Vec<String> = baseline_out["files"].as_array().unwrap().iter()
        .map(|f| f["path"].as_str().unwrap().to_string()).collect();
    assert_eq!(baseline_paths.len(), 2,
        "fixture sanity: baseline must index BOTH Service.cs copies, got: {:?}",
        baseline_paths);

    // Auto-convert path: dir= points at the top-level Service.cs ONLY.
    let scoped = handle_xray_grep(&ctx, &json!({
        "terms": "xrayNestedBasenameMarker",
        "dir": tmp.join("Service.cs").to_string_lossy().to_string()
    }));
    assert!(!scoped.is_error, "auto-convert query must succeed: {}", scoped.content[0].text);
    let scoped_out: Value = serde_json::from_str(&scoped.content[0].text).unwrap();
    let scoped_paths: Vec<String> = scoped_out["files"].as_array().unwrap().iter()
        .map(|f| f["path"].as_str().unwrap().to_string()).collect();

    assert_eq!(scoped_paths.len(), 1,
        "BUG REGRESSION (nested-basename leak): dir=<file> must scope to exactly that one file, \
         not also accept `<parent>/sub/<same-name>`. Got: {:?}",
        scoped_paths);
    let only = scoped_paths[0].replace('\\', "/");
    assert!(
        !only.contains("/sub/"),
        "BUG REGRESSION (nested-basename leak): the nested `sub/Service.cs` must be filtered out, got: {}",
        only
    );

    let _ = std::fs::remove_dir_all(&tmp);
}


#[test]
fn test_grep_dir_as_file_auto_convert_preserves_logical_symlink_path() {
    static C: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = C.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_raw = std::env::temp_dir().join(format!("grep_symlink_exact_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp_raw);
    std::fs::create_dir_all(&tmp_raw).unwrap();
    // Canonicalize the tmp root so all derived paths match the form the
    // indexer records. On Windows CI `std::env::temp_dir()` returns the 8.3
    // short form (`RUNNER~1`) while `WalkBuilder` records the long form
    // (`runneradmin`); without this normalisation the fixture's
    // `logical_norm` (built from the short form) would not equal the indexed
    // path and the sanity assertion would fire spuriously on the runner.
    let tmp = match std::fs::canonicalize(&tmp_raw) {
        Ok(p) => std::path::PathBuf::from(crate::clean_path(&p.to_string_lossy())),
        Err(_) => tmp_raw.clone(),
    };
    let root = tmp.join("root");
    let external = tmp.join("external");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&external).unwrap();
    std::fs::write(external.join("note.md"), "xraySymlinkExactMarker\n").unwrap();

    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&external, root.join("personal")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, root.join("personal")).unwrap();

    let logical_file = root.join("personal").join("note.md");
    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: root.to_string_lossy().to_string(),
        ext: "md".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(root.to_string_lossy().to_string()))),
        index_base: root.join(".index"),
        ..Default::default()
    };

    let baseline = handle_xray_grep(&ctx, &json!({ "terms": "xraySymlinkExactMarker" }));
    assert!(!baseline.is_error, "baseline must succeed: {}", baseline.content[0].text);
    let baseline_out: Value = serde_json::from_str(&baseline.content[0].text).unwrap();
    let baseline_paths: Vec<String> = baseline_out["files"].as_array().unwrap().iter()
        .map(|f| f["path"].as_str().unwrap().replace('\\', "/"))
        .collect();
    let logical_norm = logical_file.to_string_lossy().replace('\\', "/");
    assert_eq!(baseline_paths, vec![logical_norm.clone()],
        "fixture sanity: index must record the logical symlink path, got: {:?}",
        baseline_paths);

    let scoped = handle_xray_grep(&ctx, &json!({
        "terms": "xraySymlinkExactMarker",
        "dir": logical_file.to_string_lossy().to_string()
    }));
    assert!(!scoped.is_error, "auto-convert query must succeed: {}", scoped.content[0].text);
    let scoped_out: Value = serde_json::from_str(&scoped.content[0].text).unwrap();
    let scoped_paths: Vec<String> = scoped_out["files"].as_array().unwrap().iter()
        .map(|f| f["path"].as_str().unwrap().replace('\\', "/"))
        .collect();

    assert_eq!(scoped_paths, vec![logical_norm.clone()],
        "exact-file auto-convert must preserve the logical symlink path, not canonicalize to the external target. Got: {:?}",
        scoped_paths);
    assert!(scoped_out["summary"]["dirAutoConverted"].as_str().is_some(),
        "dirAutoConverted note must still be present after logical-path-preserving fix");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_grep_explicit_file_filter_keeps_substring_semantics() {
    // Regression guard for the OTHER direction: user-provided file= must KEEP
    // substring/comma-OR semantics (the fix only flipped behavior for the
    // auto-convert path). file='Service' should match Service.cs, MyService.cs,
    // OldService.cs — same as before.
    use std::io::Write;
    static C: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = C.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp = std::env::temp_dir().join(format!("grep_substring_keep_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    for name in ["Service.cs", "MyService.cs", "OldService.cs"] {
        let mut f = std::fs::File::create(tmp.join(name)).unwrap();
        writeln!(f, "// xraySubstringKeepMarker token {}", name).unwrap();
    }
    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp.to_string_lossy().to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp.to_string_lossy().to_string()))),
        index_base: tmp.join(".index"),
        ..Default::default()
    };

    let result = handle_xray_grep(&ctx, &json!({
        "terms": "xraySubstringKeepMarker",
        "file": "Service"
    }));
    assert!(!result.is_error, "explicit file= must succeed: {}", result.content[0].text);
    let out: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let paths: Vec<String> = out["files"].as_array().unwrap().iter()
        .map(|f| f["path"].as_str().unwrap().to_string()).collect();

    assert_eq!(paths.len(), 3,
        "explicit file='Service' (substring) must still match all 3 sibling files (got: {:?})",
        paths);

    // dirAutoConverted must NOT be present — user did not pass a file path in dir=.
    assert!(out["summary"]["dirAutoConverted"].as_str().is_none(),
        "dirAutoConverted must not be set when user passes plain file=: {}",
        serde_json::to_string_pretty(&out).unwrap());

    let _ = std::fs::remove_dir_all(&tmp);
}


#[test]
fn test_grep_with_relative_subdir_filter() {
    let tmp_holder = tempfile::tempdir().unwrap();
    let tmp_buf = crate::canonicalize_test_root(tmp_holder.path());
    let tmp = tmp_buf.as_path();
    let sub_a = tmp.join("subA"); let sub_b = tmp.join("subB");
    std::fs::create_dir_all(&sub_a).unwrap(); std::fs::create_dir_all(&sub_b).unwrap();
    std::fs::write(sub_a.join("hello.txt"), "ProductCatalog usage here").unwrap();
    std::fs::write(sub_b.join("other.txt"), "ProductCatalog other usage").unwrap();
    let index = crate::build_content_index(&crate::ContentIndexArgs { dir: tmp.to_string_lossy().to_string(), ext: "txt".to_string(), threads: 1, ..Default::default() }).unwrap();
    let ctx = HandlerContext { index: Arc::new(RwLock::new(index)), workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp.to_string_lossy().to_string()))), server_ext: "txt".to_string(), index_base: tmp.to_path_buf(), ..Default::default() };

    // Use RELATIVE dir path "subA" instead of absolute
    let r_sub = handle_xray_grep(&ctx, &json!({"terms": "productcatalog", "dir": "subA"}));
    assert!(!r_sub.is_error, "Relative dir should work for grep: {}", r_sub.content[0].text);
    let o_sub: Value = serde_json::from_str(&r_sub.content[0].text).unwrap();
    assert_eq!(o_sub["summary"]["totalFiles"], 1, "Should find 1 file in subA with relative dir");
}

// ─── Block A: countOnly should NOT include matchedTokens ─────────

/// Block A regression: substring countOnly=true must NOT include matchedTokens in response.
/// Before fix, matchedTokens was always emitted, causing false truncation warnings.
#[test]
fn test_substring_count_only_no_matched_tokens() {
    let ctx = make_substring_ctx(
        vec![
            ("userservice", 0, vec![5]),
            ("orderservice", 1, vec![10]),
        ],
        vec![
            "C:\\project\\UserService.cs",
            "C:\\project\\OrderService.cs",
        ],
    );
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "service",
        "substring": true,
        "countOnly": true
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    // countOnly should still return counts
    assert_eq!(output["summary"]["totalFiles"], 2);
    assert!(output["summary"]["totalOccurrences"].as_u64().unwrap() >= 2);

    // But matchedTokens should be absent
    assert!(output["summary"].get("matchedTokens").is_none(),
        "matchedTokens should NOT be present in countOnly response. Got: {:?}",
        output["summary"]["matchedTokens"]);

    // files array should also be absent
    assert!(output.get("files").is_none(),
        "files should not be present in countOnly mode");
}

/// Block A: non-countOnly substring should still include matchedTokens (regression guard).
#[test]
fn test_substring_non_count_only_still_has_matched_tokens() {
    let ctx = make_substring_ctx(
        vec![
            ("userservice", 0, vec![5]),
        ],
        vec![
            "C:\\project\\UserService.cs",
        ],
    );
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "service",
        "substring": true
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    // matchedTokens SHOULD be present in non-countOnly mode
    let tokens = output["summary"]["matchedTokens"].as_array()
        .expect("matchedTokens should be present in non-countOnly response");
    assert!(!tokens.is_empty(), "matchedTokens should not be empty");
}

// ─── Block B: auto-switch hint should be actionable ─────────

/// Block B: when dotted terms trigger phrase auto-switch, the hint should contain
/// actionable advice ("Tip: use last segment only") not just explain the mechanism.
#[test]
fn test_auto_switch_phrase_hint_is_actionable() {
    let (ctx, tmp) = make_e2e_substring_ctx();
    // Use a dotted namespace term that triggers auto-switch
    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": "System.Data.SqlClient"
    }));
    assert!(!result.is_error, "Dotted term grep should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    let note = output["summary"]["searchModeNote"].as_str()
        .expect("searchModeNote should exist for auto-switched phrase search");

    // Hint should contain actionable advice
    assert!(note.contains("Tip:"),
        "searchModeNote should contain 'Tip:' with actionable advice. Got: {}", note);
    assert!(note.contains("SqlClient"),
        "searchModeNote should contain example like 'SqlClient'. Got: {}", note);
    assert!(note.contains("~100x slower"),
        "searchModeNote should warn about slowdown (~100x slower). Got: {}", note);

    cleanup_tmp(&tmp);
}
