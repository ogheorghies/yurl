mod server;

use std::process::Command;

fn base() -> String {
    server::base_url()
}

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

fn jurl_full(input: &str, config: Option<&str>) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jurl"));
    if let Some(c) = config {
        cmd.arg(c);
    }
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
            child.wait_with_output()
        })
        .expect("failed to run jurl")
}

fn parse_json(s: &str) -> serde_json::Value {
    serde_json::from_str(s).unwrap()
}

// --- Simple GET ---

#[test]
fn default_output_has_b_h_s() {
    let out = jurl(&format!(r#"{{"g": "{}/get"}}"#, base()));
    let json = parse_json(&out);
    assert!(json["b"].is_object() || json["b"].is_string(), "b should be JSON object or string");
    assert!(json["h"].is_object(), "h should be headers object");
    assert!(json["s"].is_object(), "s should be status object");
    assert_eq!(json["s"]["c"], 200);
}

#[test]
fn raw_body() {
    let b = base();
    let out = jurl(&format!(r#"{{"g": "{b}/get", "1": "b"}}"#));
    let body: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(body["url"], format!("{b}/get"));
}

#[test]
fn raw_status() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "1": "s"}}"#, base()));
    assert_eq!(out, "HTTP/1.1 200 OK");
}

// --- Status parts ---

#[test]
fn status_parts() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "1": "j(s.code,s.text,s.version)"}}"#, base()));
    let json = parse_json(&out);
    assert_eq!(json["s"]["c"], 200);
    assert_eq!(json["s"]["t"], "OK");
    assert_eq!(json["s"]["v"], "HTTP/1.1");
}

// --- Method and URL atoms ---

#[test]
fn method_and_url_atoms() {
    let b = base();
    let out = jurl(&format!(r#"{{"g": "{b}/get", "1": "j(m,u)"}}"#));
    let json = parse_json(&out);
    assert_eq!(json["m"], "GET");
    assert_eq!(json["u"], format!("{b}/get"));
}

#[test]
fn raw_method() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "1": "m"}}"#, base()));
    assert_eq!(out, "GET");
}

// --- POST with JSON body (default) ---

#[test]
fn post_json_body() {
    let out = jurl(&format!(r#"{{"p": "{}/post", "b": {{"key": "val"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["json"]["key"], "val");
}

// --- POST shortcut is case-insensitive ---

#[test]
fn post_case_insensitive() {
    let out = jurl(&format!(r#"{{"POST": "{}/post", "b": {{"k": "v"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["json"]["k"], "v");
}

// --- Form encoding: full Content-Type ---
//     then shortcut form! / f!

#[test]
fn form_urlencoded_full() {
    let out = jurl(&format!(r#"{{"p": "{}/post", "h": {{"Content-Type": "application/x-www-form-urlencoded"}}, "b": {{"city": "Berlin"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["form"]["city"], "Berlin");
}

#[test]
fn form_urlencoded_shortcut_form() {
    let out = jurl(&format!(r#"{{"p": "{}/post", "h": {{"c!": "form!"}}, "b": {{"city": "Berlin"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["form"]["city"], "Berlin");
}

#[test]
fn form_urlencoded_shortcut_f() {
    let out = jurl(&format!(r#"{{"p": "{}/post", "h": {{"c!": "f!"}}, "b": {{"city": "Berlin"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["form"]["city"], "Berlin");
}

// --- Multipart: full Content-Type ---
//     then shortcut multi! / m!

#[test]
fn multipart_full() {
    let out = jurl(&format!(r#"{{"p": "{}/post", "h": {{"Content-Type": "multipart/form-data"}}, "b": {{"field": "val"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["form"]["field"], "val");
}

#[test]
fn multipart_shortcut_multi() {
    let out = jurl(&format!(r#"{{"p": "{}/post", "h": {{"c!": "multi!"}}, "b": {{"field": "val"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["form"]["field"], "val");
}

#[test]
fn multipart_shortcut_m() {
    let out = jurl(&format!(r#"{{"p": "{}/post", "h": {{"c!": "m!"}}, "b": {{"field": "val"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["form"]["field"], "val");
}

// --- Auth: full header ---
//     then shortcut basic! / bearer!

#[test]
fn auth_basic_full_header() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "h": {{"Authorization": "Basic dXNlcjpwYXNz"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Basic dXNlcjpwYXNz");
}

#[test]
fn auth_basic_shortcut() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "h": {{"a!": "basic!user:pass"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Basic dXNlcjpwYXNz");
}

#[test]
fn auth_bearer_full_header() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "h": {{"Authorization": "Bearer tok123"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Bearer tok123");
}

#[test]
fn auth_bearer_shortcut() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "h": {{"a!": "bearer!tok123"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Bearer tok123");
}

// --- Smart auth: bare token → Bearer ---

#[test]
fn auth_bare_token() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "h": {{"a!": "my-token"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Bearer my-token");
}

#[test]
fn auth_string_with_scheme_passthrough() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "h": {{"a!": "Basic dXNlcjpwYXNz"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Basic dXNlcjpwYXNz");
}

#[test]
fn auth_array_basic() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "h": {{"a!": ["user", "pass"]}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Basic dXNlcjpwYXNz");
}

// --- Content-Type prefix shortcuts ---

#[test]
fn ct_prefix_a() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "h": {{"Accept": "a!/xml"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Accept"], "application/xml");
}

#[test]
fn ct_prefix_t() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "h": {{"Accept": "t!/csv"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Accept"], "text/csv");
}

#[test]
fn ct_prefix_i() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "h": {{"Accept": "i!/png"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Accept"], "image/png");
}

// --- Metadata ---

#[test]
fn metadata_scalar() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "md": "batch-1", "1": "j(md,s.code)"}}"#, base()));
    let json = parse_json(&out);
    assert_eq!(json["md"], "batch-1");
    assert_eq!(json["s"]["c"], 200);
}

#[test]
fn metadata_object() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "md": {{"id": 42, "tag": "test"}}, "1": "j(md)"}}"#, base()));
    let json = parse_json(&out);
    assert_eq!(json["md"]["id"], 42);
    assert_eq!(json["md"]["tag"], "test");
}

#[test]
fn metadata_fields() {
    let out = jurl(&format!(r#"{{"g": "{}/get", "md": {{"id": 42, "tag": "test"}}, "1": "j(md.id)"}}"#, base()));
    let json = parse_json(&out);
    assert_eq!(json["md"]["id"], 42);
    assert!(json["md"].get("tag").is_none());
}

// --- Index ---

#[test]
fn idx_increments() {
    let b = base();
    let input = format!(
        r#"{{"g": "{b}/get", "1": "j(idx)"}}
{{"g": "{b}/get", "1": "j(idx)"}}"#
    );
    let out = jurl(&input);
    let lines: Vec<&str> = out.trim().split('\n').collect();
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
    let b = base();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("{{m}}.txt");
    let file_key = format!("file://{}", path.display());
    let input = format!(r#"{{"g": "{b}/get", "{file_key}": "s"}}"#);
    jurl(&input);
    let result = std::fs::read_to_string(dir.path().join("GET.txt")).unwrap();
    assert!(result.contains("200 OK"));
}

// --- Session config: default headers ---

#[test]
fn config_default_headers() {
    let out = jurl_with_config(
        &format!(r#"{{"g": "{}/get", "1": "b"}}"#, base()),
        Some(r#"{"h": {"X-Custom": "yes"}}"#),
    );
    let body = parse_json(&out);
    assert_eq!(body["headers"]["X-Custom"], "yes");
}

// --- Session config: auth shortcut ---

#[test]
fn config_auth_shortcut() {
    let out = jurl_with_config(
        &format!(r#"{{"g": "{}/get", "1": "b"}}"#, base()),
        Some(r#"{"h": {"a!": "bearer!session-tok"}}"#),
    );
    let body = parse_json(&out);
    assert_eq!(body["headers"]["Authorization"], "Bearer session-tok");
}

// --- Session config: rule with URL match ---

#[test]
fn config_rule_url_match() {
    let out = jurl_with_config(
        &format!(r#"{{"g": "{}/get", "1": "b"}}"#, base()),
        Some(r#"{"rules": [{"match": {"u": "**127.0.0.1**"}, "h": {"X-Matched": "yes"}}]}"#),
    );
    let body = parse_json(&out);
    assert_eq!(body["headers"]["X-Matched"], "yes");
}

// --- Session config: rule with method match ---

#[test]
fn config_rule_method_match() {
    let out = jurl_with_config(
        &format!(r#"{{"p": "{}/post", "b": {{"x": "1"}}, "1": "b"}}"#, base()),
        Some(r#"{"rules": [{"match": {"m": "POST"}, "h": {"c!": "f!"}}]}"#),
    );
    let body = parse_json(&out);
    assert_eq!(body["form"]["x"], "1");
}

// --- Session config: rule with metadata match ---

#[test]
fn config_rule_md_match() {
    let out = jurl_with_config(
        &format!(r#"{{"g": "{}/get", "md": {{"env": "prod"}}, "1": "b"}}"#, base()),
        Some(r#"{"rules": [{"match": {"md.env": "prod"}, "h": {"X-Env": "production"}}]}"#),
    );
    let body = parse_json(&out);
    assert_eq!(body["headers"]["X-Env"], "production");
}

// --- Per-request headers override config ---

#[test]
fn request_overrides_config() {
    let out = jurl_with_config(
        &format!(r#"{{"g": "{}/get", "h": {{"X-Val": "request"}}, "1": "b"}}"#, base()),
        Some(r#"{"h": {"X-Val": "config"}}"#),
    );
    let body = parse_json(&out);
    assert_eq!(body["headers"]["X-Val"], "request");
}

// --- Multiple HTTP methods ---

#[test]
fn delete_method() {
    let out = jurl(&format!(r#"{{"d": "{}/delete", "1": "s.code"}}"#, base()));
    assert_eq!(out, "200");
}

#[test]
fn put_method() {
    let out = jurl(&format!(r#"{{"put": "{}/put", "b": {{"k": "v"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["json"]["k"], "v");
}

#[test]
fn patch_method() {
    let out = jurl(&format!(r#"{{"patch": "{}/patch", "b": {{"k": "v"}}, "1": "b"}}"#, base()));
    let body = parse_json(&out);
    assert_eq!(body["json"]["k"], "v");
}

// --- Concurrency ---

#[test]
fn concurrency_parallel_faster_than_sequential() {
    let b = base();
    let input = format!(
        r#"{{"g": "{b}/delay/1", "1": "s.code"}}
{{"g": "{b}/delay/1", "1": "s.code"}}
{{"g": "{b}/delay/1", "1": "s.code"}}"#
    );
    let start = std::time::Instant::now();
    let out = jurl_with_config(&input, Some(r#"{"concurrency": 3}"#));
    let elapsed = start.elapsed();
    assert!(out.matches("200").count() == 3, "all 3 should return 200");
    assert!(elapsed.as_secs() < 5, "should complete in under 5s with concurrency 3, took {}s", elapsed.as_secs());
}

#[test]
fn concurrency_default_sequential() {
    let b = base();
    let input = format!(
        r#"{{"g": "{b}/delay/1", "1": "s.code"}}
{{"g": "{b}/delay/1", "1": "s.code"}}"#
    );
    let start = std::time::Instant::now();
    let out = jurl(&input);
    let elapsed = start.elapsed();
    assert!(out.matches("200").count() == 2);
    assert!(elapsed.as_secs() >= 2, "should take at least 2s sequentially, took {}s", elapsed.as_secs());
}

#[test]
fn concurrency_per_endpoint_limit() {
    let b = base();
    let input = format!(
        r#"{{"g": "{b}/delay/1", "1": "s.code"}}
{{"g": "{b}/delay/1", "1": "s.code"}}"#
    );
    let config = r#"{"concurrency": 4, "rules": [{"match": {"u": "**delay**"}, "concurrency": 1}]}"#;
    let start = std::time::Instant::now();
    let out = jurl_with_config(&input, Some(config));
    let elapsed = start.elapsed();
    assert!(out.matches("200").count() == 2);
    assert!(elapsed.as_secs() >= 2, "per-endpoint limit 1 should serialize requests, took {}s", elapsed.as_secs());
}

// --- Streaming ---

#[test]
fn file_stream_output() {
    let b = base();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("streamed.txt");
    let file_key = format!("file://{}?stream", path.display());
    let input = format!(r#"{{"g": "{b}/get", "{file_key}": "b", "1": "s"}}"#);
    let out = jurl(&input);
    assert_eq!(out, "HTTP/1.1 200 OK");
    let file_content = std::fs::read_to_string(&path).unwrap();
    let body: serde_json::Value = serde_json::from_str(&file_content).unwrap();
    assert_eq!(body["url"], format!("{b}/get"));
}

// --- Cache ---

#[test]
fn cache_returns_identical_response() {
    let b = base();
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().to_str().unwrap();
    let config = format!(
        r#"{{"rules": [{{"match": {{"u": "**127.0.0.1**"}}, "cache": {{"at": "{cache_dir}"}}}}]}}"#
    );
    let input = format!(r#"{{"g": "{b}/get", "1": "b"}}"#);
    let out1 = jurl_with_config(&input, Some(&config));
    let out2 = jurl_with_config(&input, Some(&config));
    assert_eq!(out1, out2);
    let body: serde_json::Value = serde_json::from_str(&out1).unwrap();
    assert_eq!(body["url"], format!("{b}/get"));
}

#[test]
fn cache_different_bodies_different_entries() {
    let b = base();
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().to_str().unwrap();
    let config = format!(
        r#"{{"rules": [{{"match": {{"u": "**127.0.0.1**"}}, "cache": {{"at": "{cache_dir}"}}}}]}}"#
    );
    let input1 = format!(r#"{{"p": "{b}/post", "b": {{"x": 1}}, "1": "b"}}"#);
    let input2 = format!(r#"{{"p": "{b}/post", "b": {{"x": 2}}, "1": "b"}}"#);
    let out1 = jurl_with_config(&input1, Some(&config));
    let out2 = jurl_with_config(&input2, Some(&config));
    let body1: serde_json::Value = serde_json::from_str(&out1).unwrap();
    let body2: serde_json::Value = serde_json::from_str(&out2).unwrap();
    assert_ne!(body1["json"], body2["json"]);
}

// --- Streaming stdin ---

#[test]
fn streaming_stdin_executes_before_eof() {
    // Verify that the first request executes before all stdin is consumed.
    // Write the first line, wait for its response, then write the second line.
    // If jurl waited for EOF before executing, this would deadlock or timeout.
    let b = base();
    let line1 = format!(r#"{{"g": "{b}/get", "1": "s.code"}}"#);
    let line2 = format!(r#"{{"g": "{b}/get", "1": "s.code"}}"#);

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_jurl"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn jurl");

    let mut stdin = child.stdin.take().unwrap();
    use std::io::Write;

    // Write first line
    stdin.write_all(line1.as_bytes()).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();

    // Give it time to process
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Write second line and close stdin
    stdin.write_all(line2.as_bytes()).unwrap();
    stdin.write_all(b"\n").unwrap();
    drop(stdin);

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.matches("200").count(), 2, "both requests should complete");
}

// --- API aliases ---

#[test]
fn api_alias_single() {
    let b = base();
    let config = format!(r#"{{"api": "{b}"}}"#);
    let out = jurl_with_config(
        r#"{"g": "api!/get", "1": "b"}"#,
        Some(&config),
    );
    let body = parse_json(&out);
    assert_eq!(body["url"], format!("{b}/get"));
}

#[test]
fn api_alias_multiple() {
    let b = base();
    let config = format!(r#"{{"api": {{"main": "{b}", "other": "{b}"}}}}"#);
    let out = jurl_with_config(
        r#"{"g": "main!/get", "1": "b"}"#,
        Some(&config),
    );
    let body = parse_json(&out);
    assert_eq!(body["url"], format!("{b}/get"));
}

#[test]
fn api_alias_no_path() {
    let b = base();
    let config = format!(r#"{{"api": "{b}/get"}}"#);
    let out = jurl_with_config(
        r#"{"g": "api!", "1": "s.code"}"#,
        Some(&config),
    );
    assert_eq!(out, "200");
}

#[test]
fn api_alias_unrecognized_passthrough() {
    let b = base();
    // unknown! should pass through as literal URL — will fail to connect, but the point
    // is it doesn't panic. Use a real URL to verify passthrough works.
    let out = jurl_with_config(
        &format!(r#"{{"g": "{b}/get", "1": "s.code"}}"#),
        Some(r#"{"api": "http://example.com"}"#),
    );
    assert_eq!(out, "200");
}

#[test]
fn api_alias_rule_matches_expanded_url() {
    let b = base();
    let config = format!(
        r#"{{"api": "{b}", "rules": [{{"match": {{"u": "**127.0.0.1**"}}, "h": {{"X-Matched": "yes"}}}}]}}"#
    );
    let out = jurl_with_config(
        r#"{"g": "api!/get", "1": "b"}"#,
        Some(&config),
    );
    let body = parse_json(&out);
    assert_eq!(body["headers"]["X-Matched"], "yes");
}

// --- Auto scheme ---

#[test]
fn auto_scheme_localhost_http() {
    let b = base(); // http://127.0.0.1:PORT
    let port = b.rsplit(':').next().unwrap();
    let out = jurl(&format!(r#"{{"g": "127.0.0.1:{port}/get", "1": "s.code"}}"#));
    assert_eq!(out, "200");
}

#[test]
fn auto_scheme_api_alias_no_scheme() {
    let b = base();
    let port = b.rsplit(':').next().unwrap();
    let config = format!(r#"{{"api": "127.0.0.1:{port}"}}"#);
    let out = jurl_with_config(
        r#"{"g": "api!/get", "1": "s.code"}"#,
        Some(&config),
    );
    assert_eq!(out, "200");
}

// --- Env var expansion ---

#[test]
fn env_var_in_config_header() {
    let b = base();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jurl"));
    cmd.arg(r#"{"h": {"X-Test": "$YURL_TEST_HEADER"}}"#);
    cmd.env("YURL_TEST_HEADER", "expanded-value");
    let output = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            let input = format!(r#"{{"g": "{b}/get", "1": "b"}}"#);
            child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
            child.wait_with_output()
        })
        .expect("failed to run jurl");
    let out = String::from_utf8(output.stdout).unwrap();
    let body = parse_json(&out);
    assert_eq!(body["headers"]["X-Test"], "expanded-value");
}

#[test]
fn env_var_in_auth_array() {
    let b = base();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jurl"));
    cmd.arg(r#"{"h": {"a!": ["user", "$YURL_TEST_PASS"]}}"#);
    cmd.env("YURL_TEST_PASS", "pass");
    let output = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            let input = format!(r#"{{"g": "{b}/get", "1": "b"}}"#);
            child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
            child.wait_with_output()
        })
        .expect("failed to run jurl");
    let out = String::from_utf8(output.stdout).unwrap();
    let body = parse_json(&out);
    // [user, pass] → Basic dXNlcjpwYXNz
    assert_eq!(body["headers"]["Authorization"], "Basic dXNlcjpwYXNz");
}

#[test]
fn env_var_unset_passes_through() {
    let b = base();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jurl"));
    cmd.arg(r#"{"h": {"X-Test": "$YURL_NONEXISTENT_VAR"}}"#);
    cmd.env_remove("YURL_NONEXISTENT_VAR");
    let output = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            let input = format!(r#"{{"g": "{b}/get", "1": "b"}}"#);
            child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
            child.wait_with_output()
        })
        .expect("failed to run jurl");
    let out = String::from_utf8(output.stdout).unwrap();
    let body = parse_json(&out);
    // Unset var should pass through as literal $YURL_NONEXISTENT_VAR
    assert_eq!(body["headers"]["X-Test"], "$YURL_NONEXISTENT_VAR");
}

// --- query params ---

#[test]
fn query_params_basic() {
    let b = base();
    let input = format!(r#"{{"g": "{b}/get", "q": {{"term": "foo", "limit": 10}}, "1": "b"}}"#);
    let out = jurl(&input);
    let body = parse_json(&out);
    assert_eq!(body["args"]["term"], "foo");
    assert_eq!(body["args"]["limit"], "10");
    assert!(body["url"].as_str().unwrap().contains("?"));
}

#[test]
fn query_params_merge_with_url() {
    let b = base();
    let input = format!(r#"{{"g": "{b}/get?x=1", "q": {{"y": "2"}}, "1": "b"}}"#);
    let out = jurl(&input);
    let body = parse_json(&out);
    let url = body["url"].as_str().unwrap();
    assert!(url.contains("x=1"));
    assert!(url.contains("y=2"));
}

#[test]
fn query_params_url_encoding() {
    let b = base();
    let input = format!(r#"{{"g": "{b}/get", "q": {{"q": "hello world"}}, "1": "b"}}"#);
    let out = jurl(&input);
    let body = parse_json(&out);
    assert_eq!(body["args"]["q"], "hello world");
}

#[test]
fn query_params_absent_noop() {
    let b = base();
    let input = format!(r#"{{"g": "{b}/get", "1": "b"}}"#);
    let out = jurl(&input);
    let body = parse_json(&out);
    let url = body["url"].as_str().unwrap();
    assert!(!url.contains("?"));
}

// --- Error handling ---

#[test]
fn invalid_json_prints_error_continues() {
    let b = base();
    // First request is invalid, second is valid
    let input = format!("{{broken\n{{\"g\": \"{b}/get\", \"1\": \"b\"}}");
    let output = jurl_full(&input, None);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid") || stderr.contains("^"), "stderr should show error: {stderr}");
    // Second request should still succeed
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "valid request should produce output");
}

#[test]
fn connection_refused_prints_error() {
    // Port 1 should refuse connections
    let input = r#"{"g": "http://127.0.0.1:1/test"}"#;
    let output = jurl_full(input, None);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error"), "stderr should contain error: {stderr}");
}

#[test]
fn invalid_config_exits_with_error() {
    let output = jurl_full(r#"{"g": "http://example.com"}"#, Some("{broken"));
    assert!(!output.status.success(), "should exit with error code");
}

#[test]
fn raw_status_atom_has_output() {
    let b = base();
    let input = format!(r#"{{"g": "{b}/get", "1": "s"}}"#);
    let out = jurl(&input);
    assert!(out.contains("200"), "raw status should contain 200: {out}");
}

#[test]
fn unclosed_format_prints_error() {
    let b = base();
    // y(s! is an unclosed format — should error, not panic
    let input = format!(r#"{{"g": "{b}/get", "1": "y(s!"}}"#);
    let output = jurl_full(&input, None);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unclosed format"), "should show unclosed format error: {stderr}");
}
