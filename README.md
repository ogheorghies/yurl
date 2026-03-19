# yurl

`yurl` is an HTTP client built for clarity, supporting one-off and batch requests with concurrency and streaming.
Built on [`yttp`](https://crates.io/crates/yttp), the ["Better HTTP"](#better-http---request-and-response) JSON/YAML façade. Adds flexible output routing and rule-based middleware.

Want JSON output? Use `jurl` - same binary.

[Shortcuts](#header-shortcuts) · [Auth](#authorization) · [Output](#output) · [Concurrency](#concurrency-and-streaming) ·
[Caching](#caching) · [Progress](#progress) · [Batch config](#batch-config) · [Cookbook](#cookbook) · [Reference](#reference)

Install with: `cargo install yurl`

If the first examples make sense, jump to the [Reference](#reference).

![demo](demo/demo-crud.gif)

```bash
echo '{g: https://jsonplaceholder.typicode.com/posts/1}' | yurl
```
Output, edited for brevity:
```yaml
s: {v: HTTP/1.1, c: 200, t: OK}    # status (inline by default)
h:                                 # response (output) headers
  content-type: application/json
  server: cloudflare
b:                                  # response (output) body — JSON preserved as structure
  id: 1                             # ← API response content, not jurl
  title: sunt aut facere...
  userId: 1
```

Batch mode:
```bash
echo '
{g: https://jsonplaceholder.typicode.com/posts/1}
---
{g: https://jsonplaceholder.typicode.com/posts/2}' | yurl '{h: {a!: [user, pass]}}'
```

Multiline YAML helps with readability, but single-line JSON is also fine.

```bash
cat <<'EOF' | jurl
p: https://httpbin.org/post      # HTTP method shortcut + URL

h:                               # request (input) headers
  a!: my-token                   # or auth! → Authorization: Bearer my-token
  c!: f!                         # or ct!   → Content-Type: application/x-www-form-urlencoded
  Accept: j!                     # or json! or a!/json → application/json
  X-Request-Id: abc-123
b:                               # request (input) body, form-encoded per c!:
  city: Berlin
  lang: de

md:                              # metadata, see the output file name below
  env: prod
  batch: 7

1: j(h)                          # stdout ← or j(o.h) response (output) headers as JSON
2: s                             # stderr ← raw status line - j() or y() would work
file://{{md.env}}/{{idx}}.raw: b # file ← raw body (not base64), path templated
file://out.yaml: y(i.h, o.h)     # file ← YAML of request (input) and response (output) headers
EOF
```

Shared settings for batched requests can be passed as a CLI argument:

```bash
# save config to file
cat <<'EOF' > /tmp/api-config.yaml
h:                                     # default headers for all requests
  a!: bearer!my-token                  # Authorization: Bearer my-token
  User-Agent: jurl/0.1
1: j(idx,md,s.code)                    # JSON to stdout: index, metadata, status code

rules:                                 # conditional header overrides
  - match: {m: POST}                   # all POST requests →
    h: {c!: f!}                        #   use form encoding
  - match: {md.env: prod}              # requests tagged env: prod →
    h:                                 #   add debug header
      X-Debug: "false"
EOF

# send requests — each YAML document (---) becomes one JSONL line
cat <<'EOF' | jurl "$(cat /tmp/api-config.yaml)"
g: https://httpbin.org/get             # GET, tagged as prod
md: {env: prod}
---
p: https://httpbin.org/post            # POST, body form-encoded by rule
b: {city: Berlin, lang: de}
---
g: https://httpbin.org/get             # GET, tagged as staging
md: {env: staging}
EOF
```

## Request

Reads requests from stdin as JSON (one per line) or YAML (documents separated by `---`).
The HTTP method key holds the URL. Any capitalization is accepted; `g`, `p`, `d` are shortcuts for get, post, delete.

```
echo '{p: https://httpbin.org/post, b: {key: val}}' | jurl
```

### Request keys

- HTTP method (`get`, `post`, `put`, `delete`, `patch`, `head`, `options`, `trace`) — URL
- `h` / `headers` — request (input) headers (keys and values support shortcuts, see below)
- `b` / `body` — request (input) body (encoding determined by Content-Type)
- `md` — arbitrary metadata (any JSON value), echoed into output

### Body encoding

The `Content-Type` header determines how `b` is encoded:

- `application/json` (default, `c!: j!` is implied) — JSON body
- `application/x-www-form-urlencoded` — form encoding (`b` object becomes `key=value&...`)
- `multipart/form-data` — multipart encoding; values starting with `file://` are read from disk

```bash
cat <<'EOF' | jurl
p: https://httpbin.org/post
h:
  c!: f!
b: {city: Berlin, lang: de}
EOF

cat <<'EOF' | jurl
p: https://httpbin.org/post
h:
  c!: m!
b:
  desc: a photo
  file: file:///tmp/img.png
EOF
```

### Authorization

The `a!` (or `auth!`) key inside `h` sets the `Authorization` header. The auth scheme is inferred from the value type:

**Bearer** — pass a string token:

```
echo '{g: https://httpbin.org/get, h: {a!: my-token}}' | jurl
# → Authorization: Bearer my-token
```

**Basic** — pass credentials as an array:

```
echo '{g: https://httpbin.org/get, h: {a!: [user, pass]}}' | jurl
# → Authorization: Basic dXNlcjpwYXNz
```

**Other schemes** — if the string already contains a scheme prefix (has a space), it's passed through as-is:

```
echo '{g: https://httpbin.org/get, h: {a!: Digest abc123}}' | jurl
# → Authorization: Digest abc123
```

The explicit `basic!` and `bearer!` value prefixes also still work:

```
echo '{g: https://httpbin.org/get, h: {a!: basic!user:pass}}' | jurl
echo '{g: https://httpbin.org/get, h: {a!: [user, pass]}}' | jurl
echo '{g: https://httpbin.org/get, h: {a!: bearer!my-token}}' | jurl
echo '{g: https://httpbin.org/get, h: {a!: my-token}}' | jurl
```

In multi-line YAML:

```yaml
h:
  a!: my-token          # bearer (most common)
  # or
  a!: [user, pass]      # basic
  # or
  a!: Digest abc123     # explicit scheme
```

`a!` works inside `h:` in requests, config defaults, and rules.

### Header shortcuts

Shortcuts expand in header keys and values:

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
| `a!` / `auth!` | `Authorization` (header key) |
| `c!` / `ct!` | `Content-Type` (header key) |

```
echo '{g: https://httpbin.org/get, h: {a!: basic!user:pass}}' | jurl
echo '{g: https://httpbin.org/get, h: {Accept: a!/xml}}' | jurl
```

## Output

By default, `jurl` writes `j(s!,h,b)` (JSON with body, headers, and status) to stdout. 
To customize output, add to the request JSON key-value pairs like so:
- key: the destination — `"1"` (stdout), `"2"` (stderr), or `"file://path"` (supports `{{atom}}` templates)
- value: what to write — a raw atom like `b` or `s`, or `j(...)` to output them as JSON

Multiple destinations can be used in a single request. In the unlikely case that the files resolve to the same name, the last value wins.

### Format

Atoms reference parts of the response (output) or request (input):

**Response (output)** — the default, most common:

- `b` / `o.b` — response (output) body:
  - outside `j()`/`y()`: raw bytes
  - inside `j()`/`y()`: smart encoding — JSON body → embedded as structured value, UTF-8 text → string, binary → base64 string
- `h` / `o.h` — response (output) headers (raw HTTP format outside `j()`, JSON object inside `j()`)
- `s` / `o.s` — response status line; `s.code`, `s.text`, `s.version` for parts

**Request (input)** — echo what was sent:

- `i.b` — request (input) body
- `i.h` — request (input) headers
- `i.s` — request status line

**Other:**

- `m` — request method
- `u` — full request URL
- `idx` — auto-incrementing request index (0-based)
- `md` — metadata (entire value); `md.x`, `md.y` → grouped as `"md": {"x": ..., "y": ...}`

URL parts and metadata are available for file path templates: `u.scheme`, `u.host`, `u.port`, `u.path`, `u.query`, `u.fragment`, `idx`, `md`, `md.*`.

`j(...)` wraps atoms into a JSON object.

Default output (when no destination key is present): `{"1": "j(s!,h,b)"}`

### Examples

```
echo '{g: https://httpbin.org/get}' | jurl | jq .b       # body is structured JSON, not base64

echo '{g: https://httpbin.org/get, 1: b}' | jurl

echo '{g: https://httpbin.org/get, 1: j(s.code,s.text)}' | jurl

echo '{g: https://httpbin.org/get, file://./out/{{u.host}}/{{m}}.txt: b}' | jurl
```

## Batch config

An optional CLI argument provides shared configuration for all requests: default headers, output format, and conditional rules.

```
jurl '{h: {a!: bearer!tok}}'
```

All stdin requests inherit these headers. Per-request (input) headers override config headers.
Shortcuts (`c!`/`ct!`, `a!`/`auth!`, value shortcuts) work in config and rules too.

### Rules

Rules conditionally add headers based on URL, method, or metadata matching.

```bash
# save config to file
cat <<'EOF' > /tmp/rules.yaml
h: {User-Agent: jurl/0.1}
rules:
  - match: {u: "**httpbin.org**"}
    h: {X-Custom: "yes"}
  - match: {m: POST}
    c!: j!
  - match: {md.env: prod}
    h: {X-Debug: "false"}
EOF

# use config
echo '{g: https://httpbin.org/get}' | jurl "$(cat /tmp/rules.yaml)"
```

- `u` — URL glob (`*` matches non-`/`, `**` matches anything)
- `m` — HTTP method (exact, case-insensitive)
- `md.<field>` — exact metadata field match

Merge order: config defaults → matching rules (in order) → per-request.

### Concurrency and streaming

By default, requests run sequentially (`concurrency: 1`). Set `concurrency` in batch config to run requests in parallel:

```
jurl '{concurrency: 10}'
```

Per-endpoint limits can be set via rules — a request must hold a permit from the global semaphore **and** from every matching rule before executing:

```bash
cat <<'EOF' > /tmp/config.yaml
concurrency: 10                      # up to 10 requests in flight globally
rules:
  - match: {u: "**slow-api.com**"}
    concurrency: 2                   # but at most 2 to slow-api.com
EOF
```

If a request matches multiple rules with concurrency limits, it acquires all of them. The effective concurrency is the minimum — the most restrictive rule wins:

```yaml
concurrency: 10
rules:
  - match: {u: "**api.example.com**"}
    concurrency: 5                   # up to 5 to this API
  - match: {m: POST}
    concurrency: 2                   # up to 2 POSTs globally
# A POST to api.example.com needs both permits → at most 2 concurrent
```

When running requests concurrently, outputs could interleave. `jurl` handles this automatically:

- **File outputs with `{{idx}}` in the path** are guaranteed unique per request. These are streamed directly to disk — no buffering, constant memory regardless of response size.
- **File outputs without `{{idx}}`** *could* collide across requests, so they are buffered and written atomically, unless otherwise stated with `?stream`
- **stdout/stderr with `concurrency: 1`** — streamed directly (no interleaving risk with a single request in flight).
- **stdout/stderr with `concurrency > 1`** — buffered and flushed atomically to prevent interleaving.

**Override with `?stream`.** to force streaming on a file destination that `jurl` would otherwise buffer:
- a static path you know is only used by one request
- a dynamic path that acquires its uniqueness from other components apart from `{{idx}}`, e.g. some `{{md.*}}`

Note that when `concurrency` is `1`, as is the case for one-off requests, streaming is automatically enabled, so you don't need to worry about
large payloads causing OOM errors.

```bash
cat <<'EOF' | jurl
g: https://example.com/large.bin
file://./large.bin?stream: b
1: j(s)
EOF
```

When streaming, the body is written chunk-by-chunk as it arrives. If another (non-streaming) destination also needs the body, it is still buffered for that destination — but the streaming file never accumulates the full response in memory.

### Caching

Rules can cache responses in a local SQLite database. Useful for avoiding redundant API calls during development, retries, or batch reruns.

```yaml
rules:
  - match: {u: "**api.openai.com**"}
    cache: true                      # cache indefinitely with default settings
```

`cache: true` is shorthand for:

```yaml
cache:
  ttl: 0                            # seconds until expiry (0 = no expiry)
  keys: [m, u, b]                   # what to hash for the cache key
  at: ~/Library/Caches/yurl         # macOS default (Linux: ~/.cache/yurl)
```

The cache key is a SHA-256 hash of the selected request parts. Two requests match the same cache entry only if all selected parts are identical.

**`keys`** controls which parts of the request are included in the hash:

| Key | Meaning |
|---|---|
| `m` | HTTP method |
| `u` | URL |
| `b` | request body |
| `a` | Authorization header |
| `h` | all headers |
| `h.<name>` | specific header (e.g. `h.x-api-key`) |

The default `[m, u, b]` means: same method + same URL + same body = cache hit. Add `a` to separate caches per API key.

**Examples:**

Cache all requests to an API with a 1-hour TTL:

```yaml
rules:
  - match: {u: "**api.example.com**"}
    cache: {ttl: 3600}
```

Cache OpenAI requests indefinitely, keyed by body and auth (different API keys get separate caches):

```yaml
rules:
  - match: {u: "**api.openai.com**"}
    cache: {keys: [u, b, a]}
```

Use a project-local cache directory:

```yaml
rules:
  - match: {u: "**api.example.com**"}
    cache: {at: ./.cache}
```

Expired entries are cleaned up automatically on startup. To clear the cache entirely, delete the cache directory.

> **Note:** Currently, this is application-level caching, not HTTP-compliant caching. It does not respect `Cache-Control`, `ETag`, or `Vary` headers. Responses are cached based solely on the configured `keys` and `ttl`. This makes it useful for memoizing API calls (e.g. LLM endpoints) but not as a general HTTP cache.

### Progress

Set `progress` in batch config to show a progress bar on stderr:

```
jurl '{progress: true}'
```

If the number of requests is known, pass it as a number for a proper progress bar instead of a spinner:

```
jurl '{progress: 100, concurrency: 10}'
```

When progress is active, any request output directed to stderr (`"2"`) is silently suppressed. A warning line appears below the progress bar showing how many requests had their stderr output suppressed.

## Cookbook

Simple GET — default output is JSON with body, headers, status:

```
$ echo '{g: https://httpbin.org/get}' | jurl
{"s": {"v": "HTTP/1.1", "c": 200, "t": "OK"}, "h": {"content-type": "application/json", ...}, "b": {"url": "https://httpbin.org/get", ...}}
```

Raw body to stdout:

```
$ echo '{g: https://httpbin.org/get, 1: b}' | jurl
{"args": {}, "headers": {"Host": "httpbin.org", ...}, "url": "https://httpbin.org/get"}
```

Just the status line:

```
$ echo '{g: https://httpbin.org/get, 1: s}' | jurl
HTTP/1.1 200 OK
```

Status code and text as JSON:

```
$ echo '{g: https://httpbin.org/get, 1: j(s.code,s.text)}' | jurl
{"s": {"c": 200, "t": "OK"}}
```

POST with JSON body (default encoding). `c!` is optional since JSON is the default, but `json!` / `j!` work:

```
$ echo '{p: https://httpbin.org/post, b: {key: val}, 1: b}' | jurl
$ echo '{p: https://httpbin.org/post, h: {c!: j!}, b: {key: val}, 1: b}' | jurl
{..."json": {"key": "val"}...}
```

Form POST — full header, then with `form!` / `f!`:

```bash
# full Content-Type header
$ echo '{p: https://httpbin.org/post, h: {Content-Type: application/x-www-form-urlencoded}, b: {city: Berlin}, 1: b}' | jurl

# shortcut
$ echo '{p: https://httpbin.org/post, h: {c!: f!}, b: {city: Berlin}, 1: b}' | jurl

$ cat <<'EOF' | jurl
p: https://httpbin.org/post
h: {c!: f!}
b: {city: Berlin}
1: b
EOF

# output (all three)
{..."form": {"city": "Berlin"}...}
```

Multipart upload — full header, then with `multi!` / `m!`:

```bash
# full Content-Type header
$ echo '{p: https://httpbin.org/post, h: {Content-Type: multipart/form-data}, b: {desc: test, file: file:///tmp/f.txt}, 1: b}' | jurl

# shortcut
$ echo '{p: https://httpbin.org/post, h: {c!: m!}, b: {desc: test, file: file:///tmp/f.txt}, 1: b}' | jurl

$ cat <<'EOF' | jurl
p: https://httpbin.org/post
h: {c!: m!}
b:
  desc: test
  file: file:///tmp/f.txt
1: b
EOF

# output (all three)
{..."form": {"desc": "test"}, "files": {"file": "..."}...}
```

Basic auth — full header, then `basic!` shortcut:

```
$ echo '{g: https://httpbin.org/get, h: {Authorization: Basic dXNlcjpwYXNz}, 1: b}' | jurl
$ echo '{g: https://httpbin.org/get, h: {a!: basic!user:pass}, 1: b}' | jurl
$ echo '{g: https://httpbin.org/get, h: {a!: [user, pass]}, 1: b}' | jurl
{..."headers": {..."Authorization": "Basic dXNlcjpwYXNz"...}...}
```

Bearer auth — full header, then `bearer!` shortcut:

```
$ echo '{g: https://httpbin.org/get, h: {Authorization: Bearer tok123}, 1: b}' | jurl
$ echo '{g: https://httpbin.org/get, h: {a!: bearer!tok123}, 1: b}' | jurl
{..."headers": {..."Authorization": "Bearer tok123"...}...}
```

MIME prefix shortcuts — `a!/`, `t!/`, `i!/`:

```
$ echo '{g: https://httpbin.org/get, h: {Accept: a!/xml}, 1: b}' | jurl
{..."headers": {..."Accept": "application/xml"...}...}

$ echo '{g: https://httpbin.org/get, h: {Accept: t!/csv}, 1: b}' | jurl
{..."headers": {..."Accept": "text/csv"...}...}

$ echo '{g: https://httpbin.org/get, h: {Accept: i!/png}, 1: b}' | jurl
{..."headers": {..."Accept": "image/png"...}...}
```

Named shortcuts — long and short forms:

```
$ echo '{g: https://httpbin.org/get, h: {Accept: x!}, 1: b}' | jurl
{..."headers": {..."Accept": "application/xml"...}...}

$ echo '{g: https://httpbin.org/get, h: {Accept: h!}, 1: b}' | jurl
{..."headers": {..."Accept": "text/html"...}...}

$ echo '{g: https://httpbin.org/get, h: {Accept: t!}, 1: b}' | jurl
{..."headers": {..."Accept": "text/plain"...}...}
```

Metadata — scalar, object, and field selection:

```bash
$ echo '{g: https://httpbin.org/get, md: batch-1, 1: j(idx,md,s.code)}' | jurl
{"idx": 0, "md": "batch-1", "s": {"code": 200}}

# YAML with metadata object
$ cat <<'EOF' | jurl
g: https://httpbin.org/get
md:
  id: 42
  tag: test
1: j(md)
EOF
{"md": {"id": 42, "tag": "test"}}

# selecting specific metadata fields
$ echo '{g: https://httpbin.org/get, md: {id: 42, tag: test}, 1: j(md.id)}' | jurl
{"md": {"id": 42}}
```

JSONL — multiple requests, idx auto-increments:

```
$ printf '{"g":"https://httpbin.org/get","1":"j(idx,s.code)"}\n{"g":"https://httpbin.org/get","1":"j(idx,s.code)"}\n' | jurl
{"idx": 0, "s": {"code": 200}}
{"idx": 1, "s": {"code": 200}}
```

Default output format in config — requests don't need to repeat it:

```
$ printf '{g: https://httpbin.org/get}\n{p: https://httpbin.org/post, b: {x: "1"}}\n' | jurl '{1: j(idx,m,s.code)}'
{"idx": 0, "m": "GET", "s": {"code": 200}}
{"idx": 1, "m": "POST", "s": {"code": 200}}
```

Per-request output overrides the config default:

```
$ printf '{g: https://httpbin.org/get}\n{g: https://httpbin.org/get, 1: s}\n' | jurl '{1: j(idx,s.code)}'
{"idx": 0, "s": {"code": 200}}
HTTP/1.1 200 OK
```

Multiple destinations — body to file, headers to stdout, status to stderr:

```bash
$ cat <<'EOF' | jurl
g: https://httpbin.org/get
1: j(h)                          # stdout ← headers as JSON
2: s                             # stderr ← raw status line
file://body.out: b               # file   ← raw body
EOF

# stdout
{"h": {"content-type": "application/json", ...}}
# stderr
HTTP/1.1 200 OK
# body.out contains the raw response (output) body
```

Templated file output:

```bash
$ cat <<'EOF' | jurl
g: https://httpbin.org/get
file://./out/{{u.host}}/{{m}}.txt: b
EOF
# writes response (output) body to ./out/httpbin.org/GET.txt
```

Config — default auth for all requests:

```
$ echo '{g: https://httpbin.org/get, 1: b}' | jurl '{h: {a!: bearer!session-tok}}'
{..."headers": {..."Authorization": "Bearer session-tok"...}...}
```

Config — rule adds form encoding to all POSTs:

```
$ echo '{p: https://httpbin.org/post, b: {x: "1"}, 1: b}' | jurl '{rules: [{match: {m: POST}, h: {c!: f!}}]}'
{..."form": {"x": "1"}...}
```

Config — rule matches metadata:

```bash
$ cat <<'EOF' | jurl '{rules: [{match: {md.env: prod}, h: {X-Env: production}}]}'
g: https://httpbin.org/get
md: {env: prod}
1: b
EOF
{..."headers": {..."X-Env": "production"...}...}
```

Per-request headers override config:

```
$ echo '{g: https://httpbin.org/get, h: {X-Val: custom}, 1: b}' | jurl '{h: {X-Val: default}}'
{..."headers": {..."X-Val": "custom"...}...}
```

Config — two APIs with different tokens, matched by URL:

```bash
cat <<'EOF' > /tmp/multi-api.yaml
h:
  User-Agent: jurl/0.1
1: j(idx,md,s.code)

rules:
  - match: {u: "**httpbin.org**"}
    h: {a!: bearer!httpbin-token}
  - match: {u: "**jsonplaceholder**"}
    h: {a!: bearer!placeholder-token}
EOF

cat <<'EOF' | jurl "$(cat /tmp/multi-api.yaml)"
g: https://httpbin.org/get
md: {api: httpbin}
---
g: https://jsonplaceholder.typicode.com/posts/1
md: {api: placeholder}
EOF

# output
{"idx": 0, "md": {"api": "httpbin"}, "s": {"code": 200}}
{"idx": 1, "md": {"api": "placeholder"}, "s": {"code": 200}}
```

## Reference

Commented YAML schema by example, not a valid request.

### "Better HTTP" - request and response ([`yttp`](https://crates.io/crates/yttp))

Full specification: [yttp reference](https://github.com/ogheorghies/yttp#reference) — method shortcuts, header shortcuts (`a!`, `c!`, value shortcuts), auth, body encoding, and response formatting.

```yaml
# request
g: https://example.com                # method shortcuts: g p d, or full names
h: {a!: my-token, c!: j!}             # header key/value shortcuts expand in place
b: {city: Berlin}                     # body encoding follows Content-Type

# response — default output: y(s!,h,b)
s: {v: HTTP/1.1, c: 200, t: OK}       # s! → status inline object
h: {content-type: application/json}   # response (output) headers
b: {city: Berlin, lang: de}           # JSON → structured, UTF-8 → string, binary → base64
```

### jurl extensions

```yaml
# ============================
# METADATA
# ============================

md:                                  # arbitrary value, available in output and file path templates
  env: prod                          # {{md.env}}, md.env in j()/y()
  batch: 7                           # {{md.batch}}, md.batch in j()/y()

# ============================
# OUTPUT DESTINATIONS
# ============================

1: j(s!,h,b)                               # fd 1 (stdout) ← default for jurl
1: y(s!,h,b)                               # fd 1 (stdout) ← default for yurl
2: s                                       # fd 2 (stderr) ← raw status line
file://response.raw: b                     # file ← raw body
file://{{md.env}}/{{idx}}.json: j(s!,h,b)  # file ← templated path, auto-streamed (has {{idx}})
file://large.bin?stream: b                 # file ← explicit streaming (no buffering)

# ============================
# OUTPUT ATOMS
# ============================

# response (output):   b    h    s!   s    s.c  s.t  s.v  (or s.code s.text s.version)
#              or:     o.b  o.h  ...
# request (input):     i.b  i.h  i.s
# URL parts:           u.scheme  u.host  u.port  u.path  u.query  u.fragment
# other:               m    u    idx  md  md.*

# body encoding in j()/y():
#   JSON body   → embedded as structured value (object/array)
#   UTF-8 text  → embedded as string
#   binary      → base64-encoded string
#   (detect by type: object/array = JSON, string = text or base64, check h for Content-Type)

# output formats:
#   j(s!,h,b)  → JSON object with selected atoms
#   y(s!,h,b)  → YAML object with selected atoms
#   b          → raw (no wrapping)
```

### Batch config (middleware)

Passed as a CLI argument. Acts as middleware — applied to every request before it's sent.

```yaml
# --- Default headers ---
h:
  a!: bearer!my-token                # applied to all requests
  User-Agent: jurl/0.1

# --- Default output ---
1: j(idx, s.code)                    # applied when request has no output keys

# --- Concurrency ---
concurrency: 10                      # global max in-flight requests (default: 1)

# --- Progress ---
progress: true                       # spinner (unknown count)
progress: 100                        # progress bar (known count)
                                     # suppresses stderr output, shows warning count

# --- Rules ---
rules:
  - match: {u: "**slow-api**"}       # URL glob (* = segment, ** = any)
    concurrency: 2                   # per-endpoint concurrency limit

  - match: {m: POST}                 # method match (case-insensitive)
    h: {c!: f!}                      # add/override headers

  - match: {md.env: prod}            # metadata field match (exact)
    h: {X-Debug: "false"}

  - match: {m: POST, u: "**api.example.com**", md.env: prod}  # multiple criteria (AND)
    h: {a!: bearer!prod-token, c!: j!}

  - match: {u: "**api.openai.com**"}
    cache: true                      # shorthand: ttl=0, keys=[m,u,b], default dir
  - match: {u: "**api.example.com**"}
    cache:
      ttl: 3600                      # seconds (0 = no expiry)
      keys: [u, b, a]                # m u b a h h.<name>
      at: ./.cache                   # cache directory (default: ~/Library/Caches/yurl)

# merge order: config defaults → matching rules (in order) → per-request
```
