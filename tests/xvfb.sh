#!/bin/sh
# End-to-end check in a private Xvfb: two bars (one per monitor) share one
# PipeWire reader, follow volume and microphone events from it without
# running wpctl or niructl, hand the reader over when its bar exits, and
# stay idle. Uses a fake pw-dump; never touches the machine's audio.
# Usage: tests/xvfb.sh PATH/TO/nirubar   (XVFB_DISPLAY=:61 to choose)
set -eu
BIN=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
T=$(mktemp -d)
PIDS=""
cleanup() {
  for p in $PIDS; do kill "$p" 2>/dev/null || :; done
  sleep 0.2
  rm -rf "$T"
}
trap cleanup EXIT INT TERM
fail() {
  echo "FAIL: $*" >&2
  for f in "$T"/bar*.log; do echo "--- $f"; cat "$f"; done >&2
  exit 1
}
D=${XVFB_DISPLAY:-:$((70 + $$ % 20))}
Xvfb "$D" -screen 0 2400x900x24 -nolisten tcp >/dev/null 2>&1 &
PIDS="$PIDS $!"
export DISPLAY=$D
mkdir -p "$T/bin" "$T/config/niruwm"
mkfifo "$T/pw-events"
cat > "$T/graph.json" <<'JSON'
[
 {"id": 40, "type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "default"},
  "metadata": [{"subject": 0, "key": "default.audio.sink", "value": {"name": "speaker"}},
               {"subject": 0, "key": "default.audio.source", "value": {"name": "mic"}}]},
 {"id": 66, "type": "PipeWire:Interface:Node", "info": {"state": "suspended",
  "props": {"media.class": "Audio/Sink", "node.name": "speaker", "node.description": "Test speaker"},
  "params": {"Props": [{"mute": false, "channelVolumes": [0.064, 0.064]}]}}},
 {"id": 65, "type": "PipeWire:Interface:Node", "info": {"state": "suspended",
  "props": {"media.class": "Audio/Source", "node.name": "mic", "node.description": "Test mic"},
  "params": {"Props": [{"mute": false, "channelVolumes": [0.729, 0.729]}]}}}
]
JSON
cat > "$T/bin/pw-dump" <<'EOF'
#!/bin/sh
[ "$1" = --monitor ] || exit 1
printf '%s\n' "$$" >> "$PW_PIDS"
cat "$PW_GRAPH"
exec 3<> "$PW_EVENTS"
while IFS= read -r line <&3; do printf '%s\n' "$line"; done
EOF
for tool in wpctl niructl pactl amixer xrandr tailscale; do
  printf '#!/bin/sh\nprintf "%%s %%s\\n" "%s" "$*" >> "$PROBES"\nexit 1\n' "$tool" > "$T/bin/$tool"
done
chmod +x "$T/bin/"*
cat > "$T/config/niruwm/config.toml" <<'CONFIG'
[bar]
modules = ["workspaces", "privacy", "volume", "microphone", "display", "clock"]
icons = false
tray = false
CONFIG
export PATH="$T/bin:$PATH" XDG_CONFIG_HOME="$T/config" HOME="$T"
export PW_PIDS="$T/pw.pids" PW_GRAPH="$T/graph.json" PW_EVENTS="$T/pw-events" PROBES="$T/probes"
export NIRUBAR_TRACE_AUDIO=1 NIRUBAR_NO_UPDATE_CHECK=1
: > "$PROBES"
sleep 1

now_ms() { date +%s%3N; }
start=$(now_ms)
"$BIN" --monitor 0,0,1200,900,LEFT > "$T/bar1.log" 2>&1 &
B1=$!; PIDS="$PIDS $B1"
"$BIN" --monitor 1200,0,1200,900,RIGHT > "$T/bar2.log" 2>&1 &
B2=$!; PIDS="$PIDS $B2"
i=0
while [ "$(xwininfo -root -tree 2>/dev/null | grep -c '"NIRUBAR"')" -lt 2 ]; do
  i=$((i + 1)); [ "$i" -lt 200 ] || fail "two bars did not map"
  sleep 0.01
done
first=$(( $(now_ms) - start ))
[ "$first" -lt 2000 ] || fail "bars took $first ms to map"

wait_log() { # FILE PATTERN WHAT
  i=0
  while ! grep -q "$2" "$1"; do
    i=$((i + 1)); [ "$i" -lt 100 ] || fail "$3"
    sleep 0.05
  done
}
for log in "$T/bar1.log" "$T/bar2.log"; do
  wait_log "$log" 'audio volume=40 ' "volume 40 % from the graph ($log)"
  wait_log "$log" 'audio input=90$' "microphone 90 % from the graph ($log)"
done
[ "$(wc -l < "$PW_PIDS")" -eq 1 ] || fail "expected one pw-dump, got $(wc -l < "$PW_PIDS")"
# The leader is the parent of the one pw-dump.
leader=$(ps -o ppid= -p "$(cat "$PW_PIDS")" | tr -d ' ')
case "$leader" in
  "$B1") follower=$B2; follower_log=$T/bar2.log ;;
  "$B2") follower=$B1; follower_log=$T/bar1.log ;;
  *) fail "pw-dump belongs to $leader, not to a bar" ;;
esac
echo "first frame (both bars mapped) ${first} ms, one pw-dump shared"

# Events: volume and mute of the sink, then the default source changes.
printf '%s\n' '[{"id": 66, "type": "PipeWire:Interface:Node", "info": {"change-mask": ["params"], "params": {"Props": [{"mute": false, "channelVolumes": [0.274625, 0.274625]}]}}}]' > "$PW_EVENTS"
wait_log "$T/bar1.log" 'audio volume=65 ' "volume event (bar 1)"
wait_log "$T/bar2.log" 'audio volume=65 ' "volume event (bar 2)"
printf '%s\n' '[{"id": 65, "type": "PipeWire:Interface:Node", "info": {"params": {"Props": [{"mute": true, "channelVolumes": [0.729, 0.729]}]}}}]' > "$PW_EVENTS"
wait_log "$follower_log" 'audio input=90/muted' "microphone mute event"
printf '%s\n' '[{"id": 40, "type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "default"}, "metadata": [{"subject": 0, "key": "default.audio.source", "value": null}]}]' > "$PW_EVENTS"
wait_log "$follower_log" 'audio input=unavailable' "default source removal"
echo "volume, microphone and default events reach both bars"

# Idle: no probe process ran, and the bars sleep.
cpu() { awk '{print $14+$15}' "/proc/$1/stat"; }
a1=$(cpu "$B1"); a2=$(cpu "$B2")
sleep 10
b1=$(cpu "$B1"); b2=$(cpu "$B2")
ticks=$(( b1 - a1 + b2 - a2 ))
[ "$ticks" -le 10 ] || fail "two idle bars used $ticks ticks in 10 s"
rss1=$(awk '/VmRSS/ {print $2}' "/proc/$B1/status"); rss2=$(awk '/VmRSS/ {print $2}' "/proc/$B2/status")
[ "$rss1" -lt 40000 ] && [ "$rss2" -lt 40000 ] || fail "memory $rss1 kB / $rss2 kB"
grep . "$PROBES" && fail "a polled provider ran"
echo "idle: $ticks ticks in 10 s for two bars, ${rss1} kB + ${rss2} kB"

# The leader exits: the follower takes over the reader.
kill "$leader"
i=0
while [ "$(wc -l < "$PW_PIDS")" -lt 2 ]; do
  i=$((i + 1)); [ "$i" -lt 100 ] || fail "no new pw-dump after the hand-over"
  sleep 0.05
done
printf '%s\n' '[{"id": 66, "type": "PipeWire:Interface:Node", "info": {"params": {"Props": [{"mute": true, "channelVolumes": [0.125, 0.125]}]}}}]' > "$PW_EVENTS"
wait_log "$follower_log" 'audio volume=50 ' "events after the hand-over"
kill "$follower"
sleep 0.5
for p in $(cat "$PW_PIDS"); do
  if kill -0 "$p" 2>/dev/null; then fail "pw-dump $p outlived its bar"; fi
done
echo "PASS: shared PipeWire reader, events, hand-over, idle"
