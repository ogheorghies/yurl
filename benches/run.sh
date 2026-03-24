#!/usr/bin/env bash
set -euo pipefail

# yurl throughput benchmark — side-by-side comparison with wrk and curl --parallel.
# Requires: wrk, curl (7.66+), mockinx (or cargo-built mockinx), yurl (release build).
# Usage: ./benches/run.sh [requests=10000]

REQUESTS=${1:-10000}
DURATION=10  # wrk duration in seconds

# ── Find binaries ────────────────────────────────────────────────────

find_bin() {
    if command -v "$1" &>/dev/null; then
        command -v "$1"
    elif [ -f "../mockinx/target/release/$1" ]; then
        echo "../mockinx/target/release/$1"
    elif [ -f "target/release/$1" ]; then
        echo "target/release/$1"
    else
        echo ""
    fi
}

WRK=$(find_bin wrk)
MOCKINX=$(find_bin mockinx)
CURL=$(command -v curl 2>/dev/null || echo "")
[ -z "$WRK" ] && { echo "error: wrk not found (brew install wrk)"; exit 1; }
[ -z "$MOCKINX" ] && { echo "error: mockinx not found (cargo install mockinx or build ../mockinx)"; exit 1; }
[ -z "$CURL" ] && { echo "error: curl not found"; exit 1; }

if [ ! -f "target/release/yurl" ]; then
    echo "building yurl (release)..."
    cargo build --release --bin yurl 2>/dev/null
fi
YURL=target/release/yurl

# ── Start mockinx ────────────────────────────────────────────────────

PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("",0)); print(s.getsockname()[1]); s.close()')

$MOCKINX "$PORT" &
MX_PID=$!
TMPFILE=$(mktemp)
CURL_CFG=$(mktemp)
cleanup() { kill $MX_PID 2>/dev/null; wait $MX_PID 2>/dev/null; rm -f "$TMPFILE" "$CURL_CFG"; }
trap cleanup EXIT

BASE="http://127.0.0.1:$PORT"

for i in $(seq 1 20); do
    if curl -s "$BASE/_mx" >/dev/null 2>&1; then break; fi
    sleep 0.1
done

curl -s -X PUT "$BASE/_mx" \
    -H 'Content-Type: application/json' \
    -d '[{"match":{"g":"/bench"},"reply":{"s":200,"b":"ok"}}]' >/dev/null

echo "mockinx on port $PORT | requests: $REQUESTS | wrk duration: ${DURATION}s"
echo ""

# ── Concurrency levels ───────────────────────────────────────────────
# (concurrency, wrk_threads)
LEVELS="1:1 10:1 50:2 100:4"

# ── Run functions ────────────────────────────────────────────────────

run_wrk() {
    local conc=$1 threads=$2
    $WRK -t"$threads" -c"$conc" -d${DURATION}s "$BASE/bench" 2>&1 | grep 'Requests/sec' | awk '{print $2}'
}

run_curl() {
    local conc=$1
    # Generate curl config file: url + output per request
    python3 -c "
for _ in range($REQUESTS):
    print('url = $BASE/bench')
    print('output = /dev/null')
" > "$CURL_CFG"

    local start end
    start=$(python3 -c 'import time; print(time.time())')
    $CURL --parallel --parallel-max "$conc" -s -K "$CURL_CFG" 2>/dev/null
    end=$(python3 -c 'import time; print(time.time())')

    python3 -c "print(f'{$REQUESTS / ($end - $start):.0f}')"
}

run_yurl() {
    local conc=$1
    yes "{g: $BASE/bench}" | head -n "$REQUESTS" > "$TMPFILE" || true

    local start end
    start=$(python3 -c 'import time; print(time.time())')
    $YURL "{concurrency: $conc, 1: s.code}" < "$TMPFILE" >/dev/null 2>&1
    end=$(python3 -c 'import time; print(time.time())')

    python3 -c "print(f'{$REQUESTS / ($end - $start):.0f}')"
}

# ── Collect results ──────────────────────────────────────────────────

declare -a WRK_RESULTS CURL_RESULTS YURL_RESULTS

echo "running wrk..."
for level in $LEVELS; do
    conc=${level%%:*}; threads=${level##*:}
    WRK_RESULTS+=("$conc:$(run_wrk "$conc" "$threads")")
done

echo "running curl --parallel..."
for level in $LEVELS; do
    conc=${level%%:*}
    CURL_RESULTS+=("$conc:$(run_curl "$conc")")
done

echo "running yurl..."
for level in $LEVELS; do
    conc=${level%%:*}
    YURL_RESULTS+=("$conc:$(run_yurl "$conc")")
done

# ── Results table ────────────────────────────────────────────────────

echo ""
printf "%-14s %12s %12s %12s %10s\n" "concurrency" "wrk req/s" "curl req/s" "yurl req/s" "yurl/curl"
printf "%-14s %12s %12s %12s %10s\n" "-----------" "---------" "----------" "----------" "---------"

for i in "${!WRK_RESULTS[@]}"; do
    conc=${WRK_RESULTS[$i]%%:*}
    wrk_rps=${WRK_RESULTS[$i]##*:}
    curl_rps=${CURL_RESULTS[$i]##*:}
    yurl_rps=${YURL_RESULTS[$i]##*:}
    ratio=$(python3 -c "
c = float('$curl_rps')
y = float('$yurl_rps')
print(f'{y/c:.0%}' if c > 0 else 'n/a')
")
    printf "%-14s %12s %12s %12s %10s\n" "$conc" "$wrk_rps" "$curl_rps" "$yurl_rps" "$ratio"
done

echo ""
echo "done."
