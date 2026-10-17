# NIRUBAR

Minimal Rust/X11 status bar for [NIRUWM](https://github.com/nirucon/niruwm).
**0.2.0**, with **Niru Noir** as the built-in default.

The bar shows what you actually look at: clickable workspaces, the Swedish
weekday and ISO date, and the time (`lör 2026-10-17 16:48`). It is not a
system monitor.

## Implemented

- Clickable workspaces with active/occupied states.
- Swedish weekday, ISO 8601 date and `hh:mm`; built-in weekday names so no
  `sv_SE` locale is required.
- Optional focused-window title (`[bar] title = true`).
- Small anti-aliased Unicode TrueType renderer, without GTK/Qt or a GPU
  requirement, double buffered so redraws never flicker.
- X11 dock/strut properties and singleton protection.
- Blocking X11 event loop; a one-second tick repaints only when the rendered
  string changes.
- Runtime palette, font and height reload through `niructl reload`.
- Separate process: WM windows remain usable if the bar exits.

## Install

```sh
git clone https://github.com/nirucon/nirubar.git
cd nirubar
./install.sh
~/.local/bin/nirubar --check-config
```

NIRUWM starts `nirubar` from PATH automatically. For a standalone test in an
isolated X session, run `~/.local/bin/nirubar`. Do not expect integration with
unpatched dwm: this bar uses EWMH workspaces rather than dwm status text.
The installer preserves existing configuration and backs up an existing binary.

## Alpha limits / roadmap

One bar across the X screen for now; per-monitor panels and hotplug accompany
NIRUWM's monitor milestone. Audio, network, media/MPRIS, update counts, weather,
tray and module pop-ups are not implemented yet. The CPU/memory impact and redraw
frequency must be measured before calling this faster than the existing bar.
Unicode coverage depends on the selected font; there is no fallback-font chain yet.

## Build requirements

Rust/Cargo 1.75 or newer, a C linker, and an X11 session. No systemd dependency.
The bar needs fontconfig plus a TrueType font (JetBrainsMono Nerd Font is preferred,
DejaVu Sans Mono works). This first renderer targets 24/32-bit TrueColor screens.

```sh
# Arch
sudo pacman -S --needed rust base-devel fontconfig ttf-dejavu kitty rofi
# Debian
sudo apt install cargo rustc build-essential fontconfig fonts-dejavu-core kitty rofi
# Void (glibc)
sudo xbps-install -S rust cargo base-devel fontconfig dejavu-fonts-ttf kitty rofi
```

Void musl is not yet a tested target. CI builds on Debian, Arch and Void glibc;
these builds do not replace real hardware/session testing.

## Configuration and themes

Both programs read `~/.config/niru/config.toml` (or `$XDG_CONFIG_HOME/niru/config.toml`).
See `config.example.toml`. Commands are argument arrays, never passed through a shell.
**Niru Noir is the built-in default**, using NIRUPI's original palette:
`0f0f10`, `e5e5e5`, `a8a8a8`, `2a2a2d`, `5a5a60`.

Additional themes are files in `~/.config/niru/themes/NAME.toml`, following
`themes/niru-noir.toml`. Set `theme = "NAME"` and run `niructl reload`, or use
`niructl theme NAME`. This rewrites the TOML configuration while preserving values
(comments are not preserved). Invalid configurations/themes are rejected.
The working runtime configuration is retained when reload fails.

`font_size` and optional `font_path` control the bar's Unicode TrueType renderer.
Changing themes does not modify GTK, terminals or other applications yet.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
# Isolated X11 tests (install xvfb and xauth first):
xvfb-run -a cargo test --locked --test x11 -- --ignored --test-threads=1
```

Initial local verification: Rust 1.75, unit tests and real Xvfb integration tests.
No performance advantage over dwm is claimed without measurements.

MIT. Copyright 2026 Nicklas Rudolfsson.
