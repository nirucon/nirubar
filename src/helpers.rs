//! Bounded short-lived helpers for media and audio controls.
//!
//! A scroll wheel can produce events faster than `wpctl` or `playerctl` exits.
//! The pool coalesces a control that is already running and caps the number of
//! concurrent children, so the bar cannot be used to create a process storm.

use std::{
    collections::HashMap,
    process::{Child, Command},
};

pub(crate) struct HelperPool {
    running: HashMap<String, Child>,
    limit: usize,
}

impl HelperPool {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            running: HashMap::new(),
            limit,
        }
    }

    pub(crate) fn spawn(&mut self, key: &str, program: &str, args: &[String]) -> bool {
        self.reap();
        if self.running.contains_key(key) || self.running.len() >= self.limit {
            return false;
        }
        match Command::new(program).args(args).spawn() {
            Ok(child) => {
                self.running.insert(key.to_string(), child);
                true
            }
            Err(e) => {
                eprintln!("nirubar: cannot start {program}: {e}");
                false
            }
        }
    }

    pub(crate) fn reap(&mut self) {
        self.running
            .retain(|_, child| !matches!(child.try_wait(), Ok(Some(_))));
    }
}

#[cfg(test)]
mod tests {
    use super::HelperPool;

    fn sleeper() -> Vec<String> {
        vec!["-c".into(), "sleep 0.2".into()]
    }

    #[test]
    fn coalesces_the_same_key_and_bounds_children() {
        let mut pool = HelperPool::new(2);
        assert!(pool.spawn("volume", "sh", &sleeper()));
        assert!(
            !pool.spawn("volume", "sh", &sleeper()),
            "same key coalesced"
        );
        assert!(pool.spawn("media", "sh", &sleeper()));
        assert!(!pool.spawn("mic", "sh", &sleeper()), "limit is bounded");
        std::thread::sleep(std::time::Duration::from_millis(350));
        assert!(
            pool.spawn("mic", "sh", &sleeper()),
            "finished slots are reused"
        );
    }
}
