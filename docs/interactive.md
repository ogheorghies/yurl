# Interactive Mode

Interactive mode lets you step through requests one at a time, inspect them before sending, and manage config on the fly.

## Getting Started

Enter interactive mode in three ways:

```bash
yurl                                    # TTY detected → REPL
yurl -i                                 # force REPL (even with piped stdin)
cat requests.yaml | yurl -i             # REPL with piped requests as source
```

You'll see:

```
yurl v0.12.0

>
```

Type requests directly, or load them from a file:

```
> .open requests.yaml
  opened requests.yaml
```

## Stepping Through Requests

### .pop — get the next request

`.pop` (or `.p`) takes the next request from the source and pre-fills the prompt:

```
> .pop
> {g: api!/toys}█
```

The cursor is at the end — edit the request, then press **Enter** to send. Press **Ctrl-C** to discard.

### .repop — recall the last pop

If you discarded a request by accident:

```
> .pop
> {g: api!/toys}
  (Ctrl-C — discarded)
> .repop
> {g: api!/toys}█
```

`.repop` always returns the last popped request, whether it was sent or discarded.

### Editing before sending

The pre-filled text is fully editable. Add headers, change the URL, modify the body:

```
> .pop
> {g: api!/toys}
  (edit to add auth)
> {g: api!/toys, h: {a!: my-token}}
  (Enter to send)
```

## Inspecting Requests

### .x — expand and inspect

`.x [flags] {request}` expands shortcuts and shows the full request.

**Default** — horizontal YAML flow, pre-fills prompt for editing:

```
> .x {g: api!/toys}
> {get: https://api.example.com/toys}█
```

**Flags** compose freely:

| Flag | Effect |
|------|--------|
| `m` | merged — include config headers and rules |
| `v` | vertical — multiline output (prints, doesn't pre-fill) |
| `j` | JSON format |
| `c` | curl format |
| `s` | short headers (yttp shortcuts) |

**Examples:**

```
> .x m {g: api!/toys}
> {get: https://api.example.com/toys, h: {Authorization: Bearer tok}}█

> .x v {g: api!/toys}
get: https://api.example.com/toys

> .x mv {g: api!/toys}
get: https://api.example.com/toys
h:
  Authorization: Bearer tok

> .x c {g: api!/toys}
curl -X GET 'https://api.example.com/toys'

> .x mc {g: api!/toys}
curl -X GET 'https://api.example.com/toys' -H 'Authorization: Bearer tok'

> .x vc {g: api!/toys, h: {a!: tok}}
curl -X GET 'https://api.example.com/toys' \
  -H 'Authorization: Bearer tok'

> .x ms {g: api!/toys}
> {get: https://api.example.com/toys, h: {a!: bearer!tok}}█
```

Horizontal output pre-fills the prompt — edit and send. Vertical and curl output prints to screen.

### Combining .pop and .x

Pop a request, then expand it before sending:

```
> .pop
> {g: api!/toys}
  (Ctrl-A to go to start, prepend .x m)
> .x m {g: api!/toys}
> {get: https://api.example.com/toys, h: {Authorization: Bearer tok}}█
  (Enter to send)
```

## Managing Config

### .c — show and replace config

Show current config:

```
> .c
  config: api: api | h: 2 headers | output: 1
```

Replace config mid-session:

```
> .c {api: https://staging.example.com, h: {a!: staging-token}}
  config: api: api | h: 1 header
```

Config affects all subsequent requests — piped, popped, or ad-hoc.

### Config from CLI

Pass config as the first positional arg:

```bash
yurl -i '{api: https://api.example.com, h: {a!: bearer!$TOKEN}}'
```

Then `.c` shows it, `.x m` merges it into expansions, and all requests use it.

## Batch Execution

### .go — run remaining requests

`.go` (or `.g`) reads and executes all remaining requests from the source:

```
> .go
{"s":"200 OK","b":[{"id":1,"name":"Fox"}]}
{"s":"200 OK","b":{"id":1,"name":"Fox"}}
{"s":"201 Created","b":{"id":3,"name":"Owl"}}
  3 requests executed
```

### Ctrl-C — cancel

Press **Ctrl-C** during a request to cancel it:

- While a request is in flight (spinner showing): cancels the HTTP request, returns to prompt
- During `.go`: stops execution, returns to prompt
- At the prompt with a pre-fill: discards the pre-filled text

## Reference

| Command | Description |
|---|---|
| `.open file` | open requests from file |
| `.pop` / `.p` | pop next request, edit, Enter to send |
| `.repop` | re-pop last popped request |
| `.go` / `.g` | run all remaining, Ctrl-C to stop |
| `.x [flags] {req}` | expand/inspect (`.help x` for flag reference) |
| `.c` | show config |
| `.c {cfg}` | replace config |
| `.t` | show request templates |
| `.ref` / `.r` | show reference card |
| `.help` / `.h` | show help |
| **Ctrl-C** | cancel request or discard pre-fill |
| **Ctrl-D** | exit |
| **Up/Down** | navigate history |

## Workflow Examples

### Debugging an API

```bash
yurl -i '{api: https://api.example.com/v1, h: {a!: bearer!$TOKEN}}'
```

```
> .x m {g: api!/users/1}                    # inspect the full request
> {get: https://api.example.com/v1/users/1, h: {Authorization: Bearer tok123}}
  (looks good — Enter to send)
{"s":{"c":200},"b":{"id":1,"name":"Alice"}}

> {p: api!/users, b: {name: Bob}}           # ad-hoc POST
{"s":{"c":201},"b":{"id":2,"name":"Bob"}}
```

### Stepping through a test file

```bash
yurl -i '{api: https://staging.example.com, h: {a!: bearer!test-tok}}'
```

```
> .open tests/api-tests.yaml
  loaded tests/api-tests.yaml

> .pop                                       # first request
> {g: api!/health}
  (Enter)
{"s":{"c":200},"b":"ok"}

> .pop                                       # second request
> {p: api!/users, b: {name: Test}}
  (edit the body)
> {p: api!/users, b: {name: "Test User", role: admin}}
  (Enter)
{"s":{"c":201},"b":{"id":42,"name":"Test User","role":"admin"}}

> .go                                        # run the rest
{"s":{"c":200},...}
{"s":{"c":204}}
  2 requests executed
```

### Switching environments

```
> .c
  config: api: api (staging)

> .c {api: https://api.example.com, h: {a!: bearer!prod-tok}}
  config: api: api | h: 1 header

> {g: api!/users/1}                          # now hits production
```
