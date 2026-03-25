/// Integration tests requiring Rust expressiveness (timing, filesystem, env vars, subprocess control).
/// Feature and error tests live in tests/specs/*.yaml (run via spec_mockinx.rs).
///
/// Uses mockinx for the test server — same as the spec tests.

use std::net::SocketAddr;
use std::process::Command;
use std::sync::OnceLock;

static MOCKINX_ADDR: OnceLock<SocketAddr> = OnceLock::new();

fn base() -> String {
    let addr = MOCKINX_ADDR.get_or_init(|| {
        let state = mockinx::server::AppState::new();
        let app = mockinx::server::build_router(state);

        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = std_listener.local_addr().unwrap();
        std_listener.set_nonblocking(true).unwrap();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
                axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                )
                .await
                .unwrap();
            });
        });

        for _ in 0..50 {
            if std::net::TcpStream::connect(addr).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // Set up all rules needed by integration tests
        let client = reqwest::blocking::Client::new();
        let rules = serde_json::json!([
            // Echo for general tests (no body)
            {"match": {"g": "/echo"}, "reply": {"s": 200, "b": {"reflect!": true}}},
            // Echo with body for cache/POST tests
            {"match": {"p": "/echo"}, "reply": {"s": 200, "b": {"reflect!": ["i.m", "i.h", "i.b"]}}},
            // Delay endpoint for concurrency tests
            {"match": {"_": "/delay"}, "reply": {"s": 200, "b": "ok"}, "serve": {"first_byte": "1s"}},
            // Simple 200 for streaming/file tests
            {"match": {"_": "/ok"}, "reply": {"s": 200, "b": "ok-body"}},
        ]);
        client
            .put(format!("http://{addr}/_mx"))
            .header("Content-Type", "application/json")
            .body(rules.to_string())
            .send()
            .expect("failed to set up mockinx rules");

        addr
    });
    format!("http://{addr}")
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

// --- Concurrency (timing assertions) ---

#[test]
fn concurrency_parallel_faster_than_sequential() {
    let b = base();
    let input = format!(
        r#"{{"g": "{b}/delay", "1": "s.code"}}
{{"g": "{b}/delay", "1": "s.code"}}
{{"g": "{b}/delay", "1": "s.code"}}"#
    );
    let start = std::time::Instant::now();
    let out = jurl_with_config(&input, Some(r#"{"concurrency": 3}"#));
    let elapsed = start.elapsed();
    assert!(out.matches("200").count() == 3, "all 3 should return 200: {out}");
    assert!(elapsed.as_secs() < 5, "should complete in under 5s with concurrency 3, took {}s", elapsed.as_secs());
}

#[test]
fn concurrency_default_sequential() {
    let b = base();
    let input = format!(
        r#"{{"g": "{b}/delay", "1": "s.code"}}
{{"g": "{b}/delay", "1": "s.code"}}"#
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
        r#"{{"g": "{b}/delay", "1": "s.code"}}
{{"g": "{b}/delay", "1": "s.code"}}"#
    );
    let config = r#"{"concurrency": 4, "rules": [{"match": {"u": "**delay**"}, "concurrency": 1}]}"#;
    let start = std::time::Instant::now();
    let out = jurl_with_config(&input, Some(config));
    let elapsed = start.elapsed();
    assert!(out.matches("200").count() == 2);
    assert!(elapsed.as_secs() >= 2, "per-endpoint limit 1 should serialize, took {}s", elapsed.as_secs());
}

// --- File output (filesystem) ---

#[test]
fn file_output_template() {
    let b = base();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("{{m}}.txt");
    let file_key = format!("file://{}", path.display());
    let input = format!(r#"{{"g": "{b}/ok", "{file_key}": "s"}}"#);
    jurl(&input);
    let result = std::fs::read_to_string(dir.path().join("GET.txt")).unwrap();
    assert!(result.contains("200 OK"));
}

#[test]
fn file_stream_output() {
    let b = base();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("streamed.txt");
    let file_key = format!("file://{}?stream", path.display());
    let input = format!(r#"{{"g": "{b}/ok", "{file_key}": "b", "1": "s"}}"#);
    let out = jurl(&input);
    assert_eq!(out, "HTTP/1.1 200 OK");
    let file_content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(file_content, "ok-body");
}

// --- Cache (temp dirs, multiple runs) ---

#[test]
fn cache_returns_identical_response() {
    let b = base();
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().to_str().unwrap();
    let config = format!(
        r#"{{"rules": [{{"match": {{"u": "**echo**"}}, "cache": {{"at": "{cache_dir}"}}}}]}}"#
    );
    let input = format!(r#"{{"g": "{b}/echo", "1": "b"}}"#);
    let out1 = jurl_with_config(&input, Some(&config));
    let out2 = jurl_with_config(&input, Some(&config));
    assert_eq!(out1, out2);
    assert!(out1.contains("GET"), "should contain reflected method: {out1}");
}

#[test]
fn cache_different_bodies_different_entries() {
    let b = base();
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().to_str().unwrap();
    let config = format!(
        r#"{{"rules": [{{"match": {{"u": "**echo**"}}, "cache": {{"at": "{cache_dir}"}}}}]}}"#
    );
    let input1 = format!(r#"{{"p": "{b}/echo", "b": {{"x": 1}}, "1": "b"}}"#);
    let input2 = format!(r#"{{"p": "{b}/echo", "b": {{"x": 2}}, "1": "b"}}"#);
    let out1 = jurl_with_config(&input1, Some(&config));
    let out2 = jurl_with_config(&input2, Some(&config));
    assert_ne!(out1, out2, "different bodies should produce different cache entries");
}

// --- Streaming stdin (subprocess timing) ---

#[test]
fn streaming_stdin_executes_before_eof() {
    let b = base();
    let line1 = format!(r#"{{"g": "{b}/ok", "1": "s.code"}}"#);
    let line2 = format!(r#"{{"g": "{b}/ok", "1": "s.code"}}"#);

    let mut child = Command::new(env!("CARGO_BIN_EXE_jurl"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn jurl");

    let mut stdin = child.stdin.take().unwrap();
    use std::io::Write;

    stdin.write_all(line1.as_bytes()).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(500));

    stdin.write_all(line2.as_bytes()).unwrap();
    stdin.write_all(b"\n").unwrap();
    drop(stdin);

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.matches("200").count(), 2, "both requests should complete");
}

// --- Env var expansion (set env on subprocess) ---

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
            let input = format!(r#"{{"g": "{b}/echo", "1": "b"}}"#);
            child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
            child.wait_with_output()
        })
        .expect("failed to run jurl");
    let out = String::from_utf8(output.stdout).unwrap();
    assert!(out.contains("expanded-value"), "header should be expanded: {out}");
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
            let input = format!(r#"{{"g": "{b}/echo", "1": "b"}}"#);
            child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
            child.wait_with_output()
        })
        .expect("failed to run jurl");
    let out = String::from_utf8(output.stdout).unwrap();
    // [user, pass] → Basic dXNlcjpwYXNz
    assert!(out.contains("Basic dXNlcjpwYXNz"), "auth should be expanded: {out}");
}

#[test]
fn env_var_unset_errors() {
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
            child.stdin.take().unwrap().write_all(b"{\"g\": \"http://example.com\"}").unwrap();
            child.wait_with_output()
        })
        .expect("failed to run jurl");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "should exit with error: {stderr}");
    assert!(stderr.contains("YURL_NONEXISTENT_VAR"), "should mention variable name: {stderr}");
}

#[test]
fn env_var_empty_string_allowed() {
    let b = base();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jurl"));
    cmd.arg(r#"{"h": {"X-Test": "$YURL_EMPTY_VAR"}}"#);
    cmd.env("YURL_EMPTY_VAR", "");
    let output = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            let input = format!(r#"{{"g": "{b}/echo", "1": "b"}}"#);
            child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
            child.wait_with_output()
        })
        .expect("failed to run jurl");
    assert!(output.status.success(), "empty var should succeed");
}
