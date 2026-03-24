use console::style;

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
  Request atoms   i.b → body    i.h → headers    m → method
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

macro_rules! b { ($s:expr) => { style($s).bold() } }
macro_rules! d { ($s:expr) => { style($s).dim() } }

macro_rules! row {
    ([$($part:expr),*], $desc:expr) => {{
        let parts: Vec<(String, usize)> = vec![
            $({
                let s = $part;
                let displayed = s.to_string();
                // Strip ANSI codes to get visible length
                let visible = console::strip_ansi_codes(&displayed).len();
                (displayed, visible)
            }),*
        ];
        let styled: String = parts.iter().map(|(s, _)| s.as_str()).collect();
        let visible_len: usize = parts.iter().map(|(_, l)| l).sum();
        let pad = 20usize.saturating_sub(visible_len).max(1);
        format!("  {styled}{:pad$}{}", "", $desc)
    }};
}

pub fn help_text(history_path: &Option<String>) -> String {
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

    let lines = [
        row!(["{request}"],                            "send a JSON/YAML request"),
        row!([b!(".x"), " ", d!("mvjcs"), " {req}"],   format!("expand — {} for flags", b!(".help x"))),
        row!([b!(".c")],                               "show current config"),
        row!([b!(".c"), " {cfg}"],                     "replace active config"),
        row!([b!(".open file")],                       "open requests from file"),
        row!([b!(".pop"), " ", d!(".p")],              "pop next request, edit, Enter to send"),
        row!([b!(".repop")],                           "re-pop last popped request"),
        row!([b!(".go"), " ", d!(".g")],               "run remaining, Ctrl-C to stop"),
        row!([b!(".t")],                               "show request templates"),
        row!([b!(".ref"), " ", d!(".r")],              "show reference card"),
        row!([b!(".help"), " ", d!(".h")],             "show this help"),
        row!([b!("Ctrl-D")],                           "exit"),
    ];
    format!("\n{}\n{history_line}\n", lines.join("\n"))
}

pub fn expand_help() -> String {
    format!("\
{title}

  Dimension    Options                   Default
  Resolution   {m} merged with config      unmerged
  Layout       {v} vert. / {h} horiz.        horizontal
  Format       {c} curl / {j} JSON           YAML
  Headers      {s} shortcuts per yttp      HTTP expanded

  Horizontal: single line (prompt edit)
  Vertical  : multiline, indented (print only, technical limitation)

  .x     {{req}}    YAML horizontal
  .x {m}   {{req}}    YAML horizontal, merged w/ config
  .x {v}   {{req}}    YAML vertical
  .x {m}{v}  {{req}}    YAML merged, vertical
  .x {m}{s}  {{req}}    YAML merged, short yttp, horizontal
  .x {m}{s}{v} {{req}}    YAML merged, short yttp, vertical
  .x {j}   {{req}}    JSON horizontal
  .x {j}{v}  {{req}}    JSON vertical
  .x {c}   {{req}}    curl horizontal
  .x {c}{v}  {{req}}    curl vertical\n",
        title = style(".x [flags] {request}").bold(),
        m = style("m").bold(),
        v = style("v").bold(),
        h = style("h").bold(),
        j = style("j").bold(),
        c = style("c").bold(),
        s = style("s").bold(),
    )
}

pub const TEMPLATES: &str = "\n\
  {g: url}                                    GET\n\
  {g: url, q: {k: v}}                         GET + query\n\
  {p: url, b: {k: v}}                         POST json\n\
  {p: url, h: {ct!: f!}, b: {k: v}}           POST form\n\
  {p: url, h: {ct!: m!}, b: {k: file://path}} POST multipart\n\
  {p: url, h: {a!: bearer!tok}, b: {k: v}}    POST + headers auth\n\
  {put: url, b: {k: v}}                       PUT\n\
  {d: url}                                    DELETE\n";
