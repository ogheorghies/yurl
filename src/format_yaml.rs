use crate::yaml_util;
use serde_json::Value;

// ANSI color codes
const BLUE: &str = "\x1b[34m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

/// Render a JSON value as YAML, with specified keys rendered inline (flow style).
///
/// When `color` is false (batch/pipe mode), non-inline keys are serialized via
/// `serde_yml` for guaranteed valid YAML. When `color` is true (TTY), the custom
/// renderer is used for ANSI coloring.
pub fn to_yaml(val: &Value, inline_keys: &[&str], color: bool) -> String {
    let mut out = String::new();
    if let Value::Object(map) = val {
        for (k, v) in map {
            if inline_keys.contains(&k.as_str()) {
                if color {
                    out.push_str(&format!("{BLUE}{k}{RESET}: "));
                } else {
                    out.push_str(k);
                    out.push_str(": ");
                }
                write_flow(&mut out, v, color);
                out.push('\n');
            } else if !color {
                // Batch mode: use serde_yml for guaranteed valid YAML
                write_serde_yaml_key_value(&mut out, k, v);
            } else {
                write_yaml_key_value(&mut out, k, v, 0, color);
            }
        }
    }
    out
}

/// Serialize a key-value pair using serde_yml. The value is serialized as a
/// standalone YAML document, then indented under the key.
fn write_serde_yaml_key_value(out: &mut String, key: &str, val: &Value) {
    match val {
        // Scalars: render inline
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            out.push_str(key);
            out.push_str(": ");
            let s = serde_yml::to_string(val).unwrap_or_default();
            out.push_str(s.trim_end());
            out.push('\n');
        }
        // Compound: render block under key
        _ => {
            out.push_str(key);
            out.push_str(":\n");
            let s = serde_yml::to_string(val).unwrap_or_default();
            for line in s.lines() {
                out.push_str("  ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }
}

fn write_yaml_key_value(out: &mut String, key: &str, val: &Value, indent: usize, color: bool) {
    write_indent(out, indent);
    if color {
        out.push_str(&format!("{BLUE}{key}{RESET}"));
    } else {
        out.push_str(key);
    }
    match val {
        Value::Object(map) if !map.is_empty() => {
            out.push_str(":\n");
            for (k, v) in map {
                write_yaml_key_value(out, k, v, indent + 1, color);
            }
        }
        Value::Array(arr) if !arr.is_empty() => {
            out.push_str(":\n");
            for v in arr {
                write_indent(out, indent + 1);
                out.push_str("- ");
                write_yaml_value_inline_or_block(out, v, indent + 2, color);
                out.push('\n');
            }
        }
        _ => {
            out.push_str(": ");
            write_scalar(out, val, color, false);
            out.push('\n');
        }
    }
}

fn write_scalar(out: &mut String, val: &Value, color: bool, flow: bool) {
    match val {
        Value::Null => {
            if color { out.push_str(&format!("{DIM}null{RESET}")); }
            else { out.push_str("null"); }
        }
        Value::Bool(b) => {
            let s = if *b { "true" } else { "false" };
            if color { out.push_str(&format!("{DIM}{s}{RESET}")); }
            else { out.push_str(s); }
        }
        Value::Number(n) => {
            let s = n.to_string();
            if color { out.push_str(&format!("{YELLOW}{s}{RESET}")); }
            else { out.push_str(&s); }
        }
        Value::String(s) => {
            let formatted = if flow {
                yaml_util::yaml_flow_scalar(s)
            } else {
                yaml_util::yaml_block_scalar(s)
            };
            if color { out.push_str(&format!("{GREEN}{formatted}{RESET}")); }
            else { out.push_str(&formatted); }
        }
        _ => {
            write_flow(out, val, color);
        }
    }
}

fn write_yaml_value_inline_or_block(out: &mut String, val: &Value, _indent: usize, color: bool) {
    write_flow(out, val, color);
}

/// Write a value in YAML flow style (inline, like JSON but unquoted keys).
fn write_flow(out: &mut String, val: &Value, color: bool) {
    match val {
        Value::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                if color {
                    out.push_str(&format!("{BLUE}{k}{RESET}: "));
                } else {
                    out.push_str(k);
                    out.push_str(": ");
                }
                write_flow(out, v, color);
            }
            out.push('}');
        }
        Value::Array(arr) => {
            out.push('[');
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_flow(out, v, color);
            }
            out.push(']');
        }
        _ => write_scalar(out, val, color, true),
    }
}

fn write_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}
