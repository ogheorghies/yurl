mod driver;
mod help;

pub use help::reference_card;
pub use driver::{Driver, Input, Effect, StdinSource};

use arc_swap::ArcSwap;
use console::style;
use rustyline::Editor;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::{Validator, ValidationResult, ValidationContext};
use rustyline::completion::{Completer, Pair};
use rustyline::Helper;
use rustyline::Context;
use std::borrow::Cow;
use std::sync::Arc;

use crate::config::Config;

struct YurlHelper {
    has_source: bool,
}

impl Helper for YurlHelper {}

impl Completer for YurlHelper {
    type Candidate = Pair;
    fn complete(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> rustyline::Result<(usize, Vec<Pair>)> {
        Ok((0, vec![]))
    }
}

impl Hinter for YurlHelper {
    type Hint = String;
    fn hint(&self, line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<String> {
        if line.is_empty() {
            let hint = if self.has_source {
                "type .next — .help, .t, or .ref"
            } else {
                "type request — .help, .t, or .ref"
            };
            Some(format!("\x1b[2m{hint}\x1b[0m"))
        } else {
            None
        }
    }
}

impl Validator for YurlHelper {
    fn validate(&self, _ctx: &mut ValidationContext) -> rustyline::Result<ValidationResult> {
        Ok(ValidationResult::Valid(None))
    }
}

impl Highlighter for YurlHelper {
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(&'s self, prompt: &'p str, _default: bool) -> Cow<'b, str> {
        Cow::Owned(format!("\x1b[36m{prompt}\x1b[0m"))
    }

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Borrowed(hint)
    }
}

const PROMPT: &str = "> ";
const CONTINUATION: &str = "… ";
const EXAMPLE: &str = "{g: https://httpbin.org/get}";

/// Read requests interactively. Calls `on_request` for each complete request string.
/// `config` is shared via ArcSwap — `.x` reads it, `.c` replaces it.
/// If `stdin_source` is Some, enables .next and .go commands for stepping through piped requests.
pub fn run<F>(mut on_request: F, config: &Arc<ArcSwap<Config>>, stdin_source: Option<StdinSource>)
where
    F: FnMut(String),
{
    let history_path = dirs_hint();
    let has_source = stdin_source.is_some();

    let rl_config = rustyline::Config::builder()
        .behavior(rustyline::config::Behavior::PreferTerm)
        .build();
    let mut rl = Editor::with_config(rl_config).expect("failed to initialize editor");
    rl.set_helper(Some(YurlHelper { has_source }));

    let mut has_history = false;
    if let Some(ref path) = history_path {
        if rl.load_history(path).is_ok() {
            has_history = true;
        }
    }

    if !has_history {
        rl.add_history_entry(EXAMPLE).ok();
    }

    let yurl = style("yurl").bold().cyan();
    let version = env!("CARGO_PKG_VERSION");
    eprintln!("{yurl} v{version}\n");

    let mut driver = Driver::new(Arc::clone(config), stdin_source, history_path.clone());

    #[cfg(debug_assertions)]
    let exe_mtime_at_start = exe_mtime();

    loop {
        // Determine prompt: if driver has a prefill, use readline_with_initial
        let prompt_str = if driver.in_yaml_mode() { CONTINUATION } else { PROMPT };

        let input = if let Some(prefill) = driver.pending_prefill().map(|s| s.to_string()) {
            match rl.readline_with_initial(PROMPT, (&prefill, "")) {
                Ok(line) => Input::Text(line),
                Err(rustyline::error::ReadlineError::Interrupted) => Input::CtrlC,
                Err(rustyline::error::ReadlineError::Eof) => Input::CtrlD,
                Err(_) => Input::CtrlD,
            }
        } else {
            match rl.readline(prompt_str) {
                Ok(line) => Input::Text(line),
                Err(rustyline::error::ReadlineError::Interrupted) => Input::CtrlC,
                Err(rustyline::error::ReadlineError::Eof) => Input::CtrlD,
                Err(_) => Input::CtrlD,
            }
        };

        let effects = driver.handle_input(input);

        for effect in effects {
            match effect {
                Effect::Execute(req) => on_request(req),
                Effect::Prefill(_) => {
                    // Prefill is consumed by the driver state — the next loop
                    // iteration will use pending_prefill() to show it
                }
                Effect::Print(msg) => {
                    if msg.ends_with('\n') {
                        eprint!("{msg}");
                    } else {
                        eprintln!("{msg}");
                    }
                }
                Effect::AddHistory(entry) => { rl.add_history_entry(&entry).ok(); }
                Effect::Exit => {
                    if let Some(ref path) = history_path {
                        let _ = rl.save_history(path);
                    }
                    return;
                }
            }
        }

        #[cfg(debug_assertions)]
        if let Some(start) = exe_mtime_at_start {
            if let Some(current) = exe_mtime() {
                if current > start {
                    eprintln!("  {} binary has been rebuilt — restart for latest changes",
                        style("warning:").yellow().bold());
                }
            }
        }
    }
}

#[cfg(debug_assertions)]
fn exe_mtime() -> Option<std::time::SystemTime> {
    let exe = std::env::current_exe().ok()?;
    std::fs::metadata(&exe).ok()?.modified().ok()
}

fn dirs_hint() -> Option<String> {
    let data_dir = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{home}/.local/share"))
            .unwrap_or_default()
    });
    if data_dir.is_empty() {
        return None;
    }
    let dir = format!("{data_dir}/yurl");
    std::fs::create_dir_all(&dir).ok()?;
    Some(format!("{dir}/history"))
}
