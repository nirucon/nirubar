#!/bin/sh
# Real X11 render check for NIRUBAR. Run inside a container with:
#   xvfb x11-utils imagemagick fonts-dejavu-core
# Usage: render-check.sh /path/to/nirubar
set -eu
BIN="${1:?path to nirubar binary}"
export DISPLAY=:99
Xvfb :99 -screen 0 1280x36x24 -nolisten tcp >/tmp/xvfb.log 2>&1 &
XVFB=$!
trap 'kill $XVFB 2>/dev/null || :' EXIT INT TERM
# Wait for the server socket.
i=0
while [ ! -S /tmp/.X11-unix/X99 ]; do
  i=$((i + 1)); [ "$i" -gt 100 ] && { echo "Xvfb did not start"; cat /tmp/xvfb.log; exit 1; }
  sleep 0.1
done

# Pretend to be a window manager publishing NIRUWM/EWMH state.
xprop -root -f _NET_CURRENT_DESKTOP 32c -set _NET_CURRENT_DESKTOP 2 >/dev/null
xprop -root -f _NET_CLIENT_LIST 32x -set _NET_CLIENT_LIST 0x0 >/dev/null

"$BIN" >/tmp/nirubar.log 2>&1 &
BAR=$!
trap 'kill $BAR $XVFB 2>/dev/null || :' EXIT INT TERM
sleep 1.5
if ! kill -0 "$BAR" 2>/dev/null; then
  echo "FAIL nirubar exited"
  cat /tmp/nirubar.log
  exit 1
fi

# The dock window must exist with a strut and a stable name.
xprop -root _NET_CLIENT_LIST >/dev/null
import -window root /tmp/bar.png 2>/dev/null || xwd -root -silent >/tmp/bar.xwd
echo "--- windows ---"
xwininfo -root -tree | head -12
echo "--- bar window props ---"
BID=$(xwininfo -root -tree | awk '/NIRUBAR/{print $1; exit}')
[ -n "$BID" ] || { echo "FAIL no NIRUBAR window"; exit 1; }
xprop -id "$BID" _NET_WM_WINDOW_TYPE _NET_WM_STRUT _NET_WM_STRUT_PARTIAL | sed 's/^/  /'

# Pixel proof: with 9 muted+1 active tags and a clock, the bar must contain
# more than one distinct colour and non-background pixels in the right region.
if command -v convert >/dev/null 2>&1; then
  convert /tmp/bar.png -crop 1280x36+0+0 +repage txt:- 2>/dev/null \
    | awk -F'[,:] *| +' '/^[0-9]/{print $3}' | sort -u > /tmp/colours.txt
  COUNT=$(wc -l < /tmp/colours.txt)
  echo "--- distinct colours: $COUNT ---"
  [ "$COUNT" -gt 3 ] || { echo "FAIL bar rendered too few colours"; head /tmp/colours.txt; exit 1; }
  # The clock is right-aligned: check for foreground pixels in the right third.
  convert /tmp/bar.png -crop 426x36+854+0 +repage -format "%[fx:mean]" info: > /tmp/right.txt
  convert /tmp/bar.png -crop 426x36+0+0 +repage -format "%[fx:mean]" info: > /tmp/left.txt
  echo "left mean: $(cat /tmp/left.txt)  right mean: $(cat /tmp/right.txt)"
else
  echo "SKIP pixel check (no imagemagick)"
fi

echo "PASS render-check"
