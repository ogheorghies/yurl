# Demo: yurl interactive CRUD against a REST API
# Render with: vhs demo/demo-crud.tape
#
# Shows: auth config, X-Verbose header, CRUD operations against a faux REST API

Output demo/demo-crud.gif

Set Shell "bash"
Set FontSize 13
Set Width 600
Set Height 550
Set Padding 10

# Build and start demo server silently
Hide
Type "export PS1='$ '"
Enter
Sleep 200ms
Type "pkill -f demo-server 2>/dev/null; sleep 0.5"
Enter
Sleep 1000ms
Type "cargo build --example demo-server 2>/dev/null && cargo build 2>/dev/null"
Enter
Sleep 5000ms
Type "target/debug/examples/demo-server >/dev/null 2>&1 &"
Enter
Sleep 1000ms
Type "export FORCE_COLOR=1 PASS=secret PATH=$(pwd)/target/debug:$PATH"
Enter
Sleep 200ms
Type "clear"
Enter
Sleep 500ms
Show

# Launch yurl with auth + verbose header config
Type `yurl '{api: 127.0.0.1:3456, h: {a!: [admin, $PASS], X-Debug: 1}}'`
Enter
Sleep 2500ms

# List all items
Type "{g: api!/toys}"
Enter
Sleep 2500ms

# Get a single item
Type "{g: api!/toys/1}"
Enter
Sleep 2500ms

# Create a new item
Type "{p: api!/toys, b: {name: Owl, price: 5.99}}"
Enter
Sleep 2500ms

# Update an item
Type "{put: api!/toys/1, b: {name: Fox, price: 12.99}}"
Enter
Sleep 2500ms

# Delete an item
Type "{d: api!/toys/2}"
Enter
Sleep 2500ms

# Verify final state
Type "{g: api!/toys}"
Enter
Sleep 3000ms

# Exit
Ctrl+D
Sleep 1000ms

Hide
Type "kill %1 2>/dev/null"
Enter
Sleep 500ms
Show
