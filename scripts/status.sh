#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BIN="$ROOT/target/release/avrig"
PID_FILE="$ROOT/.run/avrig.pid"

echo "=== avrig API Service Status (WSL) ==="

RUNNING=0
if [ -f "$PID_FILE" ]; then
    PID=$(cat "$PID_FILE")
    case "$PID" in
        ''|*[!0-9]*)
            echo "PID file contains invalid value: $(cat "$PID_FILE")"
            ;;
        *)
            if kill -0 "$PID" 2>/dev/null; then
                RUNNING=1
                echo "Process: Running (PID $PID)"
            else
                echo "Process: Not running (stale PID $PID in $PID_FILE)"
            fi
            ;;
    esac
else
    echo "Process: Not running (no PID file at $PID_FILE)"
fi

echo "\n=== Local Port 8000 Listener ==="
ss -lntp 2>/dev/null | grep ":8000\b" || echo "No process listening on port 8000"

echo "\n=== Health Endpoint Checks ==="
echo -n "Direct local loopback (http://127.0.0.1:8000/api/v1/health): "
curl -s -o /dev/null -w "HTTP %{http_code}\n" http://127.0.0.1:8000/api/v1/health || echo "Connection failed"

echo -n "Reverse proxy endpoint (https://api.avrig.ai/api/v1/health): "
curl --cacert /etc/nginx/ssl/avrig-root-ca.crt -s -o /dev/null -w "HTTP %{http_code}\n" https://api.avrig.ai/api/v1/health || echo "Connection failed"

if [ -f "$ROOT/logs/avrig.log" ]; then
    echo "\n=== Recent Application Log ==="
    tail -n 3 "$ROOT/logs/avrig.log"
fi
