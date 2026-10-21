# NIRUBAR

Small native X11 status bar with monitor-local workspaces, calendar, network,
battery, audio/media, brightness, a multi-display count, work-mode countdown, microphone/Bluetooth status and optional weather,
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

### Microphone (optional)

Add `"microphone"` to `[bar] modules` in the shared config. Set
`microphone_visibility = "always"` (default), or `"muted"` to hide a healthy
unmuted input. Missing input/provider remains visible. The label shows
`mic mute`, input gain, `mic none` or `mic --`; unmuted never means recording.

- Left click: selected input, gain and mute information; no setting changes.
- Right click: toggle mute. Scroll: adjust input gain, preserving mute.
- Middle click: `nirulauncher --microphone` for keyboard source selection,
  mute and gain controls. The same commands are in Super+Shift+P.

Requires the suite's `niructl audio mic-status` / `mic-level` commands and
WirePlumber (`wpctl`). A single bounded worker polls every two seconds; the
X11 render/input thread never waits for the audio provider. Gain is the
configured input level, not a live sound meter.

### Display profiles

The display panel (middle click) opens `nirulauncher --displays`, which lists
saved `niructl display` profiles. Apply a profile with preview/keep/revert;
reading the panel does not change the layout.

### Bluetooth (optional)

Add `"bluetooth"` to `[bar] modules`. The module is hidden when no adapter
exists; otherwise it shows off, on, or the connected device. Its panel lists
paired devices and battery only when BlueZ reports it. Right click toggles the
adapter, middle click opens `nirulauncher --bluetooth` for keyboard
connect/disconnect/power controls. Requires BlueZ (`bluetoothctl`, installed by
NIRUX) and the distro Bluetooth service. Reading status/panel never pairs or
connects; unknown battery is shown as unknown, not 0%.

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
