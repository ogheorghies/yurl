/// Embedded httpbin-like test server for integration tests.
///
/// Supports: GET/POST/PUT/PATCH/DELETE echo endpoints, /delay/{secs}.
/// All echo endpoints return JSON mirroring httpbin's format:
///   { "url", "headers", "json", "form", "args", "method", "origin" }

use axum::{
    Router,
    body::Bytes,
    extract::Path,
    http::{HeaderMap, Method, Uri},
    routing::{delete, get, patch, post, put},
};
use serde_json::{Map, Value, json};
use std::net::SocketAddr;
use std::sync::OnceLock;

static SERVER_ADDR: OnceLock<SocketAddr> = OnceLock::new();

/// Returns the base URL of the running test server (e.g. "http://127.0.0.1:PORT").
/// Starts the server on first call; subsequent calls return the same address.
pub fn base_url() -> String {
    let addr = SERVER_ADDR.get_or_init(|| {
        let app = Router::new()
            .route("/get", get(echo_handler))
            .route("/post", post(echo_handler))
            .route("/put", put(echo_handler))
            .route("/patch", patch(echo_handler))
            .route("/delete", delete(echo_handler))
            .route("/delay/{secs}", get(delay_handler));

        // Bind with std listener to get the address synchronously
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
                axum::serve(listener, app).await.unwrap();
            });
        });

        // Give the server thread a moment to start
        std::thread::sleep(std::time::Duration::from_millis(50));

        addr
    });
    format!("http://{addr}")
}

async fn echo_handler(
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> axum::Json<Value> {
    let mut headers_obj = Map::new();
    for (name, value) in &headers {
        let v = value.to_str().unwrap_or("").to_string();
        // Skip hop-by-hop and internal headers for cleaner output
        let n = name.as_str();
        if n == "host" || n == "content-length" || n == "transfer-encoding" {
            continue;
        }
        // Capitalize header names like httpbin does (Title-Case)
        let capitalized = n
            .split('-')
            .map(|part| {
                let mut c = part.chars();
                match c.next() {
                    Some(first) => {
                        let upper: String = first.to_uppercase().collect();
                        format!("{upper}{}", c.as_str())
                    }
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join("-");
        headers_obj.insert(capitalized, Value::String(v));
    }

    // Reconstruct full URL including query string (tests compare against it)
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let full_url = if let Some(q) = uri.query() {
        format!("http://{host}{}?{q}", uri.path())
    } else {
        format!("http://{host}{}", uri.path())
    };

    // Parse query string into args
    let mut args = Map::new();
    if let Some(q) = uri.query() {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                let k = urldecode(k);
                let v = urldecode(v);
                args.insert(k, Value::String(v));
            }
        }
    }

    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let mut json_body = Value::Null;
    let mut form_body = Value::Null;

    if !body.is_empty() {
        if content_type.contains("application/json") {
            json_body = serde_json::from_slice(&body).unwrap_or(Value::Null);
        } else if content_type.contains("application/x-www-form-urlencoded") {
            let mut form = Map::new();
            let text = String::from_utf8_lossy(&body);
            for pair in text.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    let k = urldecode(k);
                    let v = urldecode(v);
                    form.insert(k, Value::String(v));
                }
            }
            form_body = Value::Object(form);
        } else if content_type.contains("multipart/form-data") {
            if let Some(boundary) = content_type
                .split("boundary=")
                .nth(1)
                .map(|b| b.trim().to_string())
            {
                let mut form = Map::new();
                let stream = futures_util::stream::once(async { Ok::<_, std::io::Error>(body.clone()) });
                let mut multipart = multer::Multipart::new(stream, boundary);
                while let Ok(Some(field)) = multipart.next_field().await {
                    let name = field.name().unwrap_or("").to_string();
                    if let Ok(text) = field.text().await {
                        form.insert(name, Value::String(text));
                    }
                }
                form_body = Value::Object(form);
            }
        }
    }

    axum::Json(json!({
        "url": full_url,
        "headers": headers_obj,
        "json": json_body,
        "form": form_body,
        "args": args,
        "method": method.as_str(),
        "origin": "127.0.0.1"
    }))
}

async fn delay_handler(Path(secs): Path<u64>, method: Method, uri: Uri, headers: HeaderMap) -> axum::Json<Value> {
    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;

    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let full_url = format!("http://{host}{}", uri.path());

    axum::Json(json!({
        "url": full_url,
        "args": {},
        "method": method.as_str(),
        "origin": "127.0.0.1"
    }))
}

fn urldecode(s: &str) -> String {
    let s = s.replace('+', " ");
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            }
        } else {
            result.push(c);
        }
    }
    result
}
