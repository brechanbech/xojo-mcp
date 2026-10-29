//! How one answer from the IDE is recognised as complete and folded into one
//! envelope.
//!
//! The IDE answers once per `Print`, as each runs, and not at all for a script
//! that prints nothing. So the communicator appends a `Print` of a marker unique
//! to the request, and an answer is complete once that marker arrives -- or once
//! a compile or runtime error stops the script before it gets there. Reading
//! only the first reply and closing, as before, left the IDE writing the rest
//! into a closed socket, which kills it with SIGPIPE on macOS.
//!
//! The rules follow upstream XMCP, which measured them on Xojo 2026r2.1.

use serde_json::{Value, json};

const END_MARKER_PREFIX: &str = "xmcp-end:";

/// The text the appended `Print` outputs for this request.
pub fn end_marker(tag: &str) -> String {
    format!("{END_MARKER_PREFIX}{tag}")
}

/// The script as sent: with the end-marker `Print` appended, unless the last
/// line ends in a line continuation that would swallow it.
pub fn with_end_marker(script: &str, tag: &str) -> String {
    if ends_with_line_continuation(script) {
        script.to_string()
    } else {
        format!("{script}\nPrint \"{}\"", end_marker(tag))
    }
}

/// True when this reply part is the marker's own output. Compared exactly, so
/// a line of the script's own output is never taken for it.
pub fn is_end_marker(envelope: &Value, tag: &str) -> bool {
    envelope.get("response").and_then(Value::as_str) == Some(end_marker(tag).as_str())
}

/// True when the last line of code ends in a line continuation, so that an
/// appended line would be absorbed into it.
///
/// Not just "ends in an underscore": a trailing comment can end in one, and so
/// can a name (`Var foo_ As Integer`). The comment is removed first, then an
/// underscore counts only when it does not end a name. When unsure this says
/// false, which appends the marker -- the safe way to be wrong, since a marker
/// added to a real continuation only changes the compile error of a script
/// that could not compile anyway.
pub fn ends_with_line_continuation(script: &str) -> bool {
    let Some(last) = script.lines().rev().find(|l| !l.trim().is_empty()) else {
        return false;
    };
    // The code part of the line: up to a ' or // outside a string literal. A
    // doubled quote toggles twice and stays balanced.
    let mut code = String::new();
    let mut in_string = false;
    let mut chars = last.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            in_string = !in_string;
        } else if !in_string && (c == '\'' || (c == '/' && chars.peek() == Some(&'/'))) {
            break;
        }
        code.push(c);
    }
    let code = code.trim();
    if let (Some(word), Some(rest)) = (code.get(..3), code.get(3..))
        && word.eq_ignore_ascii_case("rem")
        && (rest.is_empty() || rest.starts_with([' ', '\t']))
    {
        return false;
    }
    let Some(before) = code.strip_suffix('_') else { return false };
    match before.chars().last() {
        None => true,
        Some(c) => !(c.is_ascii_alphanumeric() || c == '_' || !c.is_ascii()),
    }
}

/// What one reply part carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A compile or runtime error, a build with errors, a failed load.
    Error,
    /// Something the script printed.
    Output,
    /// Diagnostics that are warnings only: the script or build still ran.
    Warning,
    /// An empty object: the IDE's answer to `Print ""`.
    Empty,
    /// No `response` at all.
    Unknown,
}

/// A scriptError list holds an error unless every entry's type ends in
/// "warning". Anything that is not a list of such entries counts as an error.
fn script_error_has_error(value: &Value) -> bool {
    let Some(items) = value.as_array() else { return true };
    items.iter().any(|entry| {
        !entry
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| t.to_ascii_lowercase().ends_with("warning"))
    })
}

/// A buildError the IDE is known to send holds only non-empty "errors" and/or
/// "warnings" lists; any other shape is not taken for a clean build.
fn build_error_kind(value: &Value) -> Kind {
    let Some(obj) = value.as_object().filter(|o| !o.is_empty()) else { return Kind::Error };
    let mut warning = false;
    for (key, list) in obj {
        let Some(items) = list.as_array() else { return Kind::Error };
        match key.as_str() {
            "errors" if !items.is_empty() => return Kind::Error,
            "warnings" if !items.is_empty() => warning = true,
            "errors" | "warnings" => {}
            _ => return Kind::Error,
        }
    }
    if warning { Kind::Warning } else { Kind::Empty }
}

/// openErrors is a warning only in the shape opening a project saved by an
/// older Xojo produces: every entry a loadError of severity "warning".
fn open_errors_are_warnings(value: &Value) -> bool {
    value.as_array().is_some_and(|items| {
        !items.is_empty()
            && items.iter().all(|entry| {
                entry
                    .get("loadError")
                    .and_then(|e| e.get("severity"))
                    .and_then(Value::as_str)
                    == Some("warning")
            })
    })
}

pub fn kind(envelope: &Value) -> Kind {
    let Some(response) = envelope.get("response") else { return Kind::Unknown };
    let Some(obj) = response.as_object() else { return Kind::Output };
    if obj.is_empty() {
        return Kind::Empty;
    }
    if obj.contains_key("missingFiles") || obj.contains_key("loadError") {
        return Kind::Error;
    }
    let mut known = false;
    let mut warning = false;
    if let Some(se) = obj.get("scriptError") {
        known = true;
        if script_error_has_error(se) {
            return Kind::Error;
        }
        warning = true;
    }
    if let Some(be) = obj.get("buildError") {
        known = true;
        match build_error_kind(be) {
            Kind::Error => return Kind::Error,
            Kind::Warning => warning = true,
            _ => {}
        }
    }
    if let Some(oe) = obj.get("openErrors") {
        known = true;
        if !open_errors_are_warnings(oe) {
            return Kind::Error;
        }
        warning = true;
    }
    match (known, warning) {
        (false, _) => Kind::Output,
        (true, true) => Kind::Warning,
        (true, false) => Kind::Empty,
    }
}

/// True when this part is a compile or runtime error: the IDE stops the script
/// there, so no further output and no end marker will come. A buildError is
/// not one -- `DoCommand "BuildApp"` returns it as a value and the script goes on.
pub fn stops_script(envelope: &Value) -> bool {
    envelope
        .get("response")
        .and_then(|r| r.get("scriptError"))
        .is_some_and(script_error_has_error)
}

/// The reply for a script whose only part was the end marker: what the IDE
/// sends for `Print ""`.
pub fn empty(tag: &str) -> Value {
    json!({ "tag": tag, "response": {} })
}

/// Fold the parts of one answer into one envelope, so callers read
/// `response` as before. The primary part is chosen by weight -- an error over
/// output, output over a warning, a warning over an empty answer -- and among
/// outputs one that says something. The others' `response` values are kept
/// under `xmcp_parts`.
pub fn merge(mut frames: Vec<Value>) -> Option<Value> {
    if frames.len() <= 1 {
        return frames.pop();
    }
    let says_something = |f: &Value| f.get("response").and_then(Value::as_str) != Some("");
    let primary = [Kind::Error, Kind::Output, Kind::Warning, Kind::Empty, Kind::Unknown]
        .iter()
        .find_map(|&k| {
            frames
                .iter()
                .position(|f| kind(f) == k && (k != Kind::Output || says_something(f)))
        })
        // Only every part being an empty printed string gets here; that is
        // still the script's answer.
        .unwrap_or(0);
    let mut merged = frames.remove(primary);
    let parts: Vec<Value> = frames.into_iter().filter_map(|mut f| f.get_mut("response").map(Value::take)).collect();
    if !parts.is_empty()
        && let Some(obj) = merged.as_object_mut()
    {
        obj.insert("xmcp_parts".into(), Value::Array(parts));
    }
    Some(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(response: Value) -> Value {
        json!({ "tag": "xmcp_1", "response": response })
    }

    #[test]
    fn the_marker_is_appended_unless_a_continuation_would_swallow_it() {
        assert_eq!(with_end_marker("Print 1", "t"), "Print 1\nPrint \"xmcp-end:t\"");
        assert_eq!(with_end_marker("Dim x As Integer = 1 + _", "t"), "Dim x As Integer = 1 + _");
        assert!(with_end_marker("", "t").ends_with("Print \"xmcp-end:t\""));
    }

    #[test]
    fn only_a_real_continuation_counts() {
        assert!(ends_with_line_continuation("x = 1 + _"));
        assert!(ends_with_line_continuation("x = 1 +_\n\n  "));
        assert!(ends_with_line_continuation("_"));
        assert!(!ends_with_line_continuation("Var foo_ As Integer = 7\nfoo_"));
        assert!(!ends_with_line_continuation("Var x As Integer = 1 // rename to foo_"));
        assert!(!ends_with_line_continuation("x = 1 ' see bar_"));
        assert!(!ends_with_line_continuation("Rem _"));
        assert!(!ends_with_line_continuation("Print \"a // b _\""));
        assert!(ends_with_line_continuation("Print \"a // b\" + _"));
        assert!(!ends_with_line_continuation("Var größe_"), "non-ASCII counts as a name character");
    }

    #[test]
    fn classifies_reply_parts() {
        assert_eq!(kind(&part(json!("hello"))), Kind::Output);
        assert_eq!(kind(&part(json!(42))), Kind::Output);
        assert_eq!(kind(&part(json!({}))), Kind::Empty);
        assert_eq!(kind(&json!({ "tag": "x" })), Kind::Unknown);
        let warning = json!({ "scriptError": [{ "type": "scriptCompilerWarning", "message": "w" }] });
        assert_eq!(kind(&part(warning.clone())), Kind::Warning);
        let runtime = json!({ "scriptError": [{ "type": "scriptRuntimeError", "message": "" }] });
        assert_eq!(kind(&part(runtime)), Kind::Error);
        let untyped = json!({ "scriptError": [{ "message": "?" }] });
        assert_eq!(kind(&part(untyped)), Kind::Error);
        assert_eq!(kind(&part(json!({ "buildError": { "errors": [1] } }))), Kind::Error);
        assert_eq!(kind(&part(json!({ "buildError": { "warnings": [1] } }))), Kind::Warning);
        assert_eq!(kind(&part(json!({ "buildError": { "odd": [] } }))), Kind::Error);
        assert_eq!(kind(&part(json!({ "buildError": {} }))), Kind::Error);
        let old_project = json!({ "openErrors": [{ "loadError": { "type": "IDE Version Conflict", "severity": "warning" } }] });
        assert_eq!(kind(&part(old_project)), Kind::Warning);
        assert_eq!(kind(&part(json!({ "openErrors": [{ "loadError": {} }] }))), Kind::Error);
        assert_eq!(kind(&part(json!({ "missingFiles": [] }))), Kind::Error);
        assert_eq!(kind(&part(json!({ "something": 1 }))), Kind::Output);
    }

    #[test]
    fn only_a_script_error_stops_the_script() {
        let runtime = part(json!({ "scriptError": [{ "type": "scriptRuntimeError" }] }));
        assert!(stops_script(&runtime));
        let warning = part(json!({ "scriptError": [{ "type": "scriptCompilerWarning" }] }));
        assert!(!stops_script(&warning));
        assert!(!stops_script(&part(json!({ "buildError": { "errors": [1] } }))));
        assert!(!stops_script(&part(json!("out"))));
    }

    #[test]
    fn recognises_only_its_own_marker() {
        assert!(is_end_marker(&part(json!("xmcp-end:xmcp_1")), "xmcp_1"));
        assert!(!is_end_marker(&part(json!("xmcp-end:xmcp_2")), "xmcp_1"));
        assert!(!is_end_marker(&part(json!("XMCP-END:xmcp_1")), "xmcp_1"));
    }

    #[test]
    fn merge_picks_the_weightiest_part_and_keeps_the_rest() {
        assert_eq!(merge(vec![]), None);
        assert_eq!(merge(vec![part(json!("only"))]), Some(part(json!("only"))));

        let warning = json!({ "scriptError": [{ "type": "scriptCompilerWarning" }] });
        let merged = merge(vec![part(json!("first")), part(json!("second")), part(warning.clone())]).unwrap();
        assert_eq!(merged["response"], "first");
        assert_eq!(merged["xmcp_parts"], json!(["second", warning]));

        let error = json!({ "scriptError": [{ "type": "scriptRuntimeError" }] });
        let merged = merge(vec![part(json!("printed")), part(error.clone())]).unwrap();
        assert_eq!(merged["response"], error, "an error outranks output");

        let merged = merge(vec![part(json!("")), part(json!("said"))]).unwrap();
        assert_eq!(merged["response"], "said", "output that says something wins");

        let merged = merge(vec![part(json!({})), part(warning.clone())]).unwrap();
        assert_eq!(merged["response"], warning, "an empty answer never displaces a warning");

        let merged = merge(vec![part(json!("")), part(json!(""))]).unwrap();
        assert_eq!(merged["response"], "");
    }
}
