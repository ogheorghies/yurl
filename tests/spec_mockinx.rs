/// YAML spec-driven integration tests backed by mockinx.
///
/// Each spec file in tests/specs/*.yaml contains test cases.
/// The runner starts mockinx on a random port, configures rules via /_mx,
/// runs yurl/jurl, and asserts on output.
///
/// Assertion format:
///   assert:
///     exit_code: 0
///     stdout:
///       is: "exact match"
///       contains: ["substring"]
///       not_contains: ["bad"]
///       json:
///         s.c: 200
///     stderr:
///       contains: ["error"]

use serde::Deserialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::process::Command;
use std::sync::OnceLock;

static MOCKINX_ADDR: OnceLock<SocketAddr> = OnceLock::new();

fn mockinx_url() -> String {
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

        addr
    });
    format!("http://{addr}")
}

#[derive(Debug, Deserialize)]
struct Spec {
    name: String,
    #[serde(default)]
    rules: Vec<serde_json::Value>,
    input: String,
    #[serde(default)]
    config: Option<String>,
    #[serde(default = "default_bin")]
    bin: String,
    #[serde(default)]
    assert: Assertions,
}

fn default_bin() -> String {
    "jurl".to_string()
}

#[derive(Debug, Deserialize, Default)]
struct Assertions {
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default, deserialize_with = "deser_stream_assert")]
    stdout: Option<StreamAssert>,
    #[serde(default, deserialize_with = "deser_stream_assert")]
    stderr: Option<StreamAssert>,
}

/// Deserialize StreamAssert from either a string (shorthand for {is: "..."})
/// or a full object.
fn deser_stream_assert<'de, D>(deserializer: D) -> Result<Option<StreamAssert>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct StreamAssertVisitor;

    impl<'de> de::Visitor<'de> for StreamAssertVisitor {
        type Value = Option<StreamAssert>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or a stream assert object")
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(Some(StreamAssert {
                is: Some(v.to_string()),
                ..Default::default()
            }))
        }

        fn visit_map<M: de::MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
            let sa = StreamAssert::deserialize(de::value::MapAccessDeserializer::new(map))?;
            Ok(Some(sa))
        }
    }

    deserializer.deserialize_any(StreamAssertVisitor)
}

#[derive(Debug, Deserialize, Default)]
struct StreamAssert {
    #[serde(default)]
    is: Option<String>,
    #[serde(default)]
    contains: Vec<String>,
    #[serde(default)]
    not_contains: Vec<String>,
    #[serde(default)]
    json: HashMap<String, serde_json::Value>,
}

fn setup_rules(base: &str, rules: &[serde_json::Value]) {
    let client = reqwest::blocking::Client::new();
    let body = serde_json::to_string(rules).unwrap();
    let resp = client
        .put(format!("{base}/_mx"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .expect("failed to PUT rules to mockinx");
    assert!(
        resp.status().is_success(),
        "failed to set mockinx rules: {}",
        resp.text().unwrap_or_default()
    );
}

fn resolve_mx(s: &str, base: &str) -> String {
    let s = s.replace("mx!", &format!("{base}/"));
    let s = s.replace(&format!("{base}//"), &format!("{base}/"));
    s.replace("\\n", "\n")
}

fn run_spec(spec: &Spec, base: &str) {
    if !spec.rules.is_empty() {
        setup_rules(base, &spec.rules);
    } else {
        setup_rules(base, &[]);
    }

    let input = resolve_mx(&spec.input, base);

    let bin_exe = match spec.bin.as_str() {
        "yurl" => env!("CARGO_BIN_EXE_yurl"),
        _ => env!("CARGO_BIN_EXE_jurl"),
    };

    let mut cmd = Command::new(bin_exe);
    if let Some(ref cfg) = spec.config {
        cmd.arg(resolve_mx(cfg, base));
    }

    let output = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            child.wait_with_output()
        })
        .expect("failed to run binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let name = &spec.name;

    // Assert exit code
    let expected_code = spec.assert.exit_code.unwrap_or(0);
    let actual_code = output.status.code().unwrap_or(-1);
    assert_eq!(
        actual_code, expected_code,
        "[{name}] exit code: expected {expected_code}, got {actual_code}\nstdout: {stdout}\nstderr: {stderr}",
    );

    // Assert stdout
    if let Some(ref a) = spec.assert.stdout {
        assert_stream(name, "stdout", &stdout, &stderr, a);
    }

    // Assert stderr
    if let Some(ref a) = spec.assert.stderr {
        assert_stream(name, "stderr", &stderr, &stdout, a);
    }
}

fn assert_stream(name: &str, stream: &str, value: &str, other: &str, a: &StreamAssert) {
    if let Some(ref expected) = a.is {
        let trimmed = value.trim();
        let expected = expected;
        assert_eq!(
            trimmed, expected.as_str(),
            "[{name}] {stream}.is: expected {expected:?}, got {trimmed:?}\nother stream: {other}",
        );
    }

    for needle in &a.contains {
        assert!(
            value.contains(needle.as_str()),
            "[{name}] {stream} should contain {needle:?}\n{stream}: {value}\nother: {other}",
        );
    }

    for needle in &a.not_contains {
        assert!(
            !value.contains(needle.as_str()),
            "[{name}] {stream} should NOT contain {needle:?}\n{stream}: {value}",
        );
    }

    if !a.json.is_empty() {
        let json: serde_json::Value = serde_json::from_str(value.trim())
            .or_else(|_| {
                value
                    .lines()
                    .find_map(|line| serde_json::from_str(line).ok())
                    .ok_or("no JSON found")
            })
            .unwrap_or_else(|_| {
                panic!("[{name}] {stream}.json: no valid JSON\n{stream}: {value}")
            });

        for (path, expected) in &a.json {
            let actual = json_path(&json, path);
            assert_eq!(
                actual, Some(expected),
                "[{name}] {stream}.json {path:?}: expected {expected}, got {actual:?}\nfull: {json}",
            );
        }
    }
}

/// Simple dot-notation JSON path lookup: "s.c" → json["s"]["c"]
fn json_path<'a>(json: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = json;
    for key in path.split('.') {
        current = current.get(key)?;
    }
    Some(current)
}

fn load_and_run_specs(path: &str) {
    let base = mockinx_url();
    let content = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let specs: Vec<Spec> = serde_yml::from_str(&content)
        .unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    for spec in &specs {
        run_spec(spec, &base);
    }
}

/// All specs run in a single test to avoid parallel rule conflicts on the shared mockinx instance.
#[test]
fn all_specs() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/specs/");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .expect("failed to read tests/specs/")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "yaml"))
        .collect();
    paths.sort();

    assert!(!paths.is_empty(), "no spec files found in {dir}");

    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy();
        eprintln!("--- {name} ---");
        load_and_run_specs(path.to_str().unwrap());
    }
}
