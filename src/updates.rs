//! Package update counts.
//!
//! A count of available upgrades, cached so the bar never runs a package query
//! in a redraw loop. Checking is expensive on every distribution, so the result
//! lives for `update_check_minutes` and the value is read from a small file
//! that the bar writes.
//!
//! Only reading is done here: nothing is installed and no database is locked
//! for longer than the query itself.

use std::{
    path::PathBuf,
    process::Command,
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
    let out = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .output()
        .ok()?;
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

/// The cached count, refreshed when it is older than `max_age`.
///
/// Returns `None` when this system has no known package manager, so a bar on an
/// unsupported distribution simply omits the module.
pub fn cached(max_age: Duration) -> Option<usize> {
    let path = cache_path();
    if let Ok(meta) = std::fs::metadata(&path) {
        let fresh = meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .map(|age| age < max_age)
            .unwrap_or(false);
        if fresh {
            if let Ok(text) = std::fs::read_to_string(&path) {
                return text.trim().parse::<usize>().ok();
            }
        }
    }
    let count = query()?;
    // Write the new value; a failure to cache is not worth reporting.
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, count.to_string());
    Some(count)
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
    fn unknown_manager_has_no_command() {
        assert!(Manager::Unknown.command().is_none());
    }
}
