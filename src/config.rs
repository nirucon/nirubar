//! Configuration shared by NIRUWM and NIRUBAR.
//!
//! The file lives in `~/.config/niruwm/config.toml` (or `$XDG_CONFIG_HOME`).
//! Both programs read it; unknown keys are ignored so that installing NIRUBAR
//! before or after NIRUWM never rejects the other program's settings.
//!
//! The old 0.1 path `~/.config/niru/config.toml` is still read when the new
//! file does not exist, so an existing alpha configuration keeps working.
use serde::Deserialize;
use std::{error::Error, fs, path::PathBuf};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub theme: String,
    pub gap: u16,
    pub border_width: u16,
    pub bar_height: u16,
    pub master_ratio: f32,
    pub terminal: Vec<String>,
    /// NIRUWM-only: second terminal. Read-only here so the shared file is
    /// accepted unchanged; NIRUBAR never starts programs from it.
    pub terminal_alt: Vec<String>,
    /// NIRUWM-only: browser command.
    pub browser: Vec<String>,
    pub launcher: Vec<String>,
    pub font_path: Option<String>,
    pub font_size: f32,
    pub bar: Bar,
    /// NIRUWM-only key bindings. Parsed as an opaque value: the bar must not
    /// reject a configuration that mentions keys it does not use.
    pub keys: Vec<toml::Value>,
    /// NIRUWM-only wallpaper settings. Parsed as an opaque value for the same
    /// reason as `keys`: one shared file must be accepted by both programs.
    pub wallpaper: toml::Value,
    /// NIRUWM-only monitor settings. Opaque here for the same reason as keys:
    /// the bar must accept the shared file whatever the window manager stores.
    pub monitors: toml::Value,
    /// NIRUWM-only scratchpad settings.
    pub scratchpad: toml::Value,
    /// NIRUWM-only window rules.
    pub rules: Vec<toml::Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bar {
    /// `true` renders the bar as a dock window (NIRUWM only). Ignored by
    /// standalone use.
    pub enabled: bool,
    /// Show a short hint of the focused window's title between the tags and
    /// the clock. Both `show_title` and `title` are accepted in the file.
    #[serde(alias = "title")]
    pub show_title: bool,
    /// Maximum number of characters kept from the window title.
    pub title_max: usize,
    /// Width reserved per workspace cell, in pixels.
    pub cell_width: u16,
    /// Separator drawn between the status groups.
    pub separator: String,
    /// Modules to show, in order: workspaces, network, battery, clock.
    pub modules: Vec<String>,
    /// Workspace labels shown instead of 1..9.
    pub tags: Vec<String>,
    pub show_date: bool,
    pub show_time: bool,
    pub date_format: String,
    pub time_format: String,
    pub popups: bool,
    /// Seconds before an open panel closes itself. 0 keeps it until dismissed.
    pub popup_timeout_seconds: u32,
    /// Show a battery segment on laptops. Machines without a battery never
    /// show one, whatever this is set to.
    pub battery: bool,
    /// Show a network icon while a wired or wireless link is up.
    pub network: bool,
    /// Use Nerd Font glyphs. When the configured font lacks them, set this to
    /// false for plain ASCII labels.
    pub icons: bool,
    /// Characters kept from the media title before an ellipsis.
    pub media_width: usize,
    /// Minutes between package update checks. 0 asks every redraw, which is
    /// never a good idea; 30 is the default.
    pub update_check_minutes: u32,
    /// Whether the volume and media icons use glyphs. Follows `icons`; the
    /// bar narrows it per glyph to what the font can draw.
    #[serde(skip)]
    pub volume_icons: Option<crate::av::VolumeIcons>,
    #[serde(skip)]
    pub media_icons: Option<crate::av::MediaIcons>,
}

impl Bar {
    /// Volume icons resolved against the loaded font.
    pub fn volume_icons(&self) -> crate::av::VolumeIcons {
        self.volume_icons
            .clone()
            .unwrap_or_else(crate::av::VolumeIcons::plain)
    }

    pub fn media_icons(&self) -> crate::av::MediaIcons {
        self.media_icons
            .clone()
            .unwrap_or_else(crate::av::MediaIcons::plain)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "niru-noir".into(),
            gap: 8,
            border_width: 2,
            bar_height: 28,
            master_ratio: 0.55,
            terminal: vec!["kitty".into()],
            terminal_alt: Vec::new(),
            browser: vec!["brave".into()],
            launcher: vec!["rofi".into(), "-show".into(), "drun".into()],
            font_path: None,
            font_size: 15.0,
            bar: Bar::default(),
            keys: Vec::new(),
            wallpaper: toml::Value::Table(Default::default()),
            monitors: toml::Value::Table(Default::default()),
            scratchpad: toml::Value::Table(Default::default()),
            rules: Vec::new(),
        }
    }
}

impl Default for Bar {
    fn default() -> Self {
        Self {
            enabled: true,
            show_title: false,
            title_max: 60,
            cell_width: 30,
            separator: "|".into(),
            modules: vec![
                "workspaces".into(),
                "network".into(),
                "battery".into(),
                "clock".into(),
            ],
            tags: (1..=9).map(|n| n.to_string()).collect(),
            show_date: true,
            show_time: true,
            date_format: "%Y-%m-%d".into(),
            time_format: "%H:%M".into(),
            popups: true,
            popup_timeout_seconds: 4,
            battery: true,
            network: true,
            icons: true,
            media_width: 32,
            update_check_minutes: 30,
            volume_icons: None,
            media_icons: None,
        }
    }
}

/// The directory that holds `config.toml` and `themes/`.
pub fn directory() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
        .join("niruwm")
}

fn legacy_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
        .join("niru")
        .join("config.toml")
}

impl Config {
    pub fn load() -> Result<Self> {
        let primary = directory().join("config.toml");
        let text = match fs::read_to_string(&primary) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match fs::read_to_string(legacy_path()) {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(Self::default())
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            Err(e) => return Err(e.into()),
        };
        let c: Self = toml::from_str(&text)?;
        c.validate()?;
        Ok(c)
    }

    fn validate(&self) -> Result<()> {
        if !(8.0..=40.0).contains(&self.font_size)
            || !(0.1..=0.9).contains(&self.master_ratio)
            || self.gap > 100
            || self.border_width > 20
            || !(18..=100).contains(&self.bar_height)
            || self.terminal.is_empty()
            || self.launcher.is_empty()
        {
            return Err("Invalid geometry or empty command".into());
        }
        if self.bar.cell_width < 12 || self.bar.cell_width > 200 {
            return Err("bar.cell_width must be between 12 and 200".into());
        }
        if self.bar.title_max > 512 {
            return Err("bar.title_max must be at most 512".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub name: String,
    pub background: String,
    pub foreground: String,
    pub muted: String,
    pub border: String,
    pub focus: String,
}

impl Theme {
    pub fn load(name: &str) -> Result<Self> {
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("Invalid theme name".into());
        }
        // Built-in themes are compiled in; a file of the same name in
        // ~/.config/niruwm/themes takes precedence so a user can tweak one.
        let user_file = directory().join("themes").join(format!("{name}.toml"));
        let text = match (name, user_file.exists()) {
            ("niru-noir", false) => include_str!("../themes/niru-noir.toml").to_string(),
            ("niru-nord", false) => include_str!("../themes/niru-nord.toml").to_string(),
            ("niru-satie", false) => include_str!("../themes/niru-satie.toml").to_string(),
            ("niru-carl-larsson", false) => {
                include_str!("../themes/niru-carl-larsson.toml").to_string()
            }
            _ => fs::read_to_string(&user_file)?,
        };
        let theme: Self = toml::from_str(&text)?;
        for s in [
            &theme.background,
            &theme.foreground,
            &theme.muted,
            &theme.border,
            &theme.focus,
        ] {
            Self::pixel(s)?;
        }
        Ok(theme)
    }

    pub fn pixel(s: &str) -> Result<u32> {
        if s.len() != 6 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Theme colours must be six hex digits".into());
        }
        Ok(u32::from_str_radix(s, 16)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_minimal() {
        let c = Config::default();
        assert!(c.bar.enabled);
        assert!(!c.bar.show_title, "the bar defaults to tags and clock only");
        assert_eq!(c.bar.cell_width, 30);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        // deny_unknown_fields keeps a typo from silently doing nothing, but the
        // file is shared, so both programs must describe every key they use.
        let err = toml::from_str::<Config>("gap = 8\nbogus = 1");
        assert!(err.is_err());
    }

    #[test]
    fn accepts_the_wm_config_unchanged() {
        // The two programs share one file. If the bar rejects a key the WM
        // writes, the bar dies at login — so every WM key must parse here.
        let shared = r#"
theme = "niru-noir"
terminal = ["kitty"]
terminal_alt = ["alacritty"]
browser = ["brave"]

[bar]
title = true

[[keys]]
keys = "super+Return"
action = "terminal"
"#;
        let c: Config = toml::from_str(shared).expect("shared config must parse");
        assert!(c.bar.show_title);
        assert_eq!(c.terminal_alt, vec!["alacritty"]);
        assert_eq!(c.keys.len(), 1);
    }

    #[test]
    fn accepts_the_shipped_example_file() {
        // The strongest guard against the two programs drifting apart: parse
        // the file NIRUWM ships. This broke three times before the test existed.
        let example = include_str!("../config.example.toml");
        toml::from_str::<Config>(example).expect("config.example.toml must parse in NIRUBAR");
    }

    #[test]
    fn accepts_every_wm_key() {
        // NIRUWM writes this file. If the bar rejects one of its keys the bar
        // dies at login, which happened when the WM gained new settings.
        let shared = r#"
theme = "niru-noir"
gap = 8
border_width = 2
bar_height = 28
master_ratio = 0.55
terminal = ["kitty"]
terminal_alt = []
browser = ["brave"]
launcher = ["rofi", "-show", "drun"]
font_size = 15.0

[wallpaper]
enabled = true
dir = "~/Pictures/Wallpapers"
mode = "random"
file = ""
rotation_minutes = 10

[bar]
title = true

[[keys]]
keys = "super+Return"
action = "terminal"
"#;
        let c: Config = toml::from_str(shared).expect("every WM key must parse");
        assert!(c.bar.show_title);
    }

    #[test]
    fn partial_config_falls_back_to_defaults() {
        let c: Config = toml::from_str("[bar]\ntitle = true\n").unwrap();
        assert!(c.bar.show_title);
        assert_eq!(c.bar.cell_width, 30);
        assert_eq!(c.gap, 8);
    }

    #[test]
    fn all_builtin_themes_load() {
        for name in ["niru-noir", "niru-nord", "niru-satie", "niru-carl-larsson"] {
            Theme::load(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    #[test]
    fn reject_bad_theme_name() {
        assert!(Theme::load("../bad").is_err());
    }

    #[test]
    fn reject_bad_cell_width() {
        let mut c = Config::default();
        c.bar.cell_width = 4;
        assert!(c.validate().is_err());
    }
}
