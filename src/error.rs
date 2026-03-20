use console::style;
use std::fmt;

/// Error type for request processing.
#[derive(Debug)]
pub enum RequestError {
    /// Invalid JSON/YAML input.
    Parse {
        input: String,
        line: Option<usize>,
        column: Option<usize>,
        msg: String,
    },
    /// Valid parse but bad structure (no method, wrong types).
    Structure {
        msg: String,
    },
    /// Network/connection error.
    Network {
        msg: String,
    },
    /// Malformed URL.
    Url {
        url: String,
        msg: String,
    },
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequestError::Parse { msg, line, column, .. } => {
                write!(f, "parse error: {msg}")?;
                match (line, column) {
                    (Some(l), Some(c)) => write!(f, " (line {l}, column {c})"),
                    (Some(l), None) => write!(f, " (line {l})"),
                    (None, Some(c)) => write!(f, " (column {c})"),
                    (None, None) => Ok(()),
                }
            }
            RequestError::Structure { msg } => write!(f, "{msg}"),
            RequestError::Network { msg } => write!(f, "network error: {msg}"),
            RequestError::Url { url, msg } => write!(f, "URL error: {msg} ({url})"),
        }
    }
}

impl RequestError {
    /// Create from a yttp parse error, attaching the original input.
    pub fn from_parse(input: &str, err: yttp::Error) -> Self {
        match err {
            yttp::Error::Parse { msg, line, column } => RequestError::Parse {
                input: input.to_string(),
                msg,
                line,
                column,
            },
            other => RequestError::Parse {
                input: input.to_string(),
                msg: other.to_string(),
                line: None,
                column: None,
            },
        }
    }

    /// Colored error display for terminal output.
    ///
    /// For parse errors with column info, shows the input with the valid prefix
    /// in green and the error point onward in red, with a caret below.
    pub fn display_colored(&self) -> String {
        match self {
            RequestError::Parse { input, column, msg, .. } => {
                if let Some(col) = column {
                    // Column is 1-based from serde, convert to 0-based index
                    let col_idx = col.saturating_sub(1);
                    let (valid, bad) = if col_idx < input.len() {
                        (&input[..col_idx], &input[col_idx..])
                    } else {
                        (input.as_str(), "")
                    };
                    let mut out = String::new();
                    out.push_str(&format!("  {}{}\n", style(valid).green(), style(bad).red()));
                    // Caret line: spaces to align under the error column, +2 for indent
                    out.push_str(&format!("  {:>width$} {}", style("^").red(), style(msg).dim(), width = col_idx + 1));
                    out
                } else {
                    format!("  {} {}", style("error:").red(), msg)
                }
            }
            RequestError::Structure { msg } => {
                format!("  {} {}", style("error:").red(), msg)
            }
            RequestError::Network { msg } => {
                format!("  {} {}", style("error:").red(), msg)
            }
            RequestError::Url { url, msg } => {
                format!("  {} {} ({})", style("error:").red(), msg, style(url).dim())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_display_plain() {
        let err = RequestError::Parse {
            input: "{broken".to_string(),
            line: Some(1),
            column: Some(8),
            msg: "expected '}'".to_string(),
        };
        let display = format!("{err}");
        assert!(display.contains("parse error:"));
        assert!(display.contains("expected '}'"));
        assert!(display.contains("line 1"));
        assert!(display.contains("column 8"));
    }

    #[test]
    fn parse_error_colored_has_caret() {
        let err = RequestError::Parse {
            input: "{g: ok, b: {".to_string(),
            line: Some(1),
            column: Some(12),
            msg: "expected '}'".to_string(),
        };
        let colored = err.display_colored();
        // Should contain caret
        assert!(colored.contains('^'), "colored: {colored}");
    }

    #[test]
    fn parse_error_no_column_fallback() {
        let err = RequestError::Parse {
            input: "bad".to_string(),
            line: None,
            column: None,
            msg: "invalid input".to_string(),
        };
        let colored = err.display_colored();
        assert!(colored.contains("invalid input"));
        assert!(!colored.contains('^'));
    }

    #[test]
    fn structure_error_display() {
        let err = RequestError::Structure {
            msg: "no HTTP method found".to_string(),
        };
        assert_eq!(format!("{err}"), "no HTTP method found");
        assert!(err.display_colored().contains("no HTTP method found"));
    }

    #[test]
    fn network_error_display() {
        let err = RequestError::Network {
            msg: "connection refused".to_string(),
        };
        assert!(format!("{err}").contains("connection refused"));
    }

    #[test]
    fn url_error_display() {
        let err = RequestError::Url {
            url: "not://[valid".to_string(),
            msg: "invalid URL".to_string(),
        };
        let display = format!("{err}");
        assert!(display.contains("invalid URL"));
        assert!(display.contains("not://[valid"));
    }

    #[test]
    fn from_parse_extracts_position() {
        let yttp_err = yttp::Error::parse("bad json", Some(1), Some(5));
        let err = RequestError::from_parse("{test", yttp_err);
        match err {
            RequestError::Parse { line, column, msg, input } => {
                assert_eq!(line, Some(1));
                assert_eq!(column, Some(5));
                assert!(msg.contains("bad json"));
                assert_eq!(input, "{test");
            }
            _ => panic!("expected Parse"),
        }
    }
}
