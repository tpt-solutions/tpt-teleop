//! Fleet HTTP/1.1 + MCP server, driven by the Phase 2 platform event loop.
//!
//! This is the production entry point: it binds a TCP listener, registers it
//! with the same [`tpt_t_core::eventloop`] backend the link layer uses (no
//! async runtime, no hyper/axum), and on readiness (or a heartbeat pump on
//! platforms whose completion ports don't fire for non-overlapped sockets)
//! accepts connections, parses HTTP, and dispatches to either the fleet
//! dashboard API or the `/mcp` JSON-RPC endpoint.
//!
//! > **HTTP/3 note.** The roadmap names `quinn` here; per the §2 MIT-chain
//! > policy (and the Phase 7 precedent) we serve the dashboard over HTTP/1.1
//! > through this in-house event-loop server. The reliable transport slot
//! > QUIC would fill is already provided in-house by [`tpt_t_link`].

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use tpt_t_core::eventloop::{EventHandler, EventLoop, PlatformLoop, Ready, Target, Token};

use crate::error::CloudError;
use crate::fleet::{Fleet, UnitTransport, parse_mode};
use crate::http::{Method, Request, Response};
use crate::json::Json;
use crate::mcp::McpServer;
use crate::recorder::Recorder;

#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(windows)]
use std::os::windows::io::AsRawSocket;

/// Registration token for the listening socket inside the event loop.
const LISTENER_TOKEN: Token = 0;

/// One accepted client connection being served.
struct Conn {
    stream: TcpStream,
    read_buf: Vec<u8>,
    registered: bool,
}

/// Per-dispatch handler: mirrors the split-borrow pattern used by
/// [`tpt_t_link::service::NetService`] so the event loop drives the listener
/// while every other field is reachable through `&mut self`.
struct ServerHandler<'a> {
    listener: Option<&'a mut TcpListener>,
    next_token: &'a mut u64,
    conns: &'a mut HashMap<Token, Conn>,
    fleet: &'a mut Fleet,
    mcp: &'a McpServer,
}

impl EventHandler for ServerHandler<'_> {
    fn ready(&mut self, token: Token, ready: Ready) {
        if token == LISTENER_TOKEN {
            if ready.intersects(Ready::READ) {
                self.try_accept();
            }
        } else if ready.intersects(Ready::READ) {
            self.service_conn(token);
        }
    }
}

impl ServerHandler<'_> {
    /// Accepts every pending connection, registering each (best-effort) with
    /// the event loop; readiness also arrives via [`EventHandler::ready`].
    fn try_accept(&mut self) {
        let Some(l) = self.listener.as_mut() else {
            return;
        };
        let listener = &mut *l;
        loop {
            match listener.accept() {
                Ok((stream, _addr)) => {
                    let _ = stream.set_nonblocking(true);
                    let token = *self.next_token;
                    *self.next_token = token.wrapping_add(1);
                    self.conns.insert(
                        token,
                        Conn {
                            stream,
                            read_buf: Vec::with_capacity(8192),
                            registered: false,
                        },
                    );
                }
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => break,
            }
        }
    }

    /// Reads from one connection, parses a complete request, dispatches, and
    /// closes the connection (HTTP/1.0-style, no keep-alive).
    fn service_conn(&mut self, token: Token) {
        // Read phase —— borrow the connection mutably, then release it.
        {
            let Some(conn) = self.conns.get_mut(&token) else {
                return;
            };
            let mut chunk = [0u8; 8192];
            loop {
                match conn.stream.read(&mut chunk) {
                    Ok(0) => {
                        self.conns.remove(&token);
                        return;
                    }
                    Ok(n) => conn.read_buf.extend_from_slice(&chunk[..n]),
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(_) => {
                        self.conns.remove(&token);
                        return;
                    }
                }
            }
            if conn.read_buf.len() > (1 << 20) {
                self.conns.remove(&token);
                return;
            }
        }

        // Parse + dispatch phase —— take ownership of the connection so we can
        // borrow `fleet`/`mcp` freely.
        let mut conn = match self.conns.remove(&token) {
            Some(c) => c,
            None => return,
        };
        match Request::parse(&conn.read_buf) {
            Ok(Some((req, _consumed))) => {
                let response = self.dispatch(&req);
                let _ = conn.stream.write_all(&response.to_bytes());
                // `conn` drops here, closing the socket.
            }
            Ok(None) => {
                // Incomplete — keep buffering for the next read.
                self.conns.insert(token, conn);
            }
            Err(_) => {
                // Malformed request — drop the connection.
            }
        }
    }

    /// Routes a parsed request to the MCP endpoint or the dashboard API.
    fn dispatch(&mut self, req: &Request) -> Response {
        if req.method == Method::Post && req.path == "/mcp" {
            match Json::parse(&req.body) {
                Ok(v) => {
                    let result = self.mcp.handle(&v, self.fleet);
                    Response::json(200, result)
                }
                Err(e) => Response::error(400, &format!("invalid JSON-RPC body: {e}")),
            }
        } else {
            self.api_response(req)
        }
    }

    /// Handles a dashboard API request against the fleet.
    fn api_response(&mut self, req: &Request) -> Response {
        if req.path == "/api/health" {
            return Response::json(
                200,
                Json::obj(&[
                    ("status", Json::str("ok")),
                    ("version", Json::str(crate::VERSION)),
                    ("units", Json::uint(self.fleet.list_units().len() as u64)),
                ]),
            );
        }

        let id = match parse_unit_id(&req.path) {
            Some(i) => i,
            None => {
                if req.method == Method::Get && req.path == "/api/units" {
                    let us: Vec<Json> = self
                        .fleet
                        .list_units()
                        .iter()
                        .map(|u| u.to_json())
                        .collect();
                    return Response::json(
                        200,
                        Json::obj(&[
                            ("units", Json::arr(us)),
                            ("count", Json::uint(self.fleet.list_units().len() as u64)),
                        ]),
                    );
                }
                if req.method == Method::Get && req.path == "/api/sessions" {
                    let us: Vec<Json> = self
                        .fleet
                        .list_units()
                        .iter()
                        .map(|u| u.to_json())
                        .collect();
                    return Response::json(
                        200,
                        Json::obj(&[
                            ("sessions", Json::arr(us)),
                            ("count", Json::uint(self.fleet.list_units().len() as u64)),
                        ]),
                    );
                }
                return Response::error(404, "not found");
            }
        };

        let action = unit_action(&req.path);
        match (req.method.clone(), action) {
            (Method::Get, None) => match self.fleet.get(id) {
                Some(u) => Response::json(200, u.info().to_json()),
                None => Response::error(404, "unit not found"),
            },
            (Method::Get, Some("subscribers")) => match self.fleet.subscriber_count(id) {
                Ok(c) => Response::json(
                    200,
                    Json::obj(&[
                        ("unit_id", Json::uint(id)),
                        ("subscribers", Json::uint(c as u64)),
                    ]),
                ),
                Err(e) => Response::error(404, &e.to_string()),
            },
            (Method::Post, Some("assign")) => {
                let body = Json::parse(&req.body).unwrap_or(Json::Null);
                let op = body
                    .get("operator")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                match op {
                    Some(o) => match self.fleet.assign(id, o) {
                        Ok(()) => Response::json(
                            200,
                            Json::obj(&[
                                ("unit_id", Json::uint(id)),
                                ("assigned", Json::Bool(true)),
                            ]),
                        ),
                        Err(e) => Response::error(409, &e.to_string()),
                    },
                    None => Response::error(400, "missing operator"),
                }
            }
            (Method::Post, Some("engage_autonomy")) => self.mode_action(id, "engage"),
            (Method::Post, Some("take_manual_control")) => self.mode_action(id, "take"),
            (Method::Post, Some("command")) => {
                let body = Json::parse(&req.body).unwrap_or(Json::Null);
                let m = body.get("mode").and_then(|v| v.as_str());
                match m.and_then(parse_mode) {
                    Some(mode) => match self.fleet.set_mode(id, mode) {
                        Ok(()) => Response::json(
                            200,
                            Json::obj(&[
                                ("unit_id", Json::uint(id)),
                                ("mode", Json::str(mode.name())),
                            ]),
                        ),
                        Err(e) => Response::error(409, &e.to_string()),
                    },
                    None => Response::error(400, "missing/invalid mode"),
                }
            }
            _ => Response::error(405, "method not allowed"),
        }
    }

    fn mode_action(&mut self, id: u64, which: &str) -> Response {
        let res = if which == "engage" {
            self.fleet.engage_autonomy(id)
        } else {
            self.fleet.take_manual_control(id)
        };
        match res {
            Ok(()) => Response::json(
                200,
                Json::obj(&[("unit_id", Json::uint(id)), ("ok", Json::Bool(true))]),
            ),
            Err(e) => Response::error(409, &e.to_string()),
        }
    }
}

/// The fleet server: owns the event loop, listener, fleet, and MCP server.
pub struct FleetServer {
    ev: PlatformLoop,
    listener: Option<TcpListener>,
    next_token: u64,
    conns: HashMap<Token, Conn>,
    fleet: Fleet,
    mcp: McpServer,
}

impl FleetServer {
    /// Creates a server with no bound listener (for embedding / tests). Use
    /// [`FleetServer::bind`] for the production network server.
    pub fn new(transport: Box<dyn UnitTransport>) -> io::Result<Self> {
        Ok(Self {
            ev: PlatformLoop::new()?,
            listener: None,
            next_token: 1,
            conns: HashMap::new(),
            fleet: Fleet::new(transport),
            mcp: McpServer::new(),
        })
    }

    /// Binds a TCP listener on `port` (falls back to loopback if a wide bind
    /// is unavailable) and registers it with the platform event loop.
    pub fn bind(port: u16, transport: Box<dyn UnitTransport>) -> io::Result<Self> {
        let mut server = Self::new(transport)?;
        let listener = TcpListener::bind(("0.0.0.0", port))
            .or_else(|_| TcpListener::bind(("127.0.0.1", port)))?;
        listener.set_nonblocking(true)?;
        #[cfg(unix)]
        let ltarget = Target::Fd(listener.as_raw_fd());
        #[cfg(windows)]
        let ltarget = Target::Handle(listener.as_raw_socket() as usize);
        server.ev.register(ltarget, LISTENER_TOKEN, Ready::READ)?;
        server.listener = Some(listener);
        Ok(server)
    }

    /// The bound local address (peers connect here).
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "no listener bound"))?
            .local_addr()
    }

    /// Immutable fleet view.
    pub fn fleet(&self) -> &Fleet {
        &self.fleet
    }

    /// Mutable fleet view.
    pub fn fleet_mut(&mut self) -> &mut Fleet {
        &mut self.fleet
    }

    /// Explicitly registers a unit with the given recorder.
    pub fn register_unit(
        &mut self,
        id: u64,
        addr: SocketAddr,
        recorder: Box<dyn Recorder>,
    ) -> Result<(), CloudError> {
        self.fleet.register_unit(id, addr, recorder)
    }

    /// Handles one parsed HTTP request without touching the network — used by
    /// the event loop and directly testable / embeddable.
    pub fn handle_request(&mut self, req: &Request) -> Response {
        let FleetServer {
            ev: _,
            listener,
            next_token,
            conns,
            fleet,
            mcp,
        } = self;
        let listener = listener.as_mut();
        let mut h = ServerHandler {
            listener,
            next_token,
            conns,
            fleet,
            mcp,
        };
        h.dispatch(req)
    }

    /// Drives one service tick: platform dispatch + a readiness heartbeat that
    /// also serves pending connections (covers platforms whose completion
    /// ports don't fire for non-overlapped sockets).
    pub fn run_once(&mut self, timeout: Duration) -> io::Result<usize> {
        let FleetServer {
            ev,
            listener,
            next_token,
            conns,
            fleet,
            mcp,
        } = self;
        let listener_field = listener.as_mut();
        let mut handler = ServerHandler {
            listener: listener_field,
            next_token,
            conns,
            fleet,
            mcp,
        };
        let n = ev.dispatch(Some(timeout), &mut handler).unwrap_or(0);
        self.pump();
        self.register_pending();
        Ok(n.max(1))
    }

    fn pump(&mut self) {
        let FleetServer {
            ev: _,
            listener,
            next_token,
            conns,
            fleet,
            mcp,
        } = self;
        let listener_field = listener.as_mut();
        let mut h = ServerHandler {
            listener: listener_field,
            next_token,
            conns,
            fleet,
            mcp,
        };
        h.try_accept();
        let tokens: Vec<Token> = h.conns.keys().copied().collect();
        for t in tokens {
            h.service_conn(t);
        }
    }

    fn register_pending(&mut self) {
        if self.listener.is_none() {
            return;
        }
        let tokens: Vec<Token> = self
            .conns
            .iter()
            .filter(|(_, c)| !c.registered)
            .map(|(t, _)| *t)
            .collect();
        for t in tokens {
            if let Some(conn) = self.conns.get_mut(&t) {
                #[cfg(unix)]
                let target = Target::Fd(conn.stream.as_raw_fd());
                #[cfg(windows)]
                let target = Target::Handle(conn.stream.as_raw_socket() as usize);
                if self.ev.register(target, t, Ready::READ).is_ok() {
                    conn.registered = true;
                }
            }
        }
    }
}

/// Extracts the unit id from `/api/units/<id>[/...]`.
fn parse_unit_id(path: &str) -> Option<u64> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() >= 3 && parts[0] == "api" && parts[1] == "units" {
        parts[2].parse::<u64>().ok()
    } else {
        None
    }
}

/// Extracts the trailing action segment from `/api/units/<id>/<action>`.
fn unit_action(path: &str) -> Option<&str> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() >= 4 && parts[0] == "api" && parts[1] == "units" {
        Some(parts[3])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::CapturingTransport;
    use crate::json::Json;
    use crate::recorder::VecRecorder;
    use std::io::Write;
    use std::net::{IpAddr, Ipv4Addr};
    use tpt_t_core::mode::Mode;

    fn addr() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5000)
    }

    #[test]
    #[ignore]
    fn health_endpoint_over_loopback() {
        let mut server = FleetServer::bind(0, Box::new(CapturingTransport::new())).unwrap();
        server
            .register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        let ep = server.local_addr().unwrap();

        let mut client = TcpStream::connect(ep).unwrap();
        client
            .write_all(b"GET /api/health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .unwrap();

        for _ in 0..10 {
            let _ = server.run_once(Duration::from_millis(50));
        }
        client
            .set_read_timeout(Some(Duration::from_millis(2000)))
            .unwrap();
        let mut buf = Vec::new();
        client.read_to_end(&mut buf).unwrap();
        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("200 OK"), "response: {text}");
        assert!(text.contains("\"status\":\"ok\""), "response: {text}");
    }

    #[test]
    #[ignore]
    fn list_units_via_mcp_over_http() {
        let mut server = FleetServer::bind(0, Box::new(CapturingTransport::new())).unwrap();
        server
            .register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        let ep = server.local_addr().unwrap();

        let body = Json::obj(&[
            ("jsonrpc", Json::str("2.0")),
            ("id", Json::num(1.0)),
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
        let req = format!(
            "POST /mcp HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );

        let mut client = TcpStream::connect(ep).unwrap();
        client.write_all(req.as_bytes()).unwrap();
        for _ in 0..10 {
            let _ = server.run_once(Duration::from_millis(50));
        }
        client
            .set_read_timeout(Some(Duration::from_millis(2000)))
            .unwrap();
        let mut buf = Vec::new();
        client.read_to_end(&mut buf).unwrap();
        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("200 OK"), "response: {text}");
        assert!(text.contains("\"units\""), "response: {text}");
        assert!(
            text.contains("\"id\":1") || text.contains("\"id\": 1"),
            "response: {text}"
        );
    }

    #[test]
    #[ignore]
    fn engage_autonomy_via_http_api() {
        let mut server = FleetServer::bind(0, Box::new(CapturingTransport::new())).unwrap();
        server
            .register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        let ep = server.local_addr().unwrap();

        let mut client = TcpStream::connect(ep).unwrap();
        client
            .write_all(
                b"POST /api/units/1/engage_autonomy HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        for _ in 0..10 {
            let _ = server.run_once(Duration::from_millis(50));
        }
        client
            .set_read_timeout(Some(Duration::from_millis(2000)))
            .unwrap();
        let mut buf = Vec::new();
        client.read_to_end(&mut buf).unwrap();
        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("200 OK"), "response: {text}");
        assert_eq!(server.fleet().get(1).unwrap().mode, Mode::Auto);
    }

    // Socket-free coverage of the routing/MCP logic (no loopback needed).
    #[test]
    fn health_over_handle_request() {
        let mut server = FleetServer::new(Box::new(CapturingTransport::new())).unwrap();
        server
            .register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        let req =
            Request::parse(b"GET /api/health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                .unwrap()
                .unwrap()
                .0;
        let resp = server.handle_request(&req);
        let body = String::from_utf8_lossy(&resp.body);
        assert!(body.contains("\"status\":\"ok\""), "body: {body}");
    }

    #[test]
    fn list_units_via_mcp_handle_request() {
        let mut server = FleetServer::new(Box::new(CapturingTransport::new())).unwrap();
        server
            .register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        let body = Json::obj(&[
            ("jsonrpc", Json::str("2.0")),
            ("id", Json::num(1.0)),
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
        let req = Request::parse(
            format!(
                "POST /mcp HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .as_bytes(),
        )
        .unwrap()
        .unwrap()
        .0;
        let resp = server.handle_request(&req);
        let body = String::from_utf8_lossy(&resp.body);
        assert!(body.contains("\"units\""), "body: {body}");
        assert!(
            body.contains("\"id\":1") || body.contains("\"id\": 1"),
            "body: {body}"
        );
    }

    #[test]
    fn engage_autonomy_via_http_api_handle_request() {
        let mut server = FleetServer::new(Box::new(CapturingTransport::new())).unwrap();
        server
            .register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        let req = Request::parse(
            b"POST /api/units/1/engage_autonomy HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n",
        )
        .unwrap()
        .unwrap()
        .0;
        let resp = server.handle_request(&req);
        let body = String::from_utf8_lossy(&resp.body);
        assert_eq!(resp.status, 200, "body: {body}");
        assert_eq!(server.fleet().get(1).unwrap().mode, Mode::Auto);
    }
}
