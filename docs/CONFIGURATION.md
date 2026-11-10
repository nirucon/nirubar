# Configuration

NIRUBAR shares theme, font and layout settings with NIRUWM in
`~/.config/niruwm/config.toml`. Weather, calendar and per-panel timeout
options have their own optional file: `~/.config/nirubar/config.toml`. Calendar feed URLs live in
`~/.config/nirubar/calendars.toml`.

## Modules

| Module | Shows |
|---|---|
| `workspaces` | Ten workspace cells; click to switch. A small accent marker shows occupied non-active workspaces. A cell with a window that asked for attention is filled with the theme's attention colour (see below); NIRUWM 0.19 lists those windows in `_NIRUWM_URGENT`, and each bar marks only its own screen's cells |
| `network` | Wired link; `!` means link up without a default route. Wireless appears here only with `wifi = false`; click for interface, route and address |
| `wifi` | Wi-Fi radio and connection; `!` means radio off or not connected; click to open the Wi-Fi selector |
| `battery` | Aggregated battery packs; panel shows charge, state and remaining time when available |
| `volume` | Output level or mute; click for info, scroll to change, right-click mute, middle-click microphone mute |
| `microphone` | Input mute and gain; right-click mute, scroll gain, middle-click source selector |
| `scratchpad` | NIRUWM scratchpads: a terminal mark (nf-fa-terminal, plain `>_`) while at least one exists, the mark and a count for several, accent while one is shown, nothing otherwise. Click: panel with key, slot, title and state per scratchpad; a row shows or hides it. Right-click: the default scratchpad. Reads the `_NIRUWM_SCRATCHPADS` root property (NIRUWM 0.17.4) only when it changes. A scratchpad that asked for attention turns the mark into the alert colour with a `!` (and its row starts with `!`); without this module a line in the attention colour under the workspace cells says the same |
| `dictation` | NIRUDICTATE: a red `●` while recording, an accent `…` while transcribing, followed by `AI` while the dictation goes to the opencode agent (NIRUDICTATE 0.2 agent mode, Shift+Alt+Space), nothing when idle or not installed; right-click stops and inserts (or sends), middle-click opens its settings. Reads `$XDG_RUNTIME_DIR/nirudictate/state.json` only when it changed and repaints at once on the `_NIRUDICTATE_STATE` root property |
| `recording` | NIRUCLIP indicator: `● rec MM:SS` while recording, `○ paused MM:SS` while paused (recorded time, pauses excluded), with `· cam off` (camera privacy image or switch) or `· cam !` (frozen picture, no frames, camera busy) when NIRUCLIP 0.4+ reports a camera problem; the popup names it; right-click stop and save, middle-click capture panel |
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
| `display` | This bar's screen position (`Left`, `Center`, `Right`, `Top`, `Bottom`, diagonals or `Mirror`); click for outputs, Return/Open controls for display profiles |
| `tailscale` | Tailscale peers; shown only when `tailscale` is installed. Click for peer state, Open controls opens `nirulauncher --tailscale` |
| `mode` | Active NIRU work mode (focus/presentation) and minutes left, and *studio* while NIRUWM's studio mode is on (`_NIRUWM_STUDIO`, NIRUWM 0.20); click shows `niructl mode stop` or `niructl studio off` |
| `tasks` | Running or waiting NIRU/OpenCode tasks from `niructl task list`; click for recent states, Open controls opens NIRU Actions |
| `agent` | OpenCode entry; click to type a short prompt and start OpenCode in a centred floating terminal |
| `dnd` | Bell or slashed bell; left click toggles DND, right click opens Quick Settings |
| `notifications` | NIRUNOTIFY history count; click for recent notifications |
| `quicksettings` | Gear entry that opens Quick Settings in the launcher |
| `share` | NIRUSHARE entry; opens the inbox with `nirulauncher --share` |
| `nirusec` | NIRUSEC state mark from its shared cache (refreshed with `nirusec --check --if-stale 300`) |
| `niruxupdate` | A newer NIRUX release than the installed one, or an installed release that needs a new login; silent otherwise |

The `updates` module uses `xbps-install -un` on Void, `pacman -Qu` on Arch and
`apt list --upgradable` on Debian. Results are cached in
`~/.cache/nirubar/updates` for `update_check_minutes`.

## Attention colour

Urgent workspace cells use a semantic attention colour. The ten built-in
themes each have one (an amber or orange that differs from the theme's focus
colour). A user theme may set `urgent = "rrggbb"` in its theme file (read by
NIRUWM and NIRUBAR 0.19 and later; other programs that read theme files may
not know the key yet), or `[colors] urgent` or `warning` in
`~/.config/niruwm/appearance.toml` sets it for the built-in themes. Without
either, a user theme gets an amber derived from its background.

## Shared example

```toml
theme = "niru-noir"
font_size = 15.0
weather_latitude = 59.33
weather_longitude = 18.07

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

Panels close on a click on the same module, a click elsewhere or Escape.
Quick-glance panels also close after `popup_timeout_seconds` (default 4;
0 keeps them open until dismissed). Reading panels ignore that timeout and
stay open until dismissed:

| Stays open until dismissed | Closes after `popup_timeout_seconds` |
| --- | --- |
| `clock` (calendar), `weather` (rain timeline), `tasks`, `pulse` (load graph), `notifications` (history) | every other panel: `volume`, `microphone`, `network`, `battery`, `bluetooth`, `media`, `powerprofile`, `brightness`, `updates`, `niruxupdate`, `uptime`, `display`, `sun`, `tailscale`, `quicksettings`, `mode`, `layout`, `recording`, `dictation`, `scratchpad`, tray menus, the `+N` list |

(`wifi`, `agent`, `share`, `nirusec` and `dnd` open their own control
window on a click rather than a panel, so no timeout applies to them.)

Any panel can have its own timeout in NIRUBAR's own file,
`~/.config/nirubar/config.toml`. Keys are the names from `[bar] modules`;
`calendar` is accepted for the clock's panel, `tray` for tray menus and
`overflow` for the `+N` list. Values are whole seconds, 0 to 86400, and 0
means until dismissed:

```toml
# ~/.config/nirubar/config.toml
[popup_timeout]
calendar = 0
volume = 2
weather = 20
```

Precedence: a `[popup_timeout]` entry, then the reading-panel default (0),
then `[bar] popup_timeout_seconds`. Without the table nothing changes for
quick panels. NIRUWM never reads this file, so its strict check of the shared
config is not affected; do not put `[popup_timeout]` in
`~/.config/niruwm/config.toml`, where both programs would reject it. Unknown
names and bad values are ignored and listed by `nirubar --check-config`.
`niructl reload` applies changes.

Media titles longer than `media_width` scroll and rest
at the beginning. At narrow widths, low-priority modules collapse into `+N`.
Battery packs are aggregated by energy when available. Weather is opt-in and
keeps its last successful result.

Reload after editing with `niructl reload` or Super+R. Validate with
`nirubar --check-config`.

## Calendar events

The clock's calendar popup can show events from read-only iCalendar (ICS)
feeds. There is no account login: each calendar is one URL.

1. Get the URL. Google Calendar: open calendar.google.com on a computer,
   Settings (gear) > under "Settings for my calendars" pick the calendar >
   "Integrate calendar" > copy "Secret address in iCal format". Nextcloud,
   iCloud (public link) and Outlook (published ICS link) work the same way.
2. Create `~/.config/nirubar/calendars.toml` with mode 0600, for example
   `install -m 600 calendars.example.toml ~/.config/nirubar/calendars.toml`,
   and fill in `name`, `url` and optionally `color` and `open`.
3. Run `nirubar --check-config`. The running bar notices the new or edited
   file within a few seconds by itself; `niructl reload` (or `Super+R`)
   applies it at once. A new or changed calendar is fetched straight away.

```toml
[[calendar]]
name = "Privat"
url = "https://calendar.google.com/calendar/ical/.../private-.../basic.ics"
color = "#7aa2f7"   # optional; else the theme accent, attention colour, foreground
open = "https://calendar.google.com/calendar/r/day/{yyyy}/{m}/{d}"  # optional
```

The secret address gives read access to anyone who has it. NIRUBAR refuses
the file unless only you can read it (mode 0600, owned by you) and says so in
`--check-config` and in the popup. It never logs, prints or shows a URL, and
passes it to curl on standard input rather than on the command line. If an
address leaks, reset it in Google Calendar's settings.

Non-secret options go in `~/.config/nirubar/config.toml` (NIRUWM does not read
that file, so the shared config is unchanged):

```toml
[calendar]
refresh_minutes = 15   # 5 to 1440
days_ahead = 7         # agenda: today and the next N days, 0 to 31
show_in_bar = false    # show the next event within 12 hours before the clock
```

Feeds are fetched in the background with curl (25 s limit, 16 MiB cap), never
while drawing. Parsed events are cached in `~/.cache/nirubar/calendar/`
(mode 0600) and stay visible offline; the popup says when they are stale.
Bars on several monitors share that cache: one fetches, the others wait for
it and read the result.

The bar re-reads `calendars.toml` and the `[calendar]` options on
`niructl reload` and whenever either file's size, modification or change
time, mode or inode changes (checked every 3 seconds, outside drawing; an
editor that saves through a new file is noticed too). Calendars whose URL
did not change keep their events; added or changed ones are fetched at once.
The 0600 check applies to every re-read: `chmod 644` turns calendars off
again, `chmod 600` turns them back on.

The popup marks each day that has events with a dot per calendar, lists
today and the next `days_ahead` days (time or "all day", title, a square in
the calendar's colour), and a click on a day or event opens the calendar's
`open` page for that date in the shared config's `browser`. Google feeds
open Google's day view by default.

### Reminders

Each timed event gets a notification before it starts, through the session's
notification service (NIRUNOTIFY, or any freedesktop notification daemon):
the title, the start time and how long until it, the location and the
calendar's name. A click on it opens the calendar's `open` page for that day
in the configured `browser`, like a click in the clock's panel.

```toml
remind_minutes = 10     # top of calendars.toml: every calendar, 0 to 1440; 0 is off

[[calendar]]
name = "Jobb"
url = "https://..."
remind_minutes = 15     # optional: this calendar only; 0 turns its reminders off
```

- Without the key a calendar is reminded 10 minutes before each event, so an
  existing `calendars.toml` gets reminders after an update. Set
  `remind_minutes = 0` at the top to turn them all off.
- All-day events have no reminder. The feed's own alarms (VALARM) are not
  used; `remind_minutes` decides.
- Every occurrence of a recurring event is reminded once. Times follow the
  event's time zone (TZID, UTC or floating) and daylight saving.
- A bar that starts within the lead time still reminds of an event that has
  not begun; an event that has started is not reminded.
- Bars on several monitors share the reminders, and a restarted bar does not
  repeat one: what was sent is kept in `~/.local/state/nirubar/reminders.json`
  (mode 0600) as hashes, without event titles, and forgotten three days after
  the event. Reminders are not logged.
- The clock module drives them: the bar looks for due reminders every 20
  seconds from the clock's once-a-second update, without another timer or
  network request. The clock must be in `[bar] modules`.
- A click works while the bar that sent the reminder runs; after a bar
  restart the notification still shows, but a click only closes it.
- `nirubar --check-config` lists each calendar's reminder time.

Supported iCalendar: VEVENT with DATE and DATE-TIME start/end (UTC, TZID or
floating), DURATION, all-day and multi-day events, line folding and text
escapes, SUMMARY and LOCATION, STATUS:CANCELLED, RRULE with
FREQ=DAILY/WEEKLY/MONTHLY/YEARLY, INTERVAL, COUNT, UNTIL, BYDAY (with
ordinals such as `-1FR`), BYMONTHDAY, BYMONTH, BYSETPOS and WKST, plus
EXDATE, RDATE and RECURRENCE-ID overrides (moved or cancelled instances).
TZIDs are read from the system zone database, including vendor-prefixed
names and common Windows names. A rule NIRUBAR does not understand (for
example FREQ=HOURLY or BYWEEKNO) shows the first instance only.

