use rusqlite::Connection;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

// --- Config types (issue 0010) ---

#[derive(Clone, Debug, PartialEq)]
pub enum CacheKey {
    Method,
    Url,
    Body,
    Auth,
    AllHeaders,
    Header(String),
}

#[derive(Clone, Debug)]
pub struct CacheConfig {
    pub ttl: u64,
    pub keys: Vec<CacheKey>,
    pub at: String,
}

impl Default for CacheConfig {
    fn default() -> Self {
        CacheConfig {
            ttl: 0,
            keys: vec![CacheKey::Method, CacheKey::Url, CacheKey::Body],
            at: default_cache_dir(),
        }
    }
}

fn default_cache_dir() -> String {
    dirs_cache_dir().unwrap_or_else(|| "~/.cache/yurl".to_string())
}

fn dirs_cache_dir() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        std::env::var("HOME")
            .ok()
            .map(|h| format!("{h}/Library/Caches/yurl"))
    }
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_CACHE_HOME")
            .ok()
            .or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.cache")))
            .map(|d| format!("{d}/yurl"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

fn parse_cache_key(s: &str) -> CacheKey {
    match s {
        "m" => CacheKey::Method,
        "u" => CacheKey::Url,
        "b" => CacheKey::Body,
        "a" => CacheKey::Auth,
        "h" => CacheKey::AllHeaders,
        other if other.starts_with("h.") => CacheKey::Header(other[2..].to_string()),
        other => panic!("unknown cache key: {other}"),
    }
}

/// Parse `cache` field from a rule JSON object.
/// Returns `None` if the field is absent.
/// `cache: true` → defaults.
/// `cache: {ttl, keys, at}` → override defaults for provided fields.
pub fn parse_cache(val: &Value) -> Option<CacheConfig> {
    match val {
        Value::Bool(true) => Some(CacheConfig::default()),
        Value::Bool(false) => None,
        Value::Object(obj) => {
            let defaults = CacheConfig::default();
            let ttl = obj
                .get("ttl")
                .and_then(|v| v.as_u64())
                .unwrap_or(defaults.ttl);
            let keys = obj
                .get("keys")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(parse_cache_key)
                        .collect()
                })
                .unwrap_or(defaults.keys);
            let at = obj
                .get("at")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or(defaults.at);
            Some(CacheConfig { ttl, keys, at })
        }
        _ => None,
    }
}

// --- Cache store (issue 0020) ---

pub struct CachedResponse {
    pub status: u16,
    pub headers: Map<String, Value>,
    pub body: Vec<u8>,
}

pub struct CacheStore {
    conn: Connection,
}

impl CacheStore {
    pub fn open(dir: &str) -> Self {
        let dir = expand_tilde(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = Path::new(&dir).join("cache.db");
        let conn = Connection::open(db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cache (
                key     TEXT PRIMARY KEY,
                url     TEXT,
                status  INTEGER,
                headers TEXT,
                body    BLOB,
                size    INTEGER,
                created INTEGER,
                expires INTEGER
            );",
        )
        .unwrap();
        // Startup eviction
        let now = unix_now();
        conn.execute(
            "DELETE FROM cache WHERE expires > 0 AND expires < ?1",
            [now as i64],
        )
        .unwrap();
        CacheStore { conn }
    }

    pub fn get(&self, key: &str) -> Option<CachedResponse> {
        let now = unix_now();
        let mut stmt = self
            .conn
            .prepare("SELECT status, headers, body, expires FROM cache WHERE key = ?1")
            .unwrap();
        stmt.query_row([key], |row| {
            let expires: i64 = row.get(3)?;
            if expires > 0 && (expires as u64) < now {
                return Ok(None);
            }
            let status: u32 = row.get(0)?;
            let headers_json: String = row.get(1)?;
            let body: Vec<u8> = row.get(2)?;
            let headers: Map<String, Value> =
                serde_json::from_str(&headers_json).unwrap_or_default();
            Ok(Some(CachedResponse {
                status: status as u16,
                headers,
                body,
            }))
        })
        .ok()
        .flatten()
    }

    pub fn put(&self, key: &str, url: &str, resp: &CachedResponse, ttl: u64) {
        let now = unix_now();
        let expires = if ttl == 0 { 0 } else { now + ttl };
        let headers_json = serde_json::to_string(&resp.headers).unwrap();
        self.conn
            .execute(
                "INSERT OR REPLACE INTO cache (key, url, status, headers, body, size, created, expires)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    key,
                    url,
                    resp.status as u32,
                    headers_json,
                    resp.body,
                    resp.body.len() as i64,
                    now as i64,
                    expires as i64,
                ],
            )
            .unwrap();
    }
}

fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}{}", &path[1..]);
        }
    }
    path.to_string()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Compute a cache key by hashing the selected request parts.
pub fn compute_cache_key(
    config: &CacheConfig,
    method: &str,
    url: &str,
    body: &Option<Value>,
    headers: &Map<String, Value>,
) -> String {
    let mut hasher = Sha256::new();
    for key in &config.keys {
        match key {
            CacheKey::Method => hasher.update(method.as_bytes()),
            CacheKey::Url => hasher.update(url.as_bytes()),
            CacheKey::Body => {
                if let Some(b) = body {
                    hasher.update(serde_json::to_string(b).unwrap().as_bytes());
                }
            }
            CacheKey::Auth => {
                if let Some(v) = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("authorization")).map(|(_, v)| v) {
                    hasher.update(v.as_str().unwrap_or("").as_bytes());
                }
            }
            CacheKey::AllHeaders => {
                // Sorted iteration for deterministic hashing
                let mut pairs: Vec<_> = headers.iter().collect();
                pairs.sort_by_key(|(k, _)| k.to_lowercase());
                for (k, v) in pairs {
                    hasher.update(k.as_bytes());
                    hasher.update(v.as_str().unwrap_or("").as_bytes());
                }
            }
            CacheKey::Header(name) => {
                if let Some(v) = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v) {
                    hasher.update(v.as_str().unwrap_or("").as_bytes());
                }
            }
        }
    }
    format!("{:x}", hasher.finalize())
}

// --- Store registry (issue 0030) ---

/// Manages lazily-opened cache stores keyed by directory path.
pub struct CacheStores {
    stores: Mutex<HashMap<String, Arc<Mutex<CacheStore>>>>,
}

impl CacheStores {
    pub fn new() -> Self {
        CacheStores {
            stores: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, dir: &str) -> Arc<Mutex<CacheStore>> {
        let mut map = self.stores.lock().unwrap();
        map.entry(dir.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(CacheStore::open(dir))))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cache_true() {
        let config = parse_cache(&Value::Bool(true)).unwrap();
        assert_eq!(config.ttl, 0);
        assert_eq!(
            config.keys,
            vec![CacheKey::Method, CacheKey::Url, CacheKey::Body]
        );
    }

    #[test]
    fn parse_cache_false() {
        assert!(parse_cache(&Value::Bool(false)).is_none());
    }

    #[test]
    fn parse_cache_object_partial() {
        let val: Value = serde_json::json!({"ttl": 3600, "keys": ["u", "b"]});
        let config = parse_cache(&val).unwrap();
        assert_eq!(config.ttl, 3600);
        assert_eq!(config.keys, vec![CacheKey::Url, CacheKey::Body]);
        // `at` should be default
        assert!(!config.at.is_empty());
    }

    #[test]
    fn parse_cache_object_full() {
        let val: Value = serde_json::json!({"ttl": 60, "keys": ["m", "u", "b", "a", "h", "h.x-api-key"], "at": "/tmp/yurl-cache"});
        let config = parse_cache(&val).unwrap();
        assert_eq!(config.ttl, 60);
        assert_eq!(config.at, "/tmp/yurl-cache");
        assert_eq!(
            config.keys,
            vec![
                CacheKey::Method,
                CacheKey::Url,
                CacheKey::Body,
                CacheKey::Auth,
                CacheKey::AllHeaders,
                CacheKey::Header("x-api-key".to_string()),
            ]
        );
    }

    #[test]
    fn cache_store_put_get() {
        let dir = tempfile::tempdir().unwrap();
        let store = CacheStore::open(dir.path().to_str().unwrap());
        let resp = CachedResponse {
            status: 200,
            headers: {
                let mut m = Map::new();
                m.insert(
                    "content-type".to_string(),
                    Value::String("application/json".to_string()),
                );
                m
            },
            body: b"{\"ok\":true}".to_vec(),
        };
        store.put("key1", "https://example.com", &resp, 0);
        let got = store.get("key1").unwrap();
        assert_eq!(got.status, 200);
        assert_eq!(got.body, b"{\"ok\":true}");
        assert_eq!(
            got.headers["content-type"],
            Value::String("application/json".to_string())
        );
    }

    #[test]
    fn cache_store_miss() {
        let dir = tempfile::tempdir().unwrap();
        let store = CacheStore::open(dir.path().to_str().unwrap());
        assert!(store.get("nonexistent").is_none());
    }

    #[test]
    fn cache_store_expired() {
        let dir = tempfile::tempdir().unwrap();
        let store = CacheStore::open(dir.path().to_str().unwrap());
        let resp = CachedResponse {
            status: 200,
            headers: Map::new(),
            body: b"data".to_vec(),
        };
        // expires = 1 (in the past)
        let now = unix_now();
        let key = "expired_key";
        store.conn.execute(
            "INSERT INTO cache (key, url, status, headers, body, size, created, expires)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![key, "https://example.com", 200u32, "{}", resp.body, 4i64, now as i64, 1i64],
        ).unwrap();
        assert!(store.get(key).is_none());
    }

    #[test]
    fn compute_key_deterministic() {
        let config = CacheConfig::default();
        let headers = Map::new();
        let body = Some(serde_json::json!({"prompt": "hello"}));
        let k1 = compute_cache_key(&config, "POST", "https://api.openai.com/v1/chat", &body, &headers);
        let k2 = compute_cache_key(&config, "POST", "https://api.openai.com/v1/chat", &body, &headers);
        assert_eq!(k1, k2);
    }

    #[test]
    fn compute_key_differs_by_body() {
        let config = CacheConfig::default();
        let headers = Map::new();
        let b1 = Some(serde_json::json!({"prompt": "hello"}));
        let b2 = Some(serde_json::json!({"prompt": "world"}));
        let k1 = compute_cache_key(&config, "POST", "https://example.com", &b1, &headers);
        let k2 = compute_cache_key(&config, "POST", "https://example.com", &b2, &headers);
        assert_ne!(k1, k2);
    }

    #[test]
    fn cache_stores_reuses_instance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let stores = CacheStores::new();
        let s1 = stores.get(path);
        let s2 = stores.get(path);
        assert!(Arc::ptr_eq(&s1, &s2));
    }
}
