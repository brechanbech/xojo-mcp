use std::collections::HashMap;

use serde_json::Value;

use crate::mcp::tool::*;
use crate::tools::*;

pub struct EstimateRequestCost;

impl Tool for EstimateRequestCost {
    fn name(&self) -> &'static str { "estimate_request_cost" }
    fn annotations(&self) -> ToolAnnotations { ToolAnnotations::read_only("Estimate Request Cost") }
    fn description(&self) -> &'static str {
        "Estimates likely token cost for a proposed request and suggests cheaper \
         alternative approaches."
    }
    fn parameters(&self) -> &[ToolParam] {
        static P: &[ToolParam] = &[ToolParam {
            name: "request",
            param_type: ParamType::String,
            description: "Natural language request (e.g. 'Add a ListBox to Window1')",
            required: true,
            default: None,
        }, ToolParam {
            name: "planned_tools",
            param_type: ParamType::String,
            description: "Comma-separated list of expected tool calls",
            required: false,
            default: None,
        }];
        P
    }
    fn run(&self, args: &HashMap<String, Value>, _ctx: &ToolContext) -> ToolResult {
        let request = arg_str(args, "request", "");
        let planned_tools = arg_str(args, "planned_tools", "");

        let request_lower = request.to_lowercase();
        let mut score: i32 = 0;
        let mut reasons = Vec::new();

        // Request length heuristics.
        if request.len() > 250 {
            score += 2;
            reasons.push("Long request text (>250 chars)".to_string());
        } else if request.len() > 100 {
            score += 1;
            reasons.push("Moderate request text (>100 chars)".to_string());
        }

        // Broad scope keywords.
        let broad_keywords = [
            "entire", "whole", "full", "everything", "all files", "codebase",
            "architecture", "refactor", "analyze", "analyse", "review", "audit",
        ];
        if has_any_keyword(&request_lower, &broad_keywords) {
            score += 3;
            reasons.push("Contains broad-scope keywords".to_string());
        }

        // Documentation keywords.
        let doc_keywords = ["documentation", "docs", "api reference", "lookup"];
        if has_any_keyword(&request_lower, &doc_keywords) {
            score += 2;
            reasons.push("Contains documentation-related keywords".to_string());
        }

        // Focused keywords (reduce cost).
        let focused_keywords = [
            "add", "rename", "single", "window1", "listbox", "button", "one control",
        ];
        if has_any_keyword(&request_lower, &focused_keywords) {
            score -= 1;
            reasons.push("Contains focused/specific keywords".to_string());
        }

        // Tool-specific cost adjustments.
        let mut alternatives = Vec::new();
        if !planned_tools.is_empty() {
            let tools: Vec<&str> = planned_tools.split(',').map(|t| t.trim()).collect();
            for tool in &tools {
                match *tool {
                    "list_doc_topics" => {
                        score += 3;
                        reasons
                            .push("list_doc_topics returns large output".to_string());
                        alternatives
                            .push("Use a narrow filter with list_doc_topics".to_string());
                    }
                    "lookup_class" => {
                        score += 2;
                        reasons.push("lookup_class returns full class docs".to_string());
                        alternatives
                            .push("Try search_docs first to confirm the class name".to_string());
                    }
                    "search_docs" => {
                        score += 2;
                        reasons.push("search_docs can return large context".to_string());
                        alternatives.push(
                            "Use low max_results and context_lines values".to_string(),
                        );
                    }
                    "get_code" | "set_code" => {
                        score += 1;
                        alternatives
                            .push("Process one location at a time".to_string());
                    }
                    _ => {}
                }
            }
        }

        // Floor at 0.
        score = score.max(0);

        // Determine cost level.
        let (level, tokens) = if score <= 2 {
            ("LOW", "~1K tokens")
        } else if score <= 5 {
            ("MEDIUM", "~1K-5K tokens")
        } else {
            ("HIGH", ">5K tokens")
        };

        // De-duplicate alternatives.
        alternatives.dedup();

        let alt_text = if alternatives.is_empty() {
            "None — request appears well-scoped.".to_string()
        } else {
            alternatives
                .iter()
                .map(|a| format!("  - {a}"))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let reasons_text = if reasons.is_empty() {
            "Standard request".to_string()
        } else {
            reasons.join("; ")
        };

        ToolResult::success(format!(
            "Cost estimate: {level}\nToken impact: {tokens}\nWhy: {reasons_text}\nCheaper alternatives:\n{alt_text}"
        ))
    }
}

fn has_any_keyword(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}
