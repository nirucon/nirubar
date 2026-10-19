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

**0.18.2:** adds an optional `notifications` unread indicator and history panel backed by nirunotify. **0.18.1:** keeps deliberately configured `0,0` coordinates distinct from a missing weather location. **0.18.0:** adds an opt-in `weather` module backed by Open-Meteo, with a background refresh through the bounded probe runner, a versioned atomic cache, stale/offline states and a detailed panel. Own settings live in `~/.config/nirubar/config.toml`; legacy shared coordinates remain the fallback. **0.17.2:** accepts the shared `nirupi_compat` setting used by the window manager's explicit NIRUPI compatibility profile. **0.17.1:** adds a laptop `brightness` module and accepts shared `[nightlight]` settings. **0.17.0:** one bar per monitor on a shared X screen, with a global EWMH desktop
contract from niruwm, monitor-local workspace cells and global workspace commands.
**0.16.11:** corrected XEmbed tray embedding (mapped manager, real `_XEMBED`
notification) and a tray selection loss no longer terminates the bar. Also includes
fixed-cadence load sampling, correct ALSA parsing, locale-stable/failure-aware
update queries, target-correct filesystem bindings, popup/reload pixmap cleanup and
information-only volume/media clicks. Playback and volume use explicit player
controls/media keys. Improved panels and weather remain in the [roadmap](https://github.com/nirucon/nirux/blob/main/ROADMAP.md).

[Code audit](https://github.com/nirucon/nirux/blob/main/AUDIT.md) · [X11 tests](tests/x11.rs)

MIT. Copyright 2026 Nicklas Rudolfsson.
