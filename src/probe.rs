//! Bounded command execution for information probes.
//!
//! A bar must never wait indefinitely for `wpctl`, `playerctl`, `ip` or a
//! package manager. Each call has a deadline and a stored-output cap; excess
//! output is drained and discarded so the child cannot block on a full pipe.

use std::{
    io::{ErrorKind, Read},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

extern "C" {
    fn poll(fds: *mut PollFd, nfds: u64, timeout: i32) -> i32;
}

const POLLIN: i16 = 1;

fn nonblocking(fd: i32) -> bool {
    // SAFETY: fd is an open pipe owned by the child until it is dropped.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == 0
    }
}

fn read_available(pipe: &mut impl Read, kept: &mut Vec<u8>, cap: usize) -> (bool, bool) {
    let mut chunk = [0u8; 4096];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => return (true, false),
            Ok(n) => {
                let room = cap.saturating_sub(kept.len());
                if room > 0 {
                    kept.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => return (false, false),
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return (true, true),
        }
    }
}

fn stop_group(child: &mut Child) {
    // SAFETY: the child was placed in its own process group. A negative pid
    // addresses exactly that group, including descendants that inherited the
    // probe's output pipes.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
}

/// Run a program with one deadline for the child and its output pipes.
///
/// The previous implementation joined reader threads without a deadline after
/// the leader exited. A descendant that inherited stdout/stderr could therefore
/// keep the probe alive far past its timeout. Readers here are nonblocking and
/// are drained only while the shared deadline remains.
pub fn run(
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
    timeout: Duration,
    cap: usize,
) -> Option<Output> {
    run_inner(program, args, envs, timeout, cap, None)
}

/// Like `run`, with `input` on the child's standard input. Secrets such as a
/// calendar URL go this way, so they never appear in a process list.
pub fn run_with_input(
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
    timeout: Duration,
    cap: usize,
    input: &[u8],
) -> Option<Output> {
    run_inner(program, args, envs, timeout, cap, Some(input))
}

fn run_inner(
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
    timeout: Duration,
    cap: usize,
    input: Option<&[u8]>,
) -> Option<Output> {
    let mut child = Command::new(program)
        .args(args)
        .envs(envs.iter().copied())
        .process_group(0)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        use std::io::Write;
        // Inputs are small (well under a pipe buffer), so this cannot block
        // on a child that does not read. Dropping closes the pipe.
        if input.len() > 16 * 1024 || stdin.write_all(input).is_err() {
            stop_group(&mut child);
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    }
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let stdout_fd = stdout.as_raw_fd();
    let stderr_fd = stderr.as_raw_fd();
    if !nonblocking(stdout_fd) || !nonblocking(stderr_fd) {
        stop_group(&mut child);
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut status = None;
    let mut stdout_open = true;
    let mut stderr_open = true;
    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(exit)) => {
                    status = Some(exit);
                    // A probe is finished when its leader finishes. Do not let
                    // inherited pipes keep the sample alive.
                    stop_group(&mut child);
                }
                Ok(None) => {}
                Err(_) => return None,
            }
        }
        let mut fds = [
            PollFd {
                fd: if stdout_open { stdout_fd } else { -1 },
                events: POLLIN,
                revents: 0,
            },
            PollFd {
                fd: if stderr_open { stderr_fd } else { -1 },
                events: POLLIN,
                revents: 0,
            },
        ];
        let remaining = deadline.saturating_duration_since(Instant::now());
        if status.is_some() && !stdout_open && !stderr_open {
            break;
        }
        if remaining.is_zero() {
            stop_group(&mut child);
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        let wait_ms = remaining.min(Duration::from_millis(20)).as_millis() as i32;
        // SAFETY: two valid pollfd entries, and fd=-1 entries are ignored.
        unsafe {
            poll(fds.as_mut_ptr(), 2, wait_ms);
        }
        if stdout_open {
            let (eof, _) = read_available(&mut stdout, &mut out, cap);
            stdout_open = !eof;
        }
        if stderr_open {
            let (eof, _) = read_available(&mut stderr, &mut err, cap);
            stderr_open = !eof;
        }
    }
    let status = status?;
    let _ = child.wait();
    Some(Output {
        status,
        stdout: out,
        stderr: err,
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
    fn a_descendant_holding_the_pipe_cannot_defeat_the_deadline() {
        let start = Instant::now();
        let out = run(
            "sh",
            &["-c", "sleep 5 & exit 0"],
            &[],
            Duration::from_millis(100),
            1024,
        );
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "the probe ended after its end-to-end deadline: {:?}",
            start.elapsed()
        );
        assert!(
            out.is_some(),
            "the leader completed, so its output can be sampled after its group closes"
        );
    }

    #[test]
    fn an_escaped_descendant_is_cut_off_at_the_shared_deadline() {
        let marker =
            std::env::temp_dir().join(format!("nirubar-probe-escaped-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let marker_text = marker.to_string_lossy().to_string();
        let start = Instant::now();
        let out = run(
            "sh",
            &["-c", "setsid sh -c 'printf %s $$ > \"$PROBE_READY\"; exec sleep 2' & while [ ! -s \"$PROBE_READY\" ]; do sleep 0.005; done"],
            &[("PROBE_READY", &marker_text)],
            Duration::from_millis(300),
            1024,
        );
        let pid: i32 = std::fs::read_to_string(&marker)
            .expect("escaped child did not signal readiness")
            .parse()
            .unwrap();
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        let _ = std::fs::remove_file(marker);
        assert!(out.is_none());
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "escaped pipe holder defeated the deadline: {:?}",
            start.elapsed()
        );
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
    fn input_reaches_the_child_on_stdin() {
        let out = run_with_input(
            "sh",
            &["-c", "cat"],
            &[],
            Duration::from_secs(2),
            64,
            b"secret stays off argv",
        )
        .unwrap();
        assert_eq!(out.stdout, b"secret stays off argv");
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
