use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

use crate::ide::communicator::Communicator;

/// JSON Schema types for tool parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    Array,
    Boolean,
    Integer,
    Number,
    Object,
    String,
}

impl ParamType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Array => "array",
            Self::Boolean => "boolean",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::Object => "object",
            Self::String => "string",
        }
    }
}

impl Serialize for ParamType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Describes a single parameter a tool accepts.
#[derive(Debug, Clone)]
pub struct ToolParam {
    pub name: &'static str,
    pub param_type: ParamType,
    pub description: &'static str,
    pub required: bool,
    pub default: Option<Value>,
}

/// Result of running a tool.
pub struct ToolResult {
    pub output: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
        }
    }

    pub fn failure(msg: impl Into<String>) -> Self {
        Self {
            output: msg.into(),
            is_error: true,
        }
    }
}

/// Context passed to every tool's run method.
pub struct ToolContext<'a> {
    pub ide: Option<&'a Communicator>,
    pub docs_path: Option<&'a Path>,
    #[allow(dead_code)]
    pub verbose: bool,
}

/// The trait all tools implement.
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> &[ToolParam];
    fn run(&self, args: &HashMap<String, Value>, ctx: &ToolContext) -> ToolResult;

    /// Whether this tool modifies the project (source code, items, or the
    /// saved-on-disk state). Read-only mode hides and rejects these tools.
    /// Defaults to `false`; mutating tools override it.
    fn mutates(&self) -> bool {
        false
    }
}

/// Infer the ParamType from a serde_json::Value.
pub fn type_from_value(v: &Value) -> ParamType {
    match v {
        Value::Array(_) => ParamType::Array,
        Value::String(_) => ParamType::String,
        Value::Bool(_) => ParamType::Boolean,
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                ParamType::Integer
            } else {
                ParamType::Number
            }
        }
        _ => ParamType::Object, // includes Null
    }
}

/// Check if argument type is compatible with parameter type.
fn compatible(param_type: ParamType, arg_type: ParamType) -> bool {
    if param_type == arg_type {
        return true;
    }
    // Number accepts Integer, but Integer does not accept Number.
    if param_type == ParamType::Number && arg_type == ParamType::Integer {
        return true;
    }
    false
}

/// Validate arguments against a tool's parameter definitions.
/// Returns Ok(()) if valid, Err(message) if invalid.
pub fn validate_arguments(
    tool: &dyn Tool,
    args: &HashMap<String, Value>,
) -> Result<(), String> {
    let params = tool.parameters();

    // Check required parameters are present and types match.
    // Null values are treated as absent (the parameter was not provided).
    for param in params {
        if let Some(value) = args.get(param.name)
            && !value.is_null()
        {
            let arg_type = type_from_value(value);
            if !compatible(param.param_type, arg_type) {
                return Err(format!(
                    "Wrong parameter type for parameter named `{}`. Expected {} but received {}.",
                    param.name,
                    param.param_type.as_str(),
                    arg_type.as_str()
                ));
            }
        } else if param.required {
            return Err(format!(
                "Missing the required `{}` parameter.",
                param.name
            ));
        }
    }

    // Check for unexpected parameters.
    let known_names: Vec<&str> = params.iter().map(|p| p.name).collect();
    let extra: Vec<&str> = args
        .keys()
        .filter(|k| !known_names.contains(&k.as_str()))
        .map(|k| k.as_str())
        .collect();
    if !extra.is_empty() {
        let quoted: Vec<String> = extra.iter().map(|n| format!("`{n}`")).collect();
        return Err(format!(
            "Unexpected parameters ({}) passed to `{}` tool.",
            quoted.join(", "),
            tool.name()
        ));
    }

    Ok(())
}

/// Serialize a tool's metadata to the JSON schema MCP expects.
pub fn tool_to_json(tool: &dyn Tool) -> Value {
    let params = tool.parameters();

    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for param in params {
        let mut prop = serde_json::Map::new();
        prop.insert(
            "type".into(),
            Value::String(param.param_type.as_str().into()),
        );
        prop.insert("description".into(), Value::String(param.description.into()));
        if let Some(ref default) = param.default {
            prop.insert("default".into(), default.clone());
        }
        properties.insert(param.name.into(), Value::Object(prop));

        if param.required {
            required.push(Value::String(param.name.into()));
        }
    }

    serde_json::json!({
        "name": tool.name(),
        "description": tool.description(),
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
        }
    })
}
