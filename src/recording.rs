//! NIRUCLIP state companion. The bar observes a versioned state file and never
//! owns frames, ffmpeg or recovery. Only a validated interrupted partial is
//! offered as recoverable; a dead process alone is a failure.
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Default)]
struct Performance {
    fps: Option<f64>,
    frame: Option<u64>,
    drop_frames: Option<u64>,
    dup_frames: Option<u64>,
    speed: Option<f64>,
}

impl Performance {
    fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(fps) = self.fps {
            parts.push(format!("{fps:.1} fps"));
        }
        if let Some(frames) = self.frame {
            parts.push(format!("{frames} frames"));
        }
        if let Some(dropped) = self.drop_frames {
            if dropped > 0 {
                parts.push(format!("{dropped} dropped"));
            }
        }
        if let Some(duplicated) = self.dup_frames {
            if duplicated > 0 {
                parts.push(format!("{duplicated} duplicated"));
            }
        }
        if let Some(speed) = self.speed {
            parts.push(format!("{speed:.2}x"));
        }
        parts.join(", ")
    }

    fn needs_attention(&self) -> bool {
        let frames = self.frame.unwrap_or(0);
        let dropped = self.drop_frames.unwrap_or(0);
        let duplicated = self.dup_frames.unwrap_or(0);
        if frames == 0 {
            return dropped > 0 || duplicated >= 10;
        }
        let frames = frames as f64;
        dropped as f64 / frames > 0.02 || duplicated as f64 / frames > 0.05
    }
}

#[derive(Clone, Debug, Default)]
struct State {
    state: String,
    pid: u32,
    pid_start: u64,
    partial: String,
    output: String,
    started: f64,
    recoverable: bool,
    performance: Option<Performance>,
    reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Recording { seconds: u64 },
    Recover,
    Failed,
    Hidden,
}

fn state_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
        });
    base.join("niruclip/session.json")
}

fn text_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(|value| {
        value
            .as_str()
            .map(str::to_string)
            .or_else(|| value.as_i64().map(|number| number.to_string()))
    })
}

fn number_field(value: &serde_json::Value, key: &str) -> Option<f64> {
    value.get(key).and_then(|value| value.as_f64())
}

fn read() -> Option<State> {
    let text = std::fs::read_to_string(state_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(State {
        state: text_field(&value, "state").unwrap_or_default(),
        pid: number_field(&value, "pid").unwrap_or(0.0).max(0.0) as u32,
        pid_start: number_field(&value, "pid_start").unwrap_or(0.0).max(0.0) as u64,
        partial: text_field(&value, "partial").unwrap_or_default(),
        output: text_field(&value, "output").unwrap_or_default(),
        started: number_field(&value, "started").unwrap_or(0.0),
        recoverable: value
            .get("recoverable")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        performance: value.get("performance").and_then(|value| {
            value.as_object()?;
            Some(Performance {
                fps: number_field(value, "fps"),
                frame: number_field(value, "frame").map(|number| number.max(0.0) as u64),
                drop_frames: number_field(value, "drop_frames")
                    .map(|number| number.max(0.0) as u64),
                dup_frames: number_field(value, "dup_frames").map(|number| number.max(0.0) as u64),
                speed: number_field(value, "speed"),
            })
        }),
        reason: text_field(&value, "reason").unwrap_or_default(),
    })
}

fn proc_start(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = text.rfind(')')?;
    text[close + 1..].split_whitespace().nth(19)?.parse().ok()
}

fn alive(pid: u32, pid_start: u64) -> bool {
    if pid == 0 {
        return false;
    }
    let Some(actual) = proc_start(pid) else {
        return false;
    };
    pid_start == 0 || actual == pid_start
}

fn partial_usable(path: &str) -> bool {
    let path = Path::new(path);
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.len() > 0)
        .unwrap_or(false)
}

fn classify(state: &State, partial_exists: bool, alive: bool) -> View {
    match state.state.as_str() {
        "recording" | "starting" | "finalizing" | "countdown" if alive => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            let started = state.started.max(0.0) as u64;
            View::Recording {
                seconds: now.saturating_sub(started),
            }
        }
        "recording" | "starting" | "finalizing" | "countdown" | "failed" | "interrupted"
            if state.recoverable && partial_exists =>
        {
            View::Recover
        }
        "recording" | "starting" | "finalizing" | "countdown" | "failed" | "interrupted" => {
            View::Failed
        }
        _ => View::Hidden,
    }
}

fn current() -> (State, View) {
    let Some(state) = read() else {
        return (State::default(), View::Hidden);
    };
    let view = classify(
        &state,
        partial_usable(&state.partial),
        alive(state.pid, state.pid_start),
    );
    (state, view)
}

pub fn text() -> String {
    let (_, view) = current();
    match view {
        View::Recording { seconds } => format!("● rec {:02}:{:02}", seconds / 60, seconds % 60),
        View::Recover => "● recover".into(),
        View::Failed => "● failed".into(),
        View::Hidden => String::new(),
    }
}

pub fn detail() -> Vec<String> {
    let (state, view) = current();
    if view == View::Hidden && state.state.is_empty() {
        return vec!["no active NIRUCLIP session".into()];
    }
    let mut lines = vec![format!("state: {}", state.state)];
    if !state.output.is_empty() {
        lines.push(format!("output: {}", state.output));
    }
    if !state.partial.is_empty() {
        lines.push(format!("partial: {}", state.partial));
    }
    if !state.reason.is_empty() {
        lines.push(format!("reason: {}", state.reason));
    }
    if let Some(performance) = &state.performance {
        let summary = performance.summary();
        if !summary.is_empty() {
            lines.push(format!("capture: {summary}"));
        }
        if performance.needs_attention() {
            lines.push(
                "warning: capture cannot keep up; try preset small or disable the camera overlay"
                    .into(),
            );
        }
    }
    match view {
        View::Recording { .. } => {
            lines.push("right-click: stop recording".into());
            lines.push("middle-click: open capture panel".into());
        }
        View::Recover => {
            lines.push("right-click: validate and recover the partial file".into());
            lines.push("left-click: open capture panel".into());
        }
        View::Failed => {
            lines.push("no recoverable partial is available".into());
            lines.push("left-click: open capture panel".into());
        }
        View::Hidden => lines.push("no active recording".into()),
    }
    lines
}

pub fn can_stop() -> bool {
    matches!(current().1, View::Recording { .. })
}

pub fn can_recover() -> bool {
    matches!(current().1, View::Recover)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str, recoverable: bool) -> State {
        State {
            state: name.into(),
            recoverable,
            started: 10.0,
            ..State::default()
        }
    }

    #[test]
    fn a_dead_recorder_without_a_validated_partial_is_a_failure() {
        assert_eq!(
            classify(&state("recording", false), false, false),
            View::Failed
        );
        assert_eq!(
            classify(&state("recording", true), false, false),
            View::Failed
        );
    }

    #[test]
    fn only_a_validated_partial_is_offered_for_recovery() {
        assert_eq!(
            classify(&state("interrupted", true), true, false),
            View::Recover
        );
        assert_eq!(
            classify(&state("interrupted", false), true, false),
            View::Failed
        );
        assert_eq!(classify(&state("failed", true), true, false), View::Recover);
        assert_eq!(classify(&state("failed", false), true, false), View::Failed);
    }

    #[test]
    fn finished_and_cancelled_sessions_hide_the_module() {
        assert_eq!(
            classify(&state("finished", false), true, false),
            View::Hidden
        );
        assert_eq!(
            classify(&state("cancelled", false), false, false),
            View::Hidden
        );
    }

    #[test]
    fn an_active_pid_shows_recording() {
        assert!(matches!(
            classify(&state("recording", false), false, true),
            View::Recording { .. }
        ));
    }

    #[test]
    fn capture_performance_flags_a_stream_that_cannot_keep_up() {
        let healthy = Performance {
            fps: Some(29.8),
            frame: Some(120),
            drop_frames: Some(0),
            dup_frames: Some(0),
            speed: Some(0.99),
        };
        assert!(!healthy.needs_attention());
        assert!(healthy.summary().contains("29.8 fps"));
        let struggling = Performance {
            fps: Some(20.0),
            frame: Some(90),
            drop_frames: Some(4),
            dup_frames: Some(55),
            speed: Some(0.68),
        };
        assert!(struggling.needs_attention());
        assert!(struggling.summary().contains("4 dropped"));
        assert!(struggling.summary().contains("55 duplicated"));
    }
}
