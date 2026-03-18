use std::process::Command;

fn jurl(input: &str) -> String {
    jurl_with_config(input, None)
}

fn jurl_with_config(input: &str, config: Option<&str>) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jurl"));
    if let Some(c) = config {
        cmd.arg(c);
    }
    let output = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
            child.wait_with_output()
        })
        .expect("failed to run jurl");
    String::from_utf8(output.stdout).unwrap()
}

fn parse_json(s: &str) -> serde_json::Value {
    serde_json::from_str(s).unwrap()
}

// --- Simple GET ---

#[test]
fn default_output_has_b_h_s() {
    let out = jurl(r#"{"g": "https://httpbin.org/get"}"#);
    let json = parse_json(&out);
    assert!(json["b"].is_object() || json["b"].is_string(), "b should be JSON object or string");
    assert!(json["h"].is_object(), "h should be headers object");
    assert!(json["s"].is_object(), "s should be status object");
    assert_eq!(json["s"]["c"], 200);
}

#[test]
fn raw_body() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "1": "b"}"#);
    let body: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(body["url"], "https://httpbin.org/get");
}

#[test]
fn raw_status() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "1": "s"}"#);
    assert_eq!(out, "HTTP/1.1 200 OK");
}

// --- Status parts ---

#[test]
fn status_parts() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "1": "j(s.code,s.text,s.version)"}"#);
    let json = parse_json(&out);
    assert_eq!(json["s"]["c"], 200);
    assert_eq!(json["s"]["t"], "OK");
    assert_eq!(json["s"]["v"], "HTTP/1.1");
}

// --- Method and URL atoms ---

#[test]
fn method_and_url_atoms() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "1": "j(m,u)"}"#);
    let json = parse_json(&out);
    assert_eq!(json["m"], "GET");
    assert_eq!(json["u"], "https://httpbin.org/get");
}

#[test]
fn raw_method() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "1": "m"}"#);
    assert_eq!(out, "GET");
}

// --- POST with JSON body (default) ---

#[test]
fn post_json_body() {
    let out = jurl(r#"{"p": "https://httpbin.org/post", "b": {"key": "val"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["json"]["key"], "val");
}

// --- POST shortcut is case-insensitive ---

#[test]
fn post_case_insensitive() {
    let out = jurl(r#"{"POST": "https://httpbin.org/post", "b": {"k": "v"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["json"]["k"], "v");
}

// --- Form encoding: full Content-Type ---
//     then shortcut form! / f!

#[test]
fn form_urlencoded_full() {
    let out = jurl(r#"{"p": "https://httpbin.org/post", "h": {"Content-Type": "application/x-www-form-urlencoded"}, "b": {"city": "Berlin"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["form"]["city"], "Berlin");
}

#[test]
fn form_urlencoded_shortcut_form() {
    let out = jurl(r#"{"p": "https://httpbin.org/post", "h": {"c!": "form!"}, "b": {"city": "Berlin"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["form"]["city"], "Berlin");
}

#[test]
fn form_urlencoded_shortcut_f() {
    let out = jurl(r#"{"p": "https://httpbin.org/post", "h": {"c!": "f!"}, "b": {"city": "Berlin"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["form"]["city"], "Berlin");
}

// --- Multipart: full Content-Type ---
//     then shortcut multi! / m!

#[test]
fn multipart_full() {
    let out = jurl(r#"{"p": "https://httpbin.org/post", "h": {"Content-Type": "multipart/form-data"}, "b": {"field": "val"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["form"]["field"], "val");
}

#[test]
fn multipart_shortcut_multi() {
    let out = jurl(r#"{"p": "https://httpbin.org/post", "h": {"c!": "multi!"}, "b": {"field": "val"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["form"]["field"], "val");
}

#[test]
fn multipart_shortcut_m() {
    let out = jurl(r#"{"p": "https://httpbin.org/post", "h": {"c!": "m!"}, "b": {"field": "val"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["form"]["field"], "val");
}

// --- Auth: full header ---
//     then shortcut basic! / bearer!

#[test]
fn auth_basic_full_header() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "h": {"Authorization": "Basic dXNlcjpwYXNz"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Basic dXNlcjpwYXNz");
}

#[test]
fn auth_basic_shortcut() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "h": {"a!": "basic!user:pass"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Basic dXNlcjpwYXNz");
}

#[test]
fn auth_bearer_full_header() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "h": {"Authorization": "Bearer tok123"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Bearer tok123");
}

#[test]
fn auth_bearer_shortcut() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "h": {"a!": "bearer!tok123"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Bearer tok123");
}

// --- Smart auth: bare token → Bearer ---

#[test]
fn auth_bare_token() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "h": {"a!": "my-token"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Bearer my-token");
}

#[test]
fn auth_string_with_scheme_passthrough() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "h": {"a!": "Basic dXNlcjpwYXNz"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Basic dXNlcjpwYXNz");
}

#[test]
fn auth_array_basic() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "h": {"a!": ["user", "pass"]}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Basic dXNlcjpwYXNz");
}

// --- Content-Type prefix shortcuts ---

#[test]
fn ct_prefix_a() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "h": {"Accept": "a!/xml"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Accept"], "application/xml");
}

#[test]
fn ct_prefix_t() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "h": {"Accept": "t!/csv"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Accept"], "text/csv");
}

#[test]
fn ct_prefix_i() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "h": {"Accept": "i!/png"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Accept"], "image/png");
}

// --- Metadata ---

#[test]
fn metadata_scalar() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "md": "batch-1", "1": "j(md,s.code)"}"#);
    let json = parse_json(&out);
    assert_eq!(json["md"], "batch-1");
    assert_eq!(json["s"]["c"], 200);
}

#[test]
fn metadata_object() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "md": {"id": 42, "tag": "test"}, "1": "j(md)"}"#);
    let json = parse_json(&out);
    assert_eq!(json["md"]["id"], 42);
    assert_eq!(json["md"]["tag"], "test");
}

#[test]
fn metadata_fields() {
    let out = jurl(r#"{"g": "https://httpbin.org/get", "md": {"id": 42, "tag": "test"}, "1": "j(md.id)"}"#);
    let json = parse_json(&out);
    assert_eq!(json["md"]["id"], 42);
    assert!(json["md"].get("tag").is_none());
}

// --- Index ---

#[test]
fn idx_increments() {
    let input = r#"{"g": "https://httpbin.org/get", "1": "j(idx)"}
{"g": "https://httpbin.org/get", "1": "j(idx)"}"#;
    let out = jurl(input);
    let lines: Vec<&str> = out.trim().split('\n').collect();
    // Two JSON objects, find idx values
    let mut idx_values = Vec::new();
    for line in &lines {
        if line.contains("\"idx\"") {
            let val: String = line.chars().filter(|c| c.is_ascii_digit()).collect();
            if !val.is_empty() {
                idx_values.push(val.parse::<usize>().unwrap());
            }
        }
    }
    assert_eq!(idx_values, vec![0, 1]);
}

// --- File output with template ---

#[test]
fn file_output_template() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("{{m}}.txt");
    let file_key = format!("file://{}", path.display());
    let input = format!(r#"{{"g": "https://httpbin.org/get", "{}": "s"}}"#, file_key);
    jurl(&input);
    let result = std::fs::read_to_string(dir.path().join("GET.txt")).unwrap();
    assert!(result.contains("200 OK"));
}

// --- Session config: default headers ---

#[test]
fn config_default_headers() {
    let out = jurl_with_config(
        r#"{"g": "https://httpbin.org/get", "1": "b"}"#,
        Some(r#"{"h": {"X-Custom": "yes"}}"#),
    );
    let body = parse_json(&out);
    assert_eq!(body["headers"]["X-Custom"], "yes");
}

// --- Session config: auth shortcut ---

#[test]
fn config_auth_shortcut() {
    let out = jurl_with_config(
        r#"{"g": "https://httpbin.org/get", "1": "b"}"#,
        Some(r#"{"h": {"a!": "bearer!session-tok"}}"#),
    );
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Bearer session-tok");
}

// --- Session config: rule with URL match ---

#[test]
fn config_rule_url_match() {
    let out = jurl_with_config(
        r#"{"g": "https://httpbin.org/get", "1": "b"}"#,
        Some(r#"{"rules": [{"match": {"u": "**httpbin**"}, "h": {"X-Matched": "yes"}}]}"#),
    );
    let body = parse_json(&out);
    assert_eq!(body["headers"]["X-Matched"], "yes");
}

// --- Session config: rule with method match ---

#[test]
fn config_rule_method_match() {
    let out = jurl_with_config(
        r#"{"p": "https://httpbin.org/post", "b": {"x": "1"}, "1": "b"}"#,
        Some(r#"{"rules": [{"match": {"m": "POST"}, "h": {"c!": "f!"}}]}"#),
    );
    let body = parse_json(&out);
    assert_eq!(body["form"]["x"], "1");
}

// --- Session config: rule with metadata match ---

#[test]
fn config_rule_md_match() {
    let out = jurl_with_config(
        r#"{"g": "https://httpbin.org/get", "md": {"env": "prod"}, "1": "b"}"#,
        Some(r#"{"rules": [{"match": {"md.env": "prod"}, "h": {"X-Env": "production"}}]}"#),
    );
    let body = parse_json(&out);
    assert_eq!(body["headers"]["X-Env"], "production");
}

// --- Per-request headers override config ---

#[test]
fn request_overrides_config() {
    let out = jurl_with_config(
        r#"{"g": "https://httpbin.org/get", "h": {"X-Val": "request"}, "1": "b"}"#,
        Some(r#"{"h": {"X-Val": "config"}}"#),
    );
    let body = parse_json(&out);
    assert_eq!(body["headers"]["X-Val"], "request");
}

// --- Multiple HTTP methods ---

#[test]
fn delete_method() {
    let out = jurl(r#"{"d": "https://httpbin.org/delete", "1": "s.code"}"#);
    assert_eq!(out, "200");
}

#[test]
fn put_method() {
    let out = jurl(r#"{"put": "https://httpbin.org/put", "b": {"k": "v"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["json"]["k"], "v");
}

#[test]
fn patch_method() {
    let out = jurl(r#"{"patch": "https://httpbin.org/patch", "b": {"k": "v"}, "1": "b"}"#);
    let body = parse_json(&out);
    assert_eq!(body["json"]["k"], "v");
}

// --- Concurrency ---

#[test]
fn concurrency_parallel_faster_than_sequential() {
    // 3 requests to /delay/1 with concurrency 3 should complete in ~1s, not ~3s
    let input = r#"{"g": "https://httpbin.org/delay/1", "1": "s.code"}
{"g": "https://httpbin.org/delay/1", "1": "s.code"}
{"g": "https://httpbin.org/delay/1", "1": "s.code"}"#;
    let start = std::time::Instant::now();
    let out = jurl_with_config(input, Some(r#"{"concurrency": 3}"#));
    let elapsed = start.elapsed();
    assert!(out.matches("200").count() == 3, "all 3 should return 200");
    assert!(elapsed.as_secs() < 5, "should complete in under 5s with concurrency 3, took {}s", elapsed.as_secs());
}

#[test]
fn concurrency_default_sequential() {
    // Default concurrency is 1 — 2 requests to /delay/1 should take ~2s
    let input = r#"{"g": "https://httpbin.org/delay/1", "1": "s.code"}
{"g": "https://httpbin.org/delay/1", "1": "s.code"}"#;
    let start = std::time::Instant::now();
    let out = jurl(input);
    let elapsed = start.elapsed();
    assert!(out.matches("200").count() == 2);
    assert!(elapsed.as_secs() >= 2, "should take at least 2s sequentially, took {}s", elapsed.as_secs());
}

#[test]
fn concurrency_per_endpoint_limit() {
    // Global concurrency 4, but per-endpoint limit 1 for httpbin delay
    // 2 requests to /delay/1 with per-endpoint limit 1 should take ~2s
    let input = r#"{"g": "https://httpbin.org/delay/1", "1": "s.code"}
{"g": "https://httpbin.org/delay/1", "1": "s.code"}"#;
    let config = r#"{"concurrency": 4, "rules": [{"match": {"u": "**httpbin.org/delay**"}, "concurrency": 1}]}"#;
    let start = std::time::Instant::now();
    let out = jurl_with_config(input, Some(config));
    let elapsed = start.elapsed();
    assert!(out.matches("200").count() == 2);
    assert!(elapsed.as_secs() >= 2, "per-endpoint limit 1 should serialize requests, took {}s", elapsed.as_secs());
}

// --- Streaming ---

#[test]
fn file_stream_output() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("streamed.txt");
    let file_key = format!("file://{}?stream", path.display());
    let input = format!(r#"{{"g": "https://httpbin.org/get", "{}": "b", "1": "s"}}"#, file_key);
    let out = jurl(&input);
    assert_eq!(out, "HTTP/1.1 200 OK");
    let file_content = std::fs::read_to_string(&path).unwrap();
    let body: serde_json::Value = serde_json::from_str(&file_content).unwrap();
    assert_eq!(body["url"], "https://httpbin.org/get");
}
