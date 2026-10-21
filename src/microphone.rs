//! Read-only microphone snapshots via the suite's audio CLI. All provider
//! work runs off the X11 thread and has a deadline; painting only reads memory.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Snapshot {
    Ready {
        name: String,
        percent: u32,
        muted: bool,
    },
    NoInput,
    Unavailable(String),
}

fn parse(text: &str) -> Snapshot {
    let value = |key: &str| {
        text.lines()
            .filter_map(|line| line.split_once('='))
            .find_map(|(name, value)| (name == key).then_some(value))
    };
    match value("state") {
        Some("ready") => match (value("name"), value("percent"), value("muted")) {
            (Some(name), Some(percent), Some(muted)) if matches!(muted, "true" | "false") => {
                match percent.parse::<u32>() {
                    Ok(percent) if !name.is_empty() => Snapshot::Ready {
                        name: name.to_string(),
                        percent,
                        muted: muted == "true",
                    },
                    _ => Snapshot::Unavailable("invalid input level".into()),
                }
            }
            _ => Snapshot::Unavailable("incomplete microphone status".into()),
        },
        Some("no-input") => Snapshot::NoInput,
        _ => Snapshot::Unavailable(
            value("reason")
                .unwrap_or("microphone status unavailable")
                .into(),
        ),
    }
}

fn query() -> Snapshot {
    match crate::probe::run(
        "niructl",
        &["audio", "mic-status"],
        &[],
        Duration::from_secs(5),
        16 * 1024,
    ) {
        Some(output) if output.status.success() => parse(&String::from_utf8_lossy(&output.stdout)),
        _ => Snapshot::Unavailable("niructl audio mic-status did not respond".into()),
    }
}

pub struct Monitor {
    state: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Monitor {
    pub fn new() -> Self {
        let state = Arc::new(Mutex::new(Snapshot::Unavailable(
            "checking microphone".into(),
        )));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_state = state.clone();
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("nirubar-microphone".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Relaxed) {
                    let snapshot = query();
                    if let Ok(mut state) = worker_state.lock() {
                        *state = snapshot;
                    }
                    for _ in 0..20 {
                        if worker_stop.load(Ordering::Relaxed) {
                            return;
                        }
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            })
            .ok();
        Self {
            state,
            stop,
            worker,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|_| Snapshot::Unavailable("microphone status unavailable".into()))
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn text(snapshot: &Snapshot, icon: &str, visibility: &str) -> String {
    match snapshot {
        Snapshot::Ready { muted: true, .. } => format!("{icon} mute"),
        Snapshot::Ready { .. } if visibility == "muted" => String::new(),
        Snapshot::Ready { percent, .. } => format!("{icon} {percent}%"),
        Snapshot::NoInput => format!("{icon} none"),
        Snapshot::Unavailable(_) => format!("{icon} --"),
    }
}

pub fn detail(snapshot: &Snapshot) -> Vec<String> {
    let mut lines = match snapshot {
        Snapshot::Ready {
            name,
            percent,
            muted,
        } => vec![
            format!("input: {name}"),
            format!("state: {}", if *muted { "muted" } else { "unmuted" }),
            format!("input gain: {percent}%"),
            "recording activity: not monitored".into(),
        ],
        Snapshot::NoInput => vec!["no audio input is available".into()],
        Snapshot::Unavailable(reason) => vec![format!("unavailable: {reason}")],
    };
    lines.extend([
        String::new(),
        "right-click: toggle microphone mute".into(),
        "scroll: input gain (keeps mute unchanged)".into(),
        "middle-click: choose microphone / keyboard controls".into(),
        "keyboard: nirulauncher --microphone".into(),
    ]);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_is_not_recording_and_missing_values_are_not_zero() {
        let snapshot = parse("state=ready\nname=USB=Mic\npercent=75\nmuted=false\n");
        assert_eq!(text(&snapshot, "mic", "always"), "mic 75%");
        assert_eq!(text(&snapshot, "mic", "muted"), "");
        assert!(detail(&snapshot).contains(&"recording activity: not monitored".into()));
        assert!(matches!(
            parse("state=ready\nname=Mic\nmuted=false"),
            Snapshot::Unavailable(_)
        ));
        assert_eq!(parse("state=no-input"), Snapshot::NoInput);
        assert_eq!(
            text(
                &parse("state=ready\nname=Mic\npercent=0\nmuted=true"),
                "mic",
                "muted"
            ),
            "mic mute"
        );
    }
}
