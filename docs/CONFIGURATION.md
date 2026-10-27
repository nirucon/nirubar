# Configuration

NIRUBAR shares theme, font and layout settings with NIRUWM in
`~/.config/niruwm/config.toml`. Weather has its own optional file:
`~/.config/nirubar/config.toml`.

## Modules

| Module | Shows |
|---|---|
| `workspaces` | Ten workspace cells; click to switch. A small accent marker shows occupied non-active workspaces |
| `network` | Wired link; `!` means link up without a default route. Wireless appears here only with `wifi = false`; click for interface, route and address |
| `wifi` | Wi-Fi radio and connection; `!` means radio off or not connected; click to open the Wi-Fi selector |
| `battery` | Aggregated battery packs; panel shows charge, state and remaining time when available |
| `volume` | Output level or mute; click for info, scroll to change, right-click mute, middle-click microphone mute |
| `microphone` | Input mute and gain; right-click mute, scroll gain, middle-click source selector |
| `brightness` | Backlight percentage; hidden without a readable backlight |
| `media` | Playing title; right-click play/pause, scroll previous/next, middle-click next player |
| `powerprofile` | Active power profile; right-click next profile, middle-click launcher selector |
| `layout` | Current NIRUWM layout; click for the list. Scroll shows `scroll 1`/`scroll 2` and lists the column controls |
| `bluetooth` | Adapter/device state; right-click power, middle-click device selector |
| `updates` | Available package updates; click for details and command |
| `pulse` | CPU and memory; click for a graph of the last minute |
| `uptime` | Uptime; panel also shows memory use and fullest filesystem |
| `sun` | Sunrise and sunset for the configured coordinates |
| `weather` | Cached temperature, condition and rain; panel shows an eight-hour timeline |
| `clock` | Date and time |

The `updates` module uses `xbps-install -un` on Void, `pacman -Qu` on Arch and
`apt list --upgradable` on Debian. Results are cached in
`~/.cache/nirubar/updates` for `update_check_minutes`.

## Shared example

```toml
theme = "niru-noir"
font_size = 15.0
weather_latitude = 58.22
weather_longitude = 11.92

[bar]
modules = ["workspaces", "network", "wifi", "battery", "clock"]
tags = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"]
date_format = "%Y-%m-%d"
time_format = "%H:%M"
icons = true
popups = true
popup_timeout_seconds = 8
media_width = 32
update_check_minutes = 30
tray = true
title_delay_seconds = 0
graph_width = 24
microphone_visibility = "always"
```

`tray = true` shows both XEmbed and D-Bus StatusNotifierItem icons on the
primary monitor. SNI menus open in a selectable bar panel; left click activates
an item, right click opens its menu when it has one, and items without a menu
receive the secondary action. The strip scales with the normal bar geometry.

On high-DPI displays the bar reads `Xft.dpi` at startup and scales `font_size`,
`bar_height` and `bar.cell_width`. Set `NIRU_UI_SCALE` (for example `1.25`) to
override detection, or `1` to disable it. The value is bounded to 0.75–2.5.

`tags` accepts words or glyphs, for example `["web", "code", "chat"]` or a
Nerd Font code point. `microphone_visibility = "muted"` hides a healthy unmuted
input; missing providers remain visible.

## Optional modules

### Wi-Fi

The `wifi` module appears automatically after `network` when an adapter exists,
including while the radio is off. Left click opens `nirulauncher --wifi`; the
bar does not keep a separate network editor. Set `wifi = false` in `[bar]` to
keep wireless state in the combined `network` module instead.

### Microphone

Requires WirePlumber (`wpctl`) and `niructl audio` support. A single bounded
worker polls every two seconds; the X11 thread never waits for the audio
provider. Gain is the configured input level, not a recording indicator.

```sh
niructl audio mic-status
niructl audio mic-mute on
niructl audio mic-level up
nirulauncher --microphone
```

### Bluetooth

Requires BlueZ and the distribution Bluetooth service. The module is hidden
when no adapter exists. The service is enabled by a normal NIRUX install.

```sh
niructl bluetooth status
niructl bluetooth power toggle
nirulauncher --bluetooth
```

### Power profiles

Requires `power-profiles-daemon` and `powerprofilesctl`. Only profiles reported
by the provider are offered; unsupported or denied changes are reported.

```sh
niructl powerprofile status
niructl powerprofile set balanced
nirulauncher --power
```

### Display profiles

The display panel opens `nirulauncher --displays`, which lists saved
`niructl display` profiles with preview, keep and revert actions. Opening the
panel never applies a layout.

## Display and panels

Panels close on the same module, a click elsewhere, Escape or
`popup_timeout_seconds`. Media titles longer than `media_width` scroll and rest
at the beginning. At narrow widths, low-priority modules collapse into `+N`.
Battery packs are aggregated by energy when available. Weather is opt-in and
keeps its last successful result.

Reload after editing with `niructl reload` or Super+R. Validate with
`nirubar --check-config`.

MIT. Copyright 2026 Nicklas Rudolfsson.
