# jurl

`jurl` is an HTTP client focused on clarity, useful in one-off and batch mode.
The `j` comes from JSON, since the default representation of requests, responses, and middleware is JSON.

It offers sensible shortcuts for common headers, flexible output routing, and rule-based middleware.

Install with: `cargo install jurl`

```
echo '{"g": "https://httpbin.org/get"}' | jurl | yq -P

# response
b: ewogICJ...XQiCn0K                     # response body, by default base64 - can be changed
h:                                       # response headers (edited)
  date: Tue, 17 Mar 2026 11:26:31 GMT
  server: gunicorn/19.9.0
s: HTTP/1.1 200 OK                       # raw status line - can be changed
```

For complex requests, YAML via `yq` provides extra readability:

```bash
cat <<'EOF' | yq -o json | jurl
p: https://httpbin.org/post      # HTTP method shortcut + URL

h:                               # request headers
  a!: bearer!my-token            # or auth!: Authorization: Bearer my-token
  c!: f!                         # or ct!:   Content-Type: application/x-www-form-urlencoded
  Accept: j!                     # or json! or a!/json → application/json
  X-Request-Id: abc-123
b:                               # request body, form-encoded per c!:
  user: alice
  pass: secret

md:                              # metadata, referenced into output
  env: prod
  batch: 7

1: j(h)                          # stdout ← headers as JSON
2: s                             # stderr ← raw status line
file://{{md.env}}/{{idx}}.raw: b # file ← raw body (not base64), path templated
EOF
```

Shared settings for batched requests can be passed as a CLI argument:

```bash
# convert config to JSON
cat <<'EOF' | yq -o json > /tmp/api-config.json
a!: bearer!my-token                    # add Authorization header for all requests
h:                                     # add headers for all requests
  User-Agent: jurl/0.1
1: j(idx,md,s.code)                    # custom output format: index, metadata, status code

rules:                                 # conditional header overrides
  - match: {m: POST}                   # all POST requests →
    c!: f!                             #   use form encoding
  - match: {md.env: prod}              # requests tagged env: prod →
    h:                                 #   add debug header
      X-Debug: "false"
EOF

# send requests — each YAML document (---) becomes one JSONL line
cat <<'EOF' | yq -o json -I0 | jurl "$(cat /tmp/api-config.json)"
g: https://httpbin.org/get             # GET, tagged as prod
md: {env: prod}
---
p: https://httpbin.org/post            # POST, body form-encoded by rule
b: {user: alice, pass: secret}
---
g: https://httpbin.org/get             # GET, tagged as staging
md: {env: staging}
EOF
```

## Request

Reads JSON requests from stdin (one per line, JSONL).
The HTTP method key holds the URL. Any capitalization is accepted; `g`, `p`, `d` are shortcuts for get, post, delete.

```
echo '{"p": "https://httpbin.org/post", "b": {"key": "val"}}' | jurl
```

### Request keys

- HTTP method (`get`, `post`, `put`, `delete`, `patch`, `head`, `options`, `trace`) — URL
- `h` / `headers` — request headers object (values support shortcuts, see below)
- `b` / `body` — request body (encoding determined by Content-Type)
- `c!` / `ct!` — shortcut for Content-Type header (also works inside `h`)
- `a!` / `auth!` — shortcut for Authorization header (also works inside `h`)
- `md` — arbitrary metadata (any JSON value), echoed into output

### Body encoding

The `Content-Type` header determines how `b` is encoded:

- `application/json` (default) — JSON body
- `application/x-www-form-urlencoded` — form encoding (`b` object becomes `key=value&...`)
- `multipart/form-data` — multipart encoding; values starting with `file://` are read from disk

```
echo '{"p": "https://httpbin.org/post", "c!": "f!", "b": {"user": "alice", "pass": "secret"}}' | jurl

echo '{"p": "https://httpbin.org/post", "c!": "m!", "b": {"desc": "a photo", "file": "file:///tmp/img.png"}}' | jurl
```

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
echo '{"g": "https://httpbin.org/get", "a!": "basic!user:pass"}' | jurl
echo '{"g": "https://httpbin.org/get", "h": {"Accept": "a!/xml"}}' | jurl
```

## Output

By default, jurl writes `j(b,h,s)` (JSON with base64 body, headers, and status) to stdout. 
To customize output, add to the request JSON key-value pairs like so:
- key: the destination — `"1"` (stdout), `"2"` (stderr), or `"file://path"` (supports `{{atom}}` templates)
- value: what to write — a raw atom like `b` or `s`, or `j(...)` to output them as JSON

Multiple destinations can be used in a single request. In the unlikely case that the files resolve to the same name, the last value wins.

### Format

Atoms reference parts of the response (or request):

- `b` — response body (raw bytes outside `j()`, base64-encoded inside `j()`)
- `h` — response headers (raw HTTP format outside `j()`, JSON object inside `j()`)
- `s` — response status line (raw outside `j()`, string inside `j()`); `s.code`, `s.text`, `s.version` for parts
- `m` — request method
- `u` — full request URL
- `idx` — auto-incrementing request index (0-based)
- `md` — metadata (entire value); `md.x`, `md.y` → grouped as `"md": {"x": ..., "y": ...}`
- `ab`, `ah`, `as` — request body, headers, status (same encoding rules)

URL parts and metadata are available for file path templates: `u.scheme`, `u.host`, `u.port`, `u.path`, `u.query`, `u.fragment`, `idx`, `md`, `md.*`.

`j(...)` wraps atoms into a JSON object.

Default output (when no destination key is present): `{"1": "j(b,h,s)"}`

### Examples

```
echo '{"g": "https://httpbin.org/get"}' | jurl | jq -r .b | base64 -d

echo '{"g": "https://httpbin.org/get", "1": "b"}' | jurl

echo '{"g": "https://httpbin.org/get", "1": "j(s.code,s.text)"}' | jurl

echo '{"g": "https://httpbin.org/get", "file://./out/{{u.host}}/{{m}}.txt": "b"}' | jurl
```

## Batch config

An optional CLI argument provides shared configuration for all requests: default headers, output format, and conditional rules.

```
jurl '{"a!": "bearer!tok"}'
```

All stdin requests inherit these headers. Per-request headers override config headers.
Shortcuts (`c!`/`ct!`, `a!`/`auth!`, value shortcuts) work in config and rules too.

### Rules

Rules conditionally add headers based on URL, method, or metadata matching.

```
jurl '{
  "h": {"User-Agent": "jurl/0.1"},
  "rules": [
    {"match": {"u": "**httpbin.org**"}, "h": {"X-Custom": "yes"}},
    {"match": {"m": "POST"}, "c!": "j!"},
    {"match": {"md.env": "prod"}, "h": {"X-Debug": "false"}}
  ]
}'
```

- `u` — URL glob (`*` matches non-`/`, `**` matches anything)
- `m` — HTTP method (exact, case-insensitive)
- `md.<field>` — exact metadata field match

Merge order: config defaults → matching rules (in order) → per-request headers.

## Cookbook

Simple GET — default output is JSON with base64 body, headers, status:

```
$ echo '{"g": "https://httpbin.org/get"}' | jurl
{"b": "eyJhcmdzIjp7fSwi...", "h": {"content-type": "application/json", ...}, "s": "HTTP/1.1 200 OK"}
```

Raw body to stdout:

```
$ echo '{"g": "https://httpbin.org/get", "1": "b"}' | jurl
{"args": {}, "headers": {"Host": "httpbin.org", ...}, "url": "https://httpbin.org/get"}
```

Just the status line:

```
$ echo '{"g": "https://httpbin.org/get", "1": "s"}' | jurl
HTTP/1.1 200 OK
```

Status code and text as JSON:

```
$ echo '{"g": "https://httpbin.org/get", "1": "j(s.code,s.text)"}' | jurl
{"s": {"code": 200, "text": "OK"}}
```

POST with JSON body (default encoding). `c!` is optional since JSON is the default, but `json!` / `j!` work:

```
$ echo '{"p": "https://httpbin.org/post", "b": {"key": "val"}, "1": "b"}' | jurl
$ echo '{"p": "https://httpbin.org/post", "c!": "j!", "b": {"key": "val"}, "1": "b"}' | jurl
{..."json": {"key": "val"}...}
```

Form POST — full header, then with `form!` / `f!`:

```bash
# full Content-Type header
$ echo '{"p": "https://httpbin.org/post", "h": {"Content-Type": "application/x-www-form-urlencoded"}, "b": {"user": "alice"}, "1": "b"}' | jurl

# shortcut
$ echo '{"p": "https://httpbin.org/post", "c!": "f!", "b": {"user": "alice"}, "1": "b"}' | jurl

# same thing in YAML
$ cat <<'EOF' | yq -o json | jurl
p: https://httpbin.org/post
c!: f!
b: {user: alice}
1: b
EOF

# output (all three)
{..."form": {"user": "alice"}...}
```

Multipart upload — full header, then with `multi!` / `m!`:

```bash
# full Content-Type header
$ echo '{"p": "https://httpbin.org/post", "h": {"Content-Type": "multipart/form-data"}, "b": {"desc": "test", "file": "file:///tmp/f.txt"}, "1": "b"}' | jurl

# shortcut
$ echo '{"p": "https://httpbin.org/post", "c!": "m!", "b": {"desc": "test", "file": "file:///tmp/f.txt"}, "1": "b"}' | jurl

# YAML
$ cat <<'EOF' | yq -o json | jurl
p: https://httpbin.org/post
c!: m!
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
$ echo '{"g": "https://httpbin.org/get", "h": {"Authorization": "Basic dXNlcjpwYXNz"}, "1": "b"}' | jurl
$ echo '{"g": "https://httpbin.org/get", "a!": "basic!user:pass", "1": "b"}' | jurl
{..."headers": {..."Authorization": "Basic dXNlcjpwYXNz"...}...}
```

Bearer auth — full header, then `bearer!` shortcut:

```
$ echo '{"g": "https://httpbin.org/get", "h": {"Authorization": "Bearer tok123"}, "1": "b"}' | jurl
$ echo '{"g": "https://httpbin.org/get", "a!": "bearer!tok123", "1": "b"}' | jurl
{..."headers": {..."Authorization": "Bearer tok123"...}...}
```

MIME prefix shortcuts — `a!/`, `t!/`, `i!/`:

```
$ echo '{"g": "https://httpbin.org/get", "h": {"Accept": "a!/xml"}, "1": "b"}' | jurl
{..."headers": {..."Accept": "application/xml"...}...}

$ echo '{"g": "https://httpbin.org/get", "h": {"Accept": "t!/csv"}, "1": "b"}' | jurl
{..."headers": {..."Accept": "text/csv"...}...}

$ echo '{"g": "https://httpbin.org/get", "h": {"Accept": "i!/png"}, "1": "b"}' | jurl
{..."headers": {..."Accept": "image/png"...}...}
```

Named shortcuts — long and short forms:

```
$ echo '{"g": "https://httpbin.org/get", "h": {"Accept": "x!"}, "1": "b"}' | jurl
{..."headers": {..."Accept": "application/xml"...}...}

$ echo '{"g": "https://httpbin.org/get", "h": {"Accept": "h!"}, "1": "b"}' | jurl
{..."headers": {..."Accept": "text/html"...}...}

$ echo '{"g": "https://httpbin.org/get", "h": {"Accept": "t!"}, "1": "b"}' | jurl
{..."headers": {..."Accept": "text/plain"...}...}
```

Metadata — scalar, object, and field selection:

```bash
$ echo '{"g": "https://httpbin.org/get", "md": "batch-1", "1": "j(idx,md,s.code)"}' | jurl
{"idx": 0, "md": "batch-1", "s": {"code": 200}}

# YAML with metadata object
$ cat <<'EOF' | yq -o json | jurl
g: https://httpbin.org/get
md:
  id: 42
  tag: test
1: j(md)
EOF
{"md": {"id": 42, "tag": "test"}}

# selecting specific metadata fields
$ echo '{"g": "https://httpbin.org/get", "md": {"id": 42, "tag": "test"}, "1": "j(md.id)"}' | jurl
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
$ printf '{"g":"https://httpbin.org/get"}\n{"p":"https://httpbin.org/post","b":{"x":"1"}}\n' | jurl '{"1": "j(idx,m,s.code)"}'
{"idx": 0, "m": "GET", "s": {"code": 200}}
{"idx": 1, "m": "POST", "s": {"code": 200}}
```

Per-request output overrides the config default:

```
$ printf '{"g":"https://httpbin.org/get"}\n{"g":"https://httpbin.org/get","1":"s"}\n' | jurl '{"1": "j(idx,s.code)"}'
{"idx": 0, "s": {"code": 200}}
HTTP/1.1 200 OK
```

Multiple destinations — body to file, headers to stdout, status to stderr:

```bash
$ cat <<'EOF' | yq -o json | jurl
g: https://httpbin.org/get
1: j(h)                          # stdout ← headers as JSON
2: s                             # stderr ← raw status line
file://body.out: b               # file   ← raw body
EOF

# stdout
{"h": {"content-type": "application/json", ...}}
# stderr
HTTP/1.1 200 OK
# body.out contains the raw response body
```

Templated file output:

```bash
$ cat <<'EOF' | yq -o json | jurl
g: https://httpbin.org/get
file://./out/{{u.host}}/{{m}}.txt: b
EOF
# writes response body to ./out/httpbin.org/GET.txt
```

Config — default auth for all requests:

```
$ echo '{"g": "https://httpbin.org/get", "1": "b"}' | jurl '{"a!": "bearer!session-tok"}'
{..."headers": {..."Authorization": "Bearer session-tok"...}...}
```

Config — rule adds form encoding to all POSTs:

```
$ echo '{"p": "https://httpbin.org/post", "b": {"x": "1"}, "1": "b"}' | jurl '{"rules": [{"match": {"m": "POST"}, "c!": "f!"}]}'
{..."form": {"x": "1"}...}
```

Config — rule matches metadata:

```bash
$ cat <<'EOF' | yq -o json | jurl '{"rules": [{"match": {"md.env": "prod"}, "h": {"X-Env": "production"}}]}'
g: https://httpbin.org/get
md: {env: prod}
1: b
EOF
{..."headers": {..."X-Env": "production"...}...}
```

Per-request headers override config:

```
$ echo '{"g": "https://httpbin.org/get", "h": {"X-Val": "custom"}, "1": "b"}' | jurl '{"h": {"X-Val": "default"}}'
{..."headers": {..."X-Val": "custom"...}...}
```

Config — two APIs with different tokens, matched by URL:

```bash
cat <<'EOF' | yq -o json > /tmp/multi-api.json
h:
  User-Agent: jurl/0.1
1: j(idx,md,s.code)

rules:
  - match: {u: "**httpbin.org**"}
    a!: bearer!httpbin-token
  - match: {u: "**jsonplaceholder**"}
    a!: bearer!placeholder-token
EOF

cat <<'EOF' | yq -o json -I0 | jurl "$(cat /tmp/multi-api.json)"
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
