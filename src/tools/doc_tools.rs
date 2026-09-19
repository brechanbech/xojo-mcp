use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use serde_json::Value;

use crate::mcp::tool::*;
use crate::tools::*;

// ---------------------------------------------------------------------------
// search_docs
// ---------------------------------------------------------------------------
pub struct SearchDocs {
    cached_lines: OnceLock<Vec<String>>,
}

impl SearchDocs {
    pub fn new() -> Self {
        Self {
            cached_lines: OnceLock::new(),
        }
    }

    fn load_lines(&self, docs_path: &Path) -> &[String] {
        self.cached_lines.get_or_init(|| {
            let file = docs_path.join("llms-full.txt");
            match std::fs::read_to_string(&file) {
                Ok(content) => content.lines().map(String::from).collect(),
                Err(_) => Vec::new(),
            }
        })
    }
}

impl Tool for SearchDocs {
    fn name(&self) -> &'static str { "search_docs" }
    fn description(&self) -> &'static str {
        "Searches local Xojo documentation guides and tutorials by keyword. Returns matching \
         sections with context. For specific class/method lookup, use lookup_class instead."
    }
    fn parameters(&self) -> &[ToolParam] {
        static P: &[ToolParam] = &[ToolParam {
            name: "query",
            param_type: ParamType::String,
            description: "Search term (e.g. 'JSONItem', 'database')",
            required: true,
            default: None,
        }, ToolParam {
            name: "max_results",
            param_type: ParamType::Integer,
            description: "Max sections to return",
            required: false,
            default: None,
        }, ToolParam {
            name: "context_lines",
            param_type: ParamType::Integer,
            description: "Lines before/after match to include",
            required: false,
            default: None,
        }];
        P
    }
    fn run(&self, args: &HashMap<String, Value>, ctx: &ToolContext) -> ToolResult {
        let docs_path = match ctx.docs_path {
            Some(p) => p,
            None => return ToolResult::failure("Xojo documentation path not configured."),
        };

        let query = arg_str(args, "query", "");
        let max_results = arg_i64(args, "max_results", 5) as usize;
        let context_lines = arg_i64(args, "context_lines", 10) as usize;

        if query.is_empty() {
            return ToolResult::failure("The `query` parameter cannot be empty.");
        }

        let lines = self.load_lines(docs_path);
        if lines.is_empty() {
            return ToolResult::failure("Documentation not loaded. Check docs path.");
        }

        let query_lower = query.to_lowercase();
        let mut results = Vec::new();
        let mut current_section = String::new();
        let mut last_end: usize = 0;

        for (i, line) in lines.iter().enumerate() {
            if line.starts_with('#') {
                current_section = line.clone();
            }

            if line.to_lowercase().contains(&query_lower) {
                // Skip if within previous context window.
                if i < last_end {
                    continue;
                }

                let start = i.saturating_sub(context_lines);
                let end = (i + context_lines + 1).min(lines.len());
                last_end = end;

                let mut section_output = String::new();
                if !current_section.is_empty() {
                    section_output.push_str(&format!("--- {current_section} ---\n"));
                }

                for (j, line) in lines.iter().enumerate().take(end).skip(start) {
                    if j == i {
                        section_output.push_str(&format!(">>> {line}\n"));
                    } else {
                        section_output.push_str(&format!("    {line}\n"));
                    }
                }

                results.push(section_output);
                if results.len() >= max_results {
                    break;
                }
            }
        }

        if results.is_empty() {
            ToolResult::success(format!("No matches found for '{query}'."))
        } else {
            ToolResult::success(results.join("\n"))
        }
    }
}

// ---------------------------------------------------------------------------
// lookup_class
// ---------------------------------------------------------------------------
pub struct LookupClass;

impl Tool for LookupClass {
    fn name(&self) -> &'static str { "lookup_class" }
    fn description(&self) -> &'static str {
        "Looks up detailed documentation for a specific Xojo class, control, data type, or API by name."
    }
    fn parameters(&self) -> &[ToolParam] {
        static P: &[ToolParam] = &[
            ToolParam {
                name: "class_name",
                param_type: ParamType::String,
                description: "Class name to look up (e.g. 'DesktopButton', 'JSONItem')",
                required: true,
                default: None,
            },
            ToolParam {
                name: "member",
                param_type: ParamType::String,
                description: "Return only this member's entry (e.g. 'Middle', 'Pressed') \
                              rather than the class summary. Case-insensitive.",
                required: false,
                default: None,
            },
            ToolParam {
                name: "full",
                param_type: ParamType::Boolean,
                description: "Return the unprocessed documentation page. Large: String is \
                              over 70,000 characters of raw reStructuredText.",
                required: false,
                default: None,
            },
        ];
        P
    }
    fn run(&self, args: &HashMap<String, Value>, ctx: &ToolContext) -> ToolResult {
        let docs_path = match ctx.docs_path {
            Some(p) => p,
            None => return ToolResult::failure("Xojo documentation path not configured."),
        };

        let class_name = arg_str(args, "class_name", "");
        if class_name.is_empty() {
            return ToolResult::failure("The `class_name` parameter cannot be empty.");
        }

        let sources_dir = docs_path.join("_sources");
        if !sources_dir.exists() {
            return ToolResult::failure("Documentation _sources directory not found.");
        }

        let lower = class_name.to_lowercase();
        let candidates = [
            format!("{lower}.rst.txt"),
            format!("desktop{lower}.rst.txt"),
            format!("web{lower}.rst.txt"),
        ];

        let Some(path) = candidates
            .iter()
            .find_map(|c| find_file_recursive(&sources_dir, c))
        else {
            return ToolResult::failure(format!(
                "No documentation found for '{class_name}'. Try search_docs instead."
            ));
        };

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                return ToolResult::failure(format!("Could not read {}: {e}", path.display()));
            }
        };

        // full=true is the previous behaviour, kept for the case where the
        // shaping below hides something -- but bounded, since a raw page can be
        // larger than a client will accept.
        if arg_bool(args, "full", false) {
            return ToolResult::success(truncate_page(content));
        }

        // Anchors are keyed on the file's own stem, which is the spelling the
        // page uses: string.rst.txt labels its members `.. _string.middle:`.
        let class_key = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.strip_suffix(".rst.txt").unwrap_or(n).to_lowercase())
            .unwrap_or(lower);

        let lines: Vec<&str> = content.lines().collect();
        let anchors = member_anchors(&lines, &class_key);

        let member = arg_str(args, "member", "");
        if !member.is_empty() {
            let want = member.to_lowercase();
            return match anchors.iter().position(|(_, m)| *m == want) {
                Some(idx) => ToolResult::success(member_entry(&lines, &anchors, idx)),
                None => ToolResult::failure(format!(
                    "'{class_name}' documents no member '{member}'. Known members: {}",
                    member_list(&anchors)
                )),
            };
        }

        ToolResult::success(class_summary(&lines, &anchors))
    }
}

// ---------------------------------------------------------------------------
// lookup_class output shaping
//
// A documentation page is reStructuredText written for Sphinx, not for a
// reader: string.rst.txt is 72 KB, most of it cross-reference targets and table
// scaffolding. Returning it whole spends a caller's context on markup and
// buries the answer. These helpers cut a page down to its own description plus
// the member tables, or to a single member's entry.
// ---------------------------------------------------------------------------

/// Cap on `full=true` output. A larger page is truncated rather than refused:
/// a cut answer is still usable, an unsendable one is not.
const FULL_PAGE_LIMIT: usize = 100_000;

/// Characters RST uses to underline a section heading.
const UNDERLINE_CHARS: &[char] = &['=', '-', '~', '^', '"', '\'', '#', '*', '+'];

/// Sphinx roles that carry display text ahead of their target.
const ROLE_PREFIXES: [&str; 5] = [":ref:`", ":doc:`", ":class:`", ":func:`", ":meth:`"];

/// Rewrite Sphinx cross-reference roles down to the text a reader wants.
///
/// ``:ref:`Length<string.length>``` becomes `Length` and
/// ``:doc:`Integer</api/data_types/integer>``` becomes `Integer`: the display
/// text is everything before `<`, and a role without one is its own label.
/// This is the single largest contributor to a raw page's size, since a method
/// table row is mostly target paths.
///
/// Plain scanning rather than a regex — this crate has no regex dependency and
/// pulling one in to strip five known prefixes would not pay for itself.
fn resolve_roles(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;

    while !rest.is_empty() {
        // The earliest role start in what remains, whichever prefix it uses.
        let mut best: Option<(usize, usize)> = None;
        for prefix in ROLE_PREFIXES {
            if let Some(pos) = rest.find(prefix)
                && best.is_none_or(|(at, _)| pos < at)
            {
                best = Some((pos, prefix.len()));
            }
        }

        let Some((start, prefix_len)) = best else { break };
        let after = &rest[start + prefix_len..];
        // An unterminated role is malformed source; leave it exactly as it is
        // rather than guessing where it was meant to end.
        let Some(close) = after.find('`') else { break };

        out.push_str(&rest[..start]);
        let body = &after[..close];
        out.push_str(body.split('<').next().unwrap_or(body).trim());
        rest = &after[close + 1..];
    }

    out.push_str(rest);
    out
}

/// Is `lines[i]` a section heading?
///
/// A heading is a line of text immediately followed by a rule at least as long,
/// with no blank line between. That adjacency is the whole test: a standalone
/// transition rule sits between blank lines, and mistaking one for a heading is
/// what lets the last member entry on a page swallow the page's trailing matter.
fn is_heading_at(lines: &[&str], i: usize) -> bool {
    let Some(text) = lines.get(i).map(|l| l.trim()) else {
        return false;
    };
    if text.is_empty() {
        return false;
    }
    let Some(rule) = lines.get(i + 1).map(|l| l.trim_end()) else {
        return false;
    };
    if rule.len() < text.len() {
        return false;
    }
    let mut chars = rule.chars();
    match chars.next() {
        Some(first) => UNDERLINE_CHARS.contains(&first) && chars.all(|c| c == first),
        None => false,
    }
}

/// The next section heading at or after `from`.
fn next_heading_at(lines: &[&str], from: usize) -> Option<usize> {
    (from..lines.len()).find(|&i| is_heading_at(lines, i))
}

/// Directive and table scaffolding that says nothing once roles are resolved.
///
/// `:header-rows:` earns its place beside `:header:`: it is the same directive
/// spelled differently, and missing it leaves raw RST in the output.
fn is_scaffolding(line: &str) -> bool {
    let t = line.trim();
    t == "----"
        || t.starts_with(".. rst-class::")
        || t.starts_with(".. csv-table::")
        || t.starts_with(".. code::")
        || t.starts_with(":header:")
        || t.starts_with(":header-rows:")
        || t.starts_with(":widths:")
}

/// Resolve roles, drop scaffolding, and collapse the blank runs that removing
/// scaffolding leaves behind.
fn tidy(lines: &[&str]) -> String {
    let mut out: Vec<String> = Vec::new();

    for line in lines {
        if is_scaffolding(line) {
            continue;
        }
        let resolved = resolve_roles(line);
        if resolved.trim().is_empty() {
            if out.last().is_some_and(|l| l.is_empty()) {
                continue;
            }
            out.push(String::new());
        } else {
            out.push(resolved.trim_end().to_string());
        }
    }

    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// Anchor labels of the form `.. _<class>.<member>:`, with the line each is on.
fn member_anchors(lines: &[&str], class_key: &str) -> Vec<(usize, String)> {
    let prefix = format!(".. _{class_key}.");
    lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| {
            let rest = line.trim().strip_prefix(&prefix)?.strip_suffix(':')?;
            (!rest.is_empty()).then(|| (i, rest.to_string()))
        })
        .collect()
}

/// The documented member names, so a bad `member` argument can say what was
/// available instead of only that the guess was wrong.
///
/// These are the anchor keys, which are lowercase. Recovering each member's
/// documented spelling is not worth it: the bare name sits a variable distance
/// below its anchor, Xojo identifiers are case-insensitive anyway, and matching
/// already lowercases both sides.
fn member_list(anchors: &[(usize, String)]) -> String {
    if anchors.is_empty() {
        return "none — this page documents no members".to_string();
    }
    anchors
        .iter()
        .map(|(_, m)| m.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The page down to its first member anchor: description plus the member
/// tables, which is what a question about a class almost always means.
fn class_summary(lines: &[&str], anchors: &[(usize, String)]) -> String {
    let end = anchors.first().map_or(lines.len(), |(i, _)| *i);
    let mut head: Vec<&str> = lines[..end].to_vec();

    while head.last().is_some_and(|l| l.trim().is_empty()) {
        head.pop();
    }
    // Whatever heading introduced the member entries is stranded once those
    // entries are split off below it.
    if head.len() >= 2 && is_heading_at(&head, head.len() - 2) {
        head.truncate(head.len() - 2);
    }

    tidy(&head)
}

/// One member's entry: from its anchor to the next anchor, or — for the last
/// member on a page — to the next section heading.
///
/// The heading bound is what stops that last entry running on into Interfaces,
/// Compatibility and `.. seealso::`, none of which belongs to any member. The
/// anchor line itself is dropped: it delimits the section rather than
/// describing it.
fn member_entry(lines: &[&str], anchors: &[(usize, String)], idx: usize) -> String {
    let (start, _) = anchors[idx];
    let end = match anchors.get(idx + 1) {
        Some((next, _)) => *next,
        None => next_heading_at(lines, start + 1).unwrap_or(lines.len()),
    };
    tidy(&lines[start + 1..end])
}

/// Bound a raw page, saying plainly that it was cut so a caller does not read
/// the tail as the end of the document.
fn truncate_page(content: String) -> String {
    if content.len() <= FULL_PAGE_LIMIT {
        return content;
    }
    let mut cut = FULL_PAGE_LIMIT;
    while cut > 0 && !content.is_char_boundary(cut) {
        cut -= 1;
    }
    format!(
        "{}\n\n[truncated at {FULL_PAGE_LIMIT} of {} characters — pass full=false for the \
         shaped summary]",
        &content[..cut],
        content.len()
    )
}

fn find_file_recursive(dir: &Path, target: &str) -> Option<std::path::PathBuf> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return None,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file_recursive(&path, target) {
                return Some(found);
            }
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && name.to_lowercase() == target
        {
            return Some(path);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// list_doc_topics
// ---------------------------------------------------------------------------
pub struct ListDocTopics;

impl Tool for ListDocTopics {
    fn name(&self) -> &'static str { "list_doc_topics" }
    fn description(&self) -> &'static str {
        "Lists available Xojo documentation topics and pages. Optionally filtered by keyword."
    }
    fn parameters(&self) -> &[ToolParam] {
        static P: &[ToolParam] = &[ToolParam {
            name: "filter",
            param_type: ParamType::String,
            description: "Keyword to filter topics (e.g. 'Desktop', 'database'). Returns all if empty.",
            required: false,
            default: None,
        }];
        P
    }
    fn run(&self, args: &HashMap<String, Value>, ctx: &ToolContext) -> ToolResult {
        let docs_path = match ctx.docs_path {
            Some(p) => p,
            None => return ToolResult::failure("Xojo documentation path not configured."),
        };

        let filter = arg_str(args, "filter", "");
        let file = docs_path.join("llms.txt");

        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(e) => return ToolResult::failure(format!("Could not read llms.txt: {e}")),
        };

        if filter.is_empty() {
            return ToolResult::success(content);
        }

        let filter_lower = filter.to_lowercase();
        let matched: Vec<&str> = content
            .lines()
            .filter(|line| line.to_lowercase().contains(&filter_lower))
            .collect();

        if matched.is_empty() {
            ToolResult::success(format!("No topics found matching: {filter}"))
        } else {
            ToolResult::success(format!(
                "{} topics matching '{filter}':\n{}",
                matched.len(),
                matched.join("\n")
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        class_summary, is_heading_at, member_anchors, member_entry, resolve_roles, tidy,
    };

    /// A page shaped like the real ones: title, description, a member table,
    /// two member entries, then trailing matter belonging to no member.
    const PAGE: &str = "\
======
String
======

Description
-----------
A String is a series of characters.

Methods
-------
.. csv-table::
   :header: \"Name\", \"Returns\"
   :widths: auto

   :ref:`Middle<string.middle>`, \":doc:`String</api/data_types/string>`\"
   :ref:`Val<string.val>`, \":doc:`Double</api/data_types/double>`\"

Method descriptions
-------------------


.. _string.middle:



----

**Middle**\\(index As :doc:`Integer</api/data_types/integer>`) As String

    Returns a portion of a String.

.. _string.val:



----

**Val**\\() As Double

    Converts to a number.

Compatibility
-------------

.. csv-table::
  :header-rows: 0

  \"**Project Types**\", \"Desktop\"

.. seealso::

     :doc:`Double</api/data_types/double>` data type.
";

    fn lines() -> Vec<&'static str> {
        PAGE.lines().collect()
    }

    #[test]
    fn roles_collapse_to_their_display_text() {
        assert_eq!(
            resolve_roles(":ref:`Middle<string.middle>`, \":doc:`String</api/data_types/string>`\""),
            "Middle, \"String\""
        );
        // A role with no angle bracket is its own label.
        assert_eq!(resolve_roles("see :doc:`Nil`"), "see Nil");
        // Text with no roles is returned untouched.
        assert_eq!(resolve_roles("plain prose"), "plain prose");
    }

    #[test]
    fn an_unterminated_role_is_left_alone() {
        // Malformed source: guessing where it was meant to end would corrupt it.
        let broken = "before :ref:`Middle<string.middle";
        assert_eq!(resolve_roles(broken), broken);
    }

    #[test]
    fn a_transition_rule_is_not_mistaken_for_a_heading() {
        let l = lines();
        let heading = l.iter().position(|s| *s == "Compatibility").unwrap();
        assert!(is_heading_at(&l, heading), "text above an underline");

        // `----` between blank lines delimits a member entry; reading it as a
        // heading is what made the last entry stop in the wrong place.
        let rule = l.iter().position(|s| *s == "----").unwrap();
        assert!(!is_heading_at(&l, rule));
    }

    #[test]
    fn the_summary_stops_before_the_members_and_drops_their_heading() {
        let l = lines();
        let anchors = member_anchors(&l, "string");
        let summary = class_summary(&l, &anchors);

        assert!(summary.contains("A String is a series of characters."));
        // Roles resolved, table scaffolding gone.
        assert!(summary.contains("Middle, \"String\""));
        assert!(!summary.contains(":widths:"));
        assert!(!summary.contains(":header:"));
        // "Methods" heads the table, so it stays. "Method descriptions" heads
        // the entries we just split off, so it is stranded and goes.
        assert!(summary.contains("Methods"), "table heading lost: {summary}");
        assert!(
            !summary.contains("Method descriptions"),
            "stranded heading kept: {summary}"
        );
        // And no member body leaks upward.
        assert!(!summary.contains("Returns a portion"));
    }

    #[test]
    fn the_last_member_stops_at_the_next_section() {
        let l = lines();
        let anchors = member_anchors(&l, "string");
        assert_eq!(anchors.len(), 2);

        let val = member_entry(&l, &anchors, 1);
        assert!(val.contains("Converts to a number."));
        // The trailing matter belongs to the page, not to Val. This is the leak
        // that bounding on end-of-file produces.
        assert!(!val.contains("Compatibility"), "leaked: {val}");
        assert!(!val.contains("seealso"), "leaked: {val}");
        assert!(!val.contains(":header-rows:"), "leaked: {val}");
    }

    #[test]
    fn a_member_stops_at_the_next_member() {
        let l = lines();
        let anchors = member_anchors(&l, "string");
        let middle = member_entry(&l, &anchors, 0);
        assert!(middle.contains("Returns a portion of a String."));
        assert!(!middle.contains("Converts to a number."));
        // The anchor label delimits the section rather than describing it.
        assert!(!middle.contains(".. _string."));
    }

    #[test]
    fn tidy_collapses_the_blank_runs_that_stripping_leaves() {
        let input = vec!["a", "", "", "", ".. rst-class:: forsearch", "", "b"];
        assert_eq!(tidy(&input), "a\n\nb");
    }
}
