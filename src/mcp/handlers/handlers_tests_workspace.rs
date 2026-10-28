//! P2 Group D: Regression tests for the `cmd_serve` / `handle_xray_reindex_inner`
//! complexity refactoring. Validates the state-machine helpers extracted during
//! that refactor: `rollback_workspace_state`, `cross_load_definition_index`,
//! and the PinnedCli-mode workspace-switch guard inside `handle_xray_reindex_inner`.
//!
//! These tests live in a #[path]-mounted submodule of `mcp::handlers::mod` so
//! they can call the private (`fn`, not `pub`) helpers directly without exposing
//! them in the public API surface.

// Test setup constructs HandlerContext::default() then assigns a couple of fields
// per test; converting each to a struct-literal would obscure which fields differ.
#![allow(clippy::field_reassign_with_default)]

use super::{
    HandlerContext, WorkspaceBindingMode, WorkspaceStatus, WorkspaceSwitchPanicGuard,
    cross_load_content_index, cross_load_definition_index,
    handle_xray_reindex_definitions_inner, handle_xray_reindex_inner,
    rollback_workspace_state,
};
use super::handlers_test_utils::make_ctx_with_defs;
use serde_json::{Value, json};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

struct Definition503Fixture {
    _temp: tempfile::TempDir,
    ctx: HandlerContext,
    alternate: String,
}

#[derive(Debug, PartialEq)]
struct Definition503Snapshot {
    state: Value,
    content: Vec<u8>,
    definitions: Vec<u8>,
    cache: std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>>,
}

impl Definition503Fixture {
    fn new() -> Self {
        std::fs::create_dir_all("target").unwrap();
        let temp = tempfile::tempdir_in(std::fs::canonicalize("target").unwrap()).unwrap();
        let workspace = temp.path().join("workspace");
        let alternate = temp.path().join("alternate");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&alternate).unwrap();
        std::fs::write(
            workspace.join("marker.sql"),
            "CREATE PROCEDURE dbo.Definition503Marker AS SELECT 1;\n",
        ).unwrap();
        let root = crate::clean_path(
            &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
        );
        let mut ctx = make_ctx_with_defs();
        ctx.server_ext = "json".to_string();
        ctx.def_extensions = vec!["sql".to_string()];
        ctx.index_base = temp.path().join("indexes");
        ctx.watch_enabled = true;
        let mut definitions = crate::definitions::build_definition_index(
            &crate::definitions::DefIndexArgs {
                dir: root.clone(), ext: "sql".to_string(), threads: 1,
                respect_git_exclude: false,
            },
        );
        assert!(definitions.definitions.iter().any(|d| d.name == "Definition503Marker"));
        definitions.definition_generation = 41;
        crate::definitions::save_definition_index(&definitions, &ctx.index_base).unwrap();
        *ctx.def_index.as_ref().unwrap().write().unwrap() =
            crate::definitions::load_definition_index(&root, "sql", &ctx.index_base).unwrap();
        ctx.index.write().unwrap().root = root.clone();
        {
            let mut ws = ctx.workspace.write().unwrap();
            ws.set_dir(root);
            ws.mode = WorkspaceBindingMode::ManualOverride;
            ws.status = WorkspaceStatus::Resolved;
            ws.generation = 9;
        }
        ctx.index_epoch.store(17, Ordering::Release);
        ctx.watcher_generation.store(13, Ordering::Release);
        let fixture = Self {
            _temp: temp, ctx,
            alternate: crate::clean_path(
                &std::fs::canonicalize(alternate).unwrap().to_string_lossy(),
            ),
        };
        fixture.assert_marker();
        fixture
    }

    fn query(&self) -> Value {
        let result = super::dispatch_tool(&self.ctx, "xray_definitions", &json!({
            "name": ["Definition503Marker"], "exactNameOnly": true,
        }));
        assert!(!result.is_error, "{}", result.content[0].text);
        serde_json::from_str(&result.content[0].text).unwrap()
    }

    fn assert_marker(&self) {
        for _ in 0..2 {
            let output = self.query();
            assert!(output["definitions"].as_array().unwrap().iter().any(
                |d| d["name"] == "Definition503Marker"
            ), "{output}");
        }
    }

    fn snapshot(&self) -> Definition503Snapshot {
        fn inventory(
            root: &std::path::Path,
            path: &std::path::Path,
            entries: &mut std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>>,
        ) {
            let bytes = if path.is_dir() { None } else { Some(std::fs::read(path).unwrap()) };
            entries.insert(path.strip_prefix(root).unwrap().to_path_buf(), bytes);
            if path.is_dir() {
                for entry in std::fs::read_dir(path).unwrap() {
                    inventory(root, &entry.unwrap().path(), entries);
                }
            }
        }
        let ws = self.ctx.workspace.read().unwrap();
        let state = json!({
            "dir": ws.dir, "canonicalDir": ws.canonical_dir,
            "mode": format!("{:?}", ws.mode), "status": format!("{:?}", ws.status),
            "generation": ws.generation,
            "contentReady": self.ctx.content_ready.load(Ordering::Acquire),
            "defReady": self.ctx.def_ready.load(Ordering::Acquire),
            "epoch": self.ctx.index_epoch.load(Ordering::Acquire),
            "watcherGeneration": self.ctx.watcher_generation.load(Ordering::Acquire),
            "contentBuilding": self.ctx.content_building.load(Ordering::Acquire),
            "defBuilding": self.ctx.def_building.load(Ordering::Acquire),
            "fileIndexDirty": self.ctx.file_index_dirty.load(Ordering::Acquire),
            "autosaveDirty": self.ctx.autosave_dirty.load(Ordering::Acquire),
        });
        drop(ws);
        let mut cache = std::collections::BTreeMap::new();
        inventory(&self.ctx.index_base, &self.ctx.index_base, &mut cache);
        Definition503Snapshot {
            state,
            content: bincode::serialize(&*self.ctx.index.read().unwrap()).unwrap(),
            definitions: bincode::serialize(
                &*self.ctx.def_index.as_ref().unwrap().read().unwrap(),
            ).unwrap(),
            cache,
        }
    }

    fn refuse(&self, args: Value, expected: &str) {
        let before = self.snapshot();
        let result = super::dispatch_tool(&self.ctx, "xray_reindex_definitions", &args);
        let after_query = self.query();
        assert!(result.is_error,
            "args={args}; reindex={}; subsequent query={after_query}", result.content[0].text);
        assert!(result.content[0].text.contains(expected), "{}", result.content[0].text);
        assert_eq!(self.snapshot(), before, "args={args}");
        self.assert_marker();
    }
}

#[test]
fn test_503_invalid_definition_extensions_preserve_live_and_persisted_state() {
    let fixture = Definition503Fixture::new();
    let primary = crate::index::build_counts::Probe::new(&fixture.ctx.server_dir());
    let alternate = crate::index::build_counts::Probe::new(&fixture.alternate);
    let cases = [
        (json!([","]), "empty"),
        (json!([",,,"]), "empty"),
        (json!(["sql,"]), "empty"),
        (json!([",sql"]), "empty"),
        (json!(["sql,,json"]), "empty"),
        (json!(["sql, \t ,json"]), "empty"),
        (json!(["sql", ","]), "empty"),
        (json!(["json", "toml"]), "parser"),
        (json!(["xml"]), "parser"),
        (json!(["csproj,config,resx"]), "parser"),
        (json!("sql"), "must be an array of strings"),
        (json!(true), "must be an array of strings"),
        (json!(7), "must be an array of strings"),
        (json!({}), "must be an array of strings"),
        (json!(["sql", null]), "element [1]"),
        (json!(["sql", false]), "element [1]"),
        (json!(["sql", 7]), "element [1]"),
        (json!(["sql", {}]), "element [1]"),
        (json!(["sql", []]), "element [1]"),
    ];
    for (ext, expected) in cases {
        for switch in [false, true] {
            let mut args = json!({"ext": ext});
            if switch { args["dir"] = json!(fixture.alternate); }
            fixture.refuse(args, expected);
        }
    }
    for (feature_enabled, ext) in [
        (cfg!(feature = "lang-csharp"), "cs"),
        (cfg!(feature = "lang-typescript"), "ts"),
        (cfg!(feature = "lang-typescript"), "tsx"),
        (cfg!(feature = "lang-rust"), "rs"),
    ] {
        if !feature_enabled {
            fixture.refuse(json!({"dir": fixture.alternate, "ext": [ext]}), "parser");
        }
    }
    assert_eq!(primary.counts(), [0, 0]);
    assert_eq!(alternate.counts(), [0, 0]);
}

#[test]
fn test_503_definition_extension_fallbacks_are_validated() {
    let mut fixture = Definition503Fixture::new();
    fixture.ctx.watch_enabled = false;
    let primary = crate::index::build_counts::Probe::new(&fixture.ctx.server_dir());
    let alternate = crate::index::build_counts::Probe::new(&fixture.alternate);
    let mut builds = 0;
    let fallback_forms = [
        json!({}), json!({"ext": null}), json!({"ext": []}),
        json!({"ext": ["", " \t "]}),
    ];
    for defaults in [vec![" SQL ".to_string()], Vec::new()] {
        fixture.ctx.def_extensions = defaults.clone();
        for args in &fallback_forms {
            let result = super::dispatch_tool(&fixture.ctx, "xray_reindex_definitions", args);
            assert!(!result.is_error, "{}", result.content[0].text);
            builds += 1;
            let expected = if defaults.is_empty() {
                crate::definitions::definition_extensions()[0]
            } else { "sql" };
            assert_eq!(
                fixture.ctx.def_index.as_ref().unwrap().read().unwrap().extensions,
                vec![expected.to_string()],
            );
            assert_eq!(primary.counts(), [0, builds]);
            assert_eq!(alternate.counts(), [0, 0]);
        }
    }
    // Restore the known SQL graph before testing invalid configured defaults.
    let result = super::dispatch_tool(&fixture.ctx, "xray_reindex_definitions", &json!({"ext": ["sql"]}));
    assert!(!result.is_error, "{}", result.content[0].text);
    builds += 1;
    fixture.assert_marker();
    fixture.ctx.watch_enabled = true;
    for defaults in [vec![",".to_string()], vec!["sql,".to_string()],
        vec!["json".to_string()], vec!["xml".to_string()], vec![" ".to_string()]]
    {
        fixture.ctx.def_extensions = defaults;
        for args in &fallback_forms {
            let mut args = args.clone();
            args["dir"] = json!(fixture.alternate);
            fixture.refuse(args, "'ext'");
        }
    }
    assert_eq!(primary.counts(), [0, builds]);
    assert_eq!(alternate.counts(), [0, 0]);
}

#[test]
fn test_503_definition_extensions_normalize_without_filtering_and_reload() {
    let mut fixture = Definition503Fixture::new();
    fixture.ctx.watch_enabled = false;
    let probe = crate::index::build_counts::Probe::new(&fixture.ctx.server_dir());
    let expected = vec!["json", "sql", "toml", "sql", "xml"];
    for builds in 1..=2 {
        let result = super::dispatch_tool(&fixture.ctx, "xray_reindex_definitions", &json!({
            "ext": ["", " JSON , SQL ", " \t ", "TOML", " sql , XML "],
        }));
        assert!(!result.is_error, "{}", result.content[0].text);
        let live = fixture.ctx.def_index.as_ref().unwrap().read().unwrap();
        assert_eq!(live.extensions, expected);
        assert_eq!(live.definition_generation, 41 + builds as u64);
        let loaded = crate::definitions::load_definition_index(
            &fixture.ctx.server_dir(), &expected.join(","), &fixture.ctx.index_base,
        ).unwrap();
        assert_eq!(loaded.extensions, expected);
        assert!(loaded.definitions.iter().any(|d| d.name == "Definition503Marker"));
        drop(live);
        fixture.assert_marker();
        assert_eq!(probe.counts(), [0, builds]);
    }
}

#[test]
fn test_503_valid_definition_scope_can_have_zero_files_or_definitions() {
    for empty_file in [false, true] {
        let mut fixture = Definition503Fixture::new();
        fixture.ctx.watch_enabled = false;
        if empty_file {
            std::fs::write(
                std::path::Path::new(&fixture.alternate).join("empty.sql"),
                "-- no definitions here\n",
            ).unwrap();
        }
        let content = crate::index::build_content_index(&crate::ContentIndexArgs {
            dir: fixture.alternate.clone(), ext: fixture.ctx.server_ext.clone(),
            threads: 1, ..Default::default()
        }).unwrap();
        crate::index::save_content_index(&content, &fixture.ctx.index_base).unwrap();
        let primary = crate::index::build_counts::Probe::new(&fixture.ctx.server_dir());
        let alternate = crate::index::build_counts::Probe::new(&fixture.alternate);
        let result = super::dispatch_tool(&fixture.ctx, "xray_reindex_definitions", &json!({
            "dir": fixture.alternate, "ext": [" SQL , sql "],
        }));
        assert!(wait_until(|| fixture.ctx.git_cache_ready.load(Ordering::Acquire)));
        assert!(!result.is_error, "{}", result.content[0].text);
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert_eq!(output["status"], "ok");
        assert_eq!(output["files"], usize::from(empty_file));
        assert_eq!(output["definitions"], 0);
        assert_eq!(output["contentIndexAction"], "loaded_cache");
        assert!(fixture.ctx.def_ready.load(Ordering::Acquire));
        assert_eq!(fixture.ctx.workspace.read().unwrap().status, WorkspaceStatus::Resolved);
        let loaded = crate::definitions::load_definition_index(
            &fixture.alternate, "sql,sql", &fixture.ctx.index_base,
        ).unwrap();
        assert_eq!(loaded.extensions, ["sql", "sql"]);
        assert!(loaded.definitions.is_empty());
        assert_eq!(loaded.files.len(), usize::from(empty_file));
        for _ in 0..2 {
            assert!(fixture.query()["definitions"].as_array().unwrap().is_empty());
        }
        assert_eq!(primary.counts(), [0, 0]);
        assert_eq!(alternate.counts(), [0, 1]);
    }
}

#[test]
fn test_503_definition_extension_schema_preserves_optional_array_fallback() {
    let tools = super::tool_definitions_with_runtime(&["sql".to_string()], false);
    let schema = &tools.iter().find(|tool| tool.name == "xray_reindex_definitions")
        .unwrap().input_schema;
    let ext = &schema["properties"]["ext"];
    assert_eq!(ext["type"], "array");
    assert_eq!(ext["items"]["type"], "string");
    assert!(ext.get("minItems").is_none());
    assert!(ext.get("enum").is_none());
    assert!(ext["items"].get("enum").is_none());
    assert!(!schema["required"].as_array().unwrap().iter().any(|key| key == "ext"));
}

#[test]
fn test_workspace_completion_companion_publication_ownership() {
    use std::sync::atomic::Ordering;
    for round_trip in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let a = crate::clean_path(&std::fs::canonicalize(a).unwrap().to_string_lossy());
        let b = crate::clean_path(&std::fs::canonicalize(b).unwrap().to_string_lossy());
        let ctx = HandlerContext::default();
        {
            let mut ws = ctx.workspace.write().unwrap();
            ws.set_dir(a.clone());
            ws.generation = 0;
        }
        ctx.content_ready.store(false, Ordering::Release);
        let old_index = ctx.index.write().unwrap();
        std::thread::scope(|threads| {
            let result = threads.spawn(|| super::publish_companion_content_index(
                &ctx, &a, 0, crate::ContentIndex { root: a.clone(), ..Default::default() }, false,
            ));
            {
                let mut ws = ctx.workspace.write().unwrap();
                ws.set_dir(b.clone());
                ws.generation += 1;
                if round_trip {
                    ws.set_dir(a.clone());
                    ws.generation += 1;
                }
            }
            drop(old_index);
            assert!(!result.join().unwrap());
        });
        assert!(!ctx.content_ready.load(Ordering::Acquire));
        let (dir, generation) = {
            let ws = ctx.workspace.read().unwrap();
            (ws.dir.clone(), ws.generation)
        };
        assert!(super::publish_companion_content_index(
            &ctx, &dir, generation,
            crate::ContentIndex { root: dir.clone(), ..Default::default() }, false,
        ));
        assert!(ctx.content_ready.load(Ordering::Acquire));
    }
}

#[test]
fn test_workspace_completion_cold_content_publication() {
    use std::sync::atomic::Ordering;
    for superseded in [false, true] {
        let mut fixture = ContentBuildFixture::new();
        let dir = fixture.dirs[0].clone();
        fixture.pause(&dir);
        fixture.start(false);
        fixture.reached(0);
        if superseded {
            fixture.ctx.workspace.write().unwrap().generation += 2;
            std::fs::write(std::path::Path::new(&dir).join("probe.rs"), "fn fresh_marker() {}\n").unwrap();
            fixture.pause(&dir);
            fixture.release(0);
            fixture.reached(1);
            assert!(!fixture.ctx.content_ready.load(Ordering::Acquire));
            assert!(!fixture.ctx.index.read().unwrap().index.contains_key("outgoing_marker"));
            fixture.release(1);
        } else {
            fixture.release(0);
        }
        fixture.finish();
        assert!(fixture.ctx.content_ready.load(Ordering::Acquire));
        assert!(fixture.ctx.index.read().unwrap().index.contains_key(if superseded { "fresh_marker" } else { "outgoing_marker" }));
    }
}

struct ContentBuildFixture {
    _tmp: tempfile::TempDir,
    ctx: HandlerContext,
    dirs: Vec<String>,
    probe: Arc<super::ContentBuildProbe>,
    barriers: Vec<(String, std::sync::mpsc::Receiver<()>, Option<std::sync::mpsc::Sender<()>>)>,
    completion: Option<std::sync::mpsc::Receiver<()>>,
    terminal: Arc<AtomicBool>,
}

impl ContentBuildFixture {
    fn new() -> Self {
        std::fs::create_dir_all("target").unwrap();
        let tmp = tempfile::tempdir_in(std::fs::canonicalize("target").unwrap()).unwrap();
        let mut dirs = Vec::new();
        for (name, token) in [("b", "outgoing_marker"), ("c", "current_marker"), ("d", "latest_marker")] {
            let root = tmp.path().join(name);
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("probe.rs"), format!("fn {token}() {{}}\n")).unwrap();
            dirs.push(crate::clean_path(&std::fs::canonicalize(root).unwrap().to_string_lossy()));
        }
        let ctx = HandlerContext {
            server_ext: "rs".into(), index_base: tmp.path().join("indexes"),
            ..Default::default()
        };
        ctx.workspace.write().unwrap().set_dir(dirs[0].clone());
        ctx.content_ready.store(false, Ordering::Release);
        ctx.file_index_dirty.store(false, Ordering::Release);
        let probe = Arc::new(super::ContentBuildProbe::default());
        super::CONTENT_BUILD_PROBES.lock().unwrap().insert(ctx.index_base.clone(), Arc::clone(&probe));
        Self { _tmp: tmp, ctx, dirs, probe, barriers: Vec::new(), completion: None,
            terminal: Arc::new(AtomicBool::new(false)) }
    }

    fn pause(&mut self, dir: &str) {
        let (reached, paused) = std::sync::mpsc::channel();
        let (resume, release) = std::sync::mpsc::channel();
        super::CONTENT_CROSS_BUILD_BARRIERS.lock().unwrap().insert(
            dir.to_string(), super::ContentCrossBuildBarrier { reached, resume: release },
        );
        self.barriers.push((dir.to_string(), paused, Some(resume)));
    }

    fn pause_preparation(&mut self, dir: &str) {
        let (reached, paused) = std::sync::mpsc::channel();
        let (resume, release) = std::sync::mpsc::channel();
        super::CONTENT_PREPARATION_BARRIERS.lock().unwrap().insert(
            dir.to_string(), super::ContentCrossBuildBarrier { reached, resume: release },
        );
        self.barriers.push((dir.to_string(), paused, Some(resume)));
    }

    fn reached(&self, stage: usize) {
        self.barriers[stage].1.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    }

    fn release(&mut self, stage: usize) {
        if let Some(resume) = self.barriers[stage].2.take() { let _ = resume.send(()); }
    }

    fn start(&mut self, startup: bool) {
        let (done, completion) = std::sync::mpsc::channel();
        super::CONTENT_CROSS_BUILD_COMPLETED.lock().unwrap().insert(self.dirs[0].clone(), done);
        self.completion = Some(completion);
        if startup {
            super::start_content_index_build(&self.ctx, 1, Arc::clone(&self.terminal));
        } else {
            assert_eq!(super::cross_load_content_index(&self.ctx, &self.dirs[0]), Some("background_build"));
        }
    }

    fn switch(&self, dir: &str) -> Option<&'static str> {
        {
            let mut ws = self.ctx.workspace.write().unwrap();
            ws.set_dir(dir.to_string());
            ws.generation += 1;
        }
        super::cross_load_content_index(&self.ctx, dir)
    }

    fn pause_definitions(&mut self, dir: &str, panic_after_resume: bool) {
        let (reached, paused) = std::sync::mpsc::channel();
        let (resume, release) = std::sync::mpsc::channel();
        let (completed, completion) = std::sync::mpsc::channel();
        super::DEFINITION_CROSS_BUILD_BARRIERS.lock().unwrap().insert(
            dir.to_string(), super::DefinitionCrossBuildBarrier {
                reached, resume: release, completed, panic_after_resume,
            },
        );
        self.barriers.push((dir.to_string(), paused, Some(resume)));
        self.completion = Some(completion);
    }

    fn finish_definitions(&mut self) {
        self.completion.as_ref().unwrap().recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        self.completion = None;
        assert!(!self.ctx.def_building.load(Ordering::Acquire));
    }

    fn cache(&self, dir: &str) {
        let index = crate::build_content_index(&crate::ContentIndexArgs {
            dir: dir.to_string(), ext: "rs".into(), threads: 1, ..Default::default()
        }).unwrap();
        crate::save_content_index(&index, &self.ctx.index_base).unwrap();
    }

    fn finish(&mut self) {
        self.completion.as_ref().unwrap().recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        self.completion = None;
        assert!(!self.ctx.content_building.load(Ordering::Acquire));
        assert_eq!(self.probe.active.load(Ordering::Acquire), 0);
        assert_eq!(self.probe.max_active.load(Ordering::Acquire), 1);
    }

    fn calls(&self) -> Vec<String> {
        self.probe.calls.lock().unwrap().clone()
    }
}

impl Drop for ContentBuildFixture {
    fn drop(&mut self) {
        for dir in &self.dirs {
            super::CONTENT_CROSS_BUILD_BARRIERS.lock().unwrap().remove(dir);
            super::CONTENT_PREPARATION_BARRIERS.lock().unwrap().remove(dir);
            super::DEFINITION_CROSS_BUILD_BARRIERS.lock().unwrap().remove(dir);
        }
        for (_, _, resume) in &mut self.barriers {
            if let Some(resume) = resume.take() { let _ = resume.send(()); }
        }
        if let Some(completion) = self.completion.take() {
            let _ = completion.recv_timeout(std::time::Duration::from_secs(10));
        }
        for dir in &self.dirs {
            super::CONTENT_CROSS_BUILD_BARRIERS.lock().unwrap().remove(dir);
            super::CONTENT_CROSS_BUILD_COMPLETED.lock().unwrap().remove(dir);
            super::DEFINITION_CROSS_BUILD_BARRIERS.lock().unwrap().remove(dir);
        }
        super::CONTENT_BUILD_PROBES.lock().unwrap().remove(&self.ctx.index_base);
    }
}

#[cfg(feature = "lang-rust")]
#[test]
fn test_reindex_watcher_waits_for_definition_companion_after_startup() {
    let mut fixture = ContentBuildFixture::new();
    let target = fixture.dirs[0].clone();
    let outgoing = fixture.dirs[1].clone();
    fixture.ctx.def_index = make_ctx_with_defs().def_index;
    fixture.ctx.def_extensions = vec!["rs".into()];
    fixture.ctx.watch_enabled = true;
    fixture.ctx.watch_debounce_ms = 20;
    fixture.ctx.content_ready.store(true, Ordering::Release);
    {
        let mut ws = fixture.ctx.workspace.write().unwrap();
        ws.set_dir(outgoing.clone());
        ws.mode = WorkspaceBindingMode::ManualOverride;
    }
    *fixture.ctx.index.write().unwrap() = crate::mcp::watcher::build_watch_index_from(
        crate::ContentIndex {
            root: outgoing,
            extensions: vec!["rs".into()],
            ..Default::default()
        },
    );
    let _stop = WatcherScopeStop(Arc::clone(&fixture.ctx.watcher_generation));
    assert!(fixture.ctx.content_build_terminal.load(Ordering::Acquire));
    assert!(fixture.ctx.def_build_terminal.load(Ordering::Acquire));
    fixture.pause_definitions(&target, false);
    let result = super::dispatch_tool(&fixture.ctx, "xray_reindex",
        &json!({"dir": target, "ext": ["rs"]}));
    assert!(!result.is_error, "{}", result.content[0].text);
    fixture.reached(0);
    assert!(fixture.ctx.def_building.load(Ordering::Acquire));
    assert!(!fixture.ctx.def_ready.load(Ordering::Acquire));
    let pending = std::path::Path::new(&target).join("pending.rs");
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
    while std::time::Instant::now() < deadline {
        std::fs::write(&pending, "pub fn buffered_definition() {}\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let premature_events = fixture.ctx.watcher_stats.events_total.load(Ordering::Relaxed);
    fixture.release(0);
    fixture.finish_definitions();
    assert_eq!(premature_events, 0, "restart must buffer events while definitions build");
    assert!(wait_until(|| fixture.ctx.index.read().unwrap().index.contains_key("buffered_definition")));
    assert!(wait_until(|| fixture.ctx.watcher_stats.events_total.load(Ordering::Relaxed) > 0));
    assert!(fixture.ctx.def_ready.load(Ordering::Acquire));
    let defs = fixture.ctx.def_index.as_ref().unwrap().read().unwrap();
    assert!(crate::path_eq(&defs.root, &target));
    assert!(defs.definitions.iter().any(|definition| definition.name == "buffered_definition"));
}

#[cfg(feature = "lang-rust")]
#[test]
fn test_reindex_definition_companion_clears_building_on_panic() {
    let mut fixture = ContentBuildFixture::new();
    let target = fixture.dirs[1].clone();
    fixture.ctx.def_index = make_ctx_with_defs().def_index;
    fixture.ctx.def_extensions = vec!["rs".into()];
    fixture.ctx.workspace.write().unwrap().mode = WorkspaceBindingMode::ManualOverride;
    fixture.pause_definitions(&target, true);
    let result = super::dispatch_tool(&fixture.ctx, "xray_reindex",
        &json!({"dir": target, "ext": ["rs"]}));
    assert!(!result.is_error, "{}", result.content[0].text);
    fixture.reached(0);
    assert!(fixture.ctx.def_building.load(Ordering::Acquire));
    fixture.release(0);
    fixture.finish_definitions();
    assert!(!fixture.ctx.def_ready.load(Ordering::Acquire));
    let retry = super::dispatch_tool(&fixture.ctx, "xray_reindex_definitions",
        &json!({"ext": ["rs"]}));
    assert!(!retry.is_error, "{}", retry.content[0].text);
    assert!(fixture.ctx.def_ready.load(Ordering::Acquire));
    assert!(!fixture.ctx.def_building.load(Ordering::Acquire));
}

#[cfg(feature = "lang-rust")]
#[test]
fn test_reindex_watcher_waits_for_companion_build_after_startup() {
    let mut fixture = ContentBuildFixture::new();
    let target = fixture.dirs[0].clone();
    let outgoing = fixture.dirs[1].clone();
    fixture.ctx.def_index = make_ctx_with_defs().def_index;
    fixture.ctx.watch_enabled = true;
    fixture.ctx.watch_debounce_ms = 20;
    fixture.ctx.content_ready.store(true, Ordering::Release);
    {
        let mut ws = fixture.ctx.workspace.write().unwrap();
        ws.set_dir(outgoing.clone());
        ws.mode = WorkspaceBindingMode::ManualOverride;
    }
    *fixture.ctx.index.write().unwrap() = crate::mcp::watcher::build_watch_index_from(
        crate::ContentIndex {
            root: outgoing,
            extensions: vec!["rs".into()],
            ..Default::default()
        },
    );
    let _stop = WatcherScopeStop(Arc::clone(&fixture.ctx.watcher_generation));
    fixture.pause(&target);
    let (done, completion) = std::sync::mpsc::channel();
    super::CONTENT_CROSS_BUILD_COMPLETED.lock().unwrap().insert(target.clone(), done);
    fixture.completion = Some(completion);
    let result = super::dispatch_tool(&fixture.ctx, "xray_reindex_definitions",
        &json!({"dir": target, "ext": ["rs"]}));
    assert!(!result.is_error, "{}", result.content[0].text);
    fixture.reached(0);
    assert!(fixture.ctx.content_building.load(Ordering::Acquire));
    let signal = std::path::Path::new(&target).join("watcher.signal");
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
    while std::time::Instant::now() < deadline {
        std::fs::write(&signal, "pending").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let premature_events = fixture.ctx.watcher_stats.events_total.load(Ordering::Relaxed);
    fixture.release(0);
    fixture.finish();
    assert!(wait_until(|| fixture.ctx.watcher_stats.events_total.load(Ordering::Relaxed) > 0));
    assert_eq!(premature_events, 0, "restart must buffer events while companion build runs");
    assert!(fixture.ctx.content_ready.load(Ordering::Acquire));
    assert!(crate::path_eq(&fixture.ctx.index.read().unwrap().root, &target));
}

#[test]
fn test_cross_load_content_index_superseded_build_follows_current_workspace() {
    let mut fixture = ContentBuildFixture::new();
    fixture.pause(&fixture.dirs[0].clone());
    fixture.start(false);
    fixture.reached(0);
    assert_eq!(fixture.switch(&fixture.dirs[1]), Some("build_in_progress"));
    for _ in 0..3 {
        assert_eq!(super::cross_load_content_index(&fixture.ctx, &fixture.dirs[1]), Some("build_in_progress"));
    }
    assert!(!fixture.ctx.content_ready.load(Ordering::Acquire));
    fixture.release(0);
    fixture.finish();
    assert!(fixture.ctx.content_ready.load(Ordering::Acquire), "latest workspace must eventually become ready");
    let index = fixture.ctx.index.read().unwrap();
    assert!(crate::path_eq(&index.root, &fixture.dirs[1]));
    assert!(index.index.contains_key("current_marker"));
    assert!(!index.index.contains_key("outgoing_marker"));
    assert_eq!(fixture.calls(), fixture.dirs[..2]);
}

#[test]
fn test_cross_load_content_index_preparation_remains_responsive_and_rechecks_binding() {
    for (startup, publish_cache) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut fixture = ContentBuildFixture::new();
        let b = fixture.dirs[0].clone();
        let c = fixture.dirs[1].clone();
        if startup {
            fixture.ctx.watch_enabled = true;
        } else {
            fixture.ctx.index.write().unwrap().file_tokens_authoritative = true;
        }
        fixture.pause_preparation(&b);
        fixture.start(startup);
        fixture.reached(0);

        let ctx = fixture.ctx.clone();
        let (responded, response) = std::sync::mpsc::channel();
        let request = std::thread::spawn(move || {
            let info = super::dispatch_tool(&ctx, "xray_info", &json!({}));
            let progress = super::dispatch_tool(&ctx, "xray_grep", &json!({"terms": ["current_marker"]}));
            let _ = responded.send((info, progress));
        });
        let (info, progress) = response.recv_timeout(std::time::Duration::from_secs(2))
            .expect("real dispatch must respond while successful result preparation is paused");
        request.join().unwrap();
        let info: Value = serde_json::from_str(&info.content[0].text).unwrap();
        assert_eq!(info["indexes"][0]["status"], "building");
        assert!(progress.content[0].text.contains("building"));
        assert!(!fixture.ctx.content_ready.load(Ordering::Acquire));
        assert!(fixture.ctx.content_building.load(Ordering::Acquire));
        assert!(!fixture.terminal.load(Ordering::Acquire));
        assert!(!fixture.ctx.file_index_dirty.load(Ordering::Acquire));
        let epoch = fixture.ctx.index_epoch.load(Ordering::Acquire);

        if publish_cache { fixture.cache(&c); }
        assert_eq!(fixture.switch(&c), Some(if publish_cache { "loaded_cache" } else { "build_in_progress" }));
        assert_eq!(fixture.ctx.content_ready.load(Ordering::Acquire), publish_cache);
        assert!(fixture.ctx.content_building.load(Ordering::Acquire));
        assert_eq!(fixture.ctx.file_index_dirty.load(Ordering::Acquire), publish_cache);
        fixture.release(0);
        fixture.finish();

        assert!(fixture.ctx.content_ready.load(Ordering::Acquire));
        assert_eq!(fixture.terminal.load(Ordering::Acquire), startup);
        assert!(fixture.ctx.content_build_failure.read().unwrap().is_none());
        assert!(fixture.ctx.file_index_dirty.load(Ordering::Acquire));
        assert_eq!(fixture.ctx.index_epoch.load(Ordering::Acquire), epoch + 1, "only the latest binding may publish");
        let index = fixture.ctx.index.read().unwrap();
        assert!(crate::path_eq(&index.root, &c));
        assert!(index.index.contains_key("current_marker"));
        assert!(!index.index.contains_key("outgoing_marker"));
        assert!(!index.path_to_id.as_ref().unwrap().is_empty());
        assert!(index.file_tokens_authoritative);
        drop(index);
        assert_eq!(fixture.calls(), if publish_cache { vec![b] } else { vec![b, c] });
    }
}

#[test]
fn test_cross_load_content_index_multiple_supersessions_and_startup_owner() {
    for (startup, aba) in [(false, false), (false, true), (true, false)] {
        let mut fixture = ContentBuildFixture::new();
        let b = fixture.dirs[0].clone();
        let c = fixture.dirs[1].clone();
        let last = fixture.dirs[if aba { 0 } else { 2 }].clone();
        fixture.pause(&b);
        fixture.pause(&c);
        fixture.start(startup);
        fixture.reached(0);
        assert_eq!(fixture.switch(&c), Some("build_in_progress"));
        fixture.release(0);
        fixture.reached(1);
        assert!(!fixture.ctx.content_ready.load(Ordering::Acquire));
        assert!(fixture.ctx.content_building.load(Ordering::Acquire));
        std::fs::write(std::path::Path::new(&last).join("probe.rs"), "fn final_marker() {}\n").unwrap();
        assert_eq!(fixture.switch(&last), Some("build_in_progress"));
        fixture.release(1);
        fixture.finish();
        assert_eq!(fixture.calls(), vec![b, c, last.clone()]);
        assert!(fixture.ctx.content_ready.load(Ordering::Acquire));
        let index = fixture.ctx.index.read().unwrap();
        assert!(crate::path_eq(&index.root, &last));
        assert!(index.index.contains_key("final_marker"));
        if startup { assert!(fixture.terminal.load(Ordering::Acquire)); }
    }
}

#[test]
fn test_cross_load_content_index_latest_cache_wins_or_appears_before_followup() {
    for publish_cache in [true, false] {
        let mut fixture = ContentBuildFixture::new();
        let b = fixture.dirs[0].clone();
        let c = fixture.dirs[1].clone();
        fixture.pause(&b);
        fixture.start(false);
        fixture.reached(0);
        if publish_cache {
            fixture.cache(&c);
            assert_eq!(fixture.switch(&c), Some("loaded_cache"));
        } else {
            assert_eq!(fixture.switch(&c), Some("build_in_progress"));
            fixture.cache(&c);
        }
        std::fs::write(std::path::Path::new(&c).join("probe.rs"), "fn must_not_rebuild() {}\n").unwrap();
        fixture.release(0);
        fixture.finish();
        assert_eq!(fixture.calls(), vec![b]);
        assert!(fixture.ctx.content_ready.load(Ordering::Acquire));
        assert!(fixture.ctx.index.read().unwrap().index.contains_key("current_marker"));
        assert!(!fixture.ctx.index.read().unwrap().index.contains_key("must_not_rebuild"));
    }
}

#[test]
fn test_cross_load_content_index_stale_failure_terminal_failure_and_manual_recovery() {
    for superseded in [true, false] {
        let mut fixture = ContentBuildFixture::new();
        let b = fixture.dirs[0].clone();
        std::fs::remove_dir_all(&b).unwrap();
        fixture.pause(&b);
        fixture.start(false);
        fixture.reached(0);
        if superseded { assert_eq!(fixture.switch(&fixture.dirs[1]), Some("build_in_progress")); }
        fixture.release(0);
        fixture.finish();
        if superseded {
            assert!(fixture.ctx.content_ready.load(Ordering::Acquire));
            assert!(super::current_content_build_failure(&fixture.ctx).is_none());
        } else {
            assert!(!fixture.ctx.content_ready.load(Ordering::Acquire));
            let error = super::build_index_not_ready_response(&fixture.ctx, "content");
            let body: Value = serde_json::from_str(&error.content[0].text).unwrap();
            assert_eq!(body["error"], "INDEX_BUILD_FAILED");
            assert!(body["hint"].as_str().unwrap().contains("xray_reindex"));
            let info = super::handle_xray_info(&fixture.ctx, &json!({}));
            let body: Value = serde_json::from_str(&info.content[0].text).unwrap();
            assert_eq!(body["indexes"][0]["status"], "failed");
            {
                let mut ws = fixture.ctx.workspace.write().unwrap();
                ws.generation += 1;
                ws.status = WorkspaceStatus::Reindexing;
            }
            assert!(super::current_content_build_failure(&fixture.ctx).is_none());
            let awaiting = super::handle_xray_info(&fixture.ctx, &json!({}));
            let body: Value = serde_json::from_str(&awaiting.content[0].text).unwrap();
            assert_eq!(body["indexes"][0]["status"], "building");
            std::fs::create_dir(&b).unwrap();
            std::fs::write(std::path::Path::new(&b).join("probe.rs"), "fn recovered_marker() {}\n").unwrap();
            let result = super::handle_xray_reindex(&fixture.ctx, &json!({}));
            assert!(!result.is_error, "{}", result.content[0].text);
            assert!(fixture.ctx.content_ready.load(Ordering::Acquire));
            assert!(super::current_content_build_failure(&fixture.ctx).is_none());
            assert!(!fixture.ctx.content_building.load(Ordering::Acquire));
            // A roots-only change is awaiting explicit reindexing, not a failed build.
            fixture.ctx.workspace.write().unwrap().generation += 1;
            fixture.ctx.content_ready.store(false, Ordering::Release);
            assert!(super::current_content_build_failure(&fixture.ctx).is_none());
        }
    }
}

#[test]
fn test_cross_load_content_index_empty_current_and_watch_save_failure() {
    for empty in [true, false] {
        let mut fixture = ContentBuildFixture::new();
        let b = fixture.dirs[0].clone();
        let c = fixture.dirs[1].clone();
        if empty {
            std::fs::remove_file(std::path::Path::new(&c).join("probe.rs")).unwrap();
        } else {
            fixture.ctx.index.write().unwrap().path_to_id = Some(Default::default());
            std::fs::write(&fixture.ctx.index_base, b"not a directory").unwrap();
        }

        fixture.pause(&b);
        fixture.start(false);
        fixture.reached(0);
        assert_eq!(fixture.switch(&c), Some("build_in_progress"));
        fixture.release(0);
        fixture.finish();
        assert!(fixture.ctx.content_ready.load(Ordering::Acquire));
        let index = fixture.ctx.index.read().unwrap();
        assert!(crate::path_eq(&index.root, &c));
        assert!(fixture.ctx.file_index_dirty.load(Ordering::Acquire));
        assert!(fixture.ctx.index_epoch.load(Ordering::Acquire) > 1);
        if empty {
            assert_eq!(index.live_file_count(), 0);
            drop(index);
            let info = super::handle_xray_info(&fixture.ctx, &json!({}));
            let body: Value = serde_json::from_str(&info.content[0].text).unwrap();
            assert_eq!(body["indexes"][0]["files"], 0);
            assert!(body["indexes"][0].get("status").is_none());
        } else {
            assert!(index.index.contains_key("current_marker"));
            assert!(!index.path_to_id.as_ref().unwrap().is_empty());
            assert!(index.file_tokens_authoritative);
        }
    }
}

struct ReleaseContentTestBarrier(Option<std::sync::mpsc::Sender<()>>);

impl Drop for ReleaseContentTestBarrier {
    fn drop(&mut self) {
        if let Some(resume) = self.0.take() { let _ = resume.send(()); }
    }
}

#[test]
fn test_primary_reindex_supersession_fences_publication_status_and_rollback() {
    for (definitions, fail, aba) in [
        (false, false, false), (false, true, false), (true, false, false),
        (false, false, true), (true, false, true),
    ] {
        let mut fixture = ContentBuildFixture::new();
        fixture.ctx.def_index = make_ctx_with_defs().def_index;
        fixture.ctx.def_extensions = vec!["rs".into()];
        let b = fixture.dirs[0].clone();
        let c = fixture.dirs[1].clone();
        let epoch = fixture.ctx.index_epoch.load(Ordering::Acquire);
        let watch_generation = fixture.ctx.watcher_generation.load(Ordering::Acquire);
        let (reached, paused) = std::sync::mpsc::channel();
        let (resume, release) = std::sync::mpsc::channel();
        super::PRIMARY_REINDEX_BARRIERS.lock().unwrap().insert(
            b.clone(), super::ContentCrossBuildBarrier { reached, resume: release },
        );
        let result = std::thread::scope(|threads| {
            let release_on_drop = ReleaseContentTestBarrier(Some(resume));
            let worker = threads.spawn(|| {
                if definitions { super::handle_xray_reindex_definitions(&fixture.ctx, &json!({})) }
                else { super::handle_xray_reindex(&fixture.ctx, &json!({})) }
            });
            paused.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            {
                let mut ws = fixture.ctx.workspace.write().unwrap();
                ws.set_dir(c.clone());
                ws.generation += 1;
                if aba {
                    ws.set_dir(b.clone());
                    ws.generation += 1;
                }
                ws.mode = WorkspaceBindingMode::ClientRoots;
                ws.status = WorkspaceStatus::Reindexing;
                fixture.ctx.content_ready.store(false, Ordering::Release);
                fixture.ctx.def_ready.store(false, Ordering::Release);
            }
            if fail { std::fs::remove_dir_all(&b).unwrap(); }
            drop(release_on_drop);
            worker.join().unwrap()
        });
        assert!(result.is_error);
        let body: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert_eq!(body["error"], "WORKSPACE_SUPERSEDED");
        let ws = fixture.ctx.workspace.read().unwrap();
        assert!(crate::path_eq(&ws.dir, if aba { &b } else { &c }));
        assert_eq!(ws.status, WorkspaceStatus::Reindexing);
        assert_eq!(ws.mode, WorkspaceBindingMode::ClientRoots);
        drop(ws);
        assert!(!fixture.ctx.content_ready.load(Ordering::Acquire));
        assert!(!fixture.ctx.def_ready.load(Ordering::Acquire));
        assert!(!fixture.ctx.content_building.load(Ordering::Acquire));
        assert!(!fixture.ctx.def_building.load(Ordering::Acquire));
        assert_eq!(fixture.ctx.index_epoch.load(Ordering::Acquire), epoch);
        assert_eq!(fixture.ctx.watcher_generation.load(Ordering::Acquire), watch_generation);
        assert!(!crate::path_eq(&fixture.ctx.index.read().unwrap().root, &b));
        assert!(!crate::path_eq(&fixture.ctx.def_index.as_ref().unwrap().read().unwrap().root, &b));
        assert!(super::current_content_build_failure(&fixture.ctx).is_none());
    }
}

#[test]
fn test_primary_content_owner_hands_off_after_new_definition_commit() {
    let mut fixture = ContentBuildFixture::new();
    fixture.ctx.def_index = make_ctx_with_defs().def_index;
    fixture.ctx.def_extensions = vec!["rs".into()];
    let b = fixture.dirs[0].clone();
    let c = fixture.dirs[1].clone();
    let (reached, paused) = std::sync::mpsc::channel();
    let (resume, release) = std::sync::mpsc::channel();
    super::PRIMARY_REINDEX_BARRIERS.lock().unwrap().insert(
        b, super::ContentCrossBuildBarrier { reached, resume: release },
    );
    let (done, completion) = std::sync::mpsc::channel();
    super::CONTENT_CROSS_BUILD_COMPLETED.lock().unwrap().insert(c.clone(), done);
    fixture.completion = Some(completion);
    std::thread::scope(|threads| {
        let release_on_drop = ReleaseContentTestBarrier(Some(resume));
        let worker = threads.spawn(|| super::handle_xray_reindex(&fixture.ctx, &json!({})));
        paused.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        {
            let mut ws = fixture.ctx.workspace.write().unwrap();
            ws.set_dir(c.clone());
            ws.generation += 1;
            ws.status = WorkspaceStatus::Reindexing;
        }
        let newer = super::handle_xray_reindex_definitions(&fixture.ctx, &json!({}));
        assert!(!newer.is_error, "{}", newer.content[0].text);
        let body: Value = serde_json::from_str(&newer.content[0].text).unwrap();
        assert_eq!(body["contentIndexAction"], "build_in_progress");
        drop(release_on_drop);
        assert!(worker.join().unwrap().is_error);
    });
    fixture.finish();
    assert!(fixture.ctx.content_ready.load(Ordering::Acquire));
    assert!(fixture.ctx.index.read().unwrap().index.contains_key("current_marker"));
    assert_eq!(fixture.calls(), vec![c]);
}

#[test]
fn test_workspace_completion_git_readiness_ownership() {
    use std::sync::atomic::Ordering;
    let ctx = HandlerContext::default();
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir("a".into());
        ws.generation = 0;
    }
    ctx.git_cache_ready.store(false, Ordering::Release);
    let publication_barrier = ctx.git_cache.write().unwrap();
    std::thread::scope(|threads| {
        let stale = threads.spawn(|| super::publish_workspace_git_cache(
            &ctx.git_cache, &ctx.git_cache_ready, &ctx.workspace, "a", 0, None,
        ));
        {
            let mut ws = ctx.workspace.write().unwrap();
            ws.set_dir("b".into());
            ws.generation += 1;
            ws.set_dir("a".into());
            ws.generation += 1;
        }
        drop(publication_barrier);
        stale.join().unwrap();
    });
    assert!(!ctx.git_cache_ready.load(Ordering::Acquire));
    super::publish_workspace_git_cache(
        &ctx.git_cache, &ctx.git_cache_ready, &ctx.workspace, "a", 2, None,
    );
    assert!(ctx.git_cache_ready.load(Ordering::Acquire));
}

#[test]
#[cfg(feature = "lang-rust")]
fn test_workspace_completion_same_root_pinned_reindexing() {
    use std::sync::{Arc, RwLock};
    use std::sync::atomic::Ordering;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("probe.rs"), "fn pinned_marker() {}\n").unwrap();
    let dir = crate::clean_path(&std::fs::canonicalize(root).unwrap().to_string_lossy());
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs".into();
    ctx.def_extensions = vec!["rs".into()];
    ctx.index_base = tmp.path().join("indexes");
    let definitions = crate::definitions::build_definition_index(&crate::definitions::DefIndexArgs {
        dir: dir.clone(), ext: "rs".into(), threads: 1, respect_git_exclude: false,
    });
    crate::definitions::save_definition_index(&definitions, &ctx.index_base).unwrap();
    ctx.def_index = Some(Arc::new(RwLock::new(definitions)));
    for tool in ["xray_reindex", "xray_reindex_definitions"] {
        {
            let mut ws = ctx.workspace.write().unwrap();
            ws.set_dir(dir.clone());
            ws.mode = WorkspaceBindingMode::PinnedCli;
            ws.status = WorkspaceStatus::Reindexing;
            ws.generation = 9;
        }

        let result = super::dispatch_tool(&ctx, tool, &json!({"ext": ["rs"]}));
        assert!(!result.is_error, "{}", result.content[0].text);
        let ws = ctx.workspace.read().unwrap();
        assert_eq!(ws.mode, WorkspaceBindingMode::PinnedCli);
        assert_eq!(ws.generation, 9);
        assert_eq!(ws.status, WorkspaceStatus::Resolved);
        assert!(ctx.content_ready.load(Ordering::Acquire));
        assert!(ctx.def_ready.load(Ordering::Acquire));
    }
}

#[test]
#[cfg(feature = "lang-rust")]
fn test_workspace_completion_failed_primary_preserves_old_workers() {
    use std::sync::{Arc, RwLock};
    use std::sync::atomic::Ordering;
    for tool in ["xray_reindex", "xray_reindex_definitions"] {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("probe.rs"), "fn new_workspace() {}\n").unwrap();
        let a = crate::clean_path(&std::fs::canonicalize(a).unwrap().to_string_lossy());
        let b = crate::clean_path(&std::fs::canonicalize(b).unwrap().to_string_lossy());
        let mut ctx = HandlerContext::default();
        ctx.server_ext = "rs".into();
        ctx.index_base = tmp.path().join("indexes");
        ctx.watch_enabled = true;
        ctx.watcher_generation.store(7, Ordering::Release);
        ctx.index.write().unwrap().root = a.clone();
        ctx.def_index = Some(Arc::new(RwLock::new(crate::definitions::DefinitionIndex {
            root: a.clone(), extensions: vec!["rs".into()], ..Default::default()
        })));
        {
            let mut ws = ctx.workspace.write().unwrap();
            ws.set_dir(a.clone());
            ws.mode = WorkspaceBindingMode::ClientRoots;
            ws.status = WorkspaceStatus::Resolved;
            ws.generation = 11;
        }
        std::thread::scope(|threads| {
            let _ = threads.spawn(|| {
                if tool == "xray_reindex" {
                    let _lock = ctx.index.write().unwrap();
                    panic!("poison primary content publication");
                } else {
                    let _lock = ctx.def_index.as_ref().unwrap().write().unwrap();
                    panic!("poison primary definition publication");
                }
            }).join();
        });
        let args = json!({"dir": b, "ext": ["rs"]});
        let result = if tool == "xray_reindex" {
            super::handle_xray_reindex_inner(&ctx, &args)
        } else {
            super::handle_xray_reindex_definitions_inner(&ctx, &args)
        };
        assert!(result.is_error);
        let ws = ctx.workspace.read().unwrap();
        assert!(code_xray::path_eq(&ws.dir, &a));
        assert_eq!(ws.mode, WorkspaceBindingMode::ClientRoots);
        assert_eq!(ws.status, WorkspaceStatus::Resolved);
        assert_eq!(ws.generation, 11);
        assert_eq!(ctx.watcher_generation.load(Ordering::Acquire), 7);
        assert!(ctx.content_ready.load(Ordering::Acquire));
        assert!(ctx.def_ready.load(Ordering::Acquire));
    }
}


#[cfg(feature = "lang-rust")]
fn definition_switch_fixture() -> (tempfile::TempDir, HandlerContext, Vec<String>) {
    let tmp = tempfile::tempdir().unwrap();
    let roots: Vec<String> = ["initial", "a", "b"].iter().map(|name| {
        let path = tmp.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        crate::clean_path(&std::fs::canonicalize(path).unwrap().to_string_lossy())
    }).collect();
    std::fs::write(std::path::Path::new(&roots[1]).join("a.rs"), "pub fn from_a() {}\n").unwrap();
    std::fs::write(std::path::Path::new(&roots[2]).join("b.rs"), "pub fn from_b() {}\n").unwrap();
    let mut ctx = HandlerContext::default();
    ctx.index_base = tmp.path().join("indexes");
    ctx.server_ext = "rs".into();
    ctx.def_extensions = vec!["rs".into()];
    ctx.def_index = Some(std::sync::Arc::new(std::sync::RwLock::new(
        crate::definitions::DefinitionIndex::default(),
    )));
    *ctx.index.write().unwrap() = crate::ContentIndex { root: roots[0].clone(), ..Default::default() };
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(roots[0].clone());
        ws.mode = WorkspaceBindingMode::ManualOverride;
    }
    (tmp, ctx, roots)
}

#[test]
#[cfg(feature = "lang-rust")]
fn test_definition_cross_build_latest_workspace_is_built() {
    use std::sync::atomic::Ordering;
    let (_tmp, ctx, roots) = definition_switch_fixture();
    let defs = ctx.def_index.as_ref().unwrap();
    let barrier = defs.read().unwrap();
    for dir in &roots[1..] {
        let response = super::dispatch_tool(&ctx, "xray_reindex", &json!({"dir":dir,"ext":["rs"]}));
        assert!(!response.is_error, "{}", response.content[0].text);
        assert!(wait_until(|| ctx.def_building.load(Ordering::Acquire)));
    }
    drop(barrier);
    assert!(wait_until(|| !ctx.def_building.load(Ordering::Acquire)));
    assert!(ctx.def_ready.load(Ordering::Acquire));
    let response = super::dispatch_tool(&ctx, "xray_definitions", &json!({"name":["from_a","from_b"],"exactNameOnly":true}));
    assert!(!response.is_error, "{}", response.content[0].text);
    let value: serde_json::Value = serde_json::from_str(&response.content[0].text).unwrap();
    let names: Vec<_> = value["definitions"].as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["from_b"], "outgoing definitions must not be served as current: {value}");
    assert!(code_xray::path_eq(&defs.read().unwrap().root, &roots[2]));
}

#[test]
#[cfg(feature = "lang-rust")]
fn test_definition_cross_build_same_workspace_control() {
    use std::sync::atomic::Ordering;
    let (_tmp, ctx, roots) = definition_switch_fixture();
    ctx.workspace.write().unwrap().set_dir(roots[1].clone());
    assert_eq!(cross_load_definition_index(&ctx, &roots[1]), Some("background_build"));
    assert!(wait_until(|| !ctx.def_building.load(Ordering::Acquire) && ctx.def_ready.load(Ordering::Acquire)));
    let defs = ctx.def_index.as_ref().unwrap().read().unwrap();
    assert!(defs.definitions.iter().any(|d| d.name == "from_a"));
    assert!(code_xray::path_eq(&defs.root, &roots[1]));
}

#[test]
#[cfg(feature = "lang-rust")]
fn test_definition_cross_build_generation_rejects_round_trip() {
    use std::sync::atomic::Ordering;
    let (_tmp, ctx, roots) = definition_switch_fixture();
    ctx.workspace.write().unwrap().set_dir(roots[1].clone());
    let defs = ctx.def_index.as_ref().unwrap();
    let barrier = defs.read().unwrap();
    assert_eq!(cross_load_definition_index(&ctx, &roots[1]), Some("background_build"));
    assert!(wait_until(|| crate::definitions::load_definition_index(&roots[1], "rs", &ctx.index_base).is_ok()));
    std::fs::write(std::path::Path::new(&roots[1]).join("a.rs"), "pub fn from_new_a() {}\n").unwrap();
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(roots[2].clone());
        ws.generation += 1;
        ws.set_dir(roots[1].clone());
        ws.generation += 1;
    }
    drop(barrier);
    assert!(wait_until(|| !ctx.def_building.load(Ordering::Acquire)));
    assert!(ctx.def_ready.load(Ordering::Acquire));
    let defs = defs.read().unwrap();
    assert!(defs.definitions.iter().any(|d| d.name == "from_new_a"));
    assert!(!defs.definitions.iter().any(|d| d.name == "from_a"));
}

#[test]
#[cfg(feature = "lang-rust")]
fn test_definition_cross_build_preserves_new_workspace_cache() {
    use std::sync::{Arc, atomic::Ordering};
    let (_tmp, ctx, roots) = definition_switch_fixture();
    let cached = crate::definitions::build_definition_index(&crate::definitions::DefIndexArgs {
        dir: roots[2].clone(), ext: "rs".into(), threads: 1, respect_git_exclude: true,
    });
    crate::definitions::save_definition_index(&cached, &ctx.index_base).unwrap();
    std::fs::write(std::path::Path::new(&roots[2]).join("b.rs"), "pub fn uncached_b() {}\n").unwrap();
    let ctx = Arc::new(ctx);
    let defs = Arc::clone(ctx.def_index.as_ref().unwrap());
    let barrier = defs.read().unwrap();
    let response = super::dispatch_tool(&ctx, "xray_reindex", &json!({"dir":roots[1],"ext":["rs"]}));
    assert!(!response.is_error, "{}", response.content[0].text);
    assert!(wait_until(|| crate::definitions::load_definition_index(&roots[1], "rs", &ctx.index_base).is_ok()));
    let next_ctx = Arc::clone(&ctx);
    let next_dir = roots[2].clone();
    let switch = std::thread::spawn(move || super::dispatch_tool(
        &next_ctx, "xray_reindex", &json!({"dir":next_dir,"ext":["rs"]}),
    ));
    assert!(wait_until(|| code_xray::path_eq(&ctx.server_dir(), &roots[2])));
    drop(barrier);
    let response = switch.join().unwrap();
    assert!(!response.is_error, "{}", response.content[0].text);
    let value: serde_json::Value = serde_json::from_str(&response.content[0].text).unwrap();
    assert_eq!(value["defIndexAction"], "loaded_cache");
    assert!(wait_until(|| !ctx.def_building.load(Ordering::Acquire)));
    assert!(ctx.def_ready.load(Ordering::Acquire));
    let defs = defs.read().unwrap();
    assert!(code_xray::path_eq(&defs.root, &roots[2]));
    assert!(defs.definitions.iter().any(|d| d.name == "from_b"));
    assert!(!defs.definitions.iter().any(|d| d.name == "from_a"));
}

struct WatcherScopeStop(std::sync::Arc<std::sync::atomic::AtomicU64>);

impl Drop for WatcherScopeStop {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
}

fn reindex_watcher_extension_scope(ext: &str, same_workspace: bool, periodic: bool) {
    use std::sync::atomic::Ordering;
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = if same_workspace { a.clone() } else { tmp.path().join("b") };
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(b.join("probe.rs"), "fn rsmarker() {}\n").unwrap();
    std::fs::write(b.join("probe.cs"), "class CsMarker {}\n").unwrap();
    let a = crate::clean_path(&std::fs::canonicalize(a).unwrap().to_string_lossy());
    let b = std::fs::canonicalize(b).unwrap();
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "cs".into();
    ctx.index_base = tmp.path().join("indexes");
    ctx.watch_enabled = true;
    ctx.watch_debounce_ms = 20;
    ctx.periodic_rescan_enabled = periodic;
    ctx.rescan_interval_sec = 10;
    ctx.content_ready.store(true, Ordering::Release);
    ctx.def_ready.store(true, Ordering::Release);
    *ctx.index.write().unwrap() = crate::mcp::watcher::build_watch_index_from(crate::ContentIndex {
        root: a.clone(), extensions: vec!["cs".into()], ..Default::default()
    });
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(a.clone());
        ws.mode = WorkspaceBindingMode::ManualOverride;
    }
    let _stop = WatcherScopeStop(std::sync::Arc::clone(&ctx.watcher_generation));
    super::restart_watcher_for_workspace(&ctx, &a);
    assert!(wait_until(|| {
        std::fs::write(std::path::Path::new(&a).join("watcher.signal"), "ready\n").unwrap();
        ctx.watcher_stats.events_total.load(Ordering::Relaxed) > 0
    }));
    let old_generation = ctx.watcher_generation.load(Ordering::Acquire);
    let result = super::dispatch_tool(&ctx, "xray_reindex", &json!({"dir":b.to_str().unwrap(),"ext":[ext]}));
    assert!(!result.is_error, "{}", result.content[0].text);
    if !same_workspace || ext != "cs" {
        assert!(ctx.watcher_generation.load(Ordering::Acquire) > old_generation);
    } else {
        assert_eq!(ctx.watcher_generation.load(Ordering::Acquire), old_generation);
    }
    std::fs::write(b.join("probe.rs"), "fn rsmarker_updated() {}\n").unwrap();
    std::fs::write(b.join("probe.cs"), "class CsMarkerUpdated {}\n").unwrap();
    let expected = if ext == "rs" { "rsmarker_updated" } else { "csmarkerupdated" };
    assert!(wait_until(|| ctx.index.read().unwrap().index.contains_key(expected)),
        "watcher did not update the requested extension");
    if periodic {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
        while ctx.watcher_stats.periodic_rescan_total.load(Ordering::Acquire) < 2 {
            assert!(std::time::Instant::now() < deadline, "periodic rescan did not run");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    {
        let index = ctx.index.read().unwrap();
        assert_eq!(index.extensions, vec![ext.to_string()]);
        assert!(index.index.contains_key(expected));
        assert!(index.files.iter().all(|file| file.is_empty() || std::path::Path::new(file).extension().is_some_and(|e| e == ext)));
    }
    std::fs::remove_file(b.join(format!("probe.{ext}"))).unwrap();
    std::fs::write(b.join(format!("created.{ext}")), "createdmarker\n").unwrap();
    assert!(wait_until(|| {
        let index = ctx.index.read().unwrap();
        index.index.contains_key("createdmarker") && !index.index.contains_key(expected)
    }), "watcher did not track create/delete after reindex");
    let index = ctx.index.read().unwrap();
    super::save_content_index(&index, &ctx.index_base).unwrap();
    let loaded = super::load_content_index(&ctx.server_dir(), ext, &ctx.index_base).unwrap();
    assert_eq!(loaded.extensions, vec![ext.to_string()]);
    assert!(loaded.index.contains_key("createdmarker"));
    assert!(loaded.files.iter().all(|file| file.is_empty() || std::path::Path::new(file).extension().is_some_and(|e| e == ext)), "{:?}", loaded.files);
}

#[test]
fn test_reindex_watcher_scope_workspace_switch() {
    reindex_watcher_extension_scope("rs", false, false);
}

#[test]
fn test_reindex_watcher_scope_same_workspace() {
    reindex_watcher_extension_scope("rs", true, false);
}

#[test]
fn test_reindex_watcher_scope_unchanged_control() {
    reindex_watcher_extension_scope("cs", true, false);
}

#[test]
fn test_reindex_watcher_scope_periodic_rescan() {
    reindex_watcher_extension_scope("rs", false, true);
}

fn wait_until(predicate: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    predicate()
}

/// D1 — `rollback_workspace_state` must restore ALL four mutable fields of
/// `WorkspaceBinding` (dir, canonical_dir-via-set_dir, mode, generation) AND
/// restore the original status. Regression of any one of these would leave the
/// server stuck in `Reindexing` or pointing at a corrupted dir after an error.
#[test]
fn test_rollback_workspace_state_restores_all_fields() {
    let ctx = HandlerContext::default();

    // Snapshot the "good" pre-switch state.
    let original_dir = "/good/workspace".to_string();
    let original_mode = WorkspaceBindingMode::ManualOverride;
    let original_generation: u64 = 42;
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(original_dir.clone());
        ws.mode = original_mode;
        ws.generation = original_generation;
        ws.status = WorkspaceStatus::Resolved;
    }

    // Simulate a workspace switch in progress: every field is changed and
    // status is flipped to `Reindexing` (mid-flight).
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir("/bad/half_built".to_string());
        ws.mode = WorkspaceBindingMode::DotBootstrap;
        ws.generation = 999;
        ws.status = WorkspaceStatus::Reindexing;
    }

    // Roll back.
    rollback_workspace_state(&ctx, ("/bad/half_built", 999), &original_dir, original_mode, WorkspaceStatus::Resolved, original_generation);

    let ws = ctx.workspace.read().unwrap();
    assert_eq!(ws.dir, original_dir,
        "rollback must restore dir to the pre-switch value");
    assert!(ws.canonical_dir.contains("good") || ws.canonical_dir == original_dir,
        "rollback must recompute canonical_dir from the restored dir; got: {}", ws.canonical_dir);
    assert_eq!(ws.mode, original_mode,
        "rollback must restore mode (regression: leaving DotBootstrap leaks bootstrap behavior)");
    assert_eq!(ws.generation, original_generation,
        "rollback must restore generation (regression: stale generation breaks generation-safe commits)");
    assert_eq!(ws.status, WorkspaceStatus::Resolved,
        "rollback must restore the original Resolved status");
}

#[test]
fn test_replace_live_definition_index_advances_above_both_generations() {
    let mut current = crate::definitions::DefinitionIndex::default();
    current.definition_generation = 7;
    let mut replacement = crate::definitions::DefinitionIndex::default();
    replacement.definition_generation = 11;

    crate::definitions::replace_live_definition_index(&mut current, replacement);

    assert_eq!(current.definition_generation, 12);
}

/// D2 — `cross_load_definition_index` must early-return `None` when the
/// HandlerContext was constructed without a definition index (`def_index = None`).
/// Regression here would either panic on `.unwrap()` of the missing Arc or spawn
/// a useless background thread that then fails to write to the absent index.
#[test]
fn test_cross_load_definition_index_returns_none_when_def_index_disabled() {
    let ctx = HandlerContext::default();
    assert!(ctx.def_index.is_none(),
        "Precondition: HandlerContext::default() must have def_index = None");

    let result = cross_load_definition_index(&ctx, "/any/dir");

    assert!(result.is_none(),
        "cross_load_definition_index must return None when def_index is disabled \
         (regression: would otherwise spawn background thread that panics or no-ops)");
}

/// Regression guard: content-index cache cross-load must preserve watch mutation mode.
/// The cached read-only index has no watch lookups; handler code must re-enable them
/// (set `file_tokens_authoritative=true`, populate `path_to_id`) and schedule a
/// background `rebuild_file_tokens` so the first user edit doesn't pay the rebuild
/// cost under the exclusive watcher write lock (C2-A, 2026-05-02).
#[test]
fn test_cross_load_content_index_preserves_watch_lazy_reverse_map_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("watched.rs"),
        "fn cross_load_watch_mode_token() {}\n",
    ).unwrap();

    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );
    let index_base = tmp.path().join("indexes");
    let content_index = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: workspace_str.clone(),
        ext: "rs".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    assert!(!content_index.file_tokens_authoritative,
        "Precondition: cached build output should be read-only/plain");
    crate::index::save_content_index(&content_index, &index_base).unwrap();

    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = index_base;
    ctx.workspace.write().unwrap().set_dir(workspace_str.clone());
    {
        let mut current = ctx.index.write().unwrap();
        *current = crate::mcp::watcher::build_watch_index_from(crate::ContentIndex {
            root: workspace_str.clone(),
            extensions: vec!["rs".to_string()],
            ..Default::default()
        });
        assert!(current.file_tokens_authoritative,
            "Precondition: existing runtime index must represent watcher mode");
    }

    let action = cross_load_content_index(&ctx, &workspace_str);

    assert_eq!(action, Some("loaded_cache"));
    let idx = ctx.index.read().unwrap();
    assert!(idx.file_tokens_authoritative,
        "cross-load must preserve watcher-authoritative reverse-map mode");
    assert!(idx.path_to_id.as_ref().map(|lookup| !lookup.is_empty()).unwrap_or(false),
        "watch-mode cross-load must rebuild path lookup for cached content index");
    drop(idx);
    // C2-A: cache-load schedules a background `rebuild_file_tokens` so the
    // first user edit doesn't pay the rebuild cost under the watcher write
    // lock. Wait until that background warmer populates the reverse map.
    assert!(
        wait_until(|| ctx.index.read().map(|i| !i.file_tokens.is_empty()).unwrap_or(false)),
        "watch-mode cross-load must warm per-file token reverse map in background"
    );
}
#[test]
fn test_cross_load_content_index_background_build_preserves_watch_lazy_reverse_map_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace_bg");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("watched.rs"),
        "fn background_cross_load_watch_mode_token() {}\n",
    ).unwrap();

    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = tmp.path().join("indexes_bg");
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(workspace_str.clone());
    }
    {
        let mut current = ctx.index.write().unwrap();
        *current = crate::mcp::watcher::build_watch_index_from(crate::ContentIndex {
            root: "old-workspace".to_string(),
            extensions: vec!["rs".to_string()],
            ..Default::default()
        });
        assert!(current.file_tokens_authoritative,
            "Precondition: existing runtime index must represent watcher mode");
    }

    let action = cross_load_content_index(&ctx, &workspace_str);

    assert_eq!(action, Some("background_build"));
    assert!(wait_until(|| ctx.content_ready.load(std::sync::atomic::Ordering::Acquire)),
        "background content build should publish readiness after installing the new index");
    let idx = ctx.index.read().unwrap();
    assert_eq!(idx.root, workspace_str);
    assert!(idx.file_tokens_authoritative,
        "background cross-load must preserve watcher-authoritative mode");
    assert!(idx.path_to_id.as_ref().map(|lookup| !lookup.is_empty()).unwrap_or(false),
        "background cross-load must build path lookup for watcher mutation mode");
    drop(idx);
    // C2-A: see sibling test above. Background build path also schedules a
    // background `rebuild_file_tokens` post-install.
    assert!(
        wait_until(|| ctx.index.read().map(|i| !i.file_tokens.is_empty()).unwrap_or(false)),
        "background cross-load must warm per-file token reverse map in background"
    );
}

#[test]
fn test_cross_load_content_index_background_build_failure_does_not_mark_ready() {
    let tmp = tempfile::tempdir().unwrap();
    let missing_workspace = tmp.path().join("missing_workspace");
    let missing_workspace_str = crate::clean_path(&missing_workspace.to_string_lossy());
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = tmp.path().join("indexes_fail");
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(missing_workspace_str.clone());
    }
    ctx.content_ready.store(true, std::sync::atomic::Ordering::Release);
    {
        let mut current = ctx.index.write().unwrap();
        *current = crate::ContentIndex {
            root: "old-workspace".to_string(),
            extensions: vec!["rs".to_string()],
            ..Default::default()
        };
    }

    let action = cross_load_content_index(&ctx, &missing_workspace_str);

    assert_eq!(action, Some("background_build"));
    assert!(wait_until(|| !ctx.content_building.load(std::sync::atomic::Ordering::Acquire)),
        "background content build should exit after build failure");
    assert!(!ctx.content_ready.load(std::sync::atomic::Ordering::Acquire),
        "failed background cross-load must not advertise content readiness");
    let idx = ctx.index.read().unwrap();
    assert_eq!(idx.root, "old-workspace",
        "failed background cross-load must not replace the previous index");
}


#[test]
fn test_cross_load_content_index_existing_background_build_does_not_mark_ready() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace_waiting");
    std::fs::create_dir_all(&workspace).unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = tmp.path().join("indexes_waiting");
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(workspace_str.clone());
    }
    ctx.content_ready.store(true, std::sync::atomic::Ordering::Release);
    ctx.content_building.store(true, std::sync::atomic::Ordering::Release);
    {
        let mut current = ctx.index.write().unwrap();
        *current = crate::ContentIndex {
            root: "old-workspace".to_string(),
            extensions: vec!["rs".to_string()],
            ..Default::default()
        };
    }

    let action = cross_load_content_index(&ctx, &workspace_str);

    assert_eq!(action, Some("build_in_progress"));
    assert!(!ctx.content_ready.load(std::sync::atomic::Ordering::Acquire),
        "new workspace must stay not-ready while another content build is in flight");
    let idx = ctx.index.read().unwrap();
    assert_eq!(idx.root, "old-workspace",
        "pre-existing build gate must not replace the index for this workspace");
}

#[test]
fn test_cross_load_content_index_stale_background_build_does_not_publish() {
    let tmp = tempfile::tempdir().unwrap();
    let stale_workspace = tmp.path().join("stale_workspace");
    let current_workspace = tmp.path().join("current_workspace");
    std::fs::create_dir_all(&stale_workspace).unwrap();
    std::fs::create_dir_all(&current_workspace).unwrap();
    std::fs::write(
        stale_workspace.join("stale.rs"),
        "fn stale_background_build_token() {}\n",
    ).unwrap();
    let stale_workspace_str = crate::clean_path(
        &std::fs::canonicalize(&stale_workspace).unwrap().to_string_lossy(),
    );
    let current_workspace_str = crate::clean_path(
        &std::fs::canonicalize(&current_workspace).unwrap().to_string_lossy(),
    );
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = tmp.path().join("indexes_stale");
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(current_workspace_str);
        ws.generation += 1;
    }
    {
        let mut current = ctx.index.write().unwrap();
        *current = crate::ContentIndex {
            root: "old-workspace".to_string(),
            extensions: vec!["rs".to_string()],
            ..Default::default()
        };
    }

    let action = cross_load_content_index(&ctx, &stale_workspace_str);

    assert_eq!(action, Some("superseded"));
    assert!(!ctx.content_building.load(std::sync::atomic::Ordering::Acquire),
        "already superseded work should not start a build");
    assert!(ctx.content_ready.load(std::sync::atomic::Ordering::Acquire),
        "superseded work must not reset the current workspace readiness");
    let idx = ctx.index.read().unwrap();
    assert_eq!(idx.root, "old-workspace",
        "stale background build must not publish an index for another workspace");
}


/// Regression guard: explicit xray_reindex must also preserve watch mutation mode.
/// This protects the handler-level sentinel from regressing back to path_to_id presence.
#[test]
fn test_handle_xray_reindex_inner_preserves_watch_lazy_reverse_map_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("watched.rs"),
        "fn handler_reindex_watch_mode_token() {}\n",
    ).unwrap();

    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = tmp.path().join("indexes");
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(workspace_str.clone());
    }
    {
        let mut current = ctx.index.write().unwrap();
        *current = crate::mcp::watcher::build_watch_index_from(crate::ContentIndex {
            root: workspace_str.clone(),
            extensions: vec!["rs".to_string()],
            ..Default::default()
        });
        assert!(current.file_tokens_authoritative,
            "Precondition: existing runtime index must represent watcher mode");
    }

    let result = handle_xray_reindex_inner(&ctx, &json!({}));

    assert!(!result.is_error,
        "same-workspace xray_reindex should succeed in PinnedCli mode");
    let idx = ctx.index.read().unwrap();
    assert!(idx.file_tokens_authoritative,
        "xray_reindex must preserve watcher-authoritative reverse-map mode");
    assert!(idx.path_to_id.as_ref().map(|lookup| !lookup.is_empty()).unwrap_or(false),
        "watch-mode xray_reindex must rebuild path lookup for the rebuilt index");
    drop(idx);
    // C2-A: xray_reindex install path also schedules a background warmer.
    assert!(
        wait_until(|| ctx.index.read().map(|i| !i.file_tokens.is_empty()).unwrap_or(false)),
        "watch-mode xray_reindex must warm per-file token reverse map in background"
    );
}


/// D3 — `handle_xray_reindex_inner` must reject a workspace switch when the
/// server is in `PinnedCli` mode (started with `--dir <path>`). The request
/// must return an error result AND must NOT mutate the workspace binding
/// (dir, mode, generation must all stay at their pinned values).
///
/// This test exercises the early-return guard at the top of
/// `handle_xray_reindex_inner` — without it, a malicious or buggy reindex call
/// could escape the CLI-pinned workspace.
#[test]
fn test_handle_xray_reindex_inner_rejects_switch_in_pinned_cli_mode() {
    let ctx = HandlerContext::default();
    // HandlerContext::default() pins ws to ".", PinnedCli mode, generation=1.
    let pinned_dir_before = ctx.server_dir();
    let canonical_dir_before;
    let mode_before;
    let generation_before;
    {
        let ws = ctx.workspace.read().unwrap();
        assert_eq!(ws.mode, WorkspaceBindingMode::PinnedCli,
            "Precondition: default ctx must be in PinnedCli mode");
        canonical_dir_before = ws.canonical_dir.clone();
        mode_before = ws.mode;
        generation_before = ws.generation;
    }

    // Request a switch to a totally different directory.
    // We pick a non-existent path so canonicalize() falls back to the literal
    // string — that's still distinct from the pinned ".", so workspace_changed=true,
    // which triggers the PinnedCli guard.
    let switch_target = if cfg!(windows) {
        "Z:\\nonexistent_xray_test_target_d3"
    } else {
        "/nonexistent_xray_test_target_d3"
    };
    let args = json!({"dir": switch_target});

    let result = handle_xray_reindex_inner(&ctx, &args);
    assert!(result.is_error,
        "PinnedCli mode must reject workspace switches; got success result");
    let msg = result.content.iter()
        .map(|c| c.text.as_str())
        .collect::<Vec<_>>().join(" ");
    let msg_lower = msg.to_lowercase();
    assert!(msg_lower.contains("pinned") || msg_lower.contains("--dir"),
        "Error message must explain the pinning constraint; got: {}", msg);
    assert!(msg.contains("omit the `dir` argument"),
        "Error message must tell callers how to rebuild the pinned workspace; got: {}", msg);

    // Critical: workspace state must be UNCHANGED (no partial mutation).
    let ws = ctx.workspace.read().unwrap();
    assert_eq!(ws.dir, pinned_dir_before,
        "Rejected switch must NOT mutate workspace.dir");
    assert_eq!(ws.canonical_dir, canonical_dir_before,
        "Rejected switch must NOT mutate workspace.canonical_dir");
    assert_eq!(ws.mode, mode_before,
        "Rejected switch must NOT mutate workspace.mode");
    assert_eq!(ws.generation, generation_before,
        "Rejected switch must NOT bump workspace.generation");
}

#[cfg(windows)]
#[test]
fn test_handle_xray_reindex_definitions_allows_same_pinned_dir_with_separator_variation() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("sample.rs"),
        "fn separator_regression_sample() {}\n",
    ).unwrap();

    let canonical = std::fs::canonicalize(&workspace).unwrap();
    let forward_dir = code_xray::clean_path(&canonical.to_string_lossy());
    let backslash_dir = forward_dir.replace('/', "\\");

    let mut ctx = make_ctx_with_defs();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = tmp.path().join("indexes");
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(backslash_dir.clone());
        ws.mode = WorkspaceBindingMode::PinnedCli;
    }
    let (canonical_dir_before, generation_before) = {
        let ws = ctx.workspace.read().unwrap();
        (ws.canonical_dir.clone(), ws.generation)
    };

    let result = handle_xray_reindex_definitions_inner(
        &ctx,
        &json!({ "dir": forward_dir, "ext": ["rs"] }),
    );

    assert!(
        !result.is_error,
        "same pinned workspace with separator variation should reindex; got: {}",
        result.content[0].text
    );
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["status"], "ok");
    assert!(
        output.get("workspaceChanged").is_none(),
        "same workspace must not be reported as a switch: {}",
        output
    );

    let ws = ctx.workspace.read().unwrap();
    assert_eq!(ws.dir, backslash_dir);
    assert_eq!(ws.canonical_dir, canonical_dir_before);
    assert_eq!(ws.mode, WorkspaceBindingMode::PinnedCli);
    assert_eq!(ws.generation, generation_before);
}

#[test]
fn test_reindex_pinned_switch_errors_tell_callers_to_omit_dir() {
    let switch_target = if cfg!(windows) {
        "Z:\\nonexistent_xray_test_target_guidance"
    } else {
        "/nonexistent_xray_test_target_guidance"
    };
    let args = json!({"dir": switch_target});

    let content_ctx = HandlerContext::default();
    let definitions_ctx = make_ctx_with_defs();
    for ctx in [&content_ctx, &definitions_ctx] {
        ctx.workspace.write().unwrap().mode = WorkspaceBindingMode::PinnedCli;
    }
    let cases = [
        ("xray_reindex", handle_xray_reindex_inner(&content_ctx, &args)),
        (
            "xray_reindex_definitions",
            handle_xray_reindex_definitions_inner(&definitions_ctx, &args),
        ),
    ];

    for (tool_name, result) in cases {
        assert!(result.is_error, "{} must reject pinned workspace switches", tool_name);
        let msg = result.content.iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>().join(" ");
        assert!(
            msg.contains("omit the `dir` argument"),
            "{} pinned switch error must tell callers to omit dir; got: {}",
            tool_name,
            msg
        );
        assert!(
            msg.contains("start another server instance or use CLI"),
            "{} pinned switch error must explain how to index a different workspace; got: {}",
            tool_name,
            msg
        );
    }
}

// ─── Bug-report 2026-04-28: force-rebuild + live branch warning ─────────
//
// Validates that `xray_reindex` against the *current* workspace forces a
// fresh rebuild instead of silently returning the on-disk cache, and that
// the optional `useCache: true` opt-out preserves the legacy fast path for
// callers who prefer cold-rebuild latency over freshness. Also covers the
// branch_warning live-probe TTL cache and its workspace-switch invalidation.

/// Helper: build a content-index cache on disk for `workspace_str`, then
/// register `ctx` against it without loading anything into memory yet — so
/// each test starts from a known "cache-on-disk, empty-in-memory" state.
fn seed_disk_cache(
    ctx: &mut HandlerContext,
    tmp: &tempfile::TempDir,
    workspace_str: &str,
) {
    let index_base = tmp.path().join("indexes");
    let cache = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: workspace_str.to_string(),
        ext: "rs".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    crate::index::save_content_index(&cache, &index_base).unwrap();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = index_base;
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(workspace_str.to_string());
        ws.mode = WorkspaceBindingMode::ManualOverride;
    }
}

/// Bug-report 2026-04-28, Acceptance #3: `xray_reindex` against the current
/// workspace must rebuild from the live filesystem (`indexAction="rebuilt"`,
/// `rebuiltFromDisk=true`, `forceRebuild=true`) rather than silently reusing
/// the existing on-disk cache.
#[test]
fn test_handle_xray_reindex_inner_forces_rebuild_for_current_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("seed.rs"), "fn force_rebuild_seed() {}\n").unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );

    let mut ctx = HandlerContext::default();
    seed_disk_cache(&mut ctx, &tmp, &workspace_str);

    let result = handle_xray_reindex_inner(&ctx, &json!({}));
    assert!(!result.is_error, "current-workspace reindex must succeed");
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["indexAction"], "rebuilt",
        "current-workspace reindex must rebuild from disk, got: {}", output);
    assert_eq!(output["rebuiltFromDisk"], true,
        "rebuiltFromDisk must mirror indexAction=='rebuilt'");
    assert_eq!(output["forceRebuild"], true,
        "forceRebuild must be true for current-workspace reindex without useCache");
}

/// Bug-report 2026-04-28, Acceptance #3 (opt-out): explicit `useCache: true`
/// preserves the legacy cache-load fast path for callers who prefer
/// cold-rebuild latency over freshness.
#[test]
fn test_handle_xray_reindex_inner_use_cache_opt_in_keeps_loaded_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace_use_cache");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("seed.rs"), "fn use_cache_seed() {}\n").unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );

    let mut ctx = HandlerContext::default();
    seed_disk_cache(&mut ctx, &tmp, &workspace_str);

    let result = handle_xray_reindex_inner(&ctx, &json!({"useCache": true}));
    assert!(!result.is_error);
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["indexAction"], "loaded_cache",
        "useCache=true must keep the legacy fast path");
    assert_eq!(output["rebuiltFromDisk"], false);
    assert_eq!(output["forceRebuild"], false);
}

/// Bug-report 2026-04-28, Acceptance #4: a token added to disk after the
/// cache was built becomes searchable in the in-memory index immediately
/// after `xray_reindex` (because force-rebuild walks the live filesystem).
#[test]
fn test_handle_xray_reindex_inner_picks_up_new_tokens_from_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace_new_token");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("seed.rs"), "fn before_reindex_token() {}\n").unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );

    let mut ctx = HandlerContext::default();
    seed_disk_cache(&mut ctx, &tmp, &workspace_str);

    // Mutate the workspace AFTER the cache was built. Use a uniquely-shaped
    // identifier so substring-search is unambiguous.
    std::fs::write(
        workspace.join("seed.rs"),
        "fn after_reindex_uniq_zzqq_token() {}\n",
    ).unwrap();

    let result = handle_xray_reindex_inner(&ctx, &json!({}));
    assert!(!result.is_error);
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["indexAction"], "rebuilt");

    let idx = ctx.index.read().unwrap();
    let tokens: Vec<&String> = idx.index.keys().collect();
    assert!(
        tokens.iter().any(|t| t.contains("after_reindex_uniq_zzqq_token")),
        "force-rebuild must surface tokens added after the cache was built; tokens={:?}",
        tokens.iter().filter(|t| t.contains("reindex")).collect::<Vec<_>>()
    );
}

#[test]
fn test_507_content_cold_build_counts() {
    content_507_build_matrix(false);
}

#[test]
fn test_507_content_forced_build_counts() {
    content_507_build_matrix(true);
}

#[test]
fn test_507_content_configured_defaults_and_queries_build_counts() {
    std::fs::create_dir_all("target").unwrap();
    let tmp = tempfile::tempdir_in(std::fs::canonicalize("target").unwrap()).unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let root = crate::clean_path(&std::fs::canonicalize(&workspace).unwrap().to_string_lossy());
    let ctx = HandlerContext {
        server_ext: " Rs , JSON ,rS ".into(), index_base: tmp.path().join("indexes"),
        ..Default::default()
    };
    ctx.workspace.write().unwrap().set_dir(root.clone());
    let probe = crate::index::build_counts::Probe::new(&root);
    for (iteration, args) in [
        json!({}), json!({"ext": null}), json!({"ext": []}),
        json!({"ext": ["", " \t "]}), json!({"ext": [" RS ", " JSON ,rs "]}),
    ].iter().enumerate() {
        let marker = format!("configured_marker_{iteration}");
        std::fs::write(workspace.join("seed.rs"), format!("fn {marker}() {{}}\n")).unwrap();
        let result = super::dispatch_tool(&ctx, "xray_reindex", args);
        assert!(!result.is_error, "{}", result.content[0].text);
        let loaded = crate::index::load_content_index(&root, "rs,json,rs", &ctx.index_base).unwrap();
        assert_eq!(loaded.extensions, ["rs", "json", "rs"]);
        assert!(loaded.index.contains_key(&marker));
        for _ in 0..2 {
            let query = super::dispatch_tool(&ctx, "xray_grep", &json!({"terms": [marker]}));
            assert!(!query.is_error, "{}", query.content[0].text);
        }
        assert_eq!(probe.counts(), [iteration + 1, 0], "args={args}");
    }
}

#[test]
fn test_507_content_reindex_cached_definition_companion_build_counts() {
    let mut fixture = Definition503Fixture::new();
    fixture.ctx.watch_enabled = false;
    let definitions = crate::definitions::build_definition_index(&crate::definitions::DefIndexArgs {
        dir: fixture.alternate.clone(), ext: "sql".into(), threads: 1,
        respect_git_exclude: false,
    });
    crate::definitions::save_definition_index(&definitions, &fixture.ctx.index_base).unwrap();
    let primary = crate::index::build_counts::Probe::new(&fixture.ctx.server_dir());
    let alternate = crate::index::build_counts::Probe::new(&fixture.alternate);
    let result = super::dispatch_tool(&fixture.ctx, "xray_reindex", &json!({
        "dir": fixture.alternate, "ext": [" Rs ,JSON ,RS "],
    }));
    assert!(wait_until(|| fixture.ctx.git_cache_ready.load(Ordering::Acquire)));
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["defIndexAction"], "loaded_cache");
    assert!(!fixture.ctx.content_building.load(Ordering::Acquire));
    assert!(!fixture.ctx.def_building.load(Ordering::Acquire));
    for _ in 0..2 {
        assert!(fixture.query()["definitions"].as_array().unwrap().is_empty());
        let query = super::dispatch_tool(&fixture.ctx, "xray_grep", &json!({"terms": ["absent_marker"]}));
        assert!(!query.is_error, "{}", query.content[0].text);
    }
    assert_eq!(primary.counts(), [0, 0]);
    assert_eq!(alternate.counts(), [1, 0]);
}

#[test]
fn test_507_content_real_reload_failure_build_counts() {
    std::fs::create_dir_all("target").unwrap();
    let tmp = tempfile::tempdir_in(std::fs::canonicalize("target").unwrap()).unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("seed.rs"), "fn reload_failure_marker() {}\n").unwrap();
    let root = crate::clean_path(&std::fs::canonicalize(&workspace).unwrap().to_string_lossy());
    let index_base = tmp.path().join("indexes");
    let probe = crate::index::build_counts::Probe::new(&root);
    probe.remove_next_saved_content();
    let (index, action) = super::build_or_load_content_index(
        &root, " RS ,rs ", &index_base, false, true,
    ).unwrap();
    assert_eq!(action, "rebuilt");
    assert_eq!(index.extensions, ["rs", "rs"]);
    assert!(index.index.contains_key("reload_failure_marker"));
    assert_eq!(probe.counts(), [2, 0]);
    assert!(crate::index::load_content_index(&root, "rs,rs", &index_base).is_err());
}

#[test]
fn test_507_content_empty_segments_remain_invalid() {
    std::fs::create_dir_all("target").unwrap();
    let tmp = tempfile::tempdir_in(std::fs::canonicalize("target").unwrap()).unwrap();
    let root = crate::clean_path(&tmp.path().to_string_lossy());
    let probe = crate::index::build_counts::Probe::new(&root);
    for (iteration, ext) in ["RS,", ",rs", " rs , \t ,JSON"].iter().enumerate() {
        assert!(super::build_or_load_content_index(
            &root, ext, &tmp.path().join("indexes"), false, true,
        ).is_err());
        assert_eq!(probe.counts(), [iteration + 1, 0]);
    }
}

fn content_507_build_matrix(seed_cache: bool) {
    for ext in ["RS", " rs ", " Rs , JSON ,rS "] {
        for populated in [false, true] {
            std::fs::create_dir_all("target").unwrap();
            let tmp = tempfile::tempdir_in(std::fs::canonicalize("target").unwrap()).unwrap();
            let workspace = tmp.path().join("workspace");
            std::fs::create_dir(&workspace).unwrap();
            if populated {
                std::fs::write(workspace.join("seed.rs"), "fn original_marker() {}\n").unwrap();
                std::fs::write(workspace.join("data.json"), "{\"json_marker\": true}\n").unwrap();
            }
            let root = crate::clean_path(&std::fs::canonicalize(&workspace).unwrap().to_string_lossy());
            let index_base = tmp.path().join("indexes");
            let expected: Vec<String> = ext.split(',').map(|s| s.trim().to_lowercase()).collect();
            let key = expected.join(",");
            if seed_cache {
                let cache = crate::index::build_content_index(&crate::ContentIndexArgs {
                    dir: root.clone(), ext: key.clone(), threads: 1, ..Default::default()
                }).unwrap();
                crate::index::save_content_index(&cache, &index_base).unwrap();
            }
            let probe = crate::index::build_counts::Probe::new(&root);
            if populated && seed_cache {
                std::fs::write(workspace.join("seed.rs"), "fn fresh_marker() {}\n").unwrap();
            }
            let (index, action) = super::build_or_load_content_index(
                &root, ext, &index_base, false, seed_cache,
            ).unwrap();
            assert_eq!(probe.counts(), [1, 0], "ext={ext:?}, populated={populated}, forced={seed_cache}");
            assert_eq!(action, "rebuilt");
            assert_eq!(index.extensions, expected);
            assert_eq!(index.files.len(), if populated && expected.contains(&"json".to_string()) { 2 }
                else { usize::from(populated) });
            if populated {
                let marker = if seed_cache { "fresh_marker" } else { "original_marker" };
                assert!(index.index.contains_key(marker));
                if seed_cache { assert!(!index.index.contains_key("original_marker")); }
            }
            let disk = crate::index::load_content_index(&root, &key, &index_base).unwrap();
            assert_eq!(disk.extensions, expected);
            assert_eq!(serde_json::to_value(&disk.index).unwrap(), serde_json::to_value(&index.index).unwrap());
            let (cached, action) = super::build_or_load_content_index(
                &root, ext, &index_base, false, false,
            ).unwrap();
            assert_eq!(action, "loaded_cache");
            assert_eq!(cached.extensions, expected);
            assert_eq!(serde_json::to_value(&cached.index).unwrap(), serde_json::to_value(&index.index).unwrap());
            assert_eq!(probe.counts(), [1, 0], "cache reuse must not build");
        }
    }
}

/// `build_or_load_content_index(force_rebuild=false)` must still serve the
/// existing on-disk cache. Pins the workspace-switch fast path semantics so a
/// future refactor can't accidentally make every cross-load do a full rebuild.
#[test]
fn test_build_or_load_content_index_force_false_uses_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace_keep_cache");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("seed.rs"), "fn keep_cache_seed() {}\n").unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );
    let index_base = tmp.path().join("indexes");
    let cache = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: workspace_str.clone(),
        ext: "rs".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    crate::index::save_content_index(&cache, &index_base).unwrap();

    let (_idx, action) = super::build_or_load_content_index(
        &workspace_str, "rs", &index_base, false, /* force_rebuild = */ false,
    ).expect("cached index should load");
    assert_eq!(action, "loaded_cache",
        "force_rebuild=false must keep the cache fast path");
}

/// `build_or_load_content_index(force_rebuild=true)` must rebuild even when a
/// matching on-disk cache exists.
#[test]
fn test_build_or_load_content_index_force_true_ignores_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace_force_true");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("seed.rs"), "fn force_true_seed() {}\n").unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );
    let index_base = tmp.path().join("indexes");
    let cache = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: workspace_str.clone(),
        ext: "rs".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    crate::index::save_content_index(&cache, &index_base).unwrap();

    let (_idx, action) = super::build_or_load_content_index(
        &workspace_str, "rs", &index_base, false, /* force_rebuild = */ true,
    ).expect("force-rebuild should still produce an index");
    assert_eq!(action, "rebuilt",
        "force_rebuild=true must bypass the cache fast path");
}

/// Bug-report 2026-04-28: `branch_warning` with `live_branch_probe_enabled=false`
/// (test default) keeps reading the static `current_branch` snapshot — this
/// guarantees that turning the long-running probe off via the flag keeps the
/// pre-fix behaviour for tests and CLI subcommands.
#[test]
fn test_branch_warning_live_probe_disabled_uses_startup_value() {
    let ctx = HandlerContext {
        current_branch: Some("feature/legacy".to_string()),
        ..HandlerContext::default()
    };
    assert!(!ctx.live_branch_probe_enabled,
        "Default ctx must keep the live probe disabled");
    let warning = super::utils::branch_warning(&ctx);
    assert!(warning.is_some(),
        "feature-branch startup value must still surface a warning");
    assert!(warning.unwrap().contains("feature/legacy"));
}

/// Bug-report 2026-04-28: a workspace switch via `handle_xray_reindex_inner`
/// must drop the cached live-branch probe entry for the outgoing workspace,
/// so a subsequent call probing the same dir cannot serve a pre-switch value.
#[test]
fn test_workspace_switch_invalidates_current_branch_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace_a = tmp.path().join("ws_a");
    let workspace_b = tmp.path().join("ws_b");
    for w in [&workspace_a, &workspace_b] {
        std::fs::create_dir_all(w).unwrap();
        std::fs::write(w.join("seed.rs"), "fn ws_switch_seed() {}\n").unwrap();
    }
    let dir_a = crate::clean_path(
        &std::fs::canonicalize(&workspace_a).unwrap().to_string_lossy(),
    );
    let dir_b = crate::clean_path(
        &std::fs::canonicalize(&workspace_b).unwrap().to_string_lossy(),
    );

    let mut ctx = HandlerContext::default();
    seed_disk_cache(&mut ctx, &tmp, &dir_a);
    // Workspace must be switchable for this test (default is PinnedCli).
    ctx.workspace.write().unwrap().mode = WorkspaceBindingMode::ManualOverride;

    // Pre-populate cache for the outgoing workspace so we can observe the
    // explicit removal that `handle_xray_reindex_inner` performs on switch.
    {
        let mut cache = ctx.current_branch_cache.write().unwrap();
        cache.insert(
            dir_a.clone(),
            (std::time::Instant::now(), Some("feature/should_be_evicted".to_string())),
        );
    }
    assert!(
        ctx.current_branch_cache.read().unwrap().contains_key(&dir_a),
        "Precondition: cache must contain the outgoing-workspace entry"
    );

    // Build a cache on disk for ws_b too so the switch's cross-load succeeds.
    let cache_b = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: dir_b.clone(),
        ext: "rs".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    crate::index::save_content_index(&cache_b, &ctx.index_base).unwrap();

    let result = handle_xray_reindex_inner(&ctx, &json!({"dir": dir_b}));
    assert!(!result.is_error, "workspace switch must succeed: {}", result.content[0].text);

    let cache = ctx.current_branch_cache.read().unwrap();
    assert!(
        !cache.contains_key(&dir_a),
        "Workspace switch must drop the outgoing-workspace branch cache entry"
    );
}

/// Bug-report 2026-04-28 (reviewer follow-up): when `save_content_index` fails
/// for a force-rebuilt index, the function MUST return the freshly-built
/// in-memory index instead of re-loading the surviving (stale) on-disk cache.
/// We force the failure by pointing `index_base` at a regular file — the
/// `create_dir_all(index_base)` at the top of `save_content_index` then errors
/// because the path component already exists as a non-directory.
#[test]
fn test_build_or_load_content_index_save_failure_keeps_fresh_in_memory_index() {
    std::fs::create_dir_all("target").unwrap();
    let tmp = tempfile::tempdir_in(std::fs::canonicalize("target").unwrap()).unwrap();
    let workspace = tmp.path().join("workspace_save_fail");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("seed.rs"),
        "fn save_fail_uniq_zzqq_token() {}\n",
    ).unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );

    // Point index_base at a regular FILE — save_content_index's create_dir_all
    // will fail with ErrorKind::NotADirectory (Linux) / similar on Windows.
    let index_base_file = tmp.path().join("not_a_dir.dat");
    std::fs::write(&index_base_file, b"placeholder").unwrap();
    let probe = crate::index::build_counts::Probe::new(&workspace_str);

    let (idx, action) = super::build_or_load_content_index(
        &workspace_str,
        " RS ",
        &index_base_file,
        false,
        /* force_rebuild = */ true,
    ).expect("force-rebuild must yield an in-memory index even when save fails");

    assert_eq!(
        action, "rebuilt",
        "save failure must NOT downgrade indexAction to loaded_cache"
    );
    assert_eq!(idx.extensions, ["rs"]);
    let tokens: Vec<&String> = idx.index.keys().collect();
    assert!(
        tokens.iter().any(|t| t.contains("save_fail_uniq_zzqq_token")),
        "in-memory idx must contain freshly-built tokens despite save failure; matching={:?}",
        tokens.iter().filter(|t| t.contains("save_fail")).collect::<Vec<_>>()
    );
    assert_eq!(probe.counts(), [1, 0]);
}

/// Bug-report 2026-04-28 (reviewer Round-2 finding): the same save-failure
/// stale-cache bug existed in `handle_xray_reindex_definitions_inner` (the
/// `xray_reindex_definitions` handler). Mirror the content-side fix: when
/// `save_definition_index` fails, keep the freshly-built `new_index` in
/// memory and skip the drop+reload — otherwise the handler would silently
/// re-publish stale definitions while reporting fresh counts to the caller.
#[test]
fn test_handle_xray_reindex_definitions_inner_save_failure_keeps_fresh_in_memory_index() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws_def_save_fail");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("seed.rs"),
        "fn def_save_fail_uniq_zzqq_marker() {}\n",
    ).unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );

    let mut ctx = make_ctx_with_defs();
    ctx.server_ext = "rs".to_string();
    ctx.def_extensions = vec!["rs".to_string()];
    // Force save_definition_index to fail by pointing index_base at a regular
    // file: the `create_dir_all(index_base)` inside the save layer will error
    // because the path component already exists as a non-directory.
    let index_base_file = tmp.path().join("not_a_dir.dat");
    std::fs::write(&index_base_file, b"placeholder").unwrap();
    ctx.index_base = index_base_file;
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(workspace_str.clone());
        ws.mode = WorkspaceBindingMode::ManualOverride;
    }

    let result = handle_xray_reindex_definitions_inner(&ctx, &json!({}));
    assert!(
        !result.is_error,
        "definition reindex must succeed even when save fails: {}",
        result.content[0].text
    );

    // Verify the in-memory definition index has the fresh symbol — not the
    // stale entries baked into make_ctx_with_defs() (ResilientClient etc.).
    let def_arc = ctx.def_index.as_ref().expect("ctx must have def_index_arc").clone();
    let def_idx = def_arc.read().unwrap();
    assert!(
        def_idx.definitions.iter().any(|d| d.name.contains("def_save_fail_uniq_zzqq_marker")),
        "in-memory def index must contain freshly-built symbol despite save failure; got {} definitions",
        def_idx.definitions.len()
    );
}


/// Save-failure smoke test (2026-05-05) for `cross_load_definition_index`'s
/// background-build path. Forces `save_definition_index` to fail by pointing
/// `index_base` at a regular file (so `create_dir_all(index_base)` inside the
/// save layer errors), then waits for the BG build to publish via `def_ready`
/// and asserts the in-memory index has the freshly-built marker.
///
/// **Documentary, NOT mutation-killing.** This test cannot distinguish the
/// FIXED path (`if save_failed { new_idx } else { drop+reload }`) from the
/// inverted-conditional mutation: when `index_base` is a regular file the
/// drop+reload's `load_definition_index` ALSO fails and the rebuild-fallback
/// rescues correctness with fresh data either way. A stale-shard
/// mutation-killer (pre-seed shard → read-only → force save fail → expect
/// drop+reload to read stale) cannot work here either, because the cache
/// check at the top of `cross_load_definition_index` and the drop+reload at
/// the bottom both call `load_definition_index` — if a pre-seeded shard is
/// readable the cache check short-circuits to `"loaded_cache"` before the
/// BG build runs, so we never reach the save-failed branch under test. The
/// mutation-killing variant lives in
/// `test_handle_xray_reindex_definitions_inner_save_failure_does_not_publish_stale_shard`,
/// which exercises a sibling path (`xray_reindex_definitions`) that always
/// rebuilds and therefore can be tested with a stale-shard fixture. This
/// test catches grosser regressions only: removed save call, missing
/// fallback, or BG build never publishing on save failure.
#[test]
#[cfg(feature = "lang-rust")]
fn test_cross_load_definition_index_save_failure_keeps_fresh_in_memory_index() {
    use std::sync::atomic::Ordering;

    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws_cross_load_save_fail");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("seed.rs"),
        "fn cross_load_save_fail_uniq_zzqq_marker() {}\n",
    ).unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );

    let mut ctx = make_ctx_with_defs();
    ctx.def_extensions = vec!["rs".to_string()];
    ctx.def_index
        .as_ref()
        .unwrap()
        .write()
        .unwrap()
        .definition_generation = 73;
    // Force save_definition_index to fail by pointing index_base at a regular
    // file: the `create_dir_all(index_base)` inside the save layer will error
    // because the path component already exists as a non-directory.
    let index_base_file = tmp.path().join("not_a_dir.dat");
    std::fs::write(&index_base_file, b"placeholder").unwrap();
    ctx.index_base = index_base_file;
    ctx.workspace.write().unwrap().set_dir(workspace_str.clone());
    // Reset def_ready so we can poll it for background completion below.
    ctx.def_ready.store(false, Ordering::Release);

    let action = cross_load_definition_index(&ctx, &workspace_str);
    assert_eq!(
        action,
        Some("background_build"),
        "no cache exists — must spawn background build, not load_cache"
    );

    // Wait for background build to publish to the def_index Arc.
    assert!(
        wait_until(|| ctx.def_ready.load(Ordering::Acquire)),
        "background build did not signal def_ready within timeout"
    );

    // Verify the in-memory def index has the fresh symbol — not the stale
    // entries baked into make_ctx_with_defs() (ResilientClient etc.).
    let def_arc = ctx.def_index.as_ref().expect("ctx must have def_index_arc").clone();
    let def_idx = def_arc.read().unwrap();
    assert_eq!(def_idx.definition_generation, 74);
    assert!(
        def_idx.definitions.iter().any(|d| d.name.contains("cross_load_save_fail_uniq_zzqq_marker")),
        "cross-loaded def index must contain freshly-built symbol despite save failure; got {} definitions",
        def_idx.definitions.len()
    );
}

/// Bug-report 2026-04-28 (reviewer Round-3 stricter coverage): the
/// previous save-failure tests only prove fresh data ends up in memory —
/// they do not distinguish the BUGGY drop+reload+rebuild fallback path
/// (which would also publish fresh data when load also fails) from the
/// FIXED path that returns the in-memory new_index directly. This test
/// explicitly distinguishes them by:
///   1. seeding a readable stale on-disk shard with a unique stale-only marker
///   2. mutating the workspace so the stale marker no longer exists
///   3. making every shard file read-only so save_sharded's
///      rename(temp, target) fails with ACCESS_DENIED while load_definition_index
///      can still open the (read-only but readable) stale shard
///   4. asserting the published index contains the FRESH marker AND does NOT
///      contain the stale-only marker.
///
/// Pre-fix code would publish the stale-only marker via the load fallback;
/// post-fix code keeps the in-memory fresh new_index.
///
/// Windows-only: POSIX rename over a read-only target succeeds when the
/// parent directory is writable, so this trick does not apply on Linux/macOS.
#[cfg(windows)]
#[test]
fn test_handle_xray_reindex_definitions_inner_save_failure_does_not_publish_stale_shard() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws_def_stale_pin");
    std::fs::create_dir_all(&workspace).unwrap();
    let seed_path = workspace.join("seed.rs");
    std::fs::write(&seed_path, "fn stale_only_zzqq_marker() {}\n").unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );
    let index_base = tmp.path().join("indexes_stale_pin");

    // 1. Build & save STALE def index containing the unique stale-only marker.
    let stale_idx = crate::definitions::build_definition_index(
        &crate::definitions::DefIndexArgs {
            dir: workspace_str.clone(),
            ext: "rs".to_string(),
            threads: 1,
            respect_git_exclude: false,
        },
    );
    crate::definitions::save_definition_index(&stale_idx, &index_base)
        .expect("seeding stale shard must succeed");
    drop(stale_idx);

    // 2. Rewrite the workspace so the stale marker is GONE and the fresh marker
    //    is the only symbol. A correct rebuild MUST surface fresh_zzqq_marker
    //    and MUST NOT surface stale_only_zzqq_marker.
    std::fs::write(&seed_path, "fn fresh_zzqq_marker() {}\n").unwrap();

    // 3. Make every shard file read-only so save_sharded's atomic-rename fails
    //    on Windows. Load can still read the (read-only) stale shard.
    fn set_readonly_recursive(path: &std::path::Path) {
        if path.is_file() {
            let mut perm = std::fs::metadata(path).unwrap().permissions();
            perm.set_readonly(true);
            std::fs::set_permissions(path, perm).unwrap();
        } else if path.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                set_readonly_recursive(&entry.unwrap().path());
            }
        }
    }
    set_readonly_recursive(&index_base);

    // 4. Reindex via the handler.
    let mut ctx = make_ctx_with_defs();
    ctx.server_ext = "rs".to_string();
    ctx.def_extensions = vec!["rs".to_string()];
    ctx.index_base = index_base.clone();
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(workspace_str.clone());
        ws.mode = WorkspaceBindingMode::ManualOverride;
    }

    let result = handle_xray_reindex_definitions_inner(&ctx, &json!({}));

    // Restore writable so tempdir cleanup succeeds regardless of test outcome.
    // The `permissions_set_readonly_false` lint warns that `set_readonly(false)`
    // doesn't fully restore Unix permissions; here we only need write-back
    // for tempdir teardown on Windows, so the lint is intentional.
    #[allow(clippy::permissions_set_readonly_false)]
    fn unset_readonly_recursive(path: &std::path::Path) {
        if path.is_file() {
            if let Ok(meta) = std::fs::metadata(path) {
                let mut perm = meta.permissions();
                perm.set_readonly(false);
                let _ = std::fs::set_permissions(path, perm);
            }
        } else if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                unset_readonly_recursive(&entry.path());
            }
        }
    }
    unset_readonly_recursive(&index_base);

    assert!(
        !result.is_error,
        "definition reindex must succeed even when save fails: {}",
        result.content[0].text
    );

    // 5. Assert fresh marker present AND stale-only marker absent.
    let def_arc = ctx.def_index.as_ref().expect("ctx must have def_index_arc").clone();
    let def_idx = def_arc.read().unwrap();
    let names: Vec<&str> = def_idx.definitions.iter().map(|d| d.name.as_str()).collect();
    assert!(
        names.iter().any(|n| n.contains("fresh_zzqq_marker")),
        "in-memory def index must contain freshly-built symbol; got {:?}",
        names
    );
    assert!(
        !names.iter().any(|n| n.contains("stale_only_zzqq_marker")),
        "in-memory def index must NOT contain the stale-only symbol that exists only on the read-only shard; got {:?}",
        names
    );
}

#[cfg(feature = "lang-csharp")]
#[test]
fn test_effective_definition_build_extensions_uses_first_parser_fallback() {
    let mut ctx = make_ctx_with_defs();
    ctx.def_extensions.clear();

    assert_eq!(
        super::effective_definition_build_extensions(&ctx),
        vec!["cs".to_string()]
    );
}

#[cfg(feature = "lang-rust")]
#[test]
fn test_handle_xray_reindex_definitions_inner_omitted_ext_defaults_to_def_extensions() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws_def_extension_default");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("lib.rs"), "fn definition_extension_marker() {}\n").unwrap();
    std::fs::write(workspace.join("model.cs"), "class ContentOnlyExtensionMarker {}\n").unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );

    let mut ctx = make_ctx_with_defs();
    ctx.server_ext = "rs,cs".to_string();
    ctx.def_extensions = vec!["rs".to_string()];
    ctx.def_index
        .as_ref()
        .unwrap()
        .write()
        .unwrap()
        .definition_generation = 41;
    ctx.index_base = tmp.path().join("indexes");
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(workspace_str);
        ws.mode = WorkspaceBindingMode::ManualOverride;
    }

    let result = handle_xray_reindex_definitions_inner(&ctx, &json!({}));
    assert!(!result.is_error, "{}", result.content[0].text);

    let definition_index = ctx.def_index.as_ref().unwrap().read().unwrap();
    assert_eq!(definition_index.definition_generation, 42);
    assert_eq!(definition_index.extensions, vec!["rs".to_string()]);
    assert!(definition_index.definitions.iter().any(|definition| {
        definition.name == "definition_extension_marker"
    }));
    assert!(!definition_index.definitions.iter().any(|definition| {
        definition.name == "ContentOnlyExtensionMarker"
    }));
}

#[cfg(feature = "lang-csharp")]
#[test]
fn test_handle_xray_reindex_definitions_inner_empty_def_extensions_uses_parser_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws_def_extension_fallback");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("model.cs"), "class ParserFallbackMarker {}\n").unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );

    let mut ctx = make_ctx_with_defs();
    ctx.server_ext = "json".to_string();
    ctx.def_extensions.clear();
    ctx.index_base = tmp.path().join("indexes");
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(workspace_str);
        ws.mode = WorkspaceBindingMode::ManualOverride;
    }

    let result = handle_xray_reindex_definitions_inner(&ctx, &json!({}));
    assert!(!result.is_error, "{}", result.content[0].text);

    let definition_index = ctx.def_index.as_ref().unwrap().read().unwrap();
    assert!(definition_index.extensions.iter().any(|extension| extension == "cs"));
    assert!(definition_index.definitions.iter().any(|definition| {
        definition.name == "ParserFallbackMarker"
    }));
}






// ── P0-1 follow-up: WorkspaceSwitchPanicGuard rolls back on panic ─────────

/// On panic during a workspace-switch reindex, the RAII guard MUST restore
/// dir/mode/generation/status — otherwise the server is stuck in
/// `Reindexing` (and pinned to a half-built dir) for the rest of its life.
/// Reproduces the failure mode flagged by the second review round on
/// PR P0-1a, where the dispatcher panic guard caught the panic but the
/// reindex inner had already advanced workspace state past the rollback
/// branches.
#[test]
fn test_workspace_switch_panic_guard_rolls_back_on_unwind() {
    let ctx = HandlerContext::default();

    let original_dir = "/good/workspace".to_string();
    let original_mode = WorkspaceBindingMode::ManualOverride;
    let original_generation: u64 = 7;
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(original_dir.clone());
        ws.mode = original_mode;
        ws.generation = original_generation;
        ws.status = WorkspaceStatus::Resolved;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Simulate the production reindex flow: flip workspace into the
        // mid-flight state, install the guard, then panic.
        {
            let mut ws = ctx.workspace.write().unwrap();
            ws.set_dir("/bad/half_built".to_string());
            ws.mode = WorkspaceBindingMode::DotBootstrap;
            ws.generation = 999;
            ws.status = WorkspaceStatus::Reindexing;
        }
        let _guard = WorkspaceSwitchPanicGuard::new(
            &ctx,
            original_dir.clone(),
            original_mode,
            WorkspaceStatus::Resolved,
            original_generation,
            true,
        );
        panic!("simulated handler panic mid-reindex");
    }));
    assert!(result.is_err(), "panic should have been caught");

    let ws = ctx.workspace.read().unwrap();
    assert_eq!(ws.dir, original_dir,
        "panic-rollback must restore dir to the pre-switch value");
    assert_eq!(ws.mode, original_mode,
        "panic-rollback must restore mode");
    assert_eq!(ws.generation, original_generation,
        "panic-rollback must restore generation");
    assert_eq!(ws.status, WorkspaceStatus::Resolved,
        "panic-rollback must force status=Resolved so subsequent tools are not blocked");
}

/// The guard MUST NOT touch workspace state on normal scope-exit. This
/// pins the `std::thread::panicking()` check inside Drop: regressing it
/// (e.g. by removing the check) would silently undo every successful
/// reindex.
#[test]
fn test_workspace_switch_panic_guard_no_op_on_normal_drop() {
    let ctx = HandlerContext::default();

    let original_dir = "/good/workspace".to_string();
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(original_dir.clone());
        ws.mode = WorkspaceBindingMode::ManualOverride;
        ws.generation = 5;
        ws.status = WorkspaceStatus::Resolved;
    }

    let new_dir = "/new/workspace".to_string();
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(new_dir.clone());
        ws.mode = WorkspaceBindingMode::ManualOverride;
        ws.generation = 6;
        ws.status = WorkspaceStatus::Resolved;
    }

    {
        // Guard goes out of scope normally; Drop runs but must NOT rollback.
        let _guard = WorkspaceSwitchPanicGuard::new(
            &ctx,
            original_dir.clone(),
            WorkspaceBindingMode::ManualOverride,
            WorkspaceStatus::Resolved,
            5,
            true,
        );
    }

    let ws = ctx.workspace.read().unwrap();
    assert_eq!(ws.dir, new_dir,
        "normal drop must NOT rollback dir (would undo a successful switch)");
    assert_eq!(ws.generation, 6,
        "normal drop must NOT rollback generation");
    assert_eq!(ws.status, WorkspaceStatus::Resolved,
        "normal drop must NOT touch status");
}

/// After dismiss() the guard MUST behave as a no-op on Drop, even when
/// unwinding. Pins the post-commit safety contract: if the new index has
/// already been published and ws.status = Resolved, a panic in
/// best-effort post-commit work (cross-load defs, watcher restart, git
/// cache) must NOT roll the workspace back into a state incoherent with
/// the published index.
#[test]
fn test_workspace_switch_panic_guard_dismiss_blocks_rollback_on_panic() {
    let ctx = HandlerContext::default();

    let original_dir = "/good/workspace".to_string();
    let original_mode = WorkspaceBindingMode::ManualOverride;
    let original_generation: u64 = 7;
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(original_dir.clone());
        ws.mode = original_mode;
        ws.generation = original_generation;
        ws.status = WorkspaceStatus::Resolved;
    }

    let new_dir = "/new/workspace".to_string();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Simulate the production flow: switch workspace, set up the guard,
        // reach the commit point, dismiss, THEN something panics in the
        // post-commit phase.
        {
            let mut ws = ctx.workspace.write().unwrap();
            ws.set_dir(new_dir.clone());
            ws.mode = WorkspaceBindingMode::ManualOverride;
            ws.generation = original_generation + 1;
            ws.status = WorkspaceStatus::Resolved; // commit point
        }
        let mut guard = WorkspaceSwitchPanicGuard::new(
            &ctx,
            original_dir.clone(),
            original_mode,
            WorkspaceStatus::Resolved,
            original_generation,
            true,
        );
        guard.dismiss();
        panic!("simulated panic in post-commit work (e.g. watcher restart)");
    }));
    assert!(result.is_err(), "panic should have been caught");

    let ws = ctx.workspace.read().unwrap();
    assert_eq!(ws.dir, new_dir,
        "after dismiss(), panic must NOT roll dir back to the previous workspace");
    assert_eq!(ws.generation, original_generation + 1,
        "after dismiss(), generation must stay at the new value");
    assert_eq!(ws.status, WorkspaceStatus::Resolved,
        "after dismiss(), status must remain Resolved");
}

#[test]
fn test_workspace_switch_panic_guard_preserves_unresolved_and_reindexing_status() {
    let temp = tempfile::tempdir().unwrap();
    let original_dir = temp.path().join("missing").to_string_lossy().into_owned();
    for (old_mode, old_status, old_generation) in [
        (WorkspaceBindingMode::Unresolved, WorkspaceStatus::Unresolved, 0),
        (WorkspaceBindingMode::ClientRoots, WorkspaceStatus::Reindexing, 1),
    ] {
        for workspace_changed in [false, true] {
            let ctx = HandlerContext::default();
            {
                let mut ws = ctx.workspace.write().unwrap();
                ws.set_dir(original_dir.clone());
                ws.mode = old_mode;
                ws.status = old_status;
                ws.generation = old_generation;
            }

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                {
                    let mut ws = ctx.workspace.write().unwrap();
                    if workspace_changed {
                        ws.set_dir(temp.path().join("other").to_string_lossy().into_owned());
                        ws.mode = WorkspaceBindingMode::ManualOverride;
                        ws.generation += 1;
                    }
                    ws.status = WorkspaceStatus::Reindexing;
                }
                let _guard = WorkspaceSwitchPanicGuard::new(
                    &ctx, original_dir.clone(), old_mode, old_status, old_generation, workspace_changed,
                );
                panic!("simulated reindex panic");
            }));

            assert!(result.is_err());
            let ws = ctx.workspace.read().unwrap();
            assert_eq!(ws.dir, original_dir);
            assert_eq!(ws.mode, old_mode);
            assert_eq!(ws.status, old_status);
            assert_eq!(ws.generation, old_generation);
        }
    }
}

#[test]
fn test_definition_reindex_publish_failure_preserves_unresolved_workspace() {
    for workspace_changed in [false, true] {
        let (temp, ctx, roots) = definition_switch_fixture();
        let missing = temp.path().join("missing").to_string_lossy().into_owned();
        *ctx.workspace.write().unwrap() = super::WorkspaceBinding::unresolved(missing.clone());
        ctx.content_ready.store(false, std::sync::atomic::Ordering::Release);
        ctx.def_ready.store(false, std::sync::atomic::Ordering::Release);
        let def_index = ctx.def_index.as_ref().unwrap();
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _lock = def_index.write().unwrap();
            panic!("poison the definition index before publication");
        }));
        assert!(poisoned.is_err());
        let args = if workspace_changed {
            json!({"dir": roots[1], "ext": ["rs"]})
        } else {
            json!({"ext": ["rs"]})
        };

        let result = handle_xray_reindex_definitions_inner(&ctx, &args);

        assert!(result.is_error);
        let ws = ctx.workspace.read().unwrap();
        assert_eq!(ws.dir, missing);
        assert_eq!(ws.mode, WorkspaceBindingMode::Unresolved);
        assert_eq!(ws.status, WorkspaceStatus::Unresolved);
        assert_eq!(ws.generation, 0);
        drop(ws);
        let probe = super::dispatch_tool(&ctx, "xray_grep", &json!({"terms": ["test"]}));
        assert!(probe.is_error);
        assert!(probe.content[0].text.contains("WORKSPACE_UNRESOLVED"), "{:?}", probe);
        assert!(!ctx.content_ready.load(std::sync::atomic::Ordering::Acquire));
        assert!(!ctx.def_ready.load(std::sync::atomic::Ordering::Acquire));
    }
}
