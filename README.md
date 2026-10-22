# NIRUBAR

**Part of [NIRUX - A minimal X11 desktop by Nicklas Rudolfsson](https://github.com/nirucon/nirux).**

**Early beta.** A native X11 status bar for the NIRU desktop. It is a separate
process, draws only when values change, reserves screen space with an X11 strut
and reads system state directly from `/sys` where possible. No daemon or
NetworkManager is required.

## Features

- One bar per monitor with monitor-local workspaces.
- Font-aware layout, semantic theme colours and four built-in NIRU themes.
- Panels with measured hitboxes, Escape/click-outside dismissal and a timeout.
- XEmbed system tray per X screen.
- Narrow-width overflow: low-priority modules collapse into a clickable `+N`.
- Bounded provider queries and caching for package updates and weather.
- Shared configuration with NIRUWM; no duplicated palette or font settings.

## Modules

Add or reorder modules in `[bar] modules` in `~/.config/niruwm/config.toml`.

| Module | Shows | Interaction |
|---|---|---|
| `workspaces` | Ten workspace cells with active/occupied state and occupancy markers | Click a cell to switch workspace |
| `network` | Wired/wireless icon; `!` means link up without a default route | Click for interface, link, route and address |
| `battery` | Aggregated battery charge and state | Click for charge, state and remaining time when available |
| `volume` | Output level or mute icon | Click for info, scroll volume, right-click mute, middle-click microphone mute |
| `microphone` | Input mute and gain; `mic none`/`mic --` when unavailable | Click for input and gain, right-click mute, scroll gain, middle-click source selector |
| `brightness` | Backlight percentage; hidden without a readable backlight | Click for details |
| `media` | Playing title with progress hairline | Right-click play/pause, scroll previous/next, middle-click next player |
| `powerprofile` | Active power profile | Right-click next profile, middle-click launcher selector |
| `bluetooth` | Off, on or connected device | Right-click power, middle-click device selector |
| `updates` | Available package upgrades | Click for package-manager detail |
| `pulse` | CPU and memory blocks | Click for a one-minute graph |
| `uptime` | System uptime | Click for memory use and fullest filesystem |
| `sun` | Sunrise and sunset | Click for daylight progress |
| `weather` | Cached temperature, condition and next-hour rain | Click for an eight-hour rain timeline and daylight progress |
| `clock` | Date and time | Click for a Monday-first ISO calendar with week numbers |

The `updates` module uses `xbps-install -un` on Void, `pacman -Qu` on Arch and
`apt list --upgradable` on Debian. Results are cached in
`~/.cache/nirubar/updates` for `update_check_minutes`.

## Install

Use [NIRUX](https://github.com/nirucon/nirux#install) for the complete desktop
and dependencies. Standalone:

```sh
git clone https://github.com/nirucon/nirubar
cd nirubar
./install.sh
```

Prefix: `NIRUBAR_PREFIX` (default `~/.local`). Existing settings are preserved.
NIRUX-owned installations are maintained with `nirux update`.

## Use

NIRUWM starts the bar at login; it can also run on its own.

- Left click opens information and never changes playback or mute.
- Right click and scroll perform the explicit playback, mute and volume actions.
- Middle click opens the keyboard-first detail view where one exists.
- One panel is open at a time. Escape or a click elsewhere closes it.
- Media titles longer than `media_width` scroll and rest at the beginning.
- Battery packs are aggregated by energy when available; unknown values are not
  shown as zero.

## Configure

Shared settings live in `~/.config/niruwm/config.toml`. Weather is opt-in in
`~/.config/nirubar/config.toml`. Start from [config.example.toml](config.example.toml)
and see [configuration](docs/CONFIGURATION.md) for the module and settings
reference.

```toml
theme = "niru-noir"
font_size = 15.0
weather_latitude = 58.22
weather_longitude = 11.92

[bar]
modules = ["workspaces", "network", "battery", "clock"]
tags = ["1", "2", "3", "4", "5", "6", "7", "8", "9"]
icons = true
popups = true
popup_timeout_seconds = 8
tray = true
```

```sh
nirubar --check-config
niructl reload
niructl restart-bar
```

Themes: Niru Noir, Nord, Satie, Carl Larsson and custom themes. Colours and font
come from the shared NIRUWM appearance.

## NIRUX suite

[NIRUX](https://github.com/nirucon/nirux) · [NIRUWM](https://github.com/nirucon/niruwm) · [NIRUBAR](https://github.com/nirucon/nirubar) · [NIRUSCREENSAVER](https://github.com/nirucon/niruscreensaver) · [NIRULOCK](https://github.com/nirucon/nirulock) · [NIRULAUNCHER](https://github.com/nirucon/nirulauncher) · [NIRUNOTIFY](https://github.com/nirucon/nirunotify) · [NIRUSHOT](https://github.com/nirucon/nirushot)

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
sh tests/config-agreement.sh
```

MIT. Copyright 2026 Nicklas Rudolfsson.
