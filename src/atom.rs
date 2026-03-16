use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Map, Value};
use std::borrow::Cow;

#[derive(Clone, PartialEq)]
pub enum Atom {
    B,
    H,
    S,
    M,
    U,
    Ab,
    Ah,
    As,
    SCode,
    SText,
    SVersion,
    Idx,
    Md,
    MdField(String),
}

pub enum Format {
    Raw(Atom),
    Json(Vec<Atom>),
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
    pub status_line: String,
    pub headers_raw: String,
    pub headers_json: Map<String, Value>,
    pub body_json: Option<Value>,
    pub idx: usize,
    pub md: Option<Value>,
}

pub fn parse_atom(s: &str) -> Option<Atom> {
    match s {
        "b" => Some(Atom::B),
        "h" => Some(Atom::H),
        "s" => Some(Atom::S),
        "s.code" => Some(Atom::SCode),
        "s.text" => Some(Atom::SText),
        "s.version" => Some(Atom::SVersion),
        "m" => Some(Atom::M),
        "u" => Some(Atom::U),
        "ab" => Some(Atom::Ab),
        "ah" => Some(Atom::Ah),
        "as" => Some(Atom::As),
        "idx" => Some(Atom::Idx),
        "md" => Some(Atom::Md),
        _ if s.starts_with("md.") => Some(Atom::MdField(s[3..].to_string())),
        _ => None,
    }
}

pub fn parse_format(s: &str) -> Format {
    let s = s.trim();
    if s.starts_with("j(") && s.ends_with(')') {
        let inner = &s[2..s.len() - 1];
        let atoms: Vec<Atom> = inner
            .split(',')
            .map(|a| parse_atom(a.trim()).unwrap_or_else(|| panic!("unknown atom: {}", a.trim())))
            .collect();
        Format::Json(atoms)
    } else {
        Format::Raw(parse_atom(s).unwrap_or_else(|| panic!("unknown format: {s}")))
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
        Atom::S => Cow::Borrowed(resp.status_line.as_bytes()),
        Atom::SCode => Cow::Borrowed(resp.status_parts.code.as_bytes()),
        Atom::SText => Cow::Borrowed(resp.status_parts.text.as_bytes()),
        Atom::SVersion => Cow::Borrowed(resp.status_parts.version.as_bytes()),
        Atom::M => Cow::Borrowed(req.method.as_bytes()),
        Atom::U => Cow::Borrowed(req.url.as_bytes()),
        Atom::Ab => req
            .body_json
            .as_ref()
            .map(|v| Cow::Owned(serde_json::to_string(v).unwrap().into_bytes()))
            .unwrap_or(Cow::Borrowed(b"")),
        Atom::Ah => Cow::Borrowed(req.headers_raw.as_bytes()),
        Atom::As => Cow::Borrowed(req.status_line.as_bytes()),
        Atom::Idx => Cow::Owned(req.idx.to_string().into_bytes()),
        Atom::Md => Cow::Owned(md_to_string(&req.md).into_bytes()),
        Atom::MdField(field) => Cow::Owned(md_field_to_string(&req.md, field).into_bytes()),
    }
}

fn atom_json_value(atom: &Atom, resp: &ResponseData, req: &RequestData) -> Value {
    match atom {
        Atom::B => Value::String(STANDARD.encode(&resp.body_bytes)),
        Atom::H => Value::Object(resp.headers_json.clone()),
        Atom::S => Value::String(resp.status_line.clone()),
        Atom::SCode => Value::Number(resp.status_parts.code.parse::<u16>().unwrap().into()),
        Atom::SText => Value::String(resp.status_parts.text.clone()),
        Atom::SVersion => Value::String(resp.status_parts.version.clone()),
        Atom::M => Value::String(req.method.clone()),
        Atom::U => Value::String(req.url.clone()),
        Atom::Ab => req
            .body_json
            .as_ref()
            .map(|v| Value::String(STANDARD.encode(serde_json::to_string(v).unwrap().as_bytes())))
            .unwrap_or(Value::Null),
        Atom::Ah => Value::Object(req.headers_json.clone()),
        Atom::As => Value::String(req.status_line.clone()),
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

pub fn render<'a>(fmt: &Format, resp: &'a ResponseData, req: &'a RequestData) -> Cow<'a, [u8]> {
    match fmt {
        Format::Raw(atom) => atom_raw(atom, resp, req),
        Format::Json(atoms) => {
            let mut map = Map::new();
            let mut md_fields: Map<String, Value> = Map::new();
            let mut s_fields: Map<String, Value> = Map::new();
            for atom in atoms {
                match atom {
                    Atom::MdField(field) => {
                        md_fields.insert(field.clone(), atom_json_value(atom, resp, req));
                    }
                    Atom::SCode => {
                        s_fields.insert("code".to_string(), atom_json_value(atom, resp, req));
                    }
                    Atom::SText => {
                        s_fields.insert("text".to_string(), atom_json_value(atom, resp, req));
                    }
                    Atom::SVersion => {
                        s_fields.insert("version".to_string(), atom_json_value(atom, resp, req));
                    }
                    _ => {
                        let key = match atom {
                            Atom::B => "b",
                            Atom::H => "h",
                            Atom::S => "s",
                            Atom::M => "m",
                            Atom::U => "u",
                            Atom::Ab => "ab",
                            Atom::Ah => "ah",
                            Atom::As => "as",
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
            let mut out = serde_json::to_string_pretty(&Value::Object(map)).unwrap();
            out.push('\n');
            Cow::Owned(out.into_bytes())
        }
    }
}
