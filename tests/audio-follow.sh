#!/bin/sh
# Exercise sink/server events and subscriber cleanup with private helpers.
# Run under xvfb-run; this never changes the machine's audio devices.
set -eu
BIN=${1:?pass path to nirubar}
TEST=$(mktemp -d)
BAR=
cleanup() {
  [ -z "$BAR" ] || { kill "$BAR" 2>/dev/null || :; wait "$BAR" 2>/dev/null || :; }
  rm -rf "$TEST"
}
trap cleanup EXIT HUP INT TERM
mkdir -p "$TEST/bin" "$TEST/config/niruwm"
mkfifo "$TEST/events"
printf '0.40\n' > "$TEST/volume"
cat > "$TEST/bin/wpctl" <<'HELPER'
#!/bin/sh
[ "$1" = get-volume ] || exit 1
printf 'Volume: %s\n' "$(cat "$VOLUME_FILE")"
HELPER
cat > "$TEST/bin/pactl" <<'HELPER'
#!/bin/sh
[ "$1" = subscribe ] || exit 1
printf '%s\n' "$$" > "$PACTL_PID"
exec 3<> "$EVENT_FIFO"
while IFS= read -r line <&3; do printf '%s\n' "$line"; done
HELPER
chmod +x "$TEST/bin/wpctl" "$TEST/bin/pactl"
cat > "$TEST/config/niruwm/config.toml" <<'CONFIG'
[bar]
modules = ["volume", "clock"]
icons = false
tray = false
CONFIG
export XDG_CONFIG_HOME="$TEST/config" VOLUME_FILE="$TEST/volume" PACTL_PID="$TEST/pactl.pid" EVENT_FIFO="$TEST/events" NIRUBAR_TRACE_AUDIO=1
export PATH="$TEST/bin:$PATH"
"$BIN" >"$TEST/bar.log" 2>&1 & BAR=$!
i=0
while [ ! -s "$PACTL_PID" ] || ! grep -q 'audio volume=40 ' "$TEST/bar.log"; do
  kill -0 "$BAR" 2>/dev/null || { cat "$TEST/bar.log"; exit 1; }
  i=$((i + 1)); [ "$i" -le 100 ] || { cat "$TEST/bar.log"; echo 'FAIL audio subscriber did not start' >&2; exit 1; }
  sleep 0.05
done
printf '0.65\n' > "$VOLUME_FILE"
printf "Event 'change' on sink #1\n" > "$EVENT_FIFO"
i=0
while ! grep -q 'audio volume=65 ' "$TEST/bar.log"; do
  i=$((i + 1)); [ "$i" -le 40 ] || { cat "$TEST/bar.log"; echo 'FAIL sink event did not refresh volume' >&2; exit 1; }
  sleep 0.05
done
printf '0.35\n' > "$VOLUME_FILE"
printf "Event 'change' on server #0\n" > "$EVENT_FIFO"
i=0
while ! grep -q 'audio volume=35 ' "$TEST/bar.log"; do
  i=$((i + 1)); [ "$i" -le 40 ] || { cat "$TEST/bar.log"; echo 'FAIL default sink event did not refresh volume' >&2; exit 1; }
  sleep 0.05
done
printf '0.70\n' > "$VOLUME_FILE"
printf "Event 'change' on source #1\n" > "$EVENT_FIFO"
sleep 0.3
if grep -q 'audio volume=70 ' "$TEST/bar.log"; then
  echo 'FAIL unrelated source event refreshed output' >&2
  exit 1
fi
subscriber=$(cat "$PACTL_PID")
kill "$subscriber"
i=0
while ! grep -q 'audio volume=70 ' "$TEST/bar.log"; do
  i=$((i + 1)); [ "$i" -le 80 ] || { cat "$TEST/bar.log"; echo 'FAIL fallback did not refresh after subscriber exit' >&2; exit 1; }
  sleep 0.05
done
# The subscriber is retried after a bounded backoff while the old 2 s polling
# path keeps working. A replacement must resume event-driven updates.
i=0
while [ "$(cat "$PACTL_PID")" = "$subscriber" ]; do
  kill -0 "$BAR" 2>/dev/null || { cat "$TEST/bar.log"; exit 1; }
  i=$((i + 1)); [ "$i" -le 800 ] || { echo 'FAIL audio subscriber did not reconnect' >&2; exit 1; }
  sleep 0.05
done
subscriber=$(cat "$PACTL_PID")
printf '0.45\n' > "$VOLUME_FILE"
printf "Event 'change' on sink #1\n" > "$EVENT_FIFO"
i=0
while ! grep -q 'audio volume=45 ' "$TEST/bar.log"; do
  i=$((i + 1)); [ "$i" -le 40 ] || { echo 'FAIL reconnected subscriber missed sink event' >&2; exit 1; }
  sleep 0.05
done
kill "$BAR"
wait "$BAR" 2>/dev/null || :
BAR=
if kill -0 "$subscriber" 2>/dev/null; then
  echo 'FAIL audio subscriber survived bar shutdown' >&2
  exit 1
fi
rm -f "$PACTL_PID"
"$BIN" >"$TEST/bar-crash.log" 2>&1 & BAR=$!
i=0
while [ ! -s "$PACTL_PID" ]; do
  kill -0 "$BAR" 2>/dev/null || { cat "$TEST/bar-crash.log"; exit 1; }
  i=$((i + 1)); [ "$i" -le 100 ] || { echo 'FAIL crash fixture subscriber did not start' >&2; exit 1; }
  sleep 0.05
done
subscriber=$(cat "$PACTL_PID")
kill -KILL "$BAR"
wait "$BAR" 2>/dev/null || :
BAR=
i=0
while kill -0 "$subscriber" 2>/dev/null; do
  state=$(ps -o stat= -p "$subscriber" 2>/dev/null || :)
  case "$state" in *Z*) break ;; esac
  i=$((i + 1)); [ "$i" -le 40 ] || { echo 'FAIL audio subscriber survived bar crash' >&2; exit 1; }
  sleep 0.05
done
printf '%s\n' 'PASS: sink/server events, fallback, reconnect and cleanup are correct'
