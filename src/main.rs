mod atom;
mod config;
mod shortcut;
mod template;

use argh::FromArgs;
use atom::{Atom, Format, RequestData, ResponseData, StatusParts, UrlParts, parse_format, render};
use config::Config;
use template::expand_path;
use reqwest::blocking::Client;
use serde_json::{Map, Value};
use std::fs;
use std::io::{self, BufRead, Write};
use url::Url;

/// JSON-driven HTTP client with shortcuts, flexible output routing, and rule-based middleware.
#[derive(FromArgs)]
struct Args {
    /// print version
    #[argh(switch, short = 'v')]
    version: bool,

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
    Stderr,
    FilePath(String),
}

fn parse_dest(key: &str) -> Dest {
    if key == "1" {
        Dest::Stdout
    } else if key == "2" {
        Dest::Stderr
    } else {
        Dest::FilePath(key.strip_prefix("file://").unwrap().to_string())
    }
}

fn execute(line: &str, client: &Client, idx: usize, config: &Config) {
    let json: Value = serde_json::from_str(line).unwrap();
    let obj = json.as_object().unwrap();

    let mut method = None;
    let mut url = None;
    let mut req_headers = None;
    let mut req_body = None;
    let mut md = None;
    let mut outputs: Vec<(Dest, Format)> = Vec::new();

    for (key, val) in obj {
        if let Some(m) = resolve_method(key) {
            method = Some(m);
            url = Some(val.as_str().unwrap().to_string());
        } else if config::is_output_key(key) {
            let dest = parse_dest(key);
            let fmt = parse_format(val.as_str().unwrap());
            outputs.push((dest, fmt));
        } else {
            match key.to_lowercase().as_str() {
                "h" | "headers" => req_headers = Some(val.clone()),
                "b" | "body" => req_body = Some(val.clone()),
                "md" => md = Some(val.clone()),
                "c!" | "ct!" | "a!" | "auth!" => {} // handled by resolve_headers
                _ => eprintln!("unknown key: {key}"),
            }
        }
    }

    if outputs.is_empty() {
        if config.default_outputs.is_empty() {
            outputs.push((Dest::Stdout, Format::Json(vec![Atom::B, Atom::H, Atom::S])));
        } else {
            for (key, fmt_str) in &config.default_outputs {
                outputs.push((parse_dest(key), parse_format(fmt_str)));
            }
        }
    }

    let method = method.expect("no HTTP method found");
    let url = url.expect("no URL found");

    let merged_headers = config.resolve_headers(method, &url, &md, &req_headers, obj);

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
        let v_str = v.as_str().unwrap();
        // Skip Content-Type for multipart — reqwest sets it with the boundary
        if k.eq_ignore_ascii_case("content-type") && content_type.starts_with("multipart/form-data")
        {
            continue;
        }
        req = req.header(k.as_str(), v_str);
        req_headers_raw.push_str(&format!("{k}: {v_str}\r\n"));
        req_headers_json.insert(k.clone(), Value::String(v_str.to_string()));
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
                let mut form = reqwest::blocking::multipart::Form::new();
                for (k, v) in fields {
                    match v {
                        Value::String(s) if s.starts_with("file://") => {
                            let path = &s[7..];
                            form = form
                                .file(k.clone(), path)
                                .unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
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

    let req_status_line = format!("{method} {url}");

    let parsed = Url::parse(&url).unwrap();
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
        status_line: req_status_line,
        headers_raw: req_headers_raw,
        headers_json: req_headers_json,
        body_json: req_body,
        idx,
        md,
    };

    let resp = req.send().unwrap();

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

    let body_bytes = resp.bytes().unwrap().to_vec();

    let resp_data = ResponseData {
        status_line,
        status_parts,
        headers_raw: resp_headers_raw,
        headers_json: resp_headers_json,
        body_bytes,
    };

    for (dest, fmt) in &outputs {
        let data = render(fmt, &resp_data, &req_data);
        match dest {
            Dest::Stdout => io::stdout().write_all(&data).unwrap(),
            Dest::Stderr => io::stderr().write_all(&data).unwrap(),
            Dest::FilePath(template) => {
                let path = expand_path(template, &resp_data, &req_data);
                if let Some(parent) = std::path::Path::new(&path).parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                let mut f = fs::File::create(&path).unwrap();
                f.write_all(&data).unwrap();
            }
        }
    }
}

fn main() {
    let args: Args = argh::from_env();

    if args.version {
        println!("jurl {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let client = Client::new();

    let config = match &args.config {
        Some(json_str) => {
            let json: Value = serde_json::from_str(json_str).unwrap();
            Config::parse(&json)
        }
        None => Config::empty(),
    };

    let stdin = io::stdin().lock();
    let mut idx = 0;
    for line in stdin.lines() {
        let line = line.unwrap();
        if !line.is_empty() {
            execute(&line, &client, idx, &config);
            idx += 1;
        }
    }
}
