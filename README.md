# yurl — "Better curl"

HTTP client — [batch](#batch-config), [interactive](#step-mode), [concurrent](#concurrency), [streaming](#concurrency), [output routing](#output), [caching](#caching).
Built on [`yttp`](https://crates.io/crates/yttp), the ["Better HTTP"](#yttp--request-and-response) JSON/YAML facade.

[Guide](docs/guide.md) · [Cookbook](docs/cookbook.md)

Install: `cargo install yurl`

```bash
echo '{g: https://jsonplaceholder.typicode.com/posts/1}' | yurl
```
```yaml
s: {v: HTTP/1.1, c: 200, t: OK}
h:
  content-type: application/json
b:
  id: 1
  title: sunt aut facere...
```

Batch with API aliases, auth from env, JSON output:

```bash
echo '
{g: api!/get}
{p: api!/post, b: {name: Owl, price: 5.99}}
' | yurl '{api: httpbin.org, h: {a!: $TOKEN}, 1: "j(s b)"}'
```

## Reference

### yttp — request and response

[Full spec](https://github.com/ogheorghies/yttp#reference) — method shortcuts, header shortcuts, auth, body encoding, response formatting.

```yaml
# request
g: https://example.com                # method shortcuts: g p d, or full names
h: {a!: my-token, c!: j!}             # header key/value shortcuts expand in place
b: {city: Berlin}                     # body encoding follows Content-Type

# response — default output: y(s! h b)
s: {v: HTTP/1.1, c: 200, t: OK}       # s! -> status inline object
h: {content-type: application/json}   # response headers
b: {city: Berlin, lang: de}           # JSON -> structured, UTF-8 -> string, binary -> base64
```

### yurl extensions

```yaml
# metadata
md: {env: prod, batch: 7}            # available in output and file path templates

# output destinations
1: j(s! h b)                          # stdout (jurl default)
1: y(s! h b)                          # stdout (yurl default)
2: s                                  # stderr
file://response.raw: b                # raw body (no base64)
file://large.bin?stream: b            # explicit streaming
file://{{md.env}}/{{idx}}.json: j(s! h b)  # templated path, auto-streamed

# atoms
# response:  b    h    s!   s    s.c  s.t  s.v  (or s.code s.text s.version)
#     or:    o.b  o.h  ...
# request:   i.b  i.h
# URL:       u.scheme  u.host  u.port  u.path  u.query  u.fragment
# other:     m    u    idx  md  md.*
```

### Batch config

```yaml
api: https://api.example.com/v1       # string or {name: url, ...}

h:                                    # default headers
  a!: bearer!$TOKEN
  User-Agent: yurl/0.1

1: j(idx s.code)                     # default output

concurrency: 10                       # global max (default: 1)
progress: true                        # spinner or N for progress bar

rules:
  - match: {u: "**slow-api**"}
    concurrency: 2
  - match: {m: POST}
    h: {c!: f!}
  - match: {md.env: prod}
    h: {X-Debug: "false"}
  - match: {u: "**api.openai.com**"}
    cache: true                       # {ttl: 0, keys: [m,u,b], at: default}
  - match: {u: "**api.example.com**"}
    cache: {ttl: 3600, keys: [u, b, a], at: ./.cache}

# merge order: config -> rules (in order) -> per-request
```

## Request

Reads from stdin as JSONL (one per line) or YAML (`---` separated). Streaming — requests execute before EOF.

| Key | Description |
|-----|-------------|
| method (`g`, `p`, `d`, `put`, `patch`, `head`, `options`, `trace`) | URL |
| `h` | headers (keys and values support [shortcuts](#header-shortcuts)) |
| `b` | body (encoding follows Content-Type) |
| `md` | metadata fields, available in output and file path templates |

URLs without a scheme: `localhost`/`127.0.0.1`/`[::1]`/bare hostnames get `http://`, else `https://`.

### Body encoding

| Content-Type | Shortcut | Encoding |
|---|---|---|
| `application/json` (default) | `c!: j!` | JSON body |
| `application/x-www-form-urlencoded` | `c!: f!` | `key=value&...` from `b` object |
| `multipart/form-data` | `c!: m!` | multipart; `file://` values read from disk |

### Header shortcuts

| Shortcut | Expands to |
|---|---|
| `json!` / `j!` | `application/json` |
| `form!` / `f!` | `application/x-www-form-urlencoded` |
| `multi!` / `m!` | `multipart/form-data` |
| `html!` / `h!` | `text/html` |
| `text!` / `t!` | `text/plain` |
| `xml!` / `x!` | `application/xml` |
| `a!/suffix` | `application/suffix` |
| `t!/suffix` | `text/suffix` |
| `i!/suffix` | `image/suffix` |
| `basic!user:pass` | `Basic base64(user:pass)` |
| `bearer!token` | `Bearer token` |
| **Key shortcuts** | |
| `a!` / `auth!` | `Authorization` header key |
| `c!` / `ct!` | `Content-Type` header key |

### Authorization

`a!` inside `h` sets Authorization. Scheme inferred from value:

| Value | Result |
|---|---|
| `a!: token` | `Bearer token` |
| `a!: [user, pass]` | `Basic base64(user:pass)` |
| `a!: Scheme value` | passthrough |

`$VAR` in config headers expands from environment. Only pure `$VAR` values (entire string is `$` + alphanumeric/underscore).

## Output

Destinations: `"1"` (stdout), `"2"` (stderr), `"file://path"` (supports `{{atom}}` templates).

Default: `y(s! h b)` for yurl, `j(s! h b)` for jurl (same binary, JSON output). Per-request output keys fully replace config defaults.

### Format atoms

| Atom | Description |
|---|---|
| `b` / `o.b` | response body (raw outside `j()`/`y()`, smart-encoded inside) |
| `h` / `o.h` | response headers |
| `s` / `o.s` | status line; `s.code`, `s.text`, `s.version` for parts |
| `i.b`, `i.h` | request body, headers |
| `m` | request method |
| `u` | full URL; `u.scheme`, `u.host`, `u.port`, `u.path`, `u.query`, `u.fragment` |
| `idx` | auto-incrementing request index (0-based) |
| `md`, `md.*` | metadata value or field |

`j(...)` wraps atoms as JSON object. `y(...)` as YAML. Body in `j()`/`y()`: JSON -> structured, UTF-8 -> string, binary -> base64.

## Batch config

CLI argument provides shared config for all stdin requests. Merge order: config -> rules -> per-request.

| Key | Description |
|-----|-------------|
| `h` | default headers (shortcuts work) |
| `1`, `2`, `file://...` | default output destinations |
| `api` | API alias(es) — string or `{name: url, ...}` |
| `concurrency` | global max in-flight requests (default: 1) |
| `progress` | `true` (spinner) or `N` (progress bar) |
| `rules` | conditional overrides (see below) |

### API aliases

```yaml
api: https://api.example.com/v1        # single, used as api!/path
api: {prod: https://api.example.com, staging: https://staging.example.com}
```

`name!/path` in URLs expands to `base/path`. Unmatched names pass through unchanged.

### Rules

| Match key | Matching |
|---|---|
| `u` | URL glob (`*` = segment, `**` = any) |
| `m` | HTTP method (case-insensitive) |
| `md.<field>` | exact metadata field match |

Rule fields: `h` (headers), `concurrency`, `cache`. Multiple match criteria are ANDed. See [batch config reference](#batch-config-1) for examples.

### Concurrency

- Global: `concurrency: N` in config
- Per-endpoint: `concurrency` on rules. Request acquires global + all matching rule permits
- Outputs buffered and flushed atomically with concurrency > 1
- File paths with `{{idx}}` auto-stream; `?stream` suffix to force streaming
- With `concurrency: 1` (default), everything auto-streams

### Caching

`cache: true` on a rule is shorthand for `{ttl: 0, keys: [m, u, b], at: ~/Library/Caches/yurl}`.

| Key | Default | Description |
|---|---|---|
| `ttl` | `0` | seconds until expiry (0 = no expiry) |
| `keys` | `[m, u, b]` | hash components: `m` `u` `b` `a` `h` `h.<name>` |
| `at` | OS cache dir | cache directory path |

Application-level caching (not HTTP-compliant). Does not respect `Cache-Control`/`ETag`/`Vary`.

### Progress

`progress: true` (spinner) or `progress: N` (progress bar). Suppresses stderr output; shows suppressed count.

### Interactive mode

yurl enters interactive mode when stdin is a terminal. Type requests directly, use `.x` to inspect, `.c` to manage config.

#### Commands

| Command | Description |
|---|---|
| `{request}` | send a JSON/YAML request |
| `.c` | show config; `.c {cfg}` to replace |
| `.t` | show request templates |
| `.ref` / `.r` | show reference card (`--ref` from CLI) |
| `.help x` | detailed `.x` flag reference |
| `.help` / `.h` | show help |

#### `.x` — expand and inspect

`.x [flags] {request}` — expand/print a request with optional flags.
Horizontal (flow) pre-fills the prompt for editing. Vertical (multiline) and curl only prints, as there is no support for multiline
  editing or curl execution yet.

| Dimension | Options | Default |
|---|---|---|
| Resolution | `m` merged | unmerged |
| Layout | `v` vertical (multiline) / `h` horizontal (flow) | horizontal |
| Format | `c` curl / `j` JSON / `y` YAML | YAML |
| Headers | `s` short (yttp shortcuts) | standard |

Flags compose freely: `.x mv` = merged multiline, `.x vc` = multiline curl, `.x ms` = merged short.

```
> .x {g: api!/toys}
> {get: https://api.example.com/toys}

> .x m {g: api!/toys}
> {get: https://api.example.com/toys, h: {Authorization: Bearer tok}}

> .x vc {g: api!/toys}
curl -X GET 'https://api.example.com/toys' \
  -H 'Authorization: Bearer tok'
```

#### Step mode

`--step` flag for interactive debugging of piped requests. See [Guide](docs/guide.md#step-mode) for full walkthrough.

| Command | Description |
|---|---|
| `.next` / `.n` | load next piped request, edit, Enter to send |
| `.go` / `.g` | run all remaining, Ctrl-C to stop |

```
$ echo '
{g: api!/toys}
{g: api!/toys/1}
{p: api!/toys, b: {name: Owl}}
' | yurl --step '{api: localhost:3000, h: {a!: bearer!tok}, 1: "j(s b)"}'

yurl v0.9.0

> .next                              # pre-fills with {g: api!/toys}
> .x m {g: api!/toys}                # Ctrl-A, prepend .x m to expand merged
> {get: http://localhost:3000/toys, h: {Authorization: Bearer tok}, 1: j(s b)}
{"s":"200 OK","b":[{"id":1,"name":"Fox"},{"id":2,"name":"Cat"}]}

> .go                                # run remaining 2 requests
{"s":"200 OK","b":{"id":1,"name":"Fox","price":12.99}}
{"s":"201 Created","b":{"id":3,"name":"Owl"}}
  2 requests executed
```
