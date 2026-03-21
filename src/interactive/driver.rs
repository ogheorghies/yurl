use arc_swap::ArcSwap;
use std::collections::VecDeque;
use std::sync::Arc;

use crate::config::Config;
use crate::{expand_request, expand_request_structured};
use super::help;

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

/// REPL driver — processes Input, produces Effects.
/// Contains all command routing, history, and expansion logic.
/// No readline dependency — fully testable.
pub struct Driver {
    config: Arc<ArcSwap<Config>>,
    history: Vec<String>,
    history_cursor: usize,
    queue: VecDeque<String>,
    step_mode: bool,
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
        queue: VecDeque<String>,
        history_path: Option<String>,
    ) -> Self {
        let step_mode = !queue.is_empty();
        Driver {
            config,
            history: Vec::new(),
            history_cursor: 0,
            queue,
            step_mode,
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
        // Check if user prepended .x or .xx
        if let Some(effects) = self.try_expand(trimmed) {
            return effects;
        }
        // Otherwise execute the edited text
        vec![Effect::Execute(trimmed.to_string())]
    }

    fn handle_command(&mut self, trimmed: &str, raw: &str) -> Vec<Effect> {
        // .help
        if trimmed == ".help" || trimmed == ".h" {
            return vec![Effect::Print(help::help_text(&self.history_path, self.step_mode))];
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

        // .xx / .x — expand
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

    fn try_expand(&mut self, input: &str) -> Option<Vec<Effect>> {
        // .xx must be checked before .x
        let result = if let Some(req) = input.strip_prefix(".xx ") {
            let req = req.trim();
            if req.is_empty() { return Some(vec![]); }
            expand_request_structured(req, &self.config.load())
        } else if let Some(req) = input.strip_prefix(".x ") {
            let req = req.trim();
            if req.is_empty() { return Some(vec![]); }
            expand_request(req, &self.config.load())
        } else {
            return None;
        };

        // Record the .x/.xx command to history
        self.add_history(input);
        let mut effects = vec![Effect::AddHistory(input.to_string())];

        match result {
            Ok(expanded) => {
                self.prefill = Some(expanded.clone());
                effects.push(Effect::Prefill(expanded));
            }
            Err(e) => {
                effects.push(Effect::Print(e.display_colored()));
                // Re-prompt with original input so user can fix
                self.prefill = Some(input.to_string());
                effects.push(Effect::Prefill(input.to_string()));
            }
        }
        Some(effects)
    }

    fn handle_next(&mut self) -> Vec<Effect> {
        if let Some(req) = self.queue.pop_front() {
            self.prefill = Some(req.clone());
            vec![Effect::Prefill(req)]
        } else {
            vec![Effect::Print("  no more requests".to_string())]
        }
    }

    fn handle_go(&mut self) -> Vec<Effect> {
        if self.queue.is_empty() {
            return vec![Effect::Print("  no more requests".to_string())];
        }
        let mut effects = Vec::new();
        let mut count = 0;
        while let Some(req) = self.queue.pop_front() {
            effects.push(Effect::Execute(req));
            count += 1;
        }
        effects.push(Effect::Print(format!("  {count} requests executed")));
        effects
    }

    fn add_history(&mut self, entry: &str) {
        self.history.push(entry.to_string());
        self.history_cursor = self.history.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_driver() -> Driver {
        let config = Arc::new(ArcSwap::from_pointee(Config::empty()));
        Driver::new(config, VecDeque::new(), None)
    }

    fn step_driver(queue: Vec<&str>) -> Driver {
        let config = Arc::new(ArcSwap::from_pointee(Config::empty()));
        Driver::new(
            config,
            queue.into_iter().map(String::from).collect(),
            None,
        )
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

    // --- Expand commands ---

    #[test]
    fn expand_x_produces_prefill_and_history() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(r#".x {g: example.com}"#.into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("get:"))));
        assert!(has_effect(&effects, |e| matches!(e, Effect::AddHistory(s) if s.starts_with(".x"))));
        // History should NOT contain the expanded text
        assert!(!has_effect(&effects, |e| matches!(e, Effect::AddHistory(s) if s.contains("get:"))));
    }

    #[test]
    fn expand_xx_produces_structured_prefill() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(r#".xx {g: "example.com?a=1"}"#.into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("q:"))));
        assert!(has_effect(&effects, |e| matches!(e, Effect::AddHistory(s) if s.starts_with(".xx"))));
    }

    #[test]
    fn expand_error_reprompts_with_original() {
        let mut d = empty_driver();
        let effects = d.handle_input(Input::Text(".x {broken".into()));
        // Should have error print and prefill with original
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(_))));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains(".x {broken"))));
    }

    #[test]
    fn expand_then_edit_with_xx_re_expands() {
        let mut d = empty_driver();
        // First: .x expands and sets prefill
        let effects = d.handle_input(Input::Text(r#".x {g: example.com}"#.into()));
        assert!(d.prefill.is_some());

        // Simulate user prepending .xx to the pre-filled text
        let prefilled = d.prefill.clone().unwrap();
        let edited = format!(".xx {}", &prefilled);
        let effects = d.handle_input(Input::Text(edited));
        // Should produce a new Prefill with structured expansion
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(_))));
        assert!(has_effect(&effects, |e| matches!(e, Effect::AddHistory(s) if s.starts_with(".xx"))));
    }

    #[test]
    fn prefill_edit_without_dot_command_executes() {
        let mut d = empty_driver();
        // Set up a prefill (as if .x just expanded)
        d.prefill = Some("{get: https://example.com}".into());
        let effects = d.handle_input(Input::Text("{get: https://example.com}".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Execute(s) if s.contains("example.com"))));
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
    fn next_pops_from_queue() {
        let mut d = step_driver(vec!["{g: example.com}", "{g: other.com}"]);
        let effects = d.handle_input(Input::Text(".next".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("example.com"))));
        assert_eq!(d.queue.len(), 1);
    }

    #[test]
    fn next_shortcut() {
        let mut d = step_driver(vec!["{g: example.com}"]);
        let effects = d.handle_input(Input::Text(".n".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Prefill(s) if s.contains("example.com"))));
    }

    #[test]
    fn next_empty_queue() {
        let mut d = step_driver(vec![]);
        let effects = d.handle_input(Input::Text(".next".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("no more"))));
    }

    #[test]
    fn go_executes_all() {
        let mut d = step_driver(vec!["{g: a.com}", "{g: b.com}", "{g: c.com}"]);
        let effects = d.handle_input(Input::Text(".go".into()));
        let exec_count = effects.iter().filter(|e| matches!(e, Effect::Execute(_))).count();
        assert_eq!(exec_count, 3);
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("3 requests"))));
        assert!(d.queue.is_empty());
    }

    #[test]
    fn go_shortcut() {
        let mut d = step_driver(vec!["{g: a.com}"]);
        let effects = d.handle_input(Input::Text(".g".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Execute(_))));
    }

    #[test]
    fn go_empty_queue() {
        let mut d = step_driver(vec![]);
        let effects = d.handle_input(Input::Text(".go".into()));
        assert!(has_effect(&effects, |e| matches!(e, Effect::Print(s) if s.contains("no more"))));
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
