# Demo: yurl interactive mode
# Render with: vhs demo/demo.tape

Output demo/demo.gif

Set Shell "bash"
Set FontSize 14
Set Width 900
Set Height 500
Set Padding 10

# Ensure no history file exists for fresh experience
Hide
Type "rm -f ~/.local/share/yurl/history"
Enter
Sleep 500ms
Show

# Start yurl
Type "yurl"
Enter
Sleep 2000ms

# Press up arrow to use the suggested example
Up
Sleep 1500ms

# Execute it
Enter
Sleep 3000ms

# Now type a POST request
Sleep 4000ms
Type "{p: https://httpbin.org/post, b: {city: Berlin, lang: de}}"
Sleep 500ms
Enter
Sleep 3000ms

# Try Ctrl-C
Sleep 500ms
Ctrl+C
Sleep 1500ms

# Exit with Ctrl-D
Ctrl+D
Sleep 1000ms
