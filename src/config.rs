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
    /// NIRUWM/niructl night-light settings; accepted but not used by the bar.
    pub nightlight: toml::Value,
    /// NIRUWM-only scratchpad settings.
    pub scratchpad: toml::Value,
    /// NIRUWM-only window rules.
    pub rules: Vec<toml::Value>,
    /// The `sun` module's location: degrees, north and east positive. Set both
    /// to 0 to hide the times and explain it in the popup instead.
    ///
    /// Read only by NIRUWM, which saves and returns the windows of a session.
    /// Accepted here because the bar reads the same document and refuses
    /// fields it does not know: this one stopped every bar from starting on
    /// 18 October, one release after the same mistake with weather_latitude.
    pub restore_session: bool,
    /// NIRUWM's opt-in NIRUPI shortcut and helper profile. The bar reads the
    /// same document and must accept the key even though it does not use it.
    pub nirupi_compat: bool,
    /// Read only by NIRUBAR, but it lives at the top level of the file so that
    /// NIRUWM accepts the same document. A setting only one of the two
    /// programs knows is a config the other one refuses to start with, and a
    /// window manager that will not start is a session that will not start.
    pub weather_latitude: f64,
    pub weather_longitude: f64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bar {
    /// Where the machine is, for the `sun` module: the sunrise and sunset times.
    /// Degrees, north and east positive. Set both to 0 to hide the times and
    /// explain it in the popup instead.
    ///
    /// Read only by NIRUBAR, but it lives at the top level of the file so NIRUWM
    /// accepts the same document. A setting that only one of the two programs
    /// knows is a config the other one refuses to start with, and a window
    /// manager that will not start is a session that will not start.
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
    /// Minutes the focused window must stay the same before its title is
    /// shown. 0 shows it at once. A title that appears while you are still
    /// switching windows is noise; a short delay makes it a reward.
    #[serde(default)]
    pub title_delay_seconds: u32,
    /// Width of the graph in the `pulse` popup, in characters.
    #[serde(default)]
    pub graph_width: usize,
    /// Own a system tray. The bar becomes the tray for the session, so
    /// applications that show an icon are visible in it.
    pub tray: bool,
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            weather_latitude: 0.0,
            weather_longitude: 0.0,
            restore_session: false,
            nirupi_compat: false,
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
            nightlight: toml::Value::Table(Default::default()),
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
                "media".into(),
                "volume".into(),
                "brightness".into(),
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
            title_delay_seconds: 0,
            graph_width: 24,
            tray: true,
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
        use std::fmt::Write as _;

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
        for (name, format) in [
            ("date_format", &self.bar.date_format),
            ("time_format", &self.bar.time_format),
        ] {
            // Token parsing alone accepts parse-only directives such as %#z.
            // Exercise formatting through a fallible sink: to_string() would
            // panic on fmt::Error instead of returning a config diagnostic.
            if format.is_empty()
                || write!(
                    &mut String::new(),
                    "{}",
                    chrono::Local::now().format(format)
                )
                .is_err()
            {
                return Err(format!("bar.{name} must be a valid nonempty strftime format").into());
            }
        }
        if self.bar.tags.len() > 9 {
            return Err("bar.tags has more than nine entries".into());
        }
        for module in &self.bar.modules {
            // NIRUWM owns the shaped-name rule; the bar owns which names exist.
            // A future module must still be accepted by both programs.
            if module.is_empty() || !module.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                return Err(format!("unknown bar module {module:?}").into());
            }
        }

        let wallpaper: WallpaperValidation = self.wallpaper.clone().try_into()?;
        if !["random", "fixed", "single"].contains(&wallpaper.mode.as_str()) {
            return Err("wallpaper.mode must be random, fixed or single".into());
        }
        if !["fill", "fit", "stretch", "center"].contains(&wallpaper.scale.as_str()) {
            return Err("wallpaper.scale must be fill, fit, stretch or center".into());
        }
        if wallpaper.rotation_minutes > 100_000 {
            return Err("wallpaper.rotation_minutes is too large".into());
        }

        let monitors: MonitorsValidation = self.monitors.clone().try_into()?;
        if monitors.workspaces == 0 || monitors.workspaces > 32 {
            return Err("monitors.workspaces must be between 1 and 32".into());
        }
        for profile in &monitors.profiles {
            if profile.name.is_empty() || profile.workspaces.is_empty() {
                return Err("monitors.profiles entries need a name and workspaces".into());
            }
        }

        let nightlight: NightlightValidation = self.nightlight.clone().try_into()?;
        if !(-90.0..=90.0).contains(&nightlight.latitude)
            || !(-180.0..=180.0).contains(&nightlight.longitude)
            || !(1000..=10_000).contains(&nightlight.temperature)
        {
            return Err("nightlight latitude/longitude/temperature out of range".into());
        }

        let scratchpad: ScratchpadValidation = self.scratchpad.clone().try_into()?;
        if !(0.1..=0.95).contains(&scratchpad.width) || !(0.1..=0.95).contains(&scratchpad.height) {
            return Err("scratchpad.width and height must be between 0.1 and 0.95".into());
        }
        if scratchpad.command.is_empty() {
            return Err("scratchpad.command must not be empty".into());
        }
        let names_a_class = scratchpad.command.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "--class" | "-class" | "--app-id" | "-name" | "--name"
            )
        });
        if !names_a_class {
            return Err(
                "scratchpad.command needs a window class, for example [\"kitty\", \"--class\", \"scratchpad\"]"
                    .into(),
            );
        }

        for value in &self.rules {
            let rule: RuleValidation = value.clone().try_into()?;
            if rule.class.is_empty() {
                return Err("rules entries need a class".into());
            }
        }
        Ok(())
    }
}

/// Validation mirrors of NIRUWM's typed shared sections. NIRUBAR stores these
/// sections as opaque TOML so it accepts settings it does not use, but it must
/// reject the same invalid values as the window manager. Keep this in sync with
/// `niruwm/src/config.rs`; the shared config fixture suite checks the contract.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct WallpaperValidation {
    enabled: bool,
    dir: String,
    mode: String,
    file: String,
    rotation_minutes: u32,
    scale: String,
    files: Vec<String>,
    shuffle_on_start: bool,
}

impl Default for WallpaperValidation {
    fn default() -> Self {
        Self {
            enabled: false,
            dir: "~/Pictures/Wallpapers".into(),
            mode: "random".into(),
            file: String::new(),
            rotation_minutes: 10,
            scale: "fill".into(),
            files: Vec::new(),
            shuffle_on_start: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct MonitorsValidation {
    workspaces: usize,
    profiles: Vec<MonitorProfileValidation>,
}

impl Default for MonitorsValidation {
    fn default() -> Self {
        Self {
            workspaces: 9,
            profiles: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // fields enforce the shared TOML schema
struct MonitorProfileValidation {
    name: String,
    workspaces: Vec<String>,
    primary: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct NightlightValidation {
    latitude: f64,
    longitude: f64,
    temperature: u32,
}

impl Default for NightlightValidation {
    fn default() -> Self {
        Self {
            latitude: 0.0,
            longitude: 0.0,
            temperature: 3400,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ScratchpadValidation {
    command: Vec<String>,
    width: f32,
    height: f32,
}

impl Default for ScratchpadValidation {
    fn default() -> Self {
        Self {
            command: vec!["kitty".into(), "--class".into(), "scratchpad".into()],
            width: 0.6,
            height: 0.6,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // fields enforce the shared TOML schema
struct RuleValidation {
    class: String,
    workspace: Option<u32>,
    floating: Option<bool>,
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
    fn tray_is_on_by_default() {
        let c = Config::default();
        assert!(c.bar.tray, "a tray nobody has to enable is the useful one");
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

    #[test]
    fn shared_invalid_fixtures_are_rejected_like_niruwm() {
        for (name, text) in [
            ("wallpaper mode", "[wallpaper]\nmode = \"bogus\"\n"),
            ("wallpaper scale", "[wallpaper]\nscale = \"bogus\"\n"),
            (
                "wallpaper rotation",
                "[wallpaper]\nrotation_minutes = 100001\n",
            ),
            ("monitor workspaces", "[monitors]\nworkspaces = 0\n"),
            (
                "monitor profile",
                "[[monitors.profiles]]\nname = \"eDP-1\"\nworkspaces = []\nprimary = true\n",
            ),
            ("nightlight latitude", "[nightlight]\nlatitude = 91.0\n"),
            (
                "nightlight temperature",
                "[nightlight]\ntemperature = 999\n",
            ),
            ("scratchpad width", "[scratchpad]\nwidth = 0.0\n"),
            (
                "scratchpad class",
                "[scratchpad]\ncommand = [\"kitty\"]\n",
            ),
            ("rule class", "[[rules]]\nworkspace = 1\n"),
            (
                "bar tags",
                "[bar]\ntags = [\"1\", \"2\", \"3\", \"4\", \"5\", \"6\", \"7\", \"8\", \"9\", \"10\"]\n",
            ),
            ("empty module", "[bar]\nmodules = [\"\"]\n"),
            ("module shape", "[bar]\nmodules = [\"UP\"]\n"),
            ("clock format", "[bar]\ndate_format = \"%Q\"\n"),
        ] {
            let rejected = match toml::from_str::<Config>(text) {
                Ok(c) => c.validate().is_err(),
                Err(_) => true,
            };
            assert!(rejected, "accepted invalid shared fixture: {name}");
        }
    }

    #[test]
    fn future_module_names_are_accepted() {
        let c: Config = toml::from_str("[bar]\nmodules = [\"future_module\"]\n").unwrap();
        assert!(c.validate().is_ok());
    }

    #[test]
    fn invalid_clock_formats_are_rejected_before_rendering() {
        for bad in ["%", "%Q", "%#", "%#z", ""] {
            let mut c = Config::default();
            c.bar.time_format = bad.into();
            assert!(c.validate().is_err(), "accepted time format {bad:?}");
            c.bar.time_format = "%H:%M".into();
            c.bar.date_format = bad.into();
            assert!(c.validate().is_err(), "accepted date format {bad:?}");
        }
        let mut c = Config::default();
        c.bar.time_format = "%H:%M %% %:z".into();
        assert!(c.validate().is_ok());
    }
}
