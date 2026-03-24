use arc_swap::ArcSwap;
use std::io::BufReader;
use std::sync::Arc;

use crate::config::Config;
use crate::error::RequestError;
use crate::{StdinReader, expand_with_flags, ExpandFlags};
use super::help;

/// A lazy source of documents for .next/.go.
/// Returns `None` when exhausted, `Some(Ok(json))` for valid documents,
/// `Some(Err(e))` for parse errors.
pub type StdinSource = Box<dyn FnMut() -> Option<Result<String, RequestError>>>;


/// Input events from the user.
#[derive(Debug, Clone)]
pub enum Input {
    /// User typed text and pressed Enter.
    Text(String),
    /// Arrow up — navigate to previous history entry.
    Up,
    /// Arrow down — navigate to next history entry.
    Down,
    /// Ctrl-C — interrupt / skip.
    CtrlC,
    /// Ctrl-D — exit.
    CtrlD,
}

/// Effects produced by the driver for the shell to apply.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Send a request to be executed.
    Execute(String),
    /// Pre-fill the next prompt with this text.
    Prefill(String),
    /// Print a message to stderr.
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
        }
    }

    /// Process an input event and return effects to apply.
    pub fn handle_input(&mut self, input: Input) -> Vec<Effect> {
        // Multi-line YAML accumulation mode
        if self.yaml_buf.is_some() {
            return self.handle_yaml_continuation(input);
        }

        match input {
            Input::CtrlD => vec![Effect::Exit],
            Input::CtrlC => {
                // Clear any pending prefill (e.g. from .x expansion)
                self.prefill = None;
                vec![]
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
        }
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
        // Otherwise execute the edited text
        vec![Effect::Execute(trimmed.to_string())]
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

        // .next
        if trimmed == ".next" || trimmed == ".n" {
            return self.handle_next();
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
            return match yttp::parse(cfg_str) {
                Ok(val) => {
                    let new_config = Config::parse(&val);
                    let summary = format!("  config: {}", new_config.summary());
                    self.config.store(Arc::new(new_config));
                    vec![Effect::Print(summary)]
                }
                Err(e) => {
                    vec![Effect::Print(format!("  error: {e}"))]
                }
            };
        }

        // .step — load requests from file
        if trimmed == ".step" || trimmed == ".s" {
            if self.stdin_source.is_some() {
                return vec![Effect::Print("  source already loaded — use .next or .go".to_string())];
            }
            return vec![Effect::Print("  usage: .step file.yaml".to_string())];
        }
        if let Some(path) = trimmed.strip_prefix(".step ").or_else(|| trimmed.strip_prefix(".s ")) {
            let path = path.trim();
            return self.handle_step(path);
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
        vec![Effect::Execute(trimmed.to_string())]
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
                    vec![Effect::Execute(final_request)]
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

    fn handle_next(&mut self) -> Vec<Effect> {
        let source = match &mut self.stdin_source {
            Some(s) => s,
            None => return vec![Effect::Print("  no more requests".to_string())],
        };
        match source() {
            Some(Ok(req)) => {
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
                    effects.push(Effect::Execute(req));
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

    fn handle_step(&mut self, path: &str) -> Vec<Effect> {
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => return vec![Effect::Print(format!("  error: {e}: {path}"))],
        };
        let reader = BufReader::new(file);
        let mut stdin_reader = StdinReader::new(reader);
        self.stdin_source = Some(Box::new(move || stdin_reader.next()));
        vec![Effect::Print(format!("  loaded {path}"))]
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

    #[derive(serde::Deserialize)]
    struct InteractiveSpec {
        name: String,
        #[serde(default)]
        config: Option<String>,
        #[serde(default)]
        source: Vec<serde_json::Value>,
        /// Mock responses: when Effect::Execute is produced, the next mock response
        /// is used as the OutputResult. Maps request substring to (stdout, stderr).
        #[serde(default)]
        mock_responses: std::collections::HashMap<String, MockResponse>,
        interaction: Vec<Interaction>,
    }

    #[derive(serde::Deserialize, Clone)]
    struct MockResponse {
        #[serde(default)]
        stdout: String,
        #[serde(default)]
        stderr: String,
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
        no_effect: Option<bool>,
        /// Display assertion: checks the OutputResult from mock executor.
        /// Asserts on the stdout portion of the response display.
        #[serde(default, deserialize_with = "deser_effect_assert")]
        display: Option<EffectAssert>,
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

    fn build_driver_from_spec(spec: &InteractiveSpec) -> Driver {
        let config = if let Some(ref cfg_str) = spec.config {
            match crate::parse_input(cfg_str) {
                Ok(json) => Config::parse(&json),
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
                    ("execute", Effect::Execute(s)) => s == exact,
                    _ => false,
                });
                assert!(found, "{ctx}: expected {kind}({exact:?}), got {effects:?}");
            }
            EffectAssert::Contains(sub) => {
                let found = effects.iter().any(|e| match (kind, e) {
                    ("prefill", Effect::Prefill(s)) => s.contains(sub.as_str()),
                    ("print", Effect::Print(s)) => s.contains(sub.as_str()),
                    ("execute", Effect::Execute(s)) => s.contains(sub.as_str()),
                    _ => false,
                });
                assert!(found, "{ctx}: expected {kind} containing {sub:?}, got {effects:?}");
            }
            EffectAssert::NotContains(sub) => {
                let found = effects.iter().any(|e| match (kind, e) {
                    ("prefill", Effect::Prefill(s)) => s.contains(sub.as_str()),
                    ("print", Effect::Print(s)) => s.contains(sub.as_str()),
                    ("execute", Effect::Execute(s)) => s.contains(sub.as_str()),
                    _ => false,
                });
                assert!(!found, "{ctx}: expected {kind} NOT containing {sub:?}, got {effects:?}");
            }
            EffectAssert::Null => {
                let found = effects.iter().any(|e| match (kind, e) {
                    ("prefill", Effect::Prefill(_)) => true,
                    ("print", Effect::Print(_)) => true,
                    ("execute", Effect::Execute(_)) => true,
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

        if assert.exit == Some(true) {
            assert!(
                effects.iter().any(|e| matches!(e, Effect::Exit)),
                "{ctx}: expected Exit, got {effects:?}"
            );
        }
    }

    /// Check display assertion against mock response output.
    fn assert_display(ctx: &str, assertion: &EffectAssert, output: &crate::OutputResult) {
        let combined = format!("{}{}", output.stdout, output.stderr);
        match assertion {
            EffectAssert::Exact(exact) => {
                assert_eq!(combined.trim(), exact.as_str(),
                    "{ctx}: display expected {exact:?}, got {combined:?}");
            }
            EffectAssert::Contains(sub) => {
                assert!(combined.contains(sub.as_str()),
                    "{ctx}: display should contain {sub:?}, got {combined:?}");
            }
            EffectAssert::NotContains(sub) => {
                assert!(!combined.contains(sub.as_str()),
                    "{ctx}: display should NOT contain {sub:?}, got {combined:?}");
            }
            EffectAssert::Null => {
                assert!(combined.trim().is_empty(),
                    "{ctx}: display should be empty, got {combined:?}");
            }
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
            let mut driver = build_driver_from_spec(spec);
            for (i, step) in spec.interaction.iter().enumerate() {
                let input = parse_user_input(&step.user);
                let effects = driver.handle_input(input);
                assert_effects(&spec.name, i, &step.user, &effects, &step.assertions);

                // Check display assertion against mock responses
                if let Some(ref display_assert) = step.assertions.display {
                    // Find the Execute effect and look up mock response
                    let exec_line = effects.iter().find_map(|e| match e {
                        Effect::Execute(s) => Some(s.clone()),
                        _ => None,
                    });
                    if let Some(ref line) = exec_line {
                        // Find matching mock response
                        let mock = spec.mock_responses.iter()
                            .find(|(key, _)| line.contains(key.as_str()))
                            .map(|(_, v)| v);
                        if let Some(resp) = mock {
                            let result = crate::OutputResult {
                                stdout: resp.stdout.clone(),
                                stderr: resp.stderr.clone(),
                            };
                            assert_display(
                                &format!("[{}] step {i} (user: {:?})", spec.name, step.user),
                                display_assert,
                                &result,
                            );
                        } else {
                            panic!("[{}] step {i}: display assertion but no mock_response matching {line:?}", spec.name);
                        }
                    } else {
                        panic!("[{}] step {i}: display assertion but no Execute effect", spec.name);
                    }
                }
            }
        }
    }
}
