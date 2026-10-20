# Configuration and operation

The status bar for [NIRUWM](https://github.com/nirucon/niruwm).

**A personal hobby project.** I build this for my own machines. It is public so
others can read it and try it, but there is no support promise. Early software,
used at your own risk.

## What it is

Modules, configured in one line each:

| Module | Shows |
| --- | --- |
| `workspaces` | nine cells, click one to switch |
| `network` | wired or wireless icon; `!` means link up without a default route; click for link, route, interface and address |
| `battery` | aggregated laptop packs; panel shows charge, state and remaining time when available |
| `volume` | level with an icon, or a mute icon; click for info, scroll to change, right-click mute, middle-click mic mute |
| `brightness` | laptop backlight percentage; hidden without a backlight device |
| `media` | what is playing, `artist - title`, scrolling when it is long, with a thin progress line; right-click play/pause, scroll previous/next, middle-click next player |
| `updates` | available package upgrades; click for the detail and the command |
| `pulse` | cpu and memory, click for a graph of the last minute |
| `uptime` | how long the machine has been on |
| `sun` | sunrise and sunset for your latitude; panel shows daylight progress |
| `weather` | cached temperature, condition and next-hour rain chance; panel has an eight-hour rain timeline and daylight progress |
| `clock` | date and time |

A system tray is on by default: applications that show a tray icon appear at
the right end of the bar.

Click a module for a panel: a month calendar, network name and address,
battery detail. Reading a panel never changes playback or mute; only the
explicit right/middle/scroll controls do. One panel is open at a time and it
closes on a click on the same module, a click anywhere else, Escape, or after
`popup_timeout_seconds`.

## How it works

NIRUBAR is a separate process. NIRUWM starts it at login and it also runs on
its own. It reserves screen space with an X11 strut and only redraws when
something it shows changes. System values come from `/sys`, so no daemon or
NetworkManager is involved.

## Install

Use [NIRUX](https://github.com/nirucon/nirux#install) for prebuilt installation.
The independent install script builds from source and needs Rust/Cargo and a C linker.

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
weather_latitude = 58.22   # sun module location, degrees north/east
weather_longitude = 11.92

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
tray = true                # own the session's tray
title_delay_seconds = 0    # with title = true: wait before showing a title
graph_width = 24           # characters in the pulse graph
```

The `updates` module reads the package manager of the distribution it runs on:
`xbps-install -un` on Void, `pacman -Qu` on Arch and `apt list --upgradable`
on Debian. The result is cached in `~/.cache/nirubar/updates` for
`update_check_minutes` to reduce package queries. Refreshes are currently synchronous;
bounded background work is tracked in the roadmap.
On a distribution without one of those it shows nothing.

A title longer than `media_width` scrolls twice, pauses between passes, and
rests showing the beginning, so the artist stays readable. A short title is
shown as it is.

`media` shows a drawn music mark (three bars, painted, so it never changes
width when the font changes), scrolls a long title to the end, rests showing
the beginning, and draws a hairline under the title that fills as the track
plays. The bar repaints at the frame rate only while that line moves.

Modules are separated by a hairline in the active workspace's colour, placed
halfway between them, rather than a `|` character, which was as loud as the
text it separated.

`pulse` draws cpu, memory and the fullest filesystem as block characters.
It samples at most once per second regardless of animation/repaint rate.
`uptime` adds two numbers in its panel that the rest of the bar does not show:
memory in use and the fullest filesystem.

`sun` computes sunrise and sunset locally with the standard solar position
formula; there is no network call and no key to set. Inside the polar day or
night it shows nothing, and the panel explains why.

The media keys work through NIRUWM, not the bar: volume up and down, mute, play
and pause, next and previous. They need `wpctl` (or `amixer`) and `playerctl`.

`tags` takes words or glyphs, for example `["web", "code", "chat"]` or a Nerd
Font code point.

**Reload after editing:** `niructl reload`, or press Super+R in NIRUWM.
Check a file without applying it with `nirubar --check-config`.

## Status

Version 0.16.7. Missing: per-monitor bars in this release.

Filesystem statistics use target-correct libc bindings. Regression checks cover
adjacent storage on glibc and the shipping musl target.

Part of a personal desktop for the author's own use; others are welcome to try it.
[Roadmap](https://github.com/nirucon/nirux/blob/main/ROADMAP.md).
Audio/media probes are sampled once per second, independently of scrolling.
The event loop uses deadlines, not an always-running animation thread.

## Build needs

Only when building yourself: Rust 1.75 or newer, a C linker, `fontconfig` and
a monospace font.

## License

MIT. Copyright 2026 Nicklas Rudolfsson.
