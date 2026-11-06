//! NIRUDICTATE state companion: a red dot while dictation records, a distinct
//! mark while it transcribes, nothing otherwise.
//!
//! NIRUDICTATE replaces `$XDG_RUNTIME_DIR/nirudictate/state.json` atomically on
//! every change and sets the root property `_NIRUDICTATE_STATE`, which wakes
//! the bar at once. The file is only re-read when its modification time or
//! size changed; a stale file from a dead service is ignored by checking the
//! recorded PID and its start time. Without NIRUDICTATE the module reads
//! nothing and draws nothing.
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Clone, Debug, Default, PartialEq)]
struct State {
    state: String,
    pid: u32,
    pid_start: u64,
    since: f64,
    language: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Recording,
    Transcribing,
    Hidden,
}

fn state_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        Some(run) => run.join("nirudictate/state.json"),
        None => std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
            })
            .join("nirudictate/run/state.json"),
    }
}

fn parse(text: &str) -> Option<State> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("version").and_then(|x| x.as_u64()) != Some(1) {
        return None;
    }
    Some(State {
        state: v.get("state")?.as_str()?.to_string(),
        pid: v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        pid_start: v.get("pid_start").and_then(|x| x.as_u64()).unwrap_or(0),
        since: v.get("since").and_then(|x| x.as_f64()).unwrap_or(0.0),
        language: v
            .get("language")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

fn proc_start(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = text.rfind(')')?;
    text[close + 1..].split_whitespace().nth(19)?.parse().ok()
}

fn classify(state: &State, alive: bool) -> View {
    if !alive {
        return View::Hidden;
    }
    match state.state.as_str() {
        "recording" => View::Recording,
        "transcribing" => View::Transcribing,
        _ => View::Hidden,
    }
}

/// The parsed file and the (mtime, size) it was parsed from.
pub struct Reader {
    path: PathBuf,
    seen: Option<(SystemTime, u64)>,
    state: Option<State>,
}

impl Reader {
    pub fn new() -> Self {
        Self::at(state_path())
    }

    fn at(path: PathBuf) -> Self {
        Self {
            path,
            seen: None,
            state: None,
        }
    }

    fn refresh(&mut self) {
        let path = self.path.clone();
        let meta = std::fs::metadata(&path)
            .ok()
            .map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()));
        if meta.is_none() {
            self.seen = None;
            self.state = None;
            return;
        }
        if meta == self.seen {
            return;
        }
        self.seen = meta;
        self.state = std::fs::read_to_string(&path).ok().and_then(|t| parse(&t));
    }

    pub fn view(&mut self) -> View {
        self.refresh();
        match &self.state {
            Some(s) => {
                let alive = s.pid != 0
                    && proc_start(s.pid)
                        .is_some_and(|start| s.pid_start == 0 || start == s.pid_start);
                classify(s, alive)
            }
            None => View::Hidden,
        }
    }

    pub fn detail(&mut self) -> Vec<String> {
        let view = self.view();
        let Some(s) = self.state.clone() else {
            return vec!["NIRUDICTATE is not running".into()];
        };
        let language = match s.language.as_str() {
            "sv" => "Swedish",
            "en" => "English",
            other => other,
        };
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let secs = (now - s.since).max(0.0) as u64;
        let mut lines = Vec::new();
        match view {
            View::Recording => {
                lines.push(format!(
                    "recording ({language}), {:02}:{:02}",
                    secs / 60,
                    secs % 60
                ));
                lines.push("right-click: stop and insert the text".into());
            }
            View::Transcribing => lines.push(format!("transcribing ({language})")),
            View::Hidden => lines.push(format!("idle ({language})")),
        }
        lines.push("middle-click: NIRUDICTATE settings".into());
        lines
    }
}

pub fn text(view: View) -> &'static str {
    match view {
        View::Recording => "●",
        // A different shape as well as a different colour.
        View::Transcribing => "…",
        View::Hidden => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_version_one_contract() {
        let s = parse(
            r#"{"version":1,"state":"recording","pid":42,"pid_start":7,"since":10.5,"language":"sv"}"#,
        )
        .unwrap();
        assert_eq!(s.state, "recording");
        assert_eq!((s.pid, s.pid_start), (42, 7));
        assert_eq!(s.language, "sv");
        assert!(parse(r#"{"version":2,"state":"recording"}"#).is_none());
        assert!(parse("not json").is_none());
    }

    #[test]
    fn only_a_live_service_shows_anything() {
        let state = |name: &str| State {
            state: name.into(),
            pid: 1,
            ..State::default()
        };
        assert_eq!(classify(&state("recording"), true), View::Recording);
        assert_eq!(classify(&state("transcribing"), true), View::Transcribing);
        assert_eq!(classify(&state("idle"), true), View::Hidden);
        assert_eq!(classify(&state("recording"), false), View::Hidden);
        assert_eq!(text(View::Hidden), "");
        assert_ne!(text(View::Recording), text(View::Transcribing));
    }

    #[test]
    fn a_missing_file_is_hidden() {
        let dir = std::env::temp_dir().join(format!("nirubar-dictation-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("nirudictate")).unwrap();
        let mut r = Reader::at(dir.join("nirudictate/state.json"));
        assert_eq!(r.view(), View::Hidden);
        // Our own PID is alive; its start time matches.
        let me = std::process::id();
        let start = proc_start(me).unwrap();
        std::fs::write(
            dir.join("nirudictate/state.json"),
            format!(r#"{{"version":1,"state":"transcribing","pid":{me},"pid_start":{start},"since":0,"language":"en"}}"#),
        )
        .unwrap();
        assert_eq!(r.view(), View::Transcribing);
        std::fs::remove_file(dir.join("nirudictate/state.json")).unwrap();
        assert_eq!(r.view(), View::Hidden);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
