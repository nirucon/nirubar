//! Read-only power-profile snapshots through the suite's shared CLI.
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub active: Option<String>,
    pub source: String,
    pub profiles: Vec<String>,
    pub error: Option<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            active: None,
            source: "unknown".into(),
            profiles: Vec::new(),
            error: Some("checking power profiles".into()),
        }
    }
}

fn parse(text: &str) -> Snapshot {
    let value = |key: &str| {
        text.lines()
            .filter_map(|line| line.split_once('='))
            .find_map(|(name, value)| (name == key).then_some(value))
    };
    if value("state") != Some("ready") {
        return Snapshot {
            error: Some(
                value("reason")
                    .unwrap_or("power profiles unavailable")
                    .into(),
            ),
            ..Snapshot::default()
        };
    }
    let mut profiles = Vec::new();
    for line in text.lines() {
        if let Some(profile) = line.strip_prefix("profile=") {
            let profile = profile.trim();
            if !profile.is_empty() && !profiles.iter().any(|item| item == profile) {
                profiles.push(profile.to_string());
            }
        }
    }
    Snapshot {
        active: value("active")
            .filter(|v| !v.is_empty())
            .map(str::to_string),
        source: value("power").unwrap_or("unknown").to_string(),
        profiles,
        error: None,
    }
}

fn query() -> Snapshot {
    match crate::probe::run(
        "niructl",
        &["powerprofile", "status"],
        &[],
        Duration::from_secs(6),
        16 * 1024,
    ) {
        Some(out) if out.status.success() => parse(&String::from_utf8_lossy(&out.stdout)),
        _ => Snapshot {
            error: Some("niructl powerprofile status did not respond".into()),
            ..Snapshot::default()
        },
    }
}

pub struct Monitor {
    sample: crate::module::Cached<Snapshot>,
}

impl Monitor {
    pub fn new() -> Self {
        Self {
            sample: crate::module::Cached::new(Duration::from_secs(2), || Some(query())),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.sample.get().unwrap_or_default()
    }
}

pub fn short(profile: &str) -> &str {
    match profile {
        "performance" => "perf",
        "balanced" => "bal",
        "power-saver" => "save",
        other => other,
    }
}

pub fn text(snapshot: &Snapshot, icon: &str) -> String {
    if snapshot.error.is_some() {
        return String::new();
    }
    match snapshot.active.as_deref() {
        Some(profile) if !profile.is_empty() => format!("{icon} {}", short(profile)),
        _ => String::new(),
    }
}

pub fn detail(snapshot: &Snapshot) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(error) = &snapshot.error {
        lines.push(format!("unavailable: {error}"));
    } else {
        lines.push(format!(
            "active: {}",
            snapshot.active.as_deref().unwrap_or("unknown")
        ));
        lines.push(format!("power source: {}", snapshot.source));
        lines.push(format!("available: {}", snapshot.profiles.join(", ")));
    }
    lines.extend([
        String::new(),
        "right-click: next profile".into(),
        "middle-click: power profile selector".into(),
    ]);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_values_are_not_invented() {
        let snapshot = parse("state=ready\nactive=balanced\npower=ac\nprofile=performance\nprofile=balanced\nprofile=power-saver\n");
        assert_eq!(text(&snapshot, "p"), "p bal");
        assert!(detail(&snapshot).contains(&"power source: ac".into()));
        assert_eq!(text(&parse("state=unavailable\nreason=missing\n"), "p"), "");
    }
}
