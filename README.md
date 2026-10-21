# NIRUBAR

Native X11 status bar for the NIRU desktop: monitor-local workspaces, calendar,
network, battery, audio/media, brightness, microphone, Bluetooth, power profile,
weather, notifications, load, updates and a system tray.

Part of the [NIRUX suite](https://github.com/nirucon/nirux): [NIRUWM](https://github.com/nirucon/niruwm) · [NIRUBAR](https://github.com/nirucon/nirubar) · [NIRUSCREENSAVER](https://github.com/nirucon/niruscreensaver) · [NIRULOCK](https://github.com/nirucon/nirulock) · [NIRULAUNCHER](https://github.com/nirucon/nirulauncher) · [NIRUNOTIFY](https://github.com/nirucon/nirunotify) · [NIRUSHOT](https://github.com/nirucon/nirushot).

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

NIRUWM starts the bar at login; it can also run on its own. It reserves screen
space with an X11 strut and redraws only when displayed values change. System
values come from `/sys`, so no daemon or NetworkManager is required.

- One bar per monitor and one XEmbed tray per X screen.
- Left click opens information; right click and scroll perform the explicit
  playback, mute or volume actions shown by each module.
- Middle click opens the keyboard-first detail view where one exists.
- One panel is open at a time. Escape or a click elsewhere closes it.
- Narrow layouts collapse lower-priority modules into a clickable `+N` panel.

## Configure

Shared settings live in `~/.config/niruwm/config.toml`. Weather is opt-in in
`~/.config/nirubar/config.toml`. Start from [config.example.toml](config.example.toml)
and see [configuration](docs/CONFIGURATION.md) for modules, panels and settings.

```sh
nirubar --check-config
niructl reload
niructl restart-bar
```

Themes: Niru Noir, Nord, Satie, Carl Larsson and custom themes. Colours and font
come from the shared NIRUWM appearance.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
sh tests/config-agreement.sh
```

MIT. Copyright 2026 Nicklas Rudolfsson.
