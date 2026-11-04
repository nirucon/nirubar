//! Output-volume snapshots driven by Pulse/PipeWire events when available.
//!
//! A single supervised `pactl subscribe` wakes the worker on sink/server
//! changes. The existing bounded wpctl/ALSA query remains the fallback.

use std::{
    io::Read,
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::av::Volume;

fn pactl_path() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("pactl"))
            .find(|candidate| candidate.is_file())
    })
}

fn affects_output(line: &str) -> bool {
    line.contains(" on sink #") || line.contains(" on server #")
}

fn update(state: &Mutex<Option<Volume>>) {
    let value = Volume::read();
    if let Ok(mut current) = state.lock() {
        if *current != value {
            if std::env::var_os("NIRUBAR_TRACE_AUDIO").is_some() {
                let time = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_micros();
                eprintln!(
                    "nirubar: audio volume={} time_us={time}",
                    value
                        .as_ref()
                        .map(|v| v.percent.to_string())
                        .unwrap_or_else(|| "unavailable".into())
                );
            }
            *current = value;
            drop(current);
            crate::wake_update();
        }
    }
}

fn spawn_subscriber(path: &PathBuf) -> std::io::Result<Child> {
    let mut cmd = Command::new(path);
    let parent = std::process::id();
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() as u32 != parent {
                return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
            }
            Ok(())
        });
    }
    cmd.arg("subscribe")
        .env("LC_ALL", "C")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

fn stop_child(child: &mut Child) {
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
}

pub(crate) struct Monitor {
    state: Arc<Mutex<Option<Volume>>>,
    child: Arc<Mutex<Option<Child>>>,
    stop: Arc<AtomicBool>,
    signal: Arc<(Mutex<bool>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl Monitor {
    pub(crate) fn new() -> Self {
        let state = Arc::new(Mutex::new(None));
        let child = Arc::new(Mutex::new(None::<Child>));
        let stop = Arc::new(AtomicBool::new(false));
        let signal = Arc::new((Mutex::new(false), Condvar::new()));
        let (shared, running, stopping, wake) =
            (state.clone(), child.clone(), stop.clone(), signal.clone());
        let worker = thread::Builder::new()
            .name("nirubar-audio".into())
            .spawn(move || {
                while !stopping.load(Ordering::Acquire) {
                    update(&shared);
                    if let Some(path) = pactl_path() {
                        if let Ok(mut process) = spawn_subscriber(&path) {
                            if let Some(mut stdout) = process.stdout.take() {
                                let fd = stdout.as_raw_fd();
                                let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
                                if flags >= 0
                                    && unsafe {
                                        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK)
                                    } >= 0
                                {
                                    if let Ok(mut slot) = running.lock() {
                                        *slot = Some(process);
                                        if stopping.load(Ordering::Acquire) {
                                            if let Some(process) = slot.as_mut() {
                                                stop_child(process);
                                            }
                                        }
                                    } else {
                                        stop_child(&mut process);
                                        let _ = process.wait();
                                        break;
                                    }
                                    // Re-read after the subscriber starts so a
                                    // change during setup is less likely to be
                                    // missed between the initial query and the
                                    // subscription becoming active.
                                    thread::sleep(Duration::from_millis(50));
                                    if !stopping.load(Ordering::Acquire) {
                                        update(&shared);
                                    }
                                    let mut last_reconcile = Instant::now();
                                    let mut buffer = Vec::new();
                                    let mut chunk = [0u8; 4096];
                                    while !stopping.load(Ordering::Acquire) {
                                        let mut poll = libc::pollfd {
                                            fd,
                                            events: libc::POLLIN,
                                            revents: 0,
                                        };
                                        unsafe {
                                            libc::poll(&mut poll, 1, 1000);
                                        }
                                        match stdout.read(&mut chunk) {
                                            Ok(0) => break,
                                            Ok(n) => {
                                                let mut changed = false;
                                                for &byte in &chunk[..n] {
                                                    if byte == b'\n' {
                                                        changed |= affects_output(
                                                            &String::from_utf8_lossy(&buffer),
                                                        );
                                                        buffer.clear();
                                                    } else if buffer.len() < 4096 {
                                                        buffer.push(byte);
                                                    }
                                                }
                                                if changed {
                                                    update(&shared);
                                                }
                                            }
                                            Err(error)
                                                if matches!(
                                                    error.kind(),
                                                    std::io::ErrorKind::WouldBlock
                                                        | std::io::ErrorKind::Interrupted
                                                ) => {}
                                            Err(_) => break,
                                        }
                                        if last_reconcile.elapsed() >= Duration::from_secs(30) {
                                            update(&shared);
                                            last_reconcile = Instant::now();
                                        }
                                        if let Ok(mut slot) = running.lock() {
                                            if slot
                                                .as_mut()
                                                .and_then(|child| child.try_wait().ok())
                                                .flatten()
                                                .is_some()
                                            {
                                                break;
                                            }
                                        }
                                    }
                                    if let Ok(mut slot) = running.lock() {
                                        if let Some(mut process) = slot.take() {
                                            if matches!(process.try_wait(), Ok(None)) {
                                                stop_child(&mut process);
                                            }
                                            let _ = process.wait();
                                        }
                                    }
                                } else {
                                    stop_child(&mut process);
                                    let _ = process.wait();
                                }
                            } else {
                                stop_child(&mut process);
                                let _ = process.wait();
                            }
                        }
                    }
                    if stopping.load(Ordering::Acquire) {
                        break;
                    }
                    // Preserve the original 2 s provider cadence while Pulse is
                    // absent, but retry a missing/broken subscriber only every
                    // 30 s rather than adding another process every sample.
                    for _ in 0..15 {
                        let (lock, condvar) = &*wake;
                        let Ok(stopped) = lock.lock() else { return };
                        let Ok((stopped, _)) = condvar.wait_timeout_while(
                            stopped,
                            Duration::from_secs(2),
                            |stopped| !*stopped,
                        ) else {
                            return;
                        };
                        if *stopped || stopping.load(Ordering::Acquire) {
                            return;
                        }
                        drop(stopped);
                        update(&shared);
                    }
                }
            })
            .ok();
        Self {
            state,
            child,
            stop,
            signal,
            worker,
        }
    }

    pub(crate) fn snapshot(&self) -> Option<Volume> {
        self.state.lock().ok().and_then(|value| value.clone())
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Ok(mut child) = self.child.lock() {
            if let Some(child) = child.as_mut() {
                stop_child(child);
            }
        }
        let (lock, condvar) = &*self.signal;
        if let Ok(mut stopped) = lock.lock() {
            *stopped = true;
            condvar.notify_one();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_sink_and_server_events_refresh_output() {
        assert!(affects_output("Event 'change' on sink #42"));
        assert!(affects_output("Event 'change' on server #0"));
        assert!(!affects_output("Event 'change' on source #42"));
        assert!(!affects_output("Event 'new' on sink-input #42"));
    }
}
