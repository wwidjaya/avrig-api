#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BIN="$ROOT/target/release/avrig"
PID_FILE="$ROOT/.run/avrig.pid"

if [ ! -f "$PID_FILE" ]; then
    echo "No make-run PID file found; avrig is not managed by make run."
    exit 0
fi

PID=$(cat "$PID_FILE")
case "$PID" in
    ''|*[!0-9]*)
        echo "Invalid PID in $PID_FILE" >&2
        exit 1
        ;;
esac

if ! kill -0 "$PID" 2>/dev/null; then
    rm -f "$PID_FILE"
    echo "The recorded avrig process is already stopped."
    exit 0
fi

COMMAND=$(ps -p "$PID" -o command= 2>/dev/null || true)
case "$COMMAND" in
    *"$BIN"*) ;;
    *)
        echo "PID $PID does not appear to be this project's avrig executable. Refusing to stop it." >&2
        exit 1
        ;;
esac

kill -TERM "$PID"
COUNT=0
while kill -0 "$PID" 2>/dev/null && [ "$COUNT" -lt 20 ]; do
    sleep 0.5
    COUNT=$((COUNT + 1))
done

if kill -0 "$PID" 2>/dev/null; then
    echo "avrig did not exit after SIGTERM; sending SIGKILL." >&2
    kill -KILL "$PID"
fi

rm -f "$PID_FILE"
echo "Stopped avrig API (PID $PID)."
