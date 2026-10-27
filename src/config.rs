//! Configuration shared by NIRUWM and NIRUBAR.
//!
//! The file lives in `~/.config/niruwm/config.toml` (or `$XDG_CONFIG_HOME`).
//! Both programs read it; unknown keys are ignored so that installing NIRUBAR
//! before or after NIRUWM never rejects the other program's settings.
//!
//! The old 0.1 path `~/.config/niru/config.toml` is still read when the new
//! file does not exist, so an existing alpha configuration keeps working.
use serde::{Deserialize, Serialize};
use std::{error::Error, fs, path::PathBuf};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn parse_clock(s: &str) -> Option<(u32, u32)> {
    let (hour, minute) = s.trim().split_once(':')?;
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    (hour < 24 && minute < 60).then_some((hour, minute))
}

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
    /// NIRUWM-only login autostart entries; accepted opaquely here.
    pub autostart: toml::Value,
    /// NIRUWM/niructl launcher recipes; accepted opaquely here.
    pub recipes: toml::Value,
    /// NIRUWM-only monitor settings. Opaque here for the same reason as keys:
    /// the bar must accept the shared file whatever the window manager stores.
    pub monitors: toml::Value,
    /// NIRUWM/niructl night-light settings; accepted but not used by the bar.
    pub nightlight: toml::Value,
    /// NIRUWM/niructl brightness device and step; accepted but not used here.
    pub brightness: toml::Value,
    /// NIRUIDLE screensaver/lock timers; accepted but not used by the bar.
    pub idle: toml::Value,
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
    /// Read only by NIRUBAR, but it lives at the top level of the file so that
    /// NIRUWM accepts the same document. A setting only one of the two
    /// programs knows is a config the other one refuses to start with, and a
    /// window manager that will not start is a session that will not start.
    pub weather_latitude: f64,
    pub weather_longitude: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
    /// Draw the bar see-through instead of opaque. The bar has no alpha of its
    /// own on X11: this sets `_NET_WM_WINDOW_OPACITY`, which the compositor
    /// (picom in NIRUX) applies. Without a compositor the bar stays opaque,
    /// which is also what `false` gives.
    pub transparent: bool,
    /// Window opacity used when `transparent` is true, 0-100. Out-of-range
    /// values fall back to `DEFAULT_BAR_OPACITY`.
    pub opacity: u32,
    /// Separator drawn between the status groups.
    pub separator: String,
    /// Modules to show, in order: workspaces, network, battery, clock.
    pub modules: Vec<String>,
    /// Always show input state, or hide healthy unmuted input until muted.
    pub microphone_visibility: String,
    /// Workspace labels shown instead of 1..9.
    pub tags: Vec<String>,
    /// Preset for the workspace labels: 1 numbers, 2 icons, 3 text. Wins over
    /// `tags` when it names a known preset; absent or unknown keeps `tags`.
    pub workspace_layout: Option<u32>,
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
    /// Show a dedicated Wi-Fi segment when an adapter is present. The segment
    /// opens the Wi-Fi selector, including while the radio is off.
    pub wifi: bool,
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
            theme: "niru-noir".into(),
            gap: 8,
            border_width: 2,
            bar_height: 28,
            master_ratio: 0.55,
            terminal: vec!["kitty".into()],
            terminal_alt: Vec::new(),
            browser: vec!["brave".into()],
            launcher: vec!["nirulauncher".into(), "--apps".into()],
            font_path: None,
            font_size: 15.0,
            bar: Bar::default(),
            keys: Vec::new(),
            wallpaper: toml::Value::Table(Default::default()),
            autostart: toml::Value::Table(Default::default()),
            recipes: toml::Value::Array(Default::default()),
            monitors: toml::Value::Table(Default::default()),
            nightlight: toml::Value::Table(Default::default()),
            brightness: toml::Value::Table(Default::default()),
            idle: toml::Value::Table(Default::default()),
            scratchpad: toml::Value::Table(Default::default()),
            rules: Vec::new(),
        }
    }
}

/// Workspace label presets selected by `[bar] workspace_layout`.
pub const WORKSPACE_LAYOUTS: u32 = 3;

/// Opacity used when the bar is transparent but `opacity` is out of range.
/// One definition for the default, the validator and the fallback.
pub const DEFAULT_BAR_OPACITY: u32 = 94;

/// Labels for a preset. `1` numbers, `2` icons, `3` text. Must match NIRUWM's
/// table: both programs render the same shared configuration.
pub fn workspace_layout_tags(layout: u32) -> Vec<String> {
    match layout {
        2 => [
            "\u{f0ac}", "\u{f121}", "\u{f086}", "\u{e795}", "\u{f075}", "\u{f0e0}", "\u{f07b}",
            "\u{f086}", "\u{f0e0}", "\u{f07b}",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
        3 => [
            "web", "ai", "ai", "ai", "term", "chat", "mail", "folder", "pchat", "term",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
        _ => (1..=10).map(|n| n.to_string()).collect(),
    }
}

impl Bar {
    /// Labels to render. `workspace_layout` wins when it names a known preset;
    /// otherwise the explicit `tags` list is used, which defaults to numbers.
    pub fn effective_tags(&self) -> Vec<String> {
        match self.workspace_layout {
            Some(n) if (1..=WORKSPACE_LAYOUTS).contains(&n) => workspace_layout_tags(n),
            _ => self.tags.clone(),
        }
    }

    /// Opacity to publish, or `None` when the bar must stay opaque.
    /// Out-of-range `opacity` falls back to `DEFAULT_BAR_OPACITY` rather than
    /// making the bar vanish or fail to start.
    pub fn effective_opacity(&self) -> Option<u32> {
        if !self.transparent {
            return None;
        }
        let percent = if (1..=100).contains(&self.opacity) {
            self.opacity
        } else {
            DEFAULT_BAR_OPACITY
        };
        // _NET_WM_WINDOW_OPACITY is a 32-bit fraction of 0xffffffff. Multiply in
        // u64 so a large percentage cannot overflow.
        let fraction = u64::from(percent) * u64::from(u32::MAX) / 100;
        Some(fraction as u32)
    }
}

impl Default for Bar {
    fn default() -> Self {
        Self {
            enabled: true,
            show_title: false,
            title_max: 60,
            cell_width: 30,
            transparent: true,
            opacity: DEFAULT_BAR_OPACITY,
            separator: "|".into(),
            modules: vec![
                "workspaces".into(),
                "media".into(),
                "volume".into(),
                "brightness".into(),
                "display".into(),
                "network".into(),
                "tasks".into(),
                "battery".into(),
                "clock".into(),
            ],
            tags: (1..=10).map(|n| n.to_string()).collect(),
            workspace_layout: None,
            microphone_visibility: "always".into(),
            show_date: true,
            show_time: true,
            date_format: "%Y-%m-%d".into(),
            time_format: "%H:%M".into(),
            popups: true,
            popup_timeout_seconds: 4,
            battery: true,
            network: true,
            wifi: true,
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

pub fn active_path() -> PathBuf {
    let primary = directory().join("config.toml");
    if primary.exists() {
        primary
    } else {
        let legacy = legacy_path();
        if legacy.exists() {
            legacy
        } else {
            primary
        }
    }
}

/// Replace only the shared `[bar]` table with current compiled defaults.
/// Returns the backup path when an existing file was preserved.
pub fn reset_bar() -> Result<(PathBuf, Option<PathBuf>)> {
    reset_bar_at(&active_path())
}

fn reset_bar_at(path: &std::path::Path) -> Result<(PathBuf, Option<PathBuf>)> {
    let path = path.to_path_buf();
    let original = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let mut document: toml_edit::DocumentMut = original
        .as_deref()
        .unwrap_or("")
        .parse()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let defaults = toml::to_string(&Bar::default())?;
    let fragment: toml_edit::DocumentMut = format!("[bar]\n{defaults}")
        .parse()
        .map_err(|e| format!("internal bar defaults: {e}"))?;
    document["bar"] = fragment["bar"].clone();
    let parent = path.parent().ok_or("configuration directory is missing")?;
    fs::create_dir_all(parent)?;
    let backup = if original.is_some() {
        let backup = PathBuf::from(format!("{}.nirubar-backup", path.display()));
        fs::write(&backup, original.as_deref().unwrap_or(""))?;
        Some(backup)
    } else {
        None
    };
    let temp = parent.join(format!(".config.toml.{}", std::process::id()));
    fs::write(&temp, document.to_string())?;
    fs::rename(&temp, &path)?;
    Ok((path, backup))
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
                        let value = toml::Value::Table(Default::default());
                        let mut c: Self = value.clone().try_into()?;
                        apply_appearance_defaults(&mut c, &value);
                        c.validate()?;
                        return Ok(c);
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            Err(e) => return Err(e.into()),
        };
        let value: toml::Value = text.parse()?;
        let mut c: Self = value.clone().try_into()?;
        apply_appearance_defaults(&mut c, &value);
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
        if self.bar.tags.len() > 10 {
            return Err("bar.tags has more than ten entries".into());
        }
        for module in &self.bar.modules {
            // NIRUWM owns the shaped-name rule; the bar owns which names exist.
            // A future module must still be accepted by both programs.
            if module.is_empty() || !module.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                return Err(format!("unknown bar module {module:?}").into());
            }
        }
        if !matches!(self.bar.microphone_visibility.as_str(), "always" | "muted") {
            return Err("bar.microphone_visibility must be always or muted".into());
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
        if wallpaper.follow_theme && wallpaper.theme_dir.trim().is_empty() {
            return Err("wallpaper.theme_dir must not be empty when follow_theme is true".into());
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
        // The same names NIRUWM accepts; a typo is refused here too, so the two
        // programs can never disagree about a file they share.
        let known = |name: &String| {
            matches!(
                name.as_str(),
                "tile"
                    | "left"
                    | "monocle"
                    | "centered"
                    | "centeredfloat"
                    | "floating"
                    | "fibonacci"
            )
        };
        if let Some(bad) = monitors.layouts.iter().find(|l| !known(l)) {
            return Err(format!("monitors.layouts: unknown layout {bad}").into());
        }
        for profile in &monitors.profiles {
            if let Some(bad) = profile.layouts.iter().find(|l| !known(l)) {
                return Err(
                    format!("monitors.profiles {}: unknown layout {bad}", profile.name).into(),
                );
            }
        }

        let nightlight: NightlightValidation = self.nightlight.clone().try_into()?;
        if !(-90.0..=90.0).contains(&nightlight.latitude)
            || !(-180.0..=180.0).contains(&nightlight.longitude)
            || !(1000..=10_000).contains(&nightlight.temperature)
            || !(1000..=10_000).contains(&nightlight.day_temperature)
        {
            return Err("nightlight latitude/longitude/temperature out of range".into());
        }
        if !matches!(nightlight.mode.as_str(), "manual" | "fixed" | "solar") {
            return Err("nightlight.mode must be manual, fixed or solar".into());
        }
        if parse_clock(&nightlight.on).is_none() || parse_clock(&nightlight.off).is_none() {
            return Err("nightlight.on/off must be HH:MM".into());
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

/// Read the versioned appearance contract. Invalid files are ignored so a bad
/// edit never replaces the currently working theme.
fn appearance_value() -> Option<toml::Value> {
    let path = directory().join("appearance.toml");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!("nirubar: {}: {e}", path.display());
            return None;
        }
    };
    let value: toml::Value = match text.parse() {
        Ok(value) => value,
        Err(e) => {
            eprintln!("nirubar: {}: {e}", path.display());
            return None;
        }
    };
    if value.get("schema").and_then(|v| v.as_integer()) != Some(1) {
        eprintln!("nirubar: {}: unsupported appearance schema", path.display());
        return None;
    }
    if let Some(colors) = value.get("colors") {
        let Some(colors) = colors.as_table() else {
            eprintln!("nirubar: {}: colors must be a table", path.display());
            return None;
        };
        for colour in colors.values() {
            let Some(text) = colour.as_str() else {
                eprintln!("nirubar: {}: colours must be strings", path.display());
                return None;
            };
            if Theme::pixel(text).is_err() {
                eprintln!("nirubar: {}: invalid colour {text:?}", path.display());
                return None;
            }
        }
    }
    if let Some(size) = value.get("font").and_then(|font| font.get("size")) {
        if !size
            .as_float()
            .is_some_and(|size| size.is_finite() && size > 0.0)
        {
            eprintln!("nirubar: {}: font.size must be positive", path.display());
            return None;
        }
    }
    if let Some(motion) = value.get("motion").and_then(|motion| motion.get("reduced")) {
        if motion.as_bool().is_none() {
            eprintln!(
                "nirubar: {}: motion.reduced must be boolean",
                path.display()
            );
            return None;
        }
    }
    Some(value)
}

fn appearance_colour(value: &toml::Value, key: &str) -> Option<String> {
    value.get("colors")?.get(key)?.as_str().map(str::to_string)
}

fn appearance_font_size(value: &toml::Value) -> Option<f32> {
    value.get("font")?.get("size")?.as_float().map(|v| v as f32)
}

fn appearance_font_path(value: &toml::Value) -> Option<String> {
    value.get("font")?.get("path")?.as_str().map(str::to_string)
}

fn appearance_spacing(value: &toml::Value, key: &str) -> Option<u16> {
    value
        .get("spacing")?
        .get(key)?
        .as_integer()
        .and_then(|n| u16::try_from(n).ok())
}

fn apply_appearance_defaults(cfg: &mut Config, file: &toml::Value) {
    let Some(appearance) = appearance_value() else {
        return;
    };
    if file.get("font_size").is_none() {
        if let Some(size) = appearance_font_size(&appearance) {
            cfg.font_size = size;
        }
    }
    if file.get("font_path").is_none() {
        if let Some(path) = appearance_font_path(&appearance) {
            cfg.font_path = Some(path);
        }
    }
    if file.get("gap").is_none() {
        if let Some(gap) = appearance_spacing(&appearance, "gap") {
            cfg.gap = gap;
        }
    }
    if file.get("border_width").is_none() {
        if let Some(width) = appearance
            .get("focus")
            .and_then(|focus| focus.get("width"))
            .and_then(|width| width.as_integer())
            .and_then(|width| u16::try_from(width).ok())
            .or_else(|| appearance_spacing(&appearance, "border_width"))
        {
            cfg.border_width = width;
        }
    }
    if file.get("bar_height").is_none() {
        if let Some(height) = appearance_spacing(&appearance, "bar_height") {
            cfg.bar_height = height;
        }
    }
}

/// Whether non-essential movement such as the scrolling media title is off.
pub fn appearance_reduced_motion() -> bool {
    appearance_value()
        .and_then(|value| value.get("motion")?.get("reduced")?.as_bool())
        .unwrap_or(false)
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
    follow_theme: bool,
    theme_dir: String,
    /// Per-screen pictures; NIRUWM validates them, the bar only has to accept
    /// the shared file unchanged.
    outputs: Vec<toml::Value>,
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
            follow_theme: true,
            theme_dir: "~/Pictures/Wallpapers/NIRU".into(),
            outputs: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct MonitorsValidation {
    workspaces: usize,
    profiles: Vec<MonitorProfileValidation>,
    /// Workspace layouts NIRUWM keeps. The bar shares this file and must
    /// accept what the window manager writes after a workspace change.
    #[serde(default)]
    layouts: Vec<String>,
}

impl Default for MonitorsValidation {
    fn default() -> Self {
        Self {
            workspaces: 10,
            profiles: Vec::new(),
            layouts: Vec::new(),
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
    /// This screen's own workspace layouts, written by NIRUWM.
    #[serde(default)]
    layouts: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct NightlightValidation {
    latitude: f64,
    longitude: f64,
    temperature: u32,
    mode: String,
    on: String,
    off: String,
    day_temperature: u32,
}

impl Default for NightlightValidation {
    fn default() -> Self {
        Self {
            latitude: 0.0,
            longitude: 0.0,
            temperature: 3400,
            mode: "manual".into(),
            on: "20:00".into(),
            off: "07:00".into(),
            day_temperature: 6500,
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
    center: Option<bool>,
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
            ("niru-forest-vigil", false) => {
                include_str!("../themes/niru-forest-vigil.toml").to_string()
            }
            ("niru-blood-of-urdr", false) => {
                include_str!("../themes/niru-blood-of-urdr.toml").to_string()
            }
            ("niru-root-and-stone", false) => {
                include_str!("../themes/niru-root-and-stone.toml").to_string()
            }
            ("niru-blue-horizon", false) => {
                include_str!("../themes/niru-blue-horizon.toml").to_string()
            }
            ("niru-the-norns", false) => include_str!("../themes/niru-the-norns.toml").to_string(),
            ("niru-winter-nights", false) => {
                include_str!("../themes/niru-winter-nights.toml").to_string()
            }
            _ => fs::read_to_string(&user_file)?,
        };
        let mut theme: Self = toml::from_str(&text)?;
        if !user_file.exists() {
            if let Some(appearance) = appearance_value() {
                for (field, key) in [
                    (&mut theme.background, "background"),
                    (&mut theme.foreground, "foreground"),
                    (&mut theme.muted, "muted"),
                    (&mut theme.border, "border"),
                    (&mut theme.focus, "focus"),
                ] {
                    if let Some(colour) = appearance_colour(&appearance, key) {
                        *field = colour;
                    }
                }
            }
        }
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
    fn workspace_layout_selects_a_preset_and_falls_back_safely() {
        let mut bar = Bar::default();
        assert_eq!(bar.effective_tags()[0], "1", "default layout is numbers");

        bar.workspace_layout = Some(2);
        let icons = bar.effective_tags();
        assert_eq!(icons.len(), 10);
        assert_ne!(icons[0], "1", "layout 2 is not numeric");

        bar.workspace_layout = Some(3);
        assert_eq!(bar.effective_tags()[0], "web");
        assert_eq!(bar.effective_tags()[9], "term");

        // An unknown preset keeps the explicit tags rather than panicking.
        bar.workspace_layout = Some(99);
        assert_eq!(bar.effective_tags()[0], "1");
        bar.workspace_layout = None;
        assert_eq!(bar.effective_tags()[0], "1");
    }

    #[test]
    fn explicit_tags_still_win_when_no_layout_is_set() {
        let bar = Bar {
            tags: vec!["a".into(), "b".into()],
            workspace_layout: None,
            ..Default::default()
        };
        assert_eq!(bar.effective_tags(), vec!["a", "b"]);
    }

    #[test]
    fn bar_opacity_is_opt_in_and_clamped() {
        let mut bar = Bar::default();
        assert!(bar.transparent, "transparent appearance is the default");
        assert_eq!(
            bar.effective_opacity(),
            Some(u64::from(DEFAULT_BAR_OPACITY) * u64::from(u32::MAX) / 100).map(|v| v as u32)
        );

        bar.transparent = false;
        assert_eq!(bar.effective_opacity(), None, "opaque when disabled");

        // Out-of-range values fall back instead of making the bar invisible.
        bar.transparent = true;
        bar.opacity = 0;
        assert_eq!(
            bar.effective_opacity(),
            Some(u64::from(DEFAULT_BAR_OPACITY) * u64::from(u32::MAX) / 100).map(|v| v as u32)
        );
        bar.opacity = 250;
        assert_eq!(
            bar.effective_opacity(),
            Some(u64::from(DEFAULT_BAR_OPACITY) * u64::from(u32::MAX) / 100).map(|v| v as u32)
        );
        bar.opacity = 80;
        assert_eq!(
            bar.effective_opacity(),
            Some(u64::from(80u32) * u64::from(u32::MAX) / 100).map(|v| v as u32)
        );
    }

    #[test]
    fn new_bar_keys_parse_from_toml() {
        let c: Config =
            toml::from_str("[bar]\nworkspace_layout = 3\ntransparent = false\nopacity = 80\n")
                .expect("new keys must parse");
        assert_eq!(c.bar.workspace_layout, Some(3));
        assert!(!c.bar.transparent);
        assert_eq!(c.bar.opacity, 80);
    }

    #[test]
    fn an_absurd_opacity_parses_and_falls_back() {
        // A percentage larger than u8 used to fail the whole config and stop
        // the bar from starting. It must be rejected as a value, not as a file.
        let c: Config = toml::from_str("[bar]\nopacity = 500\n")
            .expect("an out-of-range opacity must not stop the bar");
        assert_eq!(
            c.bar.effective_opacity(),
            Some(u64::from(DEFAULT_BAR_OPACITY) * u64::from(u32::MAX) / 100).map(|v| v as u32)
        );
    }

    #[test]
    fn nirubar_accepts_sections_owned_by_other_programs() {
        // [idle] belongs to NIRUIDLE; refusing it would stop the bar from
        // starting on a config that NIRUWM accepts.
        let c: Config =
            toml::from_str("[idle]\nscreensaver_timeout = 5\nlock_after_screensaver = 2\n")
                .expect("the bar must accept the shared file");
        assert!(c.idle.is_table());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        // deny_unknown_fields keeps a typo from silently doing nothing, but the
        // file is shared, so both programs must describe every key they use.
        let err = toml::from_str::<Config>("gap = 8\nbogus = 1");
        assert!(err.is_err());
    }

    #[test]
    fn accepts_a_wallpaper_entry_for_one_screen() {
        // NIRUWM writes this section for a per-screen picture. A bar that
        // rejected it would not start at the next login.
        let shared = r#"
[wallpaper]
enabled = true
follow_theme = true

[[wallpaper.outputs]]
output = "DP-1"
mode = "single"
file = "~/Pictures/Wallpapers/mine/tower.png"
scale = "fit"
"#;
        let c: Config = toml::from_str(shared).expect("the bar must accept a per-screen entry");
        c.validate().expect("valid settings");
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
launcher = ["nirulauncher", "--apps"]
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
        for name in [
            "niru-noir",
            "niru-nord",
            "niru-satie",
            "niru-carl-larsson",
            "niru-forest-vigil",
            "niru-blood-of-urdr",
            "niru-root-and-stone",
            "niru-blue-horizon",
            "niru-the-norns",
            "niru-winter-nights",
        ] {
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
                "[bar]\ntags = [\"1\", \"2\", \"3\", \"4\", \"5\", \"6\", \"7\", \"8\", \"9\", \"10\", \"11\"]\n",
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
    fn reset_replaces_only_the_bar_table() {
        let base = std::env::temp_dir().join(format!("nirubar-reset-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let path = base.join("config.toml");
        fs::write(&path, "theme = \"niru-nord\"\n[bar]\ntags = [\"old\"]\n").unwrap();
        let (written, backup) = reset_bar_at(&path).unwrap();
        assert_eq!(written, path);
        assert!(backup.unwrap().exists());
        let value: toml::Value = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(value["theme"].as_str(), Some("niru-nord"));
        assert_eq!(value["bar"]["tags"].as_array().unwrap().len(), 10);
        let _ = fs::remove_dir_all(&base);
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
