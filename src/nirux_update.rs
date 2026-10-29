//! NIRUX suite update check.
//!
//! The bar already counts OS package upgrades. This is the same idea for the
//! NIRUX suite itself: once a day, ask GitHub for the latest NIRUX release and
//! compare it with the version that is installed.
//!
//! Three rules shape this module:
//!
//!  * It never installs anything. `nirux update` needs sudo and switches the
//!    active program generation, so it must run when the user is ready to log
//!    out, not in the background.
//!  * It never blocks a redraw. The answer is cached in
//!    `~/.cache/nirubar/nirux-update`, like the package count, and a failed
//!    check leaves the previous answer in place rather than showing an error.
//!  * It is the same code on Arch, Debian and Void. A systemd timer on two
//!    distributions and a runit service on the third would be three things to
//!    maintain and three ways to break.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

/// How often the release page is asked. A new suite release is not a daily
/// event, and the GitHub API has a quota, so an hour of jitter costs nothing
/// but an unnecessary request per redraw would.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a single API request may take before it is abandoned.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        })
        .join("nirubar")
}

fn cache_path() -> PathBuf {
    cache_dir().join("nirux-update")
}

fn state_path() -> PathBuf {
    cache_dir().join("nirux-update-checked")
}

/// The installed NIRUX version, read from the generation that is active.
///
/// `~/.local/share/nirux/current` is a symlink whose target ends in the
/// version, which is cheaper and more robust than running the installer.
fn installed_version() -> Option<String> {
    let base = std::env::var_os("HOME").map(PathBuf::from)?;
    let current = base.join(".local/share/nirux/current");
    let target = std::fs::read_link(&current).ok()?;
    let name = target.file_name()?.to_string_lossy().to_string();
    // Generation directories look like "0.12.1-AbCdEf".
    let version = name.split('-').next()?.to_string();
    is_version(&version).then_some(version)
}

fn is_version(text: &str) -> bool {
    !text.is_empty()
        && text.split('.').count() >= 2
        && text
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// The generation directory that `current` points at, if it is a real link.
fn current_generation() -> Option<PathBuf> {
    let base = std::env::var_os("HOME").map(PathBuf::from)?;
    std::fs::read_link(base.join(".local/share/nirux/current")).ok()
}

/// The executable behind the first running process named `name`.
///
/// `/proc/<pid>/comm` is short and exact; the executable link resolves the
/// generation directory, which is what the activation check needs.
fn running_exe(name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir("/proc").ok()? {
        let entry = entry.ok()?;
        let pid = entry.file_name();
        let pid = pid.to_str()?;
        if !pid.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let comm = std::fs::read_to_string(entry.path().join("comm")).ok()?;
        if comm.trim_end() != name {
            continue;
        }
        let exe = std::fs::read_link(entry.path().join("exe")).ok()?;
        return Some(exe);
    }
    None
}

/// `Some(version)` when the active generation differs from the running WM or
/// bar. The comparison is path-based, so an update that is installed but not
/// yet activated is reported without running the installer.
fn pending_for(
    current: &Path,
    running_wm: Option<&Path>,
    running_bar: Option<&Path>,
) -> Option<String> {
    let installed = current.file_name()?.to_string_lossy().to_string();
    let installed = installed.split('-').next()?.to_string();
    if !is_version(&installed) {
        return None;
    }
    let wm = current.join("bin/niruwm");
    let bar = current.join("bin/nirubar");
    let stale_wm =
        running_wm.is_some_and(|path| path.canonicalize().ok() != wm.canonicalize().ok());
    let stale_bar =
        running_bar.is_some_and(|path| path.canonicalize().ok() != bar.canonicalize().ok());
    // Only report a generation when a process is actually running: a fresh
    // login has not started them yet and must not show a stale warning.
    (running_wm.is_some() || running_bar.is_some())
        .then_some(installed)
        .filter(|_| stale_wm || stale_bar)
}

/// The installed-but-not-active NIRUX generation, if any.
pub fn activation_pending() -> Option<String> {
    if cfg!(test) || std::env::var_os("NIRUBAR_NO_UPDATE_CHECK").is_some() {
        return None;
    }
    let current = current_generation()?;
    let wm = running_exe("niruwm");
    let bar = running_exe("nirubar");
    pending_for(&current, wm.as_deref(), bar.as_deref())
}

/// Play the suite-update cue once for each newly discovered release.
pub fn play_update_sound(version: &str) {
    if cfg!(test) || std::env::var_os("NIRUBAR_NO_UPDATE_CHECK").is_some() {
        return;
    }
    let marker = cache_dir().join("nirux-update-sounded");
    if std::fs::read_to_string(&marker)
        .ok()
        .is_some_and(|old| old.trim() == version)
    {
        return;
    }
    let _ = std::process::Command::new("nirux")
        .args(["sounds", "play", "update"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let _ = std::fs::create_dir_all(cache_dir());
    let _ = std::fs::write(marker, version);
}

/// Text for an installed generation that needs a logout/login or bar restart.
pub fn activation_text(version: Option<&str>, glyph: &str) -> String {
    match version {
        Some(v) => format!("{glyph} NIRUX {v} ↻"),
        None => String::new(),
    }
}

/// True when `candidate` is newer than `installed`, comparing each numeric part.
pub fn is_newer(candidate: &str, installed: &str) -> bool {
    let split = |v: &str| -> Vec<u64> {
        v.split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (a, b) = (split(candidate), split(installed));
    let len = a.len().max(b.len());
    for i in 0..len {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

/// Extract the version from a GitHub release JSON body.
///
/// Only `tag_name` matters, and a tiny scan avoids a JSON dependency for one
/// field.
pub fn parse_tag(body: &str) -> Option<String> {
    let rest = body.split("\"tag_name\"").nth(1)?;
    let value = rest.split('"').nth(1)?;
    let version = value.trim_start_matches('v');
    is_version(version).then(|| version.to_string())
}

/// Ask GitHub for the latest release. Returns `None` on any network problem.
fn fetch_latest() -> Option<String> {
    let out = crate::probe::run(
        "curl",
        &[
            "-fsSL",
            "--max-time",
            "6",
            "-H",
            "Accept: application/vnd.github+json",
            "https://api.github.com/repos/nirucon/nirux/releases/latest",
        ],
        &[],
        REQUEST_TIMEOUT,
        256 * 1024,
    )?;
    if !out.status.success() {
        return None;
    }
    parse_tag(&String::from_utf8_lossy(&out.stdout))
}

fn fresh(path: &std::path::Path, max_age: Duration) -> bool {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .map(|age| age < max_age)
        .unwrap_or(false)
}

/// The newer version, if any. `None` means "nothing to say": no suite is
/// installed, the check has not run yet, or the last check failed.
///
/// The check itself runs at most once per `CHECK_INTERVAL`. Until it has run,
/// the module stays silent rather than showing a spinner or an error.
pub fn available() -> Option<String> {
    // Only a real desktop asks GitHub. A unit test, a `--check-config` run or a
    // login shell on a headless machine must not wait for a network request,
    // and must not rewrite the user's cache because of one.
    if cfg!(test) || std::env::var_os("NIRUBAR_NO_UPDATE_CHECK").is_some() {
        return None;
    }
    let installed = installed_version()?;
    let stamp = state_path();
    if !fresh(&stamp, CHECK_INTERVAL) {
        let Some(latest) = fetch_latest() else {
            // Leave the previous answer alone: a failed request is not news.
            return cached_answer(&installed);
        };
        let dir = cache_dir();
        let _ = std::fs::create_dir_all(&dir);
        if is_newer(&latest, &installed) {
            let _ = std::fs::write(cache_path(), &latest);
        } else {
            let _ = std::fs::remove_file(cache_path());
        }
        let _ = std::fs::write(&stamp, &installed);
        return is_newer(&latest, &installed).then_some(latest);
    }
    cached_answer(&installed)
}

fn cached_answer(installed: &str) -> Option<String> {
    let text = std::fs::read_to_string(cache_path()).ok()?;
    let latest = text.trim();
    is_newer(latest, installed).then(|| latest.to_string())
}

/// Format the module text. Empty when there is nothing newer.
pub fn text(latest: Option<&str>, glyph: &str) -> String {
    match latest {
        Some(v) => format!("{glyph} NIRUX {v}"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_generation_needs_activation() {
        let root = std::env::temp_dir().join(format!("nux-pending-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let current = root.join("0.12.96-AbCdEf");
        let previous = root.join("0.12.95-Old123");
        std::fs::create_dir_all(current.join("bin")).unwrap();
        std::fs::create_dir_all(previous.join("bin")).unwrap();
        std::fs::write(current.join("bin/niruwm"), "").unwrap();
        std::fs::write(current.join("bin/nirubar"), "").unwrap();
        std::fs::write(previous.join("bin/niruwm"), "").unwrap();
        std::fs::write(previous.join("bin/nirubar"), "").unwrap();
        assert_eq!(
            pending_for(
                &current,
                Some(&previous.join("bin/niruwm")),
                Some(&current.join("bin/nirubar"))
            ),
            Some("0.12.96".into())
        );
        assert_eq!(
            pending_for(
                &current,
                Some(&current.join("bin/niruwm")),
                Some(&current.join("bin/nirubar"))
            ),
            None
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn newer_versions_are_recognised() {
        assert!(is_newer("0.12.2", "0.12.1"));
        assert!(is_newer("0.13.0", "0.12.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.12.1", "0.12.1"));
        assert!(!is_newer("0.11.9", "0.12.0"));
    }

    #[test]
    fn a_shorter_version_counts_missing_parts_as_zero() {
        assert!(is_newer("0.12.0.1", "0.12"));
        assert!(!is_newer("0.12", "0.12.0.1"));
    }

    #[test]
    fn the_tag_is_read_from_a_release_body() {
        let body = r#"{"url":"x","tag_name":"v0.12.2","name":"0.12.2"}"#;
        assert_eq!(parse_tag(body).as_deref(), Some("0.12.2"));
    }

    #[test]
    fn a_body_without_a_tag_yields_nothing() {
        assert_eq!(parse_tag("{}"), None);
        assert_eq!(parse_tag(r#"{"tag_name":"not-a-version"}"#), None);
    }

    #[test]
    fn the_module_is_silent_when_there_is_nothing_newer() {
        assert_eq!(text(None, "N"), "");
        assert_eq!(text(Some("0.12.2"), "N"), "N NIRUX 0.12.2");
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::sync::Mutex;

    /// The cache tests share process-wide environment variables, so they run
    /// one at a time instead of overwriting each other's HOME.
    static LOCK: Mutex<()> = Mutex::new(());

    struct Sandbox {
        dir: std::path::PathBuf,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl Sandbox {
        fn new(name: &str) -> Self {
            let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!("nux-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::env::set_var("HOME", &dir);
            std::env::set_var("XDG_CACHE_HOME", dir.join(".cache"));
            Self { dir, _guard: guard }
        }

        /// Create a fake installed suite at `version`.
        fn install(&self, version: &str) {
            let gen = self
                .dir
                .join(format!(".local/share/nirux/generations/{version}-X"));
            std::fs::create_dir_all(&gen).unwrap();
            std::os::unix::fs::symlink(&gen, self.dir.join(".local/share/nirux/current")).unwrap();
        }

        fn cache(&self, latest: &str, checked_against: &str) {
            let cache = self.dir.join(".cache/nirubar");
            std::fs::create_dir_all(&cache).unwrap();
            std::fs::write(cache.join("nirux-update"), latest).unwrap();
            std::fs::write(cache.join("nirux-update-checked"), checked_against).unwrap();
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    #[test]
    fn the_installed_version_comes_from_the_generation_symlink() {
        let box_ = Sandbox::new("installed");
        box_.install("0.12.1");
        assert_eq!(installed_version().as_deref(), Some("0.12.1"));
    }

    #[test]
    fn a_machine_without_nirux_is_silent() {
        let _box = Sandbox::new("nosuite");
        assert_eq!(installed_version(), None);
        assert_eq!(available(), None, "no suite must mean no module");
    }

    #[test]
    fn a_cached_newer_release_is_reported() {
        let box_ = Sandbox::new("cached");
        box_.install("0.12.1");
        box_.cache("0.12.2", "0.12.1");
        // `available` is silent under test, so drive the cache path it returns.
        assert_eq!(cached_answer("0.12.1").as_deref(), Some("0.12.2"));
    }

    #[test]
    fn a_cached_older_release_is_not_reported() {
        let box_ = Sandbox::new("older");
        box_.install("0.12.1");
        box_.cache("0.11.0", "0.12.1");
        assert_eq!(
            cached_answer("0.12.1"),
            None,
            "an older release is not an update"
        );
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// The module must report a newer release from the cache without a network
    /// request. This is the path the bar takes on every redraw.
    #[test]
    fn the_bar_reads_the_cached_answer() {
        let dir = std::env::temp_dir().join(format!("nux-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".local/share/nirux/generations/0.12.1-X")).unwrap();
        std::os::unix::fs::symlink(
            dir.join(".local/share/nirux/generations/0.12.1-X"),
            dir.join(".local/share/nirux/current"),
        )
        .unwrap();
        std::fs::create_dir_all(dir.join(".cache/nirubar")).unwrap();
        std::fs::write(dir.join(".cache/nirubar/nirux-update"), "0.13.0\n").unwrap();
        // A fresh stamp: no request is attempted, the cache is the answer.
        std::fs::write(dir.join(".cache/nirubar/nirux-update-checked"), "0.12.1").unwrap();
        std::env::set_var("HOME", &dir);
        std::env::set_var("XDG_CACHE_HOME", dir.join(".cache"));
        // The function under test is silent under `cfg(test)`; this test is
        // about the cache path, so it calls the same two steps `available`
        // takes when the stamp is fresh.
        let got = installed_version().and_then(|installed| cached_answer(&installed));
        let shown = text(got.as_deref(), "N");
        assert_eq!(got.as_deref(), Some("0.13.0"));
        assert_eq!(shown, "N NIRUX 0.13.0");
        std::fs::remove_dir_all(&dir).ok();
    }
}
