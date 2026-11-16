#!/bin/sh
# Run under xvfb-run. Fixtures stand in for UDisks2 and BlueZ; no drive or
# Bluetooth device is touched. Each module runs alone at the right edge.
set -eu
BIN=${1:?usage: drives-timer-headset-smoke.sh /absolute/path/to/nirubar}
SHOT=${2:-}
T=$(mktemp -d)
BAR=""
stop_bar() { [ -z "$BAR" ] || kill "$BAR" 2>/dev/null || :; [ -z "$BAR" ] || wait "$BAR" 2>/dev/null || :; BAR=""; }
cleanup() { stop_bar; rm -rf "$T"; }
trap cleanup EXIT INT TERM
export HOME=$T/home XDG_CONFIG_HOME=$T/home/.config XDG_RUNTIME_DIR=$T/run
export PATH=$T/bin:$PATH
mkdir -p "$XDG_CONFIG_HOME/niruwm" "$XDG_CONFIG_HOME/nirubar" "$T/bin" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
for p in notify-send nirulauncher niructl pw-play paplay; do
  printf '#!/bin/sh\nprintf "%%s %%s\\n" %s "$*" >> "%s/actions"\n' "$p" "$T" > "$T/bin/$p"
  chmod +x "$T/bin/$p"
done
: > "$T/actions"
printf '[timer]\nminutes = 10\n' > "$XDG_CONFIG_HOME/nirubar/config.toml"

export NIRUBAR_DRIVES_FIXTURE=$T/drives NIRUBAR_BLUEZ_FIXTURE=$T/bluez
printf '/b/sdb1\torg.freedesktop.UDisks2.Block\tDrive\ts:/d/stick\n/b/sdb1\torg.freedesktop.UDisks2.Block\tPreferredDevice\ty:/dev/sdb1\n/b/sdb1\torg.freedesktop.UDisks2.Block\tIdLabel\ts:KINGSTON\n/b/sdb1\torg.freedesktop.UDisks2.Block\tSize\tn:16000000000\n/b/sdb1\torg.freedesktop.UDisks2.Filesystem\tMountPoints\ty:\n/d/stick\torg.freedesktop.UDisks2.Drive\tRemovable\tb:true\n' > "$T/drives"
printf '/d/a\torg.bluez.Device1\tConnected\tb:true\n/d/a\torg.bluez.Device1\tAddress\ts:AA:BB\n/d/a\torg.bluez.Device1\tAlias\ts:Headphones\n/d/a\torg.bluez.Battery1\tPercentage\tn:12\n' > "$T/bluez"

start() {
  printf '[bar]\nmodules = ["%s"]\nicons = false\ntray = false\npopup_timeout_seconds = 0\n' "$1" > "$XDG_CONFIG_HOME/niruwm/config.toml"
  "$BIN" > "$T/bar.log" 2>&1 & BAR=$!
  i=0; WIN=""
  while [ -z "$WIN" ]; do
    WIN=$(xwininfo -root -tree 2>/dev/null | awk '/"NIRUBAR"/ {print $1; exit}')
    i=$((i + 1)); [ "$i" -lt 100 ] || { cat "$T/bar.log"; exit 1; }
    sleep 0.05
  done
  sleep 1.5
  WIDTH=$(xdotool getwindowgeometry --shell "$WIN" | sed -n 's/^WIDTH=//p')
  X=$(( WIDTH - 20 ))
}
wait_for() { # file pattern what
  i=0
  while ! grep -qF -- "$2" "$1" 2>/dev/null; do
    i=$((i + 1)); [ "$i" -lt 50 ] || { echo "FAIL $3"; cat "$1" 2>/dev/null; cat "$T/bar.log"; exit 1; }
    sleep 0.1
  done
}

# drives: left click reads, middle opens, right ejects.
start drives
xdotool mousemove --window "$WIN" "$X" 10 click 1
sleep 0.3
[ ! -s "$T/drives.actions" ] || { echo 'FAIL drives panel changed state'; exit 1; }
xdotool key Escape
xdotool mousemove --window "$WIN" "$X" 10 click 2
wait_for "$T/drives.actions" "open /dev/sdb1" "drives open"
xdotool mousemove --window "$WIN" "$X" 10 click 3
wait_for "$T/drives.actions" "eject /dev/sdb1" "drives eject"
stop_bar
# Nothing attached: the module is hidden and its process is quiet.
: > "$T/drives"
[ "$("$BIN" --drives list)" = "" ] || { echo 'FAIL empty drive list'; exit 1; }
echo 'OK   drives: read-only panel, open and eject'

# timer: scroll sets, right starts and pauses, middle resets; one finish.
start timer
xdotool mousemove --window "$WIN" "$X" 10 click 4
sleep 0.3
[ "$("$BIN" --timer status)" = "idle 15 min" ] || { echo "FAIL timer scroll: $("$BIN" --timer status)"; exit 1; }
xdotool mousemove --window "$WIN" "$X" 10 click 3
sleep 0.3
case $("$BIN" --timer status) in running*) ;; *) echo 'FAIL timer start'; exit 1 ;; esac
xdotool mousemove --window "$WIN" "$X" 10 click 3
sleep 0.3
case $("$BIN" --timer status) in paused*) ;; *) echo 'FAIL timer pause'; exit 1 ;; esac
xdotool mousemove --window "$WIN" "$X" 10 click 2
sleep 0.3
[ "$("$BIN" --timer status)" = "idle 15 min" ] || { echo 'FAIL timer reset'; exit 1; }
# A timer that ends: the bar tells once.
uid=$(id -u)
printf 'minutes=1\nstate=running\nend=%s\n' "$(( $(date +%s) + 2 ))" > "$XDG_RUNTIME_DIR/nirubar-timer-$uid"
wait_for "$T/actions" "notify-send" "timer end notification"
sleep 2
[ "$(grep -c '^notify-send.*Timer' "$T/actions")" = 1 ] || { echo 'FAIL timer notified more than once'; cat "$T/actions"; exit 1; }
grep -q '^pw-play\|^paplay' "$T/actions" || [ ! -f /usr/share/sounds/freedesktop/stereo/complete.oga ] || { echo 'FAIL timer sound'; exit 1; }
stop_bar
echo 'OK   timer: scroll, start, pause, reset and one end notice'

# headset: low battery notifies once, middle opens the Bluetooth devices.
: > "$T/actions"
start headset
wait_for "$T/actions" "Headphones battery low" "headset low notification"
xdotool mousemove --window "$WIN" "$X" 10 click 2
wait_for "$T/actions" "nirulauncher --bluetooth" "headset selector"
sleep 1.5
[ "$(grep -c 'battery low' "$T/actions")" = 1 ] || { echo 'FAIL headset notified more than once'; exit 1; }
stop_bar
echo 'OK   headset: one low-battery notice and the device selector'

if [ -n "$SHOT" ]; then
  printf '/d/a\torg.bluez.Device1\tConnected\tb:true\n/d/a\torg.bluez.Device1\tAlias\ts:Headphones\n/d/a\torg.bluez.Battery1\tPercentage\tn:64\n' > "$T/bluez"
  printf '/b/sdb1\torg.freedesktop.UDisks2.Block\tDrive\ts:/d/stick\n/b/sdb1\torg.freedesktop.UDisks2.Block\tIdLabel\ts:KINGSTON\n/b/sdb1\torg.freedesktop.UDisks2.Filesystem\tMountPoints\ty:\n/d/stick\torg.freedesktop.UDisks2.Drive\tRemovable\tb:true\n' > "$T/drives"
  "$BIN" --timer start > /dev/null
  printf '[bar]\nmodules = ["drives", "timer", "headset", "clock"]\ntray = false\n' > "$XDG_CONFIG_HOME/niruwm/config.toml"
  "$BIN" > "$T/bar.log" 2>&1 & BAR=$!
  sleep 3
  import -window root "$SHOT"
  stop_bar
fi
echo 'PASS: drives, timer and headset'
