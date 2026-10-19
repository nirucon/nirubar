#!/bin/sh
set -eu
cd "$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
case "${1:-}" in "") ;; *) echo "Usage: ./install.sh" >&2; exit 2;; esac
command -v cargo >/dev/null || { echo "Install Rust/Cargo and a C linker first (see README)." >&2; exit 1; }
bindir="${NIRUBAR_PREFIX:-${NIRU_PREFIX:-$HOME/.local}}/bin"
case "$(readlink "$bindir/nirubar" 2>/dev/null || :)" in
 *share/nirux/current/bin/*) echo 'NIRUBAR is managed by NIRUX; use nirux update.' >&2; exit 1 ;;
esac
cargo build --release --locked
configdir="${XDG_CONFIG_HOME:-$HOME/.config}/niruwm"
mkdir -p "$bindir" "$configdir/themes"
stamp=$(date +%Y%m%d-%H%M%S)
binary=nirubar
if [ -f "$bindir/$binary" ]; then cp -p "$bindir/$binary" "$bindir/$binary.backup-$stamp"; fi
install -m 755 "target/release/$binary" "$bindir/$binary.new"
mv "$bindir/$binary.new" "$bindir/$binary"
if [ ! -e "$configdir/config.toml" ]; then install -m 644 config.example.toml "$configdir/config.toml"; fi
for theme in themes/*.toml; do
 target="$configdir/themes/$(basename "$theme")"
 if [ ! -e "$target" ]; then install -m 644 "$theme" "$target"; fi
done

echo "Installed $("$bindir/nirubar" --version) in $bindir. Existing configuration preserved."
