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
use std::time::{Duration, Instant};

use tpt_t_core::eventloop::{EventHandler, EventLoop, PlatformLoop, Ready, Target, Token};

use tpt_t_sec::cipher::CipherSuite;
use tpt_t_sec::identity::DeviceIdentity;
use tpt_t_sec::session::{SecureSession, respond_handshake};
use tpt_t_sec::FleetAuthz;

use crate::auth::{
    authenticate_request, authorize_tool, handshake_init_from_json, handshake_resp_to_json,
};
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

/// Server admission limits (Phase 15): bound concurrent connections and drop
/// idle ones so a single peer cannot exhaust file descriptors or pin memory.
#[derive(Debug, Clone, Copy)]
pub struct ServerLimits {
    /// Maximum number of accepted client connections (0 = unbounded).
    pub max_conns: usize,
    /// Idle timeout: a connection with no activity for longer than this is
    /// reclaimed by [`FleetServer::sweep_idle`].
    pub idle_timeout: Duration,
}

impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            max_conns: 1024,
            idle_timeout: Duration::from_secs(30),
        }
    }
}

/// One accepted client connection being served.
struct Conn {
    stream: TcpStream,
    read_buf: Vec<u8>,
    registered: bool,
    last_active: Instant,
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
    authz: Option<&'a FleetAuthz>,
    identity: Option<&'a DeviceIdentity>,
    sessions: &'a mut HashMap<u64, SecureSession>,
    limits: &'a ServerLimits,
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
                    // Connection cap: stop accepting once at the limit so a
                    // single peer cannot exhaust file descriptors.
                    if self.conns.len() >= self.limits.max_conns
                        && self.limits.max_conns > 0
                    {
                        break;
                    }
                    let _ = stream.set_nonblocking(true);
                    let token = *self.next_token;
                    *self.next_token = token.wrapping_add(1);
                    self.conns.insert(
                        token,
                        Conn {
                            stream,
                            read_buf: Vec::with_capacity(8192),
                            registered: false,
                            last_active: Instant::now(),
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
                    Ok(n) => {
                        conn.read_buf.extend_from_slice(&chunk[..n]);
                        conn.last_active = Instant::now();
                    }
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
                    // Zero-trust gate: every MCP tool call requires a verified
                    // attestation. Missing/invalid → 401.
                    let auth = match &self.authz {
                        Some(authz) => match authenticate_request(authz, req) {
                            Ok(principal) => Some((&**authz, principal)),
                            Err(401) => return Response::error(401, "missing or invalid attestation"),
                            Err(code) => return Response::error(code, "authentication failed"),
                        },
                        None => None,
                    };
                    let result = self.mcp.handle(&v, self.fleet, auth);
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

        // Secure handshake bootstrap: POST /api/units/:id/secure/handshake.
        if req.path.ends_with("/secure/handshake") && req.method == Method::Post {
            return self.handle_secure_handshake(id, req);
        }

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
                if let Err(r) = self.require_auth(req, "assign_unit") {
                    return r;
                }
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
            (Method::Post, Some("engage_autonomy")) => {
                if let Err(r) = self.require_auth(req, "engage_autonomy") {
                    return r;
                }
                self.mode_action(id, "engage")
            }
            (Method::Post, Some("take_manual_control")) => {
                if let Err(r) = self.require_auth(req, "take_manual_control") {
                    return r;
                }
                self.mode_action(id, "take")
            }
            (Method::Post, Some("command")) => {
                if let Err(r) = self.require_auth(req, "send_control") {
                    return r;
                }
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

    /// Enforces zero-trust admission for a privileged fleet action. With no
    /// [`FleetAuthz`] configured the gate is open (legacy / LAN mode). With a
    /// gate: missing/invalid attestation → `401`; a verified but unauthorized
    /// principal → `403`.
    fn require_auth(&self, req: &Request, tool: &str) -> Result<(), Response> {
        match &self.authz {
            Some(authz) => {
                let principal = authenticate_request(authz, req)
                    .map_err(|code| Response::error(code, "authentication failed"))?;
                if authorize_tool(authz, &principal, tool) {
                    Ok(())
                } else {
                    Err(Response::error(403, "insufficient attestation for this action"))
                }
            }
            None => Ok(()),
        }
    }

    /// Runs a mutually-authenticated handshake for `id` (item 10). The server's
    /// identity signs the reply; the established session is stored for the
    /// unit so downstream encrypted transport can use it.
    fn handle_secure_handshake(&mut self, id: u64, req: &Request) -> Response {
        let identity = match self.identity {
            Some(i) => i,
            None => return Response::error(501, "secure handshake not configured"),
        };
        let body = Json::parse(&req.body)
            .map_err(|e| Response::error(400, &format!("invalid handshake body: {e}")))
            .unwrap_or(Json::Null);
        let init = match handshake_init_from_json(&body) {
            Some(i) => i,
            None => return Response::error(400, "missing attestation or suites"),
        };
        match respond_handshake(identity, &init, CipherSuite::all()) {
            Ok((resp, session)) => {
                self.sessions.insert(id, session);
                Response::json(200, handshake_resp_to_json(&resp))
            }
            Err(e) => Response::error(403, &format!("handshake rejected: {e}")),
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
    authz: Option<FleetAuthz>,
    identity: Option<DeviceIdentity>,
    sessions: HashMap<u64, SecureSession>,
    limits: ServerLimits,
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
            authz: None,
            identity: None,
            sessions: HashMap::new(),
            limits: ServerLimits::default(),
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

    /// Installs the zero-trust gate and the server's signing identity. With a
    /// gate installed, every privileged fleet/MCP action requires a verified
    /// attestation (see [`require_auth`](Self::require_auth)).
    pub fn with_security(&mut self, authz: FleetAuthz, identity: DeviceIdentity) -> &mut Self {
        self.authz = Some(authz);
        self.identity = Some(identity);
        self
    }

    /// Overrides the server admission limits (connection cap / idle timeout).
    pub fn with_limits(&mut self, limits: ServerLimits) -> &mut Self {
        self.limits = limits;
        self
    }

    /// Reclaims idle client connections (no activity for longer than
    /// [`ServerLimits::idle_timeout`]). Call periodically from the event loop.
    pub fn sweep_idle(&mut self) {
        let now = Instant::now();
        let timeout = self.limits.idle_timeout;
        self.conns
            .retain(|_, c| now.saturating_duration_since(c.last_active) <= timeout);
    }

    /// Immutable access to a bootstrapped secure session (per unit id),
    /// established via the `/secure/handshake` endpoint (item 10).
    pub fn session(&self, id: u64) -> Option<&SecureSession> {
        self.sessions.get(&id)
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
            authz,
            identity,
            sessions,
            limits,
        } = self;
        let listener = listener.as_mut();
        let authz_ref = authz.as_ref();
        let identity_ref = identity.as_ref();
        let limits_ref = &*limits;
        let mut h = ServerHandler {
            listener,
            next_token,
            conns,
            fleet,
            mcp,
            authz: authz_ref,
            identity: identity_ref,
            sessions,
            limits: limits_ref,
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
            authz,
            identity,
            sessions,
            limits,
        } = self;
        let listener_field = listener.as_mut();
        let authz_ref = authz.as_ref();
        let identity_ref = identity.as_ref();
        let limits_ref = &*limits;
        let mut handler = ServerHandler {
            listener: listener_field,
            next_token,
            conns,
            fleet,
            mcp,
            authz: authz_ref,
            identity: identity_ref,
            sessions,
            limits: limits_ref,
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
            authz,
            identity,
            sessions,
            limits,
        } = self;
        let listener_field = listener.as_mut();
        let authz_ref = authz.as_ref();
        let identity_ref = identity.as_ref();
        let limits_ref = &*limits;
        let mut h = ServerHandler {
            listener: listener_field,
            next_token,
            conns,
            fleet,
            mcp,
            authz: authz_ref,
            identity: identity_ref,
            sessions,
            limits: limits_ref,
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

/// Phase 15 security hardening tests: zero-trust admission, the secure
/// handshake bootstrap, and encrypted transport. These prove auth is *enforced*
/// (not merely plumbed) and that fleet traffic is actually encrypted.
#[cfg(test)]
mod phase15_tests {
    use super::*;
    use crate::auth::attestation_header;
    use crate::fleet::{CapturingTransport, SecureUdpTransport};
    use crate::recorder::VecRecorder;
    use std::net::{IpAddr, Ipv4Addr, UdpSocket};
    use std::time::Instant;
    use tpt_t_core::mode::Mode;
    use tpt_t_core::ser::{ControlCommand, FRAME_MAGIC};
    use tpt_t_sec::cipher::CipherSuite;
    use tpt_t_sec::identity::{Attestation, DeviceIdentity};
    use tpt_t_sec::rbac::Role;
    use tpt_t_sec::zerotrust::TrustStore;
    use tpt_t_sec::FleetAuthz;

    fn hex(bytes: &[u8]) -> String {
        const H: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            s.push(H[(b >> 4) as usize] as char);
            s.push(H[(b & 0x0f) as usize] as char);
        }
        s
    }

    fn addr() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5000)
    }

    #[test]
    fn gate_without_attestation_is_rejected_401() {
        let op = DeviceIdentity::generate(1, Role::Operator).unwrap();
        let mut trust = TrustStore::new();
        trust.enroll(1, op.public_key(), Role::Operator);
        let srv_identity = DeviceIdentity::generate(99, Role::Admin).unwrap();
        let mut server = FleetServer::new(Box::new(CapturingTransport::new())).unwrap();
        server
            .register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        server.with_security(FleetAuthz::with_trust(trust), srv_identity);

        let req = Request::parse(
            b"POST /api/units/1/engage_autonomy HTTP/1.1\r\nContent-Length: 0\r\n\r\n",
        )
        .unwrap()
        .unwrap()
        .0;
        let resp = server.handle_request(&req);
        assert_eq!(resp.status, 401, "no attestation must be rejected");
    }

    #[test]
    fn unenrolled_attestation_is_rejected_401() {
        let op = DeviceIdentity::generate(1, Role::Operator).unwrap();
        let srv_identity = DeviceIdentity::generate(99, Role::Admin).unwrap();
        let mut server = FleetServer::new(Box::new(CapturingTransport::new())).unwrap();
        server
            .register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        server.with_security(FleetAuthz::with_trust(TrustStore::new()), srv_identity);

        let eph = [0xABu8; 32];
        let att = Attestation::sign(&op, &eph).unwrap();
        let header = attestation_header(&att);
        let raw = format!(
            "POST /api/units/1/engage_autonomy HTTP/1.1\r\n{header}\r\nContent-Length: 0\r\n\r\n"
        );
        let req = Request::parse(raw.as_bytes()).unwrap().unwrap().0;
        let resp = server.handle_request(&req);
        assert_eq!(resp.status, 401, "unenrolled unit must be rejected");
    }

    #[test]
    fn operator_attestation_is_admitted_and_changes_mode() {
        let op = DeviceIdentity::generate(1, Role::Operator).unwrap();
        let mut trust = TrustStore::new();
        trust.enroll(1, op.public_key(), Role::Operator);
        let srv_identity = DeviceIdentity::generate(99, Role::Admin).unwrap();
        let mut server = FleetServer::new(Box::new(CapturingTransport::new())).unwrap();
        server
            .register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        server.with_security(FleetAuthz::with_trust(trust), srv_identity);

        let eph = [0xBBu8; 32];
        let att = Attestation::sign(&op, &eph).unwrap();
        let header = attestation_header(&att);
        let raw = format!(
            "POST /api/units/1/engage_autonomy HTTP/1.1\r\n{header}\r\nContent-Length: 0\r\n\r\n"
        );
        let req = Request::parse(raw.as_bytes()).unwrap().unwrap().0;
        let resp = server.handle_request(&req);
        assert_eq!(resp.status, 200, "operator must be admitted: {:?}", String::from_utf8_lossy(&resp.body));
        assert_eq!(server.fleet().get(1).unwrap().mode, Mode::Auto);
    }

    #[test]
    fn observer_cannot_engage_autonomy_403() {
        // An Observer is enrolled and verified but lacks the EngageAutonomy
        // permission → 403 (not 401).
        let obs = DeviceIdentity::generate(3, Role::Observer).unwrap();
        let mut trust = TrustStore::new();
        trust.enroll(3, obs.public_key(), Role::Observer);
        let srv_identity = DeviceIdentity::generate(99, Role::Admin).unwrap();
        let mut server = FleetServer::new(Box::new(CapturingTransport::new())).unwrap();
        server
            .register_unit(3, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        server.with_security(FleetAuthz::with_trust(trust), srv_identity);

        let eph = [0xCCu8; 32];
        let att = Attestation::sign(&obs, &eph).unwrap();
        let header = attestation_header(&att);
        let raw = format!(
            "POST /api/units/3/engage_autonomy HTTP/1.1\r\n{header}\r\nContent-Length: 0\r\n\r\n"
        );
        let req = Request::parse(raw.as_bytes()).unwrap().unwrap().0;
        let resp = server.handle_request(&req);
        assert_eq!(resp.status, 403, "observer lacks engage permission");
    }

    #[test]
    fn secure_handshake_bootstraps_a_session() {
        let op = DeviceIdentity::generate(1, Role::Operator).unwrap();
        let mut trust = TrustStore::new();
        trust.enroll(1, op.public_key(), Role::Operator);
        let srv_identity = DeviceIdentity::generate(99, Role::Admin).unwrap();
        let mut server = FleetServer::new(Box::new(CapturingTransport::new())).unwrap();
        server
            .register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        server.with_security(FleetAuthz::with_trust(trust), srv_identity);

        let eph = [0x33u8; 32];
        let init_att = Attestation::sign(&op, &eph).unwrap();
        let body = format!(
            "{{\"attestation\":\"{}\",\"suites\":[1]}}",
            hex(&init_att.to_bytes())
        );
        let raw = format!(
            "POST /api/units/1/secure/handshake HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let req = Request::parse(raw.as_bytes()).unwrap().unwrap().0;
        let resp = server.handle_request(&req);
        assert_eq!(resp.status, 200, "handshake: {:?}", String::from_utf8_lossy(&resp.body));
        assert!(server.session(1).is_some(), "session stored for unit 1");
    }

    #[test]
    fn secure_transport_emits_encrypted_secure_frames() {
        // A command sent via SecureUdpTransport must be framed on the
        // Channel::Secure (byte 8 == 6) and its plaintext rkyv magic must not
        // appear in the captured bytes — i.e. traffic is actually encrypted.
        let key = [0x07u8; 32];
        let mut transport = SecureUdpTransport::bind(&key, CipherSuite::Aes256Gcm).unwrap();
        let recv = UdpSocket::bind("127.0.0.1:0").unwrap();
        recv.set_nonblocking(true).ok();
        let dst = recv.local_addr().unwrap();

        let cmd = ControlCommand::zeroed(Mode::FullTeleop);
        transport.send_command(dst, &cmd).unwrap();

        let mut buf = [0u8; 1500];
        let mut captured = None;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok((n, _)) = recv.recv_from(&mut buf) {
                captured = Some(buf[..n].to_vec());
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let bytes = captured.expect("datagram received");
        assert_eq!(bytes[8], 6, "must be sent on Channel::Secure");
        let magic = FRAME_MAGIC.to_le_bytes();
        assert!(
            !contains_subslice(&bytes[16..], &magic),
            "plaintext command magic leaked into the secure frame"
        );
    }

    fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
        if needle.is_empty() || haystack.len() < needle.len() {
            return false;
        }
        haystack
            .windows(needle.len())
            .any(|w| w == needle)
    }
}
