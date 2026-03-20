use console::style;
use rustyline::Editor;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::{Validator, ValidationResult, ValidationContext};
use rustyline::completion::{Completer, Pair};
use rustyline::Helper;
use rustyline::Context;
use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};

struct YurlHelper {
    step_mode: bool,
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
            let hint = if self.step_mode {
                "type request or .next — .help for more"
            } else {
                "type request — .help for more"
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

fn help_text(history_path: &Option<String>, step_mode: bool) -> String {
    let ctrl_d = style("Ctrl-D").bold();
    let history_line = history_path
        .as_deref()
        .map(|p| {
            let display = if let Ok(home) = std::env::var("HOME") {
                p.replace(&home, "~")
            } else {
                p.to_string()
            };
            format!("History    :  {display}\n")
        })
        .unwrap_or_default();
    let step_hint = if step_mode {
        let next = style(".next").bold();
        let go = style(".go").bold();
        format!("  {next}    load next piped request\n  {go}      run all remaining\n")
    } else {
        String::new()
    };
    let expand = style(".x").bold();
    format!("\n\
Single line:  {{g: https://httpbin.org/get}}\n\
Multi-line :  type YAML, then --- to send.\n\
  {expand}      expand request with config, review before sending\n\
{step_hint}\
{history_line}\
  {ctrl_d} to exit.\n")
}

/// Read requests interactively. Calls `on_request` for each complete request string.
/// `on_expand` resolves a request with full config (API aliases, headers, shortcuts).
/// If `step_queue` is Some, enables .next and .go commands for stepping through piped requests.
pub fn run<F, G>(mut on_request: F, on_expand: G, step_queue: Option<VecDeque<String>>)
where
    F: FnMut(String),
    G: Fn(String) -> String,
{
    let history_path = dirs_hint();
    let mut queue = step_queue.unwrap_or_default();
    let step_mode = !queue.is_empty();

    let mut rl = Editor::new().expect("failed to initialize editor");
    rl.set_helper(Some(YurlHelper { step_mode }));

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

    loop {
        match rl.readline(PROMPT) {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                // Handle .help command
                if trimmed == ".help" || trimmed == ".h" {
                    eprint!("{}", help_text(&history_path, step_mode));
                    continue;
                }

                // Handle .next command
                if trimmed == ".next" || trimmed == ".n" {
                    if let Some(req) = queue.pop_front() {
                        match rl.readline_with_initial(PROMPT, (&req, "")) {
                            Ok(edited) => {
                                let edited = edited.trim().to_string();
                                if !edited.is_empty() {
                                    rl.add_history_entry(&edited).ok();
                                    on_request(edited);
                                }
                            }
                            Err(rustyline::error::ReadlineError::Interrupted) => {
                                eprintln!("  (skipped)");
                            }
                            Err(_) => break,
                        }
                    } else {
                        eprintln!("  no more requests");
                    }
                    continue;
                }

                // Handle .x command — expand request with config
                if let Some(req) = trimmed.strip_prefix(".x ") {
                    let req = req.trim();
                    if req.is_empty() {
                        eprintln!("  usage: .x {{request}}");
                        continue;
                    }
                    let expanded = on_expand(req.to_string());
                    match rl.readline_with_initial(PROMPT, (&expanded, "")) {
                        Ok(edited) => {
                            let edited = edited.trim().to_string();
                            if !edited.is_empty() {
                                rl.add_history_entry(&edited).ok();
                                on_request(edited);
                            }
                        }
                        Err(rustyline::error::ReadlineError::Interrupted) => {
                            eprintln!("  (skipped)");
                        }
                        Err(_) => break,
                    }
                    continue;
                }

                // Handle .go command
                if trimmed == ".go" || trimmed == ".g" {
                    if queue.is_empty() {
                        eprintln!("  no more requests");
                        continue;
                    }
                    INTERRUPTED_FLAG.store(false, Ordering::Relaxed);
                    let prev_handler = unsafe {
                        libc::signal(libc::SIGINT, sigint_flag as libc::sighandler_t)
                    };

                    let mut executed = 0;
                    while let Some(req) = queue.pop_front() {
                        if INTERRUPTED_FLAG.load(Ordering::Relaxed) {
                            queue.push_front(req);
                            eprintln!("  interrupted ({} remaining)", queue.len());
                            break;
                        }
                        on_request(req);
                        executed += 1;
                    }

                    unsafe { libc::signal(libc::SIGINT, prev_handler); }

                    if !INTERRUPTED_FLAG.load(Ordering::Relaxed) && executed > 0 {
                        eprintln!("  {executed} requests executed");
                    }
                    continue;
                }

                // Single-line flow: starts with { → execute immediately
                if trimmed.starts_with('{') {
                    rl.add_history_entry(&line).ok();
                    on_request(line);
                    continue;
                }

                // Multi-line YAML: accumulate until ---
                let mut buf = line.clone();
                buf.push('\n');

                loop {
                    match rl.readline(CONTINUATION) {
                        Ok(cont) => {
                            let cont_trimmed = cont.trim();
                            if cont_trimmed == "---" || cont_trimmed.is_empty() && buf.trim().len() > 0 {
                                break;
                            }
                            buf.push_str(&cont);
                            buf.push('\n');
                        }
                        Err(_) => break,
                    }
                }

                let final_request = buf.trim().to_string();
                if !final_request.is_empty() {
                    rl.add_history_entry(&final_request.replace('\n', " ")).ok();
                    on_request(final_request);
                }
            }
            Err(rustyline::error::ReadlineError::Eof) => break,
            Err(rustyline::error::ReadlineError::Interrupted) => {
                eprintln!("  (press {} to exit)", style("Ctrl-D").bold());
                continue;
            }
            Err(e) => {
                eprintln!("error: {e}");
                break;
            }
        }
    }

    if let Some(ref path) = history_path {
        let _ = rl.save_history(path);
    }
}

// --- Signal handling for .go Ctrl-C ---

static INTERRUPTED_FLAG: AtomicBool = AtomicBool::new(false);

extern "C" fn sigint_flag(_sig: libc::c_int) {
    INTERRUPTED_FLAG.store(true, Ordering::Relaxed);
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
