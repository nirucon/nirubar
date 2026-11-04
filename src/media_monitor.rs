//! Event-driven MPRIS snapshots through one supervised playerctl follower.
//!
//! `playerctl --follow` watches the session bus, including player arrivals,
//! metadata/status changes and seeks. The previous bounded one-shot query is
//! retained when a follower exits or is unavailable.

use std::{
    io::Read,
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::av::{self, Media};

const FORMAT: &str =
    "{{status}}\u{1f}{{position}}\u{1f}{{artist}}\u{1f}{{title}}\u{1f}{{mpris:length}}\u{1e}";
const MAX_RECORD: usize = 64 * 1024;

#[derive(Default)]
struct RecordDecoder {
    record: Vec<u8>,
    after_record: bool,
    oversized: bool,
}

impl RecordDecoder {
    fn feed(&mut self, bytes: &[u8], mut changed: impl FnMut(Option<Media>)) {
        for &byte in bytes {
            if byte == 0x1e {
                if !self.oversized {
                    changed(av::media_from_snapshot(&String::from_utf8_lossy(
                        &self.record,
                    )));
                }
                self.record.clear();
                self.oversized = false;
                self.after_record = true;
            } else if byte == b'\n' && self.record.is_empty() && !self.oversized {
                if !self.after_record {
                    // playerctl's follower writes a blank line when the last
                    // player disappears. Do not keep stale track information.
                    changed(None);
                }
                self.after_record = false;
            } else if self.record.len() < MAX_RECORD && !self.oversized {
                self.record.push(byte);
                self.after_record = false;
            } else {
                self.oversized = true;
            }
        }
    }
}

#[derive(Default)]
struct State {
    media: Option<Media>,
}

fn update(state: &Mutex<State>, media: Option<Media>) {
    if let Ok(mut state) = state.lock() {
        let changed = state.media != media;
        state.media = media;
        if changed && std::env::var_os("NIRUBAR_TRACE_MEDIA").is_some() {
            let label = match state.media.as_ref() {
                Some(media) if media.playing => "playing",
                Some(_) => "paused",
                None => "absent",
            };
            crate::trace_media(&format!("state {label}"));
        }
        drop(state);
        if changed {
            crate::wake_update();
        }
    }
}

fn follower(selected: Option<&str>) -> std::io::Result<Child> {
    let mut cmd = Command::new("playerctl");
    if let Some(player) = selected.filter(|player| !player.is_empty()) {
        cmd.args(["-p", player]);
    }
    cmd.args(["--follow", "--format", FORMAT, "metadata"])
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

fn stop_child(child: &mut Child) {
    // The follower has its own process group; terminate ordinary descendants
    // as well so a wrapper cannot keep its output pipe open.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
}

pub(crate) struct Monitor {
    state: Arc<Mutex<State>>,
    selected: Arc<Mutex<Option<String>>>,
    child: Arc<Mutex<Option<Child>>>,
    stop: Arc<AtomicBool>,
    signal: Arc<(Mutex<bool>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl Monitor {
    pub(crate) fn new(selected: Option<String>) -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let selected = Arc::new(Mutex::new(selected));
        let child = Arc::new(Mutex::new(None::<Child>));
        let stop = Arc::new(AtomicBool::new(false));
        let signal = Arc::new((Mutex::new(false), Condvar::new()));
        let (state_worker, selected_worker, child_worker, stop_worker, signal_worker) = (
            state.clone(),
            selected.clone(),
            child.clone(),
            stop.clone(),
            signal.clone(),
        );
        let worker = thread::Builder::new()
            .name("nirubar-media".into())
            .spawn(move || {
                while !stop_worker.load(Ordering::Acquire) {
                    let player = selected_worker.lock().ok().and_then(|p| p.clone());
                    match follower(player.as_deref()) {
                        Ok(mut process) => {
                            let Some(mut stdout) = process.stdout.take() else {
                                continue;
                            };
                            let fd = stdout.as_raw_fd();
                            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
                            if flags < 0
                                || unsafe {
                                    libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK)
                                } < 0
                            {
                                stop_child(&mut process);
                                let _ = process.wait();
                                break;
                            }
                            if let Ok(mut slot) = child_worker.lock() {
                                *slot = Some(process);
                                if stop_worker.load(Ordering::Acquire) {
                                    if let Some(process) = slot.as_mut() {
                                        stop_child(process);
                                    }
                                }
                            } else {
                                stop_child(&mut process);
                                let _ = process.wait();
                                break;
                            }
                            let mut decoder = RecordDecoder::default();
                            let mut chunk = [0u8; 4096];
                            loop {
                                if stop_worker.load(Ordering::Acquire) {
                                    break;
                                }
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
                                    Ok(n) => decoder
                                        .feed(&chunk[..n], |media| update(&state_worker, media)),
                                    Err(error)
                                        if matches!(
                                            error.kind(),
                                            std::io::ErrorKind::WouldBlock
                                                | std::io::ErrorKind::Interrupted
                                        ) => {}
                                    Err(_) => break,
                                }
                                // An escaped descendant might retain stdout even
                                // after the leader exits. Never wait on that pipe
                                // indefinitely, including on player selection.
                                if let Ok(mut slot) = child_worker.lock() {
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
                            if let Ok(mut slot) = child_worker.lock() {
                                if let Some(mut process) = slot.take() {
                                    if matches!(process.try_wait(), Ok(None)) {
                                        stop_child(&mut process);
                                    }
                                    let _ = process.wait();
                                }
                            }
                        }
                        Err(_) => update(&state_worker, None),
                    }
                    if stop_worker.load(Ordering::Acquire) {
                        break;
                    }
                    // Some old playerctl versions do not support --follow or the
                    // formatted metadata. Preserve the bounded one-shot fallback.
                    let player = selected_worker.lock().ok().and_then(|p| p.clone());
                    update(&state_worker, av::Media::read_player(player.as_deref()));
                    let (lock, wake) = &*signal_worker;
                    let Ok(changed) = lock.lock() else { break };
                    if let Ok((mut changed, _)) =
                        wake.wait_timeout_while(changed, Duration::from_secs(2), |changed| {
                            !*changed
                        })
                    {
                        *changed = false;
                    }
                }
            })
            .ok();
        Self {
            state,
            selected,
            child,
            stop,
            signal,
            worker,
        }
    }

    pub(crate) fn select(&self, player: Option<String>) {
        if let Ok(mut selected) = self.selected.lock() {
            *selected = player;
        }
        self.interrupt();
    }

    pub(crate) fn snapshot(&self) -> Option<Media> {
        self.state.lock().ok()?.media.clone()
    }

    fn interrupt(&self) {
        if let Ok(mut child) = self.child.lock() {
            if let Some(child) = child.as_mut() {
                // The follower owns its process group. A wrapper must not
                // keep stdout open and make shutdown wait indefinitely.
                stop_child(child);
            }
        }
        let (lock, wake) = &*self.signal;
        if let Ok(mut changed) = lock.lock() {
            *changed = true;
            wake.notify_one();
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.interrupt();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_handles_split_records_multiline_titles_and_player_exit() {
        let mut decoder = RecordDecoder::default();
        let mut events = Vec::new();
        decoder.feed(b"Playing\x1f30000000\x1fArtist\x1fTitle\n", |v| {
            events.push(v)
        });
        assert!(events.is_empty());
        decoder.feed(b"continued\x1f120000000\x1e\n", |v| events.push(v));
        let media = events[0].as_ref().unwrap();
        assert_eq!(media.title, "Title\ncontinued");
        assert_eq!(media.position, Some((30, 120)));
        decoder.feed(b"\n", |v| events.push(v));
        assert_eq!(events.len(), 2);
        assert!(events[1].is_none());
    }

    #[test]
    fn decoder_discards_oversized_records_and_recovers() {
        let mut decoder = RecordDecoder::default();
        let mut events = Vec::new();
        decoder.feed(&vec![b'x'; MAX_RECORD + 5], |v| events.push(v));
        decoder.feed(b"\x1e\nPaused\x1f0\x1f\x1fNext\x1f\x1e\n", |v| {
            events.push(v)
        });
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].as_ref().unwrap().title, "Next");
    }
}
