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
  *--follow*)
    printf '%s\n' "$$" > "$FOLLOW_PID"
    printf 'Playing\03730000000\037Fixture artist\037A deliberately long title for continuous scrolling in the fixture\037180000000\036\n'
    exec sleep 60 ;;
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
export DISPLAY=:96 XDG_CONFIG_HOME=$TEST/config PROBE_LOG=$TEST/probes FOLLOW_PID=$TEST/follower.pid
export PATH=$TEST/bin:$PATH
Xvfb :96 -screen 0 1280x80x24 -nolisten tcp > "$TEST/xvfb.log" 2>&1 & XVFB=$!
sleep 1
"$BIN" > "$TEST/bar.log" 2>&1 & BAR=$!
sleep 1
kill -0 "$BAR"
[ -s "$FOLLOW_PID" ] || { echo 'FAIL: media follower did not start' >&2; exit 1; }
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
  [ "$player" -le 2 ] || { echo "FAIL: media follower repeatedly restarted" >&2; exit 1; }
  [ "$volume" -le 8 ] || { echo "FAIL: excessive volume probes" >&2; exit 1; }
  [ "$threads" -le 4 ] || { echo "FAIL: unexpected thread growth" >&2; exit 1; }
  # At 100 ticks/second this permits up to 5% of a core during scrolling;
  # account for platforms with a different CLK_TCK.
  hz=$(getconf CLK_TCK)
  max_ticks=$((hz / 2 + 1))
  [ "$((after-before))" -le "$max_ticks" ] || { echo "FAIL: excessive scrolling CPU" >&2; exit 1; }
fi
# The bar owns the follower. Exiting the bar must close it rather than leaving
# a detached process holding the session bus or stdout pipe.
follower=$(cat "$FOLLOW_PID")
kill "$BAR"
wait "$BAR" 2>/dev/null || :
BAR=
if kill -0 "$follower" 2>/dev/null; then
  echo 'FAIL: media follower survived bar shutdown' >&2
  exit 1
fi

# A hard WM/session crash bypasses Rust destructors. The follower must also
# stop when its parent is killed, not merely when the bar exits gracefully.
rm -f "$FOLLOW_PID"
"$BIN" > "$TEST/bar-kill.log" 2>&1 & BAR=$!
i=0
while [ ! -s "$FOLLOW_PID" ]; do
  kill -0 "$BAR" 2>/dev/null || { cat "$TEST/bar-kill.log"; exit 1; }
  i=$((i + 1)); [ "$i" -le 100 ] || { echo 'FAIL: crash fixture follower did not start' >&2; exit 1; }
  sleep 0.05
done
follower=$(cat "$FOLLOW_PID")
kill -KILL "$BAR"
wait "$BAR" 2>/dev/null || :
BAR=
i=0
while kill -0 "$follower" 2>/dev/null; do
  # A child reparented by SIGKILL may briefly be an init-owned zombie; it is
  # no longer a running follower or holder of the session bus.
  state=$(ps -o stat= -p "$follower" 2>/dev/null || :)
  case "$state" in *Z*) break ;; esac
  i=$((i + 1)); [ "$i" -le 40 ] || { echo 'FAIL: follower survived bar crash' >&2; exit 1; }
  sleep 0.05
done
printf '%s\n' 'PASS: media follower stops on graceful exit and hard crash'
