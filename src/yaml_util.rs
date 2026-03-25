/// Check whether a YAML scalar value needs quoting.
///
/// Covers both block and flow contexts — flow indicators (`,`, `{`, `}`, `[`, `]`)
/// are always quoted even though they're only ambiguous in flow style, because
/// quoting them in block style is harmless and keeps one code path.
pub fn needs_yaml_quoting(s: &str) -> bool {
    // Reserved YAML words
    match s {
        "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "~"
        | "True" | "False" | "Null" | "Yes" | "No" | "On" | "Off"
        | "TRUE" | "FALSE" | "NULL" | "YES" | "NO" | "ON" | "OFF" => return true,
        _ => {}
    }
    // Looks like a number
    if s.parse::<f64>().is_ok() {
        return true;
    }
    // Flow indicators or control chars
    if s.contains(|c: char| matches!(c, '{' | '}' | '[' | ']' | ',' | '\n' | '\r' | '\t')) {
        return true;
    }
    // Mapping indicator or comment
    if s.contains(": ") || s.contains(" #") {
        return true;
    }
    // Starts with problematic chars
    if s.starts_with(|c: char| {
        matches!(c, '&' | '*' | '!' | '|' | '>' | '\'' | '"' | '%' | '@' | '`' | '?' | '-' | ' ' | ':' | '#')
    }) {
        return true;
    }
    // Ends with colon or space
    s.ends_with(':') || s.ends_with(' ')
}

/// Format a string as a YAML flow scalar: double-quoted with escapes if needed,
/// plain otherwise.
pub fn yaml_flow_scalar(s: &str) -> String {
    if s.is_empty() || needs_yaml_quoting(s) {
        let escaped = s
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

/// Format a string as a YAML block scalar: single-quoted with escaped single quotes
/// if needed, plain otherwise.
pub fn yaml_block_scalar(s: &str) -> String {
    if s.is_empty() || needs_yaml_quoting(s) {
        format!("'{}'", s.replace('\'', "''"))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_scalar_reserved_words() {
        assert_eq!(yaml_flow_scalar("true"), "\"true\"");
        assert_eq!(yaml_flow_scalar("null"), "\"null\"");
        assert_eq!(yaml_flow_scalar("yes"), "\"yes\"");
    }

    #[test]
    fn flow_scalar_numbers() {
        assert_eq!(yaml_flow_scalar("42"), "\"42\"");
        assert_eq!(yaml_flow_scalar("3.14"), "\"3.14\"");
    }

    #[test]
    fn flow_scalar_empty() {
        assert_eq!(yaml_flow_scalar(""), "\"\"");
    }

    #[test]
    fn flow_scalar_plain() {
        assert_eq!(yaml_flow_scalar("hello"), "hello");
        assert_eq!(yaml_flow_scalar("http://example.com"), "http://example.com");
        assert_eq!(yaml_flow_scalar("application/json"), "application/json");
    }

    #[test]
    fn flow_scalar_flow_indicators() {
        assert_eq!(yaml_flow_scalar("a,b"), "\"a,b\"");
        assert_eq!(yaml_flow_scalar("{x}"), "\"{x}\"");
        assert_eq!(yaml_flow_scalar("[1]"), "\"[1]\"");
    }

    #[test]
    fn flow_scalar_newlines_escaped() {
        assert_eq!(yaml_flow_scalar("line1\nline2"), "\"line1\\nline2\"");
    }

    #[test]
    fn block_scalar_quoting() {
        assert_eq!(yaml_block_scalar("true"), "'true'");
        assert_eq!(yaml_block_scalar("hello"), "hello");
        assert_eq!(yaml_block_scalar("a,b"), "'a,b'");
        assert_eq!(yaml_block_scalar("it's"), "it's"); // apostrophe mid-string is valid plain YAML
        assert_eq!(yaml_block_scalar("'quoted'"), "'''quoted'''"); // leading quote needs quoting
    }

    #[test]
    fn quoting_colon_space() {
        assert!(needs_yaml_quoting("key: value"));
        assert!(!needs_yaml_quoting("key:value")); // no space after colon
    }

    #[test]
    fn quoting_leading_special() {
        assert!(needs_yaml_quoting("&anchor"));
        assert!(needs_yaml_quoting("*alias"));
        assert!(needs_yaml_quoting("!tag"));
        assert!(needs_yaml_quoting("-item"));
        assert!(needs_yaml_quoting("# comment"));
    }
}
