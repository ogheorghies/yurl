use serde_json::{Map, Value};
use std::collections::HashMap;

use crate::cache::{self, CacheConfig};
use yttp::expand_headers;

pub fn is_output_key(key: &str) -> bool {
    key == "1" || key == "2" || key.starts_with("file://")
}

pub struct Rule {
    pub match_url: Option<String>,
    pub match_method: Option<String>,
    pub match_md: Vec<(String, Value)>,
    pub headers: Map<String, Value>,
    pub concurrency: Option<usize>,
    pub cache: Option<CacheConfig>,
}

pub enum Progress {
    Off,
    On,
    Known(u64),
}

pub struct Config {
    pub default_headers: Map<String, Value>,
    pub default_outputs: Vec<(String, String)>,
    pub global_concurrency: usize,
    pub progress: Progress,
    pub rules: Vec<Rule>,
    pub apis: HashMap<String, String>,
}

impl Config {
    pub fn empty() -> Self {
        Config {
            default_headers: Map::new(),
            default_outputs: Vec::new(),
            global_concurrency: 1,
            progress: Progress::Off,
            rules: Vec::new(),
            apis: HashMap::new(),
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

        expand_env_vars(&mut default_headers);
        expand_headers(&mut default_headers);

        let mut default_outputs = Vec::new();
        for (k, v) in obj {
            if is_output_key(k) {
                if let Some(fmt) = v.as_str() {
                    default_outputs.push((k.clone(), fmt.to_string()));
                }
            }
        }

        let global_concurrency = obj
            .get("concurrency")
            .and_then(|v| v.as_u64())
            .map(|v| v.max(1) as usize)
            .unwrap_or(1);

        let progress = match obj.get("progress") {
            Some(Value::Bool(true)) => Progress::On,
            Some(Value::Number(n)) => {
                Progress::Known(n.as_u64().unwrap_or(0).max(1))
            }
            _ => Progress::Off,
        };

        let rules = obj
            .get("rules")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().map(parse_rule).collect())
            .unwrap_or_default();

        let mut apis = HashMap::new();
        if let Some(Value::String(url)) = obj.get("api") {
            apis.insert("api".to_string(), url.clone());
        }
        if let Some(Value::Object(map)) = obj.get("apis") {
            for (k, v) in map {
                if let Some(url) = v.as_str() {
                    apis.insert(k.clone(), url.to_string());
                }
            }
        }

        Config {
            default_headers,
            default_outputs,
            global_concurrency,
            progress,
            rules,
            apis,
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

        merged
    }

    /// Return the cache config from the first matching rule with a cache field.
    pub fn resolve_cache(
        &self,
        method: &str,
        url: &str,
        md: &Option<Value>,
    ) -> Option<CacheConfig> {
        for rule in &self.rules {
            if rule.cache.is_some() && rule_matches(rule, method, url, md) {
                return rule.cache.clone();
            }
        }
        None
    }

    /// Return indices of rules that match and have a concurrency limit.
    pub fn matching_concurrency_rules(
        &self,
        method: &str,
        url: &str,
        md: &Option<Value>,
    ) -> Vec<usize> {
        self.rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| rule.concurrency.is_some() && rule_matches(rule, method, url, md))
            .map(|(i, _)| i)
            .collect()
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

    expand_env_vars(&mut headers);
    expand_headers(&mut headers);

    let concurrency = obj
        .get("concurrency")
        .and_then(|v| v.as_u64())
        .map(|v| v.max(1) as usize);

    let cache = obj.get("cache").and_then(cache::parse_cache);

    Rule {
        match_url,
        match_method,
        match_md,
        headers,
        concurrency,
        cache,
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

/// Expand `$VAR` references in header values from environment variables.
/// Only pure `$VAR` values are expanded (the entire string is `$` + alphanumeric/underscore).
/// Also expands inside arrays (e.g. `[user, $PASS]` for basic auth).
fn expand_env_vars(headers: &mut Map<String, Value>) {
    for (_k, v) in headers.iter_mut() {
        expand_env_value(v);
    }
}

fn expand_env_value(v: &mut Value) {
    match v {
        Value::String(s) => {
            if let Some(var) = s.strip_prefix('$') {
                if !var.is_empty() && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    if let Ok(val) = std::env::var(var) {
                        *s = val;
                    }
                }
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                expand_env_value(item);
            }
        }
        _ => {}
    }
}

/// Expand `name!/path` API aliases in a URL, then auto-detect scheme if missing.
pub fn expand_api_url(url: &str, apis: &HashMap<String, String>) -> String {
    let expanded = if let Some(pos) = url.find('!') {
        let name = &url[..pos];
        if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            if let Some(base) = apis.get(name) {
                let rest = &url[pos + 1..];
                format!("{}{}", base.trim_end_matches('/'), rest)
            } else {
                url.to_string()
            }
        } else {
            url.to_string()
        }
    } else {
        url.to_string()
    };
    add_scheme(&expanded)
}

/// If a URL has no scheme, prepend https:// (or http:// for localhost/loopback/bare hosts).
fn add_scheme(url: &str) -> String {
    if url.contains("://") {
        return url.to_string();
    }
    let host = url.split('/').next().unwrap_or(url);
    let host_no_port = host.split(':').next().unwrap_or(host);
    if host_no_port == "localhost" || host_no_port == "127.0.0.1" || host_no_port == "[::1]"
        || !host_no_port.contains('.')
    {
        format!("http://{url}")
    } else {
        format!("https://{url}")
    }
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
