//! Bounded command execution for information probes.
//!
//! A bar must never wait indefinitely for `wpctl`, `playerctl`, `ip` or a
//! package manager. Each call has a deadline and a stored-output cap; excess
//! output is drained and discarded so the child cannot block on a full pipe.

use std::{
    collections::HashMap,
    io::Read,
    os::unix::process::CommandExt,
    process::{Command, ExitStatus, Stdio},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

fn read_capped(mut pipe: impl Read, cap: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = cap.saturating_sub(kept.len());
                if room > 0 {
                    kept.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
        }
    }
    kept
}

/// Run a program with a deadline and bounded stdout/stderr.
#[derive(Default)]
struct AsyncEntry {
    running: bool,
    checked: Option<Instant>,
    last: Option<Output>,
}

fn async_cache() -> &'static Mutex<HashMap<String, AsyncEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, AsyncEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Return the most recent sample immediately and refresh it in the background.
///
/// The event loop must never wait for an external program. Input is processed
/// at mouse/keyboard speed; a slow `curl`, `playerctl`, `iw` or `xrandr` only
/// delays its own next sample, never the bar's event loop. The first call
/// returns `None`; the worker publishes a result for the next frame.
pub fn run_async(
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
    timeout: Duration,
    cap: usize,
) -> Option<Output> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    let envs: Vec<(String, String)> = envs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    let key = format!("{}|{:?}|{:?}|{}", program, args, envs, cap);
    let now = Instant::now();
    let mut cache = async_cache().lock().ok()?;
    let entry = cache.entry(key.clone()).or_default();
    let should_refresh = !entry.running
        && entry
            .checked
            .map(|checked| now.duration_since(checked) >= Duration::from_secs(1))
            .unwrap_or(true);
    if should_refresh {
        entry.running = true;
        let worker_key = key.clone();
        let program = program.to_string();
        let args_for_worker = args.clone();
        let envs_for_worker = envs.clone();
        drop(cache);
        let spawned = thread::Builder::new()
            .name("nirubar-probe".into())
            .spawn(move || {
                let arg_refs: Vec<&str> = args_for_worker.iter().map(String::as_str).collect();
                let env_refs: Vec<(&str, &str)> = envs_for_worker
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str()))
                    .collect();
                let result = run(&program, &arg_refs, &env_refs, timeout, cap);
                if let Ok(mut cache) = async_cache().lock() {
                    if let Some(entry) = cache.get_mut(&worker_key) {
                        entry.last = result;
                        entry.checked = Some(Instant::now());
                        entry.running = false;
                    }
                }
            })
            .is_ok();
        if !spawned {
            if let Ok(mut cache) = async_cache().lock() {
                if let Some(entry) = cache.get_mut(&key) {
                    entry.running = false;
                }
            }
        }
        return None;
    }
    entry.last.clone()
}

pub fn run(
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
    timeout: Duration,
    cap: usize,
) -> Option<Output> {
    let mut child = Command::new(program)
        .args(args)
        .envs(envs.iter().copied())
        // Own process group: killing the leader also stops a shell's children
        // that would otherwise keep the output pipe open.
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let out = thread::spawn(move || read_capped(stdout, cap));
    let err = thread::spawn(move || read_capped(stderr, cap));
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                timed_out = true;
                // SAFETY: the child was placed in its own process group; a
                // negative pid addresses exactly that group.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                break child.wait().ok()?;
            }
            Err(_) => return None,
        }
    };
    let stdout = out.join().ok()?;
    let stderr = err.join().ok()?;
    if timed_out {
        return None;
    }
    Some(Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hung_probe_is_killed_at_its_deadline() {
        let start = Instant::now();
        let out = run(
            "sh",
            &["-c", "sleep 5"],
            &[],
            Duration::from_millis(100),
            1024,
        );
        assert!(
            out.is_none(),
            "a signal-terminated child is not a usable sample"
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn output_is_capped_but_the_child_can_finish() {
        let out = run(
            "sh",
            &[
                "-c",
                "i=0; while [ $i -lt 100 ]; do echo 0123456789; i=$((i+1)); done",
            ],
            &[],
            Duration::from_secs(2),
            256,
        )
        .unwrap();
        assert_eq!(out.stdout.len(), 256);
    }

    #[test]
    fn environment_and_exit_status_are_available() {
        let out = run(
            "sh",
            &["-c", "printf %s \"$PROBE\"; exit 3"],
            &[("PROBE", "ok")],
            Duration::from_secs(1),
            64,
        )
        .unwrap();
        assert_eq!(out.stdout, b"ok");
        assert_eq!(out.status.code(), Some(3));
    }
}
