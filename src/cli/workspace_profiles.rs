mod onboarding;

use std::{collections::VecDeque, ffi::OsString, fs, io::{self, BufRead, Read, Write}, path::{Path, PathBuf}};

use clap::Parser;
use serde::Deserialize;
use serde_json::{Value, json};

use super::args::ServeArgs;
use super::shared_backend::InitializedSession;

const ROOTS_REQUEST_ID: &str = "xray/workspace-profile-roots";
const MAX_BOOTSTRAP_BYTES: usize = 1024 * 1024;

#[derive(Parser, Debug)]
pub(crate) struct WorkspaceServeArgs {
    /// Directory containing locally installed <workspace-id>/config.json profiles.
    #[arg(long)]
    profiles: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Profile {
    version: u32,
    workspace_root: PathBuf,
    args: Vec<String>,
}

pub(super) fn run(args: WorkspaceServeArgs) -> io::Result<()> {
    let cwd = std::env::current_dir()?;
    match resolve(&args.profiles, &cwd) {
        Ok(serve) => super::shared_backend::run(serve),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let selected = bootstrap(
                &args.profiles, &cwd, error, &mut io::stdin().lock(), &mut io::stdout().lock(),
            )?;
            match selected {
                Some((serve, session)) => super::shared_backend::run_with_session(serve, Some(session)),
                None => Ok(()),
            }
        }
        Err(error) => Err(error),
    }
}

fn read_message(reader: &mut impl BufRead, remaining: &mut usize) -> io::Result<(String, Value)> {
    let mut line = String::new();
    if reader.by_ref().take(*remaining as u64 + 1).read_line(&mut line)? == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "MCP client disconnected during workspace discovery"));
    }
    if line.len() > *remaining {
        return Err(invalid("MCP workspace discovery exceeded its 1 MiB input limit"));
    }
    *remaining -= line.len();
    let value = serde_json::from_str(&line).map_err(|error| invalid(format!("Invalid MCP workspace discovery message: {error}")))?;
    if !line.ends_with('\n') { line.push('\n'); }
    Ok((line, value))
}

fn send_message(writer: &mut impl Write, value: Value) -> io::Result<()> {
    writeln!(writer, "{value}")?;
    writer.flush()
}

fn bootstrap(
    profiles: &Path, cwd: &Path, cwd_error: io::Error, reader: &mut impl BufRead, writer: &mut impl Write,
) -> io::Result<Option<(ServeArgs, InitializedSession)>> {
    let mut remaining = MAX_BOOTSTRAP_BYTES;
    let (initialize, request) = loop {
        let (line, request) = match read_message(reader, &mut remaining) {
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Err(cwd_error),
            result => result?,
        };
        let id = request.get("id").filter(|id| !id.is_null());
        if request["jsonrpc"] != "2.0" || !request["method"].is_string() {
            send_message(writer, json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":"Invalid JSON-RPC request"}}))?;
            continue;
        }
        if request["method"] == "initialize" {
            if id.is_none() { return Err(invalid("Expected MCP initialize with a request ID before workspace discovery")); }
            break (line, request);
        }
        if let Some(id) = id {
            // Clients may probe optional methods such as server/discover before initialize.
            let response = if request["method"] == "ping" {
                json!({"jsonrpc":"2.0","id":id,"result":{}})
            } else {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found before MCP initialization"}})
            };
            send_message(writer, response)?;
        }
    };
    let has_roots = request.pointer("/params/capabilities/roots").is_some_and(Value::is_object);
    let cwd_root = if has_roots { None } else {
        Some(onboarding::git_root(cwd).ok_or_else(|| io::Error::new(cwd_error.kind(), format!(
            "{cwd_error} This client does not advertise MCP roots; launch it from a Git workspace or register a workspace with the installer."
        )))?)
    };
    let mut result = crate::mcp::protocol::InitializeResult::new_with_extensions_and_xml_on_demand(&[], &[], false);
    // Workspace-specific language guidance is not known until roots/list completes.
    result.instructions = None;
    result.capabilities.tools.list_changed = true;
    send_message(writer, json!({"jsonrpc": "2.0", "id": request["id"], "result": result}))?;
    let updates = crate::mcp::updates::Session::from_env(crate::index_dir());
    let mut buffered = Vec::new();
    loop {
        let (line, value) = read_message(reader, &mut remaining)?;
        if value["method"] == "notifications/initialized" { updates.start(); break; }
        if value["method"] == "ping" && value.get("id").is_some() {
            send_message(writer, json!({"jsonrpc": "2.0", "id": value["id"], "result": {}}))?;
        } else {
            buffered.extend_from_slice(line.as_bytes());
        }
    }
    if let Some(root) = cwd_root {
        return select_or_onboard(profiles, &root, InitializedSession { initialize, buffered, updates }, reader, writer);
    }
    send_message(writer, json!({"jsonrpc": "2.0", "id": ROOTS_REQUEST_ID, "method": "roots/list", "params": {}}))?;
    loop {
        let (line, value) = read_message(reader, &mut remaining)?;
        if value["id"] != ROOTS_REQUEST_ID || value.get("method").is_some() {
            buffered.extend_from_slice(line.as_bytes());
            continue;
        }
        if let Some(error) = value.get("error") {
            return Err(invalid(format!("MCP roots/list failed during workspace profile selection: {error}")));
        }
        let roots = value.pointer("/result/roots").and_then(Value::as_array)
            .ok_or_else(|| invalid("MCP roots/list did not return a roots array"))?;
        if roots.len() != 1 {
            return Err(invalid("Workspace profile discovery requires exactly one MCP root; launch the session from the intended registered workspace for multi-root windows"));
        }
        let path = roots[0]["uri"].as_str().and_then(crate::mcp::server::uri_to_path)
            .ok_or_else(|| invalid("MCP workspace root must be a valid file URI"))?;
        return select_or_onboard(profiles, Path::new(&path), InitializedSession { initialize, buffered, updates }, reader, writer);
    }
}

fn select_or_onboard(
    profiles: &Path, root: &Path, mut session: InitializedSession,
    reader: &mut impl BufRead, writer: &mut impl Write,
) -> io::Result<Option<(ServeArgs, InitializedSession)>> {
    match resolve(profiles, root) {
        Ok(serve) => return Ok(Some((serve, session))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let root = std::path::absolute(root)?;
    if !fs::canonicalize(&root)?.is_dir() { return Err(invalid("MCP workspace root must be a directory")); }
    let mut queued: VecDeque<_> = String::from_utf8(std::mem::take(&mut session.buffered))
        .map_err(|error| invalid(error.to_string()))?.lines()
        .map(|line| Ok((format!("{line}\n"), serde_json::from_str::<Value>(line).map_err(|error| invalid(error.to_string()))?)))
        .collect::<io::Result<_>>()?;
    let mut proposal = None;
    loop {
        let (_, request) = match queued.pop_front() {
            Some(message) => message,
            None => {
                let mut remaining = MAX_BOOTSTRAP_BYTES;
                match read_message(reader, &mut remaining) {
                    Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
                    result => result?,
                }
            }
        };
        if request["method"] == "notifications/roots/list_changed" {
            return Err(invalid("MCP workspace roots changed during setup. Reconnect to request a proposal for the current workspace; no profile was created."));
        }
        let Some(id) = request.get("id").filter(|id| !id.is_null()) else { continue; };
        if request["jsonrpc"] != "2.0" || !request["method"].is_string() {
            send_message(writer, json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":"Invalid JSON-RPC request"}}))?;
            continue;
        }
        let result = match request["method"].as_str() {
            Some("ping") => json!({}),
            Some("tools/list") => {
                let mut tools = crate::mcp::handlers::tool_definitions(&[]);
                tools.push(onboarding::tool());
                json!({"tools":tools})
            }
            Some("tools/call") => {
                let is_setup = request.pointer("/params/name") == Some(&json!("xray_setup"));
                let arguments = request.pointer("/params/arguments").cloned().unwrap_or_else(|| json!({}));
                let outcome = (|| -> io::Result<(Value, Option<ServeArgs>)> {
                    if !is_setup {
                        if proposal.is_none() { proposal = Some(onboarding::Proposal::scan(&root)?); }
                        let mut body = proposal.as_ref().ok_or_else(|| invalid("Workspace proposal is unavailable"))?.body();
                        body["error"] = json!("XRAY_SETUP_REQUIRED");
                        return Ok((serde_json::to_value(crate::mcp::protocol::ToolCallResult::error(body.to_string()))?, None));
                    }
                    let options: onboarding::Options = serde_json::from_value(arguments).map_err(|error| invalid(error.to_string()))?;
                    if !options.confirm {
                        let detected = onboarding::Proposal::scan(&root)?;
                        let body = detected.body();
                        proposal = Some(detected);
                        return Ok((serde_json::to_value(crate::mcp::protocol::ToolCallResult::success(body.to_string()))?, None));
                    }
                    let detected = proposal.as_ref().ok_or_else(|| invalid("Call xray_setup without confirm first, then ask the user for approval."))?;
                    let serve = detected.install(profiles, options)?;
                    let body = json!({
                        "status":"indexing", "workspaceRoot": detected.body()["workspaceRoot"], "extensions": serve.ext,
                        "hint":"Profile created. Please wait while Xray builds the indexes. Poll xray_info until no indexes[] entry has a status field, then retry the original request. A failed status is an error, not readiness; follow its recovery hint. Do not treat partial indexes as complete."
                    });
                    Ok((serde_json::to_value(crate::mcp::protocol::ToolCallResult::success(body.to_string()))?, Some(serve)))
                })();
                match outcome {
                    Ok((result, Some(serve))) => {
                        send_setup_tool_result(writer, id, result, &session.updates, &request)?;
                        send_message(writer, json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"}))?;
                        session.buffered = queued.into_iter().flat_map(|(line, _)| line.into_bytes()).collect();
                        return Ok(Some((serve, session)));
                    }
                    Ok((result, None)) => result,
                    Err(error) => {
                        eprintln!("Xray workspace setup: {error}");
                        serde_json::to_value(crate::mcp::protocol::ToolCallResult::error(error.to_string()))?
                    }
                }
            }
            _ => {
                send_message(writer, json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found during workspace setup"}}))?;
                continue;
            }
        };
        if request["method"] == "tools/call" {
            send_setup_tool_result(writer, id, result, &session.updates, &request)?;
        } else {
            send_message(writer, json!({"jsonrpc":"2.0","id":id,"result":result}))?;
        }
    }
}

fn send_setup_tool_result(
    writer: &mut impl Write, id: &Value, result: Value,
    updates: &crate::mcp::updates::Session, request: &Value,
) -> io::Result<()> {
    let mut result: crate::mcp::protocol::ToolCallResult = serde_json::from_value(result)?;
    if request.pointer("/params/name").and_then(Value::as_str) == Some("xray_info")
        && let Some(mut body) = crate::mcp::handlers::utils::tool_result_output(&result)
        && body.is_object()
        && let Some(content) = result.content.first_mut() {
        body["softwareUpdate"] = updates.snapshot();
        content.text = body.to_string();
    }
    let notice_was_sent = updates.notice_sent.load(std::sync::atomic::Ordering::Acquire);
    let result = match crate::mcp::handlers::response_transport_limit() {
        Ok(limit) => {
            let budget = limit.map(|limit| limit.min(crate::mcp::handlers::utils::DEFAULT_MAX_RESPONSE_BYTES))
                .unwrap_or(crate::mcp::handlers::utils::DEFAULT_MAX_RESPONSE_BYTES);
            let result = crate::mcp::handlers::utils::truncate_response_if_needed(result, budget);
            updates.attach_notice(result, budget)
        }
        Err(_) => {
            tracing::warn!("Xray setup update notice omitted: invalid XRAY_TRANSPORT_MAX_BYTES");
            result
        }
    };
    let mut result = serde_json::to_value(result)?;
    let notice_sent = !notice_was_sent && updates.notice_sent.load(std::sync::atomic::Ordering::Acquire);
    if notice_sent { crate::mcp::updates::add_notice_meta(&mut result); }
    send_message(writer, json!({"jsonrpc":"2.0","id":id,"result":result}))?;
    if notice_sent { crate::mcp::updates::mark_frontend_notice(); }
    Ok(())
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn comparison_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    { PathBuf::from(path.to_string_lossy().to_lowercase()) }
    #[cfg(not(windows))]
    { path.to_path_buf() }
}

fn resolve(profiles: &Path, cwd: &Path) -> io::Result<ServeArgs> {
    let cwd = comparison_path(&fs::canonicalize(cwd)?);
    let mut directories = fs::read_dir(profiles)
        .map_err(|error| io::Error::new(error.kind(), format!(
            "Cannot read Xray workspace profiles at {}: {error}. Run setup-xray.ps1 -UserMcp -RepoPath <workspace>.",
            profiles.display()
        )))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    directories.sort();
    let mut selected: Option<(PathBuf, PathBuf, Profile)> = None;
    let mut matching_roots = std::collections::BTreeMap::new();
    for directory in directories {
        if !directory.is_dir() { continue; }
        let file = directory.join("config.json");
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io::Error::new(error.kind(), format!(
                "Cannot read Xray workspace profile {}: {error}", file.display()
            ))),
        };
        let profile: Profile = serde_json::from_str(&text)
            .map_err(|error| invalid(format!("Invalid Xray workspace profile {}: {error}", file.display())))?;
        if profile.version != 1 || !profile.workspace_root.is_absolute() {
            return Err(invalid(format!(
                "Invalid Xray workspace profile {}: version must be 1 and workspaceRoot must be absolute.",
                file.display()
            )));
        }
        let root = match fs::canonicalize(&profile.workspace_root) {
            Ok(root) => root,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                eprintln!("Xray: workspace profile {} refers to an unavailable directory: {error}", file.display());
                continue;
            }
            Err(error) => return Err(io::Error::new(error.kind(), format!(
                "Cannot resolve workspaceRoot in {}: {error}", file.display()
            ))),
        };
        if !root.is_dir() {
            return Err(invalid(format!("workspaceRoot in {} is not a directory.", file.display())));
        }
        let comparable = comparison_path(&root);
        if !cwd.starts_with(&comparable) { continue; }
        if let Some(previous) = matching_roots.insert(comparable.clone(), file.clone()) {
            return Err(invalid(format!(
                "Ambiguous Xray profiles for one workspace: {} and {}. Remove the duplicate profile.",
                previous.display(), file.display()
            )));
        }
        if let Some((_, previous_root, _)) = &selected {
            let previous_root = comparison_path(previous_root);
            if previous_root.components().count() > comparable.components().count() { continue; }
        }
        selected = Some((file, root, profile));
    }
    let (file, root, profile) = selected.ok_or_else(|| io::Error::new(
        io::ErrorKind::NotFound, format!(
            "No Xray profile is installed for {}. Run setup-xray.ps1 -UserMcp -RepoPath <workspace>.",
            cwd.display()
        )
    ))?;
    let argv = [OsString::from("xray serve"), OsString::from("--dir"), root.into_os_string()]
        .into_iter().chain(profile.args.into_iter().map(OsString::from));
    let serve = ServeArgs::try_parse_from(argv)
        .map_err(|error| invalid(format!("Invalid serve arguments in {}: {error}", file.display())))?;
    if serve.internal_backend || serve.standalone {
        return Err(invalid(format!(
            "Profile {} cannot select --internal-backend or --standalone.", file.display()
        )));
    }
    eprintln!("Xray: selected workspace profile {} for {}", file.display(), serve.dir);
    Ok(serve)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(profiles: &Path, id: &str, root: &Path, args: &[&str]) -> PathBuf {
        let directory = profiles.join(id);
        fs::create_dir_all(&directory).unwrap();
        let file = directory.join("config.json");
        fs::write(&file, serde_json::to_vec(&json!({
            "version": 1, "workspaceRoot": root, "args": args,
        })).unwrap()).unwrap();
        file
    }

    #[test]
    fn workspace_profile_update_notice_obeys_transport_budget() {
        let temp = tempfile::tempdir().unwrap();
        let checked_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        fs::write(temp.path().join("update-check.json"), serde_json::to_vec(&json!({
            "schema_version":2, "checked_at":checked_at,
            "outcome":{"status":"available","release":{"tag":"v999.0.0","version":"999.0.0","windows_download":false}}
        })).unwrap()).unwrap();
        let updates = crate::mcp::updates::Session::new(
            crate::mcp::updates::Settings { policy: crate::mcp::updates::Policy::Enabled, notice_sent: false, ..Default::default() },
            temp.path().to_path_buf(),
        );
        updates.start();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while updates.snapshot()["status"] == "checking" && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(updates.snapshot()["status"], "update_available");
        let result = serde_json::to_value(crate::mcp::protocol::ToolCallResult::success(
            json!({"status":"proposal","padding":"x".repeat(300)}).to_string(),
        )).unwrap();
        let request = json!({"params":{"name":"xray_setup"}});
        let mut output = Vec::new();
        crate::mcp::handlers::with_transport_limit(512, || {
            send_setup_tool_result(&mut output, &json!(1), result.clone(), &updates, &request).unwrap();
        });
        let reply: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(reply["result"]["content"].as_array().unwrap().len(), 1);
        assert!(reply["result"]["content"][0]["text"].as_str().unwrap().len() <= 512);
        assert!(!updates.notice_sent.load(std::sync::atomic::Ordering::Acquire));
        output.clear();
        crate::mcp::handlers::with_transport_limit(1024, || {
            send_setup_tool_result(&mut output, &json!(2), result, &updates, &request).unwrap();
        });
        let reply: Value = serde_json::from_slice(&output).unwrap();
        let bytes: usize = reply["result"]["content"].as_array().unwrap().iter()
            .map(|content| content["text"].as_str().unwrap().len()).sum();
        assert!(bytes <= 1024);
        assert_eq!(reply["result"]["content"].as_array().unwrap().len(), 2);
        assert_eq!(reply["result"]["_meta"]["xray/updateNotice"], true);
    }

    #[test]
    fn workspace_profiles_select_settings_from_cwd_and_nested_directories() {
        let temp = tempfile::tempdir().unwrap();
        let profiles = temp.path().join("profiles");
        let a = temp.path().join("repo a");
        let b = temp.path().join("repo b");
        fs::create_dir_all(a.join("nested")).unwrap();
        fs::create_dir_all(&b).unwrap();
        profile(&profiles, "a", &a, &["--ext", "rs,md", "--watch", "--definitions"]);
        profile(&profiles, "b", &b, &["--ext", "ts", "--debounce-ms", "750"]);
        let first = resolve(&profiles, &a).unwrap();
        let nested = resolve(&profiles, &a.join("nested")).unwrap();
        assert_eq!(first, nested);
        assert_eq!(first.ext, ["rs,md"]);
        assert!(first.watch && first.definitions);
        let second = resolve(&profiles, &b).unwrap();
        assert_eq!(second.ext, ["ts"]);
        assert_eq!(second.debounce_ms, 750);
        assert!(!second.watch);
        assert_eq!(fs::canonicalize(first.dir).unwrap(), fs::canonicalize(a).unwrap());
        assert_eq!(fs::canonicalize(second.dir).unwrap(), fs::canonicalize(b).unwrap());
    }

    #[test]
    fn workspace_profiles_choose_nearest_registered_ancestor() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("outer").join("inner");
        fs::create_dir_all(nested.join("src")).unwrap();
        let profiles = temp.path().join("profiles");
        profile(&profiles, "parent", &temp.path().join("outer"), &["--ext", "md"]);
        profile(&profiles, "child", &nested, &["--ext", "rs"]);
        assert_eq!(resolve(&profiles, &nested.join("src")).unwrap().ext, ["rs"]);
    }

    #[test]
    fn workspace_profiles_reject_unknown_roots_and_path_prefix_siblings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let sibling = temp.path().join("repo-other");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        let profiles = temp.path().join("profiles");
        profile(&profiles, "root", &root, &["--ext", "rs"]);
        assert_eq!(resolve(&profiles, &sibling).unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn workspace_profiles_reject_invalid_or_ambiguous_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let profiles = temp.path().join("profiles");
        let file = profile(&profiles, "a", temp.path(), &["--ext", "rs"]);
        profile(&profiles, "b", temp.path(), &["--ext", "ts"]);
        assert!(resolve(&profiles, temp.path()).unwrap_err().to_string().contains("Ambiguous"));
        fs::remove_dir_all(profiles.join("b")).unwrap();
        for invalid_args in [
            vec!["--standalone"], vec!["--internal-backend"], vec!["--not-an-option"],
            vec!["--dir", "."], vec!["--help"],
        ] {
            profile(&profiles, "a", temp.path(), &invalid_args);
            assert_eq!(resolve(&profiles, temp.path()).unwrap_err().kind(), io::ErrorKind::InvalidData);
        }
        fs::write(&file, r#"{"version":2,"workspaceRoot":"relative","args":[]}"#).unwrap();
        assert_eq!(resolve(&profiles, temp.path()).unwrap_err().kind(), io::ErrorKind::InvalidData);
        fs::write(file, "{").unwrap();
        assert_eq!(resolve(&profiles, temp.path()).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn workspace_profiles_ignore_empty_and_unavailable_registrations() {
        let temp = tempfile::tempdir().unwrap();
        let profiles = temp.path().join("profiles");
        profile(&profiles, "valid", temp.path(), &["--ext", "rs"]);
        profile(&profiles, "deleted", &temp.path().join("deleted"), &[]);
        fs::create_dir_all(profiles.join("backup-only")).unwrap();
        fs::write(profiles.join("backup-only/config.json.bak"), "{}").unwrap();
        assert_eq!(resolve(&profiles, temp.path()).unwrap().ext, ["rs"]);
    }

    #[test]
    fn workspace_profiles_reject_duplicate_ancestors_even_with_a_nearer_profile() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let profiles = temp.path().join("profiles");
        profile(&profiles, "a", temp.path(), &[]);
        profile(&profiles, "b", &nested, &[]);
        profile(&profiles, "c", temp.path(), &[]);
        assert!(resolve(&profiles, &nested).unwrap_err().to_string().contains("Ambiguous"));
    }

    fn bootstrap_input(response: Value) -> io::Cursor<Vec<u8>> {
        let messages = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"roots":{}}}}),
            json!({"jsonrpc":"2.0","id":2,"method":"ping"}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
            response,
        ];
        io::Cursor::new(messages.into_iter().map(|value| format!("{value}\n")).collect::<String>().into_bytes())
    }

    #[test]
    fn workspace_profiles_bootstrap_preserves_queued_requests_and_protocol_order() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo with spaces");
        fs::create_dir(&root).unwrap();
        let profiles = temp.path().join("profiles");
        profile(&profiles, "root", &root, &["--ext", "rs"]);
        let mut input = bootstrap_input(json!({"jsonrpc":"2.0","id":ROOTS_REQUEST_ID,"result":{"roots":[{
            "uri":url::Url::from_directory_path(&root).unwrap().as_str(),
        }]}}));
        let mut output = Vec::new();
        let (serve, session) = bootstrap(&profiles, temp.path(), io::ErrorKind::NotFound.into(), &mut input, &mut output).unwrap().unwrap();
        assert_eq!(fs::canonicalize(serve.dir).unwrap(), fs::canonicalize(root).unwrap());
        let messages: Vec<Value> = String::from_utf8(output).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["id"], 1);
        assert!(messages[0]["result"].get("instructions").is_none());
        assert_eq!(messages[1]["id"], 2);
        assert_eq!(messages[2]["method"], "roots/list");
        assert_eq!(serde_json::from_str::<Value>(&session.initialize).unwrap()["method"], "initialize");
        assert_eq!(serde_json::from_slice::<Value>(&session.buffered).unwrap()["method"], "tools/list");
    }

    #[test]
    fn workspace_profiles_bootstrap_refuses_missing_ambiguous_or_invalid_roots() {
        let temp = tempfile::tempdir().unwrap();
        let profiles = temp.path().join("profiles");
        profile(&profiles, "root", temp.path(), &["--ext", "rs"]);
        let root = json!({"uri":url::Url::from_directory_path(temp.path()).unwrap().as_str()});
        for response in [
            json!({"result":{"roots":[]}}),
            json!({"result":{"roots":[root.clone(),root]}}),
            json!({"result":{"roots":[{"uri":"https://example.com/repo"}]}}),
            json!({"result":{}}),
            json!({"error":{"code":-32603,"message":"unavailable"}}),
            json!({"result":{"roots":[{"uri":url::Url::from_directory_path(temp.path().join("missing")).unwrap().as_str()}]}}),
        ] {
            let mut response = response;
            response["id"] = json!(ROOTS_REQUEST_ID);
            let error = bootstrap(&profiles, temp.path(), io::ErrorKind::NotFound.into(), &mut bootstrap_input(response), &mut Vec::new()).err().unwrap();
            assert!(matches!(error.kind(), io::ErrorKind::InvalidData | io::ErrorKind::NotFound));
        }
        let input = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}});
        let mut output = Vec::new();
        let error = bootstrap(&profiles, temp.path(), io::ErrorKind::NotFound.into(), &mut io::Cursor::new(format!("{input}\n")), &mut output).err().unwrap();
        assert!(error.to_string().contains("does not advertise MCP roots"));
        assert!(output.is_empty());
    }

    #[test]
    fn workspace_profiles_bootstrap_handles_optional_probes_before_initialize() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        let profiles = temp.path().join("profiles");
        profile(&profiles, "registered", &root, &["--ext", "rs"]);
        let mut input = format!("{}\n{}\n{}\n",
            json!({"jsonrpc":"2.0","id":"discover","method":"server/discover","params":{}}),
            json!({"jsonrpc":"2.0","id":0,"method":"ping"}),
            json!({"jsonrpc":"2.0","method":"notifications/optional-probe"}),
        ).into_bytes();
        input.extend_from_slice(&bootstrap_input(json!({"jsonrpc":"2.0","id":ROOTS_REQUEST_ID,"result":{"roots":[{
            "uri":url::Url::from_directory_path(&root).unwrap().as_str(),
        }]}})).into_inner());
        let mut output = Vec::new();
        let (serve, session) = bootstrap(&profiles, temp.path(), io::ErrorKind::NotFound.into(),
            &mut io::Cursor::new(input), &mut output).unwrap().unwrap();
        assert_eq!(fs::canonicalize(serve.dir).unwrap(), fs::canonicalize(root).unwrap());
        let messages: Vec<Value> = String::from_utf8(output).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0]["id"], "discover");
        assert_eq!(messages[0]["error"]["code"], -32601);
        assert_eq!(messages[1]["id"], 0);
        assert_eq!(messages[1]["result"], json!({}));
        assert_eq!(messages[2]["id"], 1);
        assert_eq!(messages[4]["method"], "roots/list");
        assert_eq!(serde_json::from_str::<Value>(&session.initialize).unwrap()["method"], "initialize");
        assert_eq!(serde_json::from_slice::<Value>(&session.buffered).unwrap()["method"], "tools/list");
    }

    #[test]
    fn workspace_profiles_bootstrap_rejects_invalid_probe_and_idless_initialize() {
        let temp = tempfile::tempdir().unwrap();
        let mut output = Vec::new();
        let input = format!("{}\n{}\n",
            json!({"jsonrpc":"1.0","id":1,"method":"server/discover"}),
            json!({"jsonrpc":"2.0","method":"initialize"}),
        );
        let error = bootstrap(&temp.path().join("profiles"), temp.path(), io::ErrorKind::NotFound.into(),
            &mut io::Cursor::new(input), &mut output).err().unwrap();
        assert!(error.to_string().contains("request ID"));
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["error"]["code"], -32600);
    }

    #[test]
    fn workspace_profiles_unregistered_empty_root_lists_tools_and_reports_scan_errors() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("empty");
        fs::create_dir(&root).unwrap();
        let profiles = temp.path().join("profiles");
        let response = json!({"jsonrpc":"2.0","id":ROOTS_REQUEST_ID,"result":{"roots":[{
            "uri":url::Url::from_directory_path(&root).unwrap().as_str(),
        }]}});
        let mut input = bootstrap_input(response).into_inner();
        input.extend_from_slice(format!("{}\n{}\n",
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"xray_setup"}}),
            json!({"jsonrpc":"2.0","id":5,"method":"ping"}),
        ).as_bytes());
        let mut output = Vec::new();
        let selected = bootstrap(&profiles, temp.path(), io::ErrorKind::NotFound.into(),
            &mut io::Cursor::new(input), &mut output).unwrap();
        assert!(selected.is_none());
        let messages: Vec<Value> = String::from_utf8(output).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect();
        let list = messages.iter().find(|value| value["id"] == 3).unwrap();
        assert!(list["result"]["tools"].as_array().unwrap().iter().any(|tool| tool["name"] == "xray_setup"));
        let scan = messages.iter().find(|value| value["id"] == 4).unwrap();
        assert_eq!(scan["result"]["isError"], true);
        assert!(scan["result"]["content"][0]["text"].as_str().unwrap().contains("No recognized"));
        assert_eq!(messages.last().unwrap()["id"], 5);
        assert!(!profiles.exists());
    }

    #[test]
    fn workspace_profiles_roots_changed_during_setup_cannot_confirm_old_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        let profiles = temp.path().join("profiles");
        fs::write(root.join("README.md"), "").unwrap();
        let response = json!({"jsonrpc":"2.0","id":ROOTS_REQUEST_ID,"result":{"roots":[{
            "uri":url::Url::from_directory_path(&root).unwrap().as_str(),
        }]}});
        let mut input = bootstrap_input(response).into_inner();
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/roots/list_changed\"}\n");
        let error = bootstrap(&profiles, temp.path(), io::ErrorKind::NotFound.into(),
            &mut io::Cursor::new(input), &mut Vec::new()).err().unwrap();
        assert!(error.to_string().contains("roots changed"));
        assert!(!profiles.exists());
    }

    #[test]
    fn workspace_profiles_bootstrap_bounds_unanswered_discovery_input() {
        let mut remaining = MAX_BOOTSTRAP_BYTES;
        let error = read_message(&mut io::Cursor::new(vec![b'x'; MAX_BOOTSTRAP_BYTES + 1]), &mut remaining).unwrap_err();
        assert!(error.to_string().contains("1 MiB"));
    }
}
