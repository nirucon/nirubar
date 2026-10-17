# Changelog

## 0.3.0

### Added

- Battery segment on laptops, with a charging bolt and level icons. Machines
  without a battery show nothing.
- Network icon for a wired or wireless link, read from `/sys/class/net` so no
  daemon or NetworkManager is needed.
- `[bar] battery`, `network` and `icons` settings.

### Changed

- The weekday is gone from the clock: `2026-10-17  21:16`.

### Fixed

- Icons are chosen per glyph from what the loaded font can draw, so a font
  without the full Nerd Font range falls back to plain text instead of drawing
  blank boxes.


## 0.2.0 — minimal bar, rebuilt

### Added

- Clickable NIRUWM workspaces with active/occupied states.
- Swedish weekday and ISO 8601 date plus `hh:mm` (`lör 2026-10-17 16:48`).
  Weekday names are built in, so no `sv_SE` locale is needed.
- Shared `[bar]` configuration with NIRUWM in `~/.config/niruwm/config.toml`.
- `--version` and `--check-config` unchanged; `niructl reload` now reloads the
  bar as well `niructl theme NAME` applies without restarting.

### Fixed

- **Text was almost invisible.** Glyph rasterisation applied the outline's
  pixel offset twice, so nearly every pixel fell outside the glyph box.
- **Flicker on every redraw.** The bar is now double buffered: the frame is
  composed on a pixmap and presented with one `CopyArea`.
- Bar clicks were delivered unreliably because the client message used the
  wrong event mask.
- The clock tick spawned a connection per redraw path; a single watchdog
  thread now wakes the bar once per second and repaints only when the
  rendered string changes.

### Changed

- Removed the ad hoc memory dump from the bar. It is a status line for
  NIRUWM, not a system monitor; segments return when they have popups and
  proper configuration.
- Configuration moved from `~/.config/niru/config.toml` to
  `~/.config/niruwm/config.toml`; the old path is still read.

## 0.1.0-alpha.1

First independently implemented Rust/X11 alpha. Niru Noir is the default.
