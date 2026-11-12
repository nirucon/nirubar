//! Privacy dot: shown while an app uses the microphone, a camera or records
//! the screen, hidden otherwise.
//!
//! Every source is off the X11 thread or a cheap read, and nothing polls:
//! - Microphone and PipeWire cameras: one long-lived `pw-dump --monitor`
//!   child. It prints the graph once and afterwards only the objects that
//!   change, so an idle session costs nothing (the worker sleeps in poll(2)).
//!   A capture stream counts when it runs and is linked to an audio source;
//!   captures of a sink monitor (visualisers such as CAVA, the lock screen's
//!   bars) and peak meters do not.
//! - Cameras opened directly: inotify on `/dev/video*` (open and close) wakes
//!   one scan of this user's `/proc/<pid>/fd` links. Nothing is scanned
//!   while no device is opened or closed.
//! - Screen recording: NIRUCLIP's state (`crate::recording`), read on paint.
//! - NIRUDICTATE recording counts as microphone use (`crate::dictation`).
use std::collections::HashMap;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// What the background worker sees. Screen recording and NIRUDICTATE are
/// added on paint by the module.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Apps capturing from a microphone (PipeWire).
    pub microphone: Vec<String>,
    /// Apps using a camera (PipeWire or the device node).
    pub camera: Vec<String>,
    /// False while `pw-dump` is missing or not answering.
    pub audio_known: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Object {
    kind: String,
    class: String,
    state: String,
    app: String,
    binary: String,
    pid: u32,
    node_name: String,
    media_name: String,
    device_api: String,
    capture_sink: bool,
    /// For links: (output node, input node).
    link: Option<(u32, u32)>,
}

/// The parts of the PipeWire graph the dot needs, kept up to date from the
/// `pw-dump` stream.
#[derive(Default)]
struct Graph {
    objects: HashMap<u32, Object>,
}

fn prop_str(props: &serde_json::Value, key: &str) -> String {
    match props.get(key) {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Number(number)) => number.to_string(),
        Some(serde_json::Value::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

impl Graph {
    /// Apply one element of a `pw-dump` array. An object whose `info` is
    /// null was removed.
    fn apply(&mut self, value: &serde_json::Value) {
        let Some(id) = value.get("id").and_then(|id| id.as_u64()) else {
            return;
        };
        let id = id as u32;
        let info = value.get("info").filter(|info| !info.is_null());
        let Some(info) = info else {
            self.objects.remove(&id);
            return;
        };
        let object = self.objects.entry(id).or_default();
        if let Some(kind) = value.get("type").and_then(|kind| kind.as_str()) {
            object.kind = kind.rsplit(':').next().unwrap_or(kind).to_string();
        }
        if let Some(state) = info.get("state").and_then(|state| state.as_str()) {
            object.state = state.to_string();
        }
        if let (Some(output), Some(input)) = (
            info.get("output-node-id").and_then(|id| id.as_u64()),
            info.get("input-node-id").and_then(|id| id.as_u64()),
        ) {
            object.link = Some((output as u32, input as u32));
        }
        if let Some(props) = info.get("props").filter(|props| props.is_object()) {
            object.class = prop_str(props, "media.class");
            object.app = prop_str(props, "application.name");
            object.binary = prop_str(props, "application.process.binary");
            object.pid = prop_str(props, "application.process.id")
                .parse()
                .unwrap_or(0);
            object.node_name = prop_str(props, "node.name");
            object.media_name = prop_str(props, "media.name");
            object.device_api = prop_str(props, "device.api");
            object.capture_sink = prop_str(props, "stream.capture.sink") == "true";
        }
    }

    fn sources_of(&self, node: u32) -> impl Iterator<Item = &Object> {
        self.objects
            .values()
            .filter_map(move |object| match object.link {
                Some((output, input)) if input == node => self.objects.get(&output),
                _ => None,
            })
    }

    fn consumers_of(&self, node: u32) -> impl Iterator<Item = &Object> {
        self.objects
            .values()
            .filter_map(move |object| match object.link {
                Some((output, input)) if output == node => self.objects.get(&input),
                _ => None,
            })
    }

    /// Apps capturing from an audio source right now.
    fn microphone(&self, own_pid: u32) -> Vec<String> {
        let mut apps = Vec::new();
        for (id, node) in &self.objects {
            if node.kind != "Node"
                || !node.class.starts_with("Stream/Input/Audio")
                || node.state != "running"
                || node.capture_sink
                || node.media_name == "Peak detect"
                || (own_pid != 0 && node.pid == own_pid)
                || internal(node)
            {
                continue;
            }
            // Linked to a real input. A sink (monitor capture) or another
            // app's output stream is not the microphone.
            if self.sources_of(*id).any(|source| {
                source.class.starts_with("Audio/Source") || source.class.starts_with("Audio/Duplex")
            }) {
                apps.push(app_name(node));
            }
        }
        tidy(apps)
    }

    /// Apps reading a camera through PipeWire.
    fn camera(&self) -> Vec<String> {
        let mut apps = Vec::new();
        for (id, node) in &self.objects {
            let camera = node.kind == "Node"
                && node.class.starts_with("Video/Source")
                && (matches!(node.device_api.as_str(), "v4l2" | "libcamera")
                    || node.node_name.starts_with("v4l2_")
                    || node.node_name.starts_with("libcamera_"));
            if !camera || node.state != "running" {
                continue;
            }
            let users: Vec<String> = self
                .consumers_of(*id)
                .filter(|user| !internal(user))
                .map(app_name)
                .collect();
            if users.is_empty() {
                apps.push("PipeWire".into());
            } else {
                apps.extend(users);
            }
        }
        tidy(apps)
    }
}

/// Streams PipeWire and WirePlumber create themselves (filter chains, echo
/// cancellation) are not apps using the device.
fn internal(node: &Object) -> bool {
    matches!(node.binary.as_str(), "pipewire" | "wireplumber")
}

fn app_name(node: &Object) -> String {
    let name = [&node.app, &node.binary, &node.node_name]
        .into_iter()
        .find(|name| !name.is_empty())
        .cloned()
        .unwrap_or_else(|| "unknown app".into());
    // ALSA clients through pipewire-alsa: "PipeWire ALSA [fluidsynth]".
    match name
        .strip_prefix("PipeWire ALSA [")
        .and_then(|rest| rest.strip_suffix(']'))
    {
        Some(inner) if !inner.is_empty() => inner.to_string(),
        _ => name,
    }
}

/// Sorted, without case-insensitive duplicates.
fn tidy(mut names: Vec<String>) -> Vec<String> {
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

/// Parses consecutive JSON values (pw-dump prints one array per change).
#[derive(Default)]
struct JsonStream {
    buffer: Vec<u8>,
}

/// Larger than any real graph dump; a stream that grows past it is broken.
const STREAM_CAP: usize = 16 * 1024 * 1024;

impl JsonStream {
    /// Feed bytes; returns the complete values, or Err when the stream is
    /// not JSON any more and the reader should start over.
    fn feed(&mut self, bytes: &[u8]) -> Result<Vec<serde_json::Value>, ()> {
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() > STREAM_CAP {
            self.buffer.clear();
            return Err(());
        }
        let mut values = Vec::new();
        let mut consumed = 0;
        let mut stream =
            serde_json::Deserializer::from_slice(&self.buffer).into_iter::<serde_json::Value>();
        loop {
            match stream.next() {
                Some(Ok(value)) => {
                    consumed = stream.byte_offset();
                    values.push(value);
                }
                Some(Err(error)) if error.is_eof() => break,
                Some(Err(_)) => {
                    self.buffer.clear();
                    return Err(());
                }
                None => {
                    consumed = self.buffer.len();
                    break;
                }
            }
        }
        self.buffer.drain(..consumed);
        Ok(values)
    }
}

/// The directory with the video device nodes. Tests point it elsewhere so
/// no real camera is opened.
fn dev_dir() -> PathBuf {
    std::env::var_os("NIRUBAR_PRIVACY_DEV")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/dev"))
}

fn is_video_name(name: &str) -> bool {
    name.strip_prefix("video")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

fn video_devices(dev: &Path) -> Vec<PathBuf> {
    let mut devices: Vec<PathBuf> = std::fs::read_dir(dev)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_name().to_str().is_some_and(is_video_name))
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default();
    devices.sort();
    devices
}

/// Processes of this user that hold a video device open. PipeWire and
/// WirePlumber are left out: the graph names the app behind them.
fn camera_holders(dev: &Path) -> Vec<String> {
    if video_devices(dev).is_empty() {
        return Vec::new();
    }
    let uid = unsafe { libc::getuid() };
    let own = std::process::id();
    let mut names = Vec::new();
    let Ok(processes) = std::fs::read_dir("/proc") else {
        return names;
    };
    for process in processes.flatten() {
        let Some(pid) = process
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == own {
            continue;
        }
        use std::os::unix::fs::MetadataExt;
        if process.metadata().ok().map(|meta| meta.uid()) != Some(uid) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        let holds = fds.flatten().any(|fd| {
            std::fs::read_link(fd.path()).is_ok_and(|target| {
                target.parent() == Some(dev)
                    && target
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(is_video_name)
            })
        });
        if !holds {
            continue;
        }
        let comm = std::fs::read_to_string(process.path().join("comm"))
            .map(|comm| comm.trim().to_string())
            .unwrap_or_default();
        if comm.is_empty() || matches!(comm.as_str(), "pipewire" | "wireplumber") {
            continue;
        }
        names.push(comm);
    }
    tidy(names)
}

/// inotify on the device directory (devices appearing or going) and on each
/// video device (opened or closed).
struct Watch {
    fd: i32,
    dir: i32,
    dev: PathBuf,
    devices: Vec<i32>,
}

impl Watch {
    fn new(dev: &Path) -> Option<Self> {
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let path = std::ffi::CString::new(dev.as_os_str().as_encoded_bytes()).ok()?;
        let dir = unsafe {
            libc::inotify_add_watch(fd, path.as_ptr(), libc::IN_CREATE | libc::IN_DELETE)
        };
        let mut watch = Self {
            fd,
            dir,
            dev: dev.to_path_buf(),
            devices: Vec::new(),
        };
        watch.watch_devices();
        Some(watch)
    }

    fn watch_devices(&mut self) {
        for wd in self.devices.drain(..) {
            unsafe {
                libc::inotify_rm_watch(self.fd, wd);
            }
        }
        for device in video_devices(&self.dev) {
            let Ok(path) = std::ffi::CString::new(device.as_os_str().as_encoded_bytes()) else {
                continue;
            };
            let wd = unsafe {
                libc::inotify_add_watch(
                    self.fd,
                    path.as_ptr(),
                    libc::IN_OPEN | libc::IN_CLOSE_WRITE | libc::IN_CLOSE_NOWRITE,
                )
            };
            if wd >= 0 {
                self.devices.push(wd);
            }
        }
    }

    /// Drain pending events; true when something relevant happened.
    fn drain(&mut self) -> bool {
        let mut buffer = [0u8; 4096];
        let mut relevant = false;
        let mut devices_changed = false;
        loop {
            let read = unsafe { libc::read(self.fd, buffer.as_mut_ptr() as *mut _, buffer.len()) };
            if read <= 0 {
                break;
            }
            let mut offset = 0usize;
            let header = std::mem::size_of::<libc::inotify_event>();
            while offset + header <= read as usize {
                // SAFETY: the kernel writes whole events; read_unaligned
                // copes with the byte buffer's alignment.
                let event: libc::inotify_event =
                    unsafe { std::ptr::read_unaligned(buffer.as_ptr().add(offset) as *const _) };
                let name_start = offset + header;
                let name_end = (name_start + event.len as usize).min(read as usize);
                if event.wd == self.dir {
                    let name = &buffer[name_start..name_end];
                    let name = name.split(|b| *b == 0).next().unwrap_or_default();
                    if std::str::from_utf8(name).is_ok_and(is_video_name) {
                        devices_changed = true;
                        relevant = true;
                    }
                } else {
                    relevant = true;
                }
                offset = name_end;
            }
        }
        if devices_changed {
            self.watch_devices();
        }
        relevant
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

fn pw_dump_path() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("pw-dump"))
            .find(|candidate| candidate.is_file())
    })
}

fn spawn_pw_dump(path: &Path) -> Option<(Child, ChildStdout)> {
    let mut command = Command::new(path);
    let parent = std::process::id();
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() as u32 != parent {
                return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
            }
            Ok(())
        });
    }
    let mut child = command
        .arg("--monitor")
        .env("LC_ALL", "C")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let Some(stdout) = child.stdout.take() else {
        stop_child(&mut child);
        return None;
    };
    let fd = stdout.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        stop_child(&mut child);
        return None;
    }
    Some((child, stdout))
}

fn stop_child(child: &mut Child) {
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Retry a missing or failed `pw-dump` after this long.
const RETRY: Duration = Duration::from_secs(30);
/// Coalesce a burst of device opens into one /proc scan, at most this often.
const SCAN_DELAY: Duration = Duration::from_millis(300);
const SCAN_GAP: Duration = Duration::from_secs(1);

fn worker(shared: Arc<Mutex<Snapshot>>, stop: i32) {
    let dev = dev_dir();
    let own_pid = std::process::id();
    let mut watch = Watch::new(&dev);
    let mut holders = camera_holders(&dev);
    let mut last_scan = Instant::now();
    let mut scan_at: Option<Instant> = None;
    let mut graph = Graph::default();
    let mut stream = JsonStream::default();
    let mut dump: Option<(Child, ChildStdout)> = None;
    let mut retry_at = Some(Instant::now());
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let now = Instant::now();
        if retry_at.is_some_and(|at| at <= now) {
            retry_at = None;
            dump = pw_dump_path().and_then(|path| spawn_pw_dump(&path));
            graph = Graph::default();
            stream = JsonStream::default();
            if dump.is_none() {
                retry_at = Some(now + RETRY);
            }
        }
        if scan_at.is_some_and(|at| at <= now) {
            scan_at = None;
            holders = camera_holders(&dev);
            last_scan = Instant::now();
        }
        let mut camera = graph.camera();
        camera.extend(holders.iter().cloned());
        let snapshot = Snapshot {
            microphone: graph.microphone(own_pid),
            camera: tidy(camera),
            audio_known: dump.is_some(),
        };
        if let Ok(mut current) = shared.lock() {
            if *current != snapshot {
                *current = snapshot;
                drop(current);
                crate::wake_update();
            }
        }

        let mut fds = vec![libc::pollfd {
            fd: stop,
            events: libc::POLLIN,
            revents: 0,
        }];
        if let Some(watch) = &watch {
            fds.push(libc::pollfd {
                fd: watch.fd,
                events: libc::POLLIN,
                revents: 0,
            });
        }
        if let Some((_, stdout)) = &dump {
            fds.push(libc::pollfd {
                fd: stdout.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
        }
        let deadline = [scan_at, retry_at].into_iter().flatten().min();
        let timeout = deadline
            .map(|at| {
                at.saturating_duration_since(Instant::now())
                    .as_millis()
                    .clamp(1, i32::MAX as u128) as i32
            })
            .unwrap_or(-1);
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if fds[0].revents != 0 {
            break;
        }
        let mut index = 1;
        if let Some(watch) = &mut watch {
            if fds[index].revents != 0 && watch.drain() && scan_at.is_none() {
                scan_at = Some((Instant::now() + SCAN_DELAY).max(last_scan + SCAN_GAP));
            }
            index += 1;
        }
        let mut failed = false;
        if let Some((_, stdout)) = &mut dump {
            if fds[index].revents != 0 {
                loop {
                    match stdout.read(&mut chunk) {
                        Ok(0) => {
                            failed = true;
                            break;
                        }
                        Ok(read) => match stream.feed(&chunk[..read]) {
                            Ok(values) => {
                                for value in values {
                                    match value {
                                        serde_json::Value::Array(items) => {
                                            items.iter().for_each(|item| graph.apply(item))
                                        }
                                        other => graph.apply(&other),
                                    }
                                }
                            }
                            Err(()) => {
                                failed = true;
                                break;
                            }
                        },
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(_) => {
                            failed = true;
                            break;
                        }
                    }
                }
            }
        }
        if failed {
            if let Some((mut child, _)) = dump.take() {
                stop_child(&mut child);
            }
            graph = Graph::default();
            retry_at = Some(Instant::now() + RETRY);
        }
    }
    if let Some((mut child, _)) = dump.take() {
        stop_child(&mut child);
    }
}

pub struct Monitor {
    state: Arc<Mutex<Snapshot>>,
    stop: Option<(i32, i32)>,
    worker: Option<JoinHandle<()>>,
}

impl Monitor {
    pub fn new() -> Self {
        let state = Arc::new(Mutex::new(Snapshot::default()));
        let mut fds = [0i32; 2];
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
            return Self {
                state,
                stop: None,
                worker: None,
            };
        }
        let shared = state.clone();
        let read = fds[0];
        let worker = std::thread::Builder::new()
            .name("nirubar-privacy".into())
            .spawn(move || worker(shared, read))
            .ok();
        Self {
            state,
            stop: Some((fds[0], fds[1])),
            worker,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default()
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        if let Some((_, write)) = self.stop {
            unsafe {
                let byte = [1u8];
                libc::write(write, byte.as_ptr() as *const _, 1);
            }
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some((read, write)) = self.stop.take() {
            unsafe {
                libc::close(read);
                libc::close(write);
            }
        }
    }
}

/// Everything in use, as the module shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub microphone: Vec<String>,
    pub camera: Vec<String>,
    pub screen: Vec<String>,
    pub audio_known: bool,
}

impl Usage {
    pub fn new(snapshot: Snapshot, dictating: bool, clip_recording: bool) -> Self {
        let mut microphone = snapshot.microphone;
        if dictating {
            microphone.push("NIRUDICTATE".into());
        }
        Self {
            microphone: tidy(microphone),
            camera: snapshot.camera,
            screen: if clip_recording {
                vec!["NIRUCLIP".into()]
            } else {
                Vec::new()
            },
            audio_known: snapshot.audio_known,
        }
    }

    pub fn active(&self) -> bool {
        !(self.microphone.is_empty() && self.camera.is_empty() && self.screen.is_empty())
    }
}

pub fn text(usage: &Usage) -> &'static str {
    if usage.active() {
        "●"
    } else {
        ""
    }
}

pub fn detail(usage: &Usage) -> Vec<String> {
    let mut lines = Vec::new();
    for (what, apps) in [
        ("microphone", &usage.microphone),
        ("camera", &usage.camera),
        ("screen recording", &usage.screen),
    ] {
        if !apps.is_empty() {
            lines.push(format!("{what}: {}", apps.join(", ")));
        }
    }
    if lines.is_empty() {
        lines.push("nothing uses the microphone, a camera or screen recording".into());
    }
    if !usage.audio_known {
        lines.push("microphone apps: unknown (pw-dump from PipeWire is not available)".into());
    }
    lines.push(String::new());
    lines.push("only NIRUCLIP counts as screen recording:".into());
    lines.push("X11 cannot tell when others capture it".into());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(id: u32, class: &str, state: &str, props: serde_json::Value) -> serde_json::Value {
        let mut props = props;
        props["media.class"] = json!(class);
        json!({"id": id, "type": "PipeWire:Interface:Node",
               "info": {"state": state, "props": props}})
    }

    fn link(id: u32, output: u32, input: u32) -> serde_json::Value {
        json!({"id": id, "type": "PipeWire:Interface:Link",
               "info": {"output-node-id": output, "input-node-id": input, "state": "active"}})
    }

    fn graph(values: &[serde_json::Value]) -> Graph {
        let mut graph = Graph::default();
        values.iter().for_each(|value| graph.apply(value));
        graph
    }

    #[test]
    fn a_running_capture_from_a_microphone_counts() {
        let g = graph(&[
            node(
                40,
                "Audio/Source",
                "running",
                json!({"node.name": "alsa_input.usb"}),
            ),
            node(
                50,
                "Stream/Input/Audio",
                "running",
                json!({"application.name": "Firefox", "application.process.id": 4242}),
            ),
            link(60, 40, 50),
        ]);
        assert_eq!(g.microphone(1), vec!["Firefox".to_string()]);
        // Our own capture would not count (the bar has none, but be safe).
        assert!(g.microphone(4242).is_empty());
    }

    #[test]
    fn monitor_captures_peak_meters_and_idle_streams_do_not_count() {
        let g = graph(&[
            node(
                40,
                "Audio/Sink",
                "running",
                json!({"node.name": "alsa_output"}),
            ),
            node(
                41,
                "Audio/Source",
                "running",
                json!({"node.name": "alsa_input"}),
            ),
            // CAVA on the sink monitor.
            node(
                50,
                "Stream/Input/Audio",
                "running",
                json!({"application.name": "cava"}),
            ),
            link(60, 40, 50),
            // A capture that asks for the monitor before it is linked.
            node(
                51,
                "Stream/Input/Audio",
                "running",
                json!({"application.name": "nirulock", "stream.capture.sink": "true"}),
            ),
            // pavucontrol's level meter on the microphone.
            node(
                52,
                "Stream/Input/Audio",
                "running",
                json!({"application.name": "PulseAudio Volume Control", "media.name": "Peak detect"}),
            ),
            link(61, 41, 52),
            // A browser tab that paused its capture.
            node(
                53,
                "Stream/Input/Audio",
                "idle",
                json!({"application.name": "Chromium"}),
            ),
            link(62, 41, 53),
            // Echo cancellation inside PipeWire.
            node(
                54,
                "Stream/Input/Audio",
                "running",
                json!({"node.name": "echo-cancel-capture", "application.process.binary": "pipewire"}),
            ),
            link(63, 41, 54),
        ]);
        assert!(g.microphone(1).is_empty());
    }

    #[test]
    fn removal_and_updates_follow_the_monitor_stream() {
        let mut g = graph(&[
            node(41, "Audio/Source", "running", json!({})),
            node(
                50,
                "Stream/Input/Audio",
                "suspended",
                json!({"application.name": "PipeWire ALSA [nirudictate]"}),
            ),
            link(60, 41, 50),
        ]);
        assert!(g.microphone(1).is_empty());
        // Monitor updates repeat the type and carry the changed state.
        g.apply(
            &json!({"id": 50, "type": "PipeWire:Interface:Node", "info": {"state": "running"}}),
        );
        assert_eq!(g.microphone(1), vec!["nirudictate".to_string()]);
        g.apply(&json!({"id": 50, "info": null}));
        assert!(g.microphone(1).is_empty());
    }

    #[test]
    fn running_cameras_name_their_consumers() {
        let g = graph(&[
            node(
                71,
                "Video/Source",
                "running",
                json!({"node.name": "v4l2_input.pci-usb", "device.api": "v4l2"}),
            ),
            node(
                80,
                "Stream/Input/Video",
                "running",
                json!({"application.name": "OBS Studio"}),
            ),
            link(90, 71, 80),
            node(
                73,
                "Video/Source",
                "suspended",
                json!({"node.name": "v4l2_input.other"}),
            ),
        ]);
        assert_eq!(g.camera(), vec!["OBS Studio".to_string()]);
    }

    #[test]
    fn the_json_stream_handles_split_and_consecutive_arrays() {
        let mut stream = JsonStream::default();
        assert!(stream.feed(b"[{\"id\": 1, \"info\"").unwrap().is_empty());
        let values = stream.feed(b": null}]\n[{\"id\": 2}]\n[").unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(stream.feed(b"]").unwrap().len(), 1);
        assert!(stream.feed(b"} nonsense").is_err());
    }

    #[test]
    fn video_names_are_exact() {
        assert!(is_video_name("video0"));
        assert!(is_video_name("video12"));
        assert!(!is_video_name("video"));
        assert!(!is_video_name("video0p"));
        assert!(!is_video_name("media0"));
    }

    #[test]
    fn the_dot_shows_any_use_and_lists_it() {
        let idle = Usage::new(
            Snapshot {
                audio_known: true,
                ..Snapshot::default()
            },
            false,
            false,
        );
        assert_eq!(text(&idle), "");
        let usage = Usage::new(
            Snapshot {
                microphone: vec!["nirudictate".into(), "Firefox".into()],
                camera: vec![],
                audio_known: true,
            },
            true,
            false,
        );
        assert_eq!(text(&usage), "●");
        assert_eq!(detail(&usage)[0], "microphone: Firefox, nirudictate");
        let usage = Usage::new(Snapshot::default(), false, true);
        assert!(usage.active());
        let lines = detail(&usage);
        assert_eq!(lines[0], "screen recording: NIRUCLIP");
        assert!(lines.iter().any(|line| line.contains("pw-dump")));
    }

    #[test]
    fn a_held_device_node_names_the_process() {
        let dir = std::env::temp_dir().join(format!("nirubar-privacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let device = dir.join("video0");
        std::fs::write(&device, b"").unwrap();
        let mut child = Command::new("sleep")
            .arg("30")
            .stdin(std::fs::File::open(&device).unwrap())
            .spawn()
            .unwrap();
        // The child has the file open before spawn returns.
        let holders = camera_holders(&dir);
        let _ = child.kill();
        let _ = child.wait();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(holders, vec!["sleep".to_string()]);
    }
}
