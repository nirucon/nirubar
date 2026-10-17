# NIRUBAR

The status bar for [NIRUWM](https://github.com/nirucon/niruwm).

**A personal hobby project.** I build this for my own machines. It is public so
others can read it and try it, but there is no support promise. Early software,
used at your own risk.

## What it is

A minimal X11 bar in Rust that draws:

- nine clickable workspaces, active and occupied
- a battery indicator on laptops, with a charging bolt
- a network icon while a wired or wireless link is up
- the ISO date and the time, for example `2026-10-17  21:16`

No weekday, no decoration. Text is drawn with a small built in TrueType
renderer, so no GTK, Qt or GPU is needed.

## How it works

NIRUBAR is a separate process. NIRUWM starts it at login, and it also runs on
its own. It reserves screen space with an X11 strut, so windows are never
hidden behind it. Clicking a workspace switches NIRUWM to it through the
standard EWMH protocol.

System values are read straight from the kernel (`/sys/class/power_supply` for
the battery, `/sys/class/net` for the link), so no daemon, D-Bus or
NetworkManager is involved. A machine without a battery shows no battery
segment at all.

The bar only redraws when something it shows changes.

## Install

No Rust or compiler: the installer downloads a static binary.

```sh
git clone https://github.com/nirucon/nirubar
cd nirubar
./install.sh
```

Then start NIRUWM, which starts the bar. To build from source (Rust 1.75 or
newer):

```sh
cargo build --release --locked
```

## Settings

Shared with NIRUWM in `~/.config/niruwm/config.toml`:

```toml
font_size = 15.0
# font_path = "/path/to/monospace.ttf"

[bar]
enabled = true
title = false      # show the focused window title
cell_width = 30
battery = true
network = true
icons = true       # Nerd Font glyphs; falls back to plain text per glyph
```

The bar picks icons per glyph, so a font without the full Nerd Font range still
looks right. With `icons = false` every segment uses plain ASCII labels.

Apply changes with `niructl reload`.

## Status

Version 0.3.0. Shows workspaces, battery, network, date and time.

Missing: media, updates, trays, popups, per monitor bars, and a configurable
date format.

## Build needs

Only when building yourself: Rust 1.75 or newer, a C linker, `fontconfig` and
a monospace font.

## License

MIT. Copyright 2026 Nicklas Rudolfsson.
