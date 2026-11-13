//! Disk space warning: `/` and `$HOME`, measured with statvfs.
//!
//! One background thread per bar process reads both filesystems every minute
//! and publishes a snapshot; drawing only copies it. The module stays hidden
//! until a filesystem reaches `[bar] disk_warn` percent used.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// How often the filesystems are measured.
const EVERY: Duration = Duration::from_secs(60);

/// Space on one watched filesystem, in bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    /// What the person knows it as: `/` or `home`.
    pub label: String,
    pub total: u64,
    /// Free for an unprivileged user (what `df` calls available).
    pub free: u64,
    pub used: u64,
}

impl Usage {
    /// Percent used, the way `df` counts it: used / (used + available),
    /// rounded up, so the root reserve does not hide a full disk.
    pub fn percent(&self) -> u8 {
        let base = self.used.saturating_add(self.free);
        if base == 0 {
            return 0;
        }
        let pct = (u128::from(self.used) * 100).div_ceil(u128::from(base));
        pct.min(100) as u8
    }
}

/// The filesystems at or above the threshold, fullest first.
pub fn over(usages: &[Usage], warn: u8) -> Vec<&Usage> {
    let mut hit: Vec<&Usage> = usages.iter().filter(|u| u.percent() >= warn).collect();
    hit.sort_by_key(|u| std::cmp::Reverse(u.percent()));
    hit
}

/// The watched paths with their labels: `/`, then the home directory.
fn watched() -> Vec<(String, PathBuf)> {
    let mut out = vec![("/".to_string(), PathBuf::from("/"))];
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        out.push(("home".to_string(), PathBuf::from(home)));
    }
    out
}

/// Keep the first path of each filesystem: `$HOME` on the root filesystem
/// is shown once, as `/`. `device` maps a path to its filesystem id; a path
/// that cannot be read is dropped.
pub fn dedupe<F>(paths: Vec<(String, PathBuf)>, device: F) -> Vec<(String, PathBuf)>
where
    F: Fn(&Path) -> Option<u64>,
{
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for (label, path) in paths {
        let Some(dev) = device(&path) else {
            continue;
        };
        if !seen.contains(&dev) {
            seen.push(dev);
            out.push((label, path));
        }
    }
    out
}

fn statvfs(label: &str, path: &Path) -> Option<Usage> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut buf = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: a valid NUL-terminated path and a correctly sized out-struct.
    if unsafe { libc::statvfs(c.as_ptr(), buf.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: a successful statvfs initialised it.
    let st = unsafe { buf.assume_init() };
    let frag = st.f_frsize;
    let total = st.f_blocks.saturating_mul(frag);
    let free_all = st.f_bfree.saturating_mul(frag);
    let free = st.f_bavail.saturating_mul(frag);
    Some(Usage {
        label: label.to_string(),
        total,
        free,
        used: total.saturating_sub(free_all),
    })
}

/// Measure every watched filesystem once.
pub fn measure() -> Vec<Usage> {
    dedupe(watched(), |p| std::fs::metadata(p).ok().map(|m| m.dev()))
        .into_iter()
        .filter_map(|(label, path)| statvfs(&label, &path))
        .collect()
}

/// The latest snapshot, shared by every disk module of this process.
pub fn snapshot() -> Vec<Usage> {
    static SHARED: OnceLock<Arc<Mutex<Vec<Usage>>>> = OnceLock::new();
    let shared = SHARED.get_or_init(|| {
        let shared = Arc::new(Mutex::new(measure()));
        let writer = shared.clone();
        let _ = std::thread::Builder::new()
            .name("nirubar-disk".into())
            .spawn(move || loop {
                std::thread::sleep(EVERY);
                let fresh = measure();
                let changed = match writer.lock() {
                    Ok(mut current) if *current != fresh => {
                        let before: Vec<u8> = current.iter().map(Usage::percent).collect();
                        *current = fresh;
                        before != current.iter().map(Usage::percent).collect::<Vec<_>>()
                    }
                    _ => false,
                };
                if changed {
                    crate::wake_update();
                }
            });
        shared
    });
    shared.lock().map(|u| u.clone()).unwrap_or_default()
}

/// Bytes as a short human size: `512 MB`, `3.2 GB`, `1.8 TB`.
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit <= 2 || value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Bar text: the mark and the fullest percentage, or nothing below the
/// threshold.
pub fn text(usages: &[Usage], warn: u8, icon: &str) -> String {
    match over(usages, warn).first() {
        Some(u) => format!("{icon} {}%", u.percent()),
        None => String::new(),
    }
}

/// Panel lines: every watched filesystem, then how to clean up.
pub fn detail(usages: &[Usage], warn: u8, can_clean: bool) -> Vec<String> {
    let mut lines = Vec::new();
    if usages.is_empty() {
        lines.push("cannot read the disk space".to_string());
    }
    for u in usages {
        lines.push(format!("{}: {}% used", u.label, u.percent()));
        lines.push(format!(
            "  {} used, {} free of {}",
            human(u.used),
            human(u.free),
            human(u.total)
        ));
    }
    lines.push(format!("warns at {warn}% (disk_warn)"));
    lines.push(String::new());
    if can_clean {
        lines.push("Click to clean up (nirux clean asks first)".to_string());
    } else {
        lines.push("nirux clean is not installed".to_string());
    }
    lines
}

/// The terminal command line that runs `nirux clean` and keeps the window
/// open until Enter, so the result can be read.
pub fn clean_command(terminal: &[String]) -> Option<(String, Vec<String>)> {
    let (program, rest) = terminal.split_first()?;
    let mut args: Vec<String> = rest.to_vec();
    let name = Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program);
    // kitty runs its trailing arguments as the program; the others want -e.
    let has_exec = args.iter().any(|a| a == "-e" || a == "--");
    if name != "kitty" && !has_exec {
        args.push("-e".into());
    }
    args.extend([
        "sh".into(),
        "-c".into(),
        "nirux clean; printf '\\nPress Enter to close. '; read _".into(),
    ]);
    Some((program.clone(), args))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(label: &str, used: u64, free: u64) -> Usage {
        Usage {
            label: label.into(),
            total: used + free,
            free,
            used,
        }
    }

    #[test]
    fn hidden_below_the_threshold_and_shown_at_it() {
        let u = vec![usage("/", 89, 11), usage("home", 50, 50)];
        assert_eq!(text(&u, 90, "D"), "");
        assert_eq!(text(&u, 89, "D"), "D 89%");
        assert!(over(&u, 90).is_empty());
        let u = vec![usage("/", 90, 10)];
        assert_eq!(text(&u, 90, "D"), "D 90%");
    }

    #[test]
    fn the_fullest_filesystem_is_shown() {
        let u = vec![usage("/", 91, 9), usage("home", 97, 3)];
        assert_eq!(text(&u, 90, "D"), "D 97%");
        assert_eq!(over(&u, 90)[0].label, "home");
    }

    #[test]
    fn percent_rounds_up_like_df_and_ignores_the_root_reserve() {
        // 905 used of 1000 available to users: 90.5 % shows as 91 %.
        assert_eq!(usage("/", 905, 95).percent(), 91);
        assert_eq!(usage("/", 0, 0).percent(), 0);
        // The root reserve is in total but neither used nor free.
        let u = Usage {
            label: "/".into(),
            total: 1000,
            free: 0,
            used: 950,
        };
        assert_eq!(u.percent(), 100);
    }

    #[test]
    fn home_on_the_root_filesystem_is_listed_once() {
        let paths = vec![
            ("/".to_string(), PathBuf::from("/")),
            ("home".to_string(), PathBuf::from("/home/u")),
        ];
        let same = dedupe(paths.clone(), |_| Some(7));
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].0, "/");
        let split = dedupe(paths.clone(), |p| {
            Some(if p == Path::new("/") { 1 } else { 2 })
        });
        assert_eq!(split.len(), 2);
        let unreadable = dedupe(paths, |p| (p == Path::new("/")).then_some(1));
        assert_eq!(unreadable.len(), 1);
    }

    #[test]
    fn the_root_filesystem_can_be_measured() {
        let u = statvfs("/", Path::new("/")).unwrap();
        assert!(u.total > 0 && u.used <= u.total && u.percent() <= 100);
        assert!(statvfs("x", Path::new("/no/such/nirubar/path")).is_none());
        assert!(!measure().is_empty());
    }

    #[test]
    fn sizes_are_short() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(3_200_000_000), "3.2 GB");
        assert_eq!(human(250_000_000_000), "250 GB");
        assert_eq!(human(1_800_000_000_000), "1.8 TB");
    }

    #[test]
    fn panel_lists_each_filesystem_and_the_clean_up() {
        let u = vec![usage("/", 93, 7), usage("home", 40, 60)];
        let lines = detail(&u, 90, true);
        assert!(lines[0].starts_with("/: 93% used"));
        assert!(lines.iter().any(|l| l.starts_with("home: 40% used")));
        assert!(lines.last().unwrap().contains("Click to clean up"));
        assert!(detail(&u, 90, false)
            .last()
            .unwrap()
            .contains("not installed"));
    }

    #[test]
    fn clean_runs_in_the_configured_terminal() {
        let (p, a) = clean_command(&["kitty".into()]).unwrap();
        assert_eq!(p, "kitty");
        assert_eq!(a[0], "sh");
        let (p, a) = clean_command(&["alacritty".into()]).unwrap();
        assert_eq!(p, "alacritty");
        assert_eq!(&a[..3], ["-e", "sh", "-c"]);
        assert!(a[3].starts_with("nirux clean"));
        let (_, a) = clean_command(&["xterm".into(), "-e".into()]).unwrap();
        assert_eq!(a.iter().filter(|x| *x == "-e").count(), 1);
        assert!(clean_command(&[]).is_none());
    }
}
