#!/bin/sh
# One file, two programs.
#
# NIRUWM and NIRUBAR read the same ~/.config/niruwm/config.toml, and both are
# strict about unknown fields so a typo is caught rather than ignored. That
# makes a setting that only one of them knows a session that will not start:
# 0.14.0 put the bar's `weather_latitude` under `[bar]`, and NIRUWM refused the
# whole file, so the desktop never came up. This script is the check that
# catches it before a release does.
#
#   sh tests/config-agreement.sh BINDIR
set -eu

BINDIR="${1:?usage: config-agreement.sh BINDIR}"
HERE="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
export HOME="$(mktemp -d)"
export XDG_CONFIG_HOME="$HOME/.config"
mkdir -p "$XDG_CONFIG_HOME/niruwm/themes"
cp "$HERE/config.example.toml" "$XDG_CONFIG_HOME/niruwm/config.toml"
cp "$HERE/themes/niru-noir.toml" "$XDG_CONFIG_HOME/niruwm/themes/"

fail() { echo "FAIL $*"; exit 1; }
ok() { echo "OK   $*"; }

# The example, exactly as shipped.
"$BINDIR/nirubar" --check-config >/tmp/agree-bar.out 2>&1 ||
  { cat /tmp/agree-bar.out; fail "nirubar refuses config.example.toml"; }
"$BINDIR/niruwm" --check-config >/tmp/agree-wm.out 2>&1 ||
  { cat /tmp/agree-wm.out; fail "niruwm refuses config.example.toml"; }
ok "config.example.toml is accepted by both programs"

# Every top-level and [bar] key the bar knows, written out the way a person
# would, and handed to the window manager. Values are deliberately ordinary.
cat >>"$XDG_CONFIG_HOME/niruwm/config.toml" <<'EOF'

# A bar setting belongs to the bar, but the file is shared.
weather_latitude = 59.33
weather_longitude = 18.07
graph_width = 32
EOF

"$BINDIR/nirubar" --check-config >/tmp/agree-bar.out 2>&1 ||
  { cat /tmp/agree-bar.out; fail "nirubar refuses its own settings"; }
"$BINDIR/niruwm" --check-config >/tmp/agree-wm.out 2>&1 ||
  { cat /tmp/agree-wm.out; fail "niruwm refuses a bar setting"; }
ok "the window manager accepts every setting the bar knows"

# And the other direction: a window-manager setting must not upset the bar.
cat >>"$XDG_CONFIG_HOME/niruwm/config.toml" <<'EOF'

[scratchpad]
command = ["kitty", "--class", "scratchpad"]
EOF
"$BINDIR/nirubar" --check-config >/tmp/agree-bar.out 2>&1 ||
  { cat /tmp/agree-bar.out; fail "nirubar refuses a window-manager section"; }
ok "the bar accepts every section the window manager knows"
