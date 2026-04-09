# yurl Cookbook

Worked examples. For reference, see [README](../README.md). For explanations, see [Guide](guide.md).

Examples use `jurl` for compact JSON output. `yurl` produces YAML by default; use `1: j(...)` or `1: b` to get JSON or raw body.

## Basics

```bash
# default output — JSON with status, headers, body
echo '{g: https://httpbin.org/get}' | jurl

# raw body to stdout
echo '{g: https://httpbin.org/get, 1: b}' | jurl

# just the status line
echo '{g: https://httpbin.org/get, 1: s}' | jurl
# HTTP/1.1 200 OK

# status code and text as JSON
echo '{g: https://httpbin.org/get, 1: j(s.code s.text)}' | jurl
# {"s": {"c": 200, "t": "OK"}}
```

## POST Requests

```bash
# JSON body (default encoding)
echo '{p: https://httpbin.org/post, b: {key: val}, 1: b}' | jurl

# form POST
echo '{p: https://httpbin.org/post, h: {c!: f!}, b: {city: Berlin}, 1: b}' | jurl

# multipart upload
echo '{p: https://httpbin.org/post, h: {c!: m!}, b: {desc: test, file: file:///tmp/f.txt}, 1: b}' | jurl
```

## Authentication

```bash
# bearer (string token)
echo '{g: https://httpbin.org/get, h: {a!: my-token}, 1: b}' | jurl

# basic (array)
echo '{g: https://httpbin.org/get, h: {a!: [user, pass]}, 1: b}' | jurl

# explicit scheme
echo '{g: https://httpbin.org/get, h: {a!: Digest abc123}, 1: b}' | jurl
```

## Header Shortcuts

```bash
# MIME prefix shortcuts
echo '{g: https://httpbin.org/get, h: {Accept: a!/xml}, 1: b}' | jurl   # application/xml
echo '{g: https://httpbin.org/get, h: {Accept: t!/csv}, 1: b}' | jurl   # text/csv
echo '{g: https://httpbin.org/get, h: {Accept: i!/png}, 1: b}' | jurl   # image/png

# named shortcuts
echo '{g: https://httpbin.org/get, h: {Accept: x!}, 1: b}' | jurl      # application/xml
echo '{g: https://httpbin.org/get, h: {Accept: h!}, 1: b}' | jurl      # text/html
echo '{g: https://httpbin.org/get, h: {Accept: t!}, 1: b}' | jurl      # text/plain
```

## Output Routing

```bash
# headers to stdout, status to stderr, body to file
cat <<'EOF' | jurl
g: https://httpbin.org/get
1: j(h)
2: s
file://body.out: b
EOF

# templated file path — use block-style stdin so {{...}} doesn't collide
# with flow-style YAML braces
cat <<'EOF' | jurl
g: https://httpbin.org/get
file://./out/{{u.host}}/{{m}}.txt: b
EOF
```

## Metadata

```bash
# scalar metadata
echo '{g: https://httpbin.org/get, md: batch-1, 1: j(idx md s.code)}' | jurl
# {"idx": 0, "md": "batch-1", "s": {"code": 200}}

# object metadata with field selection
echo '{g: https://httpbin.org/get, md: {id: 42, tag: test}, 1: j(md.id)}' | jurl
# {"md": {"id": 42}}
```

## Batch Requests

```bash
# JSONL with idx
printf '{g: https://httpbin.org/get}\n{g: https://httpbin.org/get}\n' | jurl '{1: j(idx s.code)}'
# {"idx": 0, "s": {"code": 200}}
# {"idx": 1, "s": {"code": 200}}

# per-request output overrides config default
printf '{g: https://httpbin.org/get}\n{g: https://httpbin.org/get, 1: s}\n' | jurl '{1: j(idx s.code)}'
# {"idx": 0, "s": {"code": 200}}
# HTTP/1.1 200 OK
```

## Config Recipes

```bash
# default auth for all requests
echo '{g: https://httpbin.org/get, 1: b}' | jurl '{h: {a!: bearer!session-tok}}'

# form encoding for all POSTs
echo '{p: https://httpbin.org/post, b: {x: "1"}, 1: b}' | jurl '{rules: [{match: {m: POST}, h: {c!: f!}}]}'

# metadata-based header injection
cat <<'EOF' | jurl '{rules: [{match: {md.env: prod}, h: {X-Env: production}}]}'
g: https://httpbin.org/get
md: {env: prod}
1: b
EOF

# per-request headers override config
echo '{g: https://httpbin.org/get, h: {X-Val: custom}, 1: b}' | jurl '{h: {X-Val: default}}'

# two APIs with different tokens
cat <<'EOF' > /tmp/multi-api.yaml
h: {User-Agent: jurl/0.1}
1: j(idx md s.code)
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
# {"idx": 0, "md": {"api": "httpbin"}, "s": {"code": 200}}
# {"idx": 1, "md": {"api": "placeholder"}, "s": {"code": 200}}
```

## Streaming

```bash
# {{idx}} in path — auto-streams, no ?stream needed.
# Block-style YAML stdin lets you use {{idx}} as-is; in a flow-style
# positional arg the '{' would need quoting (see note in README).
cat <<'EOF' | jurl
g: https://example.com/file1.bin
file://./downloads/{{idx}}.bin: b
1: j(s)
---
g: https://example.com/file2.bin
file://./downloads/{{idx}}.bin: b
1: j(s)
EOF

# static path — use ?stream to bypass buffering
cat <<'EOF' | jurl
g: https://example.com/large.bin
file://./large.bin?stream: b
1: j(s)
EOF
```
