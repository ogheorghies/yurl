use serde_json::{Map, Value};
use std::borrow::Cow;

#[derive(Clone, PartialEq)]
pub enum Atom {
    B,      // o.b — response (output) body
    H,      // o.h — response (output) headers
    S,      // o.s — response (output) status as raw string
    SInline, // s! — status as inline object {version, code, text}
    M,      // request method
    U,      // request URL
    UScheme, UHost, UPort, UPath, UQuery, UFragment, // u.* URL parts
    Ib,     // i.b — request (input) body
    Ih,     // i.h — request (input) headers
    SCode,  // o.s.code / s.code
    SText,  // o.s.text / s.text
    SVersion, // o.s.version / s.version
    Idx,
    Md,
    MdField(String),
}

pub enum Format {
    Raw(Atom),
    Json(Vec<Atom>),
    Yaml(Vec<Atom>),
}

pub struct UrlParts {
    pub scheme: String,
    pub host: String,
    pub port: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

pub struct StatusParts {
    pub code: String,
    pub text: String,
    pub version: String,
}

pub struct ResponseData {
    pub status_line: String,
    pub status_parts: StatusParts,
    pub headers_raw: String,
    pub headers_json: Map<String, Value>,
    pub body_bytes: Vec<u8>,
}

pub struct RequestData {
    pub method: String,
    pub url: String,
    pub url_parts: UrlParts,
    pub headers_raw: String,
    pub headers_json: Map<String, Value>,
    pub body_json: Option<Value>,
    pub idx: usize,
    pub md: Option<Value>,
}

pub fn parse_atom(s: &str) -> Option<Atom> {
    match s {
        "b" | "o.b" => Some(Atom::B),
        "h" | "o.h" => Some(Atom::H),
        "s" | "o.s" => Some(Atom::S),
        "s!" => Some(Atom::SInline),
        "s.code" | "s.c" | "o.s.code" => Some(Atom::SCode),
        "s.text" | "s.t" | "o.s.text" => Some(Atom::SText),
        "s.version" | "s.v" | "o.s.version" => Some(Atom::SVersion),
        "m" => Some(Atom::M),
        "u" => Some(Atom::U),
        "u.scheme" => Some(Atom::UScheme),
        "u.host" => Some(Atom::UHost),
        "u.port" => Some(Atom::UPort),
        "u.path" => Some(Atom::UPath),
        "u.query" => Some(Atom::UQuery),
        "u.fragment" => Some(Atom::UFragment),
        "i.b" => Some(Atom::Ib),
        "i.h" => Some(Atom::Ih),
        "idx" => Some(Atom::Idx),
        "md" => Some(Atom::Md),
        _ if s.starts_with("md.") => Some(Atom::MdField(s[3..].to_string())),
        _ => None,
    }
}

fn parse_atom_list(inner: &str) -> Result<Vec<Atom>, String> {
    inner
        .split_whitespace()
        .filter(|a| !a.is_empty())
        .map(|a| parse_atom(a).ok_or_else(|| format!("unknown atom: {a}")))
        .collect()
}

pub fn parse_format(s: &str) -> Result<Format, String> {
    let s = s.trim();
    if s.starts_with("j(") && s.ends_with(')') {
        Ok(Format::Json(parse_atom_list(&s[2..s.len() - 1])?))
    } else if s.starts_with("y(") && s.ends_with(')') {
        Ok(Format::Yaml(parse_atom_list(&s[2..s.len() - 1])?))
    } else if s.starts_with("j(") || s.starts_with("y(") {
        Err(format!("unclosed format: {s} (missing closing parenthesis)"))
    } else {
        Ok(Format::Raw(parse_atom(s).ok_or_else(|| format!("unknown format: {s}"))?))
    }
}

fn md_to_string(md: &Option<Value>) -> String {
    match md {
        Some(v) => match v {
            Value::String(s) => s.clone(),
            _ => serde_json::to_string(v).unwrap(),
        },
        None => String::new(),
    }
}

fn md_field_to_string(md: &Option<Value>, field: &str) -> String {
    match md {
        Some(Value::Object(obj)) => match obj.get(field) {
            Some(Value::String(s)) => s.clone(),
            Some(v) => serde_json::to_string(v).unwrap(),
            None => String::new(),
        },
        _ => String::new(),
    }
}

fn atom_raw<'a>(atom: &Atom, resp: &'a ResponseData, req: &'a RequestData) -> Cow<'a, [u8]> {
    match atom {
        Atom::B => Cow::Borrowed(&resp.body_bytes),
        Atom::H => Cow::Borrowed(resp.headers_raw.as_bytes()),
        Atom::S | Atom::SInline => Cow::Borrowed(resp.status_line.as_bytes()),
        Atom::SCode => Cow::Borrowed(resp.status_parts.code.as_bytes()),
        Atom::SText => Cow::Borrowed(resp.status_parts.text.as_bytes()),
        Atom::SVersion => Cow::Borrowed(resp.status_parts.version.as_bytes()),
        Atom::M => Cow::Borrowed(req.method.as_bytes()),
        Atom::U => Cow::Borrowed(req.url.as_bytes()),
        Atom::UScheme => Cow::Borrowed(req.url_parts.scheme.as_bytes()),
        Atom::UHost => Cow::Borrowed(req.url_parts.host.as_bytes()),
        Atom::UPort => Cow::Borrowed(req.url_parts.port.as_bytes()),
        Atom::UPath => Cow::Borrowed(req.url_parts.path.as_bytes()),
        Atom::UQuery => Cow::Borrowed(req.url_parts.query.as_bytes()),
        Atom::UFragment => Cow::Borrowed(req.url_parts.fragment.as_bytes()),
        Atom::Ib => req
            .body_json
            .as_ref()
            .map(|v| Cow::Owned(serde_json::to_string(v).unwrap().into_bytes()))
            .unwrap_or(Cow::Borrowed(b"")),
        Atom::Ih => Cow::Borrowed(req.headers_raw.as_bytes()),
        Atom::Idx => Cow::Owned(req.idx.to_string().into_bytes()),
        Atom::Md => Cow::Owned(md_to_string(&req.md).into_bytes()),
        Atom::MdField(field) => Cow::Owned(md_field_to_string(&req.md, field).into_bytes()),
    }
}


pub fn atom_json_value(atom: &Atom, resp: &ResponseData, req: &RequestData) -> Value {
    match atom {
        Atom::B => yttp::encode_body(&resp.body_bytes),
        Atom::H => Value::Object(resp.headers_json.clone()),
        Atom::S => Value::String(resp.status_line.clone()),
        Atom::SInline => {
            let mut m = Map::new();
            m.insert("v".to_string(), Value::String(resp.status_parts.version.clone()));
            m.insert("c".to_string(), Value::Number(resp.status_parts.code.parse::<u16>().unwrap().into()));
            m.insert("t".to_string(), Value::String(resp.status_parts.text.clone()));
            Value::Object(m)
        }
        Atom::SCode => Value::Number(resp.status_parts.code.parse::<u16>().unwrap().into()),
        Atom::SText => Value::String(resp.status_parts.text.clone()),
        Atom::SVersion => Value::String(resp.status_parts.version.clone()),
        Atom::M => Value::String(req.method.clone()),
        Atom::U => Value::String(req.url.clone()),
        Atom::UScheme => Value::String(req.url_parts.scheme.clone()),
        Atom::UHost => Value::String(req.url_parts.host.clone()),
        Atom::UPort => Value::String(req.url_parts.port.clone()),
        Atom::UPath => Value::String(req.url_parts.path.clone()),
        Atom::UQuery => Value::String(req.url_parts.query.clone()),
        Atom::UFragment => Value::String(req.url_parts.fragment.clone()),
        Atom::Ib => req
            .body_json
            .clone()
            .unwrap_or(Value::Null),
        Atom::Ih => Value::Object(req.headers_json.clone()),
        Atom::Idx => Value::Number(req.idx.into()),
        Atom::Md => req.md.clone().unwrap_or(Value::Null),
        Atom::MdField(field) => req
            .md
            .as_ref()
            .and_then(|v| v.as_object())
            .and_then(|obj| obj.get(field))
            .cloned()
            .unwrap_or(Value::Null),
    }
}

fn build_value_map(atoms: &[Atom], resp: &ResponseData, req: &RequestData) -> Value {
    let mut map = Map::new();
    let mut md_fields: Map<String, Value> = Map::new();
    let mut s_fields: Map<String, Value> = Map::new();
    let mut u_fields: Map<String, Value> = Map::new();
    for atom in atoms {
        match atom {
            Atom::MdField(field) => {
                md_fields.insert(field.clone(), atom_json_value(atom, resp, req));
            }
            Atom::SCode => {
                s_fields.insert("c".to_string(), atom_json_value(atom, resp, req));
            }
            Atom::SText => {
                s_fields.insert("t".to_string(), atom_json_value(atom, resp, req));
            }
            Atom::SVersion => {
                s_fields.insert("v".to_string(), atom_json_value(atom, resp, req));
            }
            Atom::UScheme => {
                u_fields.insert("scheme".to_string(), atom_json_value(atom, resp, req));
            }
            Atom::UHost => {
                u_fields.insert("host".to_string(), atom_json_value(atom, resp, req));
            }
            Atom::UPort => {
                u_fields.insert("port".to_string(), atom_json_value(atom, resp, req));
            }
            Atom::UPath => {
                u_fields.insert("path".to_string(), atom_json_value(atom, resp, req));
            }
            Atom::UQuery => {
                u_fields.insert("query".to_string(), atom_json_value(atom, resp, req));
            }
            Atom::UFragment => {
                u_fields.insert("fragment".to_string(), atom_json_value(atom, resp, req));
            }
            _ => {
                let key = match atom {
                    Atom::B => "b",
                    Atom::H => "h",
                    Atom::S | Atom::SInline => "s",
                    Atom::M => "m",
                    Atom::U => "u",
                    Atom::Ib => "i.b",
                    Atom::Ih => "i.h",
                    Atom::Idx => "idx",
                    Atom::Md => "md",
                    _ => unreachable!(),
                };
                map.insert(key.to_string(), atom_json_value(atom, resp, req));
            }
        }
    }
    if !md_fields.is_empty() {
        map.insert("md".to_string(), Value::Object(md_fields));
    }
    if !s_fields.is_empty() {
        map.insert("s".to_string(), Value::Object(s_fields));
    }
    if !u_fields.is_empty() {
        map.insert("u".to_string(), Value::Object(u_fields));
    }
    Value::Object(map)
}

fn inline_keys(atoms: &[Atom]) -> Vec<&'static str> {
    let mut keys = Vec::new();
    if atoms.iter().any(|a| matches!(a, Atom::SInline)) {
        keys.push("s");
    }
    keys
}

pub fn render<'a>(fmt: &Format, resp: &'a ResponseData, req: &'a RequestData) -> Cow<'a, [u8]> {
    render_color(fmt, resp, req, false)
}

pub fn render_color<'a>(fmt: &Format, resp: &'a ResponseData, req: &'a RequestData, color: bool) -> Cow<'a, [u8]> {
    match fmt {
        Format::Raw(atom) => atom_raw(atom, resp, req),
        Format::Json(atoms) => {
            let val = build_value_map(atoms, resp, req);
            let keys = inline_keys(atoms);
            let out = crate::format_json::to_pretty_json(&val, &keys, color);
            Cow::Owned(out.into_bytes())
        }
        Format::Yaml(atoms) => {
            let val = build_value_map(atoms, resp, req);
            let keys = inline_keys(atoms);
            let out = crate::format_yaml::to_yaml(&val, &keys, color);
            Cow::Owned(out.into_bytes())
        }
    }
}
