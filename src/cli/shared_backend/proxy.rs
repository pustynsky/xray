use super::{Configuration, InitializedSession, MAX_INPUT_CHUNK, POLL, STARTUP_TIMEOUT, ServeArgs, Stream, Workspace, connect_or_start_until, platform, retryable, send_payload};
use interprocess::local_socket::traits::Stream as _;
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    sync::{Arc, mpsc},
    thread,
    time::Instant,
};

const MAX_REQUEST_BYTES: usize = 10 * 1024 * 1024;

enum Event {
    Input(io::Result<Option<String>>),
    OversizedInput,
    Output(u64, io::Result<Option<String>>),
}

struct Pending {
    id: Value,
    initialization: Option<String>,
}

fn read_client_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = String::new();
    let size = reader.by_ref().take(MAX_REQUEST_BYTES as u64 + 1).read_line(&mut line)?;
    if size == 0 { return Ok(None); }
    if size > MAX_REQUEST_BYTES {
        let mut drained = 0;
        while !line.ends_with('\n') {
            line.clear();
            let size = reader.by_ref().take(8192).read_line(&mut line)?;
            if size == 0 { break; }
            drained += size;
            if drained > 64 * 1024 * 1024 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "Oversized MCP request exceeded drain limit"));
            }
        }
        return Ok(Some(String::new()));
    }
    if !line.ends_with('\n') { line.push('\n'); }
    Ok(Some(line))
}

fn read_backend_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 { return Ok(None); }
    if !line.ends_with('\n') {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Backend response ended before its newline; partial response discarded"));
    }
    Ok(Some(line))
}

fn input_thread(events: mpsc::SyncSender<Event>, ready: mpsc::Receiver<()>, buffered: Vec<u8>) {
    let mut reader = BufReader::new(io::Cursor::new(buffered).chain(io::stdin().lock()));
    loop {
        let result = read_client_line(&mut reader);
        if matches!(&result, Ok(Some(line)) if line.is_empty()) {
            if events.send(Event::OversizedInput).is_err() || ready.recv().is_err() { return; }
            continue;
        }
        let ended = !matches!(&result, Ok(Some(_)));
        if events.send(Event::Input(result)).is_err() || ended || ready.recv().is_err() {
            return;
        }
    }
}

fn output_thread(stream: Arc<Stream>, generation: u64, events: mpsc::SyncSender<Event>) {
    let mut reader = BufReader::new(&*stream);
    loop {
        let result = read_backend_line(&mut reader);
        let ended = !matches!(&result, Ok(Some(_)));
        if events.send(Event::Output(generation, result)).is_err() || ended { return; }
    }
}

fn send_line(stream: &Stream, line: &str) -> io::Result<()> {
    let mut writer = stream;
    for chunk in line.as_bytes().chunks(MAX_INPUT_CHUNK) {
        writer.write_all(&(chunk.len() as u32).to_le_bytes())?;
        writer.write_all(chunk)?;
    }
    Ok(())
}

fn write_output(writer: &mut impl Write, line: &str) -> io::Result<()> {
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

fn fail_pending(pending: &mut Option<Pending>, writer: &mut impl Write) -> io::Result<bool> {
    let Some(request) = pending.take() else { return Ok(false); };
    let response = json!({
        "jsonrpc": "2.0", "id": request.id,
        "error": {"code": -32000, "message": "Workspace backend disconnected before confirming this request. Its outcome may be unknown; the request was NOT replayed. Check the result before retrying an edit."}
    });
    write_output(writer, &format!("{response}\n"))?;
    Ok(true)
}

fn restore_session(stream: &mut Stream, initialize: Option<&str>, initialized: bool, deadline: Instant) -> io::Result<()> {
    let Some(initialize) = initialize else { return Ok(()); };
    stream.set_nonblocking(true)?;
    for chunk in initialize.as_bytes().chunks(MAX_INPUT_CHUNK) { send_payload(stream, chunk)?; }
    let mut response = Vec::new();
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "Restoring MCP initialization timed out"));
        }
        let mut byte = [0];
        match platform::read_handshake(stream, &mut byte) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(_) => {
                response.push(byte[0]);
                if response.len() > MAX_REQUEST_BYTES {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "Restored initialize response is too large"));
                }
                if byte[0] == b'\n' { break; }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let response: Value = serde_json::from_slice(&response)?;
    let request: Value = serde_json::from_str(initialize)?;
    if response.get("id") != request.get("id") || response.get("result").is_none() || response.get("error").is_some() {
        return Err(io::Error::other("Replacement backend rejected MCP initialization"));
    }
    if initialized {
        send_payload(stream, b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")?;
    }
    stream.set_nonblocking(false)?;
    Ok(())
}

pub(super) fn run(mut stream: Stream, args: &ServeArgs, workspace: &Workspace, configuration: &Configuration, session: Option<InitializedSession>) -> io::Result<()> {
    let (mut initialize, mut initialized, buffered) = match session {
        Some(session) => {
            restore_session(&mut stream, Some(&session.initialize), true, Instant::now() + STARTUP_TIMEOUT)?;
            (Some(session.initialize), true, session.buffered)
        }
        None => (None, false, Vec::new()),
    };
    let mut stream = Arc::new(stream);
    let mut generation = 1;
    let (events, received) = mpsc::sync_channel(4);
    let (ready, input_ready) = mpsc::sync_channel(1);
    let input_events = events.clone();
    thread::spawn(move || input_thread(input_events, input_ready, buffered));
    let output = Arc::clone(&stream);
    let output_events = events.clone();
    thread::spawn(move || output_thread(output, generation, output_events));
    let mut writer = io::stdout().lock();
    let mut pending: Option<Pending> = None;
    let mut closing = false;
    loop {
        let lost = match received.recv().map_err(io::Error::other)? {
            Event::OversizedInput => {
                eprintln!("Xray: skipped MCP request exceeding 10 MiB");
                let response = json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"MCP request exceeds 10 MiB; request skipped"}});
                write_output(&mut writer, &format!("{response}\n"))?;
                ready.send(()).map_err(io::Error::other)?;
                continue;
            }
            Event::Input(result) => match result? {
                None => {
                    closing = true;
                    match (&*stream).write_all(&0u32.to_le_bytes()) {
                        Ok(()) => continue,
                        Err(error) => error,
                    }
                }
                Some(line) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&line) {
                        if value.get("method").and_then(Value::as_str) == Some("notifications/initialized") {
                            initialized = true;
                        }
                        if let Some(id) = value.get("id").filter(|id| !id.is_null())
                            && value.get("method").and_then(Value::as_str).is_some()
                            && value.get("jsonrpc").and_then(Value::as_str).is_some() {
                                pending = Some(Pending {
                                    id: id.clone(),
                                    initialization: (value["method"] == "initialize").then(|| line.clone()),
                                });
                            }
                    }
                    match send_line(&stream, &line) {
                        Ok(()) => {
                            // One outstanding call per session preserves the server's
                            // serial semantics and bounds buffered input during recovery.
                            if pending.is_none() { ready.send(()).map_err(io::Error::other)?; }
                            continue;
                        }
                        Err(error) => {
                            if pending.is_none() { ready.send(()).map_err(io::Error::other)?; }
                            error
                        }
                    }
                }
            },
            Event::Output(source, result) => {
                if source != generation { continue; }
                match result {
                    Ok(Some(line)) => {
                        write_output(&mut writer, &line)?;
                        if let Ok(value) = serde_json::from_str::<Value>(&line) {
                            crate::mcp::updates::mark_wire_notice(&value);
                        }
                        if let Ok(value) = serde_json::from_str::<Value>(&line)
                            && value.get("method").is_none()
                            && pending.as_ref().is_some_and(|request| value.get("id") == Some(&request.id)) {
                                let request = pending.take().expect("matching pending request");
                                if value.get("error").is_none() && request.initialization.is_some() {
                                    initialize = request.initialization;
                                }
                                ready.send(()).map_err(io::Error::other)?;
                            }
                        continue;
                    }
                    Ok(None) if closing => return Ok(()),
                    Ok(None) => io::Error::new(io::ErrorKind::ConnectionAborted, "workspace backend closed its connection"),
                    Err(error) => error,
                }
            }
        };
        eprintln!("Xray: backend connection lost: {lost}. In-flight tool requests will not be replayed.");
        if fail_pending(&mut pending, &mut writer)? { ready.send(()).map_err(io::Error::other)?; }
        if closing { return Ok(()); }
        // The ownership lock prevents concurrent survivors from loading their own
        // indexes. Rehydrate only protocol state, never a tools/call request.
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let replacement = loop {
            let mut replacement = connect_or_start_until(args, workspace, configuration, deadline)?;
            match restore_session(&mut replacement, initialize.as_deref(), initialized, deadline) {
                Ok(()) => break replacement,
                Err(error) if retryable(&error) && Instant::now() < deadline => {
                    eprintln!("Xray: replacement disconnected during initialization: {error}; retrying protocol setup only.");
                    thread::sleep(POLL);
                }
                Err(error) => return Err(io::Error::new(error.kind(), format!("Cannot restore MCP session: {error}"))),
            }
        };
        generation += 1;
        stream = Arc::new(replacement);
        let output = Arc::clone(&stream);
        let output_events = events.clone();
        thread::spawn(move || output_thread(output, generation, output_events));
        eprintln!("Xray: MCP connection recovered through the shared workspace backend.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_backend_lines_are_not_forwarded() {
        for fragment in ["{\"jsonrpc\":\"2.0\",", "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}"] {
            let mut reader = io::Cursor::new(fragment.as_bytes());
            assert_eq!(read_backend_line(&mut reader).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
        }
        let mut reader = io::Cursor::new(b"{\"id\":1}\n".as_slice());
        assert_eq!(read_backend_line(&mut reader).unwrap().as_deref(), Some("{\"id\":1}\n"));
    }

    #[test]
    fn oversized_client_line_does_not_discard_following_request() {
        let mut input = vec![b'x'; MAX_REQUEST_BYTES + 128];
        input.extend_from_slice(b"\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n");
        let mut reader = io::Cursor::new(input);
        assert_eq!(read_client_line(&mut reader).unwrap(), Some(String::new()));
        let next: Value = serde_json::from_str(&read_client_line(&mut reader).unwrap().unwrap()).unwrap();
        assert_eq!(next["id"], 2);
    }

    #[test]
    fn recovery_reports_unknown_outcome_without_replaying_request() {
        let mut pending = Some(Pending { id: json!(42), initialization: None });
        let mut output = Vec::new();
        assert!(fail_pending(&mut pending, &mut output).unwrap());
        assert!(pending.is_none());
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["id"], 42);
        assert_eq!(response["error"]["code"], -32000);
        assert!(response.get("result").is_none());
        assert!(response["error"]["message"].as_str().unwrap().contains("NOT replayed"));
        assert!(!fail_pending(&mut pending, &mut output).unwrap());
    }
}
