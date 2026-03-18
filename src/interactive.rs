use console::style;
use rustyline::DefaultEditor;
use std::io::{self, Write};

const PROMPT: &str = "> ";
const CONTINUATION: &str = "… ";
const EXAMPLE: &str = "{g: https://httpbin.org/get}";

fn hint(history_path: &Option<String>, has_history: bool) -> String {
    let yurl = style("yurl").bold().cyan();
    let ctrl_d = style("Ctrl-D").bold();
    let history_line = history_path
        .as_deref()
        .map(|p| format!("\n  History: {p}\n"))
        .unwrap_or_default();
    let tip = if !has_history {
        let arrow = style("↑").bold();
        format!("\n  Tip: press {arrow} to try an example request.\n")
    } else {
        String::new()
    };
    format!("{yurl} interactive mode. Type requests as JSON/YAML. {ctrl_d} to exit.\n\
{history_line}
  Single line:  {{g: https://httpbin.org/get}}
  Multi-line :  type YAML, then --- to send.
{tip}")
}

/// Read requests interactively. Calls `on_request` for each complete request string.
pub fn run<F>(mut on_request: F)
where
    F: FnMut(String),
{
    let mut rl = DefaultEditor::new().expect("failed to initialize editor");
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

    eprintln!("{}", hint(&history_path, has_history));

    loop {
        match rl.readline(PROMPT) {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
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
                                // --- or blank line after content → send
                                break;
                            }
                            buf.push_str(&cont);
                            buf.push('\n');
                        }
                        Err(_) => break, // Ctrl-D or error → send what we have
                    }
                }

                let final_request = buf.trim().to_string();
                if !final_request.is_empty() {
                    // Add the full multi-line request as one history entry
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
