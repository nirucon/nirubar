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

/// Run a command and return trimmed stdout, or `None`.
fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
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
                // "[45%] [on]" or "[45%] [off]"
                let percent = text
                    .split('[')
                    .find(|p| p.ends_with("%]"))
                    .map(|p| p.trim_end_matches("%]"))
                    .and_then(|p| p.parse::<u32>().ok())?;
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
}

impl Media {
    /// Ask the running player through MPRIS.
    ///
    /// `playerctl` is a tiny, widely packaged tool that speaks the standard
    /// MPRIS D-Bus interface. Going through D-Bus directly would mean a
    /// dependency on a D-Bus crate and reimplementing what it already does.
    pub fn read() -> Option<Self> {
        if !which("playerctl") {
            return None;
        }
        let playing = run("playerctl", &["status"]).as_deref() == Some("Playing");
        let artist = run("playerctl", &["metadata", "artist"]).unwrap_or_default();
        let title = run("playerctl", &["metadata", "title"])?;
        Some(Self {
            artist,
            title,
            playing,
        })
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
    pub playing: &'static str,
    pub paused: &'static str,
}

impl MediaIcons {
    pub fn nerd() -> Self {
        Self {
            playing: "\u{f04b}", // play
            paused: "\u{f04c}",  // pause
        }
    }
    pub fn plain() -> Self {
        Self {
            playing: ">",
            paused: "||",
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
    let nerd = MediaIcons::nerd();
    let plain = MediaIcons::plain();
    if !want_nerd {
        return plain;
    }
    let ok = |s: &str| s.chars().all(&renderer_has);
    MediaIcons {
        playing: if ok(nerd.playing) {
            nerd.playing
        } else {
            plain.playing
        },
        paused: if ok(nerd.paused) {
            nerd.paused
        } else {
            plain.paused
        },
    }
}

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
        };
        assert_eq!(m.line(20), "A - B");
    }

    #[test]
    fn media_line_is_clipped_to_width() {
        let m = Media {
            artist: "Artist".into(),
            title: "A very long title indeed".into(),
            playing: true,
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
    fn status_is_not_required_for_the_bar_to_run() {
        // Reading volume and media must never panic or block the bar when the
        // tools are missing; they simply return nothing.
        let _ = Volume::read();
        let _ = Media::read();
    }
}
