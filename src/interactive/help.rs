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
    format!("\n\
  {{request}}            send a JSON/YAML request\n\
  {x} {flags} {{req}}   expand — {help_x} for flag reference\n\
  {c}               show current config\n\
  {c}  {{cfg}}        replace active config\n\
  {step} {sdot}   load requests from file\n\
  {next}  {ndot}   load next request, edit, Enter to send\n\
  {go}    {gdot}   run remaining requests, Ctrl-C to stop\n\
  {t}               show request templates\n\
  {r}  {rdot}         show reference card\n\
  {help}  {hdot}        show this help\n\
  {ctrl_d}           exit\n\
{history_line}\n",
        x = style(".x").bold(),
        flags = style("[flags]").dim(),
        help_x = style(".help x").bold(),
        c = style(".c").bold(),
        step = style(".step file").bold(), sdot = style(".s").dim(),
        next = style(".next").bold(), ndot = style(".n").dim(),
        go = style(".go").bold(), gdot = style(".g").dim(),
        t = style(".t").bold(),
        r = style(".ref").bold(), rdot = style(".r").dim(),
        help = style(".help").bold(), hdot = style(".h").dim(),
        ctrl_d = style("Ctrl-D").bold(),
    )
}

pub fn expand_help() -> String {
    format!("\
{title}

  Dimension    Options                   Default
  Resolution   {m} merged                unmerged
  Layout       {v} vertical (multiline) / {h} horizontal (flow)  horizontal
  Format       {c} curl / {j} JSON       YAML
  Headers      {s} short (yttp shortcuts) standard

  Flags compose freely. Flow pre-fills prompt for editing.
  Multiline and curl print to screen.

  .x {{req}}         YAML flow (edit)
  .x {m} {{req}}       merged (edit)
  .x {v} {{req}}       YAML multiline (print)
  .x {m}{v} {{req}}      merged multiline (print)
  .x {j} {{req}}       JSON flow (edit)
  .x {j}{v} {{req}}      JSON multiline (print)
  .x {c} {{req}}       curl flow (print)
  .x {v}{c} {{req}}      curl multiline (print)
  .x {m}{s} {{req}}      merged, short headers (edit)
  .x {m}{v}{s} {{req}}     merged, multiline, short (print)\n",
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
