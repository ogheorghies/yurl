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
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::Config;
use crate::{expand_request, expand_request_structured};

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
                "requests are piped. type .next — .help, .t, or .ref"
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

pub fn reference_card() -> String {
    format!("\
{request}
  g/p/d/put/patch/head/options/trace: url       method shortcuts
  h: {{key: val}}                                 headers (shortcuts below)
  b: {{key: val}}                                 body (encoding follows Content-Type)
  q: {{key: val}}                                 query parameters
  md: {{key: val}}                                metadata fields (match, log, etc.)

{shortcuts}
  Header keys     a!, auth! → Authorization      c!, ct! → Content-Type
  Content-Type    j! → json  f! → form-urlencoded  m! → multipart  h! → html  t! → text  x! → xml
  Type prefix     a!/sub → application/sub        t!/sub → text/sub         i!/sub → image/sub
  Auth            bearer!tok    basic!user:pass   [user, pass]    Scheme val → passthrough
  Env vars        $VAR in config headers expands from environment

{output}
  1: format → stdout     2: format → stderr
  file://path: format → file output          ?stream → force streaming
  file://{{{{idx}}}}.json  → templated path, {{{{atom}}}} expansion

  Formats         j(atoms) → JSON object       y(atoms) → YAML document      atom → raw

  Response atoms  b → body    h → headers    s → status line    s → status object
                  s.c → code    s.t → text    s.v → version
  Request atoms   i.b → body    i.h → headers    i.s → status    m → method
  URL atoms       u → full     u.scheme  u.host  u.port  u.path  u.query  u.fragment
  Other           idx → request index    md → metadata    md.field  → metadata field

  b → body
    inside  j()/y(): JSON → structured, UTF-8 → string, binary → base64.
    outside j()/y(): raw bytes

{config}
  api: url                            single alias (api!/path)
  api: {{name: url, ...}}               multiple aliases (name!/path)
  h: {{key: val}}                       default headers
  1/2/file: format                    default output
  concurrency: N                      global max in-flight (default: 1)
  progress: true | N                  spinner or progress bar

  rules:
    - match: {{u: \"**glob**\"}}          URL glob (* = segment, ** = any)
      match: {{m: POST}}                HTTP method
      match: {{md.field: val}}          metadata field
      h: {{key: val}}                   add/override headers
      concurrency: N                  per-endpoint limit
      cache: true                     default: {{ttl: 0, keys: [m,u,b]}}
      cache: {{ttl: 3600, keys: [u,b,a], at: ./.cache}}

  Merge order: config → rules (in order) → per-request\n",
        request = style("Request").bold().underlined(),
        shortcuts = style("Shortcuts").bold().underlined(),
        output = style("Output").bold().underlined(),
        config = style("Config").bold().underlined(),
    )
}

fn help_text(history_path: &Option<String>, step_mode: bool) -> String {
    let history_line = history_path
        .as_deref()
        .map(|p| {
            let display = if let Ok(home) = std::env::var("HOME") {
                p.replace(&home, "~")
            } else {
                p.to_string()
            };
            format!("\nHistory: {display}\n")
        })
        .unwrap_or_default();
    let step_cmds = if step_mode {
        format!("\
  {next}  {ndot}   load next piped request, edit, Enter to send\n\
  {go}    {gdot}   run all remaining piped requests, Ctrl-C to stop\n",
            next = style(".next").bold(), ndot = style(".n").dim(),
            go = style(".go").bold(), gdot = style(".g").dim(),
        )
    } else {
        String::new()
    };
    format!("\n\
  {{request}}   send a JSON/YAML request\n\
  {x}  {{req}}   expand request (wire-ready: query in URL)\n\
  {xx} {{req}}   expand request (structured: q: and b: as objects)\n\
  {c}          show current config\n\
  {c}  {{cfg}}   replace active config\n\
{step_cmds}\
  {t}          show request templates\n\
  {r}  {rdot}    show reference card\n\
  {help}  {hdot}   show this help\n\
  {ctrl_d}      exit\n\
{history_line}\n",
        x = style(".x").bold(),
        xx = style(".xx").bold(),
        c = style(".c").bold(),
        t = style(".t").bold(),
        r = style(".ref").bold(), rdot = style(".r").dim(),
        help = style(".help").bold(), hdot = style(".h").dim(),
        ctrl_d = style("Ctrl-D").bold(),
    )
}

/// Prompt with pre-filled text, let user edit, then execute. Returns false on fatal error.
fn prompt_and_send<F>(
    rl: &mut Editor<YurlHelper, rustyline::history::DefaultHistory>,
    initial: &str,
    on_request: &mut F,
) -> bool
where
    F: FnMut(String),
{
    match rl.readline_with_initial(PROMPT, (initial, "")) {
        Ok(edited) => {
            let edited = edited.trim().to_string();
            if !edited.is_empty() {
                rl.add_history_entry(&edited).ok();
                on_request(edited);
            }
            true
        }
        Err(rustyline::error::ReadlineError::Interrupted) => {
            eprintln!("  (skipped)");
            true
        }
        Err(_) => false,
    }
}

/// Try to strip a `.x` or `.xx` prefix from the input, expand, and re-prompt.
/// Returns None if no expand prefix was found (caller should handle as normal input).
/// Returns Some(true) to continue the loop, Some(false) to break.
///
/// On expand error, prints the colored error and re-prompts with the original
/// input so the user can fix it.
fn try_expand_and_send<F>(
    input: &str,
    rl: &mut Editor<YurlHelper, rustyline::history::DefaultHistory>,
    config: &Arc<ArcSwap<Config>>,
    on_request: &mut F,
) -> Option<bool>
where
    F: FnMut(String),
{
    // .xx must be checked before .x since .xx starts with .x
    let (_req, result) = if let Some(req) = input.strip_prefix(".xx ") {
        let req = req.trim();
        if req.is_empty() { return Some(true); }
        (req, expand_request_structured(req, &config.load()))
    } else if let Some(req) = input.strip_prefix(".x ") {
        let req = req.trim();
        if req.is_empty() { return Some(true); }
        (req, expand_request(req, &config.load()))
    } else {
        return None;
    };
    match result {
        Ok(expanded) => Some(prompt_and_send(rl, &expanded, on_request)),
        Err(e) => {
            eprintln!("{}", e.display_colored());
            // Re-prompt with original input so user can fix
            Some(prompt_and_send(rl, input, on_request))
        }
    }
}

/// Read requests interactively. Calls `on_request` for each complete request string.
/// `config` is shared via ArcSwap — `.x` reads it, `.c` replaces it.
/// If `step_queue` is Some, enables .next and .go commands for stepping through piped requests.
pub fn run<F>(mut on_request: F, config: &Arc<ArcSwap<Config>>, step_queue: Option<VecDeque<String>>)
where
    F: FnMut(String),
{
    let history_path = dirs_hint();
    let mut queue = step_queue.unwrap_or_default();
    let step_mode = !queue.is_empty();

    let rl_config = rustyline::Config::builder()
        .behavior(rustyline::config::Behavior::PreferTerm)
        .build();
    let mut rl = Editor::with_config(rl_config).expect("failed to initialize editor");
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

                // Handle .ref command — show reference card
                if trimmed == ".ref" || trimmed == ".r" {
                    eprint!("{}", reference_card());
                    continue;
                }

                // Handle .t command — show request templates
                if trimmed == ".t" {
                    eprintln!("\n\
  {{g: url}}                                    GET\n\
  {{g: url, q: {{k: v}}}}                         GET + query\n\
  {{p: url, b: {{k: v}}}}                         POST json\n\
  {{p: url, h: {{ct!: f!}}, b: {{k: v}}}}           POST form\n\
  {{p: url, h: {{ct!: m!}}, b: {{k: file://path}}}} POST multipart\n\
  {{p: url, h: {{a!: bearer!tok}}, b: {{k: v}}}}    POST + headers auth\n\
  {{put: url, b: {{k: v}}}}                       PUT\n\
  {{d: url}}                                    DELETE\n");
                    continue;
                }

                // Handle .next command
                if trimmed == ".next" || trimmed == ".n" {
                    if let Some(req) = queue.pop_front() {
                        match rl.readline_with_initial(PROMPT, (&req, "")) {
                            Ok(edited) => {
                                let edited = edited.trim().to_string();
                                if edited.is_empty() {
                                    // skip
                                } else if let Some(ok) = try_expand_and_send(&edited, &mut rl, config, &mut on_request) {
                                    if !ok { break; }
                                } else {
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

                // Handle .c command — show or replace config
                if trimmed == ".c" {
                    let cfg = config.load();
                    eprintln!("  config: {}", cfg.summary());
                    continue;
                }
                if let Some(cfg_str) = trimmed.strip_prefix(".c ") {
                    let cfg_str = cfg_str.trim();
                    if cfg_str.is_empty() {
                        let cfg = config.load();
                        eprintln!("  config: {}", cfg.summary());
                        continue;
                    }
                    match yttp::parse(cfg_str) {
                        Ok(val) => {
                            let new_config = Config::parse(&val);
                            eprintln!("  config: {}", new_config.summary());
                            config.store(Arc::new(new_config));
                        }
                        Err(e) => {
                            eprintln!("  error: {e}");
                        }
                    }
                    continue;
                }

                // Handle .xx / .x commands — expand request with config
                if let Some(ok) = try_expand_and_send(trimmed, &mut rl, config, &mut on_request) {
                    if !ok { break; }
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
