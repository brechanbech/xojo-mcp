use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol revisions this server implements, oldest first.
///
/// The wire surface xmcp exposes — `initialize`, `tools/list`, `tools/call`,
/// `resources/list`, `resources/read`, `ping` and the client notifications — is
/// unchanged across all of them, and every tool result is a plain text content
/// block, which is valid in each. `2026-07-28` is deliberately absent: it is the
/// first revision to require the SEP-2243 standard headers and a handshake-free
/// request path, neither of which this server implements.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

/// The revision offered when the client requests one we do not support, or
/// names none at all. Always the newest entry in [`SUPPORTED_PROTOCOL_VERSIONS`].
pub const PROTOCOL_VERSION: &str = "2025-11-25";

pub const SERVER_NAME: &str = "xmcp";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Pick the revision to answer `initialize` with.
///
/// The spec requires the server to echo the client's requested version back
/// when it supports it, and to offer its own preferred version otherwise — the
/// client then either proceeds on that version or disconnects. Replying with a
/// fixed version regardless of the request (what this did before) tells a
/// 2025-06-18 client to speak a dialect it never asked for, which that client
/// is entitled to treat as a failed handshake.
pub fn negotiate_protocol_version(requested: Option<&str>) -> &'static str {
    let Some(requested) = requested else {
        return PROTOCOL_VERSION;
    };
    SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .find(|supported| **supported == requested)
        .copied()
        .unwrap_or(PROTOCOL_VERSION)
}

#[derive(Debug, Clone, Copy)]
#[repr(i32)]
pub enum ErrorCode {
    ParseError = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams = -32602,
    #[allow(dead_code)]
    InternalError = -32603,
    #[allow(dead_code)]
    ServerError = -32000,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub id: Option<Value>,
    pub method: Option<String>,
    pub params: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcResponse {
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Value, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code: code as i32,
                message: message.into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_version_is_the_newest_supported_one() {
        assert_eq!(
            SUPPORTED_PROTOCOL_VERSIONS.last().copied(),
            Some(PROTOCOL_VERSION)
        );
    }

    #[test]
    fn supported_versions_are_sorted_oldest_first() {
        let mut sorted = SUPPORTED_PROTOCOL_VERSIONS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, SUPPORTED_PROTOCOL_VERSIONS);
    }

    #[test]
    fn a_supported_request_is_echoed_back() {
        for version in SUPPORTED_PROTOCOL_VERSIONS {
            assert_eq!(negotiate_protocol_version(Some(version)), *version);
        }
    }

    #[test]
    fn an_unsupported_request_falls_back_to_the_preferred_version() {
        // Newer than anything we know, older than anything we know, and junk.
        for version in ["2026-07-28", "2099-01-01", "2024-01-01", "", "nonsense"] {
            assert_eq!(negotiate_protocol_version(Some(version)), PROTOCOL_VERSION);
        }
    }

    #[test]
    fn a_missing_request_falls_back_to_the_preferred_version() {
        assert_eq!(negotiate_protocol_version(None), PROTOCOL_VERSION);
    }
}
