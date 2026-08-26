//! MCP (Model Context Protocol) server for AI fleet dispatch (spec §5.6).
//!
//! Exposes fleet dispatch as JSON-RPC 2.0 tools so an AI assistant / agent can
//! list units, assign operators, and drive autonomy handovers. This is the
//! "MCP server exposing fleet dispatch tools" item: `list_units`,
//! `assign_unit`, `engage_autonomy`, `take_manual_control`.
//!
//! The transport is deliberately unopinionated — [`McpServer::handle`] takes a
//! parsed JSON-RPC request and returns a JSON response, so it can be served
//! over the HTTP `/mcp` endpoint ([`crate::server`]), stdio, or any other
//! byte channel.

use crate::fleet::Fleet;
use crate::json::Json;

/// The MCP server. Stateless across calls except through the [`Fleet`] it is
/// handed on each [`handle`](Self::handle).
pub struct McpServer;

impl McpServer {
    /// A new MCP server.
    pub fn new() -> Self {
        Self
    }

    /// Handles one JSON-RPC 2.0 request object, mutating `fleet` as needed.
    pub fn handle(&self, req: &Json, fleet: &mut Fleet) -> Json {
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = req.get("id").cloned().unwrap_or(Json::Null);
        let params = req.get("params").cloned().unwrap_or(Json::Null);

        match method {
            "initialize" => json_rpc_ok(
                id,
                Json::obj(&[
                    ("protocolVersion", Json::str("2024-11-05")),
                    ("capabilities", Json::obj(&[("tools", Json::obj(&[]))])),
                    (
                        "serverInfo",
                        Json::obj(&[
                            ("name", Json::str("tpt-teleop-fleet")),
                            ("version", Json::str(crate::VERSION)),
                        ]),
                    ),
                ]),
            ),
            "tools/list" => json_rpc_ok(
                id,
                Json::obj(&[(
                    "tools",
                    Json::arr(vec![
                        tool(
                            "list_units",
                            "List every unit registered with the fleet, with mode and status.",
                            &[],
                        ),
                        tool(
                            "assign_unit",
                            "Assign a human or AI operator to a unit.",
                            &[("unit_id", "integer"), ("operator", "string")],
                        ),
                        tool(
                            "engage_autonomy",
                            "Command a unit into AUTO mode (autonomy drives; operator supervises).",
                            &[("unit_id", "integer")],
                        ),
                        tool(
                            "take_manual_control",
                            "Command a unit into FULL-TELEOP mode (operator drives directly).",
                            &[("unit_id", "integer")],
                        ),
                    ]),
                )]),
            ),
            "tools/call" => {
                let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(Json::Null);
                self.call_tool(name, &args, fleet, id)
            }
            _ => json_rpc_err(id, -32601, format!("method not found: {method}")),
        }
    }

    fn call_tool(&self, name: &str, args: &Json, fleet: &mut Fleet, id: Json) -> Json {
        match name {
            "list_units" => {
                let units: Vec<Json> = fleet.list_units().iter().map(|u| u.to_json()).collect();
                json_rpc_ok(
                    id,
                    Json::obj(&[
                        ("units", Json::arr(units)),
                        ("count", Json::uint(fleet.list_units().len() as u64)),
                    ]),
                )
            }
            "assign_unit" => {
                let uid = args.get("unit_id").and_then(|v| v.as_u64());
                let op = args
                    .get("operator")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                match (uid, op) {
                    (Some(u), Some(o)) => match fleet.assign(u, o) {
                        Ok(()) => json_rpc_ok(
                            id,
                            Json::obj(&[
                                ("unit_id", Json::uint(u)),
                                ("assigned", Json::str(&format!("{u}"))),
                            ]),
                        ),
                        Err(e) => json_rpc_err(id, -32000, e.to_string()),
                    },
                    _ => json_rpc_err(id, -32602, "missing unit_id/operator".into()),
                }
            }
            "engage_autonomy" | "take_manual_control" => {
                let mode = if name == "engage_autonomy" {
                    "engage_autonomy"
                } else {
                    "take_manual_control"
                };
                let uid = args.get("unit_id").and_then(|v| v.as_u64());
                match uid {
                    Some(u) => {
                        let res = if name == "engage_autonomy" {
                            fleet.engage_autonomy(u)
                        } else {
                            fleet.take_manual_control(u)
                        };
                        match res {
                            Ok(()) => json_rpc_ok(
                                id,
                                Json::obj(&[
                                    ("unit_id", Json::uint(u)),
                                    ("tool", Json::str(mode)),
                                    ("ok", Json::Bool(true)),
                                ]),
                            ),
                            Err(e) => json_rpc_err(id, -32000, e.to_string()),
                        }
                    }
                    None => json_rpc_err(id, -32602, "missing unit_id".into()),
                }
            }
            _ => json_rpc_err(id, -32601, format!("unknown tool: {name}")),
        }
    }
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new()
    }
}

/// Builds a tool descriptor with an `object` input schema.
fn tool(name: &str, desc: &str, props: &[(&str, &str)]) -> Json {
    Json::obj(&[
        ("name", Json::str(name)),
        ("description", Json::str(desc)),
        (
            "inputSchema",
            Json::obj(&[
                ("type", Json::str("object")),
                ("properties", Json::obj_str(props)),
                (
                    "required",
                    Json::arr(props.iter().map(|(k, _)| Json::str(k)).collect()),
                ),
            ]),
        ),
    ])
}

fn json_rpc_ok(id: Json, result: Json) -> Json {
    Json::obj(&[
        ("jsonrpc", Json::str("2.0")),
        ("id", id),
        ("result", result),
    ])
}

fn json_rpc_err(id: Json, code: i64, msg: String) -> Json {
    Json::obj(&[
        ("jsonrpc", Json::str("2.0")),
        ("id", id),
        (
            "error",
            Json::obj(&[
                ("code", Json::Num(code as f64)),
                ("message", Json::Str(msg)),
            ]),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::{CapturingTransport, NullTransport};
    use crate::recorder::VecRecorder;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn addr() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5000)
    }

    fn init_req() -> Json {
        Json::obj(&[
            ("jsonrpc", Json::str("2.0")),
            ("id", Json::num(1.0)),
            ("method", Json::str("initialize")),
        ])
    }

    #[test]
    fn initialize_handshake() {
        let srv = McpServer::new();
        let mut fleet = Fleet::new(Box::new(NullTransport));
        let r = srv.handle(&init_req(), &mut fleet);
        assert_eq!(r.get("jsonrpc").and_then(|v| v.as_str()), Some("2.0"));
        assert!(r.get("result").is_some());
    }

    #[test]
    fn tools_list_exposes_four_tools() {
        let srv = McpServer::new();
        let mut fleet = Fleet::new(Box::new(NullTransport));
        let req = Json::obj(&[
            ("jsonrpc", Json::str("2.0")),
            ("id", Json::num(2.0)),
            ("method", Json::str("tools/list")),
        ]);
        let r = srv.handle(&req, &mut fleet);
        let tools = r.get("result").and_then(|x| x.get("tools")).unwrap();
        if let Json::Arr(items) = tools {
            assert_eq!(items.len(), 4);
        } else {
            panic!("tools not an array");
        }
    }

    #[test]
    fn list_units_returns_registered_units() {
        let srv = McpServer::new();
        let mut fleet = Fleet::new(Box::new(NullTransport));
        fleet
            .register_unit(5, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        let req = Json::obj(&[
            ("jsonrpc", Json::str("2.0")),
            ("id", Json::num(3.0)),
            ("method", Json::str("tools/call")),
            (
                "params",
                Json::obj(&[
                    ("name", Json::str("list_units")),
                    ("arguments", Json::obj(&[])),
                ]),
            ),
        ]);
        let r = srv.handle(&req, &mut fleet);
        assert!(r.get("error").is_none(), "got error: {r:?}");
        let units = r.get("result").and_then(|x| x.get("units")).unwrap();
        if let Json::Arr(items) = units {
            assert_eq!(items.len(), 1);
        } else {
            panic!("units not array");
        }
    }

    #[test]
    fn engage_autonomy_drives_fleet_mode() {
        let srv = McpServer::new();
        let mut fleet = Fleet::new(Box::new(CapturingTransport::new()));
        fleet
            .register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        let req = Json::obj(&[
            ("jsonrpc", Json::str("2.0")),
            ("id", Json::num(4.0)),
            ("method", Json::str("tools/call")),
            (
                "params",
                Json::obj(&[
                    ("name", Json::str("engage_autonomy")),
                    ("arguments", Json::obj(&[("unit_id", Json::uint(1))])),
                ]),
            ),
        ]);
        let r = srv.handle(&req, &mut fleet);
        assert!(r.get("error").is_none(), "got error: {r:?}");
        assert_eq!(fleet.get(1).unwrap().mode, tpt_t_core::mode::Mode::Auto);
    }

    #[test]
    fn unknown_method_errors() {
        let srv = McpServer::new();
        let mut fleet = Fleet::new(Box::new(NullTransport));
        let req = Json::obj(&[
            ("jsonrpc", Json::str("2.0")),
            ("id", Json::num(5.0)),
            ("method", Json::str("bogus")),
        ]);
        let r = srv.handle(&req, &mut fleet);
        assert!(r.get("error").is_some());
    }
}
