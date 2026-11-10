#!/bin/sh
# Calendar reminders end to end: a real bar, a real notification daemon on a
# private session bus, and a feed served from this machine.
#
# Usage: sh tests/reminder-smoke.sh NIRUBAR NIRUNOTIFY
#
# Two bars (one per screen) share the reminder state, so the notification
# must appear exactly once. A restarted bar must not send it again. Clicking
# the notification opens the calendar's page for that day in the configured
# browser. The bar log must contain neither the event title nor the feed URL.
#
# Needs: Xvfb, dbus-run-session, dbus-monitor, python3, xdotool, xwininfo.
# Never touches DISPLAY :0 or the real HOME.
set -eu
BAR=$(readlink -f "${1:?usage: reminder-smoke.sh NIRUBAR NIRUNOTIFY}")
NOTIFY=$(readlink -f "${2:?usage: reminder-smoke.sh NIRUBAR NIRUNOTIFY}")
DISP=${REMINDER_TEST_DISPLAY:-:96}
NUM=${DISP#:}
T=$(mktemp -d "${TMPDIR:-/tmp}/nirubar-remind.XXXXXX")
H="$T/home"
mkdir -p "$H/.config/nirubar" "$H/.config/niruwm" "$T/feed" "$T/run"
chmod 700 "$T/run"
PIDS=""
cleanup() {
  for p in $PIDS; do kill "$p" 2>/dev/null || :; done
  sleep 0.3
  rm -rf "$T"
}
trap cleanup EXIT INT TERM
fail() { echo "FAIL $*"; echo "--- bar log ---"; cat "$T/bar"*.log 2>/dev/null || :; echo "--- bus ---"; tail -n 40 "$T/monitor.log" 2>/dev/null || :; exit 1; }
ok() { echo "OK   $*"; }

# An event 4 minutes from now (UTC), and a secret-looking feed path.
START=$(date -u -d '+4 minutes' '+%Y%m%dT%H%M00Z')
END=$(date -u -d '+34 minutes' '+%Y%m%dT%H%M00Z')
DAY=$(date -d '+4 minutes' '+%Y/%-m/%-d')
TITLE="Reminder probe meeting"
SECRET="private-0123456789abcdef"
cat > "$T/feed/$SECRET.ics" <<EOF
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//NIRUBAR//reminder test//EN
BEGIN:VEVENT
UID:reminder-probe@example.invalid
DTSTART:$START
DTEND:$END
SUMMARY:$TITLE
LOCATION:Room 4
END:VEVENT
BEGIN:VEVENT
UID:allday-probe@example.invalid
DTSTART;VALUE=DATE:$(date '+%Y%m%d')
DTEND;VALUE=DATE:$(date -d tomorrow '+%Y%m%d')
SUMMARY:All day probe
END:VEVENT
END:VCALENDAR
EOF
PORT=$((20000 + $$ % 20000))
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$T/feed" >"$T/http.log" 2>&1 &
PIDS="$PIDS $!"
cat > "$H/.config/nirubar/calendars.toml" <<EOF
remind_minutes = 10
[[calendar]]
name = "Probe"
url = "http://127.0.0.1:$PORT/$SECRET.ics"
open = "https://calendar.example.invalid/day/{yyyy}/{m}/{d}"
EOF
chmod 600 "$H/.config/nirubar/calendars.toml"
cat > "$T/browser" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> "$T/opened"
EOF
chmod 755 "$T/browser"
# No sound may play: notification sounds go through `nirux`, which is a no-op
# here, and the private runtime directory has no audio server socket.
mkdir -p "$T/bin"
printf '#!/bin/sh\nexit 0\n' > "$T/bin/nirux"
chmod 755 "$T/bin/nirux"
printf 'browser = ["%s"]\n' "$T/browser" > "$H/.config/niruwm/config.toml"

Xvfb "$DISP" -screen 0 2560x800x24 -nolisten tcp >"$T/xvfb.log" 2>&1 &
PIDS="$PIDS $!"
i=0
while [ ! -S "/tmp/.X11-unix/X$NUM" ]; do
  i=$((i + 1)); [ "$i" -gt 100 ] && fail "Xvfb did not start"
  sleep 0.1
done

# Everything below runs inside one private session bus.
cat > "$T/inner.sh" <<'EOF'
set -eu
dbus-monitor "interface='org.freedesktop.Notifications',member='Notify'" > "$T/monitor.log" 2>&1 &
"$NOTIFY" > "$T/notify.log" 2>&1 &
sleep 1
bar() {
  "$BAR" --monitor "$1" --position top >> "$T/bar$2.log" 2>&1 &
  echo $! > "$T/bar$2.pid"
}
bar 0,0,1280,800,LEFT 1
bar 1280,0,1280,800,RIGHT 2
count() { grep -c "string \"$TITLE\"" "$T/monitor.log" 2>/dev/null || :; }
i=0
while [ "$(count)" -lt 1 ]; do
  i=$((i + 1)); [ "$i" -gt 120 ] && { echo "no reminder within 60 s"; exit 1; }
  sleep 0.5
done
sleep 25   # both bars look again at least once
[ "$(count)" = 1 ] || { echo "reminder sent $(count) times by two bars"; exit 1; }
echo "OK   one reminder from two bars"
grep -q 'string "Calendar"' "$T/monitor.log" || { echo "wrong app name"; exit 1; }
grep -q "in [34] min" "$T/monitor.log" || { echo "body lacks the lead time"; exit 1; }
grep -q 'All day probe' "$T/monitor.log" && { echo "an all-day event got a reminder"; exit 1; }
echo "OK   no reminder for the all-day event"
# Click the notification: its default action opens the calendar day.
win=$(xdotool search --classname nirunotify 2>/dev/null | head -n 1 || :)
[ -n "$win" ] || win=$(xdotool search --name "$TITLE" 2>/dev/null | head -n 1 || :)
[ -n "$win" ] || { xwininfo -root -tree | head -30; echo "no notification window"; exit 1; }
eval "$(xdotool getwindowgeometry --shell "$win")"
xdotool mousemove $((X + 20)) $((Y + 12)) click 1
i=0
while [ ! -s "$T/opened" ]; do
  i=$((i + 1)); [ "$i" -gt 40 ] && { echo "a click did not open the calendar"; exit 1; }
  sleep 0.25
done
echo "OK   a click opened $(cat "$T/opened")"
# Restart both bars: the state file stops a second notification.
kill "$(cat "$T/bar1.pid")" "$(cat "$T/bar2.pid")"
sleep 1
bar 0,0,1280,800,LEFT 1
bar 1280,0,1280,800,RIGHT 2
sleep 25
[ "$(count)" = 1 ] || { echo "restarted bars sent the reminder again"; exit 1; }
echo "OK   restarted bars do not repeat it"
EOF
status=0
env -i HOME="$H" DISPLAY="$DISP" PATH="$T/bin:/usr/bin:/bin" XDG_RUNTIME_DIR="$T/run" \
  PULSE_SERVER=unix:/nonexistent PIPEWIRE_REMOTE=/nonexistent \
  LANG=C.UTF-8 T="$T" BAR="$BAR" NOTIFY="$NOTIFY" TITLE="$TITLE" \
  dbus-run-session -- sh "$T/inner.sh" || status=$?
[ "$status" = 0 ] || fail "see above"
grep -q "calendar.example.invalid/day/$DAY" "$T/opened" || fail "opened the wrong page: $(cat "$T/opened")"
ok "the page is the event's day"
mode=$(stat -c %a "$H/.local/state/nirubar/reminders.json")
[ "$mode" = 600 ] || fail "state file mode $mode"
grep -q "$TITLE" "$H/.local/state/nirubar/reminders.json" && fail "the state file holds the event title"
ok "state file is private and holds no event text"
if grep -q -e "$TITLE" -e "$SECRET" "$T"/bar*.log; then fail "the bar log names the event or the feed URL"; fi
ok "the bar log names neither the event nor the feed"
"$BAR" --check-config 2>/dev/null | grep -q "Calendar reminders" || :
echo "PASS calendar reminders"
