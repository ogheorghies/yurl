use serde_json::Value;

/// Render a JSON value as YAML, with specified keys rendered inline (flow style).
pub fn to_yaml(val: &Value, inline_keys: &[&str]) -> String {
    let mut out = String::new();
    if let Value::Object(map) = val {
        for (k, v) in map {
            if inline_keys.contains(&k.as_str()) {
                out.push_str(k);
                out.push_str(": ");
                write_flow(&mut out, v);
                out.push('\n');
            } else {
                write_yaml_key_value(&mut out, k, v, 0);
            }
        }
    }
    out
}

fn write_yaml_key_value(out: &mut String, key: &str, val: &Value, indent: usize) {
    write_indent(out, indent);
    out.push_str(key);
    match val {
        Value::Object(map) if !map.is_empty() => {
            out.push_str(":\n");
            for (k, v) in map {
                write_yaml_key_value(out, k, v, indent + 1);
            }
        }
        Value::Array(arr) if !arr.is_empty() => {
            out.push_str(":\n");
            for v in arr {
                write_indent(out, indent + 1);
                out.push_str("- ");
                write_yaml_value_inline_or_block(out, v, indent + 2);
                out.push('\n');
            }
        }
        _ => {
            out.push_str(": ");
            write_yaml_scalar(out, val);
            out.push('\n');
        }
    }
}

fn write_yaml_scalar(out: &mut String, val: &Value) {
    match val {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => {
            // Quote if needed
            if s.is_empty()
                || s.contains(':')
                || s.contains('#')
                || s.contains('\n')
                || s.starts_with(' ')
                || s.starts_with('"')
                || s.starts_with('\'')
                || s == "true"
                || s == "false"
                || s == "null"
            {
                out.push_str(&format!("'{}'", s.replace('\'', "''")));
            } else {
                out.push_str(s);
            }
        }
        _ => {
            // Complex value as flow
            write_flow(out, val);
        }
    }
}

fn write_yaml_value_inline_or_block(out: &mut String, val: &Value, _indent: usize) {
    write_flow(out, val);
}

/// Write a value in YAML flow style (inline, like JSON but unquoted keys).
fn write_flow(out: &mut String, val: &Value) {
    match val {
        Value::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(k);
                out.push_str(": ");
                write_flow(out, v);
            }
            out.push('}');
        }
        Value::Array(arr) => {
            out.push('[');
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_flow(out, v);
            }
            out.push(']');
        }
        _ => write_yaml_scalar(out, val),
    }
}

fn write_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}
