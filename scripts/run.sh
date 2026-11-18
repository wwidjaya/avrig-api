#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BIN="$ROOT/target/release/avrig"
CONFIG="$ROOT/config"
RUNTIME_DIR="$ROOT/.run"
PID_FILE="$RUNTIME_DIR/avrig.pid"
STDOUT_LOG="$RUNTIME_DIR/avrig.stdout.log"
STDERR_LOG="$RUNTIME_DIR/avrig.stderr.log"

ENV_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/avrig/runtime.env"
if [ ! -f "$ENV_FILE" ] && [ -f "/home/avrig/.config/avrig/runtime.env" ]; then
    ENV_FILE="/home/avrig/.config/avrig/runtime.env"
fi

if [ -f "$ENV_FILE" ]; then
    set -a
    # shellcheck disable=SC1090
    . "$ENV_FILE"
    set +a
fi

if [ ! -x "$BIN" ]; then
    echo "Release executable not found or not executable: $BIN" >&2
    exit 1
fi

mkdir -p "$RUNTIME_DIR"

if [ -f "$PID_FILE" ]; then
    SAVED_PID=$(cat "$PID_FILE")
    if kill -0 "$SAVED_PID" 2>/dev/null; then
        COMMAND=$(ps -p "$SAVED_PID" -o command= 2>/dev/null || true)
        case "$COMMAND" in
            *"$BIN"*)
                echo "avrig API is already running (PID $SAVED_PID)."
                echo "Logs: $STDOUT_LOG and $STDERR_LOG"
                exit 0
                ;;
        esac
    fi
    rm -f "$PID_FILE"
fi

nohup "$BIN" --config "$CONFIG" serve \
    >>"$STDOUT_LOG" 2>>"$STDERR_LOG" </dev/null &
PID=$!
printf '%s\n' "$PID" >"$PID_FILE"

sleep 1
if ! kill -0 "$PID" 2>/dev/null; then
    echo "avrig exited during startup. Recent stderr:" >&2
    tail -n 40 "$STDERR_LOG" >&2 || true
    rm -f "$PID_FILE"
    exit 1
fi

echo "Started avrig API in the background (PID $PID)."
echo "Health endpoint: http://127.0.0.1:8000/api/v1/health"
echo "Application log: $ROOT/logs/avrig.log"
