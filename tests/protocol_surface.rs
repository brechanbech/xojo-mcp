//! Offline checks of the MCP wire surface, run against a real `xmcp` process
//! over stdio. Nothing here touches the Xojo IDE: every request is one the
//! server answers on its own, so the suite is safe to run with no IDE open and
//! deterministic when one is.
//!
//! The negotiation cases are a regression guard. `initialize` used to answer
//! with a hardcoded `protocolVersion` no matter what the client asked for, so a
//! client on an older revision was told to speak a dialect it never requested
//! and could legitimately drop the connection.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

/// The revision xmcp offers when it cannot honour what the client asked for.
/// Kept in step with `mcp::protocol::PROTOCOL_VERSION`, which the crate's own
/// unit tests pin to the newest supported revision.
const PREFERRED: &str = "2025-11-25";

/// Every revision the server is expected to accept verbatim.
const SUPPORTED: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

/// The tools read-only mode must hide.
const MUTATING: &[&str] = &[
    "set_code",
    "edit_code",
    "property_value",
    "set_declaration",
    "set_selected_text",
    "create_project_item",
    "revert_project",
    "save_project",
];

/// A live `xmcp` child speaking JSON-RPC over stdio.
struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Server {
    fn start() -> Self {
        Self::start_with_env(&[])
    }

    fn start_with_env(env: &[(&str, &str)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_xmcp"));
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        // A socket name the IDE would never have created, so a stray tool call
        // cannot reach a real IDE that happens to be running on this machine.
        command.env("XOJO_IPCPATH", "xmcp_protocol_surface_test");
        for (key, value) in env {
            command.env(key, value);
        }

        let mut child = command.spawn().expect("failed to spawn the xmcp binary");
        let stdin = child.stdin.take().expect("no stdin");
        let stdout = BufReader::new(child.stdout.take().expect("no stdout"));

        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    /// Send a request and read its response.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));

        let response = self.read_line();
        assert_eq!(response["id"], json!(id), "response is for another request");
        assert_eq!(response["jsonrpc"], "2.0", "missing JSON-RPC version");
        response
    }

    /// Send a notification, which by definition draws no response.
    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
    }

    fn send(&mut self, message: Value) {
        writeln!(self.stdin, "{message}").expect("failed to write to the server");
        self.stdin.flush().expect("failed to flush");
    }

    fn read_line(&mut self) -> Value {
        let mut line = String::new();
        let read = self
            .stdout
            .read_line(&mut line)
            .expect("failed to read from the server");
        assert!(read > 0, "the server closed stdout instead of replying");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON: {e}: {line}"))
    }

    /// Complete the handshake, returning the version the server settled on.
    fn handshake(&mut self, requested: &str) -> String {
        let response = self.request(
            "initialize",
            json!({
                "protocolVersion": requested,
                "capabilities": {},
                "clientInfo": { "name": "xmcp-tests", "version": "0" },
            }),
        );
        self.notify("notifications/initialized", json!({}));
        response["result"]["protocolVersion"]
            .as_str()
            .unwrap_or_else(|| panic!("no protocolVersion in the initialize result: {response}"))
            .to_string()
    }

    fn tool_names(&mut self) -> Vec<String> {
        let response = self.request("tools/list", json!({}));
        response["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools/list returned no array: {response}"))
            .iter()
            .map(|tool| tool["name"].as_str().expect("a tool has no name").to_string())
            .collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_supported_version_is_echoed_back() {
    for version in SUPPORTED {
        let mut server = Server::start();
        assert_eq!(
            server.handshake(version),
            *version,
            "the server must answer {version} with {version}"
        );
    }
}

#[test]
fn an_unsupported_version_gets_the_preferred_one() {
    // Newer than anything xmcp implements, and older than anything it does.
    for version in ["2026-07-28", "2099-01-01", "2020-01-01"] {
        let mut server = Server::start();
        assert_eq!(
            server.handshake(version),
            PREFERRED,
            "the server must fall back to {PREFERRED} for {version}"
        );
    }
}

#[test]
fn a_handshake_without_a_version_gets_the_preferred_one() {
    let mut server = Server::start();
    let response = server.request(
        "initialize",
        json!({
            "capabilities": {},
            "clientInfo": { "name": "xmcp-tests", "version": "0" },
        }),
    );
    assert_eq!(response["result"]["protocolVersion"], PREFERRED);
}

#[test]
fn initialize_reports_the_capabilities_it_serves() {
    let mut server = Server::start();
    let response = server.request(
        "initialize",
        json!({
            "protocolVersion": PREFERRED,
            "capabilities": {},
            "clientInfo": { "name": "xmcp-tests", "version": "0" },
        }),
    );
    let result = &response["result"];

    assert!(
        result["capabilities"]["tools"].is_object(),
        "tools capability missing: {response}"
    );
    assert!(
        result["capabilities"]["resources"].is_object(),
        "resources capability missing: {response}"
    );
    assert_eq!(result["serverInfo"]["name"], "xmcp");
    assert_eq!(
        result["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION"),
        "serverInfo must report the crate version"
    );
}

#[test]
fn every_listed_tool_has_a_usable_schema() {
    let mut server = Server::start();
    server.handshake(PREFERRED);

    let response = server.request("tools/list", json!({}));
    let tools = response["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list returned no array: {response}"));
    assert!(!tools.is_empty(), "the server listed no tools");

    for tool in tools {
        let name = tool["name"].as_str().expect("a tool has no name");
        assert!(
            tool["description"].as_str().is_some_and(|d| !d.is_empty()),
            "{name} has no description"
        );
        assert_eq!(
            tool["inputSchema"]["type"], "object",
            "{name} has no object inputSchema"
        );
        assert!(
            tool["inputSchema"]["properties"].is_object(),
            "{name} has no schema properties"
        );
    }
}

#[test]
fn read_only_mode_hides_the_mutating_tools() {
    // Exercises the XMCP_READ_ONLY env path, which the in-crate unit tests
    // cannot reach — they construct the Server directly.
    let mut server = Server::start_with_env(&[("XMCP_READ_ONLY", "1")]);
    server.handshake(PREFERRED);
    let names = server.tool_names();

    for tool in MUTATING {
        assert!(!names.contains(&tool.to_string()), "{tool} must be hidden");
    }
    assert!(
        names.contains(&"get_code".to_string()),
        "read tools must still be listed"
    );
}

#[test]
fn a_mutating_call_is_refused_in_read_only_mode() {
    let mut server = Server::start_with_env(&[("XMCP_READ_ONLY", "1")]);
    server.handshake(PREFERRED);

    let response = server.request("tools/call", json!({ "name": "set_code", "arguments": {} }));
    assert_eq!(
        response["error"]["code"], -32600,
        "expected an InvalidRequest error: {response}"
    );
    assert!(
        response["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("read-only")),
        "the error should explain read-only mode: {response}"
    );
}

#[test]
fn resources_list_offers_the_usage_guide() {
    let mut server = Server::start();
    server.handshake(PREFERRED);

    let response = server.request("resources/list", json!({}));
    let resources = response["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("resources/list returned no array: {response}"));
    assert!(
        resources
            .iter()
            .any(|r| r["uri"] == "file://usage-guide.md"),
        "the usage guide is not listed: {response}"
    );
}

#[test]
fn ping_returns_an_empty_result() {
    let mut server = Server::start();
    server.handshake(PREFERRED);

    let response = server.request("ping", json!({}));
    assert_eq!(response["result"], json!({}), "unexpected ping result");
    assert!(response["error"].is_null());
}

#[test]
fn an_unknown_method_is_a_method_not_found_error() {
    let mut server = Server::start();
    server.handshake(PREFERRED);

    let response = server.request("prompts/list", json!({}));
    assert_eq!(
        response["error"]["code"], -32601,
        "expected MethodNotFound: {response}"
    );
}

#[test]
fn a_notification_draws_no_response() {
    let mut server = Server::start();
    server.handshake(PREFERRED);

    // If the server wrongly answered the notification, this ping would read
    // that answer instead — and `request` asserts on the id.
    server.notify("notifications/cancelled", json!({ "requestId": 1 }));
    let response = server.request("ping", json!({}));
    assert_eq!(response["result"], json!({}));
}

#[test]
fn malformed_json_is_a_parse_error_and_the_server_stays_up() {
    let mut server = Server::start();

    writeln!(server.stdin, "{{not json").expect("failed to write");
    server.stdin.flush().expect("failed to flush");

    let response = server.read_line();
    assert_eq!(
        response["error"]["code"], -32700,
        "expected ParseError: {response}"
    );
    assert!(response["id"].is_null(), "a parse error has a null id");

    // The loop must survive it rather than exiting.
    let response = server.request("ping", json!({}));
    assert_eq!(response["result"], json!({}));
}
