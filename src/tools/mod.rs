pub mod cost_tool;
pub mod debug_tools;
pub mod doc_tools;
pub mod ide_tools;

use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;

use crate::mcp::tool::{Tool, ToolContext, ToolResult};

/// Return all 26 tools.
pub fn all_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(ide_tools::ListProjectItems),
        Box::new(ide_tools::GetCurrentLocation),
        Box::new(ide_tools::SelectProjectItem),
        Box::new(ide_tools::GetCode),
        Box::new(ide_tools::SetCode),
        Box::new(ide_tools::EditCode),
        Box::new(ide_tools::GetSelectedText),
        Box::new(ide_tools::SetSelectedText),
        Box::new(ide_tools::BuildProject),
        Box::new(ide_tools::RunProject),
        Box::new(ide_tools::StopProject),
        Box::new(ide_tools::CreateProjectItem),
        Box::new(ide_tools::RunIdeScript),
        Box::new(ide_tools::GetProjectInfo),
        Box::new(ide_tools::RevertProject),
        Box::new(ide_tools::SaveProject),
        Box::new(ide_tools::GetItemDescription),
        Box::new(ide_tools::ConstantValue),
        Box::new(ide_tools::AnalyzeProject),
        Box::new(ide_tools::DebugControl),
        Box::new(doc_tools::SearchDocs::new()),
        Box::new(doc_tools::LookupClass),
        Box::new(doc_tools::ListDocTopics),
        Box::new(cost_tool::EstimateRequestCost),
        Box::new(debug_tools::GetDebugLog),
        Box::new(debug_tools::GetSystemLog),
    ]
}

/// Common helper: send an IDE script and extract the response.
pub fn ide_call(ctx: &ToolContext, script: &str, timeout: Duration) -> ToolResult {
    let ide = match ctx.ide {
        Some(ide) => ide,
        None => {
            return ToolResult::failure(
                "Xojo IDE is not connected. Start the IDE and restart xmcp.",
            )
        }
    };
    match ide.send_and_receive_with_timeout(script, timeout) {
        Err(e) => ToolResult::failure(e),
        Ok(response) => extract_response(&response),
    }
}

/// Common helper: send an IDE script with the default 10s timeout.
pub fn ide_call_default(ctx: &ToolContext, script: &str) -> ToolResult {
    ide_call(ctx, script, Duration::from_secs(10))
}

/// Extract the response value from an IDE JSON response.
pub fn extract_response(response: &Value) -> ToolResult {
    match response.get("response") {
        Some(Value::String(s)) => {
            if s.starts_with("ERROR:") {
                ToolResult::failure(s.as_str())
            } else {
                ToolResult::success(s.as_str())
            }
        }
        Some(other) => ToolResult::success(other.to_string()),
        None => ToolResult::failure(format!(
            "Unexpected response from IDE: {}",
            truncate_for_error(&response.to_string(), 200)
        )),
    }
}

/// Get a string argument, returning the default if not present.
pub fn arg_str<'a>(args: &'a HashMap<String, Value>, name: &str, default: &'a str) -> &'a str {
    args.get(name)
        .and_then(|v| v.as_str())
        .unwrap_or(default)
}

/// Get an integer argument, returning the default if not present.
pub fn arg_i64(args: &HashMap<String, Value>, name: &str, default: i64) -> i64 {
    args.get(name)
        .and_then(|v| v.as_i64())
        .unwrap_or(default)
}

/// Get a boolean argument, returning the default if not present.
pub fn arg_bool(args: &HashMap<String, Value>, name: &str, default: bool) -> bool {
    args.get(name)
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}

/// Cap a string for inclusion in a user-facing error. Clips to `cap` bytes
/// on a UTF-8 char boundary and appends an ellipsis plus the original size.
fn truncate_for_error(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        s.to_string()
    } else {
        let mut end = cap;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}… ({} bytes total)", &s[..end], s.len())
    }
}

#[cfg(test)]
mod tests {
    use super::truncate_for_error;

    #[test]
    fn short_input_passes_through() {
        assert_eq!(truncate_for_error("hello", 200), "hello");
    }

    #[test]
    fn long_ascii_input_is_clipped() {
        let input = "a".repeat(500);
        let out = truncate_for_error(&input, 200);
        assert!(out.starts_with(&"a".repeat(200)));
        assert!(out.ends_with("… (500 bytes total)"));
    }

    #[test]
    fn clip_lands_on_char_boundary() {
        // 'é' is 2 bytes in UTF-8; place one straddling the cap.
        let mut input = "a".repeat(199);
        input.push_str("é");           // bytes 199, 200
        input.push_str(&"b".repeat(50));
        let out = truncate_for_error(&input, 200);
        // Cap 200 falls inside the multi-byte char → boundary walked back to 199.
        assert!(out.starts_with(&"a".repeat(199)));
        assert!(out.contains("… ("));
    }
}
