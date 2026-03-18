use serde_json::Value;

/// Render a JSON value with pretty printing, but inline objects/arrays
/// that are marked as compact (status inline).
pub fn to_pretty_json(val: &Value, inline_keys: &[&str]) -> String {
    let mut out = String::new();
    write_value(&mut out, val, 0, inline_keys);
    out.push('\n');
    out
}

fn write_value(out: &mut String, val: &Value, indent: usize, inline_keys: &[&str]) {
    match val {
        Value::Object(map) => {
            out.push_str("{\n");
            let entries: Vec<_> = map.iter().collect();
            for (i, (k, v)) in entries.iter().enumerate() {
                write_indent(out, indent + 1);
                out.push_str(&format!("{}: ", serde_json::to_string(k).unwrap()));
                if inline_keys.contains(&k.as_str()) {
                    // Compact one-line JSON with spaces
                    write_inline_object(out, v);
                } else {
                    write_value(out, v, indent + 1, inline_keys);
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
                write_value(out, v, indent + 1, inline_keys);
                if i < arr.len() - 1 {
                    out.push(',');
                }
                out.push('\n');
            }
            write_indent(out, indent);
            out.push(']');
        }
        _ => {
            out.push_str(&serde_json::to_string(val).unwrap());
        }
    }
}

fn write_inline_object(out: &mut String, val: &Value) {
    if let Value::Object(map) = val {
        out.push('{');
        for (i, (k, v)) in map.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&format!("{}: {}", serde_json::to_string(k).unwrap(), serde_json::to_string(v).unwrap()));
        }
        out.push('}');
    } else {
        out.push_str(&serde_json::to_string(val).unwrap());
    }
}

fn write_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}
