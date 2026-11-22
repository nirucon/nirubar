//! Read-only microphone snapshots. The default source, its gain and mute
//! come from the shared PipeWire hub (`crate::privacy::Hub`) as events, with
//! the same states as `niructl audio mic-status`. Without `pw-dump` the suite's
//! audio CLI is asked every two seconds instead, off the X11 thread and with
//! a deadline; painting only reads memory.
use std::time::Duration;

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
    hub: std::sync::Arc<crate::privacy::Hub>,
    fallback: Option<crate::module::Cached<Snapshot>>,
}

/// The hub's view of the default input, in this module's states.
fn from_hub(input: &crate::privacy::Source) -> Snapshot {
    match input {
        crate::privacy::Source::Ready(level) => Snapshot::Ready {
            name: level.name.clone(),
            percent: level.percent,
            muted: level.muted,
        },
        crate::privacy::Source::NoInput => Snapshot::NoInput,
        crate::privacy::Source::Unavailable(reason) => Snapshot::Unavailable(reason.clone()),
    }
}

impl Monitor {
    pub fn new() -> Self {
        Self {
            hub: crate::privacy::Hub::get(),
            fallback: None,
        }
    }

    pub fn snapshot(&mut self) -> Snapshot {
        let audio = self.hub.snapshot();
        if !audio.audio_missing {
            self.fallback = None;
            return audio
                .source
                .as_ref()
                .map(from_hub)
                .unwrap_or_else(|| Snapshot::Unavailable("checking microphone".into()));
        }
        self.fallback
            .get_or_insert_with(|| {
                crate::module::Cached::new(Duration::from_secs(2), || Some(query()))
            })
            .get()
            .unwrap_or_else(|| Snapshot::Unavailable("checking microphone".into()))
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
