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
        if let Some(effects) = self.try_expand(trimmed) {
            return effects;
        }
        // Otherwise execute the edited text
        vec![Effect::Execute(trimmed.to_string())]
    }

    fn handle_command(&mut self, trimmed: &str, raw: &str) -> Vec<Effect> {
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
        if let Some(effects) = self.try_expand(trimmed) {
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
        if trimmed.starts_with('{') {
            self.add_history(raw);
            return vec![
                Effect::AddHistory(raw.to_string()),
                Effect::Execute(raw.to_string()),
            ];
        }

        // Multi-line YAML: start accumulating
        let mut buf = raw.to_string();
        buf.push('\n');
        self.yaml_buf = Some(buf);
        vec![] // wait for more input
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
                    let history_entry = final_request.replace('\n', " ");
                    self.add_history(&history_entry);
                    vec![
                        Effect::AddHistory(history_entry),
                        Effect::Execute(final_request),
                    ]
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
    ///
    /// Flags: m (merged), v (vertical), j (JSON), c (curl), s (short headers).
    /// If output is horizontal (editable), pre-fills prompt. Otherwise prints.
    fn try_expand(&mut self, input: &str) -> Option<Vec<Effect>> {
        let rest = input.strip_prefix(".x ")?;
        let rest = rest.trim();
        if rest.is_empty() { return Some(vec![]); }

        // Parse optional flags: first token if it's all valid flag chars and doesn't start with {
        let (flags, req) = parse_flags_and_req(rest);

        let flags = match flags {
            Ok(f) => f,
            Err(e) => return Some(vec![Effect::Print(format!("  {e}"))]),
        };

        if req.is_empty() { return Some(vec![]); }

        let result = expand_with_flags(req, &self.config.load(), &flags);

        self.add_history(input);
        let mut effects = vec![Effect::AddHistory(input.to_string())];

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

    fn empty_driver() -> Driver {
        let config = Arc::new(ArcSwap::from_pointee(Config::empty()));
        Driver::new(config, None, None)
    }

    fn step_driver(items: Vec<Result<&str, &str>>) -> Driver {
        let config = Arc::new(ArcSwap::from_pointee(Config::empty()));
        let mut queue: VecDeque<Result<String, RequestError>> = items
            .into_iter()
            .map(|r| match r {
                Ok(s) => Ok(s.to_string()),
                Err(msg) => Err(RequestError::Parse {
                    input: msg.to_string(),
                    line: None,
                    column: None,
                    msg: msg.to_string(),
                }),
            })
            .collect();
        let source: StdinSource = Box::new(move || queue.pop_front().map(|r| r));
        Driver::new(config, Some(source), None)
    }

    fn step_driver_ok(items: Vec<&str>) -> Driver {
        step_driver(items.into_iter().map(Ok).collect())
    }

    fn has_effect(effects: &[Effect], pred: impl Fn(&Effect) -> bool) -> bool {
        effects.iter().any(pred)
    }

    // --- Basic commands ---

    #[test]
    fn direct_request_executes_and_records_history() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text("{g: example.com}".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Execute(s) if s.contains("example.com"))));
        assert!(has_effect(&effects, |e| matches!(e, Effect::AddHistory(s) if s.contains("example.com"))));
    }

    #[test]
    fn help_command() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(".help".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains(".x"))));
    }

    #[test]
    fn help_shortcut() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(".h".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(_))));
    }

    #[test]
    fn ref_command() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(".ref".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("Request"))));
    }

    #[test]
    fn ref_shortcut() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(".r".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("Request"))));
    }

    #[test]
    fn templates_command() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(".t".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("GET"))));
    }

    #[test]
    fn config_show() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(".c".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("config:"))));
    }

    #[test]
    fn config_replace() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(r#".c {api: example.com}"#.into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("api:"))));
    }

    #[test]
    fn config_replace_invalid() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(".c {broken".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("error"))));
    }

    // --- Expand with flags ---

    #[test]
    fn expand_default_horizontal_yaml() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(r#".x {g: example.com}"#.into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("get:"))));
        assert!(has_effect(&effects, |e| matches!(e, Effect::AddHistory(s) if s.starts_with(".x"))));
        // History should NOT contain the expanded text
        assert!(!has_effect(&effects, |e| matches!(e, Effect::AddHistory(s) if s.contains("get:"))));
    }

    #[test]
    fn expand_vertical_prints() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(r#".x v {g: example.com}"#.into()));
        // Vertical → prints, no prefill
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("get:"))));
        assert!(!has_effect(&effects, |e| matches!(e, Effect::Prefill(_))));
        assert!(has_effect(&effects, |e| matches!(e, Effect::AddHistory(s) if s.starts_with(".x v"))));
    }

    #[test]
    fn expand_json_horizontal() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(r#".x j {g: example.com}"#.into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("\"get\""))));
    }

    #[test]
    fn expand_json_vertical() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(r#".x jv {g: example.com}"#.into()));
        // Pretty-printed JSON → prints (vertical)
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("\"get\""))));
    }

    #[test]
    fn expand_curl_horizontal() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(r#".x c {g: example.com}"#.into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("curl"))));
        assert!(!has_effect(&effects, |e| matches!(e, Effect::Prefill(_))));
    }

    #[test]
    fn expand_curl_vertical() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(r#".x vc {g: example.com, h: {a!: tok}}"#.into()));
        let effects_has_backslash = has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("\\")));
        assert!(effects_has_backslash, "vertical curl should have backslash continuations");
    }

    #[test]
    fn expand_merged() {
        let config_json = yttp::parse(r#"{h: {X-From-Config: "yes"}}"#).unwrap();
        let config = Arc::new(ArcSwap::from_pointee(Config::parse(&config_json)));
        let mut d = Driver::new(config, None, None);
        let effects = d.handle_input(Input::Text(r#".x m {g: example.com}"#.into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("X-From-Config"))));
    }

    #[test]
    fn expand_unmerged_default() {
        let config_json = yttp::parse(r#"{h: {X-From-Config: "yes"}}"#).unwrap();
        let config = Arc::new(ArcSwap::from_pointee(Config::parse(&config_json)));
        let mut d = Driver::new(config, None, None);
        // Default (no m flag) should NOT include config headers
        let effects = d.handle_input(Input::Text(r#".x {g: example.com}"#.into()));
        assert!(!has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("X-From-Config"))));
    }

    #[test]
    fn expand_flags_order_independent() {
        let mut d = empty_driver();
        let effects1 = d.handle_input(Input::Text(r#".x vm {g: example.com}"#.into()));
        let mut d = empty_driver();
        let effects2 = d.handle_input(Input::Text(r#".x mv {g: example.com}"#.into()));
        // Both should produce Print (vertical) not Prefill
        assert!(has_effect(&effects1, |e| matches!(e, Effect::Print(_))));
        assert!(has_effect(&effects2, |e| matches!(e, Effect::Print(_))));
    }

    #[test]
    fn expand_invalid_flag() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(r#".x z {g: example.com}"#.into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("unknown flag"))));
    }

    #[test]
    fn expand_mutually_exclusive_flags() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(r#".x jc {g: example.com}"#.into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("mutually exclusive"))));
    }

    #[test]
    fn expand_error_reprompts_with_original() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(".x {broken".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(_))));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains(".x {broken"))));
    }

    #[test]
    fn expand_then_prepend_x_re_expands() {
        let mut d = empty_driver();
        d.handle_input(Input::Text(r#".x {g: example.com}"#.into()));
        assert!(d.prefill.is_some());

        let prefilled = d.prefill.clone().unwrap();
        let edited = format!(".x v {}", &prefilled);
        let effects = d.handle_input(Input::Text(edited));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(_))));
    }

    #[test]
    fn prefill_edit_without_dot_command_executes() {
        let mut d = empty_driver();
        d.prefill = Some("{get: https://example.com}".into());
        let effects = d.handle_input(Input::Text("{get: https://example.com}".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Execute(s) if s.contains("example.com"))));
    }

    #[test]
    fn p_is_unknown_command() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(r#".p {g: example.com}"#.into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("unknown command"))));
    }

    #[test]
    fn xx_is_unknown_command() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(r#".xx {g: example.com}"#.into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("unknown command"))));
    }

    #[test]
    fn expand_short_headers() {
        let config_json = yttp::parse(r#"{h: {Authorization: "Bearer tok"}}"#).unwrap();
        let config = Arc::new(ArcSwap::from_pointee(Config::parse(&config_json)));
        let mut d = Driver::new(config, None, None);
        let effects = d.handle_input(Input::Text(r#".x ms {g: example.com}"#.into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("a!") && s.contains("bearer!"))));
    }

    // --- Unknown commands ---

    #[test]
    fn unknown_dot_command() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(".xj {g: example.com}".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("unknown command"))));
    }

    #[test]
    fn unknown_dot_command_no_args() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(".foo".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("unknown command: .foo"))));
    }

    // --- Step mode ---

    #[test]
    fn next_reads_from_source() {
        let mut d = step_driver_ok(vec!["{g: example.com}", "{g: other.com}"]);
        let effects = d.handle_input(Input::Text(".next".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("example.com"))));
        // Ctrl-C to clear prefill, then second .next should get the second item
        d.handle_input(Input::CtrlC);
        let effects = d.handle_input(Input::Text(".next".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("other.com"))));
    }

    #[test]
    fn next_shortcut() {
        let mut d = step_driver_ok(vec!["{g: example.com}"]);
        let effects = d.handle_input(Input::Text(".n".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("example.com"))));
    }

    #[test]
    fn next_empty_queue() {
        let mut d = step_driver_ok(vec![]);
        let effects = d.handle_input(Input::Text(".next".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("no more"))));
    }

    #[test]
    fn go_executes_all() {
        let mut d = step_driver_ok(vec!["{g: a.com}", "{g: b.com}", "{g: c.com}"]);
        let effects = d.handle_input(Input::Text(".go".into()));
        let exec_count = effects.iter().filter(|e| matches!(e, Effect::Execute(_))).count();
        assert_eq!(exec_count, 3);
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("3 requests"))));
    }

    #[test]
    fn go_shortcut() {
        let mut d = step_driver_ok(vec!["{g: a.com}"]);
        let effects = d.handle_input(Input::Text(".g".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Execute(_))));
    }

    #[test]
    fn go_empty_queue() {
        let mut d = step_driver_ok(vec![]);
        let effects = d.handle_input(Input::Text(".go".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("no more"))));
    }

    #[test]
    fn next_shows_error_from_source() {
        let mut d = step_driver(vec![Err("bad yaml")]);
        let effects = d.handle_input(Input::Text(".next".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("bad yaml"))));
        // Should not prefill
        assert!(!has_effect(&effects, |e| matches!(e, Effect::Prefill(_))));
    }

    #[test]
    fn go_stops_on_error() {
        let mut d = step_driver(vec![Ok("{g: a.com}"), Err("bad yaml"), Ok("{g: b.com}")]);
        let effects = d.handle_input(Input::Text(".go".into()));
        // Should execute the first, print error, not execute the third
        let exec_count = effects.iter().filter(|e| matches!(e, Effect::Execute(_))).count();
        assert_eq!(exec_count, 1);
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("bad yaml"))));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("1 requests"))));
    }

    #[test]
    fn next_in_non_step_mode() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(".next".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("no more"))));
    }

    // --- .step command ---

    #[test]
    fn step_loads_file() {
        let mut d = empty_driver();
        // Write a temp file with a JSONL request
        let dir = std::env::temp_dir();
        let path = dir.join("yurl_test_step.jsonl");
        std::fs::write(&path, r#"{"g": "http://example.com"}"#).unwrap();
        let effects = d.handle_input(Input::Text(format!(".step {}", path.display())));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("loaded"))));
        assert!(d.stdin_source.is_some());
        // .next should read from the file
        let effects = d.handle_input(Input::Text(".next".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("example.com"))));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn step_missing_file_error() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(".step /nonexistent/file.yaml".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("error"))));
    }

    #[test]
    fn step_no_args_no_source() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(".step".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("usage"))));
    }

    #[test]
    fn step_no_args_with_source() {
        let mut d = step_driver_ok(vec!["{g: example.com}"]);
        let effects = d.handle_input(Input::Text(".step".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("already loaded"))));
    }

    #[test]
    fn step_shortcut() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(".s /nonexistent/file.yaml".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("error"))));
    }

    // --- History navigation ---

    #[test]
    fn up_navigates_history() {
        let mut d = empty_driver();
        d.handle_input(Input::Text("{g: first.com}".into()));
        d.handle_input(Input::Text("{g: second.com}".into()));

        let effects = d.handle_input(Input::Up);
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("second.com"))));

        let effects = d.handle_input(Input::Up);
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("first.com"))));
    }

    #[test]
    fn down_navigates_forward() {
        let mut d = empty_driver();
        d.handle_input(Input::Text("{g: first.com}".into()));
        d.handle_input(Input::Text("{g: second.com}".into()));

        // Go up twice
        d.handle_input(Input::Up);
        d.handle_input(Input::Up);

        // Go down
        let effects = d.handle_input(Input::Down);
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("second.com"))));

        // Down past end — empty
        let effects = d.handle_input(Input::Down);
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.is_empty())));
    }

    #[test]
    fn up_on_empty_history() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Up);
        assert!(effects.is_empty());
    }

    #[test]
    fn text_input_resets_history_cursor() {
        let mut d = empty_driver();
        d.handle_input(Input::Text("{g: first.com}".into()));
        d.handle_input(Input::Text("{g: second.com}".into()));

        // Navigate up
        d.handle_input(Input::Up);
        assert_eq!(d.history_cursor, 1);

        // New text input resets cursor
        d.handle_input(Input::Text("{g: third.com}".into()));
        assert_eq!(d.history_cursor, d.history.len());
    }

    #[test]
    fn x_command_in_history_not_expanded() {
        let mut d = empty_driver();
        d.handle_input(Input::Text(".x {g: example.com}".into()));

        // Press up — should get .x command, not expanded text
        let effects = d.handle_input(Input::Up);
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.starts_with(".x {g:"))));
    }

    #[test]
    fn x_flags_in_history() {
        let mut d = empty_driver();
        d.handle_input(Input::Text(r#".x mv {g: example.com}"#.into()));

        let effects = d.handle_input(Input::Up);
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.starts_with(".x mv"))));
    }

    // --- Control keys ---

    #[test]
    fn ctrl_d_exits() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::CtrlD);
        assert_eq!(effects, vec![Effect::Exit]);
    }

    #[test]
    fn ctrl_c_clears_line() {
        let mut d = empty_driver();
        // Set up a prefill (as if .x just expanded)
        d.prefill = Some("{get: https://example.com}".into());
        let effects = d.handle_input(Input::CtrlC);
        assert!(effects.is_empty());
        assert!(d.prefill.is_none(), "prefill should be cleared");
    }

    #[test]
    fn empty_input_ignored() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text("".into()));
        assert!(effects.is_empty());
    }

    #[test]
    fn whitespace_only_ignored() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text("   ".into()));
        assert!(effects.is_empty());
    }

    // --- Multi-line YAML ---

    #[test]
    fn yaml_multiline_accumulates() {
        let mut d = empty_driver();
        // First line — not { so enters YAML mode
        let effects = d.handle_input(Input::Text("g: example.com".into()));
        assert!(effects.is_empty());
        assert!(d.in_yaml_mode());

        // Second line
        let effects = d.handle_input(Input::Text("h:".into()));
        assert!(effects.is_empty());

        // Empty line terminates
        let effects = d.handle_input(Input::Text("".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Execute(_))));
        assert!(!d.in_yaml_mode());
    }

    #[test]
    fn yaml_terminated_by_separator() {
        let mut d = empty_driver();
        d.handle_input(Input::Text("g: example.com".into()));
        let effects = d.handle_input(Input::Text("---".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Execute(s) if s.contains("example.com"))));
    }

    #[test]
    fn yaml_aborted_by_ctrl_c() {
        let mut d = empty_driver();
        d.handle_input(Input::Text("g: example.com".into()));
        assert!(d.in_yaml_mode());
        let effects = d.handle_input(Input::CtrlC);
        assert!(effects.is_empty());
        assert!(!d.in_yaml_mode());
    }
}
