# NIRUBAR

Small native X11 status bar with monitor-local workspaces, calendar, network,
battery, audio/media, brightness, a multi-display count and optional weather,
notifications, load and update panels. Four NIRU themes plus custom themes.

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

At narrow widths the workspace cells keep their configured width and the
lowest-priority status modules collapse into a clickable `+N` overflow panel.
Battery packs are aggregated (weighted by energy when available) and remaining
time is shown only when the kernel reports a power rate. A network icon with
`!` means the link is up but no default route exists; the panel separates link,
route and address. The optional weather cell adds the next-hour rain chance, and
its panel shows a cached eight-hour rain timeline and daylight progress.
Left-click opens information and never changes playback or mute. Media:
right-click play/pause, scroll previous/next, middle-click next MPRIS player.
Volume: right-click mute output, scroll volume, middle-click mute microphone.
Calendar: Monday first, ISO week numbers, Left/Right month navigation.
One bar per monitor; one XEmbed tray per X screen.

[Configuration](docs/CONFIGURATION.md) · [Example](config.example.toml) ·
[Roadmap](https://github.com/nirucon/nirux/blob/main/ROADMAP.md) ·
[X11 tests](tests/x11.rs)

MIT. Copyright 2026 Nicklas Rudolfsson.
