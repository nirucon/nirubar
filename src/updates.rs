//! Package update counts.
//!
//! A count of available upgrades, cached so the bar never runs a package query
//! in a redraw loop, and queried on a background thread so drawing never
//! waits for the package manager. Checking is expensive on every distribution, so the result
//! lives for `update_check_minutes` and the value is read from a small file
//! that the bar writes.
//!
//! Only reading is done here: nothing is installed and no database is locked
//! for longer than the query itself.

use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

/// Which package manager this system uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manager {
    /// Void Linux, `xbps-install`.
    Xbps,
    /// Arch Linux, `pacman`.
    Pacman,
    /// Debian, `apt`.
    Apt,
    Unknown,
}

impl Manager {
    pub fn detect() -> Self {
        if std::path::Path::new("/etc/void-release").exists() {
            return Manager::Xbps;
        }
        if std::path::Path::new("/etc/arch-release").exists() {
            return Manager::Pacman;
        }
        if std::path::Path::new("/etc/debian_version").exists() {
            return Manager::Apt;
        }
        Manager::Unknown
    }

    fn command(&self) -> Option<(&'static str, &'static [&'static str])> {
        match self {
            // `-un` lists upgradable packages without touching the system.
            Manager::Xbps => Some(("xbps-install", &["-un"])),
            // `-Qu` prints one line per upgrade. `-Q` alone lists everything.
            Manager::Pacman => Some(("pacman", &["-Qu"])),
            // `apt list --upgradable` prints a header line starting with
            // "Listing...", which is filtered out below.
            Manager::Apt => Some(("apt", &["list", "--upgradable"])),
            Manager::Unknown => None,
        }
    }
}

/// Where the cached count is stored.
fn cache_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        });
    base.join("nirubar").join("updates")
}

fn which(program: &str) -> bool {
    std::env::var("PATH")
        .map(|p| {
            p.split(':')
                .any(|d| std::path::Path::new(d).join(program).is_file())
        })
        .unwrap_or(false)
}

/// Count the available upgrades by asking the package manager.
pub fn query() -> Option<usize> {
    let manager = Manager::detect();
    let (program, args) = manager.command()?;
    if !which(program) {
        return None;
    }
    let out = crate::probe::run(
        program,
        args,
        &[("LC_ALL", "C"), ("LANG", "C")],
        Duration::from_secs(4),
        256 * 1024,
    )?;
    // `pacman -Qu` exits non-zero with no output when nothing is to be done;
    // a failed query with a diagnostic is different and must not show "0".
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() && text.trim().is_empty() && !out.stderr.is_empty() {
        return None;
    }
    let count = match manager {
        Manager::Apt => text
            .lines()
            .filter(|l| l.contains("[upgradable from:"))
            .count(),
        _ => text.lines().filter(|l| !l.trim().is_empty()).count(),
    };
    Some(count)
}

/// The cached count. When it is missing or older than `max_age`, one
/// background thread asks the package manager and wakes the bar when it has
/// the answer: a query can take seconds (apt), and the X11 thread must never
/// wait for it.
///
/// Returns `None` until the first answer, and on a system without a known
/// package manager, so the module simply stays empty.
pub fn cached(max_age: Duration) -> Option<usize> {
    let path = cache_path();
    let (value, fresh) = read_cache(&path, max_age);
    if !fresh {
        refresh_in_background(path, max_age);
    }
    value
}

fn read_cache(path: &std::path::Path, max_age: Duration) -> (Option<usize>, bool) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (None, false);
    };
    let fresh = meta
        .modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .is_some_and(|age| age < max_age);
    let value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<usize>().ok());
    (value, fresh && value.is_some())
}

/// Whether a refresh may start now: none running, and the last one started
/// at least `max(max_age, 60 s)` ago, so a failing query is not retried on
/// every redraw.
fn may_start(last: Option<std::time::Instant>, now: std::time::Instant, max_age: Duration) -> bool {
    let gap = max_age.max(Duration::from_secs(60));
    last.map_or(true, |at| now.saturating_duration_since(at) >= gap)
}

fn refresh_in_background(path: PathBuf, max_age: Duration) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    static RUNNING: AtomicBool = AtomicBool::new(false);
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let now = std::time::Instant::now();
    {
        let Ok(mut last) = LAST.lock() else {
            return;
        };
        if !may_start(*last, now, max_age) || RUNNING.swap(true, Ordering::AcqRel) {
            return;
        }
        *last = Some(now);
    }
    let spawned = std::thread::Builder::new()
        .name("nirubar-updates".into())
        .spawn(move || {
            if let Some(count) = query() {
                // Written whole and renamed: another bar never reads half a
                // number. A failure to cache is not worth reporting.
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
                if std::fs::write(&tmp, count.to_string()).is_ok()
                    && std::fs::rename(&tmp, &path).is_err()
                {
                    let _ = std::fs::remove_file(&tmp);
                }
            }
            RUNNING.store(false, Ordering::Release);
            crate::wake_update();
        });
    if spawned.is_err() {
        RUNNING.store(false, Ordering::Release);
    }
}

/// Format the module text: ` 3 updates`, ` 1 update`, or empty at zero.
pub fn text(count: usize, glyph: &str) -> String {
    match count {
        0 => String::new(),
        1 => format!("{glyph} 1 update"),
        n => format!("{glyph} {n} updates"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_updates_show_nothing() {
        assert_eq!(text(0, "U"), "");
    }

    #[test]
    fn singular_and_plural() {
        assert_eq!(text(1, "U"), "U 1 update");
        assert_eq!(text(4, "U"), "U 4 updates");
    }

    #[test]
    fn detect_returns_a_known_manager() {
        let m = Manager::detect();
        assert!(matches!(
            m,
            Manager::Xbps | Manager::Pacman | Manager::Apt | Manager::Unknown
        ));
    }

    #[test]
    fn a_refresh_is_not_retried_on_every_redraw() {
        let now = std::time::Instant::now();
        let thirty = Duration::from_secs(1800);
        assert!(may_start(None, now, thirty));
        assert!(!may_start(Some(now), now + Duration::from_secs(5), thirty));
        assert!(may_start(Some(now), now + thirty, thirty));
        // 0 minutes still waits a minute between queries.
        assert!(!may_start(
            Some(now),
            now + Duration::from_secs(30),
            Duration::ZERO
        ));
        assert!(may_start(
            Some(now),
            now + Duration::from_secs(60),
            Duration::ZERO
        ));
    }

    #[test]
    fn the_cache_is_read_without_a_query() {
        let dir = std::env::temp_dir().join(format!("nirubar-upd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("updates");
        assert_eq!(read_cache(&file, Duration::from_secs(60)), (None, false));
        std::fs::write(&file, "7").unwrap();
        assert_eq!(read_cache(&file, Duration::from_secs(60)), (Some(7), true));
        // Stale: the old value is still shown while a refresh runs.
        assert_eq!(read_cache(&file, Duration::ZERO), (Some(7), false));
        std::fs::write(&file, "junk").unwrap();
        assert_eq!(read_cache(&file, Duration::from_secs(60)), (None, false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_manager_has_no_command() {
        assert!(Manager::Unknown.command().is_none());
    }
}
