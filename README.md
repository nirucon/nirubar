# NIRUBAR

Small native X11 status bar for [NIRUWM](https://github.com/nirucon/niruwm), with
workspace, clock/calendar, network, battery, audio/media, updates, load, uptime
and sunrise/sunset modules. Niru Noir by default; Nord, Satie, Carl Larsson and
custom themes are also supported. Personal software for the author's machines.

## Install

Use [NIRUX](https://github.com/nirucon/nirux#install) for prebuilt installation on
**Void glibc, Arch and Debian**. Independently, with Rust/Cargo, a C linker and
a monospace font installed:

```sh
git clone https://github.com/nirucon/nirubar
cd nirubar
./install.sh
```

The script builds from source, preserves existing settings and respects
`NIRUBAR_PREFIX` (default `~/.local`; legacy `NIRU_PREFIX` is accepted).
Maintain an existing NIRUX-owned installation with `nirux update`.

## Configure

Shared settings: `~/.config/niruwm/config.toml` (XDG respected).
See [config.example.toml](config.example.toml) and the
[module/configuration reference](docs/CONFIGURATION.md).

```sh
nirubar --check-config
niructl reload
niructl restart-bar
```

**0.16.9:** target-correct filesystem bindings, popup/reload pixmap cleanup and
information-only volume/media clicks. Playback and volume use explicit player
controls/media keys. Per-monitor bars, improved panels, weather and fixed-cadence
background probes remain in the [roadmap](https://github.com/nirucon/nirux/blob/main/ROADMAP.md).

[Code audit](https://github.com/nirucon/nirux/blob/main/AUDIT.md) · [X11 tests](tests/x11.rs)

MIT. Copyright 2026 Nicklas Rudolfsson.
