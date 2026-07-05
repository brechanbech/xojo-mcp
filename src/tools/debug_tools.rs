use std::collections::HashMap;
use std::process::Command;

use serde_json::Value;

use crate::mcp::tool::*;
use crate::tools::*;

const DEBUG_LOG_PATH: &str = "/tmp/xmcp_debug.log";

// ---------------------------------------------------------------------------
// get_debug_log
// ---------------------------------------------------------------------------
pub struct GetDebugLog;

impl Tool for GetDebugLog {
    fn name(&self) -> &'static str { "get_debug_log" }
    fn description(&self) -> &'static str {
        "Reads the xmcp debug log file at /tmp/xmcp_debug.log \
         (written by App.UnhandledException handlers). Returns exception details \
         or empty message if no log exists."
    }
    fn parameters(&self) -> &[ToolParam] {
        static P: &[ToolParam] = &[ToolParam {
            name: "clear",
            param_type: ParamType::Boolean,
            description: "Delete log file after reading",
            required: false,
            default: None,
        }];
        P
    }
    fn run(&self, args: &HashMap<String, Value>, _ctx: &ToolContext) -> ToolResult {
        let clear = arg_bool(args, "clear", false);
        let path = std::path::Path::new(DEBUG_LOG_PATH);

        if !path.exists() {
            return ToolResult::success(
                "No debug log found at /tmp/xmcp_debug.log. \
                 This file is created when an app with the UnhandledException handler crashes.",
            );
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => return ToolResult::failure(format!("Could not read debug log: {e}")),
        };

        if clear {
            let _ = std::fs::remove_file(path);
        }

        if content.is_empty() {
            ToolResult::success("Debug log exists but is empty.")
        } else {
            ToolResult::success(content)
        }
    }
}

// ---------------------------------------------------------------------------
// get_system_log
// ---------------------------------------------------------------------------
pub struct GetSystemLog;

impl Tool for GetSystemLog {
    fn name(&self) -> &'static str { "get_system_log" }
    fn description(&self) -> &'static str {
        "Reads recent System.DebugLog output from the macOS unified log for a running \
         Xojo debug app (process name = app name + '.debug')."
    }
    fn parameters(&self) -> &[ToolParam] {
        static P: &[ToolParam] = &[ToolParam {
            name: "process_name",
            param_type: ParamType::String,
            description: "Process name filter (e.g. 'MyApp.debug')",
            required: true,
            default: None,
        }, ToolParam {
            name: "seconds",
            param_type: ParamType::Integer,
            description: "How many seconds back to search (default 60, max 3600)",
            required: false,
            default: None,
        }];
        P
    }
    fn run(&self, args: &HashMap<String, Value>, _ctx: &ToolContext) -> ToolResult {
        let process_name = arg_str(args, "process_name", "");
        let seconds = arg_i64(args, "seconds", 60).clamp(1, 3600);

        // Input sanitisation: only allow safe characters in process name.
        if !process_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == ' ' || c == '-')
        {
            return ToolResult::failure(
                "Invalid process_name. Only alphanumeric characters, underscores, dots, \
                 hyphens, and spaces are allowed.",
            );
        }

        if process_name.is_empty() {
            return ToolResult::failure("The `process_name` parameter cannot be empty.");
        }

        let output = match Command::new("log")
            .args([
                "show",
                "--last",
                &format!("{seconds}s"),
                "--predicate",
                &format!("process == \"{process_name}\""),
            ])
            .stderr(std::process::Stdio::null())
            .output()
        {
            Ok(o) => o,
            Err(e) => return ToolResult::failure(format!("Failed to run `log show`: {e}")),
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        ToolResult::success(format_system_log(&stdout, process_name, seconds))
    }
}

/// Format `log show` output for `get_system_log`.
///
/// `System.DebugLog` output is tagged with the `(XojoFramework)` sender, so
/// that is what we return when present. When no such line matches we do *not*
/// simply report "nothing found": if `log show` returned raw entries for the
/// process anyway (a differently instrumented app, or a Xojo version that tags
/// the sender differently), we surface those instead. Reporting a bare "no
/// output" in that case is misleading and is what pushes callers toward
/// disruptive MessageBox debugging.
fn format_system_log(stdout: &str, process_name: &str, seconds: i64) -> String {
    let framework_lines: Vec<&str> = stdout
        .lines()
        .filter(|line| line.contains("(XojoFramework)"))
        .collect();

    if !framework_lines.is_empty() {
        return framework_lines.join("\n");
    }

    // `log show` data rows begin with a timestamp (a digit); the header,
    // filter banner, and summary lines begin with letters.
    let raw_lines: Vec<&str> = stdout
        .lines()
        .filter(|line| line.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .collect();

    if raw_lines.is_empty() {
        return format!(
            "No log entries found for process '{process_name}' in the last {seconds} seconds. \
             get_system_log only surfaces System.DebugLog output. If the app is running, an empty \
             result almost always means no System.DebugLog calls ran — add System.DebugLog(\"...\") \
             to the code you are diagnosing and query again. Prefer this over MessageBox, which \
             blocks the UI and fires on every recursive call."
        );
    }

    const MAX: usize = 200;
    let shown: Vec<&str> = raw_lines.iter().take(MAX).copied().collect();
    let note = if raw_lines.len() > shown.len() {
        format!(" (showing the first {} of {})", shown.len(), raw_lines.len())
    } else {
        String::new()
    };
    format!(
        "No System.DebugLog (XojoFramework) lines matched, but `log show` returned {} raw \
         entr{} for process '{process_name}'{note}. These are not System.DebugLog output — the \
         app may be logging by another mechanism, or this Xojo version tags the sender \
         differently:\n\n{}",
        raw_lines.len(),
        if raw_lines.len() == 1 { "y" } else { "ies" },
        shown.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "Filtering the log data using \"process == \\\"App.debug\\\"\"\nTimestamp                       Thread     Type";

    #[test]
    fn returns_framework_lines_when_present() {
        let stdout = format!(
            "{HEADER}\n2026-07-05 16:00:00.000 0x1 Default App.debug: (XojoFramework) hello\n\
             2026-07-05 16:00:01.000 0x1 Default App.debug: (libsystem) noise\n"
        );
        let out = format_system_log(&stdout, "App.debug", 60);
        assert!(out.contains("(XojoFramework) hello"));
        // Non-framework noise is excluded when framework lines exist.
        assert!(!out.contains("libsystem"));
    }

    #[test]
    fn falls_back_to_raw_entries_when_no_framework_lines() {
        let stdout = format!(
            "{HEADER}\n2026-07-05 16:00:00.000 0x1 Default App.debug: (libsystem) something happened\n"
        );
        let out = format_system_log(&stdout, "App.debug", 60);
        assert!(out.contains("returned 1 raw entry for")); // singular
        assert!(out.contains("something happened"));
        assert!(out.contains("tags the sender differently"));
    }

    #[test]
    fn reports_true_silence_with_debuglog_guidance() {
        // Only header/banner lines, no timestamped data rows.
        let out = format_system_log(HEADER, "App.debug", 60);
        assert!(out.contains("No log entries found"));
        assert!(out.contains("System.DebugLog"));
        assert!(out.contains("MessageBox"));
    }
}
