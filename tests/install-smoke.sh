#!/bin/sh
set -eu
SOURCE=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
export HOME="$fixture/home" XDG_CONFIG_HOME="$fixture/config" NIRUBAR_PREFIX="$fixture/prefix"
mkdir -p "$HOME"
sh "$SOURCE/install.sh"
test -f "$XDG_CONFIG_HOME/niruwm/config.toml"
test ! -e "$XDG_CONFIG_HOME/niru/config.toml"
"$NIRUBAR_PREFIX/bin/nirubar" --check-config
rm "$NIRUBAR_PREFIX/bin/nirubar"
ln -s "$fixture/share/nirux/current/bin/nirubar" "$NIRUBAR_PREFIX/bin/nirubar"
if sh "$SOURCE/install.sh" > "$fixture/refusal.log" 2>&1; then
  printf '%s\n' 'FAIL: replaced a NIRUX-owned bar' >&2; exit 1
fi
test -L "$NIRUBAR_PREFIX/bin/nirubar"
printf '%s\n' 'PASS: canonical bar configuration path and NIRUX ownership refusal'
