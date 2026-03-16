use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Map, Value};

/// Expand shortcut syntax in a header value string.
pub fn expand_value(s: &str) -> String {
    // Auth shortcuts
    if let Some(rest) = s.strip_prefix("basic!") {
        return format!("Basic {}", STANDARD.encode(rest.as_bytes()));
    }
    if let Some(rest) = s.strip_prefix("bearer!") {
        return format!("Bearer {rest}");
    }

    // Full MIME shortcuts
    match s {
        "json!" | "j!" => return "application/json".to_string(),
        "form!" | "f!" => return "application/x-www-form-urlencoded".to_string(),
        "multi!" | "m!" => return "multipart/form-data".to_string(),
        "html!" | "h!" => return "text/html".to_string(),
        "text!" | "t!" => return "text/plain".to_string(),
        "xml!" | "x!" => return "application/xml".to_string(),
        _ => {}
    }

    // Prefix expansions: a!/suffix, t!/suffix, i!/suffix
    if let Some(rest) = s.strip_prefix("a!/") {
        return format!("application/{rest}");
    }
    if let Some(rest) = s.strip_prefix("t!/") {
        return format!("text/{rest}");
    }
    if let Some(rest) = s.strip_prefix("i!/") {
        return format!("image/{rest}");
    }

    s.to_string()
}

/// Expand shortcut key to full header name.
fn expand_key(key: &str) -> Option<&'static str> {
    match key {
        "a!" | "auth!" => Some("Authorization"),
        "c!" | "ct!" => Some("Content-Type"),
        _ => None,
    }
}

/// Expand shortcut keys and values in a headers map.
/// Keys like `a!`, `auth!`, `c!`, `ct!` expand to their full header names.
/// Values expand via `expand_value`.
pub fn expand_headers(headers: &mut Map<String, Value>) {
    // First collect key expansions to avoid mutating while iterating
    let expansions: Vec<(String, String, Value)> = headers
        .iter()
        .filter_map(|(k, v)| {
            expand_key(k).map(|full| (k.clone(), full.to_string(), v.clone()))
        })
        .collect();

    for (old_key, new_key, val) in expansions {
        headers.remove(&old_key);
        headers.insert(new_key, val);
    }

    // Then expand values
    for (_, v) in headers.iter_mut() {
        if let Value::String(s) = v {
            let expanded = expand_value(s);
            if expanded != *s {
                *v = Value::String(expanded);
            }
        }
    }
}

/// Extract shortcut keys (`a!`, `auth!`, `c!`, `ct!`) from a JSON object as headers.
pub fn extract_shortcut_headers(obj: &Map<String, Value>) -> Map<String, Value> {
    let mut extra = Map::new();
    for (k, v) in obj {
        if let Some(full) = expand_key(k) {
            if let Some(s) = v.as_str() {
                extra.insert(full.to_string(), Value::String(expand_value(s)));
            }
        }
    }
    extra
}
