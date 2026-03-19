/// Faux REST API server for the step-mode demo.
/// Endpoints: GET/POST /toys, GET/PUT/DELETE /toys/{id}
/// Auth: Basic admin:secret (returns 401 without it)
/// X-Verbose: true → adds _x_verbose field to responses

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{delete, get, post, put},
};
use tower::Layer;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

type Db = Arc<Mutex<BTreeMap<u32, Value>>>;

fn seed() -> BTreeMap<u32, Value> {
    let mut m = BTreeMap::new();
    m.insert(1, json!({"id": 1, "name": "Cat", "price": 9.99}));
    m.insert(2, json!({"id": 2, "name": "Bat", "price": 4.99}));
    m.insert(3, json!({"id": 3, "name": "Dog", "price": 7.50}));
    m
}

fn check_auth(headers: &HeaderMap) -> bool {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        // admin:secret → Basic YWRtaW46c2VjcmV0
        .map(|v| v == "Basic YWRtaW46c2VjcmV0")
        .unwrap_or(false)
}

fn debug_info(headers: &HeaderMap) -> Option<Value> {
    let is_debug = headers
        .get("x-debug")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    if is_debug {
        let load = format!("{}%", 30 + (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() % 60));
        let region = if std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().subsec_micros() % 2 == 0 {
            "eu-west-1"
        } else {
            "eu-west-2"
        };
        Some(json!({"load": load, "region": region}))
    } else {
        None
    }
}

fn inject_debug(mut val: Value, headers: &HeaderMap) -> Value {
    if let Some(v) = debug_info(headers) {
        if let Some(obj) = val.as_object_mut() {
            obj.insert("_x_debug".to_string(), v);
        }
    }
    val
}

const DELAY: std::time::Duration = std::time::Duration::from_millis(300);

async fn list_items(State(db): State<Db>, headers: HeaderMap) -> impl IntoResponse {
    tokio::time::sleep(DELAY).await;
    if !check_auth(&headers) {
        return (StatusCode::UNAUTHORIZED, axum::Json(json!({"error": "unauthorized"}))).into_response();
    }
    let db = db.lock().unwrap();
    let mut items: Vec<&Value> = db.values().collect();
    items.sort_by_key(|v| v["id"].as_u64());
    let mut result = json!({"toys": items});
    result = inject_debug(result, &headers);
    axum::Json(result).into_response()
}

async fn get_item(State(db): State<Db>, Path(id): Path<u32>, headers: HeaderMap) -> impl IntoResponse {
    tokio::time::sleep(DELAY).await;
    if !check_auth(&headers) {
        return (StatusCode::UNAUTHORIZED, axum::Json(json!({"error": "unauthorized"}))).into_response();
    }
    let db = db.lock().unwrap();
    match db.get(&id) {
        Some(item) => {
            let result = inject_debug(item.clone(), &headers);
            axum::Json(result).into_response()
        }
        None => (StatusCode::NOT_FOUND, axum::Json(json!({"error": "not found"}))).into_response(),
    }
}

async fn create_item(State(db): State<Db>, headers: HeaderMap, body: axum::body::Bytes) -> impl IntoResponse {
    tokio::time::sleep(DELAY).await;
    if !check_auth(&headers) {
        return (StatusCode::UNAUTHORIZED, axum::Json(json!({"error": "unauthorized"}))).into_response();
    }
    let mut input: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    let mut db = db.lock().unwrap();
    let next_id = db.keys().max().copied().unwrap_or(0) + 1;
    if let Some(obj) = input.as_object_mut() {
        obj.insert("id".to_string(), json!(next_id));
    }
    db.insert(next_id, input.clone());
    let result = inject_debug(input, &headers);
    (StatusCode::CREATED, axum::Json(result)).into_response()
}

async fn update_item(State(db): State<Db>, Path(id): Path<u32>, headers: HeaderMap, body: axum::body::Bytes) -> impl IntoResponse {
    tokio::time::sleep(DELAY).await;
    if !check_auth(&headers) {
        return (StatusCode::UNAUTHORIZED, axum::Json(json!({"error": "unauthorized"}))).into_response();
    }
    let mut input: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    let mut db = db.lock().unwrap();
    if !db.contains_key(&id) {
        return (StatusCode::NOT_FOUND, axum::Json(json!({"error": "not found"}))).into_response();
    }
    if let Some(obj) = input.as_object_mut() {
        obj.insert("id".to_string(), json!(id));
    }
    db.insert(id, input.clone());
    let result = inject_debug(input, &headers);
    axum::Json(result).into_response()
}

async fn delete_item(State(db): State<Db>, Path(id): Path<u32>, headers: HeaderMap) -> impl IntoResponse {
    tokio::time::sleep(DELAY).await;
    if !check_auth(&headers) {
        return (StatusCode::UNAUTHORIZED, axum::Json(json!({"error": "unauthorized"}))).into_response();
    }
    let mut db = db.lock().unwrap();
    if db.remove(&id).is_some() {
        let result = inject_debug(json!({"deleted": true, "id": id}), &headers);
        axum::Json(result).into_response()
    } else {
        (StatusCode::NOT_FOUND, axum::Json(json!({"error": "not found"}))).into_response()
    }
}

#[derive(Clone)]
struct StripDateLayer;

impl<S> Layer<S> for StripDateLayer {
    type Service = tower::util::MapResponse<S, fn(axum::response::Response) -> axum::response::Response>;

    fn layer(&self, inner: S) -> Self::Service {
        fn strip(mut res: axum::response::Response) -> axum::response::Response {
            res.headers_mut().remove(header::DATE);
            res
        }
        tower::util::MapResponse::new(inner, strip)
    }
}

#[tokio::main]
async fn main() {
    let db: Db = Arc::new(Mutex::new(seed()));

    let app = Router::new()
        .route("/toys", get(list_items).post(create_item))
        .route("/toys/{id}", get(get_item).put(update_item).delete(delete_item))
        .with_state(db);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3456").await.unwrap();
    eprintln!("demo server listening on http://127.0.0.1:3456");

    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let app = app.clone();
        tokio::spawn(async move {
            let io = hyper_util::rt::TokioIo::new(stream);
            let svc = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                let app = app.clone();
                async move {
                    let (parts, body) = req.into_parts();
                    let body = axum::body::Body::new(body);
                    let req = axum::http::Request::from_parts(parts, body);
                    let resp = tower::ServiceExt::oneshot(app, req).await;
                    resp.map(|b| b.into())
                }
            });
            hyper::server::conn::http1::Builder::new()
                .auto_date_header(false)
                .serve_connection(io, svc)
                .await
                .ok();
        });
    }
}
