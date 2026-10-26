//! Bounded short-lived helpers for bar controls.
//!
//! A scroll wheel can produce events faster than `wpctl` or `playerctl` exits.
//! The pool coalesces a control that is already running, caps the number of
//! concurrent children, and reports a failed exit or a stuck helper instead of
//! letting the bar claim success just because the process was spawned.

use std::{
    collections::HashMap,
    process::{Child, Command},
    time::{Duration, Instant},
};

struct Running {
    child: Child,
    started: Instant,
}

pub(crate) struct HelperPool {
    running: HashMap<String, Running>,
    limit: usize,
    timeout: Duration,
}

impl HelperPool {
    pub(crate) fn new(limit: usize) -> Self {
        Self::with_timeout(limit, Duration::from_secs(10))
    }

    fn with_timeout(limit: usize, timeout: Duration) -> Self {
        Self {
            running: HashMap::new(),
            limit,
            timeout,
        }
    }

    /// Start a control. `Ok(())` also covers a request that is already running.
    pub(crate) fn spawn(
        &mut self,
        key: &str,
        program: &str,
        args: &[String],
    ) -> Result<(), String> {
        self.reap();
        if self.running.contains_key(key) {
            return Ok(());
        }
        if self.running.len() >= self.limit {
            return Err("too many bar controls are already running".into());
        }
        match Command::new(program).args(args).spawn() {
            Ok(child) => {
                self.running.insert(
                    key.to_string(),
                    Running {
                        child,
                        started: Instant::now(),
                    },
                );
                Ok(())
            }
            Err(e) => Err(format!("cannot start {program}: {e}")),
        }
    }

    /// Reap finished helpers and return readable failures.
    pub(crate) fn reap(&mut self) -> Vec<String> {
        let mut failures = Vec::new();
        let timeout = self.timeout;
        self.running
            .retain(|key, running| match running.child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        failures.push(format!("{key} exited with {status}"));
                    }
                    false
                }
                Ok(None) if running.started.elapsed() >= timeout => {
                    let _ = running.child.kill();
                    let _ = running.child.wait();
                    failures.push(format!("{key} timed out"));
                    false
                }
                Ok(None) => true,
                Err(e) => {
                    failures.push(format!("{key}: {e}"));
                    false
                }
            });
        failures
    }
}

/// Tell the user that a control did not complete. Best effort: a session
/// without a notification daemon must not turn this into another failure.
pub(crate) fn notify_failure(detail: &str) {
    let _ = Command::new("notify-send")
        .args(["-a", "NIRUX", "-t", "4000", "Bar control", detail])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::HelperPool;
    use std::time::Duration;

    fn sleeper(seconds: &str) -> Vec<String> {
        vec!["-c".into(), format!("sleep {seconds}")]
    }

    #[test]
    fn coalesces_the_same_key_and_bounds_children() {
        let mut pool = HelperPool::with_timeout(2, Duration::from_secs(2));
        assert!(pool.spawn("volume", "sh", &sleeper("0.2")).is_ok());
        assert!(
            pool.spawn("volume", "sh", &sleeper("0.2")).is_ok(),
            "same key coalesced"
        );
        assert!(pool.spawn("media", "sh", &sleeper("0.2")).is_ok());
        assert!(pool.spawn("mic", "sh", &sleeper("0.2")).is_err());
        std::thread::sleep(std::time::Duration::from_millis(350));
        assert!(
            pool.spawn("mic", "sh", &sleeper("0.2")).is_ok(),
            "finished slots are reused"
        );
    }

    #[test]
    fn failed_exit_is_reported() {
        let mut pool = HelperPool::with_timeout(2, Duration::from_secs(2));
        pool.spawn("fail", "sh", &["-c".into(), "exit 3".into()])
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let failures = pool.reap();
        assert!(
            failures.iter().any(|failure| failure.contains("exited")),
            "{failures:?}"
        );
    }

    #[test]
    fn stuck_helper_is_killed_and_reported() {
        let mut pool = HelperPool::with_timeout(1, Duration::from_millis(50));
        pool.spawn("stuck", "sh", &sleeper("1")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(120));
        let failures = pool.reap();
        assert!(
            failures.iter().any(|failure| failure.contains("timed out")),
            "{failures:?}"
        );
    }
}
