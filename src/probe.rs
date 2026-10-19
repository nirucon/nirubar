//! Bounded command execution for information probes.
//!
//! A bar must never wait indefinitely for `wpctl`, `playerctl`, `ip` or a
//! package manager. Each call has a deadline and a stored-output cap; excess
//! output is drained and discarded so the child cannot block on a full pipe.

use std::{
    io::Read,
    os::unix::process::CommandExt,
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

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
