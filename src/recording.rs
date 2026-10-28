//! NIRUCLIP state companion. The bar reads a small state file and never owns
//! frames or ffmpeg; a stale PID is reported as recoverable rather than shown
//! as an active recording.
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn state_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
        });
    base.join("niruclip/session.json")
}

fn field(text: &str, key: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value.get(key).and_then(|value| {
        value
            .as_str()
            .or_else(|| value.as_i64().map(|_| ""))
            .map(|value| value.to_string())
    })
}

fn number(text: &str, key: &str) -> Option<f64> {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()?
        .get(key)?
        .as_f64()
}

fn alive(pid: u64) -> bool {
    pid > 0 && std::path::Path::new(&format!("/proc/{pid}")).exists()
}

pub fn text() -> String {
    let path = state_path();
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let state = field(&text, "state").unwrap_or_default();
    let pid = number(&text, "pid").unwrap_or(0.0).max(0.0) as u64;
    if state == "failed" {
        return "● recover".into();
    }
    if state != "recording" {
        return String::new();
    }
    if !alive(pid) {
        return "● recover".into();
    }
    let started = number(&text, "started").unwrap_or(0.0) as u64;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(started);
    let seconds = now.saturating_sub(started);
    format!("● rec {:02}:{:02}", seconds / 60, seconds % 60)
}

pub fn detail() -> Vec<String> {
    let path = state_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return vec!["no active NIRUCLIP session".into()];
    };
    let state = field(&text, "state").unwrap_or_default();
    let pid = number(&text, "pid").unwrap_or(0.0).max(0.0) as u64;
    let output = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("output").and_then(|x| x.as_str()).map(str::to_string))
        .unwrap_or_default();
    let mut lines = vec![format!("state: {state}")];
    if !output.is_empty() {
        lines.push(format!("output: {output}"));
    }
    if state == "recording" && !alive(pid) {
        lines.push("recording process is gone; recover the partial file".into());
    }
    lines.extend([
        "right-click: stop recording".into(),
        "middle-click: open capture panel".into(),
    ]);
    lines
}
