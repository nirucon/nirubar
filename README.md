# NIRUBAR

The status bar for [NIRUWM](https://github.com/nirucon/niruwm).

**A personal hobby project.** I build this for my own machines. It is public so
others can read it and try it, but there is no support promise. Early software,
used at your own risk.

## What it is

Modules, configured in one line each:

| Module | Shows |
| --- | --- |
| `workspaces` | nine cells, click one to switch |
| `network` | wired or wireless icon while a link is up |
| `battery` | charge and a charging bolt, laptops only |
| `volume` | level with an icon, or a mute icon; click to mute |
| `media` | what is playing, `artist - title`, scrolling when it is long |
| `updates` | available package upgrades; click for the detail and the command |
| `clock` | date and time |

Click a module for a panel: a month calendar, network name and address,
battery detail. One panel is open at a time and it closes on a click on the
same module, a click anywhere else, Escape, or after
`popup_timeout_seconds`.

## How it works

NIRUBAR is a separate process. NIRUWM starts it at login and it also runs on
its own. It reserves screen space with an X11 strut and only redraws when
something it shows changes. System values come from `/sys`, so no daemon or
NetworkManager is involved.

## Install

No Rust or compiler: the installer downloads a static binary.

```sh
git clone https://github.com/nirucon/nirubar
cd nirubar
./install.sh
```

Then start NIRUWM, which starts the bar. To build from source (Rust 1.75 or
newer): `cargo build --release --locked`.

## Settings

Shared with NIRUWM in `~/.config/niruwm/config.toml`:

```toml
theme = "niru-noir"        # niru-noir, niru-nord, niru-satie, niru-carl-larsson
font_size = 15.0

[bar]
modules = ["workspaces", "network", "battery", "clock"]
tags = ["1", "2", "3", "4", "5", "6", "7", "8", "9"]
date_format = "%Y-%m-%d"
time_format = "%H:%M"
icons = true               # Nerd Font glyphs, plain text where missing
popups = true              # click a module for detail
popup_timeout_seconds = 8  # 0 keeps a panel until dismissed
media_width = 32           # characters kept from the media title
update_check_minutes = 30  # package checks; the count is cached
```

The `updates` module reads the package manager of the distribution it runs on:
`xbps-install -un` on Void, `pacman -Qu` on Arch and `apt list --upgradable`
on Debian. The result is cached in `~/.cache/nirubar/updates` for
`update_check_minutes`, so the bar never runs a package query while redrawing.
On a distribution without one of those it shows nothing.

A title longer than `media_width` scrolls twice, pauses between passes, and
rests showing the beginning, so the artist stays readable. A short title is
shown as it is.

The media keys work through NIRUWM, not the bar: volume up and down, mute, play
and pause, next and previous. They need `wpctl` (or `amixer`) and `playerctl`.

`tags` takes words or glyphs, for example `["web", "code", "chat"]` or a Nerd
Font code point.

**Reload after editing:** `niructl reload`, or press Super+R in NIRUWM.
Check a file without applying it with `nirubar --check-config`.

## Status

Version 0.11.0. Missing: trays, per-monitor bars.

## Build needs

Only when building yourself: Rust 1.75 or newer, a C linker, `fontconfig` and
a monospace font.

## License

MIT. Copyright 2026 Nicklas Rudolfsson.
