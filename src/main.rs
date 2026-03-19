mod atom;
mod cache;
mod config;
mod format_json;
mod format_yaml;
mod interactive;
mod template;

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


fn parse_input(s: &str) -> Value {
    yttp::parse(s).unwrap()
}

async fn execute(line: &str, client: &Client, idx: usize, config: &Config, concurrent: bool, yaml_mode: bool, cache_stores: Option<&cache::CacheStores>, color_stdout: bool, color_stderr: bool) -> OutputBuffer {
    let json = parse_input(line);
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
                _ => eprintln!("unknown key: {key}"),
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
                outputs.push((parse_dest(key), parse_format(fmt_str)));
            }
        }
    }

    // Auto-promote file destinations to streaming when safe
    for (dest, fmt) in &mut outputs {
        maybe_auto_stream(dest, fmt, concurrent);
    }

    let method = method.expect("no HTTP method found");
    let url = url.expect("no URL found");

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
            return buf;
        }
    }

    let resp = req.send().await.unwrap();

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
            let chunk = chunk.unwrap();
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
        resp.bytes().await.unwrap().to_vec()
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

    buf
}

fn flush_output_locked(
    buf: OutputBuffer,
    stdout_lock: &Mutex<()>,
    stderr_lock: &Mutex<()>,
    stderr_suppressed: Option<&AtomicUsize>,
) {
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
}

fn pre_parse_for_matching(line: &str) -> (String, String, Option<Value>) {
    let json = parse_input(line);
    let obj = json.as_object().unwrap();
    let mut method = None;
    let mut url = None;
    let mut md = None;
    for (key, val) in obj {
        if let Some(m) = resolve_method(key) {
            method = Some(m.to_string());
            url = Some(val.as_str().unwrap().to_string());
        } else if key.to_lowercase() == "md" {
            md = Some(val.clone());
        }
    }
    (method.unwrap_or_default(), url.unwrap_or_default(), md)
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

                    // Auto-detect format from first non-empty line
                    if self.is_yaml.is_none() {
                        self.is_yaml = Some(serde_json::from_str::<Value>(trimmed).is_err());
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
    let client = Client::new();

    let config = match &args.config {
        Some(cfg_str) => {
            let json = parse_input(cfg_str);
            Config::parse(&json)
        }
        None => Config::empty(),
    };

    let config = Arc::new(config);
    let cache_stores = Arc::new(cache::CacheStores::new());
    let concurrent = config.global_concurrency > 1;
    let global_sem = Arc::new(Semaphore::new(config.global_concurrency));
    let stdout_lock = Arc::new(Mutex::new(()));
    let stderr_lock = Arc::new(Mutex::new(()));

    // Progress bar setup
    let show_progress = !matches!(config.progress, Progress::Off);
    let multi = if show_progress {
        Some(Arc::new(MultiProgress::new()))
    } else {
        None
    };

    let progress_bar = multi.as_ref().map(|m| {
        let pb = match config.progress {
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
        config
            .rules
            .iter()
            .map(|r| r.concurrency.map(|c| Arc::new(Semaphore::new(c))))
            .collect(),
    );

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
        let (method_str, url_str, md) = pre_parse_for_matching(&line);
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
            let buf = execute(&line, &client, idx, &config, concurrent, yaml_mode, Some(&cache_stores), color_stdout, color_stderr).await;
            flush_output_locked(
                buf,
                &stdout_lock,
                &stderr_lock,
                stderr_suppressed.as_deref(),
            );

            // Flush stdout in interactive mode
            if io::stdout().is_terminal() {
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

        let repl_handle = std::thread::spawn(move || {
            interactive::run(|line| {
                let (done_tx, done_rx) = std::sync::mpsc::sync_channel(0);
                tx.send((line, done_tx)).ok();
                done_rx.recv().ok();
            }, step_queue);
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
            spinner.enable_steady_tick(std::time::Duration::from_millis(80));

            let handle = spawn_request(
                line,
                client.clone(),
                Arc::clone(&config),
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
            handle.await.unwrap();
            spinner.finish_and_clear();
            // Signal REPL to show next prompt
            done_tx.send(()).ok();
        }

        repl_handle.join().ok();
    } else {
        // Pipe mode — streaming stdin with backpressure.
        // Bounded channel capacity: sum of per-rule concurrency slots + global concurrency.
        // This prevents head-of-line blocking when rules have different concurrency limits.
        let rule_slots: usize = config.rules.iter().filter_map(|r| r.concurrency).sum();
        let channel_capacity = rule_slots.max(config.global_concurrency).max(1);
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
                Arc::clone(&config),
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
            handle.await.unwrap();
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
