mod atom;
mod cache;
mod config;
mod error;
mod format_json;
mod format_yaml;
mod interactive;
mod template;

use arc_swap::ArcSwap;
use argh::FromArgs;
use atom::{Atom, Format, RequestData, ResponseData, StatusParts, UrlParts, parse_format, render, render_color};
use config::{Config, Progress};
use futures_util::StreamExt;
use template::expand_path;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use reqwest::Client;
use serde_json::{Map, Value};
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;
use url::Url;

/// JSON-driven HTTP client with shortcuts, flexible output routing, and rule-based middleware.
#[derive(FromArgs)]
struct Args {
    /// print version
    #[argh(switch, short = 'v')]
    version: bool,

    /// step through piped stdin requests interactively (.next / .go)
    #[argh(switch)]
    step: bool,

    /// print reference card
    #[argh(switch, long = "ref")]
    reference: bool,

    /// batch config JSON (default headers, output format, rules)
    #[argh(positional)]
    config: Option<String>,
}

fn resolve_method(key: &str) -> Option<&'static str> {
    match key.to_lowercase().as_str() {
        "get" | "g" => Some("GET"),
        "post" | "p" => Some("POST"),
        "put" => Some("PUT"),
        "delete" | "d" => Some("DELETE"),
        "patch" => Some("PATCH"),
        "head" => Some("HEAD"),
        "options" => Some("OPTIONS"),
        "trace" => Some("TRACE"),
        _ => None,
    }
}

enum Dest {
    Stdout,
    StdoutStream,
    Stderr,
    StderrStream,
    FilePath(String),
    FileStream(String),
}

struct OutputBuffer {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
}

fn path_has_unique_template(path: &str) -> bool {
    path.contains("{{idx}}")
}

fn parse_dest(key: &str) -> Dest {
    if key == "1" {
        Dest::Stdout
    } else if key == "2" {
        Dest::Stderr
    } else {
        let path = key.strip_prefix("file://").unwrap();
        if let Some(base) = path.strip_suffix("?stream") {
            Dest::FileStream(base.to_string())
        } else {
            // Auto-stream decision deferred — need to check format is Raw(B)
            Dest::FilePath(path.to_string())
        }
    }
}

/// Promote destinations to streaming when safe: format is raw body and
/// either the path contains {{idx}} (unique per request) or concurrency is 1.
fn maybe_auto_stream(dest: &mut Dest, fmt: &Format, concurrent: bool) {
    if !matches!(fmt, Format::Raw(Atom::B)) {
        return;
    }
    match dest {
        Dest::FilePath(path) => {
            if path_has_unique_template(path) || !concurrent {
                *dest = Dest::FileStream(std::mem::take(path));
            }
        }
        Dest::Stdout if !concurrent => *dest = Dest::StdoutStream,
        Dest::Stderr if !concurrent => *dest = Dest::StderrStream,
        _ => {}
    }
}


use error::RequestError;

fn parse_input(s: &str) -> Result<Value, RequestError> {
    yttp::parse(s).map_err(|e| RequestError::from_parse(s, e))
}

/// Flags for the `.x` command — composable modifiers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExpandFlags {
    pub merged: bool,    // m — merge config headers/rules
    pub vertical: bool,  // v — multiline output
    pub json: bool,      // j — JSON format
    pub curl: bool,      // c — curl format
    pub short: bool,     // s — collapse headers to yttp shortcuts
}

impl ExpandFlags {
    /// Parse flags from a string like "mv", "jv", "mcs". Returns error for invalid flags.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut flags = ExpandFlags::default();
        for ch in s.chars() {
            match ch {
                'm' => flags.merged = true,
                'v' => flags.vertical = true,
                'h' => flags.vertical = false, // explicit horizontal
                'j' => flags.json = true,
                'y' => flags.json = false, // explicit YAML
                'c' => flags.curl = true,
                's' => flags.short = true,
                _ => return Err(format!("unknown flag: {ch} (valid: m v h j y c s)")),
            }
        }
        if flags.curl && flags.json {
            return Err("c and j are mutually exclusive".to_string());
        }
        Ok(flags)
    }

    /// Whether the output can be pre-filled for editing (single-line).
    pub fn is_editable(&self) -> bool {
        !self.vertical && !self.curl
    }
}

/// Parse a request line, extracting fields. Does NOT resolve headers.
fn parse_request_fields(line: &str, config: &Config) -> Result<(
    &'static str,                  // method
    String,                        // url (with API alias expanded)
    Option<Value>,                 // query
    Option<Value>,                 // req_headers (raw, unexpanded)
    Option<Value>,                 // body
    Option<Value>,                 // md
    Vec<(String, String)>,         // outputs
), RequestError> {
    let json = parse_input(line)?;
    let obj = json.as_object().ok_or_else(|| RequestError::Structure {
        msg: "request must be a JSON/YAML object".to_string(),
    })?;

    let mut method = None;
    let mut url = None;
    let mut req_headers = None;
    let mut body = None;
    let mut md = None;
    let mut query = None;
    let mut outputs: Vec<(String, String)> = Vec::new();

    for (key, val) in obj {
        if let Some(m) = resolve_method(key) {
            method = Some(m);
            url = Some(
                val.as_str()
                    .ok_or_else(|| RequestError::Structure {
                        msg: format!("URL for '{key}' must be a string"),
                    })?
                    .to_string(),
            );
        } else if config::is_output_key(key) {
            outputs.push((
                key.clone(),
                val.as_str()
                    .ok_or_else(|| RequestError::Structure {
                        msg: format!("output key '{key}' must have a string value"),
                    })?
                    .to_string(),
            ));
        } else {
            match key.to_lowercase().as_str() {
                "h" | "headers" => req_headers = Some(val.clone()),
                "b" | "body" => body = Some(val.clone()),
                "md" => md = Some(val.clone()),
                "q" | "query" => query = Some(val.clone()),
                _ => return Err(RequestError::Structure {
                    msg: format!("unknown key: {key}"),
                }),
            }
        }
    }

    let method = method.unwrap_or("GET");
    let url = config::expand_api_url(&url.unwrap_or_default(), &config.apis);

    Ok((method, url, query, req_headers, body, md, outputs))
}

/// Resolve headers — base (request only) or merged (+ config defaults + rules).
fn resolve_headers_for_expand(
    config: &Config,
    method: &str,
    url: &str,
    md: &Option<Value>,
    req_headers: &Option<Value>,
    merged: bool,
) -> Map<String, Value> {
    if merged {
        config.resolve_headers(method, url, md, req_headers)
    } else {
        // Base: only expand shortcuts in request's own headers
        if let Some(Value::Object(h)) = req_headers {
            let mut h = h.clone();
            yttp::expand_headers(&mut h);
            h
        } else {
            Map::new()
        }
    }
}

/// Unified expand function — resolves request and renders with given flags.
pub fn expand_with_flags(line: &str, config: &Config, flags: &ExpandFlags) -> Result<String, RequestError> {
    let (method, url, query, req_headers, body, md, outputs) =
        parse_request_fields(line, config)?;

    let mut headers = resolve_headers_for_expand(
        config, method, &url, &md, &req_headers, flags.merged,
    );

    // Apply short headers if requested (not for curl)
    if flags.short && !flags.curl {
        yttp::collapse_headers(&mut headers);
    }

    // Inline query into URL
    let mut full_url = url;
    yttp::append_query_to_url(&mut full_url, &query).ok();

    if flags.curl {
        return Ok(render_curl(method, &full_url, &headers, &body, flags.vertical));
    }

    // Build result object
    let mut result = Map::new();
    result.insert(method.to_lowercase(), Value::String(full_url));
    if !headers.is_empty() {
        result.insert("h".to_string(), Value::Object(headers));
    }
    if let Some(b) = body {
        result.insert("b".to_string(), b);
    }
    if let Some(m) = md {
        result.insert("md".to_string(), m);
    }
    for (k, v) in outputs {
        result.insert(k, Value::String(v));
    }

    if flags.json {
        if flags.vertical {
            Ok(serde_json::to_string_pretty(&Value::Object(result)).unwrap_or_default())
        } else {
            Ok(serde_json::to_string(&Value::Object(result)).unwrap_or_default())
        }
    } else if flags.vertical {
        Ok(to_yaml_block(&Value::Object(result), 0))
    } else {
        Ok(to_yaml_flow(&Value::Object(result)))
    }
}

fn render_curl(method: &str, url: &str, headers: &Map<String, Value>, body: &Option<Value>, vertical: bool) -> String {
    let mut parts = vec![format!("curl -X {method} '{url}'")];
    for (k, v) in headers {
        let v_str = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        parts.push(format!("-H '{k}: {v_str}'"));
    }
    if let Some(b) = body {
        let body_str = match b {
            Value::String(s) => s.clone(),
            other => serde_json::to_string(other).unwrap_or_default(),
        };
        parts.push(format!("-d '{body_str}'"));
    }
    if vertical {
        parts.join(" \\\n  ")
    } else {
        parts.join(" ")
    }
}

/// Legacy resolve_request — used by execute(). Always merges config.
fn resolve_request<'a>(line: &str, config: &'a Config) -> Result<(
    &'a str,                       // method
    String,                        // url (with API alias expanded)
    Option<Value>,                 // query
    Map<String, Value>,            // merged headers
    Option<Value>,                 // body
    Option<Value>,                 // md
    Vec<(String, String)>,         // outputs
), RequestError> {
    let (method, url, query, req_headers, body, md, outputs) =
        parse_request_fields(line, config)?;
    let merged_headers = config.resolve_headers(method, &url, &md, &req_headers);
    Ok((method, url, query, merged_headers, body, md, outputs))
}

// Old expand_request / expand_request_structured / preview_request_* functions
// are replaced by expand_with_flags(). See ExpandFlags for the composable modifier system.

/// Serialize a serde_json::Value as multiline block YAML.
fn to_yaml_block(val: &Value, indent: usize) -> String {
    let prefix = "  ".repeat(indent);
    match val {
        Value::Null => "null\n".to_string(),
        Value::Bool(b) => format!("{b}\n"),
        Value::Number(n) => format!("{n}\n"),
        Value::String(s) => {
            if s.contains('\n') {
                let mut out = "|\n".to_string();
                for line in s.lines() {
                    out.push_str(&format!("{prefix}  {line}\n"));
                }
                out
            } else if needs_yaml_quoting(s) {
                format!("'{}'\n", s.replace('\'', "''"))
            } else {
                format!("{s}\n")
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                return "[]\n".to_string();
            }
            let mut out = "\n".to_string();
            for item in arr {
                let rendered = to_yaml_block(item, indent + 1);
                let rendered = rendered.trim_start();
                out.push_str(&format!("{prefix}- {rendered}"));
            }
            out
        }
        Value::Object(map) => {
            if map.is_empty() {
                return "{}\n".to_string();
            }
            let mut out = "\n".to_string();
            for (k, v) in map {
                let key = if needs_yaml_quoting(k) {
                    format!("'{}'", k.replace('\'', "''"))
                } else {
                    k.to_string()
                };
                match v {
                    Value::Object(_) | Value::Array(_) => {
                        let rendered = to_yaml_block(v, indent + 1);
                        out.push_str(&format!("{prefix}{key}:{rendered}"));
                    }
                    _ => {
                        let rendered = to_yaml_block(v, indent + 1);
                        out.push_str(&format!("{prefix}{key}: {rendered}"));
                    }
                }
            }
            out
        }
    }
}

/// Serialize a serde_json::Value as single-line YAML flow style.
fn to_yaml_flow(val: &Value) -> String {
    match val {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => yaml_flow_scalar(s),
        Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(to_yaml_flow).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Object(map) => {
            let pairs: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", yaml_flow_key(k), to_yaml_flow(v)))
                .collect();
            format!("{{{}}}", pairs.join(", "))
        }
    }
}

/// Quote a YAML flow key. Numbers are left unquoted since they're valid as keys.
fn yaml_flow_key(s: &str) -> String {
    if s.is_empty() || needs_yaml_key_quoting(s) {
        yaml_flow_scalar(s)
    } else {
        s.to_string()
    }
}

fn needs_yaml_key_quoting(s: &str) -> bool {
    // Numbers are fine as keys — {1: val} is valid YAML
    if s.parse::<f64>().is_ok() {
        return false;
    }
    needs_yaml_quoting(s)
}

fn yaml_flow_scalar(s: &str) -> String {
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

fn needs_yaml_quoting(s: &str) -> bool {
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

async fn execute(line: &str, client: &Client, idx: usize, config: &Config, concurrent: bool, yaml_mode: bool, cache_stores: Option<&cache::CacheStores>, color_stdout: bool, color_stderr: bool) -> Result<OutputBuffer, RequestError> {
    let json = parse_input(line)?;
    let obj = json.as_object().ok_or_else(|| RequestError::Structure {
        msg: "request must be a JSON/YAML object".to_string(),
    })?;

    let mut method = None;
    let mut url = None;
    let mut req_headers = None;
    let mut req_body = None;
    let mut md = None;
    let mut query = None;
    let mut outputs: Vec<(Dest, Format)> = Vec::new();

    for (key, val) in obj {
        if let Some(m) = resolve_method(key) {
            method = Some(m);
            url = Some(
                val.as_str()
                    .ok_or_else(|| RequestError::Structure {
                        msg: format!("URL for '{key}' must be a string"),
                    })?
                    .to_string(),
            );
        } else if config::is_output_key(key) {
            let dest = parse_dest(key);
            let fmt_str = val.as_str()
                .ok_or_else(|| RequestError::Structure {
                    msg: format!("output key '{key}' must have a string value"),
                })?;
            let fmt = parse_format(fmt_str).map_err(|e| RequestError::Structure {
                msg: e,
            })?;
            outputs.push((dest, fmt));
        } else {
            match key.to_lowercase().as_str() {
                "h" | "headers" => req_headers = Some(val.clone()),
                "b" | "body" => req_body = Some(val.clone()),
                "md" => md = Some(val.clone()),
                "q" | "query" => query = Some(val.clone()),
                _ => return Err(RequestError::Structure {
                    msg: format!("unknown key: {key}"),
                }),
            }
        }
    }

    if outputs.is_empty() {
        if config.default_outputs.is_empty() {
            let default_atoms = vec![Atom::SInline, Atom::H, Atom::B];
            let default_fmt = if yaml_mode {
                Format::Yaml(default_atoms)
            } else {
                Format::Json(default_atoms)
            };
            outputs.push((Dest::Stdout, default_fmt));
        } else {
            for (key, fmt_str) in &config.default_outputs {
                let fmt = parse_format(fmt_str).map_err(|e| RequestError::Structure { msg: e })?;
                outputs.push((parse_dest(key), fmt));
            }
        }
    }

    // Auto-promote file destinations to streaming when safe
    for (dest, fmt) in &mut outputs {
        maybe_auto_stream(dest, fmt, concurrent);
    }

    let method = method.ok_or_else(|| RequestError::Structure {
        msg: "no HTTP method found (use g, p, put, d, patch, ...)".to_string(),
    })?;
    let mut url = config::expand_api_url(&url.unwrap_or_default(), &config.apis);
    yttp::append_query_to_url(&mut url, &query).ok();

    let merged_headers = config.resolve_headers(method, &url, &md, &req_headers);

    let mut req = client.request(method.parse().unwrap(), &url);

    let content_type = merged_headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .and_then(|(_, v)| v.as_str())
        .unwrap_or("")
        .to_lowercase();

    let mut req_headers_raw = String::new();
    let mut req_headers_json = Map::new();
    for (k, v) in &merged_headers {
        let v_str = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if k.eq_ignore_ascii_case("content-type") && content_type.starts_with("multipart/form-data")
        {
            continue;
        }
        req = req.header(k.as_str(), v_str.as_str());
        req_headers_raw.push_str(&format!("{k}: {v_str}\r\n"));
        req_headers_json.insert(k.clone(), Value::String(v_str));
    }

    if let Some(b) = &req_body {
        if content_type.starts_with("application/x-www-form-urlencoded") {
            if let Value::Object(fields) = b {
                let params: Vec<(String, String)> = fields
                    .iter()
                    .map(|(k, v)| {
                        let val = match v {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        (k.clone(), val)
                    })
                    .collect();
                req = req.form(&params);
            } else {
                req = req.body(b.as_str().unwrap().to_string());
            }
        } else if content_type.starts_with("multipart/form-data") {
            if let Value::Object(fields) = b {
                let mut form = reqwest::multipart::Form::new();
                for (k, v) in fields {
                    match v {
                        Value::String(s) if s.starts_with("file://") => {
                            let path = s[7..].to_string();
                            let file_bytes = std::fs::read(&path)
                                .unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
                            let file_name = std::path::Path::new(&path)
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            let part = reqwest::multipart::Part::bytes(file_bytes)
                                .file_name(file_name);
                            form = form.part(k.clone(), part);
                        }
                        Value::String(s) => {
                            form = form.text(k.clone(), s.clone());
                        }
                        other => {
                            form = form.text(k.clone(), other.to_string());
                        }
                    }
                }
                req = req.multipart(form);
            }
        } else {
            req = req.json(b);
        }
    }

    let parsed = Url::parse(&url).map_err(|e| RequestError::Url {
        url: url.clone(),
        msg: e.to_string(),
    })?;
    let url_parts = UrlParts {
        scheme: parsed.scheme().to_string(),
        host: parsed.host_str().unwrap_or("").to_string(),
        port: parsed.port().map(|p| p.to_string()).unwrap_or_default(),
        path: parsed.path().trim_start_matches('/').to_string(),
        query: parsed.query().unwrap_or("").to_string(),
        fragment: parsed.fragment().unwrap_or("").to_string(),
    };

    let req_data = RequestData {
        method: method.to_string(),
        url,
        url_parts,
        headers_raw: req_headers_raw,
        headers_json: req_headers_json,
        body_json: req_body.clone(),
        idx,
        md: md.clone(),
    };

    // Cache lookup
    let cache_config = cache_stores.and_then(|_| config.resolve_cache(&req_data.method, &req_data.url, &md));
    let cache_key = cache_config.as_ref().map(|cc| {
        cache::compute_cache_key(cc, &req_data.method, &req_data.url, &req_body, &merged_headers)
    });

    if let (Some(cc), Some(key)) = (&cache_config, &cache_key) {
        let store = cache_stores.unwrap().get(&cc.at);
        let store_lock = store.lock().unwrap();
        if let Some(cached) = store_lock.get(key) {
            let mut resp_headers_raw = String::new();
            for (k, v) in &cached.headers {
                resp_headers_raw.push_str(&format!("{k}: {}\r\n", v.as_str().unwrap_or("")));
            }
            let resp_data = ResponseData {
                status_line: format!("HTTP/1.1 {}", cached.status),
                status_parts: StatusParts {
                    code: cached.status.to_string(),
                    text: String::new(),
                    version: "HTTP/1.1".to_string(),
                },
                headers_raw: resp_headers_raw,
                headers_json: cached.headers,
                body_bytes: cached.body,
            };
            let mut buf = OutputBuffer { stdout: Vec::new(), stderr: Vec::new(), files: Vec::new() };
            for (dest, fmt) in &outputs {
                match dest {
                    Dest::Stdout | Dest::StdoutStream => buf.stdout.extend_from_slice(&render_color(fmt, &resp_data, &req_data, color_stdout)),
                    Dest::Stderr | Dest::StderrStream => buf.stderr.extend_from_slice(&render_color(fmt, &resp_data, &req_data, color_stderr)),
                    Dest::FilePath(template) | Dest::FileStream(template) => {
                        let data = render(fmt, &resp_data, &req_data);
                        let path = expand_path(template, &resp_data, &req_data);
                        buf.files.push((path, data.to_vec()));
                    }
                }
            }
            return Ok(buf);
        }
    }

    let resp = req.send().await.map_err(|e| RequestError::Network {
        msg: e.to_string(),
    })?;

    let version = format!("{:?}", resp.version());
    let status = resp.status();
    let status_line = format!("{version} {status}");
    let status_parts = StatusParts {
        code: status.as_u16().to_string(),
        text: status.canonical_reason().unwrap_or("").to_string(),
        version: version.clone(),
    };

    let mut resp_headers_raw = String::new();
    let mut resp_headers_json = Map::new();
    for (name, value) in resp.headers() {
        let v_str = value.to_str().unwrap_or("").to_string();
        resp_headers_raw.push_str(&format!("{name}: {v_str}\r\n"));
        resp_headers_json.insert(name.to_string(), Value::String(v_str));
    }

    // Check if any output needs streaming
    let has_stream = outputs.iter().any(|(d, _)| {
        matches!(d, Dest::FileStream(_) | Dest::StdoutStream | Dest::StderrStream)
    });

    let body_bytes = if has_stream {
        let mut stream = resp.bytes_stream();
        let mut stream_files: Vec<fs::File> = Vec::new();

        // Build a temporary resp_data with empty body for template expansion
        let tmp_resp = ResponseData {
            status_line: status_line.clone(),
            status_parts: StatusParts {
                code: status_parts.code.clone(),
                text: status_parts.text.clone(),
                version: status_parts.version.clone(),
            },
            headers_raw: resp_headers_raw.clone(),
            headers_json: resp_headers_json.clone(),
            body_bytes: Vec::new(),
        };

        // Open stream file destinations
        for (dest, _fmt) in &outputs {
            if let Dest::FileStream(template) = dest {
                let path = expand_path(template, &tmp_resp, &req_data);
                if let Some(parent) = std::path::Path::new(&path).parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                stream_files.push(fs::File::create(&path).unwrap());
            }
        }

        let stream_stdout = outputs.iter().any(|(d, _)| matches!(d, Dest::StdoutStream));
        let stream_stderr = outputs.iter().any(|(d, _)| matches!(d, Dest::StderrStream));

        // Only buffer body if a non-streaming destination or cache needs it
        let needs_body_buffer = cache_config.is_some() || outputs.iter().any(|(d, _)| {
            matches!(d, Dest::Stdout | Dest::Stderr | Dest::FilePath(_))
        });

        let mut buffered_bytes = Vec::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| RequestError::Network {
                msg: format!("error reading response: {e}"),
            })?;
            for f in &mut stream_files {
                f.write_all(&chunk).unwrap();
            }
            if stream_stdout {
                io::stdout().write_all(&chunk).unwrap();
            }
            if stream_stderr {
                io::stderr().write_all(&chunk).unwrap();
            }
            if needs_body_buffer {
                buffered_bytes.extend_from_slice(&chunk);
            }
        }

        buffered_bytes
    } else {
        resp.bytes().await.map_err(|e| RequestError::Network {
            msg: format!("error reading response: {e}"),
        })?.to_vec()
    };

    // Store in cache if configured
    if let (Some(cc), Some(key)) = (&cache_config, cache_key) {
        let cached = cache::CachedResponse {
            status: status.as_u16(),
            headers: resp_headers_json.clone(),
            body: body_bytes.clone(),
        };
        let store = cache_stores.unwrap().get(&cc.at);
        let store_lock = store.lock().unwrap();
        store_lock.put(&key, &req_data.url, &cached, cc.ttl);
    }

    let resp_data = ResponseData {
        status_line,
        status_parts,
        headers_raw: resp_headers_raw,
        headers_json: resp_headers_json,
        body_bytes,
    };

    let mut buf = OutputBuffer {
        stdout: Vec::new(),
        stderr: Vec::new(),
        files: Vec::new(),
    };

    for (dest, fmt) in &outputs {
        match dest {
            Dest::FileStream(_) | Dest::StdoutStream | Dest::StderrStream => {}
            Dest::Stdout => {
                let data = render_color(fmt, &resp_data, &req_data, color_stdout);
                buf.stdout.extend_from_slice(&data);
            }
            Dest::Stderr => {
                let data = render_color(fmt, &resp_data, &req_data, color_stderr);
                buf.stderr.extend_from_slice(&data);
            }
            Dest::FilePath(template) => {
                let data = render(fmt, &resp_data, &req_data);
                let path = expand_path(template, &resp_data, &req_data);
                buf.files.push((path, data.to_vec()));
            }
        }
    }

    Ok(buf)
}

fn flush_output_locked(
    buf: OutputBuffer,
    stdout_lock: &Mutex<()>,
    stderr_lock: &Mutex<()>,
    stderr_suppressed: Option<&AtomicUsize>,
) -> bool {
    let stdout_ends_newline = buf.stdout.last() == Some(&b'\n');
    if !buf.stdout.is_empty() {
        let _lock = stdout_lock.lock().unwrap();
        io::stdout().write_all(&buf.stdout).unwrap();
    }
    if !buf.stderr.is_empty() {
        if let Some(counter) = stderr_suppressed {
            counter.fetch_add(1, Ordering::Relaxed);
        } else {
            let _lock = stderr_lock.lock().unwrap();
            io::stderr().write_all(&buf.stderr).unwrap();
        }
    }
    for (path, data) in &buf.files {
        if let Some(parent) = std::path::Path::new(path).parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(path).unwrap();
        f.write_all(data).unwrap();
    }
    stdout_ends_newline || buf.stdout.is_empty()
}

fn pre_parse_for_matching(line: &str, apis: &std::collections::HashMap<String, String>) -> Result<(String, String, Option<Value>), RequestError> {
    let json = parse_input(line)?;
    let obj = json.as_object().ok_or_else(|| RequestError::Structure {
        msg: "request must be a JSON/YAML object".to_string(),
    })?;
    let mut method = None;
    let mut url = None;
    let mut md = None;
    for (key, val) in obj {
        if let Some(m) = resolve_method(key) {
            method = Some(m.to_string());
            let u = val.as_str().ok_or_else(|| RequestError::Structure {
                msg: format!("URL for '{key}' must be a string"),
            })?;
            url = Some(config::expand_api_url(u, apis));
        } else if key.to_lowercase() == "md" {
            md = Some(val.clone());
        }
    }
    Ok((method.unwrap_or_default(), url.unwrap_or_default(), md))
}

/// Streaming stdin reader that yields one request at a time.
/// Supports JSONL (one JSON object per line) and YAML (documents separated by `---`).
/// Format is auto-detected from the first non-empty line.
struct StdinReader<R: BufRead> {
    lines: R,
    is_yaml: Option<bool>,
    buf: String,
    done: bool,
}

impl<R: BufRead> StdinReader<R> {
    fn new(lines: R) -> Self {
        StdinReader {
            lines,
            is_yaml: None,
            buf: String::new(),
            done: false,
        }
    }

    fn next(&mut self) -> Option<String> {
        if self.done {
            return None;
        }
        loop {
            let mut line = String::new();
            match self.lines.read_line(&mut line) {
                Ok(0) => {
                    // EOF
                    self.done = true;
                    return self.flush_yaml_buf();
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() && self.is_yaml.is_none() {
                        continue; // skip leading blank lines
                    }

                    // Auto-detect format from first non-empty line.
                    // Lines starting with { are JSONL/yttp; everything else is YAML.
                    if self.is_yaml.is_none() {
                        self.is_yaml = Some(!trimmed.starts_with('{'));
                    }

                    if self.is_yaml == Some(true) {
                        // YAML: accumulate until `---` separator
                        if trimmed == "---" {
                            if let Some(doc) = self.flush_yaml_buf() {
                                return Some(doc);
                            }
                            // empty doc between separators, keep reading
                        } else {
                            self.buf.push_str(&line);
                        }
                    } else {
                        // JSONL: yield each non-empty line
                        if !trimmed.is_empty() {
                            return Some(trimmed.to_string());
                        }
                    }
                }
                Err(_) => {
                    self.done = true;
                    return self.flush_yaml_buf();
                }
            }
        }
    }

    fn flush_yaml_buf(&mut self) -> Option<String> {
        let trimmed = self.buf.trim();
        if trimmed.is_empty() {
            return None;
        }
        let val: Value = serde_yml::from_str(trimmed).unwrap();
        let json = serde_json::to_string(&val).unwrap();
        self.buf.clear();
        Some(json)
    }
}

#[tokio::main]
async fn main() {
    let args: Args = argh::from_env();

    // Detect yaml mode from binary name (check filename only, not full path)
    let yaml_mode = std::env::args()
        .next()
        .and_then(|s| std::path::Path::new(&s).file_name().map(|f| f.to_string_lossy().contains("yurl")))
        .unwrap_or(false);

    if args.version {
        let name = if yaml_mode { "yurl" } else { "jurl" };
        println!("{name} {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.reference {
        eprint!("{}", interactive::reference_card());
        return;
    }
    let client = Client::new();

    let config = match &args.config {
        Some(cfg_str) => {
            match parse_input(cfg_str) {
                Ok(json) => Config::parse(&json),
                Err(e) => {
                    eprintln!("{}", e.display_colored());
                    std::process::exit(1);
                }
            }
        }
        None => Config::empty(),
    };

    let config = Arc::new(ArcSwap::from_pointee(config));
    let cache_stores = Arc::new(cache::CacheStores::new());
    let init_cfg = config.load_full();
    let concurrent = init_cfg.global_concurrency > 1;
    let global_sem = Arc::new(Semaphore::new(init_cfg.global_concurrency));
    let stdout_lock = Arc::new(Mutex::new(()));
    let stderr_lock = Arc::new(Mutex::new(()));

    // Progress bar setup
    let show_progress = !matches!(init_cfg.progress, Progress::Off);
    let multi = if show_progress {
        Some(Arc::new(MultiProgress::new()))
    } else {
        None
    };

    let progress_bar = multi.as_ref().map(|m| {
        let pb = match init_cfg.progress {
            Progress::Known(n) => {
                let pb = m.add(ProgressBar::new(n));
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template("[{bar:40}] {pos}/{len}")
                        .unwrap()
                        .progress_chars("=>-"),
                );
                pb
            }
            _ => {
                let pb = m.add(ProgressBar::new_spinner());
                pb.set_style(
                    ProgressStyle::default_spinner()
                        .template("{spinner} {pos} done")
                        .unwrap(),
                );
                pb
            }
        };
        Arc::new(pb)
    });

    let stderr_suppressed: Option<Arc<AtomicUsize>> = if show_progress {
        Some(Arc::new(AtomicUsize::new(0)))
    } else {
        None
    };

    let warn_bar = multi.as_ref().map(|m| {
        let wb = m.add(ProgressBar::new_spinner());
        wb.set_style(ProgressStyle::default_spinner().template("{msg}").unwrap());
        Arc::new(wb)
    });

    // Create per-rule semaphores for rules that have concurrency limits
    let rule_sems: Arc<Vec<Option<Arc<Semaphore>>>> = Arc::new(
        init_cfg
            .rules
            .iter()
            .map(|r| r.concurrency.map(|c| Arc::new(Semaphore::new(c))))
            .collect(),
    );
    drop(init_cfg);

    let force_color = std::env::var("FORCE_COLOR").is_ok() || std::env::var("CLICOLOR_FORCE").is_ok();
    let color_stdout = force_color || io::stdout().is_terminal();
    let color_stderr = force_color || io::stderr().is_terminal();

    let idx_counter = Arc::new(AtomicUsize::new(0));

    // Spawn a request task and return its JoinHandle
    let spawn_request = |line: String,
                         client: Client,
                         config: Arc<Config>,
                         global_sem: Arc<Semaphore>,
                         rule_sems: Arc<Vec<Option<Arc<Semaphore>>>>,
                         stdout_lock: Arc<Mutex<()>>,
                         stderr_lock: Arc<Mutex<()>>,
                         progress_bar: Option<Arc<ProgressBar>>,
                         stderr_suppressed: Option<Arc<AtomicUsize>>,
                         warn_bar: Option<Arc<ProgressBar>>,
                         cache_stores: Arc<cache::CacheStores>,
                         idx: usize,
                         concurrent: bool,
                         yaml_mode: bool| {
        let pre_parsed = match pre_parse_for_matching(&line, &config.apis) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{}", e.display_colored());
                return tokio::spawn(async {});
            }
        };
        let (method_str, url_str, md) = pre_parsed;
        let matching_rules = config.matching_concurrency_rules(&method_str, &url_str, &md);

        let needed_sems: Vec<Arc<Semaphore>> = matching_rules
            .iter()
            .filter_map(|&i| rule_sems[i].as_ref().map(Arc::clone))
            .collect();

        tokio::spawn(async move {
            let _global_permit = global_sem.acquire().await.unwrap();
            let mut _rule_permits = Vec::new();
            for s in &needed_sems {
                _rule_permits.push(s.acquire().await.unwrap());
            }
            let buf = match execute(&line, &client, idx, &config, concurrent, yaml_mode, Some(&cache_stores), color_stdout, color_stderr).await {
                Ok(buf) => buf,
                Err(e) => {
                    eprintln!("{}", e.display_colored());
                    return;
                }
            };
            let ends_newline = flush_output_locked(
                buf,
                &stdout_lock,
                &stderr_lock,
                stderr_suppressed.as_deref(),
            );

            // In interactive mode, ensure output ends with newline so
            // the spinner/prompt don't overwrite the last line of output.
            if io::stdout().is_terminal() {
                if !ends_newline {
                    let _lock = stdout_lock.lock().unwrap();
                    let _ = io::stdout().write_all(b"\n");
                }
                let _ = io::stdout().flush();
            }

            if let Some(pb) = &progress_bar {
                pb.inc(1);
            }
            if let Some(counter) = &stderr_suppressed {
                let count = counter.load(Ordering::Relaxed);
                if count > 0 {
                    if let Some(wb) = &warn_bar {
                        wb.set_message(format!(
                            "⚠ {count} request(s) had stderr output, suppressed by progress"
                        ));
                    }
                }
            }
        })
    };

    if io::stdin().is_terminal() || args.step {
        // Interactive mode — run REPL on a blocking thread, send lines via channel
        // In --step mode, pre-load stdin requests for .next/.go commands
        let step_queue = if args.step && !io::stdin().is_terminal() {
            let stdin = io::stdin().lock();
            let mut reader = StdinReader::new(stdin);
            let mut queue = std::collections::VecDeque::new();
            while let Some(line) = reader.next() {
                queue.push_back(line);
            }
            Some(queue)
        } else {
            None
        };

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, std::sync::mpsc::SyncSender<()>)>();

        let repl_config = Arc::clone(&config);
        let repl_handle = std::thread::spawn(move || {
            interactive::run(|line| {
                let (done_tx, done_rx) = std::sync::mpsc::sync_channel(0);
                tx.send((line, done_tx)).ok();
                done_rx.recv().ok();
            }, &repl_config, step_queue);
        });

        // Process lines as they arrive from the REPL
        while let Some((line, done_tx)) = rx.recv().await {
            let idx = idx_counter.fetch_add(1, Ordering::Relaxed);

            // Show spinner while request is in flight
            let spinner = ProgressBar::new_spinner();
            spinner.set_style(
                ProgressStyle::default_spinner()
                    .template("  {spinner} \x1b[2mrequest {msg}...\x1b[0m")
                    .unwrap(),
            );
            spinner.set_message(format!("{idx}"));
            eprint!("\x1b[?25l"); // hide cursor
            spinner.enable_steady_tick(std::time::Duration::from_millis(80));

            let current_config = config.load_full();
            let handle = spawn_request(
                line,
                client.clone(),
                current_config,
                Arc::clone(&global_sem),
                Arc::clone(&rule_sems),
                Arc::clone(&stdout_lock),
                Arc::clone(&stderr_lock),
                progress_bar.clone(),
                stderr_suppressed.clone(),
                warn_bar.clone(),
                Arc::clone(&cache_stores),
                idx,
                concurrent,
                yaml_mode,
            );
            if let Err(e) = handle.await {
                eprintln!("  request failed: {e}");
            }
            spinner.finish_and_clear();
            eprint!("\x1b[?25h"); // restore cursor
            // Signal REPL to show next prompt
            done_tx.send(()).ok();
        }

        repl_handle.join().ok();
    } else {
        // Pipe mode — streaming stdin with backpressure.
        // Bounded channel capacity: sum of per-rule concurrency slots + global concurrency.
        // This prevents head-of-line blocking when rules have different concurrency limits.
        let pipe_cfg = config.load_full();
        let rule_slots: usize = pipe_cfg.rules.iter().filter_map(|r| r.concurrency).sum();
        let channel_capacity = rule_slots.max(pipe_cfg.global_concurrency).max(1);
        drop(pipe_cfg);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(usize, String)>(channel_capacity);

        let idx_counter_reader = Arc::clone(&idx_counter);
        let reader_handle = std::thread::spawn(move || {
            let stdin = io::stdin().lock();
            let mut reader = StdinReader::new(stdin);
            while let Some(line) = reader.next() {
                let idx = idx_counter_reader.fetch_add(1, Ordering::Relaxed);
                if tx.blocking_send((idx, line)).is_err() {
                    break; // receiver dropped
                }
            }
        });

        let mut handles = Vec::new();

        while let Some((idx, line)) = rx.recv().await {
            let handle = spawn_request(
                line,
                client.clone(),
                config.load_full(),
                Arc::clone(&global_sem),
                Arc::clone(&rule_sems),
                Arc::clone(&stdout_lock),
                Arc::clone(&stderr_lock),
                progress_bar.clone(),
                stderr_suppressed.clone(),
                warn_bar.clone(),
                Arc::clone(&cache_stores),
                idx,
                concurrent,
                yaml_mode,
            );
            handles.push(handle);
        }

        // Wait for all in-flight tasks to complete
        for handle in handles {
            if let Err(e) = handle.await {
                eprintln!("request failed: {e}");
            }
        }
        reader_handle.join().ok();
    }

    // Finish progress bars
    if let Some(pb) = &progress_bar {
        pb.finish();
    }
    if let Some(wb) = &warn_bar {
        let count = stderr_suppressed
            .as_ref()
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0);
        if count == 0 {
            wb.finish_and_clear();
        } else {
            wb.finish();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: expand with merged flags (what old expand_request did).
    fn expand_merged(line: &str, config: &Config) -> Result<String, RequestError> {
        expand_with_flags(line, config, &ExpandFlags { merged: true, ..Default::default() })
    }

    /// Helper: expand with default flags (unmerged horizontal YAML).
    fn expand_default(line: &str, config: &Config) -> Result<String, RequestError> {
        expand_with_flags(line, config, &ExpandFlags::default())
    }

    #[test]
    fn expand_request_api_alias_and_headers() {
        let config_json = parse_input(r#"{"api": "localhost:3000", "h": {"X-Debug": "1"}}"#).unwrap();
        let config = Config::parse(&config_json);
        let result = expand_merged(r#"{"g": "api!/toys", "h": {"a!": "bearer!tok"}}"#, &config).unwrap();
        // Output is YAML flow style, round-trips through yttp::parse
        let parsed = yttp::parse(&result).unwrap();
        let obj = parsed.as_object().unwrap();

        assert_eq!(obj.get("get").unwrap(), "http://localhost:3000/toys");

        let h = obj.get("h").unwrap().as_object().unwrap();
        assert_eq!(h.get("X-Debug").unwrap(), "1");
        assert_eq!(h.get("Authorization").unwrap(), "Bearer tok");
    }

    #[test]
    fn expand_request_preserves_body_and_outputs() {
        let config = Config::empty();
        let result = expand_merged(
            r#"{"p": "https://example.com", "b": {"name": "test"}, "1": "j(s b)"}"#,
            &config,
        ).unwrap();
        let parsed = yttp::parse(&result).unwrap();
        let obj = parsed.as_object().unwrap();

        assert_eq!(obj.get("post").unwrap(), "https://example.com");
        assert_eq!(obj.get("b").unwrap().as_object().unwrap().get("name").unwrap(), "test");
        assert_eq!(obj.get("1").unwrap(), "j(s b)");
    }

    #[test]
    fn expand_request_rule_headers() {
        let config_json = parse_input(
            r#"{"rules": [{"match": {"u": "**example**"}, "h": {"X-From-Rule": "yes"}}]}"#,
        ).unwrap();
        let config = Config::parse(&config_json);
        let result = expand_merged(r#"{"g": "https://example.com/api"}"#, &config).unwrap();
        let parsed = yttp::parse(&result).unwrap();
        let h = parsed.get("h").unwrap().as_object().unwrap();
        assert_eq!(h.get("X-From-Rule").unwrap(), "yes");
    }

    #[test]
    fn config_summary_empty() {
        let config = Config::empty();
        assert_eq!(config.summary(), "(empty)");
    }

    #[test]
    fn config_summary_with_fields() {
        let config_json = parse_input(
            r#"{"api": {"main": "localhost:3000", "staging": "staging.example.com"}, "h": {"X-Test": "1"}, "rules": [{"h": {"X-R": "1"}}], "concurrency": 5}"#,
        ).unwrap();
        let config = Config::parse(&config_json);
        let summary = config.summary();
        assert!(summary.contains("api:"));
        assert!(summary.contains("h: 1 header"));
        assert!(summary.contains("rules: 1"));
        assert!(summary.contains("concurrency: 5"));
    }

    #[test]
    fn arcswap_config_replacement() {
        let config_json = parse_input(r#"{"api": "localhost:3000", "h": {"X-Old": "1"}}"#).unwrap();
        let config = Arc::new(ArcSwap::from_pointee(Config::parse(&config_json)));

        // Initial state
        let result = expand_merged(r#"{"g": "api!/test"}"#, &config.load()).unwrap();
        assert!(result.contains("localhost:3000"));
        assert!(result.contains("X-Old"));

        // Replace config
        let new_config_json = parse_input(r#"{"api": "example.com:8080", "h": {"X-New": "2"}}"#).unwrap();
        config.store(Arc::new(Config::parse(&new_config_json)));

        // Verify new config is used
        let result = expand_merged(r#"{"g": "api!/test"}"#, &config.load()).unwrap();
        assert!(result.contains("example.com:8080"));
        assert!(result.contains("X-New"));
        assert!(!result.contains("X-Old"));
    }

    #[test]
    fn expand_request_yaml_flow_format() {
        let config = Config::empty();
        let result = expand_merged(r#"{"g": "https://example.com"}"#, &config).unwrap();
        // Should be YAML flow style, not JSON — no quoted keys
        assert!(result.starts_with('{'));
        assert!(result.contains("get:"));
        assert!(!result.contains("\"get\""));
    }

    #[test]
    fn to_yaml_flow_round_trip() {
        let cases = vec![
            r#"{"get": "http://localhost:3000/toys"}"#,
            r#"{"post": "https://example.com", "h": {"Content-Type": "application/json"}, "b": {"name": "test"}}"#,
            r#"{"get": "https://example.com/search?q=hello&limit=10"}"#,
        ];
        for json_str in cases {
            let val: Value = serde_json::from_str(json_str).unwrap();
            let yaml = to_yaml_flow(&val);
            let reparsed = yttp::parse(&yaml).unwrap();
            assert_eq!(val, reparsed, "Round-trip failed for: {yaml}");
        }
    }

    #[test]
    fn to_yaml_flow_quoting() {
        // Reserved words must be quoted
        assert_eq!(yaml_flow_scalar("true"), "\"true\"");
        assert_eq!(yaml_flow_scalar("null"), "\"null\"");
        // Numbers must be quoted
        assert_eq!(yaml_flow_scalar("42"), "\"42\"");
        // Empty string must be quoted
        assert_eq!(yaml_flow_scalar(""), "\"\"");
        // Plain strings stay unquoted
        assert_eq!(yaml_flow_scalar("hello"), "hello");
        assert_eq!(yaml_flow_scalar("http://example.com"), "http://example.com");
        assert_eq!(yaml_flow_scalar("application/json"), "application/json");
        // Flow indicators require quoting
        assert_eq!(yaml_flow_scalar("a,b"), "\"a,b\"");
        assert_eq!(yaml_flow_scalar("{x}"), "\"{x}\"");
    }

    #[test]
    fn yaml_flow_key_unquoted_numbers() {
        // Numeric keys should not be quoted
        assert_eq!(yaml_flow_key("1"), "1");
        assert_eq!(yaml_flow_key("2"), "2");
        // Non-numeric keys follow normal rules
        assert_eq!(yaml_flow_key("get"), "get");
        assert_eq!(yaml_flow_key("true"), "\"true\"");
        assert_eq!(yaml_flow_key(""), "\"\"");
    }

    #[test]
    fn expand_request_numeric_key_unquoted() {
        let config = Config::empty();
        let result = expand_merged(
            r#"{"g": "https://example.com", "1": "j(s b)"}"#,
            &config,
        ).unwrap();
        // Output key 1 should not be quoted
        assert!(result.contains("1: "), "numeric key should be unquoted: {result}");
        assert!(!result.contains("\"1\""), "numeric key should not be quoted: {result}");
        // Should still round-trip
        let parsed = yttp::parse(&result).unwrap();
        assert_eq!(parsed.get("1").unwrap(), "j(s b)");
    }

    #[test]
    fn expand_request_query_params() {
        let config = Config::empty();
        let result = expand_merged(
            r#"{"g": "https://example.com/search", "q": {"term": "foo", "limit": 10}}"#,
            &config,
        ).unwrap();
        let parsed = yttp::parse(&result).unwrap();
        let url = parsed.get("get").unwrap().as_str().unwrap();
        assert!(url.contains("term=foo"));
        assert!(url.contains("limit=10"));
        assert!(url.contains("?"));
        assert!(parsed.get("q").is_none());
    }

    // --- ExpandFlags ---

    #[test]
    fn flags_parse_valid() {
        let f = ExpandFlags::parse("mv").unwrap();
        assert!(f.merged);
        assert!(f.vertical);
        assert!(!f.json);
    }

    #[test]
    fn flags_parse_order_independent() {
        assert_eq!(ExpandFlags::parse("mv").unwrap(), ExpandFlags::parse("vm").unwrap());
    }

    #[test]
    fn flags_parse_invalid() {
        assert!(ExpandFlags::parse("z").is_err());
    }

    #[test]
    fn flags_mutually_exclusive() {
        assert!(ExpandFlags::parse("jc").is_err());
    }

    #[test]
    fn flags_editable() {
        assert!(ExpandFlags::default().is_editable());
        assert!(ExpandFlags::parse("m").unwrap().is_editable());
        assert!(!ExpandFlags::parse("v").unwrap().is_editable());
        assert!(!ExpandFlags::parse("c").unwrap().is_editable());
    }

    // --- expand_with_flags ---

    #[test]
    fn expand_json_horizontal() {
        let config = Config::empty();
        let flags = ExpandFlags { json: true, ..Default::default() };
        let result = expand_with_flags(r#"{"g": "https://example.com"}"#, &config, &flags).unwrap();
        assert!(result.contains("\"get\""), "should be JSON: {result}");
    }

    #[test]
    fn expand_json_vertical() {
        let config = Config::empty();
        let flags = ExpandFlags { json: true, vertical: true, ..Default::default() };
        let result = expand_with_flags(r#"{"g": "https://example.com"}"#, &config, &flags).unwrap();
        assert!(result.contains('\n'), "should be multiline: {result}");
    }

    #[test]
    fn expand_curl_horizontal() {
        let config = Config::empty();
        let flags = ExpandFlags { curl: true, ..Default::default() };
        let result = expand_with_flags(r#"{"g": "https://example.com"}"#, &config, &flags).unwrap();
        assert!(result.starts_with("curl"), "should be curl: {result}");
        assert!(!result.contains('\\'), "horizontal curl should not have backslash: {result}");
    }

    #[test]
    fn expand_curl_vertical() {
        let config = Config::empty();
        let flags = ExpandFlags { curl: true, vertical: true, ..Default::default() };
        let result = expand_with_flags(
            r#"{"g": "https://example.com", "h": {"Accept": "j!"}}"#, &config, &flags
        ).unwrap();
        assert!(result.contains('\\'), "vertical curl should have backslash: {result}");
    }

    #[test]
    fn expand_base_vs_merged() {
        let config_json = parse_input(r#"{"h": {"X-Config": "yes"}}"#).unwrap();
        let config = Config::parse(&config_json);
        // Base: no config headers
        let base = expand_with_flags(r#"{"g": "https://example.com"}"#, &config, &ExpandFlags::default()).unwrap();
        assert!(!base.contains("X-Config"), "base should not include config headers: {base}");
        // Merged: config headers included
        let merged = expand_merged(r#"{"g": "https://example.com"}"#, &config).unwrap();
        assert!(merged.contains("X-Config"), "merged should include config headers: {merged}");
    }

    #[test]
    fn expand_short_headers() {
        let config_json = parse_input(r#"{"h": {"Authorization": "Bearer tok"}}"#).unwrap();
        let config = Config::parse(&config_json);
        let flags = ExpandFlags { merged: true, short: true, ..Default::default() };
        let result = expand_with_flags(r#"{"g": "https://example.com"}"#, &config, &flags).unwrap();
        assert!(result.contains("a!"), "should collapse Authorization: {result}");
        assert!(result.contains("bearer!"), "should collapse Bearer: {result}");
    }

    #[test]
    fn expand_request_parse_error() {
        let config = Config::empty();
        let result = expand_merged("{broken", &config);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            RequestError::Parse { .. } => {}
            _ => panic!("expected Parse error, got: {err}"),
        }
    }

    #[test]
    fn expand_request_no_method_ok() {
        // No method defaults to GET with empty URL
        let config = Config::empty();
        let result = expand_merged(r#"{"h": {"Accept": "j!"}}"#, &config);
        assert!(result.is_ok());
    }

    #[test]
    fn expand_request_url_not_string_error() {
        let config = Config::empty();
        let result = expand_merged(r#"{"g": {"nested": true}}"#, &config);
        assert!(result.is_err());
        match result.unwrap_err() {
            RequestError::Structure { msg } => {
                assert!(msg.contains("must be a string"), "msg: {msg}");
            }
            other => panic!("expected Structure error, got: {other}"),
        }
    }
}
