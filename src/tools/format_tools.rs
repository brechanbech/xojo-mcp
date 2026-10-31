//! scaffold_code_block and lint_project_file: write and check the `#tag` text
//! of `.xojo_code` and `.xojo_window` files without going through the IDE.
//!
//! Both take their rules from the JSON block under the "Format Rules" heading
//! of usage-guide.md, so a rule found wrong in the field is fixed by editing
//! the guide beside the binary, with no rebuild.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::mcp::resources::USAGE_GUIDE_FALLBACK;
use crate::mcp::tool::*;
use crate::tools::*;

// ---------------------------------------------------------------------------
// Format rules
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RawRules {
    flags: BTreeMap<String, FlagRule>,
    block_order: BlockOrder,
    constant_escape: BTreeMap<String, String>,
    constant_types: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct FlagRule {
    keyword: String,
    visibility: String,
}

#[derive(Debug, Deserialize)]
struct BlockOrder {
    xojo_code_class: Vec<Rank>,
    xojo_code_module: Vec<Rank>,
    xojo_window: Vec<Rank>,
}

/// One step of a block order: a single kind, or several kinds that may be
/// mixed in any order (the IDE interleaves Property and ComputedProperty).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Rank {
    One(String),
    Mixed(Vec<String>),
}

impl Rank {
    fn contains(&self, kind: &str) -> bool {
        match self {
            Rank::One(k) => k == kind,
            Rank::Mixed(ks) => ks.iter().any(|k| k == kind),
        }
    }

    fn label(&self) -> String {
        match self {
            Rank::One(k) => k.clone(),
            Rank::Mixed(ks) => ks.join("/"),
        }
    }
}

pub struct FormatRules {
    flags: BTreeMap<String, FlagRule>,
    block_order: BlockOrder,
    constant_escape: Vec<(char, String)>,
    constant_types: Vec<String>,
}

impl FormatRules {
    /// The rules from usage-guide.md beside the binary, or from the compiled-in
    /// guide when that file is missing or predates the Format Rules block. A
    /// block that is present but broken is an error, not a reason to fall back:
    /// someone edited it, and silently ignoring the edit would hide that.
    pub fn load(exe_dir: &Path) -> Result<Self, String> {
        let path = exe_dir.join("usage-guide.md");
        if let Ok(guide) = std::fs::read_to_string(&path)
            && let Some(block) = rules_block(&guide)
        {
            return Self::parse(block).map_err(|e| {
                format!(
                    "The Format Rules block in {} is invalid: {e}",
                    path.display()
                )
            });
        }
        let block = rules_block(USAGE_GUIDE_FALLBACK)
            .ok_or("The built-in usage guide has no Format Rules block.")?;
        Self::parse(block).map_err(|e| format!("The built-in Format Rules block is invalid: {e}"))
    }

    fn parse(json: &str) -> Result<Self, String> {
        let raw: RawRules = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut constant_escape = Vec::new();
        for (key, replacement) in raw.constant_escape {
            let mut chars = key.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => constant_escape.push((c, replacement)),
                _ => {
                    return Err(format!(
                        "constant_escape key {key:?} is not a single character"
                    ));
                }
            }
        }
        Ok(Self {
            flags: raw.flags,
            block_order: raw.block_order,
            constant_escape,
            constant_types: raw.constant_types,
        })
    }

    /// The `(flag, keyword)` pair for a visibility name such as "private".
    fn flag_for(&self, visibility: &str) -> Option<(&str, &str)> {
        self.flags
            .iter()
            .find(|(_, rule)| rule.visibility.eq_ignore_ascii_case(visibility.trim()))
            .map(|(flag, rule)| (flag.as_str(), rule.keyword.as_str()))
    }

    fn visibilities(&self) -> Vec<&str> {
        self.flags.values().map(|r| r.visibility.as_str()).collect()
    }

    /// Escape a raw value for the inside of `Default = \"…"`. The caller
    /// writes the field's own quotes.
    fn escape_default(&self, raw: &str) -> Result<String, String> {
        // Every line break becomes \n; the table's "\r" entry only exists to
        // drop the CR half of a CRLF, which would lose a lone CR.
        let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
        let mut out = String::new();
        for ch in normalized.chars() {
            if ch == '"' {
                out.push_str("\\\"");
            } else if let Some((_, rep)) = self.constant_escape.iter().find(|(c, _)| *c == ch) {
                out.push_str(rep);
            } else if ch == '\\' || ch.is_ascii_control() {
                return Err(format!(
                    "The default value contains {}, and how a #tag Constant line escapes that \
                     has not been confirmed. Create the constant in the IDE and set its value \
                     with constant_value, so Xojo does the escaping.",
                    if ch == '\\' {
                        "a backslash".to_string()
                    } else {
                        format!("the control character U+{:04X}", ch as u32)
                    }
                ));
            } else if ch.is_ascii() {
                out.push(ch);
            } else {
                // UTF-8 bytes as \xHH each, never \uXXXX.
                let mut buf = [0u8; 4];
                for b in ch.encode_utf8(&mut buf).bytes() {
                    let _ = write!(out, "\\x{b:02X}");
                }
            }
        }
        Ok(out)
    }
}

/// The JSON inside the first ```json fence after the heading that names the
/// Format Rules.
fn rules_block(guide: &str) -> Option<&str> {
    let mut offset = 0;
    let mut start = None;
    for line in guide.split_inclusive('\n') {
        offset += line.len();
        if line.starts_with('#') && line.contains("Format Rules") {
            start = Some(offset);
            break;
        }
    }
    let rest = &guide[start?..];
    let body = &rest[rest.find("```json")? + "```json".len()..];
    Some(body[..body.find("\n```")?].trim())
}

// ---------------------------------------------------------------------------
// scaffold_code_block
// ---------------------------------------------------------------------------
pub struct ScaffoldCodeBlock;

const BLOCK_KINDS: &[&str] = &[
    "method",
    "shared_method",
    "property",
    "constant",
    "event_definition",
    "control_event",
    "window_event",
];

/// Whether an optional argument means anything for a block kind. Passing one
/// that doesn't is refused rather than silently dropped.
fn kind_takes(kind: &str, arg: &str) -> bool {
    match arg {
        "visibility" => matches!(kind, "method" | "shared_method" | "property" | "constant"),
        "signature" => matches!(
            kind,
            "method" | "shared_method" | "event_definition" | "control_event" | "window_event"
        ),
        "body" => matches!(
            kind,
            "method" | "shared_method" | "control_event" | "window_event"
        ),
        "event" => kind == "control_event",
        "type" => matches!(kind, "property" | "constant"),
        "default_value" => kind == "constant",
        _ => true,
    }
}

struct ScaffoldRequest<'a> {
    kind: &'a str,
    name: &'a str,
    visibility: &'a str,
    signature: &'a str,
    event: &'a str,
    type_name: &'a str,
    default_value: &'a str,
    body: &'a str,
}

impl Tool for ScaffoldCodeBlock {
    fn name(&self) -> &'static str {
        "scaffold_code_block"
    }
    fn annotations(&self) -> ToolAnnotations {
        ToolAnnotations::read_only("Scaffold Code Block")
    }
    fn description(&self) -> &'static str {
        "Generates a correctly formatted #tag block for a .xojo_code or .xojo_window file: \
         a method, shared method, property, constant, event definition (#tag Hook), control \
         event handler (#tag Events block) or window event handler. Use it instead of writing \
         #tag syntax from memory when editing a project file on disk: it pairs each Flags value \
         with its visibility keyword, escapes a constant's Default value, and indents the body \
         the way the IDE does. It only returns text; insert it yourself, in the block order \
         usage-guide.md's Format Rules give and before #tag ViewBehavior, then check the file \
         with lint_project_file. A control_event result is a whole #tag Events block: if the \
         control already has one, insert just its inner #tag Event … #tag EndEvent. An \
         argument that does not apply to block_kind, a name that is not a Xojo identifier, or \
         a constant value it cannot escape safely is an error. Reads no files and needs no \
         IDE."
    }
    fn parameters(&self) -> &[ToolParam] {
        static P: &[ToolParam] = &[
            ToolParam {
                name: "block_kind",
                param_type: ParamType::String,
                description: "method, shared_method, property, constant, event_definition, \
                              control_event, or window_event",
                required: true,
                default: None,
            },
            ToolParam {
                name: "name",
                param_type: ParamType::String,
                description: "The member's name; for control_event, the control's name \
                              (e.g. Button1); for window_event, the event's name (e.g. Opening)",
                required: true,
                default: None,
            },
            ToolParam {
                name: "visibility",
                param_type: ParamType::String,
                description: "public, protected or private (method, shared_method, property, \
                              constant); default public. In a module, public means global",
                required: false,
                default: None,
            },
            ToolParam {
                name: "signature",
                param_type: ParamType::String,
                description: "Parameter list and optional return type, e.g. \
                              '(count As Integer) As String'. A return type makes it a Function. \
                              Defaults to '()'",
                required: false,
                default: None,
            },
            ToolParam {
                name: "event",
                param_type: ParamType::String,
                description: "control_event only: the event's name, e.g. Pressed",
                required: false,
                default: None,
            },
            ToolParam {
                name: "type",
                param_type: ParamType::String,
                description: "property: its data type (required). constant: String, Integer, \
                              Double, Boolean or Color (default String)",
                required: false,
                default: None,
            },
            ToolParam {
                name: "default_value",
                param_type: ParamType::String,
                description: "constant only: the raw, unescaped value; this tool escapes it",
                required: false,
                default: None,
            },
            ToolParam {
                name: "body",
                param_type: ParamType::String,
                description: "Code for a method or event handler, one statement per line; \
                              indent nested blocks with two spaces. Omit for an empty body",
                required: false,
                default: None,
            },
        ];
        P
    }
    fn run(&self, args: &HashMap<String, Value>, ctx: &ToolContext) -> ToolResult {
        let kind = arg_str(args, "block_kind", "");
        if !BLOCK_KINDS.contains(&kind) {
            return ToolResult::failure(format!(
                "Unknown block_kind `{kind}`. Valid values: {}.",
                BLOCK_KINDS.join(", ")
            ));
        }
        for arg in [
            "visibility",
            "signature",
            "event",
            "type",
            "default_value",
            "body",
        ] {
            if args.get(arg).is_some_and(|v| !v.is_null()) && !kind_takes(kind, arg) {
                return ToolResult::failure(format!(
                    "`{arg}` does not apply to block_kind `{kind}`."
                ));
            }
        }
        let rules = match FormatRules::load(ctx.exe_dir) {
            Ok(r) => r,
            Err(e) => return ToolResult::failure(e),
        };
        let request = ScaffoldRequest {
            kind,
            name: arg_str(args, "name", ""),
            visibility: arg_str(args, "visibility", ""),
            signature: arg_str(args, "signature", ""),
            event: arg_str(args, "event", ""),
            type_name: arg_str(args, "type", ""),
            default_value: arg_str(args, "default_value", ""),
            body: arg_str(args, "body", ""),
        };
        match scaffold(&request, &rules) {
            Ok(block) => ToolResult::success(block),
            Err(e) => ToolResult::failure(e),
        }
    }
}

fn scaffold(req: &ScaffoldRequest, rules: &FormatRules) -> Result<String, String> {
    check_identifier("name", req.name)?;
    let visibility = if req.visibility.trim().is_empty() {
        "public"
    } else {
        req.visibility
    };
    let (flag, keyword) = rules.flag_for(visibility).ok_or_else(|| {
        format!(
            "Unknown visibility `{visibility}`. Valid values: {}.",
            rules.visibilities().join(", ")
        )
    })?;
    let keyword = if keyword.is_empty() {
        String::new()
    } else {
        format!("{keyword} ")
    };
    let name = req.name;

    let lines: Vec<String> = match req.kind {
        "method" | "shared_method" => {
            let shared = if req.kind == "shared_method" {
                "Shared "
            } else {
                ""
            };
            let (head, end) = routine(name, req.signature)?;
            let mut v = vec![
                format!("\t#tag Method, Flags = {flag}"),
                format!("\t\t{keyword}{shared}{head}"),
            ];
            v.extend(body_lines(req.body));
            v.push(format!("\t\t{end}"));
            v.push("\t#tag EndMethod".into());
            v
        }
        "property" => {
            let type_name = req.type_name.trim();
            if type_name.is_empty() {
                return Err("A property needs a `type`, e.g. Integer or Dictionary.".into());
            }
            vec![
                format!("\t#tag Property, Flags = {flag}"),
                format!("\t\t{keyword}{name} As {type_name}"),
                "\t#tag EndProperty".into(),
            ]
        }
        "constant" => {
            let wanted = if req.type_name.trim().is_empty() {
                "String"
            } else {
                req.type_name.trim()
            };
            let type_name = rules
                .constant_types
                .iter()
                .find(|t| t.eq_ignore_ascii_case(wanted))
                .ok_or_else(|| {
                    format!(
                        "Unknown constant type `{wanted}`. Valid values: {}.",
                        rules.constant_types.join(", ")
                    )
                })?;
            let value = rules.escape_default(req.default_value)?;
            let scope = capitalize(visibility.trim());
            vec![
                format!(
                    "\t#tag Constant, Name = {name}, Type = {type_name}, Dynamic = False, \
                     Default = \\\"{value}\", Scope = {scope}"
                ),
                "\t#tag EndConstant".into(),
            ]
        }
        "event_definition" => {
            // An event definition has no scope of its own; the IDE writes the
            // public flag and no keyword.
            let (public_flag, _) = rules
                .flag_for("public")
                .ok_or("The Format Rules have no public flag.")?;
            let (params, returns) = split_signature(req.signature)?;
            let returns = returns.map(|r| format!(" As {r}")).unwrap_or_default();
            vec![
                format!("\t#tag Hook, Flags = {public_flag}"),
                format!("\t\tEvent {name}{params}{returns}"),
                "\t#tag EndHook".into(),
            ]
        }
        "control_event" => {
            check_identifier("event", req.event)?;
            let (head, end) = routine(req.event, req.signature)?;
            let mut v = vec![
                format!("#tag Events {name}"),
                "\t#tag Event".into(),
                format!("\t\t{head}"),
            ];
            v.extend(body_lines(req.body));
            v.push(format!("\t\t{end}"));
            v.push("\t#tag EndEvent".into());
            v.push("#tag EndEvents".into());
            v
        }
        "window_event" => {
            let (head, end) = routine(name, req.signature)?;
            let mut v = vec!["\t#tag Event".into(), format!("\t\t{head}")];
            v.extend(body_lines(req.body));
            v.push(format!("\t\t{end}"));
            v.push("\t#tag EndEvent".into());
            v
        }
        other => return Err(format!("Unknown block_kind `{other}`.")),
    };
    Ok(lines.join("\n"))
}

/// The declaration line and closing line of a Sub or Function, e.g.
/// ("Function Count(x As Integer) As String", "End Function").
fn routine(name: &str, signature: &str) -> Result<(String, &'static str), String> {
    let (params, returns) = split_signature(signature)?;
    Ok(match returns {
        Some(r) => (format!("Function {name}{params} As {r}"), "End Function"),
        None => (format!("Sub {name}{params}"), "End Sub"),
    })
}

/// Split "(a As Integer) As String()" into ("(a As Integer)", Some("String()")).
/// The parameter list ends at the parenthesis matching the first, so an array
/// return type keeps its own.
fn split_signature(signature: &str) -> Result<(String, Option<String>), String> {
    let s = signature.trim();
    if s.is_empty() {
        return Ok(("()".into(), None));
    }
    if !s.starts_with('(') {
        return Err(format!(
            "The signature `{s}` must start with its parameter list, e.g. '(x As Integer) As String' or '()'."
        ));
    }
    let mut depth = 0usize;
    let mut close = None;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close.ok_or_else(|| format!("The signature `{s}` has an unclosed parenthesis."))?;
    let params = s[..=close].to_string();
    let rest = s[close + 1..].trim();
    if rest.is_empty() {
        return Ok((params, None));
    }
    let returns = rest
        .get(..3)
        .filter(|p| p.eq_ignore_ascii_case("as "))
        .map(|_| rest[3..].trim())
        .filter(|r| !r.is_empty())
        .ok_or_else(|| {
            format!(
                "After the parameter list, the signature `{s}` can only have `As <return type>`."
            )
        })?;
    Ok((params, Some(returns.to_string())))
}

/// Body lines as the IDE indents them: two tabs, then two spaces, with blank
/// lines keeping the two spaces too.
fn body_lines(body: &str) -> Vec<String> {
    body.replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(|l| format!("\t\t  {l}"))
        .collect()
}

fn check_identifier(arg: &str, value: &str) -> Result<(), String> {
    let mut chars = value.chars();
    let ok = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(format!("`{arg}` must be a Xojo identifier, not `{value}`."))
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(|c| c.to_lowercase()))
            .collect(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// lint_project_file
// ---------------------------------------------------------------------------
pub struct LintProjectFile;

impl Tool for LintProjectFile {
    fn name(&self) -> &'static str {
        "lint_project_file"
    }
    fn annotations(&self) -> ToolAnnotations {
        ToolAnnotations::read_only("Lint Project File")
    }
    fn description(&self) -> &'static str {
        "Checks a .xojo_code or .xojo_window file on disk for mistakes the IDE reports \
         badly or not at all. Errors are unbalanced #tag/#tag End pairs, which break the \
         file. Warnings are a constant Default value with characters left unescaped, which \
         can silently truncate it, and things Xojo loads but resolves on its own terms: \
         blocks out of order or after #tag ViewBehavior, and a Flags value that disagrees \
         with the visibility keyword, where the warning says which visibility Xojo will use. \
         Run it after editing a project file directly and before revert_project. Returns \
         'No issues found' or a count and one 'error|warning: line <n>: <message>' per \
         finding; findings, errors included, are a normal result, and only a file it cannot \
         read or one of another type fails the call. It reads the file only and needs no IDE."
    }
    fn parameters(&self) -> &[ToolParam] {
        static P: &[ToolParam] = &[ToolParam {
            name: "path",
            param_type: ParamType::String,
            description: "Absolute path to the .xojo_code or .xojo_window file; \
                          get_project_info gives the project directory",
            required: true,
            default: None,
        }];
        P
    }
    fn run(&self, args: &HashMap<String, Value>, ctx: &ToolContext) -> ToolResult {
        let path_arg = arg_str(args, "path", "");
        let path = Path::new(path_arg);
        if !path.is_absolute() {
            return ToolResult::failure(format!(
                "`path` must be absolute, not `{path_arg}`. get_project_info gives the project directory."
            ));
        }
        let is_window = match path.extension().and_then(|e| e.to_str()) {
            Some(e) if e.eq_ignore_ascii_case("xojo_window") => true,
            Some(e) if e.eq_ignore_ascii_case("xojo_code") => false,
            _ => {
                return ToolResult::failure(
                    "lint_project_file checks .xojo_code and .xojo_window files only.",
                );
            }
        };
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => return ToolResult::failure(format!("Could not read {path_arg}: {e}")),
        };
        let rules = match FormatRules::load(ctx.exe_dir) {
            Ok(r) => r,
            Err(e) => return ToolResult::failure(e),
        };
        ToolResult::success(report(path_arg, &lint(&text, is_window, &rules)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Severity {
    Error,
    Warning,
}

#[derive(Debug)]
struct Diagnostic {
    line: usize,
    severity: Severity,
    message: String,
}

fn report(path: &str, diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return format!("No issues found in {path}.");
    }
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = diagnostics.len() - errors;
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    let mut out = format!(
        "{}, {} in {path}:",
        plural(errors, "error"),
        plural(warnings, "warning")
    );
    for d in diagnostics {
        let label = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let _ = write!(out, "\n{label}: line {}: {}", d.line, d.message);
    }
    out
}

/// The `#tag` on a trimmed line as (kind, closing): "#tag Method, Flags = &h0"
/// is ("Method", false) and "#tag EndMethod" is ("Method", true). The match is
/// case-sensitive on purpose: the "#Tag Instance" lines Xojo writes inside a
/// constant for per-platform values are not blocks and have no end tag.
fn tag_of(trimmed: &str) -> Option<(&str, bool)> {
    let rest = trimmed.strip_prefix("#tag ")?;
    let word = rest.split([',', ' ']).next()?.trim();
    if word.is_empty() {
        return None;
    }
    match word.strip_prefix("End") {
        Some(kind) if !kind.is_empty() => Some((kind, true)),
        _ => Some((word, false)),
    }
}

fn lint(text: &str, is_window: bool, rules: &FormatRules) -> Vec<Diagnostic> {
    let lines: Vec<&str> = text.lines().collect();
    let in_note = note_mask(&lines);
    let mut out = Vec::new();
    check_structure(&lines, &in_note, is_window, rules, &mut out);
    check_flags(&lines, &in_note, rules, &mut out);
    check_constants(&lines, &in_note, rules, &mut out);
    out.sort_by_key(|d| (d.line, d.severity));
    out
}

/// True for each line in the body of a `#tag Note`. A note is free text and
/// often quotes #tag syntax as an example, which is not structure.
fn note_mask(lines: &[&str]) -> Vec<bool> {
    let mut inside = false;
    lines
        .iter()
        .map(|line| {
            let trimmed = line.trim();
            if inside {
                if trimmed == "#tag EndNote" {
                    inside = false;
                    return false;
                }
                return true;
            }
            if tag_of(trimmed) == Some(("Note", false)) {
                inside = true;
            }
            false
        })
        .collect()
}

/// Balance of #tag/#tag End pairs, and the order of the blocks at member
/// level: inside the #tag Class or #tag Module of a .xojo_code file, at the
/// top of a .xojo_window.
fn check_structure(
    lines: &[&str],
    in_note: &[bool],
    is_window: bool,
    rules: &FormatRules,
    out: &mut Vec<Diagnostic>,
) {
    let is_module = !is_window
        && lines
            .iter()
            .find_map(|l| tag_of(l.trim()))
            .is_some_and(|(kind, closing)| kind == "Module" && !closing);
    let order = match (is_window, is_module) {
        (true, _) => &rules.block_order.xojo_window,
        (false, true) => &rules.block_order.xojo_code_module,
        (false, false) => &rules.block_order.xojo_code_class,
    };
    let order_text = order
        .iter()
        .map(Rank::label)
        .collect::<Vec<_>>()
        .join(" → ");
    let member_depth = if is_window { 0 } else { 1 };

    let mut stack: Vec<(&str, usize)> = Vec::new();
    let mut highest_rank: Option<usize> = None;
    let mut view_behavior_at: Option<usize> = None;
    let mut misplaced: Vec<(&str, usize, usize)> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if in_note[i] {
            continue;
        }
        let Some((kind, closing)) = tag_of(line.trim()) else {
            continue;
        };
        let n = i + 1;

        if closing {
            match stack.pop() {
                None => out.push(error(
                    n,
                    format!("`#tag End{kind}` has no opening `#tag {kind}`."),
                )),
                Some((open, at)) if open != kind => out.push(error(
                    n,
                    format!("`#tag End{kind}` closes `#tag {open}`, opened at line {at}."),
                )),
                Some(_) => {}
            }
            continue;
        }

        // No block nests inside one of its own kind, so a second opening
        // means the first lost its end tag. Blame that one and resync, rather
        // than let every later end tag close the wrong block.
        if let Some(pos) = stack.iter().rposition(|(open, _)| *open == kind) {
            out.push(error(
                stack[pos].1,
                format!("`#tag {kind}` has no `#tag End{kind}` before the next `#tag {kind}` at line {n}."),
            ));
            stack.truncate(pos);
        }

        // Order is a warning: Xojo 2026r3 loads misordered members, even ones
        // after ViewBehavior, and re-sorts them on the next save.
        if stack.len() == member_depth {
            if let Some(vb) = view_behavior_at {
                out.push(warning(
                    n,
                    format!(
                        "`#tag {kind}` comes after `#tag ViewBehavior` (line {vb}), which goes last. \
                         Xojo loads it and moves it on the next save."
                    ),
                ));
            } else if kind == "ViewBehavior" {
                view_behavior_at = Some(n);
            } else if let Some(rank) = order.iter().position(|r| r.contains(kind)) {
                match highest_rank {
                    Some(h) if rank < h => {
                        match misplaced.iter_mut().find(|(k, _, _)| *k == kind) {
                            Some((_, _, count)) => *count += 1,
                            None => misplaced.push((kind, n, 1)),
                        }
                    }
                    _ => highest_rank = Some(rank),
                }
            }
        }
        stack.push((kind, n));
    }

    // One warning per misplaced kind, at its first block: a file laid out in
    // another order would otherwise repeat the same warning for every member.
    for (kind, first, count) in misplaced {
        let more = match count {
            1 => String::new(),
            2 => " (and 1 more)".to_string(),
            c => format!(" (and {} more)", c - 1),
        };
        out.push(warning(
            first,
            format!(
                "`#tag {kind}`{more} is out of order; blocks go {order_text}. Xojo loads it and \
                 re-sorts on the next save."
            ),
        ));
    }

    for (kind, at) in stack {
        out.push(error(
            at,
            format!("`#tag {kind}` is never closed with `#tag End{kind}`."),
        ));
    }
}

/// A method or property's Flags value against the visibility keyword that
/// starts its declaration on the next line. A mismatch loads, so it is a
/// warning, but the result depends on which side is present: tested on Xojo
/// 2026r3, a declaration with a keyword wins and the Flags are rewritten on
/// save, while one without a keyword takes the Flags' visibility.
fn check_flags(lines: &[&str], in_note: &[bool], rules: &FormatRules, out: &mut Vec<Diagnostic>) {
    for (i, line) in lines.iter().enumerate() {
        if in_note[i] {
            continue;
        }
        let trimmed = line.trim();
        if !(trimmed.starts_with("#tag Method,") || trimmed.starts_with("#tag Property,")) {
            continue;
        }
        let Some(at) = trimmed.find("Flags = ") else {
            continue;
        };
        let flag = trimmed[at + "Flags = ".len()..]
            .split(',')
            .next()
            .unwrap_or("")
            .trim();
        let Some(rule) = rules.flags.get(flag) else {
            continue;
        };
        let Some(decl) = lines.get(i + 1).map(|l| l.trim()) else {
            continue;
        };

        let declared = rules
            .flags
            .values()
            .find(|r| !r.keyword.is_empty() && decl.starts_with(&format!("{} ", r.keyword)));
        match declared {
            Some(d) if d.keyword == rule.keyword => {}
            Some(d) => out.push(warning(
                i + 2,
                format!(
                    "the declaration says `{}` but Flags = {flag} is {}. Xojo follows the keyword, \
                     making this {}, and rewrites the Flags on the next save.",
                    d.keyword, rule.visibility, d.visibility
                ),
            )),
            None if !rule.keyword.is_empty() => out.push(warning(
                i + 2,
                format!(
                    "Flags = {flag} is {} but the declaration has no `{}`. Xojo follows the Flags, \
                     making this {}, and adds the keyword on the next save.",
                    rule.visibility, rule.keyword, rule.visibility
                ),
            )),
            None => {}
        }
    }
}

/// Characters left unescaped in a constant's `Default = \"…"` value.
fn check_constants(
    lines: &[&str],
    in_note: &[bool],
    rules: &FormatRules,
    out: &mut Vec<Diagnostic>,
) {
    const OPEN: &str = "Default = \\\"";
    // The value runs to the last `", Scope = ` on the line, not the first
    // unescaped quote: a stray quote in the middle is exactly what this looks
    // for, so it must not end the scan. Scope follows Default on every
    // constant line, and only a hex Description can follow Scope.
    const CLOSE: &str = "\", Scope = ";

    for (i, line) in lines.iter().enumerate() {
        if in_note[i] {
            continue;
        }
        let Some(rest) = line.trim().strip_prefix("#tag Constant,") else {
            continue;
        };
        let n = i + 1;
        let Some(at) = rest.find(OPEN) else {
            if rest.contains("Default = \"") {
                out.push(warning(
                    n,
                    "the Default value opens with a raw `\"` instead of `\\\"`. Xojo compiles it, \
                     but reads the value back without its first character. Re-enter the value \
                     with constant_value or in the IDE."
                        .into(),
                ));
            }
            continue;
        };
        let value_and_rest = &rest[at + OPEN.len()..];
        let Some(end) = value_and_rest.rfind(CLOSE) else {
            continue;
        };

        let mut seen: Vec<char> = Vec::new();
        let mut chars = value_and_rest[..end].chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                chars.next();
                continue;
            }
            let expected = if ch == '"' {
                Some("\\\"".to_string())
            } else if let Some((_, rep)) = rules
                .constant_escape
                .iter()
                .find(|(c, rep)| *c == ch && !rep.is_empty())
            {
                Some(rep.clone())
            } else if !ch.is_ascii() {
                Some("its UTF-8 bytes as \\xHH".to_string())
            } else {
                None
            };
            if let Some(expected) = expected
                && !seen.contains(&ch)
            {
                seen.push(ch);
                out.push(warning(
                    n,
                    format!(
                        "unescaped `{ch}` in the Default value; write it as {}.",
                        quote_code(&expected)
                    ),
                ));
            }
        }
    }
}

fn quote_code(s: &str) -> String {
    if s.starts_with('\\') {
        format!("`{s}`")
    } else {
        s.to_string()
    }
}

fn error(line: usize, message: String) -> Diagnostic {
    Diagnostic {
        line,
        severity: Severity::Error,
        message,
    }
}

fn warning(line: usize, message: String) -> Diagnostic {
    Diagnostic {
        line,
        severity: Severity::Warning,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> FormatRules {
        FormatRules::load(Path::new("/nonexistent")).expect("built-in rules parse")
    }

    fn req<'a>(kind: &'a str, name: &'a str) -> ScaffoldRequest<'a> {
        ScaffoldRequest {
            kind,
            name,
            visibility: "",
            signature: "",
            event: "",
            type_name: "",
            default_value: "",
            body: "",
        }
    }

    fn lint_messages(text: &str, is_window: bool) -> Vec<String> {
        lint(text, is_window, &rules())
            .into_iter()
            .map(|d| format!("{:?} {}: {}", d.severity, d.line, d.message))
            .collect()
    }

    #[test]
    fn the_built_in_guide_carries_complete_rules() {
        let r = rules();
        assert_eq!(r.flag_for("private"), Some(("&h21", "Private")));
        assert_eq!(r.flag_for("Public"), Some(("&h0", "")));
        assert!(r.constant_types.iter().any(|t| t == "Color"));
    }

    #[test]
    fn a_guide_on_disk_without_the_block_falls_back_to_the_built_in_rules() {
        let dir = std::env::temp_dir().join(format!("xmcp-rules-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("usage-guide.md"), "# An older guide\n").unwrap();
        assert!(FormatRules::load(&dir).is_ok());

        std::fs::write(
            dir.join("usage-guide.md"),
            "### Format Rules\n```json\n{\"flags\": 1}\n```\n",
        )
        .unwrap();
        let err = FormatRules::load(&dir)
            .err()
            .expect("an edited, broken block is reported");
        assert!(err.contains("is invalid"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn default_values_are_escaped_like_the_ide() {
        let r = rules();
        assert_eq!(
            r.escape_default("a,b=c'd\"e").unwrap(),
            "a\\x2Cb\\x3Dc\\'d\\\"e"
        );
        assert_eq!(r.escape_default("20°").unwrap(), "20\\xC2\\xB0");
        assert_eq!(
            r.escape_default("one\r\ntwo\rthree").unwrap(),
            "one\\ntwo\\nthree"
        );
        assert!(r.escape_default("C:\\temp").is_err());
        assert!(r.escape_default("tab\there").is_err());
    }

    #[test]
    fn a_return_type_makes_a_function() {
        assert_eq!(split_signature("").unwrap(), ("()".into(), None));
        assert_eq!(
            split_signature("(a As Integer, b() As String) As String()").unwrap(),
            (
                "(a As Integer, b() As String)".into(),
                Some("String()".into())
            )
        );
        assert!(split_signature("x As Integer").is_err());
        assert!(split_signature("(x As Integer").is_err());
        assert!(split_signature("() Returns Integer").is_err());
    }

    #[test]
    fn scaffolds_a_private_function_with_a_body() {
        let r = rules();
        let mut q = req("method", "Total");
        q.visibility = "private";
        q.signature = "(items() As Integer) As Integer";
        q.body = "Var sum As Integer\n\nFor Each i As Integer In items\n  sum = sum + i\nNext\nReturn sum";
        assert_eq!(
            scaffold(&q, &r).unwrap(),
            "\t#tag Method, Flags = &h21\n\
             \t\tPrivate Function Total(items() As Integer) As Integer\n\
             \t\t  Var sum As Integer\n\
             \t\t  \n\
             \t\t  For Each i As Integer In items\n\
             \t\t    sum = sum + i\n\
             \t\t  Next\n\
             \t\t  Return sum\n\
             \t\tEnd Function\n\
             \t#tag EndMethod"
        );
    }

    #[test]
    fn scaffolds_a_protected_shared_sub() {
        let mut q = req("shared_method", "Reset");
        q.visibility = "protected";
        assert_eq!(
            scaffold(&q, &rules()).unwrap(),
            "\t#tag Method, Flags = &h1\n\t\tProtected Shared Sub Reset()\n\t\tEnd Sub\n\t#tag EndMethod"
        );
    }

    #[test]
    fn scaffolds_a_constant_with_an_escaped_default() {
        let mut q = req("constant", "kGreeting");
        q.default_value = "Hello, \"world\"";
        q.visibility = "private";
        assert_eq!(
            scaffold(&q, &rules()).unwrap(),
            "\t#tag Constant, Name = kGreeting, Type = String, Dynamic = False, \
             Default = \\\"Hello\\x2C \\\"world\\\"\", Scope = Private\n\t#tag EndConstant"
        );
        q.type_name = "Decimal";
        assert!(
            scaffold(&q, &rules())
                .unwrap_err()
                .contains("Unknown constant type")
        );
    }

    #[test]
    fn a_property_needs_a_type() {
        let mut q = req("property", "Count");
        assert!(scaffold(&q, &rules()).is_err());
        q.type_name = "Integer";
        assert_eq!(
            scaffold(&q, &rules()).unwrap(),
            "\t#tag Property, Flags = &h0\n\t\tCount As Integer\n\t#tag EndProperty"
        );
    }

    #[test]
    fn scaffolds_event_blocks() {
        let r = rules();
        let mut hook = req("event_definition", "CountChanged");
        hook.signature = "(newCount As Integer)";
        assert_eq!(
            scaffold(&hook, &r).unwrap(),
            "\t#tag Hook, Flags = &h0\n\t\tEvent CountChanged(newCount As Integer)\n\t#tag EndHook"
        );

        let mut control = req("control_event", "Button1");
        control.event = "Pressed";
        control.body = "UpdateLabel(TextField1.Text)";
        assert_eq!(
            scaffold(&control, &r).unwrap(),
            "#tag Events Button1\n\t#tag Event\n\t\tSub Pressed()\n\t\t  UpdateLabel(TextField1.Text)\n\
             \t\tEnd Sub\n\t#tag EndEvent\n#tag EndEvents"
        );
        assert!(
            scaffold(&req("control_event", "Button1"), &r).is_err(),
            "event is required"
        );

        let mut key = req("window_event", "KeyDown");
        key.signature = "(key As String) As Boolean";
        assert_eq!(
            scaffold(&key, &r).unwrap(),
            "\t#tag Event\n\t\tFunction KeyDown(key As String) As Boolean\n\t\tEnd Function\n\t#tag EndEvent"
        );
    }

    #[test]
    fn names_must_be_identifiers() {
        assert!(scaffold(&req("method", "Do It"), &rules()).is_err());
        assert!(scaffold(&req("method", "1st"), &rules()).is_err());
        assert!(scaffold(&req("method", ""), &rules()).is_err());
    }

    #[test]
    fn arguments_that_do_not_apply_are_named() {
        assert!(!kind_takes("property", "body"));
        assert!(!kind_takes("method", "default_value"));
        assert!(kind_takes("control_event", "event"));
        assert!(!kind_takes("window_event", "event"));
    }

    const CLASS: &str = "#tag Class\n\
Protected Class Counter\n\
\t#tag Event\n\
\t\tSub Opening()\n\
\t\tEnd Sub\n\
\t#tag EndEvent\n\
\n\
\t#tag Method, Flags = &h0\n\
\t\tSub Increment()\n\
\t\t  mCount = mCount + 1\n\
\t\tEnd Sub\n\
\t#tag EndMethod\n\
\n\
\t#tag Note, Name = DesignNotes\n\
\t\tExample text, not structure:\n\
\t\t#tag Constant, Name = kX, Type = String, Dynamic = False, Default = \"raw, value\", Scope = Public\n\
\t#tag EndNote\n\
\n\
\t#tag ComputedProperty, Flags = &h0\n\
\t\t#tag Getter\n\
\t\t\tGet\n\
\t\t\t  Return mCount\n\
\t\t\tEnd Get\n\
\t\t#tag EndGetter\n\
\t\tCount As Integer\n\
\t#tag EndComputedProperty\n\
\n\
\t#tag Property, Flags = &h21\n\
\t\tPrivate mCount As Integer\n\
\t#tag EndProperty\n\
\n\
\t#tag Constant, Name = kEditClear, Type = String, Dynamic = False, Default = \\\"&Delete\", Scope = Public\n\
\t\t#Tag Instance, Platform = Windows, Language = Default, Definition  = \\\"&Delete\"\n\
\t#tag EndConstant\n\
\n\
\t#tag Constant, Name = kTimeout, Type = Double, Dynamic = False, Default = \\\"300000\", Scope = Private, Description = 486F77\n\
\t#tag EndConstant\n\
\n\
\t#tag ViewBehavior\n\
\t\t#tag ViewProperty\n\
\t\t\tName=\"Name\"\n\
\t\t#tag EndViewProperty\n\
\t#tag EndViewBehavior\n\
End Class\n\
#tag EndClass\n";

    #[test]
    fn an_ide_shaped_class_is_clean() {
        assert_eq!(lint_messages(CLASS, false), Vec::<String>::new());
        assert_eq!(
            lint_messages(&CLASS.replace('\n', "\r\n"), false),
            Vec::<String>::new()
        );
    }

    #[test]
    fn flags_blocks_out_of_order_and_after_view_behavior() {
        let moved = CLASS.replace(
            "\t#tag Event\n\t\tSub Opening()\n\t\tEnd Sub\n\t#tag EndEvent\n",
            "",
        )
        .replace(
            "\t#tag EndViewBehavior\n",
            "\t#tag EndViewBehavior\n\t#tag Event\n\t\tSub Opening()\n\t\tEnd Sub\n\t#tag EndEvent\n",
        );
        let msgs = lint_messages(&moved, false);
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        assert!(msgs[0].contains("after `#tag ViewBehavior`"), "{msgs:?}");

        let swapped = CLASS
            .replacen(
                "\t#tag Property, Flags = &h21",
                "\t#tag Method, Flags = &h21",
                1,
            )
            .replacen("\t#tag EndProperty", "\t#tag EndMethod", 1)
            .replace("Private mCount As Integer", "Private Sub Later()");
        let msgs = lint_messages(&swapped, false);
        assert!(
            msgs.iter()
                .any(|m| m.contains("`#tag Method` is out of order")),
            "{msgs:?}"
        );
    }

    #[test]
    fn module_members_go_method_property_constant() {
        // The order Xojo 2026r3 wrote for a module whose members were created
        // constant first, then property, then method.
        let module = "#tag Module\nProtected Module M\n\
\t#tag Method, Flags = &h0\n\t\tSub DoLast()\n\t\tEnd Sub\n\t#tag EndMethod\n\
\t#tag Property, Flags = &h0\n\t\tpLater As Integer\n\t#tag EndProperty\n\
\t#tag Constant, Name = kB, Type = Integer, Dynamic = False, Default = \\\"1\", Scope = Private\n\t#tag EndConstant\n\
\t#tag ViewBehavior\n\t#tag EndViewBehavior\nEnd Module\n#tag EndModule\n";
        assert_eq!(lint_messages(module, false), Vec::<String>::new());

        let constant_first = module.replace(
            "\t#tag Constant, Name = kB, Type = Integer, Dynamic = False, Default = \\\"1\", Scope = Private\n\t#tag EndConstant\n",
            "",
        )
        .replace(
            "\t#tag Method,",
            "\t#tag Constant, Name = kB, Type = Integer, Dynamic = False, Default = \\\"1\", Scope = Private\n\t#tag EndConstant\n\t#tag Method,",
        );
        let msgs = lint_messages(&constant_first, false);
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        assert!(
            msgs.iter()
                .all(|m| m.starts_with("Warning") && m.contains("out of order")),
            "{msgs:?}"
        );

        let two_properties = constant_first.replace(
            "\t#tag ViewBehavior",
            "\t#tag Property, Flags = &h0\n\t\tpMore As Integer\n\t#tag EndProperty\n\t#tag ViewBehavior",
        );
        let msgs = lint_messages(&two_properties, false);
        assert_eq!(msgs.len(), 2, "one warning per kind: {msgs:?}");
        assert!(
            msgs.iter()
                .any(|m| m.contains("`#tag Property` (and 1 more)")),
            "{msgs:?}"
        );
    }

    #[test]
    fn warns_which_side_wins_when_flags_and_keyword_disagree() {
        let missing = CLASS.replace("\t\tPrivate mCount As Integer", "\t\tmCount As Integer");
        let msgs = lint_messages(&missing, false);
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        assert!(
            msgs[0].starts_with("Warning 29:")
                && msgs[0].contains("no `Private`")
                && msgs[0].contains("follows the Flags, making this private"),
            "{msgs:?}"
        );

        let extra = CLASS.replace("\t\tSub Increment()", "\t\tProtected Sub Increment()");
        let msgs = lint_messages(&extra, false);
        assert!(
            msgs[0].contains("says `Protected`")
                && msgs[0].contains("follows the keyword, making this protected"),
            "{msgs:?}"
        );

        let other = CLASS.replace(
            "\t\tPrivate mCount As Integer",
            "\t\tProtected mCount As Integer",
        );
        let msgs = lint_messages(&other, false);
        assert!(
            msgs[0].contains("says `Protected` but Flags = &h21 is private"),
            "{msgs:?}"
        );
    }

    #[test]
    fn flags_unbalanced_tags() {
        let unclosed = CLASS.replace("\t#tag EndProperty\n", "");
        let msgs = lint_messages(&unclosed, false);
        assert!(
            msgs.iter().any(|m| m.contains("closes `#tag Property`")),
            "{msgs:?}"
        );

        // Two methods, the first missing its end tag.
        let lost_end = CLASS
            .replacen("\t#tag EndMethod\n", "", 1)
            .replacen(
                "\t#tag Property, Flags = &h21",
                "\t#tag Method, Flags = &h21",
                1,
            )
            .replacen("\t#tag EndProperty", "\t#tag EndMethod", 1)
            .replace("Private mCount As Integer", "Private Sub Later()");
        let msgs = lint_messages(&lost_end, false);
        assert_eq!(
            msgs,
            [
                "Error 8: `#tag Method` has no `#tag EndMethod` before the next `#tag Method` at line 27."
            ],
            "the block that lost its end is blamed, with no cascade"
        );

        let stray = format!("{CLASS}#tag EndClass\n");
        let msgs = lint_messages(&stray, false);
        assert!(
            msgs.iter().any(|m| m.contains("has no opening")),
            "{msgs:?}"
        );
    }

    #[test]
    fn warns_about_unescaped_default_values() {
        let line = |default: &str| {
            CLASS.replace(
                "Default = \\\"&Delete\", Scope = Public\n",
                &format!("{default}, Scope = Public\n"),
            )
        };
        let msgs = lint_messages(&line("Default = \\\"a,b,c='d'\""), false);
        assert_eq!(
            msgs.len(),
            3,
            "one warning per character, not per occurrence: {msgs:?}"
        );
        assert!(
            msgs.iter()
                .any(|m| m.contains("`,`") && m.contains("`\\x2C`")),
            "{msgs:?}"
        );

        let msgs = lint_messages(&line("Default = \\\"say \"hi\\\" 20°\""), false);
        assert!(
            msgs.iter().any(|m| m.contains("unescaped `\"`")),
            "{msgs:?}"
        );
        assert!(msgs.iter().any(|m| m.contains("`°`")), "{msgs:?}");

        let msgs = lint_messages(&line("Default = \"MyApp\""), false);
        assert!(msgs.len() == 1 && msgs[0].contains("raw `\"`"), "{msgs:?}");

        let escaped = lint_messages(&line("Default = \\\"a\\x2Cb \\\"q\\\" \\xC2\\xB0\""), false);
        assert_eq!(escaped, Vec::<String>::new());
    }

    #[test]
    fn a_window_checks_its_top_level_order() {
        let window = "#tag DesktopWindow\nBegin DesktopWindow Window1\nEnd\n#tag EndDesktopWindow\n\n\
#tag WindowCode\n\t#tag Event\n\t\tSub Opening()\n\t\tEnd Sub\n\t#tag EndEvent\n#tag EndWindowCode\n\n\
#tag Events Button1\n\t#tag Event\n\t\tSub Pressed()\n\t\tEnd Sub\n\t#tag EndEvent\n#tag EndEvents\n\
#tag ViewBehavior\n#tag EndViewBehavior\n";
        assert_eq!(lint_messages(window, true), Vec::<String>::new());
        let events_first = window.replacen(
            "#tag WindowCode",
            "#tag Events Early\n#tag EndEvents\n#tag WindowCode",
            1,
        );
        let msgs = lint_messages(&events_first, true);
        assert!(
            msgs.iter()
                .any(|m| m.contains("`#tag WindowCode` is out of order")),
            "{msgs:?}"
        );
    }

    #[test]
    fn scaffolded_blocks_lint_clean_in_place() {
        let r = rules();
        let mut method = req("method", "Total");
        method.visibility = "protected";
        method.signature = "() As Integer";
        method.body = "Return 1";
        let mut prop = req("property", "mName");
        prop.visibility = "private";
        prop.type_name = "String";
        let mut constant = req("constant", "kTitle");
        constant.default_value = "Größe, 'x' = \"y\"";
        let hook = req("event_definition", "Changed");
        let class = format!(
            "#tag Class\nProtected Class C\n{}\n\n{}\n\n{}\n\n{}\n\n\t#tag ViewBehavior\n\t#tag EndViewBehavior\nEnd Class\n#tag EndClass\n",
            scaffold(&hook, &r).unwrap(),
            scaffold(&method, &r).unwrap(),
            scaffold(&prop, &r).unwrap(),
            scaffold(&constant, &r).unwrap(),
        );
        assert_eq!(lint_messages(&class, false), Vec::<String>::new());
    }

    #[test]
    fn the_report_counts_and_lists() {
        assert_eq!(
            report("/p/A.xojo_code", &[]),
            "No issues found in /p/A.xojo_code."
        );
        let text = report(
            "/p/A.xojo_code",
            &[
                error(3, "bad".into()),
                warning(9, "odd".into()),
                warning(10, "odd".into()),
            ],
        );
        assert_eq!(
            text,
            "1 error, 2 warnings in /p/A.xojo_code:\nerror: line 3: bad\nwarning: line 9: odd\nwarning: line 10: odd"
        );
    }
}
