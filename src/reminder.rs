//! Calendar reminders: a notification a few minutes before a timed event.
//!
//! The calendar decides what is due (see `Calendar::remind`); this module
//! remembers what was already sent and shows the notification.
//!
//! What was sent lives in `~/.local/state/nirubar/reminders.json` (mode
//! 0600) as hashes of the feed, the event UID and the start time, never as
//! event text. One bar per monitor shares the file: whoever claims a
//! reminder under the file lock sends it, so it appears once, and a
//! restarted bar does not send it again.
//!
//! The notification goes straight to the session's notification service over
//! D-Bus (NIRUNOTIFY, or any freedesktop notification daemon), so the event
//! title is never on a command line where other users could read it. When the
//! calendar has a page to open, the notification's default action (a click)
//! opens that day in the browser.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

/// How long sent reminders are remembered after their event started.
const KEEP_SECS: i64 = 3 * 24 * 3600;

/// How long a notification with an action waits for a click.
const ACTION_WAIT: Duration = Duration::from_secs(2 * 3600);

#[derive(Default, Serialize, Deserialize)]
struct State {
    schema: u32,
    /// Reminder key -> event start (UTC seconds), for pruning.
    sent: BTreeMap<String, i64>,
}

pub fn state_path() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
        })
        .join("nirubar")
        .join("reminders.json")
}

/// FNV-1a over the parts, so the file holds no event text.
pub fn key(parts: &[&str]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.bytes().chain(std::iter::once(0x1f)) {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// Mark `wanted` (key, start) as sent and return the ones that were not sent
/// before. Every bar calls this; the file lock makes the answer exclusive.
/// An unreadable or unwritable state file sends nothing: a missed reminder is
/// better than one repeated every twenty seconds.
pub fn claim(path: &Path, wanted: &[(String, i64)], now: i64) -> Vec<String> {
    if wanted.is_empty() {
        return Vec::new();
    }
    match claim_in(path, wanted, now) {
        Ok(fresh) => fresh,
        Err(e) => {
            eprintln!("nirubar: calendar reminders: state file: {}", e.kind());
            Vec::new()
        }
    }
}

fn claim_in(path: &Path, wanted: &[(String, i64)], now: i64) -> std::io::Result<Vec<String>> {
    use std::os::unix::io::AsRawFd;
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    // SAFETY: flock on an open descriptor; released when `file` is closed.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let mut state: State = if text.trim().is_empty() {
        State::default()
    } else {
        // A damaged file is replaced; the reminders it held may repeat once.
        serde_json::from_str(&text).unwrap_or_default()
    };
    state.schema = 1;
    let before = state.sent.len();
    state.sent.retain(|_, start| *start + KEEP_SECS > now);
    let pruned = state.sent.len() != before;
    let mut fresh = Vec::new();
    for (k, start) in wanted {
        if !state.sent.contains_key(k) {
            state.sent.insert(k.clone(), *start);
            fresh.push(k.clone());
        }
    }
    if !fresh.is_empty() || pruned {
        let bytes = serde_json::to_vec(&state).map_err(std::io::Error::other)?;
        file.set_len(0)?;
        file.rewind()?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    Ok(fresh)
}

/// One notification to show.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub summary: String,
    pub body: String,
    /// The calendar page for the event's day, opened on a click.
    pub open: Option<String>,
}

/// Show the notification on a short-lived thread: the bar's draw loop never
/// waits for D-Bus. `browser` is the configured browser command (empty means
/// `xdg-open`).
pub fn send(notice: Notice, browser: Vec<String>) {
    let spawned = std::thread::Builder::new()
        .name("calendar-reminder".into())
        .spawn(move || {
            if let Err(e) = notify(&notice, &browser) {
                // The error text is about D-Bus, never about the event.
                eprintln!("nirubar: calendar reminder could not be shown: {e}");
            }
        });
    if let Err(e) = spawned {
        eprintln!("nirubar: calendar reminder thread: {e}");
    }
}

fn notify(notice: &Notice, browser: &[String]) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        use futures_lite::StreamExt;
        let connection = tokio::time::timeout(Duration::from_secs(3), zbus::Connection::session())
            .await
            .map_err(|_| "no session bus".to_string())?
            .map_err(|e| e.to_string())?;
        let proxy = zbus::Proxy::new(
            &connection,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
        )
        .await
        .map_err(|e| e.to_string())?;
        // Listen before the notification exists, so a fast click is not lost.
        let mut actions = match notice.open {
            Some(_) => Some(
                proxy
                    .receive_signal("ActionInvoked")
                    .await
                    .map_err(|e| e.to_string())?,
            ),
            None => None,
        };
        let mut closed = match notice.open {
            Some(_) => Some(
                proxy
                    .receive_signal("NotificationClosed")
                    .await
                    .map_err(|e| e.to_string())?,
            ),
            None => None,
        };
        let action_list: Vec<&str> = if notice.open.is_some() {
            vec!["default", "Open calendar"]
        } else {
            Vec::new()
        };
        let mut hints: std::collections::HashMap<&str, zbus::zvariant::Value<'_>> =
            std::collections::HashMap::new();
        hints.insert("urgency", zbus::zvariant::Value::U8(1));
        hints.insert("category", zbus::zvariant::Value::from("x-nirux.calendar"));
        let reply = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call_method(
                "Notify",
                &(
                    "Calendar",
                    0u32,
                    "x-office-calendar",
                    notice.summary.as_str(),
                    notice.body.as_str(),
                    action_list,
                    hints,
                    // 0: until dismissed (NIRUNOTIFY caps it at its longest
                    // timeout), so a reminder is not missed in a glance away.
                    0i32,
                ),
            ),
        )
        .await
        .map_err(|_| "the notification service did not answer".to_string())?
        .map_err(|e| e.to_string())?;
        let id: u32 = reply.body().deserialize().map_err(|e| e.to_string())?;
        let (Some(url), Some(actions), Some(closed)) =
            (&notice.open, actions.as_mut(), closed.as_mut())
        else {
            return Ok(());
        };
        let wait = async {
            loop {
                tokio::select! {
                    Some(message) = actions.next() => {
                        if let Ok((got, key)) = message.body().deserialize::<(u32, String)>() {
                            if got == id {
                                return key == "default";
                            }
                        }
                    }
                    Some(message) = closed.next() => {
                        if let Ok((got, _reason)) = message.body().deserialize::<(u32, u32)>() {
                            if got == id {
                                return false;
                            }
                        }
                    }
                    else => return false,
                }
            }
        };
        if tokio::time::timeout(ACTION_WAIT, wait).await == Ok(true) {
            open(url, browser);
        }
        Ok(())
    })
}

/// Open a calendar page the same way a click in the clock's panel does.
pub fn open(url: &str, browser: &[String]) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    let mut command = std::process::Command::new("sh");
    command.args(["-c", "\"$@\" </dev/null >/dev/null 2>&1 &", "nirubar-open"]);
    if browser.is_empty() {
        command.arg("xdg-open");
    } else {
        command.args(browser);
    }
    command.arg(url);
    if let Ok(mut child) = command.spawn() {
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("nirubar-remind-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("reminders.json")
    }

    #[test]
    fn a_reminder_is_claimed_once_across_bars_and_restarts() {
        let path = temp("once");
        let wanted = vec![("a".to_string(), 1000), ("b".to_string(), 2000)];
        assert_eq!(claim(&path, &wanted, 900), vec!["a", "b"]);
        // A second bar, or the same bar after a restart: nothing new.
        assert!(claim(&path, &wanted, 950).is_empty());
        let more = vec![("b".to_string(), 2000), ("c".to_string(), 3000)];
        assert_eq!(claim(&path, &more, 960), vec!["c"]);
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn old_entries_are_pruned_and_a_damaged_file_is_replaced() {
        let path = temp("prune");
        assert_eq!(claim(&path, &[("old".into(), 0)], 10), vec!["old"]);
        // Long after the event the key is forgotten (it cannot come again).
        let later = KEEP_SECS + 100;
        assert_eq!(claim(&path, &[("new".into(), later)], later), vec!["new"]);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("\"old\""), "{text}");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(claim(&path, &[("x".into(), later)], later), vec!["x"]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn keys_hide_the_event_and_differ_per_occurrence() {
        let a = key(&["feed", "uid-1", "1000"]);
        let b = key(&["feed", "uid-1", "2000"]);
        assert_ne!(a, b);
        assert_eq!(a, key(&["feed", "uid-1", "1000"]));
        assert_eq!(a.len(), 16);
        assert_ne!(key(&["ab", "c"]), key(&["a", "bc"]));
    }
}
