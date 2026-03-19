use serde_json::Value;

// ANSI color codes
const BLUE: &str = "\x1b[34m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

/// Render a JSON value with pretty printing, but inline objects/arrays
/// that are marked as compact (status inline).
pub fn to_pretty_json(val: &Value, inline_keys: &[&str], color: bool) -> String {
    let mut out = String::new();
    write_value(&mut out, val, 0, inline_keys, color);
    out.push('\n');
    out
}

fn write_value(out: &mut String, val: &Value, indent: usize, inline_keys: &[&str], color: bool) {
    match val {
        Value::Object(map) => {
            out.push_str("{\n");
            let entries: Vec<_> = map.iter().collect();
            for (i, (k, v)) in entries.iter().enumerate() {
                write_indent(out, indent + 1);
                if color {
                    out.push_str(&format!("{BLUE}{}{RESET}: ", serde_json::to_string(k).unwrap()));
                } else {
                    out.push_str(&format!("{}: ", serde_json::to_string(k).unwrap()));
                }
                if inline_keys.contains(&k.as_str()) {
                    write_inline_object(out, v, color);
                } else {
                    write_value(out, v, indent + 1, inline_keys, color);
                }
                if i < entries.len() - 1 {
                    out.push(',');
                }
                out.push('\n');
            }
            write_indent(out, indent);
            out.push('}');
        }
        Value::Array(arr) => {
            out.push_str("[\n");
            for (i, v) in arr.iter().enumerate() {
                write_indent(out, indent + 1);
                write_value(out, v, indent + 1, inline_keys, color);
                if i < arr.len() - 1 {
                    out.push(',');
                }
                out.push('\n');
            }
            write_indent(out, indent);
            out.push(']');
        }
        _ => write_scalar(out, val, color),
    }
}

fn write_scalar(out: &mut String, val: &Value, color: bool) {
    if !color {
        out.push_str(&serde_json::to_string(val).unwrap());
        return;
    }
    match val {
        Value::String(_) => {
            out.push_str(&format!("{GREEN}{}{RESET}", serde_json::to_string(val).unwrap()));
        }
        Value::Number(_) => {
            out.push_str(&format!("{YELLOW}{}{RESET}", serde_json::to_string(val).unwrap()));
        }
        Value::Bool(_) | Value::Null => {
            out.push_str(&format!("{DIM}{}{RESET}", serde_json::to_string(val).unwrap()));
        }
        _ => {
            out.push_str(&serde_json::to_string(val).unwrap());
        }
    }
}

fn write_inline_object(out: &mut String, val: &Value, color: bool) {
    if let Value::Object(map) = val {
        out.push('{');
        for (i, (k, v)) in map.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            if color {
                out.push_str(&format!("{BLUE}{}{RESET}: ", serde_json::to_string(k).unwrap()));
                write_scalar(out, v, true);
            } else {
                out.push_str(&format!("{}: {}", serde_json::to_string(k).unwrap(), serde_json::to_string(v).unwrap()));
            }
        }
        out.push('}');
    } else {
        write_scalar(out, val, color);
    }
}

fn write_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}
