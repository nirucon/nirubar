//! Media and volume modules.
//!
//! Both read the session without a daemon: volume and mute come from
//! PipeWire through `wpctl`, falling back to ALSA, and the player comes from
//! MPRIS over D-Bus when `playerctl` is installed. Every command is short-lived
//! and cached, so the bar never polls in a tight loop.
//!
//! Icons are Nerd Font glyphs chosen per glyph by the bar, so a font without
//! them falls back to plain text instead of drawing empty boxes.

use std::process::Command;

/// Run a bounded command and return trimmed stdout, or `None`.
fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = crate::probe::run(
        program,
        args,
        &[],
        std::time::Duration::from_secs(2),
        64 * 1024,
    )?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Parse the first `[NN%]` from `amixer get` output.
///
/// A plain `split('[')` followed by `ends_with("%]")` failed on the real
/// `"[45%] [on]"` line, where the first segment also contains `"[on]"`.
fn parse_amixer_percent(text: &str) -> Option<u32> {
    text.split('[')
        .skip(1)
        .find_map(|part| part.split("%]").next()?.trim().parse::<u32>().ok())
}

fn run_owned(program: &str, args: &[String]) -> Option<String> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run(program, &refs)
}

fn player_args(player: Option<&str>) -> Vec<String> {
    match player {
        Some(player) if !player.is_empty() => vec!["-p".into(), player.into()],
        _ => Vec::new(),
    }
}

fn player_query(player: Option<&str>, args: &[&str]) -> Option<String> {
    let mut full = player_args(player);
    full.extend(args.iter().map(|arg| (*arg).to_string()));
    run_owned("playerctl", &full)
}

/// Command that performs a media action for the selected player.
pub fn player_command(player: Option<&str>, action: &str) -> (String, Vec<String>) {
    let mut args = player_args(player);
    args.push(action.into());
    ("playerctl".into(), args)
}

fn which(program: &str) -> bool {
    std::env::var("PATH")
        .map(|p| {
            p.split(':')
                .any(|d| std::path::Path::new(d).join(program).is_file())
        })
        .unwrap_or(false)
}

// ── Volume ──────────────────────────────────────────────────────────────────

/// The current output volume.
#[derive(Clone, Debug, PartialEq)]
pub struct Volume {
    /// 0..=100, or larger when the volume was raised past 100%.
    pub percent: u32,
    pub muted: bool,
}

impl Volume {
    /// Read the default sink volume.
    ///
    /// `wpctl` is preferred because it is what PipeWire systems use; `amixer`
    /// covers plain ALSA and older setups.
    pub fn read() -> Option<Self> {
        if which("wpctl") {
            if let Some(text) = run("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]) {
                // "Volume: 0.45" or "Volume: 0.45 [MUTED]"
                let muted = text.contains("[MUTED]");
                let value = text
                    .split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse::<f32>().ok())?;
                return Some(Self {
                    percent: (value * 100.0).round() as u32,
                    muted,
                });
            }
        }
        if which("amixer") {
            if let Some(text) = run("amixer", &["get", "Master"]) {
                let percent = parse_amixer_percent(&text)?;
                let muted = text.contains("[off]");
                return Some(Self { percent, muted });
            }
        }
        None
    }

    /// Step the volume up or down by `delta` percent, unmuting if muted.
    ///
    /// Works through the same tools `read` uses, so the bar and the media keys
    /// always agree.
    #[allow(dead_code)] // used by NIRUWM's media keys through handle_key
    pub fn step(delta: i32) -> bool {
        let arg = if delta > 0 {
            format!("{delta}%+")
        } else {
            format!("{}%-", -delta)
        };
        if which("wpctl") {
            // Unmute first: raising the volume on a muted sink looks broken.
            if let Some(v) = Self::read() {
                if v.muted {
                    let _ = Command::new("wpctl")
                        .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "0"])
                        .status();
                }
            }
            return Command::new("wpctl")
                .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &arg])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
        }
        if which("amixer") {
            return Command::new("amixer")
                .args(["-q", "set", "Master", &arg, "unmute"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
        }
        false
    }

    /// Command form for a scroll-wheel step, using the same provider as read().
    pub fn step_command(delta: i32) -> Option<(String, Vec<String>)> {
        let arg = if delta > 0 {
            format!("{delta}%+")
        } else {
            format!("{}%-", -delta)
        };
        if which("wpctl") {
            return Some((
                "wpctl".into(),
                vec!["set-volume".into(), "@DEFAULT_AUDIO_SINK@".into(), arg],
            ));
        }
        if which("amixer") {
            return Some((
                "amixer".into(),
                vec![
                    "-q".into(),
                    "set".into(),
                    "Master".into(),
                    arg,
                    "unmute".into(),
                ],
            ));
        }
        None
    }

    /// Command form for output mute.
    pub fn mute_command() -> Option<(String, Vec<String>)> {
        if which("wpctl") {
            return Some((
                "wpctl".into(),
                vec![
                    "set-mute".into(),
                    "@DEFAULT_AUDIO_SINK@".into(),
                    "toggle".into(),
                ],
            ));
        }
        if which("amixer") {
            return Some((
                "amixer".into(),
                vec!["-q".into(), "set".into(), "Master".into(), "toggle".into()],
            ));
        }
        None
    }

    /// Command form for microphone mute. PipeWire exposes a source node; plain
    /// ALSA without wpctl has no portable equivalent, so the action is disabled.
    pub fn mic_mute_command() -> Option<(String, Vec<String>)> {
        which("niructl").then(|| ("niructl".into(), vec!["audio".into(), "mic-mute".into()]))
    }

    /// Whether the default microphone is muted, when PipeWire can report it.
    pub fn mic_muted() -> Option<bool> {
        which("wpctl")
            .then(|| run("wpctl", &["get-volume", "@DEFAULT_AUDIO_SOURCE@"]))
            .flatten()
            .map(|text| text.contains("[MUTED]"))
    }

    #[allow(dead_code)] // used by NIRUWM's media keys through handle_key
    pub fn toggle_mute() -> bool {
        if which("wpctl") {
            return Command::new("wpctl")
                .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
        }
        if which("amixer") {
            return Command::new("amixer")
                .args(["-q", "set", "Master", "toggle"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
        }
        false
    }
}

/// Glyphs for the volume level, with plain fallbacks.
#[derive(Clone, Debug)]
pub struct VolumeIcons {
    pub high: &'static str,
    pub medium: &'static str,
    pub low: &'static str,
    pub muted: &'static str,
}

impl VolumeIcons {
    pub fn nerd() -> Self {
        Self {
            high: "\u{f028}",   // nf-fa-volume_up
            medium: "\u{f027}", // volume_down
            low: "\u{f026}",    // volume_off
            muted: "\u{f026}",  // a crossed speaker reads better than a bar
        }
    }
    pub fn plain() -> Self {
        Self {
            high: "vol",
            medium: "vol",
            low: "vol",
            muted: "mute",
        }
    }
}

/// Format the volume segment: ` 45%` or ` mute`.
pub fn volume_text(v: &Volume, icons: &VolumeIcons) -> String {
    if v.muted {
        return icons.muted.to_string();
    }
    let icon = match v.percent {
        70.. => icons.high,
        30..=69 => icons.medium,
        _ => icons.low,
    };
    format!("{icon} {}%", v.percent)
}

// ── Media ───────────────────────────────────────────────────────────────────

/// What the player is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Media {
    pub artist: String,
    pub title: String,
    pub playing: bool,
    /// How far into the track, and how long it is, in seconds. `None` when the
    /// player does not report a length, which is common for streams: no bar is
    /// better than a bar that is always full or always empty.
    pub position: Option<(u64, u64)>,
}

impl Media {
    /// Ask the running player through MPRIS.
    ///
    /// `playerctl` is a tiny, widely packaged tool that speaks the standard
    /// MPRIS D-Bus interface. Going through D-Bus directly would mean a
    /// dependency on a D-Bus crate and reimplementing what it already does.
    /// Read a specific MPRIS player, or the active one when `player` is None.
    pub fn read_player(player: Option<&str>) -> Option<Self> {
        if !which("playerctl") {
            return None;
        }
        let playing = player_query(player, &["status"]).as_deref() == Some("Playing");
        let artist = player_query(player, &["metadata", "artist"]).unwrap_or_default();
        let title = player_query(player, &["metadata", "title"])?;
        let position = player_query(player, &["position"]).and_then(|p| {
            let pos: f64 = p.parse().ok()?;
            let len = player_query(player, &["metadata", "mpris:length"])
                .and_then(|l| l.parse::<f64>().ok())
                .map(|us| us / 1_000_000.0)?;
            (len > 1.0).then_some((pos.max(0.0) as u64, len as u64))
        });
        Some(Self {
            artist,
            title,
            playing,
            position,
        })
    }

    /// Names of every MPRIS player on the session bus.
    pub fn players() -> Vec<String> {
        run("playerctl", &["-l"])
            .map(|text| {
                text.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// One line, clipped to `width` characters: `artist - title`.
    pub fn line(&self, width: usize) -> String {
        let full = if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} - {}", self.artist, self.title)
        };
        // Count characters, not bytes: an accented title would otherwise be
        // cut mid-character.
        if full.chars().count() <= width {
            full
        } else {
            full.chars()
                .take(width.saturating_sub(1))
                .collect::<String>()
                + "…"
        }
    }

    pub fn toggle() {
        let _ = Command::new("playerctl").arg("play-pause").status();
    }

    #[allow(dead_code)] // used by NIRUWM's media keys through handle_key
    pub fn next() {
        let _ = Command::new("playerctl").arg("next").status();
    }

    #[allow(dead_code)] // used by NIRUWM's media keys through handle_key
    pub fn previous() {
        let _ = Command::new("playerctl").arg("previous").status();
    }
}

#[derive(Clone, Debug)]
pub struct MediaIcons {
    /// Text shown before the track when the font has no music mark: ">" and
    /// "||", which every font draws.
    pub playing: &'static str,
    pub paused: &'static str,
    /// The mark as shapes (block characters), or empty when the font has no
    /// blocks and the text above is used instead.
    pub playing_codes: &'static [char],
    /// Kept so a future paused mark (a dimmer set of bars) needs no change to
    /// callers; today both states draw the same shape.
    #[allow(dead_code)]
    pub paused_codes: &'static [char],
}

impl MediaIcons {
    /// Shapes for a font with block characters: three bars, the middle one
    /// lower, so the mark reads as sound rather than as a button. Its width
    /// never changes, whatever the font does with a glyph.
    pub fn blocks() -> Self {
        Self {
            playing: ">",
            paused: "||",
            playing_codes: &NOTE_MARK,
            paused_codes: &NOTE_MARK,
        }
    }
    /// Plain text, for a font without blocks.
    pub fn plain() -> Self {
        Self {
            playing: ">",
            paused: "||",
            playing_codes: &[],
            paused_codes: &[],
        }
    }
}

#[allow(dead_code)] // the module renders the scrolling form itself
pub fn media_text(m: &Media, icons: &MediaIcons, width: usize) -> String {
    let glyph = if m.playing {
        icons.playing
    } else {
        icons.paused
    };
    format!("{glyph} {}", m.line(width))
}

// ── Hardware keys ───────────────────────────────────────────────────────────

/// Handle a media key press. Returns true when the key was recognised.
///
/// Called by NIRUWM for the XF86 key names, so the keys work without a
/// separate daemon and without stealing them from other applications.
#[allow(dead_code)] // NIRUWM performs the same commands from its key table
pub fn handle_key(name: &str) -> bool {
    match name {
        "XF86AudioRaiseVolume" => Volume::step(5),
        "XF86AudioLowerVolume" => Volume::step(-5),
        "XF86AudioMute" => Volume::toggle_mute(),
        "XF86AudioPlay" | "XF86AudioPause" => {
            Media::toggle();
            true
        }
        "XF86AudioNext" => {
            Media::next();
            true
        }
        "XF86AudioPrev" => {
            Media::previous();
            true
        }
        _ => false,
    }
}

/// Build the icons the font can draw.
pub fn volume_icons(renderer_has: impl Fn(char) -> bool, want_nerd: bool) -> VolumeIcons {
    let nerd = VolumeIcons::nerd();
    let plain = VolumeIcons::plain();
    if !want_nerd {
        return plain;
    }
    let ok = |s: &str| s.chars().all(&renderer_has);
    VolumeIcons {
        high: if ok(nerd.high) { nerd.high } else { plain.high },
        medium: if ok(nerd.medium) {
            nerd.medium
        } else {
            plain.medium
        },
        low: if ok(nerd.low) { nerd.low } else { plain.low },
        muted: if ok(nerd.muted) {
            nerd.muted
        } else {
            plain.muted
        },
    }
}

pub fn media_icons(renderer_has: impl Fn(char) -> bool, want_nerd: bool) -> MediaIcons {
    let blocks = MediaIcons::blocks();
    if !want_nerd || !blocks.playing_codes.iter().all(|c| renderer_has(*c)) {
        return MediaIcons::plain();
    }
    blocks
}

/// The music mark, as block characters the bar draws as shapes.
const NOTE_MARK: [char; 3] = ['\u{2588}', '\u{2584}', '\u{2588}'];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_text_shows_a_percentage() {
        let icons = VolumeIcons::plain();
        let v = Volume {
            percent: 45,
            muted: false,
        };
        assert!(volume_text(&v, &icons).contains("45%"));
    }

    #[test]
    fn muted_volume_shows_only_the_mute_icon() {
        let icons = VolumeIcons::plain();
        let v = Volume {
            percent: 45,
            muted: true,
        };
        assert_eq!(volume_text(&v, &icons), "mute");
    }

    #[test]
    fn media_line_joins_artist_and_title() {
        let m = Media {
            artist: "A".into(),
            title: "B".into(),
            playing: true,
            position: None,
        };
        assert_eq!(m.line(20), "A - B");
    }

    #[test]
    fn media_line_is_clipped_to_width() {
        let m = Media {
            artist: "Artist".into(),
            title: "A very long title indeed".into(),
            playing: true,
            position: None,
        };
        let line = m.line(10);
        assert_eq!(line.chars().count(), 10);
        assert!(line.ends_with('…'));
    }

    #[test]
    fn media_line_handles_missing_artist() {
        let m = Media {
            artist: String::new(),
            title: "Only a title".into(),
            playing: false,
            position: None,
        };
        assert_eq!(m.line(30), "Only a title");
    }

    #[test]
    fn unknown_key_is_not_handled() {
        assert!(!handle_key("XF86Bogus"));
    }

    #[test]
    fn plain_icons_are_not_glyphs() {
        let v = VolumeIcons::plain();
        for icon in [v.high, v.medium, v.low, v.muted] {
            assert!(icon.is_ascii());
        }
    }

    #[test]
    fn icons_fall_back_per_glyph() {
        // A font that draws nothing: every icon comes from the plain set.
        let icons = volume_icons(|_| false, true);
        assert!(icons.high.is_ascii());
    }

    #[test]
    fn amixer_output_parses_real_world_lines() {
        assert_eq!(parse_amixer_percent("[45%] [on]"), Some(45));
        assert_eq!(parse_amixer_percent("[100%] [off]"), Some(100));
        assert_eq!(parse_amixer_percent("no level"), None);
    }

    #[test]
    fn player_commands_select_a_player_when_requested() {
        assert_eq!(
            player_command(None, "next"),
            ("playerctl".into(), vec!["next".into()])
        );
        assert_eq!(
            player_command(Some("mpv"), "previous"),
            (
                "playerctl".into(),
                vec!["-p".into(), "mpv".into(), "previous".into()]
            )
        );
    }

    #[test]
    fn status_is_not_required_for_the_bar_to_run() {
        // Reading volume and media must never panic or block the bar when the
        // tools are missing; they simply return nothing.
        let _ = Volume::read();
        let _ = Media::read_player(None);
    }
}
