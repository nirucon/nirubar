# NIRUBAR

The status bar for [NIRUWM](https://github.com/nirucon/niruwm).

**A personal hobby project.** I build this for my own machines. It is public
so others can read it and try it, but there is no support promise. Treat it as
alpha software and use it at your own risk.

## What it is

A minimal X11 bar in Rust that draws:

- nine clickable workspaces (active and occupied states)
- the Swedish weekday and ISO date, e.g. `lör 2026-10-17`
- the time, `hh:mm`

That is all. It is a status line, not a system monitor. Text is rendered with
a small built-in TrueType renderer, so no GTK, Qt or GPU is needed.

## How it works

NIRUBAR is a separate process. NIRUWM starts it at login, and it can also be
run on its own. It reserves screen space with an X11 strut, so windows are
never hidden behind it. Clicking a workspace switches NIRUWM to it through the
standard EWMH protocol.

Both programs share `~/.config/niruwm/config.toml`. The bar only reads the
`[bar]` section and the theme; it never rejects settings meant for the window
manager.

## Install

No Rust or compiler is required: the installer downloads a static binary.

```sh
git clone https://github.com/nirucon/nirubar
cd nirubar
./install.sh
```

Then start NIRUWM (which starts the bar automatically). To build from source
instead (Rust 1.75+):

```sh
cargo build --release --locked
```

## Configuration

```toml
font_size = 15.0
# font_path = "/path/to/monospace.ttf"

[bar]
title = false      # show the focused window title
cell_width = 30
```

Apply changes with `niructl reload`.

## Status

Version 0.2.0. Shows workspaces, date and time.

Not there yet: battery, volume, network, media, trays, popups, per-monitor
bars, and a configurable date format.

## Build needs (only when building yourself)

Rust 1.75+, a C linker, `fontconfig` and a monospace font.

## License

MIT. Copyright 2026 Nicklas Rudolfsson.
