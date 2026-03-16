use serde_json::{Map, Value};

use crate::shortcut::{expand_headers, extract_shortcut_headers};

pub fn is_output_key(key: &str) -> bool {
    key == "1" || key == "2" || key.starts_with("file://")
}

pub struct Rule {
    pub match_url: Option<String>,
    pub match_method: Option<String>,
    pub match_md: Vec<(String, Value)>,
    pub headers: Map<String, Value>,
}

pub struct Config {
    pub default_headers: Map<String, Value>,
    pub default_outputs: Vec<(String, String)>,
    pub rules: Vec<Rule>,
}

impl Config {
    pub fn empty() -> Self {
        Config {
            default_headers: Map::new(),
            default_outputs: Vec::new(),
            rules: Vec::new(),
        }
    }

    pub fn parse(json: &Value) -> Self {
        let obj = json.as_object().expect("config must be a JSON object");

        let mut default_headers = obj
            .get("h")
            .or_else(|| obj.get("headers"))
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        // Merge ct/auth shortcut keys
        for (k, v) in extract_shortcut_headers(obj) {
            default_headers.entry(k).or_insert(v);
        }
        expand_headers(&mut default_headers);

        let mut default_outputs = Vec::new();
        for (k, v) in obj {
            if is_output_key(k) {
                if let Some(fmt) = v.as_str() {
                    default_outputs.push((k.clone(), fmt.to_string()));
                }
            }
        }

        let rules = obj
            .get("rules")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().map(parse_rule).collect())
            .unwrap_or_default();

        Config {
            default_headers,
            default_outputs,
            rules,
        }
    }

    /// Compute merged headers: defaults → matching rules → per-request.
    /// Later values override earlier ones for the same key.
    pub fn resolve_headers(
        &self,
        method: &str,
        url: &str,
        md: &Option<Value>,
        request_headers: &Option<Value>,
        request_obj: &Map<String, Value>,
    ) -> Map<String, Value> {
        let mut merged = self.default_headers.clone();

        for rule in &self.rules {
            if rule_matches(rule, method, url, md) {
                for (k, v) in &rule.headers {
                    merged.insert(k.clone(), v.clone());
                }
            }
        }

        if let Some(Value::Object(h)) = request_headers {
            let mut h = h.clone();
            expand_headers(&mut h);
            for (k, v) in h {
                merged.insert(k, v);
            }
        }

        // Merge ct/auth from request level (highest priority)
        for (k, v) in extract_shortcut_headers(request_obj) {
            merged.insert(k, v);
        }

        merged
    }
}

fn parse_rule(val: &Value) -> Rule {
    let obj = val.as_object().expect("rule must be a JSON object");

    let match_obj = obj.get("match").and_then(|v| v.as_object());

    let mut match_url = None;
    let mut match_method = None;
    let mut match_md = Vec::new();

    if let Some(m) = match_obj {
        if let Some(u) = m.get("u").and_then(|v| v.as_str()) {
            match_url = Some(u.to_string());
        }
        if let Some(method) = m.get("m").and_then(|v| v.as_str()) {
            match_method = Some(method.to_uppercase());
        }
        for (k, v) in m {
            if k.starts_with("md.") {
                match_md.push((k[3..].to_string(), v.clone()));
            }
        }
    }

    let mut headers = obj
        .get("h")
        .or_else(|| obj.get("headers"))
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    for (k, v) in extract_shortcut_headers(obj) {
        headers.entry(k).or_insert(v);
    }
    expand_headers(&mut headers);

    Rule {
        match_url,
        match_method,
        match_md,
        headers,
    }
}

fn rule_matches(rule: &Rule, method: &str, url: &str, md: &Option<Value>) -> bool {
    if let Some(pattern) = &rule.match_url {
        if !glob_match(pattern, url) {
            return false;
        }
    }

    if let Some(m) = &rule.match_method {
        if m != method {
            return false;
        }
    }

    for (field, expected) in &rule.match_md {
        let actual = md
            .as_ref()
            .and_then(|v| v.as_object())
            .and_then(|obj| obj.get(field));
        if actual != Some(expected) {
            return false;
        }
    }

    true
}

/// Simple glob matching supporting `*` (any chars except `/`) and `**` (any chars including `/`).
fn glob_match(pattern: &str, text: &str) -> bool {
    let mut parts: Vec<&str> = Vec::new();
    let mut rest = pattern;

    // Split pattern on ** and * into literal segments and wildcards
    while !rest.is_empty() {
        if rest.starts_with("**") {
            parts.push("**");
            rest = &rest[2..];
        } else if rest.starts_with('*') {
            parts.push("*");
            rest = &rest[1..];
        } else {
            let end = rest.find('*').unwrap_or(rest.len());
            parts.push(&rest[..end]);
            rest = &rest[end..];
        }
    }

    glob_match_parts(&parts, text)
}

fn glob_match_parts(parts: &[&str], text: &str) -> bool {
    if parts.is_empty() {
        return text.is_empty();
    }

    let part = parts[0];
    let remaining = &parts[1..];

    match part {
        "**" => {
            // Match any number of characters (including /)
            for i in 0..=text.len() {
                if glob_match_parts(remaining, &text[i..]) {
                    return true;
                }
            }
            false
        }
        "*" => {
            // Match any number of characters except /
            for i in 0..=text.len() {
                if i > 0 && text.as_bytes()[i - 1] == b'/' {
                    break;
                }
                if glob_match_parts(remaining, &text[i..]) {
                    return true;
                }
            }
            false
        }
        literal => {
            if text.starts_with(literal) {
                glob_match_parts(remaining, &text[literal.len()..])
            } else {
                false
            }
        }
    }
}
