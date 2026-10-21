#!/bin/sh
# Run under xvfb-run. Fake the shared CLI; never touches BlueZ.
set -eu
BIN=${1:?usage: bluetooth-smoke.sh /absolute/path/to/nirubar}
T=$(mktemp -d)
BAR=""
cleanup() { [ -z "$BAR" ] || kill "$BAR" 2>/dev/null || :; [ -z "$BAR" ] || wait "$BAR" 2>/dev/null || :; rm -rf "$T"; }
trap cleanup EXIT INT TERM
export HOME=$T/home XDG_CONFIG_HOME=$T/home/.config BT_FIXTURE=$T
export PATH=$T/bin:$PATH
mkdir -p "$XDG_CONFIG_HOME/niruwm" "$T/bin"
printf '[bar]\nmodules = ["bluetooth"]\nicons = false\ntray = false\npopup_timeout_seconds = 0\n' > "$XDG_CONFIG_HOME/niruwm/config.toml"
cat > "$T/bin/niructl" <<'EOF'
#!/bin/sh
case "$*" in
  'bluetooth status') cat "$BT_FIXTURE/status" ;;
  *) printf '%s\n' "$*" >> "$BT_FIXTURE/actions" ;;
esac
EOF
cat > "$T/bin/nirulauncher" <<'EOF'
#!/bin/sh
printf 'launcher %s\n' "$*" >> "$BT_FIXTURE/actions"
EOF
chmod +x "$T/bin/niructl" "$T/bin/nirulauncher"
printf 'state=ready\nadapter=AA:BB:CC\npowered=true\ndevice=CC:DD:EE\tHeadphones\ttrue\t75\n' > "$T/status"
: > "$T/actions"
"$BIN" > "$T/bar.log" 2>&1 & BAR=$!
i=0; WIN=""
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
[ ! -s "$T/actions" ] || { echo 'FAIL Bluetooth info changed state'; exit 1; }
xdotool key Escape
xdotool mousemove --window "$WIN" "$X" 10 click 3
sleep 0.2
xdotool mousemove --window "$WIN" "$X" 10 click 2
i=0
while ! grep -qxF 'launcher --bluetooth' "$T/actions"; do
  i=$((i + 1)); [ "$i" -lt 40 ] || { echo 'FAIL Bluetooth selector'; exit 1; }
  sleep 0.1
done
grep -qxF 'bluetooth power toggle' "$T/actions"
echo 'PASS: Bluetooth info is read-only and explicit power/selector controls work'
