use arc_swap::ArcSwap;
use std::io::BufReader;
use std::sync::Arc;

use crate::config::Config;
use crate::error::RequestError;
use crate::OutputResult;
use crate::{StdinReader, expand_with_flags, ExpandFlags};
use super::help;

/// A lazy source of documents for .pop/.go.
/// Returns `None` when exhausted, `Some(Ok(json))` for valid documents,
/// `Some(Err(e))` for parse errors.
pub type StdinSource = Box<dyn FnMut() -> Option<Result<String, RequestError>>>;


/// Input events — from the user or the system.
#[derive(Debug, Clone)]
#[allow(dead_code)]  // Up/Down constructed in test specs
pub enum Input {
    /// User typed text and pressed Enter.
    Text(String),
    /// Arrow up — navigate to previous history entry.
    Up,
    /// Arrow down — navigate to next history entry.
    Down,
    /// Ctrl-C — interrupt / cancel.
    CtrlC,
    /// Ctrl-D — exit.
    CtrlD,
    /// A request completed — the system delivers the result.
    RequestComplete { id: usize, result: OutputResult },
}

/// Effects produced by the driver for the shell to apply.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Send a request to be executed.
    Execute { id: usize, line: String },
    /// Cancel the in-flight request.
    Cancel { id: usize },
    /// Display response output on stdout.
    Stdout(String),
    /// Display response output on stderr.
    Stderr(String),
    /// Pre-fill the next prompt with this text.
    Prefill(String),
    /// Print a message to stderr (REPL messages, not response output).
    Print(String),
    /// Record an entry in readline history.
    AddHistory(String),
    /// Exit the REPL.
    Exit,
}

/// Parse flags and request from the text after `.x `.
/// The first token is flags if it's short, all alphabetic, doesn't start
/// with `{`, and is followed by a space + more text.
fn parse_flags_and_req(rest: &str) -> (Result<ExpandFlags, String>, &str) {
    if let Some(pos) = rest.find(' ') {
        let maybe_flags = &rest[..pos];
        // If it looks like a flag token: short, all alpha, not starting with {
        if !maybe_flags.starts_with('{')
            && !maybe_flags.is_empty()
            && maybe_flags.len() <= 5
            && maybe_flags.chars().all(|c| c.is_ascii_alphabetic())
        {
            let req = rest[pos..].trim();
            return (ExpandFlags::parse(maybe_flags), req);
        }
    }
    // No flags — whole thing is the request
    (Ok(ExpandFlags::default()), rest)
}

/// REPL driver — processes Input, produces Effects.
/// Contains all command routing, history, and expansion logic.
/// No readline dependency — fully testable.
pub struct Driver {
    config: Arc<ArcSwap<Config>>,
    history: Vec<String>,
    history_cursor: usize,
    stdin_source: Option<StdinSource>,
    /// When Some, the next Text input is an edit of this pre-filled prompt.
    prefill: Option<String>,
    /// Path to history file (for .help display).
    history_path: Option<String>,
    /// Whether the REPL is in the middle of accumulating multi-line YAML.
    yaml_buf: Option<String>,
    /// ID of the currently in-flight request, if any.
    request_in_flight: Option<usize>,
    /// Counter for assigning request IDs.
    next_request_id: usize,
    /// Last popped request — for .repop.
    last_popped: Option<String>,
}

impl Driver {
    pub fn new(
        config: Arc<ArcSwap<Config>>,
        stdin_source: Option<StdinSource>,
        history_path: Option<String>,
    ) -> Self {
        Driver {
            config,
            history: Vec::new(),
            history_cursor: 0,
            stdin_source,
            prefill: None,
            history_path,
            yaml_buf: None,
            request_in_flight: None,
            next_request_id: 0,
            last_popped: None,
        }
    }

    /// Whether a request is currently in flight.
    pub fn is_request_in_flight(&self) -> bool {
        self.request_in_flight.is_some()
    }

    /// The ID of the currently in-flight request.
    pub fn in_flight_id(&self) -> Option<usize> {
        self.request_in_flight
    }

    /// Create an Execute effect and track it as in-flight.
    fn make_execute(&mut self, line: String) -> Effect {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.request_in_flight = Some(id);
        Effect::Execute { id, line }
    }

    /// Process an input event and return effects to apply.
    pub fn handle_input(&mut self, input: Input) -> Vec<Effect> {
        // Request completion — always handle regardless of other state
        if let Input::RequestComplete { id, result } = input {
            return self.handle_request_complete(id, result);
        }

        // Ctrl-C during in-flight request → cancel
        if let Input::CtrlC = &input {
            if let Some(id) = self.request_in_flight {
                self.request_in_flight = None;
                return vec![Effect::Cancel { id }];
            }
        }

        // Multi-line YAML accumulation mode
        if self.yaml_buf.is_some() {
            return self.handle_yaml_continuation(input);
        }

        match input {
            Input::CtrlD => vec![Effect::Exit],
            Input::CtrlC => {
                // No request in flight — clear any pending prefill, otherwise hint exit
                if self.prefill.take().is_some() {
                    vec![]
                } else {
                    vec![Effect::Print("  (press Ctrl-D to exit)".to_string())]
                }
            }
            Input::Up => self.handle_up(),
            Input::Down => self.handle_down(),
            Input::Text(text) => {
                let trimmed = text.trim().to_string();
                if trimmed.is_empty() {
                    return vec![];
                }

                // If there's a pending prefill, the text is an edit of that prefill
                if self.prefill.take().is_some() {
                    self.history_cursor = self.history.len();
                    return self.handle_prefill_edit(&trimmed);
                }

                self.history_cursor = self.history.len();
                self.handle_command(&trimmed, &text)
            }
            Input::RequestComplete { .. } => unreachable!(), // handled above
        }
    }

    fn handle_request_complete(&mut self, id: usize, result: OutputResult) -> Vec<Effect> {
        // Only process if this matches the in-flight request
        if self.request_in_flight != Some(id) {
            return vec![];
        }
        self.request_in_flight = None;
        let mut effects = vec![];
        if !result.stdout.is_empty() {
            effects.push(Effect::Stdout(result.stdout));
        }
        if !result.stderr.is_empty() {
            effects.push(Effect::Stderr(result.stderr));
        }
        effects
    }

    /// Whether the driver has a pending pre-fill for the next prompt.
    pub fn pending_prefill(&self) -> Option<&str> {
        self.prefill.as_deref()
    }

    /// Whether the driver is accumulating multi-line YAML.
    pub fn in_yaml_mode(&self) -> bool {
        self.yaml_buf.is_some()
    }

    fn handle_up(&mut self) -> Vec<Effect> {
        if self.history.is_empty() {
            return vec![];
        }
        if self.history_cursor > 0 {
            self.history_cursor -= 1;
        }
        let entry = self.history[self.history_cursor].clone();
        self.prefill = Some(entry.clone());
        vec![Effect::Prefill(entry)]
    }

    fn handle_down(&mut self) -> Vec<Effect> {
        if self.history_cursor >= self.history.len() {
            return vec![];
        }
        self.history_cursor += 1;
        if self.history_cursor < self.history.len() {
            let entry = self.history[self.history_cursor].clone();
            self.prefill = Some(entry.clone());
            vec![Effect::Prefill(entry)]
        } else {
            // Past the end — clear prompt
            self.prefill = None;
            vec![Effect::Prefill(String::new())]
        }
    }

    fn handle_prefill_edit(&mut self, trimmed: &str) -> Vec<Effect> {
        // Check if user prepended .x
        if let Some(effects) = self.try_expand_command(trimmed) {
            return effects;
        }
        // Otherwise execute the edited text and record it in history
        self.add_history(trimmed);
        vec![
            Effect::AddHistory(trimmed.to_string()),
            self.make_execute(trimmed.to_string()),
        ]
    }

    fn handle_command(&mut self, trimmed: &str, raw: &str) -> Vec<Effect> {
        // Multi-line YAML: start accumulating
        if !trimmed.starts_with('.') && !trimmed.starts_with('{') {
            self.add_history(raw);
            let mut buf = raw.to_string();
            buf.push('\n');
            self.yaml_buf = Some(buf);
            return vec![Effect::AddHistory(raw.to_string())];
        }

        // All other input goes to history immediately
        self.add_history(raw);
        let mut effects = vec![Effect::AddHistory(raw.to_string())];

        let command_effects = self.dispatch_command(trimmed, raw);
        effects.extend(command_effects);
        effects
    }

    fn dispatch_command(&mut self, trimmed: &str, _raw: &str) -> Vec<Effect> {
        // .help / .help x
        if trimmed == ".help" || trimmed == ".h" {
            return vec![Effect::Print(help::help_text(&self.history_path))];
        }
        if trimmed == ".help x" || trimmed == ".h x" {
            return vec![Effect::Print(help::expand_help())];
        }

        // .ref
        if trimmed == ".ref" || trimmed == ".r" {
            return vec![Effect::Print(help::reference_card())];
        }

        // .t — templates
        if trimmed == ".t" {
            return vec![Effect::Print(help::TEMPLATES.to_string())];
        }

        // .pop
        if trimmed == ".pop" || trimmed == ".p" {
            return self.handle_pop();
        }

        // .repop
        if trimmed == ".repop" {
            return self.handle_repop();
        }

        // .c — config
        if trimmed == ".c" {
            let cfg = self.config.load();
            return vec![Effect::Print(format!("  config: {}", cfg.summary()))];
        }
        if let Some(cfg_str) = trimmed.strip_prefix(".c ") {
            let cfg_str = cfg_str.trim();
            if cfg_str.is_empty() {
                let cfg = self.config.load();
                return vec![Effect::Print(format!("  config: {}", cfg.summary()))];
            }
            return match yttp::from_str(cfg_str) {
                Ok(val) => match Config::parse(&val) {
                    Ok(new_config) => {
                        let summary = format!("  config: {}", new_config.summary());
                        self.config.store(Arc::new(new_config));
                        vec![Effect::Print(summary)]
                    }
                    Err(e) => vec![Effect::Print(format!("  error: {e}"))],
                },
                Err(e) => {
                    vec![Effect::Print(format!("  error: {e}"))]
                }
            };
        }

        // .open — load requests from file
        if trimmed == ".open" {
            if self.stdin_source.is_some() {
                return vec![Effect::Print("  source already loaded — use .pop or .go".to_string())];
            }
            return vec![Effect::Print("  usage: .open file.yaml".to_string())];
        }
        if let Some(path) = trimmed.strip_prefix(".open ") {
            let path = path.trim();
            return self.handle_open(path);
        }

        // .x — expand with composable flags
        if let Some(effects) = self.try_expand_command(trimmed) {
            return effects;
        }

        // .go
        if trimmed == ".go" || trimmed == ".g" {
            return self.handle_go();
        }

        // Unknown dot-command
        if trimmed.starts_with('.') {
            let cmd = trimmed.split_whitespace().next().unwrap_or(trimmed);
            return vec![Effect::Print(format!("  unknown command: {cmd} (try .help)"))];
        }

        // Single-line flow: starts with {
        vec![self.make_execute(trimmed.to_string())]
    }

    fn handle_yaml_continuation(&mut self, input: Input) -> Vec<Effect> {
        match input {
            Input::Text(text) => {
                let cont_trimmed = text.trim();
                let buf = self.yaml_buf.as_mut().unwrap();
                if cont_trimmed == "---" || (cont_trimmed.is_empty() && !buf.trim().is_empty()) {
                    // End of YAML document
                    let final_request = buf.trim().to_string();
                    self.yaml_buf = None;
                    if final_request.is_empty() {
                        return vec![];
                    }
                    vec![self.make_execute(final_request)]
                } else {
                    buf.push_str(&text);
                    buf.push('\n');
                    vec![] // keep accumulating
                }
            }
            Input::CtrlC | Input::CtrlD => {
                // Abort YAML accumulation
                self.yaml_buf = None;
                vec![]
            }
            _ => vec![],
        }
    }

    /// Parse `.x [flags] {req}` — expand with composable modifier flags.
    /// History is handled by the caller (handle_command).
    fn try_expand_command(&mut self, input: &str) -> Option<Vec<Effect>> {
        let rest = input.strip_prefix(".x ")?;
        let rest = rest.trim();
        if rest.is_empty() { return Some(vec![]); }

        let (flags, req) = parse_flags_and_req(rest);

        let flags = match flags {
            Ok(f) => f,
            Err(e) => return Some(vec![Effect::Print(format!("  {e}"))]),
        };

        if req.is_empty() { return Some(vec![]); }

        let result = expand_with_flags(req, &self.config.load(), &flags);

        let mut effects = vec![];
        match result {
            Ok(output) => {
                if flags.is_editable() {
                    self.prefill = Some(output.clone());
                    effects.push(Effect::Prefill(output));
                } else {
                    effects.push(Effect::Print(output));
                }
            }
            Err(e) => {
                effects.push(Effect::Print(e.display_colored()));
                self.prefill = Some(input.to_string());
                effects.push(Effect::Prefill(input.to_string()));
            }
        }
        Some(effects)
    }

    fn handle_pop(&mut self) -> Vec<Effect> {
        let source = match &mut self.stdin_source {
            Some(s) => s,
            None => return vec![Effect::Print("  no more requests".to_string())],
        };
        match source() {
            Some(Ok(req)) => {
                self.last_popped = Some(req.clone());
                self.prefill = Some(req.clone());
                vec![Effect::Prefill(req)]
            }
            Some(Err(e)) => {
                vec![Effect::Print(e.display_colored())]
            }
            None => {
                vec![Effect::Print("  no more requests".to_string())]
            }
        }
    }

    fn handle_repop(&mut self) -> Vec<Effect> {
        match &self.last_popped {
            Some(req) => {
                let req = req.clone();
                self.prefill = Some(req.clone());
                vec![Effect::Prefill(req)]
            }
            None => {
                vec![Effect::Print("  nothing to repop".to_string())]
            }
        }
    }

    fn handle_go(&mut self) -> Vec<Effect> {
        let source = match &mut self.stdin_source {
            Some(s) => s,
            None => return vec![Effect::Print("  no more requests".to_string())],
        };
        let mut effects = Vec::new();
        let mut count = 0;
        loop {
            match source() {
                Some(Ok(req)) => {
                    let id = self.next_request_id;
                    self.next_request_id += 1;
                    self.request_in_flight = Some(id);
                    effects.push(Effect::Execute { id, line: req });
                    count += 1;
                }
                Some(Err(e)) => {
                    effects.push(Effect::Print(e.display_colored()));
                    break;
                }
                None => break,
            }
        }
        if count > 0 {
            effects.push(Effect::Print(format!("  {count} requests executed")));
        } else if effects.is_empty() {
            effects.push(Effect::Print("  no more requests".to_string()));
        }
        effects
    }

    fn handle_open(&mut self, path: &str) -> Vec<Effect> {
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => return vec![Effect::Print(format!("  error: {e}: {path}"))],
        };
        let reader = BufReader::new(file);
        let mut stdin_reader = StdinReader::new(reader);
        self.stdin_source = Some(Box::new(move || stdin_reader.next()));
        vec![Effect::Print(format!("  opened {path}"))]
    }

    fn add_history(&mut self, entry: &str) {
        self.history.push(entry.to_string());
        self.history_cursor = self.history.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    // ── YAML spec-driven interactive tests ─────────────────────────────
    //
    // Loads tests/specs/interactive.yaml and drives the Driver programmatically.
    // Each spec defines a sequence of user inputs and expected effects.

    static MOCKINX_URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

    fn mockinx_url() -> &'static str {
        MOCKINX_URL.get_or_init(|| {
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

            format!("http://{addr}")
        })
    }

    fn setup_rules(base: &str, rules: &[serde_json::Value]) {
        let client = reqwest::blocking::Client::new();
        let body = serde_json::to_string(rules).unwrap();
        client
            .put(format!("{base}/_mx"))
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .expect("failed to set mockinx rules");
    }

    /// Execute a request line via jurl subprocess and return the output.
    fn execute_request(line: &str, config: Option<&str>) -> crate::OutputResult {
        // Find the jurl binary — in unit tests CARGO_BIN_EXE_ isn't available
        let jurl = std::env::var("CARGO_BIN_EXE_jurl").unwrap_or_else(|_| {
            let mut path = std::env::current_exe().unwrap();
            path.pop(); // remove test binary name
            path.pop(); // remove deps/
            path.push("jurl");
            path.to_string_lossy().to_string()
        });
        let mut cmd = std::process::Command::new(jurl);
        if let Some(cfg) = config {
            cmd.arg(cfg);
        }
        let output = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(line.as_bytes()).unwrap();
                child.wait_with_output()
            })
            .expect("failed to run jurl");
        crate::OutputResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }

    #[derive(serde::Deserialize)]
    struct InteractiveSpec {
        name: String,
        #[serde(default)]
        config: Option<String>,
        #[serde(default)]
        source: Vec<serde_json::Value>,
        /// mockinx rules — when present, Execute effects run against mockinx for real.
        #[serde(default)]
        rules: Vec<serde_json::Value>,
        interaction: Vec<Interaction>,
    }

    #[derive(serde::Deserialize)]
    struct Interaction {
        user: String,
        #[serde(default, rename = "assert")]
        assertions: StepAssert,
    }

    #[derive(serde::Deserialize, Default)]
    struct StepAssert {
        /// Prefill assertion: string (exact), {contains: "sub"}, or null (cleared/absent)
        #[serde(default, deserialize_with = "deser_effect_assert")]
        prefill: Option<EffectAssert>,
        /// Print assertion: string (exact), {contains: "sub"}
        #[serde(default, deserialize_with = "deser_effect_assert")]
        print: Option<EffectAssert>,
        /// Execute assertion: string (exact), {contains: "sub"}
        #[serde(default, deserialize_with = "deser_effect_assert")]
        execute: Option<EffectAssert>,
        #[serde(default)]
        exit: Option<bool>,
        #[serde(default)]
        cancel: Option<bool>,
        #[serde(default)]
        no_effect: Option<bool>,
        /// Stdout assertion: checks Effect::Stdout from RequestComplete.
        #[serde(default, deserialize_with = "deser_effect_assert")]
        stdout: Option<EffectAssert>,
        /// Stderr assertion: checks Effect::Stderr from RequestComplete.
        #[serde(default, deserialize_with = "deser_effect_assert")]
        stderr: Option<EffectAssert>,
    }

    #[derive(Debug)]
    enum EffectAssert {
        Exact(String),
        Contains(String),
        NotContains(String),
        Null, // no such effect expected (e.g. prefill cleared)
    }

    fn deser_effect_assert<'de, D>(deserializer: D) -> Result<Option<EffectAssert>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;

        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = Option<EffectAssert>;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("string, null, or {contains/not_contains: ...}")
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> { Ok(None) }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> { Ok(Some(EffectAssert::Null)) }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(Some(EffectAssert::Exact(v.to_string())))
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut result = None;
                while let Some(key) = map.next_key::<String>()? {
                    let val: String = map.next_value()?;
                    result = Some(match key.as_str() {
                        "contains" => EffectAssert::Contains(val),
                        "not_contains" => EffectAssert::NotContains(val),
                        _ => return Err(de::Error::unknown_field(&key, &["contains", "not_contains"])),
                    });
                }
                Ok(result)
            }
        }

        deserializer.deserialize_any(Visitor)
    }

    fn parse_user_input(s: &str) -> Input {
        match s {
            "ctrl-c" => Input::CtrlC,
            "ctrl-d" => Input::CtrlD,
            "up" => Input::Up,
            "down" => Input::Down,
            text => Input::Text(text.to_string()),
        }
    }

    fn resolve_mx(s: &str) -> String {
        if !s.contains("mx!") { return s.to_string(); }
        let base = mockinx_url();
        let s = s.replace("mx!", &format!("{base}/"));
        s.replace(&format!("{base}//"), &format!("{base}/"))
    }

    fn build_driver_from_spec(spec: &InteractiveSpec) -> Driver {
        let config = if let Some(ref cfg_str) = spec.config {
            let resolved = resolve_mx(cfg_str);
            match crate::parse_input(&resolved) {
                Ok(json) => Config::parse(&json).unwrap_or_else(|_| Config::empty()),
                Err(_) => Config::empty(),
            }
        } else {
            Config::empty()
        };
        let config = Arc::new(ArcSwap::from_pointee(config));

        let source: Option<StdinSource> = if spec.source.is_empty() {
            None
        } else {
            let mut queue: VecDeque<Result<String, RequestError>> = spec
                .source
                .iter()
                .map(|v| {
                    if let Some(ok) = v.get("Ok").and_then(|v| v.as_str()) {
                        Ok(ok.to_string())
                    } else if let Some(err) = v.get("Err").and_then(|v| v.as_str()) {
                        Err(RequestError::Parse {
                            input: err.to_string(),
                            line: None,
                            column: None,
                            msg: err.to_string(),
                        })
                    } else if let Some(s) = v.as_str() {
                        // Plain string = Ok
                        Ok(s.to_string())
                    } else {
                        panic!("invalid source item: {v}");
                    }
                })
                .collect();
            Some(Box::new(move || queue.pop_front()))
        };

        Driver::new(config, source, None)
    }

    fn check_effect_assert(ctx: &str, kind: &str, assertion: &EffectAssert, effects: &[Effect]) {
        match assertion {
            EffectAssert::Exact(exact) => {
                let found = effects.iter().any(|e| match (kind, e) {
                    ("prefill", Effect::Prefill(s)) => s == exact,
                    ("print", Effect::Print(s)) => s.trim() == exact,
                    ("execute", Effect::Execute { line: s, .. }) => s == exact,
                    ("stdout", Effect::Stdout(s)) => s.trim() == exact,
                    ("stderr_out", Effect::Stderr(s)) => s.trim() == exact,
                    _ => false,
                });
                assert!(found, "{ctx}: expected {kind}({exact:?}), got {effects:?}");
            }
            EffectAssert::Contains(sub) => {
                let found = effects.iter().any(|e| match (kind, e) {
                    ("prefill", Effect::Prefill(s)) => s.contains(sub.as_str()),
                    ("print", Effect::Print(s)) => s.contains(sub.as_str()),
                    ("execute", Effect::Execute { line: s, .. }) => s.contains(sub.as_str()),
                    ("stdout", Effect::Stdout(s)) => s.contains(sub.as_str()),
                    ("stderr_out", Effect::Stderr(s)) => s.contains(sub.as_str()),
                    _ => false,
                });
                assert!(found, "{ctx}: expected {kind} containing {sub:?}, got {effects:?}");
            }
            EffectAssert::NotContains(sub) => {
                let found = effects.iter().any(|e| match (kind, e) {
                    ("prefill", Effect::Prefill(s)) => s.contains(sub.as_str()),
                    ("print", Effect::Print(s)) => s.contains(sub.as_str()),
                    ("execute", Effect::Execute { line: s, .. }) => s.contains(sub.as_str()),
                    ("stdout", Effect::Stdout(s)) => s.contains(sub.as_str()),
                    ("stderr_out", Effect::Stderr(s)) => s.contains(sub.as_str()),
                    _ => false,
                });
                assert!(!found, "{ctx}: expected {kind} NOT containing {sub:?}, got {effects:?}");
            }
            EffectAssert::Null => {
                let found = effects.iter().any(|e| match (kind, e) {
                    ("prefill", Effect::Prefill(_)) => true,
                    ("print", Effect::Print(_)) => true,
                    ("execute", Effect::Execute { .. }) => true,
                    ("stdout", Effect::Stdout(_)) => true,
                    ("stderr_out", Effect::Stderr(_)) => true,
                    _ => false,
                });
                assert!(!found, "{ctx}: expected no {kind}, got {effects:?}");
            }
        }
    }

    fn assert_effects(name: &str, step: usize, user: &str, effects: &[Effect], assert: &StepAssert) {
        let ctx = format!("[{name}] step {step} (user: {user:?})");

        if assert.no_effect == Some(true) {
            assert!(effects.is_empty(), "{ctx}: expected no effects, got {effects:?}");
            return;
        }

        if let Some(ref a) = assert.prefill {
            check_effect_assert(&ctx, "prefill", a, effects);
        }

        if let Some(ref a) = assert.print {
            check_effect_assert(&ctx, "print", a, effects);
        }

        if let Some(ref a) = assert.execute {
            check_effect_assert(&ctx, "execute", a, effects);
        }

        if let Some(ref a) = assert.stdout {
            check_effect_assert(&ctx, "stdout", a, effects);
        }

        if let Some(ref a) = assert.stderr {
            check_effect_assert(&ctx, "stderr_out", a, effects);
        }

        if assert.exit == Some(true) {
            assert!(
                effects.iter().any(|e| matches!(e, Effect::Exit)),
                "{ctx}: expected Exit, got {effects:?}"
            );
        }

        if assert.cancel == Some(true) {
            assert!(
                effects.iter().any(|e| matches!(e, Effect::Cancel { .. })),
                "{ctx}: expected Cancel, got {effects:?}"
            );
        }
    }


    /// Find the Execute effect's (id, line) if present.
    fn find_execute(effects: &[Effect]) -> Option<(usize, String)> {
        effects.iter().find_map(|e| match e {
            Effect::Execute { id, line } => Some((*id, line.clone())),
            _ => None,
        })
    }

    /// Execute a request against mockinx and return the response.
    fn get_response(spec: &InteractiveSpec, line: &str) -> crate::OutputResult {
        if !spec.rules.is_empty() {
            let resolved_line = resolve_mx(line);
            let resolved_config = spec.config.as_deref().map(|c| resolve_mx(c));
            execute_request(&resolved_line, resolved_config.as_deref())
        } else {
            // No rules — return empty (pure Driver logic tests)
            crate::OutputResult { stdout: String::new(), stderr: String::new() }
        }
    }

    #[test]
    fn interactive_specs() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/specs/interactive.yaml");
        let content = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
        let specs: Vec<InteractiveSpec> = serde_yml::from_str(&content)
            .unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

        for spec in &specs {
            // Set up mockinx rules if present
            if !spec.rules.is_empty() {
                let base = mockinx_url();
                setup_rules(base, &spec.rules);
            }

            let mut driver = build_driver_from_spec(spec);
            for (i, step) in spec.interaction.iter().enumerate() {
                let input = parse_user_input(&step.user);
                let mut effects = driver.handle_input(input);

                // If Execute was produced, check if we should auto-complete
                // (feed RequestComplete) or leave in-flight (for cancel tests).
                if let Some((id, ref line)) = find_execute(&effects) {
                    let next_is_cancel = spec.interaction.get(i + 1)
                        .is_some_and(|s| s.user == "ctrl-c");

                    if !next_is_cancel {
                        // Auto-complete: execute against mockinx or use mock response
                        let result = get_response(spec, line);
                        let completion_effects = driver.handle_input(
                            Input::RequestComplete { id, result }
                        );
                        effects.extend(completion_effects);
                    }
                }

                assert_effects(&spec.name, i, &step.user, &effects, &step.assertions);
            }
        }
    }
}
