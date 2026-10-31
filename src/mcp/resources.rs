use serde_json::{Value, json};
use std::path::Path;

pub const USAGE_GUIDE_FALLBACK: &str = include_str!("../../usage-guide.md");

/// Load the usage guide from disk (next to the executable), falling back
/// to the compile-time embedded copy.
pub fn load_usage_guide(exe_dir: &Path) -> String {
    let path = exe_dir.join("usage-guide.md");
    std::fs::read_to_string(&path).unwrap_or_else(|_| USAGE_GUIDE_FALLBACK.to_string())
}

/// Build the resources/list response result.
pub fn resources_list(exe_dir: &Path) -> Value {
    let guide_path = exe_dir.join("usage-guide.md");
    let guide_exists = guide_path.exists() || !USAGE_GUIDE_FALLBACK.is_empty();

    let mut resources = Vec::new();
    if guide_exists {
        resources.push(json!({
            "uri": "file://usage-guide.md",
            "name": "xmcp Usage Guide",
            "description": "Guide for AI assistants: xmcp capabilities, limitations, and fallback strategies for direct file editing.",
            "mimeType": "text/markdown"
        }));
    }

    json!({ "resources": resources })
}

/// Build the resources/read response result for a given URI.
pub fn resources_read(uri: &str, exe_dir: &Path) -> Result<Value, String> {
    if uri != "file://usage-guide.md" {
        return Err(format!("Unknown resource URI: {uri}"));
    }

    let content = load_usage_guide(exe_dir);
    Ok(json!({
        "contents": [{
            "uri": "file://usage-guide.md",
            "mimeType": "text/markdown",
            "text": content
        }]
    }))
}
