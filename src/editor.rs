//! Arrange the bar from the command line: `nirubar --modules ...` and
//! `nirubar --set KEY VALUE`.
//!
//! The configuration file stays the only place the bar is described. Every
//! change is one edit of the shared `[bar]` table with `toml_edit`, so
//! comments and every other key are kept; the whole file is checked before it
//! replaces the old one, and the bars reload. NIRUWM keeps the file as it was
//! in its settings history, so `niructl config undo` goes back.
//!
//! A list arranged here is the person's own: `follow_default_modules` is set
//! to `false`, so `nirux update` never puts the default list back over it.
//! `--modules default` returns to the NIRUX list and follows it again.
use crate::config::{active_path, Bar, Config, Result};
use std::fs;

/// Every module the bar knows, in the order the arranger lists the ones that
/// are off, with what it shows.
pub const CATALOG: &[(&str, &str)] = &[
    ("workspaces", "Workspace cells; click to switch"),
    ("dictation", "NIRUDICTATE recording and transcribing mark"),
    (
        "privacy",
        "A dot while the microphone, a camera or a screen recording is in use",
    ),
    ("recording", "NIRUCLIP recording timer"),
    ("scratchpad", "NIRUWM scratchpads"),
    ("layout", "The current NIRUWM layout"),
    ("media", "The playing track with a progress line"),
    ("volume", "Output volume and mute"),
    ("microphone", "Microphone mute and gain"),
    ("brightness", "Backlight; hidden without one"),
    ("display", "Where this screen sits among several"),
    ("tailscale", "Tailscale peers, when Tailscale is installed"),
    ("network", "Wired network"),
    ("wifi", "Wi-Fi connection"),
    ("bluetooth", "Bluetooth power and connected device"),
    ("powerprofile", "Active power profile"),
    ("mode", "Work mode countdown and studio mode"),
    ("battery", "Battery charge and state"),
    ("updates", "Available package updates"),
    ("niruxupdate", "A newer NIRUX release"),
    (
        "restart",
        "Programs to restart after a NIRUX update; hidden when none",
    ),
    ("agent", "OpenCode prompt entry"),
    ("tasks", "NIRU and OpenCode task status"),
    ("nirusec", "NIRUSEC firewall state"),
    ("dnd", "Do Not Disturb"),
    ("notifications", "Notification history"),
    ("disk", "A warning when the disk is nearly full"),
    (
        "drives",
        "USB sticks and SD cards: open and eject; hidden without one",
    ),
    ("timer", "Focus and egg timer"),
    ("headset", "Battery of connected Bluetooth devices"),
    ("pulse", "CPU and memory"),
    ("uptime", "Time since start"),
    ("sun", "Sunrise and sunset"),
    ("weather", "Weather with a rain timeline"),
    ("quicksettings", "Quick Settings"),
    ("share", "NIRUSHARE inbox"),
    ("clock", "Date and time"),
];

/// Settings `--set` changes, with the values each accepts.
pub const SETTINGS: &[(&str, &str)] = &[
    ("position", "top|bottom"),
    ("title", "true|false"),
    ("icons", "true|false"),
    ("show_date", "true|false"),
    ("show_time", "true|false"),
    ("tray", "true|false"),
    ("popups", "true|false"),
];

pub fn known(name: &str) -> bool {
    CATALOG.iter().any(|(n, _)| *n == name)
}

/// The usage line for the arranger.
pub const USAGE: &str = "Usage: nirubar --modules [all | add NAME [POSITION] | remove NAME | \
toggle NAME | move NAME up|down|first|last|POSITION | default]\n       \
nirubar --settings\n       nirubar --set KEY VALUE (position, title, icons, show_date, \
show_time, tray, popups)";

/// One change to the module list.
#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    Add(String, Option<usize>),
    Remove(String),
    Toggle(String),
    Move(String, Place),
    Default,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Place {
    Up,
    Down,
    First,
    Last,
    /// 1-based position.
    At(usize),
}

pub fn parse_change(args: &[String]) -> std::result::Result<Change, String> {
    let word = |i: usize| args.get(i).map(String::as_str);
    let name = |i: usize| -> std::result::Result<String, String> {
        let name = word(i).ok_or_else(|| USAGE.to_string())?;
        if known(name) {
            Ok(name.to_string())
        } else {
            Err(format!(
                "unknown module {name:?}; `nirubar --modules all` lists them"
            ))
        }
    };
    let position = |text: &str| -> std::result::Result<usize, String> {
        text.parse::<usize>()
            .ok()
            .filter(|n| *n >= 1)
            .ok_or_else(|| format!("position must be 1 or more, not {text:?}"))
    };
    let change = match word(0) {
        Some("add") => Change::Add(name(1)?, word(2).map(position).transpose()?),
        Some("remove") => Change::Remove(name(1)?),
        Some("toggle") => Change::Toggle(name(1)?),
        Some("move") => {
            let module = name(1)?;
            let place = match word(2) {
                Some("up") => Place::Up,
                Some("down") => Place::Down,
                Some("first") => Place::First,
                Some("last") => Place::Last,
                Some(text) => Place::At(position(text)?),
                None => return Err(USAGE.into()),
            };
            Change::Move(module, place)
        }
        Some("default") => Change::Default,
        _ => return Err(USAGE.into()),
    };
    let expected = match change {
        Change::Add(_, Some(_)) | Change::Move(..) => 3,
        Change::Add(_, None) | Change::Remove(_) | Change::Toggle(_) => 2,
        Change::Default => 1,
    };
    if args.len() != expected {
        return Err(USAGE.into());
    }
    Ok(change)
}

/// The list after `change`, or the reason it cannot be made.
pub fn apply(modules: &[String], change: &Change) -> std::result::Result<Vec<String>, String> {
    let mut out = modules.to_vec();
    let index = |list: &[String], name: &str| list.iter().position(|m| m == name);
    match change {
        Change::Add(name, at) => {
            if index(&out, name).is_some() {
                return Err(format!("{name} is already on the bar"));
            }
            let at = at.map_or(out.len(), |n| (n - 1).min(out.len()));
            out.insert(at, name.clone());
        }
        Change::Remove(name) => {
            let at = index(&out, name).ok_or_else(|| format!("{name} is not on the bar"))?;
            out.remove(at);
        }
        Change::Toggle(name) => match index(&out, name) {
            Some(at) => {
                out.remove(at);
            }
            None => out.push(name.clone()),
        },
        Change::Move(name, place) => {
            let from = index(&out, name).ok_or_else(|| format!("{name} is not on the bar"))?;
            let last = out.len() - 1;
            let to = match place {
                Place::Up => from.saturating_sub(1),
                Place::Down => (from + 1).min(last),
                Place::First => 0,
                Place::Last => last,
                Place::At(n) => (n - 1).min(last),
            };
            let item = out.remove(from);
            out.insert(to, item);
        }
        Change::Default => out = Bar::default().modules,
    }
    Ok(out)
}

/// Every known module, the ones on the bar first and in their order, then the
/// rest in catalog order: `name<TAB>on|off<TAB>description`. A name in the
/// file that this bar does not know is listed as on, without a description.
pub fn all_lines(modules: &[String]) -> Vec<String> {
    let describe = |name: &str| {
        CATALOG
            .iter()
            .find(|(n, _)| *n == name)
            .map_or("", |(_, d)| *d)
    };
    let mut lines: Vec<String> = modules
        .iter()
        .map(|name| format!("{name}\ton\t{}", describe(name)))
        .collect();
    for (name, description) in CATALOG {
        if !modules.iter().any(|m| m == name) {
            lines.push(format!("{name}\toff\t{description}"));
        }
    }
    lines
}

/// The current values of the settings `--set` changes: `key<TAB>value`.
pub fn settings_lines(config: &Config) -> Vec<String> {
    let bar = &config.bar;
    let position = if bar.position == "bottom" {
        "bottom"
    } else {
        "top"
    };
    vec![
        format!("position\t{position}"),
        format!("title\t{}", bar.show_title),
        format!("icons\t{}", bar.icons),
        format!("show_date\t{}", bar.show_date),
        format!("show_time\t{}", bar.show_time),
        format!("tray\t{}", bar.tray),
        format!("popups\t{}", bar.popups),
        format!("follow_default_modules\t{}", bar.follow_default_modules),
    ]
}

/// The file text with the new module list. `follow` is the new value of
/// `follow_default_modules`.
pub fn with_modules(text: &str, modules: &[String], follow: bool) -> Result<String> {
    let mut document = parse(text)?;
    let table = bar_table(&mut document)?;
    let mut array = toml_edit::Array::new();
    for module in modules {
        array.push(module.as_str());
    }
    table["modules"] = toml_edit::value(array);
    table["follow_default_modules"] = toml_edit::value(follow);
    checked(document)
}

/// The file text with one setting changed.
pub fn with_setting(text: &str, key: &str, value: &str) -> Result<String> {
    let mut document = parse(text)?;
    let table = bar_table(&mut document)?;
    let boolean = || match value {
        "true" | "on" | "yes" => Ok(true),
        "false" | "off" | "no" => Ok(false),
        _ => Err(format!("{key} must be true or false, not {value:?}")),
    };
    match key {
        "position" => {
            if !matches!(value, "top" | "bottom") {
                return Err(format!("position must be top or bottom, not {value:?}").into());
            }
            table["position"] = toml_edit::value(value);
        }
        // Older files write `title`; both are the same setting.
        "title" | "show_title" => {
            let on = boolean()?;
            let key = if table.contains_key("show_title") {
                "show_title"
            } else {
                "title"
            };
            table[key] = toml_edit::value(on);
        }
        "icons" | "show_date" | "show_time" | "tray" | "popups" => {
            table[key] = toml_edit::value(boolean()?);
        }
        _ => {
            return Err(format!(
                "unknown setting {key:?}; it can be {}",
                SETTINGS
                    .iter()
                    .map(|(k, _)| *k)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .into())
        }
    }
    checked(document)
}

fn parse(text: &str) -> Result<toml_edit::DocumentMut> {
    Ok(text
        .parse()
        .map_err(|e| format!("{}: {e}", active_path().display()))?)
}

fn bar_table(document: &mut toml_edit::DocumentMut) -> Result<&mut toml_edit::Table> {
    let item = document
        .entry("bar")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    Ok(item.as_table_mut().ok_or("bar must be a [bar] table")?)
}

/// The whole file must still load before it replaces the old one: a typing
/// error elsewhere is reported, never written over.
fn checked(document: toml_edit::DocumentMut) -> Result<String> {
    let text = document.to_string();
    Config::from_text(&text)?;
    Ok(text)
}

/// The current file text and its module list.
pub fn read() -> Result<(String, Vec<String>)> {
    let path = active_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let modules = Config::from_text(&text)?.bar.modules;
    Ok((text, modules))
}

/// Replace the file in one step, keeping its permissions.
pub fn write(text: &str) -> Result<()> {
    let path = active_path();
    let parent = path.parent().ok_or("configuration directory is missing")?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".config.toml.{}", std::process::id()));
    fs::write(&temp, text)?;
    if let Ok(metadata) = fs::metadata(&path) {
        let _ = fs::set_permissions(&temp, metadata.permissions());
    }
    fs::rename(&temp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn args(words: &str) -> Vec<String> {
        words.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_catalog_holds_every_default_module_once() {
        for name in Bar::default().modules {
            assert!(known(&name), "{name} is missing from the catalog");
        }
        let mut names: Vec<&str> = CATALOG.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), CATALOG.len(), "a name is listed twice");
    }

    #[test]
    fn changes_parse_and_reject_unknown_names() {
        assert_eq!(
            parse_change(&args("add pulse 3")),
            Ok(Change::Add("pulse".into(), Some(3)))
        );
        assert_eq!(
            parse_change(&args("move clock up")),
            Ok(Change::Move("clock".into(), Place::Up))
        );
        assert_eq!(parse_change(&args("default")), Ok(Change::Default));
        assert!(parse_change(&args("add nonsense")).is_err());
        assert!(parse_change(&args("move clock 0")).is_err());
        assert!(parse_change(&args("remove clock extra")).is_err());
        assert!(parse_change(&args("")).is_err());
    }

    #[test]
    fn modules_are_added_removed_toggled_and_moved() {
        let start = list(&["workspaces", "volume", "clock"]);
        let step = |list: &[String], words: &str| apply(list, &parse_change(&args(words)).unwrap());
        assert_eq!(
            step(&start, "add pulse 2").unwrap(),
            list(&["workspaces", "pulse", "volume", "clock"])
        );
        assert_eq!(
            step(&start, "add pulse").unwrap(),
            list(&["workspaces", "volume", "clock", "pulse"])
        );
        assert!(step(&start, "add clock").is_err());
        assert_eq!(
            step(&start, "remove volume").unwrap(),
            list(&["workspaces", "clock"])
        );
        assert!(step(&start, "remove pulse").is_err());
        assert_eq!(
            step(&start, "toggle volume").unwrap(),
            list(&["workspaces", "clock"])
        );
        assert_eq!(
            step(&start, "toggle pulse").unwrap(),
            list(&["workspaces", "volume", "clock", "pulse"])
        );
        assert_eq!(
            step(&start, "move clock up").unwrap(),
            list(&["workspaces", "clock", "volume"])
        );
        assert_eq!(step(&start, "move workspaces up").unwrap(), start);
        assert_eq!(step(&start, "move clock down").unwrap(), start);
        assert_eq!(
            step(&start, "move clock first").unwrap(),
            list(&["clock", "workspaces", "volume"])
        );
        assert_eq!(
            step(&start, "move workspaces 9").unwrap(),
            list(&["volume", "clock", "workspaces"])
        );
        assert_eq!(step(&start, "default").unwrap(), Bar::default().modules);
    }

    #[test]
    fn all_lists_the_bar_first_then_the_rest() {
        let lines = all_lines(&list(&["clock", "volume"]));
        assert_eq!(lines[0], "clock\ton\tDate and time");
        assert_eq!(lines[1], "volume\ton\tOutput volume and mute");
        assert_eq!(lines.len(), CATALOG.len());
        assert!(lines[2..].iter().all(|l| l.contains("\toff\t")));
    }

    const FILE: &str = "# my bar\ngap = 8\n\n[bar]\n# keep this comment\nmodules = [\"workspaces\", \"clock\"]\nseparator = \"|\"\n";

    #[test]
    fn a_new_list_keeps_comments_and_stops_following_the_default() {
        let text = with_modules(FILE, &list(&["clock", "workspaces"]), false).unwrap();
        assert!(text.contains("# my bar") && text.contains("# keep this comment"));
        assert!(
            text.contains("modules = [\"clock\", \"workspaces\"]"),
            "{text}"
        );
        assert!(text.contains("follow_default_modules = false"));
        assert!(text.contains("gap = 8"));
        let text = with_modules(FILE, &Bar::default().modules, true).unwrap();
        assert!(text.contains("follow_default_modules = true"));
    }

    #[test]
    fn settings_are_checked_and_written() {
        let text = with_setting(FILE, "position", "bottom").unwrap();
        assert!(text.contains("position = \"bottom\""));
        assert!(with_setting(FILE, "position", "left").is_err());
        let text = with_setting(FILE, "icons", "off").unwrap();
        assert!(text.contains("icons = false"));
        assert!(with_setting(FILE, "icons", "maybe").is_err());
        assert!(with_setting(FILE, "nonsense", "true").is_err());
        let text = with_setting("[bar]\nshow_title = false\n", "title", "true").unwrap();
        assert!(text.contains("show_title = true") && !text.contains("\ntitle"));
    }

    #[test]
    fn a_broken_file_is_never_written_over() {
        assert!(with_modules("[bar\nmodules = []", &list(&["clock"]), false).is_err());
        assert!(with_modules("[bar]\ncell_width = 5\n", &list(&["clock"]), false).is_err());
    }
}
