#!/bin/sh
# Run under xvfb-run. Fake the shared CLI; never touches real audio.
set -eu
BIN=${1:?usage: microphone-smoke.sh /absolute/path/to/nirubar}
T=$(mktemp -d)
BAR=""
cleanup() {
  [ -z "$BAR" ] || kill "$BAR" 2>/dev/null || :
  [ -z "$BAR" ] || wait "$BAR" 2>/dev/null || :
  rm -rf "$T"
}
trap cleanup EXIT INT TERM
export HOME=$T/home XDG_CONFIG_HOME=$T/home/.config MIC_FIXTURE=$T
export PATH=$T/bin:$PATH
mkdir -p "$XDG_CONFIG_HOME/niruwm" "$T/bin"
printf '[bar]\nmodules = ["microphone"]\nicons = false\ntray = false\npopup_timeout_seconds = 0\n' > "$XDG_CONFIG_HOME/niruwm/config.toml"
printf 'state=ready\nname=USB microphone\npercent=45\nmuted=true\n' > "$T/status"
: > "$T/actions"
cat > "$T/bin/niructl" <<'EOF'
#!/bin/sh
case "$*" in
  'audio mic-status') cat "$MIC_FIXTURE/status" ;;
  *) printf '%s\n' "$*" >> "$MIC_FIXTURE/actions" ;;
esac
EOF
cat > "$T/bin/nirulauncher" <<'EOF'
#!/bin/sh
printf 'launcher %s\n' "$*" >> "$MIC_FIXTURE/actions"
EOF
chmod +x "$T/bin/niructl" "$T/bin/nirulauncher"
"$BIN" > "$T/bar.log" 2>&1 &
BAR=$!
i=0
WIN=""
while [ -z "$WIN" ]; do
  WIN=$(xwininfo -root -tree 2>/dev/null | awk '/"NIRUBAR"/ {print $1; exit}')
  i=$((i + 1)); [ "$i" -lt 100 ] || { cat "$T/bar.log"; exit 1; }
  sleep 0.05
done
sleep 1.2
WIDTH=$(xdotool getwindowgeometry --shell "$WIN" | sed -n 's/^WIDTH=//p')
X=$(( WIDTH - 20 ))
xdotool mousemove --window "$WIN" "$X" 10 click 1
sleep 0.2
[ ! -s "$T/actions" ] || { echo 'FAIL reading microphone panel changed audio'; exit 1; }
xdotool key Escape

# Changing the provider snapshot externally changes the rendered module.
xwd -id "$WIN" -silent > "$T/before.xwd"
before=$(sha256sum "$T/before.xwd" | cut -d' ' -f1)
printf 'state=ready\nname=External input\npercent=73\nmuted=false\n' > "$T/status"
i=0
while :; do
  xwd -id "$WIN" -silent > "$T/after.xwd"
  after=$(sha256sum "$T/after.xwd" | cut -d' ' -f1)
  [ "$after" != "$before" ] && break
  i=$((i + 1)); [ "$i" -lt 80 ] || { echo 'FAIL external microphone update'; exit 1; }
  sleep 0.1
done
xdotool mousemove --window "$WIN" "$X" 10 click 3
sleep 0.2
xdotool click 4
sleep 0.2
xdotool click 2
i=0
while ! grep -qxF 'launcher --microphone' "$T/actions"; do
  i=$((i + 1)); [ "$i" -lt 40 ] || { echo 'FAIL keyboard selector control'; exit 1; }
  sleep 0.1
done
grep -qxF 'audio mic-mute' "$T/actions"
grep -qxF 'audio mic-level up' "$T/actions"
echo 'PASS: cached microphone updates, read-only panel and explicit shared controls'
