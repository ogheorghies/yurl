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

/// Configuration for query array serialization.
/// Controls how `q: {tags: [a, b]}` is encoded in URLs.
#[derive(Clone)]
pub struct QueryArrayConfig {
    pub default: String,
    pub overrides: HashMap<String, String>,
}

impl QueryArrayConfig {
    fn new() -> Self {
        QueryArrayConfig {
            default: ",".to_string(),
            overrides: HashMap::new(),
        }
    }

    /// Build a closure that implements the configured array join strategy.
    pub fn to_join_fn(&self) -> Box<dyn Fn(&str, &[String]) -> Vec<String> + Send + Sync> {
        let default = self.default.clone();
        let overrides = self.overrides.clone();
        Box::new(move |key: &str, vals: &[String]| {
            let sep = overrides.get(key).unwrap_or(&default);
            match sep.as_str() {
                "," => yttp::comma_join(key, vals),
                "&" => yttp::repeat_keys(key, vals),
                "[]" => yttp::bracket_join(key, vals),
                ";" => yttp::semicolon_join(key, vals),
                _ => yttp::comma_join(key, vals),
            }
        })
    }
}

pub fn parse_qarray_value(val: &Value) -> QueryArrayConfig {
    match val {
        Value::String(s) => QueryArrayConfig {
            default: s.clone(),
            overrides: HashMap::new(),
        },
        Value::Array(arr) => {
            let default = arr.first()
                .and_then(|v| v.as_str())
                .unwrap_or(",")
                .to_string();
            let overrides = arr.get(1)
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            QueryArrayConfig { default, overrides }
        }
        _ => QueryArrayConfig::new(),
    }
}

pub struct Config {
    pub default_headers: Map<String, Value>,
    pub default_outputs: Vec<(String, String)>,
    pub global_concurrency: usize,
    pub progress: Progress,
    pub rules: Vec<Rule>,
    pub apis: HashMap<String, String>,
    pub qarray: QueryArrayConfig,
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
            qarray: QueryArrayConfig::new(),
        }
    }

    pub fn parse(json: &Value) -> Result<Self, String> {
        let obj = json.as_object().expect("config must be a JSON object");

        let mut default_headers = obj
            .get("h")
            .or_else(|| obj.get("headers"))
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        expand_env_in_headers(&mut default_headers)?;
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

        let rules: Vec<Rule> = obj
            .get("rules")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().map(parse_rule).collect::<Result<Vec<_>, _>>())
            .transpose()?
            .unwrap_or_default();

        let mut apis = HashMap::new();
        match obj.get("api") {
            Some(Value::String(url)) => {
                apis.insert("api".to_string(), url.clone());
            }
            Some(Value::Object(map)) => {
                for (k, v) in map {
                    if let Some(url) = v.as_str() {
                        apis.insert(k.clone(), url.to_string());
                    }
                }
            }
            _ => {}
        }

        let qarray = obj.get("qarray")
            .map(|v| parse_qarray_value(v))
            .unwrap_or_else(QueryArrayConfig::new);

        Ok(Config {
            default_headers,
            default_outputs,
            global_concurrency,
            progress,
            rules,
            apis,
            qarray,
        })
    }

    /// Return a human-readable one-line summary of the active config.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.apis.is_empty() {
            let names: Vec<&str> = self.apis.keys().map(|s| s.as_str()).collect();
            parts.push(format!("api: {}", names.join(", ")));
        }
        if !self.default_headers.is_empty() {
            let n = self.default_headers.len();
            parts.push(format!("h: {n} header{}", if n == 1 { "" } else { "s" }));
        }
        if !self.rules.is_empty() {
            let n = self.rules.len();
            parts.push(format!("rules: {n}"));
        }
        if self.global_concurrency > 1 {
            parts.push(format!("concurrency: {}", self.global_concurrency));
        }
        if !self.default_outputs.is_empty() {
            let keys: Vec<&str> = self.default_outputs.iter().map(|(k, _)| k.as_str()).collect();
            parts.push(format!("output: {}", keys.join(", ")));
        }
        if parts.is_empty() {
            "(empty)".to_string()
        } else {
            parts.join(" | ")
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
    ) -> Result<Map<String, Value>, String> {
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
            expand_env_in_headers(&mut h)?;
            expand_headers(&mut h);
            for (k, v) in h {
                merged.insert(k, v);
            }
        }

        Ok(merged)
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

fn parse_rule(val: &Value) -> Result<Rule, String> {
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

    expand_env_in_headers(&mut headers)?;
    expand_headers(&mut headers);

    let concurrency = obj
        .get("concurrency")
        .and_then(|v| v.as_u64())
        .map(|v| v.max(1) as usize);

    let cache = obj.get("cache").and_then(cache::parse_cache);

    Ok(Rule {
        match_url,
        match_method,
        match_md,
        headers,
        concurrency,
        cache,
    })
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
pub fn expand_env_in_headers(headers: &mut Map<String, Value>) -> Result<(), String> {
    for (_k, v) in headers.iter_mut() {
        expand_env_value(v)?;
    }
    Ok(())
}

fn expand_env_value(v: &mut Value) -> Result<(), String> {
    match v {
        Value::String(s) => {
            if let Some(var) = s.strip_prefix('$') {
                if !var.is_empty() && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    match std::env::var(var) {
                        Ok(val) => *s = val,
                        Err(std::env::VarError::NotPresent) => {
                            return Err(format!("undefined environment variable: ${var}"));
                        }
                        Err(std::env::VarError::NotUnicode(_)) => {
                            return Err(format!("environment variable ${var} is not valid UTF-8"));
                        }
                    }
                }
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                expand_env_value(item)?;
            }
        }
        _ => {}
    }
    Ok(())
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
