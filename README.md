# NIRUBAR

Small native X11 status bar with monitor-local workspaces, calendar, network,
battery, audio/media, brightness and optional weather, notifications, load and
update panels. Four NIRU themes plus custom themes.

## Install

Use [NIRUX](https://github.com/nirucon/nirux#install) for prebuilt installation
and dependencies on **Void glibc, Debian and Arch Linux** (x86_64/glibc).
Standalone source installation needs Rust 1.75+, a C linker and a monospace font:

```sh
git clone https://github.com/nirucon/nirubar
cd nirubar
./install.sh
```

Prefix: `NIRUBAR_PREFIX` (default `~/.local`). Existing settings are preserved.
Maintain NIRUX-owned installations with `nirux update`.

## Configure

Shared config: `~/.config/niruwm/config.toml` (XDG respected).
Weather: `~/.config/nirubar/config.toml`; weather is opt-in.

```sh
nirubar --check-config
niructl reload
niructl restart-bar
```

Clicks open information; media/volume information does not pause or mute.
Calendar: Monday first, ISO week numbers, Left/Right month navigation.
One bar per monitor; one XEmbed tray per X screen.

[Configuration](docs/CONFIGURATION.md) · [Example](config.example.toml) ·
[Roadmap](https://github.com/nirucon/nirux/blob/main/ROADMAP.md) ·
[X11 tests](tests/x11.rs)

MIT. Copyright 2026 Nicklas Rudolfsson.
