//! Programs that still run from an older NIRUX version.
//!
//! After `nirux update`, a program that kept running loads its libraries and
//! helpers from a generation that is gone (a browser whose directory was
//! replaced loses its camera, for example). `nirux stale --json` lists them:
//!
//! ```json
//! {"stale":[{"name":"Helium","program":"helium","restart":"manual"},
//!           {"name":"NIRUWM","program":"niruwm","restart":"auto"}]}
//! ```
//!
//! `restart` is `auto` when NIRUX can restart the program in place
//! (`nirux stale restart` does it) and `manual` when you must close and
//! reopen it yourself.
//!
//! The question runs on a worker thread, shortly after the bar starts, a few
//! seconds after a click, and every ten minutes while something is stale or
//! a NIRUX generation (`~/.local/share/nirux/current`, the browser, LocalSend
//! and OpenCode links) changed since the
//! last answer. A program can only become stale when the generation changes,
//! so while nothing is stale the check (half a second of CPU) waits for that,
//! with one safety check every half hour. A redraw only reads the last
//! answer, so the module costs a lock and a clone per second while idle. An
//! old NIRUX without the command, or any failure, means "nothing is stale".

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// How often `nirux stale --json` runs while something is stale. A click
/// on the module (which restarts what it can) checks again after
/// `AFTER_CLICK`; a program closed and reopened by hand is noticed here.
pub const CHECK_EVERY: Duration = Duration::from_secs(600);
/// The first check after the bar starts: the session is still settling.
pub const FIRST_CHECK: Duration = Duration::from_secs(8);
/// The check after a click, when the restarted programs are back.
pub const AFTER_CLICK: Duration = Duration::from_secs(3);
/// While nothing is stale and the generation stays the same.
pub const QUIET_CHECK_EVERY: Duration = Duration::from_secs(30 * 60);
/// How long one check may take before it counts as a failure.
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
/// The most entries shown; a longer list is cut, never an unbounded panel.
const MAX_ENTRIES: usize = 24;
/// The longest name kept, in characters.
const MAX_NAME: usize = 40;

/// One program that runs from an old version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// NIRUX restarts it in place; otherwise the user closes and reopens it.
    pub auto: bool,
}

/// Read the `nirux stale --json` answer. Anything unexpected is "nothing".
pub fn parse(text: &str) -> Vec<Entry> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(list) = value.get("stale").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let field = |item: &serde_json::Value, key: &str| {
        item.get(key)
            .and_then(|v| v.as_str())
            .map(|s| {
                s.trim()
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect::<String>()
            })
            .unwrap_or_default()
    };
    list.iter()
        .filter_map(|item| {
            let mut name = field(item, "name");
            if name.is_empty() {
                name = field(item, "program");
            }
            if name.is_empty() {
                return None;
            }
            Some(Entry {
                name: name.chars().take(MAX_NAME).collect(),
                auto: field(item, "restart") == "auto",
            })
        })
        .take(MAX_ENTRIES)
        .collect()
}

/// The bar text: empty, `GLYPH Name` for one program, `GLYPH N` for several.
pub fn label(entries: &[Entry], glyph: &str) -> String {
    match entries {
        [] => String::new(),
        [one] => format!("{glyph} {}", one.name),
        many => format!("{glyph} {}", many.len()),
    }
}

/// The panel lines: each program and how it restarts, then what a click does.
pub fn detail(entries: &[Entry]) -> Vec<String> {
    if entries.is_empty() {
        return vec!["every program runs the installed version".into()];
    }
    let mut lines: Vec<String> = entries
        .iter()
        .map(|e| {
            if e.auto {
                format!("{}: restarts automatically", e.name)
            } else {
                format!("{}: close and reopen it", e.name)
            }
        })
        .collect();
    lines.push(String::new());
    let any_auto = entries.iter().any(|e| e.auto);
    lines.push(if any_auto {
        "left click: restart the automatic ones".into()
    } else {
        "left click: show which to reopen".into()
    });
    lines.push("nirux stale restart".into());
    lines
}

struct Shared {
    entries: Vec<Entry>,
    next: Instant,
    running: bool,
    /// The generation the last answer was given for, and when.
    generation: Vec<Option<std::path::PathBuf>>,
    answered: Option<Instant>,
    /// A click asked for the next check regardless of the generation.
    forced: bool,
}

/// The links `nirux stale` compares running programs with: the suite
/// generation and the separately updated browser, LocalSend and OpenCode.
fn generation() -> Vec<Option<std::path::PathBuf>> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let state = std::path::Path::new(&home).join(".local/share/nirux");
    [
        "current",
        "browser-current",
        "localsend/current",
        "opencode/current",
    ]
    .iter()
    .map(|link| std::fs::read_link(state.join(link)).ok())
    .collect()
}

/// The last answer and when to ask again, shared with the worker thread.
#[derive(Clone)]
pub struct Monitor {
    shared: Arc<Mutex<Shared>>,
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

fn ask() -> Vec<Entry> {
    if !on_path("nirux") {
        return Vec::new();
    }
    match crate::probe::run("nirux", &["stale", "--json"], &[], CHECK_TIMEOUT, 64 * 1024) {
        Some(out) if out.status.success() => parse(&String::from_utf8_lossy(&out.stdout)),
        _ => Vec::new(),
    }
}

impl Monitor {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared {
                entries: Vec::new(),
                next: Instant::now() + FIRST_CHECK,
                running: false,
                generation: Vec::new(),
                answered: None,
                forced: false,
            })),
        }
    }

    /// The last answer. Starts a check on a worker when one is due; never
    /// waits for it.
    pub fn entries(&self) -> Vec<Entry> {
        let Ok(mut shared) = self.shared.lock() else {
            return Vec::new();
        };
        let due = !shared.running && Instant::now() >= shared.next && !disabled() && {
            let current = generation();
            let quiet = shared.entries.is_empty()
                && !shared.forced
                && current == shared.generation
                && shared
                    .answered
                    .is_some_and(|at| at.elapsed() < QUIET_CHECK_EVERY);
            if !quiet {
                shared.generation = current;
            }
            !quiet
        };
        if due {
            shared.running = true;
            shared.forced = false;
            let worker = self.shared.clone();
            let spawned = std::thread::Builder::new()
                .name("nirubar-stale".into())
                .spawn(move || {
                    let entries = ask();
                    if let Ok(mut shared) = worker.lock() {
                        shared.answered = Some(Instant::now());
                        shared.entries = entries;
                        shared.next = Instant::now() + CHECK_EVERY;
                        shared.running = false;
                    }
                });
            if spawned.is_err() {
                shared.running = false;
                shared.next = Instant::now() + CHECK_EVERY;
            }
        }
        shared.entries.clone()
    }

    /// Ask again `after` from now (a click restarted something).
    pub fn check_in(&self, after: Duration) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.next = Instant::now() + after;
            shared.forced = true;
        }
    }

    #[cfg(test)]
    fn set(&self, entries: Vec<Entry>) {
        self.shared.lock().unwrap().entries = entries;
    }

    #[cfg(test)]
    fn due_in(&self) -> Duration {
        self.shared
            .lock()
            .unwrap()
            .next
            .saturating_duration_since(Instant::now())
    }
}

/// Unit tests and `NIRUBAR_NO_UPDATE_CHECK` never run `nirux`.
fn disabled() -> bool {
    cfg!(test) || std::env::var_os("NIRUBAR_NO_UPDATE_CHECK").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, auto: bool) -> Entry {
        Entry {
            name: name.into(),
            auto,
        }
    }

    #[test]
    fn the_stale_list_is_read() {
        let json = r#"{"stale":[{"name":"Helium","program":"helium","restart":"manual"},{"name":"NIRUWM","program":"niruwm","restart":"auto"}]}"#;
        assert_eq!(
            parse(json),
            vec![entry("Helium", false), entry("NIRUWM", true)]
        );
    }

    #[test]
    fn an_empty_or_broken_answer_is_nothing() {
        assert!(parse(r#"{"stale":[]}"#).is_empty());
        assert!(parse("").is_empty());
        assert!(parse("nirux: unknown command stale").is_empty());
        assert!(parse(r#"{"stale":"helium"}"#).is_empty());
        assert!(parse(r#"{"other":[]}"#).is_empty());
    }

    #[test]
    fn a_missing_name_falls_back_to_the_program_and_odd_entries_are_skipped() {
        let json = r#"{"stale":[{"program":"kitty","restart":"auto"},{"restart":"auto"},42,{"name":"  ","program":""},{"name":"Zed","restart":"later"}]}"#;
        assert_eq!(parse(json), vec![entry("kitty", true), entry("Zed", false)]);
    }

    #[test]
    fn names_are_bounded_and_control_characters_dropped() {
        let long = "x".repeat(200);
        let json = format!(
            r#"{{"stale":[{{"name":"{long}","restart":"auto"}},{{"name":"A\u001b[31mB","restart":"manual"}}]}}"#
        );
        let got = parse(&json);
        assert_eq!(got[0].name.chars().count(), MAX_NAME);
        assert_eq!(got[1].name, "A[31mB");
        let many = format!(
            r#"{{"stale":[{}]}}"#,
            vec![r#"{"name":"a"}"#; 100].join(",")
        );
        assert_eq!(parse(&many).len(), MAX_ENTRIES);
    }

    #[test]
    fn the_label_is_hidden_named_or_counted() {
        assert_eq!(label(&[], "R"), "");
        assert_eq!(label(&[entry("Helium", false)], "R"), "R Helium");
        assert_eq!(
            label(&[entry("Helium", false), entry("NIRUWM", true)], "R"),
            "R 2"
        );
    }

    #[test]
    fn the_panel_says_how_each_program_restarts() {
        let lines = detail(&[entry("Helium", false), entry("NIRUWM", true)]);
        assert_eq!(lines[0], "Helium: close and reopen it");
        assert_eq!(lines[1], "NIRUWM: restarts automatically");
        assert!(lines.iter().any(|l| l == "nirux stale restart"));
        let manual = detail(&[entry("Helium", false)]);
        assert!(manual.iter().any(|l| l.contains("show which to reopen")));
        assert_eq!(detail(&[]).len(), 1);
    }

    #[test]
    fn the_monitor_never_runs_nirux_under_test_and_rechecks_after_a_click() {
        let m = Monitor::new();
        assert!(m.entries().is_empty());
        m.set(vec![entry("Helium", false)]);
        assert_eq!(m.entries(), vec![entry("Helium", false)]);
        m.check_in(AFTER_CLICK);
        assert!(m.due_in() <= AFTER_CLICK);
    }
}
