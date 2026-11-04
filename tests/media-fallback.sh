#!/bin/sh
# A provider without --follow must fall back to bounded one-shot snapshots.
# Run under xvfb-run; the private PATH and config never touch the user's session.
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
cat > "$TEST/bin/playerctl" <<'HELPER'
#!/bin/sh
case "$*" in
  *--follow*) printf 'follow\n' >> "$PROBE_LOG"; exit 2 ;;
  *--format*) printf 'snapshot\n' >> "$PROBE_LOG"; printf 'Playing\03730000000\037Artist\037Fallback track\037180000000\n' ;;
  *) exit 1 ;;
esac
HELPER
chmod +x "$TEST/bin/playerctl"
cat > "$TEST/config/niruwm/config.toml" <<'CONFIG'
[bar]
modules = ["media", "clock"]
icons = false
tray = false
CONFIG
export XDG_CONFIG_HOME="$TEST/config" PROBE_LOG="$TEST/probes" NIRUBAR_TRACE_MEDIA=1
export PATH="$TEST/bin:$PATH"
unset DBUS_SESSION_BUS_ADDRESS
"$BIN" >"$TEST/bar.log" 2>&1 & BAR=$!
i=0
while ! grep -q 'media state playing' "$TEST/bar.log"; do
  kill -0 "$BAR" 2>/dev/null || { cat "$TEST/bar.log"; exit 1; }
  i=$((i + 1)); [ "$i" -le 100 ] || { cat "$TEST/bar.log"; echo 'FAIL media fallback did not appear' >&2; exit 1; }
  sleep 0.05
done
grep -q '^follow$' "$PROBE_LOG"
grep -q '^snapshot$' "$PROBE_LOG"
printf '%s\n' 'PASS: unsupported follower uses one-shot media fallback'
