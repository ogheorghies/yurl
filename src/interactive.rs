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

struct YurlHelper;

impl Helper for YurlHelper {}

impl Completer for YurlHelper {
    type Candidate = Pair;
    fn complete(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> rustyline::Result<(usize, Vec<Pair>)> {
        Ok((0, vec![]))
    }
}

impl Hinter for YurlHelper {
    type Hint = String;
    fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<String> {
        None
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
}

const PROMPT: &str = "> ";
const CONTINUATION: &str = "… ";
const EXAMPLE: &str = "{g: https://httpbin.org/get}";

fn hint(history_path: &Option<String>, has_history: bool, step_mode: bool) -> String {
    let yurl = style("yurl").bold().cyan();
    let ctrl_d = style("Ctrl-D").bold();
    let history_line = history_path
        .as_deref()
        .map(|p| {
            let display = if let Ok(home) = std::env::var("HOME") {
                p.replace(&home, "~")
            } else {
                p.to_string()
            };
            format!("\n  History: {display}\n")
        })
        .unwrap_or_default();
    let tip = if !has_history {
        let arrow = style("↑").bold();
        format!("\n  Tip: press {arrow} to try an example request.\n")
    } else {
        String::new()
    };
    let step_hint = if step_mode {
        let next = style(".next").bold();
        let go = style(".go").bold();
        format!("\n  Step mode: {next} to load next request, {go} to run remaining.\n")
    } else {
        String::new()
    };
    let version = env!("CARGO_PKG_VERSION");
    format!("{yurl} v{version}. {ctrl_d} to exit.\n\
{history_line}
  Single line:  {{g: https://httpbin.org/get}}
  Multi-line :  type YAML, then --- to send.
{step_hint}{tip}")
}

/// Read requests interactively. Calls `on_request` for each complete request string.
/// If `step_queue` is Some, enables .next and .go commands for stepping through piped requests.
pub fn run<F>(mut on_request: F, step_queue: Option<VecDeque<String>>)
where
    F: FnMut(String),
{
    let mut rl = Editor::new().expect("failed to initialize editor");
    rl.set_helper(Some(YurlHelper));
    let history_path = dirs_hint();

    let mut has_history = false;
    if let Some(ref path) = history_path {
        if rl.load_history(path).is_ok() {
            has_history = true;
        }
    }

    if !has_history {
        rl.add_history_entry(EXAMPLE).ok();
    }

    let mut queue = step_queue.unwrap_or_default();
    let step_mode = !queue.is_empty();

    eprintln!("{}", hint(&history_path, has_history, step_mode));

    loop {
        match rl.readline(PROMPT) {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                // Handle .next command
                if trimmed == ".next" || trimmed == ".n" {
                    if let Some(req) = queue.pop_front() {
                        // Pre-fill the editor with the next request
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
                            // Put back the current request
                            queue.push_front(req);
                            eprintln!("  interrupted ({} remaining)", queue.len());
                            break;
                        }
                        on_request(req);
                        executed += 1;
                    }

                    // Restore previous signal handler
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
