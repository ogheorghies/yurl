use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::borrow::Cow;

use crate::atom::{RequestData, ResponseData};

fn resolve_var<'a>(key: &str, resp: &'a ResponseData, req: &'a RequestData) -> Cow<'a, str> {
    match key {
        "b" => Cow::Owned(STANDARD.encode(&resp.body_bytes)),
        "h" => Cow::Borrowed(&resp.headers_raw),
        "s" => Cow::Borrowed(&resp.status_line),
        "s.code" => Cow::Borrowed(&resp.status_parts.code),
        "s.text" => Cow::Borrowed(&resp.status_parts.text),
        "s.version" => Cow::Borrowed(&resp.status_parts.version),
        "m" => Cow::Borrowed(&req.method),
        "u" => Cow::Borrowed(&req.url),
        "as" => Cow::Borrowed(&req.status_line),
        "ah" => Cow::Borrowed(&req.headers_raw),
        "ab" => req
            .body_json
            .as_ref()
            .map(|v| Cow::Owned(serde_json::to_string(v).unwrap()))
            .unwrap_or(Cow::Borrowed("")),
        "u.scheme" => Cow::Borrowed(&req.url_parts.scheme),
        "u.host" => Cow::Borrowed(&req.url_parts.host),
        "u.port" => Cow::Borrowed(&req.url_parts.port),
        "u.path" => Cow::Borrowed(&req.url_parts.path),
        "u.query" => Cow::Borrowed(&req.url_parts.query),
        "u.fragment" => Cow::Borrowed(&req.url_parts.fragment),
        "idx" => Cow::Owned(req.idx.to_string()),
        "md" => Cow::Owned(match &req.md {
            Some(Value::String(s)) => s.clone(),
            Some(v) => serde_json::to_string(v).unwrap(),
            None => String::new(),
        }),
        _ if key.starts_with("md.") => {
            let field = &key[3..];
            Cow::Owned(match &req.md {
                Some(Value::Object(obj)) => match obj.get(field) {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) => serde_json::to_string(v).unwrap(),
                    None => String::new(),
                },
                _ => String::new(),
            })
        }
        _ => panic!("unknown template variable: {key}"),
    }
}

/// Expand `{{var}}` placeholders in a string.
pub fn expand_path(template: &str, resp: &ResponseData, req: &RequestData) -> String {
    let mut result = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let key = after[..end].trim();
            result.push_str(&resolve_var(key, resp, req));
            rest = &after[end + 2..];
        } else {
            panic!("unclosed {{{{ in template");
        }
    }
    result.push_str(rest);
    result
}
