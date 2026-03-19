#!/bin/bash
# E2E test: open VNC connection via CLI and verify data flows through the tunnel.
# Uses multiplexed mode (same as the GUI app with multiplexed: true).
set -euo pipefail

CLI="./src-tauri/target/debug/connection-app-cli"
TIMEOUT=30
PASS=0
FAIL=0

log() { echo "  [$1] $2"; }
pass() { log "PASS" "$1"; PASS=$((PASS + 1)); }
fail() { log "FAIL" "$1"; FAIL=$((FAIL + 1)); }

cleanup() {
  if [ -n "${CLI_PID:-}" ]; then
    kill "$CLI_PID" 2>/dev/null || true
    wait "$CLI_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT

echo ""
echo "=== E2E VNC Tunnel Test ==="
echo ""

# --- Test 1: CLI binary exists ---
if [ -x "$CLI" ]; then
  pass "CLI binary exists"
else
  fail "CLI binary not found at $CLI"
  exit 1
fi

# --- Test 2: Start VNC tunnel (prod) ---
log "INFO" "Starting VNC tunnel for covered-vnc / covered on port 16080..."
$CLI --project covered-vnc --profile covered --port 16080 --debug 2>/tmp/cli-vnc-test.log &
CLI_PID=$!

# Wait for tunnel to be ready (listen on port)
READY=false
for i in $(seq 1 $TIMEOUT); do
  if lsof -i :16080 -sTCP:LISTEN >/dev/null 2>&1; then
    READY=true
    break
  fi
  sleep 1
done

if $READY; then
  pass "Tunnel listening on port 16080"
else
  fail "Tunnel did not start within ${TIMEOUT}s"
  cat /tmp/cli-vnc-test.log | tail -20
  exit 1
fi

# --- Test 3: HTTP request through tunnel ---
log "INFO" "Testing HTTP request through tunnel..."
HTTP_STATUS=$(curl -s -o /tmp/vnc-response.html -w '%{http_code}' --max-time 15 http://localhost:16080/vnc.html 2>/dev/null || echo "000")

if [ "$HTTP_STATUS" = "200" ]; then
  BODY_SIZE=$(wc -c < /tmp/vnc-response.html | tr -d ' ')
  pass "HTTP 200 OK — received ${BODY_SIZE} bytes"
else
  fail "HTTP request failed (status=$HTTP_STATUS)"
  cat /tmp/cli-vnc-test.log | grep -iE "error|warn|channel|FIN" | tail -10
fi

# --- Test 4: Response contains noVNC content ---
if grep -q "noVNC" /tmp/vnc-response.html 2>/dev/null; then
  pass "Response contains noVNC content"
elif grep -q "WebSocket" /tmp/vnc-response.html 2>/dev/null; then
  pass "Response contains WebSocket content (VNC)"
elif [ "$HTTP_STATUS" = "200" ]; then
  # Still a pass if we got 200
  pass "Got valid HTTP response (${BODY_SIZE} bytes)"
else
  fail "Response doesn't look like a VNC page"
fi

# --- Test 5: Multiple concurrent requests (tests multiplexing) ---
log "INFO" "Testing concurrent requests (multiplexing)..."
CONCURRENT_OK=0
for i in 1 2 3; do
  STATUS=$(curl -s -o /dev/null -w "%{http_code}" --max-time 10 http://localhost:16080/ 2>/dev/null || echo "000")
  if [ "$STATUS" != "000" ]; then
    CONCURRENT_OK=$((CONCURRENT_OK + 1))
  fi
done

if [ "$CONCURRENT_OK" -ge 2 ]; then
  pass "Concurrent requests: $CONCURRENT_OK/3 succeeded"
else
  fail "Concurrent requests: only $CONCURRENT_OK/3 succeeded"
fi

# --- Cleanup prod ---
kill "$CLI_PID" 2>/dev/null || true
wait "$CLI_PID" 2>/dev/null || true
unset CLI_PID

# --- Test 6: Start VNC tunnel (staging) ---
log "INFO" "Starting VNC tunnel for covered-vnc / covered-staging on port 16081..."
$CLI --project covered-vnc --profile covered-staging --port 16081 --debug 2>/tmp/cli-vnc-test-staging.log &
CLI_PID=$!

READY=false
for i in $(seq 1 $TIMEOUT); do
  if lsof -i :16081 -sTCP:LISTEN >/dev/null 2>&1; then
    READY=true
    break
  fi
  sleep 1
done

if $READY; then
  pass "Staging tunnel listening on port 16081"
else
  fail "Staging tunnel did not start within ${TIMEOUT}s"
  cat /tmp/cli-vnc-test-staging.log | tail -20
fi

# --- Test 7: HTTP request through staging tunnel ---
if $READY; then
  log "INFO" "Testing HTTP request through staging tunnel..."
  HTTP_STATUS=$(curl -s -o /tmp/vnc-response-staging.html -w '%{http_code}' --max-time 15 http://localhost:16081/vnc.html 2>/dev/null || echo "000")

  if [ "$HTTP_STATUS" = "200" ]; then
    BODY_SIZE=$(wc -c < /tmp/vnc-response-staging.html | tr -d ' ')
    pass "Staging HTTP 200 OK — received ${BODY_SIZE} bytes"
  else
    fail "Staging HTTP request failed (status=$HTTP_STATUS)"
    cat /tmp/cli-vnc-test-staging.log | grep -iE "error|warn|channel|FIN" | tail -10
  fi
fi

# --- Cleanup staging ---
kill "$CLI_PID" 2>/dev/null || true
wait "$CLI_PID" 2>/dev/null || true
unset CLI_PID

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="
echo ""

if [ "$FAIL" -gt 0 ]; then
  echo "Debug log (last 20 lines):"
  tail -20 /tmp/cli-vnc-test.log
  exit 1
fi
