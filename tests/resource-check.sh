#!/bin/sh
# Deterministic ten-second scrolling workload; no real player/audio server needed.
set -eu
BIN=${1:?path to nirubar}
MODE=${2:-check}
TEST=$(mktemp -d)
BAR=; XVFB=
cleanup() {
  if [ -n "$BAR" ]; then kill "$BAR" 2>/dev/null || :; wait "$BAR" 2>/dev/null || :; fi
  if [ -n "$XVFB" ]; then kill "$XVFB" 2>/dev/null || :; wait "$XVFB" 2>/dev/null || :; fi
  rm -rf "$TEST"
}
trap cleanup EXIT INT TERM
mkdir -p "$TEST/bin" "$TEST/config/niruwm"
cat > "$TEST/bin/playerctl" <<'HELPER'
#!/bin/sh
printf 'playerctl\n' >> "$PROBE_LOG"
case "$*" in
  *--format*) printf 'Playing\03730000000\037Fixture artist\037A deliberately long title for continuous scrolling in the fixture\037180000000\n' ;;
  status) printf 'Playing\n' ;;
  'metadata artist') printf 'Fixture artist\n' ;;
  'metadata title') printf 'A deliberately long title for continuous scrolling in the fixture\n' ;;
  position) printf '30\n' ;;
  'metadata mpris:length') printf '180000000\n' ;;
esac
HELPER
cat > "$TEST/bin/wpctl" <<'HELPER'
#!/bin/sh
printf 'wpctl\n' >> "$PROBE_LOG"
printf 'Volume: 0.50\n'
HELPER
chmod +x "$TEST/bin/playerctl" "$TEST/bin/wpctl"
cat > "$TEST/config/niruwm/config.toml" <<'CONFIG'
[bar]
modules = ["volume", "media", "clock"]
icons = false
tray = false
media_width = 12
CONFIG
export DISPLAY=:96 XDG_CONFIG_HOME=$TEST/config PROBE_LOG=$TEST/probes
export PATH=$TEST/bin:$PATH
Xvfb :96 -screen 0 1280x80x24 -nolisten tcp > "$TEST/xvfb.log" 2>&1 & XVFB=$!
sleep 1
"$BIN" > "$TEST/bar.log" 2>&1 & BAR=$!
sleep 1
kill -0 "$BAR"
: > "$PROBE_LOG"
before=$(awk '{print $14+$15}' "/proc/$BAR/stat")
sleep 10
kill -0 "$BAR"
after=$(awk '{print $14+$15}' "/proc/$BAR/stat")
player=$(awk '/playerctl/{n++} END {print n+0}' "$PROBE_LOG")
volume=$(awk '/wpctl/{n++} END {print n+0}' "$PROBE_LOG")
threads=$(awk '/Threads:/{print $2}' "/proc/$BAR/status")
rss=$(awk '/VmRSS:/{print $2}' "/proc/$BAR/status")
printf '10s scrolling: playerctl=%s wpctl=%s threads=%s CPU_ticks=%s RSS_KiB=%s\n' "$player" "$volume" "$threads" "$((after-before))" "$rss"
if [ "$MODE" != baseline ]; then
  # The media and volume sources deliberately use background workers. The
  # previous one-thread assertion rejected every current build, not a resource
  # regression. Bound the two sources' process cadence and allow only the main
  # thread, those two workers, and one optional tray/runtime worker.
  [ "$player" -le 15 ] || { echo "FAIL: excessive media probes" >&2; exit 1; }
  [ "$volume" -le 8 ] || { echo "FAIL: excessive volume probes" >&2; exit 1; }
  [ "$threads" -le 4 ] || { echo "FAIL: unexpected thread growth" >&2; exit 1; }
  # At 100 ticks/second this permits up to 5% of a core during scrolling;
  # account for platforms with a different CLK_TCK.
  hz=$(getconf CLK_TCK)
  max_ticks=$((hz / 2 + 1))
  [ "$((after-before))" -le "$max_ticks" ] || { echo "FAIL: excessive scrolling CPU" >&2; exit 1; }
fi
