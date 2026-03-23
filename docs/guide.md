# yurl Guide

Tutorial-style explanations for yurl features. For quick reference, see [README](../README.md). For worked examples, see [Cookbook](cookbook.md).

![demo](demo-crud.gif)

## Reading Requests

Requests are read from stdin as JSONL (one per line) or YAML (separated by `---`). Stdin is consumed as a stream — requests execute before EOF, supporting slow producers like `tail -f requests.jsonl | yurl`.

Single-line yttp/JSON:
```
echo '{g: https://httpbin.org/get}' | yurl
```

Multiline YAML for readability:
```bash
cat <<'EOF' | jurl
p: https://httpbin.org/post
h:
  a!: my-token
  c!: f!
  Accept: j!
  X-Request-Id: abc-123
b:
  city: Berlin
  lang: de
md:
  env: prod
  batch: 7
1: j(h)
2: s
file://{{md.env}}/{{idx}}.raw: b
file://out.yaml: y(i.h o.h)
EOF
```

The HTTP method key holds the URL. Any capitalization is accepted; `g`, `p`, `d` are shortcuts for get, post, delete. URLs without a scheme are auto-detected: `localhost`, `127.0.0.1`, `[::1]`, and bare hostnames get `http://`, everything else gets `https://`.

## Authorization

The `a!` (or `auth!`) key inside `h` sets the `Authorization` header. The scheme is inferred:

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

**Other schemes** — if the string already contains a space, it's passed through:
```
echo '{g: https://httpbin.org/get, h: {a!: Digest abc123}}' | jurl
# → Authorization: Digest abc123
```

The explicit `basic!` and `bearer!` value prefixes also work: `a!: basic!user:pass`, `a!: bearer!tok`.

In YAML config or requests:
```yaml
h:
  a!: my-token          # bearer (most common)
  a!: [user, pass]      # basic
  a!: Digest abc123     # explicit scheme
```

`a!` works inside `h:` in requests, config defaults, and rules.

## Body Encoding

The Content-Type header determines how `b` is encoded. JSON is the default.

Form encoding:
```bash
cat <<'EOF' | jurl
p: https://httpbin.org/post
h: {c!: f!}
b: {city: Berlin, lang: de}
1: b
EOF
```

Multipart with file uploads:
```bash
cat <<'EOF' | jurl
p: https://httpbin.org/post
h: {c!: m!}
b:
  desc: a photo
  file: file:///tmp/img.png
1: b
EOF
```

## Environment Variables

Config header values can reference environment variables with `$VAR`:

```yaml
h:
  a!: $API_TOKEN           # bearer auth from env
  a!: [admin, $DB_PASS]    # basic auth, password from env
  X-Api-Key: $API_KEY      # custom header from env
```

Only pure `$VAR` values are expanded — the entire string must be `$` followed by alphanumeric/underscore characters. This keeps credentials out of CLI args and shell history.

## Output Routing

Destinations: `"1"` (stdout), `"2"` (stderr), or `"file://path"` (supports `{{atom}}` templates). Multiple destinations per request.

`j(...)` wraps atoms as JSON, `y(...)` as YAML. Default: `j(s! h b)` for jurl, `y(s! h b)` for yurl.

Body encoding inside `j()`/`y()`: JSON body becomes structured value, UTF-8 text becomes string, binary becomes base64. Outside `j()`/`y()`, `b` is raw bytes.

Templated file paths: `{{idx}}`, `{{md.*}}`, `{{u.host}}`, etc.

```bash
cat <<'EOF' | jurl
g: https://httpbin.org/get
1: j(h)
2: s
file://body.out: b
file://./out/{{u.host}}/{{m}}.txt: b
EOF
```

## Batch Config

Shared settings for all stdin requests, passed as a CLI argument:

```bash
cat <<'EOF' > /tmp/api-config.yaml
h:
  a!: bearer!my-token
  User-Agent: jurl/0.1
1: j(idx md s.code)

rules:
  - match: {m: POST}
    h: {c!: f!}
  - match: {md.env: prod}
    h: {X-Debug: "false"}
EOF

cat <<'EOF' | jurl "$(cat /tmp/api-config.yaml)"
g: https://httpbin.org/get
md: {env: prod}
---
p: https://httpbin.org/post
b: {city: Berlin, lang: de}
EOF
```

Merge order: config defaults -> matching rules (in order) -> per-request. Per-request headers override config. Per-request output keys fully replace (not merge with) config defaults.

## Rules

Rules conditionally add headers based on URL, method, or metadata:

```yaml
rules:
  - match: {u: "**httpbin.org**"}
    h: {X-Custom: "yes"}
  - match: {m: POST}
    h: {c!: j!}
  - match: {md.env: prod}
    h: {X-Debug: "false"}
  - match: {m: POST, u: "**api.example.com**", md.env: prod}   # AND
    h: {a!: bearer!prod-token}
```

URL glob: `*` matches non-`/`, `**` matches anything. Method match is case-insensitive. Multiple criteria are ANDed.

## API Aliases

Define named base URLs in config:

```yaml
api: https://api.example.com/v1          # single alias, used as api!
api:                                      # multiple named aliases
  prod: https://api.example.com/v1
  staging: https://staging.example.com/v1
```

Use `name!/path` in request URLs:
```
{g: api!/toys}           # -> GET https://api.example.com/v1/toys
{g: staging!/toys}       # -> GET https://staging.example.com/v1/toys
```

If `name` doesn't match any alias, the URL is used unchanged.

## Step Mode

The `-i` flag enables interactive debugging of piped requests. You can also load requests from a file mid-session with `.step file.yaml`.

```
cat requests.yaml | yurl -i '{api: api.example.com/v1, h: {a!: my-token}}'
```

This enters the REPL with piped requests available via `.next`/`.go`. Commands:

- **`.step file`** (`.s file`) — loads requests from a file for stepping through.
- **`.next`** (`.n`) — loads the next request into the editor for review/edit. Press Enter to send, Ctrl-C to skip.
- **`.go`** (`.g`) — executes all remaining requests. Ctrl-C breaks back to the prompt.
- **`.x {request}`** — expands a request with full config resolution (API aliases, header shortcuts, env vars, rule merging) and presents the result for review. Press Enter to send, Ctrl-C to discard. Combine with `.next`: press Ctrl-A and prepend `.x ` to expand a queued request.
- **`.c`** — shows the current config summary. **`.c {config}`** replaces the active config. Subsequent requests and `.x` expansions use the new config.
- **`.help`** (`.h`) — shows help.

You can also type ad-hoc requests at any time.

Example session:

```
$ echo '
{g: api!/toys}
{g: api!/toys/1}
{p: api!/toys, b: {name: Owl}}
' | yurl -i '{api: localhost:3000, h: {a!: bearer!tok}, 1: "j(s b)"}'

yurl v0.5.0

> .c
  config: api: api | h: 1 header | output: 1

> .next                              # pre-fills with {g: api!/toys}
> .x {g: api!/toys}                  # Ctrl-A, prepend .x to expand
> {"get":"http://localhost:3000/toys","h":{"Authorization":"Bearer tok"},"1":"j(s b)"}
{"s":"200 OK","b":[{"id":1,"name":"Fox"},{"id":2,"name":"Cat"}]}

> .go                                # run remaining 2 requests
{"s":"200 OK","b":{"id":1,"name":"Fox","price":12.99}}
{"s":"201 Created","b":{"id":3,"name":"Owl"}}
  2 requests executed

> .c {api: {s: staging.example.com}}
  config: api: s

> {g: s!/toys}                       # ad-hoc request with new config
{"s":"200 OK","b":[...]}
```

## Concurrency and Streaming

Set `concurrency` in batch config for parallel requests:

```
jurl '{concurrency: 10}'
```

Per-endpoint limits via rules — a request must hold a permit from the global semaphore **and** from every matching rule:

```yaml
concurrency: 10
rules:
  - match: {u: "**slow-api.com**"}
    concurrency: 2                   # at most 2 to slow-api.com
  - match: {m: POST}
    concurrency: 2                   # at most 2 POSTs globally
# A POST to slow-api.com needs both permits -> at most 2 concurrent
```

Buffering behavior with concurrency > 1:
- **File with `{{idx}}`** — auto-streamed (guaranteed unique), constant memory
- **File without `{{idx}}`** — buffered, written atomically. Override with `?stream` if you know the path is unique
- **stdout/stderr** — buffered and flushed atomically to prevent interleaving

With `concurrency: 1` (default), everything auto-streams — no OOM risk for large payloads.

```bash
cat <<'EOF' | jurl
g: https://example.com/large.bin
file://./large.bin?stream: b
1: j(s)
EOF
```

When streaming, the body is written chunk-by-chunk. If a non-streaming destination also needs the body, it is still buffered for that destination.

## Caching

Rules can cache responses in a local SQLite database:

```yaml
rules:
  - match: {u: "**api.openai.com**"}
    cache: true
```

`cache: true` is shorthand for `{ttl: 0, keys: [m, u, b], at: ~/Library/Caches/yurl}` (macOS) or `~/.cache/yurl` (Linux).

The cache key is SHA-256 of selected request parts. Configuration options:

```yaml
cache:
  ttl: 3600              # seconds (0 = no expiry)
  keys: [u, b, a]        # add 'a' to separate caches per API key
  at: ./.cache           # project-local cache directory
```

Expired entries are cleaned up on startup. Delete the cache directory to clear entirely.

> **Note:** This is application-level caching, not HTTP-compliant. It does not respect `Cache-Control`, `ETag`, or `Vary` headers. Useful for memoizing API calls (e.g. LLM endpoints) but not as a general HTTP cache.

## Progress

```
jurl '{progress: true}'              # spinner (unknown count)
jurl '{progress: 100, concurrency: 10}'   # progress bar (known count)
```

When progress is active, stderr output (`"2"`) is suppressed. A warning line shows how many requests had their stderr output suppressed.
