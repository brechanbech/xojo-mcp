/// Build an IDE script that safely assigns a multi-line string to a variable.
///
/// Handles embedded quotes (doubled for Xojo IDE script escaping) and
/// trailing newlines. The generated script, when executed, sets the named
/// variable to the exact value of `value`.
pub fn build_string_variable_script(var_name: &str, value: &str) -> String {
    let lines: Vec<&str> = value.split('\n').collect();
    let mut script_lines = Vec::new();

    script_lines.push(format!("Dim {var_name} As String = \"\""));

    for line in &lines {
        let escaped = line.replace('"', "\"\"");
        script_lines.push(format!(
            "{var_name} = {var_name} + \"{escaped}\" + EndOfLine"
        ));
    }

    if !value.ends_with('\n') {
        script_lines.push("Dim __eol As String = EndOfLine".into());
        script_lines.push(format!("If {var_name}.Length >= __eol.Length Then"));
        script_lines.push(format!("  If {var_name}.Right(__eol.Length) = __eol Then"));
        script_lines.push(format!(
            "    {var_name} = {var_name}.Left({var_name}.Length - __eol.Length)"
        ));
        script_lines.push("  End If".into());
        script_lines.push("End If".into());
    }

    script_lines.join("\n")
}

/// Escape a string for use inside an IDE script string literal.
/// Doubles all quote characters, and turns each line break into
/// `" + EndOfLine + "`: a literal cannot span lines, so a raw newline would end
/// it there and fail to compile. Every caller puts the result inside a literal
/// that is part of an expression, where that concatenation is valid.
pub fn escape_ide_string(s: &str) -> String {
    s.replace('"', "\"\"")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\" + EndOfLine + \"")
}

/// Indent every non-empty line of `text` by the given prefix.
pub fn indent_lines(text: &str, indent: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_ide_string() {
        assert_eq!(escape_ide_string(r#"hello "world""#), r#"hello ""world"""#);
        assert_eq!(escape_ide_string("no quotes"), "no quotes");
    }

    #[test]
    fn a_line_break_never_reaches_the_literal() {
        assert_eq!(escape_ide_string("a\nb"), "a\" + EndOfLine + \"b");
        assert_eq!(
            escape_ide_string("a\r\nb\rc"),
            "a\" + EndOfLine + \"b\" + EndOfLine + \"c"
        );
        assert!(!escape_ide_string("x\r\ny\n").contains(['\n', '\r']));
    }

    #[test]
    fn test_build_string_variable_script_simple() {
        let script = build_string_variable_script("__code", "hello");
        assert!(script.contains("Dim __code As String = \"\""));
        assert!(script.contains("__code = __code + \"hello\" + EndOfLine"));
        // No trailing newline in input, so strip logic should be present.
        assert!(script.contains("Dim __eol As String = EndOfLine"));
    }

    #[test]
    fn test_build_string_variable_script_trailing_newline() {
        let script = build_string_variable_script("__code", "hello\n");
        // Has trailing newline, so no strip logic.
        assert!(!script.contains("Dim __eol"));
    }

    #[test]
    fn test_build_string_variable_script_multiline() {
        let script = build_string_variable_script("__v", "line1\nline2\nline3");
        let lines: Vec<&str> = script.lines().collect();
        // Dim + 3 content lines + 6 strip lines = 10
        assert_eq!(lines.len(), 10);
    }

    #[test]
    fn test_build_string_variable_script_embedded_quotes() {
        let script = build_string_variable_script("__v", r#"say "hi""#);
        assert!(script.contains(r#"__v = __v + "say ""hi""" + EndOfLine"#));
    }

    #[test]
    fn test_indent_lines() {
        assert_eq!(indent_lines("a\nb\n\nc", "  "), "  a\n  b\n\n  c");
    }
}
