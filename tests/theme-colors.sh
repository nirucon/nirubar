#!/bin/sh
# Exact rendered background colour for every built-in theme and a custom theme.
# Usage: theme-colors.sh /path/to/nirubar
set -eu
BIN="${1:?path to nirubar binary}"
HERE=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
export DISPLAY=:93
T=$(mktemp -d)
XVFB=""
BAR=""
cleanup() {
  [ -z "$BAR" ] || kill "$BAR" 2>/dev/null || :
  [ -z "$XVFB" ] || kill "$XVFB" 2>/dev/null || :
  rm -rf "$T"
}
trap cleanup EXIT INT TERM

Xvfb :93 -screen 0 800x32x24 -nolisten tcp >"$T/xvfb.log" 2>&1 &
XVFB=$!
i=0
while [ ! -S /tmp/.X11-unix/X93 ]; do
  i=$((i + 1)); [ "$i" -gt 100 ] && { cat "$T/xvfb.log"; exit 1; }
  sleep 0.05
done
xprop -root -f _NET_CURRENT_DESKTOP 32c -set _NET_CURRENT_DESKTOP 0 >/dev/null
xprop -root -f _NET_CLIENT_LIST 32x -set _NET_CLIENT_LIST 0x0 >/dev/null

export HOME="$T/home" XDG_CONFIG_HOME="$T/home/.config"
mkdir -p "$XDG_CONFIG_HOME/niruwm/themes"
cp "$HERE"/themes/*.toml "$XDG_CONFIG_HOME/niruwm/themes/"

sample() {
  xwd -root -silent > "$T/bar.xwd"
  convert "$T/bar.xwd" -crop 1x1+500+16 +repage txt:- 2>/dev/null |
    sed -n 's/.*#\([0-9A-Fa-f]\{6\}\).*/\1/p' | head -1 | tr 'A-F' 'a-f'
}

check_theme() {
  theme=$1
  expected=$2
  cat > "$XDG_CONFIG_HOME/niruwm/config.toml" <<EOF
theme = "$theme"
[bar]
modules = ["workspaces", "clock"]
EOF
  "$BIN" >"$T/bar.log" 2>&1 &
  BAR=$!
  i=0
  while kill -0 "$BAR" 2>/dev/null && [ "$i" -lt 100 ]; do
    i=$((i + 1))
    got=$(sample 2>/dev/null || :)
    [ -n "$got" ] && { sleep 0.2; got=$(sample 2>/dev/null || :); [ "$got" = "$expected" ] && break; }
    sleep 0.05
  done
  [ "$got" = "$expected" ] || {
    echo "FAIL $theme: background $got, expected $expected"
    cat "$T/bar.log"
    exit 1
  }
  echo "PASS $theme: $got"
  kill "$BAR" 2>/dev/null || :
  wait "$BAR" 2>/dev/null || :
  BAR=""
  sleep 0.2
}

for theme in niru-noir niru-nord niru-satie niru-carl-larsson; do
  expected=$(sed -n 's/^background = "\([0-9A-Fa-f]*\)".*/\1/p' "$HERE/themes/$theme.toml" | head -1 | tr 'A-F' 'a-f')
  check_theme "$theme" "$expected"
done

cat > "$XDG_CONFIG_HOME/niruwm/themes/custom.toml" <<'EOF'
name = "custom"
background = "212121"
foreground = "f0f0f0"
muted = "aaaaaa"
border = "404040"
focus = "ffaa00"
EOF
check_theme custom 212121
echo "PASS rendered theme colours"
