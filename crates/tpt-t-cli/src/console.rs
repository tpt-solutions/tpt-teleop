//! `tpt-t-cli console` — AI co-pilot console over the secured MCP fleet
//! dispatch (spec §5.6, Phase 17).
//!
//! An interactive JSON-RPC 2.0 client for a running `tpt-t-cloud`
//! `FleetServer`'s `/mcp` endpoint. No new external dependencies: it speaks
//! the same minimal HTTP/1.1 the server implements (`tpt_t_cloud::http`)
//! over a plain `std::net::TcpStream`, one connection per call (the server
//! closes the socket after every response). Every fleet-dispatch tool
//! (`list_units`, `assign_unit`, `engage_autonomy`, `take_manual_control`) is
//! reachable as a console command, so an operator — or an AI agent driving
//! this console's stdin — dispatches the fleet the same way the MCP
//! `tools/call` contract describes it.
//!
//! When the server has a `FleetAuthz` gate installed, pass `--attestation
//! <FILE>` naming a file holding the hex-encoded `Attestation::to_bytes()`
//! wire form (see `tpt_t_cloud::auth::attestation_header`, minus the
//! `Authorization: TPT-Attestation ` prefix this console adds itself).
//!
//! ```text
//! tpt-t-cli console [--host <ADDR:PORT>] [--attestation <FILE>]
//! ```

use std::io::{Read, Write};
use std::net::TcpStream;

use tpt_t_cloud::Json;

pub fn run(args: &[String]) -> i32 {
    let mut host = "127.0.0.1:8080".to_string();
    let mut auth: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--host" => {
                i += 1;
                match args.get(i) {
                    Some(h) => host = h.clone(),
                    None => {
                        eprintln!("error: --host requires an address");
                        return 1;
                    }
                }
            }
            "--attestation" => {
                i += 1;
                match args.get(i) {
                    Some(path) => match std::fs::read_to_string(path) {
                        Ok(hex) => auth = Some(hex.trim().to_string()),
                        Err(e) => {
                            eprintln!("error: cannot read attestation file {path:?}: {e}");
                            return 1;
                        }
                    },
                    None => {
                        eprintln!("error: --attestation requires a file path");
                        return 1;
                    }
                }
            }
            other => {
                eprintln!("error: unrecognized argument {other:?}");
                return 1;
            }
        }
        i += 1;
    }

    println!("tpt-teleop AI co-pilot console — dispatching to {host} via MCP");
    println!("type 'help' for commands, 'quit' to exit");

    let mut next_id = 1u64;
    let stdin = std::io::stdin();
    loop {
        print!("> ");
        if std::io::stdout().flush().is_err() {
            return 1;
        }
        let mut line = String::new();
        if stdin.read_line(&mut line).unwrap_or(0) == 0 {
            println!();
            return 0;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        match words[0] {
            "quit" | "exit" => return 0,
            "help" => print_help(),
            "list" if words.len() == 1 => {
                dispatch(
                    &host,
                    auth.as_deref(),
                    &mut next_id,
                    "list_units",
                    Json::obj(&[]),
                );
            }
            "assign" if words.len() == 3 => match words[1].parse::<u64>() {
                Ok(uid) => dispatch(
                    &host,
                    auth.as_deref(),
                    &mut next_id,
                    "assign_unit",
                    Json::obj(&[
                        ("unit_id", Json::uint(uid)),
                        ("operator", Json::str(words[2])),
                    ]),
                ),
                Err(_) => eprintln!("error: unit id must be an integer"),
            },
            "auto" if words.len() == 2 => call_unit_tool(
                &host,
                auth.as_deref(),
                &mut next_id,
                "engage_autonomy",
                words[1],
            ),
            "manual" if words.len() == 2 => call_unit_tool(
                &host,
                auth.as_deref(),
                &mut next_id,
                "take_manual_control",
                words[1],
            ),
            _ => eprintln!("error: unrecognized command {line:?} (type 'help')"),
        }
    }
}

fn call_unit_tool(host: &str, auth: Option<&str>, next_id: &mut u64, tool: &str, unit: &str) {
    match unit.parse::<u64>() {
        Ok(uid) => dispatch(
            host,
            auth,
            next_id,
            tool,
            Json::obj(&[("unit_id", Json::uint(uid))]),
        ),
        Err(_) => eprintln!("error: unit id must be an integer"),
    }
}

fn print_help() {
    println!(
        "commands:\n\
         \x20 list                          list every registered unit\n\
         \x20 assign <unit_id> <operator>   assign an operator to a unit\n\
         \x20 auto <unit_id>                engage autonomy (AUTO mode)\n\
         \x20 manual <unit_id>              take manual control (FULL-TELEOP mode)\n\
         \x20 help                          show this message\n\
         \x20 quit                          exit the console"
    );
}

/// Sends one JSON-RPC 2.0 `tools/call` request over a fresh HTTP/1.1
/// connection and prints the decoded result or error.
fn dispatch(host: &str, auth: Option<&str>, next_id: &mut u64, tool: &str, args: Json) {
    let id = *next_id;
    *next_id += 1;
    let body = Json::obj(&[
        ("jsonrpc", Json::str("2.0")),
        ("id", Json::uint(id)),
        ("method", Json::str("tools/call")),
        (
            "params",
            Json::obj(&[("name", Json::str(tool)), ("arguments", args)]),
        ),
    ])
    .to_string();

    match send_request(host, auth, &body) {
        Ok((status, resp_body)) => match Json::parse(resp_body.as_bytes()) {
            Ok(v) => {
                if let Some(err) = v.get("error") {
                    eprintln!("error: {err}");
                } else if let Some(result) = v.get("result") {
                    println!("{result}");
                } else {
                    println!("{v}");
                }
            }
            Err(e) => {
                eprintln!("error: invalid JSON response (HTTP {status}): {e}\n{resp_body}")
            }
        },
        Err(e) => eprintln!("error: request to {host} failed: {e}"),
    }
}

/// Issues one HTTP/1.1 request over a plain `TcpStream` and returns the
/// status code and body. The fleet server always responds with
/// `Connection: close`, so reading to EOF after the write is sufficient —
/// no `Content-Length`-aware framing is needed on the client side.
fn send_request(host: &str, auth: Option<&str>, json_body: &str) -> std::io::Result<(u16, String)> {
    let mut stream = TcpStream::connect(host)?;
    let mut req = format!(
        "POST /mcp HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        json_body.len()
    );
    if let Some(h) = auth {
        req.push_str("Authorization: TPT-Attestation ");
        req.push_str(h);
        req.push_str("\r\n");
    }
    req.push_str("Connection: close\r\n\r\n");
    req.push_str(json_body);

    stream.write_all(req.as_bytes())?;
    stream.flush()?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let mut parts = text.splitn(2, "\r\n\r\n");
    let head = parts.next().unwrap_or("");
    let body = parts.next().unwrap_or("").to_string();
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    /// A one-shot fake server that echoes back a canned JSON-RPC response,
    /// proving the request framing (`send_request`) round-trips against a
    /// real `Content-Length`-delimited HTTP response.
    fn spawn_echo_server(response_body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        addr
    }

    #[test]
    fn send_request_round_trips_status_and_body() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"count":0,"units":[]}}"#;
        let addr = spawn_echo_server(body);
        let (status, resp) = send_request(&addr, None, "{}").unwrap();
        assert_eq!(status, 200);
        assert_eq!(resp, body);
    }

    #[test]
    fn dispatch_prints_result_without_panicking() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        let addr = spawn_echo_server(body);
        let mut next_id = 1u64;
        dispatch(&addr, None, &mut next_id, "list_units", Json::obj(&[]));
        assert_eq!(next_id, 2);
    }

    /// End-to-end against a real `tpt_t_cloud::FleetServer` (not the hand-
    /// rolled echo mock above) to prove `send_request`'s HTTP framing is
    /// compatible with the actual `/mcp` endpoint, mirroring the `#[ignore]`d
    /// real-socket integration tests in `tpt_t_cloud::server`.
    #[test]
    #[ignore]
    fn console_dispatches_list_units_against_a_real_fleet_server() {
        use std::net::{IpAddr, Ipv4Addr, SocketAddr};
        use std::time::Duration;
        use tpt_t_cloud::{CapturingTransport, FleetServer, VecRecorder};

        let mut server = FleetServer::bind(0, Box::new(CapturingTransport::new())).unwrap();
        let unit_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 6000);
        server
            .register_unit(1, unit_addr, Box::new(VecRecorder::new()))
            .unwrap();
        // `local_addr()` reports the `0.0.0.0` bind address on some
        // platforms, which `TcpStream::connect` then refuses; dial
        // loopback on the bound port instead.
        let host = format!("127.0.0.1:{}", server.local_addr().unwrap().port());
        let body = Json::obj(&[
            ("jsonrpc", Json::str("2.0")),
            ("id", Json::uint(1)),
            ("method", Json::str("tools/call")),
            (
                "params",
                Json::obj(&[
                    ("name", Json::str("list_units")),
                    ("arguments", Json::obj(&[])),
                ]),
            ),
        ])
        .to_string();

        // `FleetServer` isn't `Send` (it owns `Box<dyn Recorder>`), so *it*
        // must stay on this thread and be pumped via `run_once`; the client
        // call (our own `send_request`, exercised for real here) runs
        // concurrently on a spawned thread since `TcpStream`/`String` are
        // `Send`.
        let client = std::thread::spawn(move || send_request(&host, None, &body));
        for _ in 0..40 {
            let _ = server.run_once(Duration::from_millis(50));
            if client.is_finished() {
                break;
            }
        }
        let (status, resp_body) = client.join().unwrap().unwrap();
        assert_eq!(status, 200);
        assert!(resp_body.contains("\"units\""), "body: {resp_body}");
    }
}
