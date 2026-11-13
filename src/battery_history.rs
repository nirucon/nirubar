//! A small battery charge history for the battery panel's 24 h graph.
//!
//! A background thread samples the charge every 10 minutes into
//! `$XDG_STATE_HOME/nirubar/battery-history`, one `epoch percent` pair per
//! line. The file keeps 48 hours, is size-bounded and is replaced
//! atomically. Bars on several monitors share it: a sample younger than
//! 9 minutes means another bar already wrote one.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

const EVERY: Duration = Duration::from_secs(600);
/// A sample this recent makes another one unnecessary (several bars).
const MIN_GAP: i64 = 540;
const KEEP: i64 = 48 * 3600;
/// 48 h at one sample per 10 min is 288 lines; leave room for restarts.
const MAX_LINES: usize = 600;
const HOURS: i64 = 24;
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub type Sample = (i64, u8);

pub fn path() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
        })
        .join("nirubar/battery-history")
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Read `epoch percent` lines; anything else is skipped.
pub fn parse(text: &str) -> Vec<Sample> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let at = parts.next()?.parse::<i64>().ok()?;
            let pct = parts.next()?.parse::<u8>().ok().filter(|p| *p <= 100)?;
            parts.next().is_none().then_some((at, pct))
        })
        .collect()
}

/// Keep the last 48 hours in time order, newest `MAX_LINES` at most. Samples
/// from the future (a clock that jumped back) are dropped.
pub fn prune(mut samples: Vec<Sample>, now: i64) -> Vec<Sample> {
    samples.retain(|(at, _)| *at > now - KEEP && *at <= now + 60);
    samples.sort_by_key(|(at, _)| *at);
    samples.dedup_by_key(|(at, _)| *at);
    if samples.len() > MAX_LINES {
        samples.drain(..samples.len() - MAX_LINES);
    }
    samples
}

pub fn serialize(samples: &[Sample]) -> String {
    samples
        .iter()
        .map(|(at, pct)| format!("{at} {pct}\n"))
        .collect()
}

pub fn load(path: &Path, now: i64) -> Vec<Sample> {
    std::fs::read_to_string(path)
        .map(|text| prune(parse(&text), now))
        .unwrap_or_default()
}

/// Add one sample unless a fresh one exists. Written to a temporary file and
/// renamed, so a reader never sees half a file.
pub fn record(path: &Path, now: i64, percent: u8) -> std::io::Result<bool> {
    let mut samples = load(path, now);
    if samples
        .last()
        .is_some_and(|(at, _)| now - *at < MIN_GAP && *at <= now)
    {
        return Ok(false);
    }
    samples.push((now, percent.min(100)));
    let samples = prune(samples, now);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, serialize(&samples))?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(true)
}

/// One block character per hour for the last 24 hours, oldest first; the
/// last cell is the current hour. An hour without samples is a space.
pub fn graph(samples: &[Sample], now: i64) -> String {
    (0..HOURS)
        .map(|i| {
            let start = now - (HOURS - i) * 3600;
            let end = start + 3600;
            let hour: Vec<u32> = samples
                .iter()
                .filter(|(at, _)| *at > start && *at <= end)
                .map(|(_, p)| u32::from(*p))
                .collect();
            if hour.is_empty() {
                return ' ';
            }
            let avg = hour.iter().sum::<u32>() / hour.len() as u32;
            BLOCKS[((avg * 7 + 50) / 100).min(7) as usize]
        })
        .collect()
}

/// The graph line for the panel, or `None` before the first sample.
pub fn panel_line() -> Option<String> {
    let at = now();
    let samples = load(&path(), at);
    (!samples.is_empty()).then(|| format!("last 24 h: {}", graph(&samples, at)))
}

/// Start sampling once per process; does nothing on a machine without a
/// battery.
pub fn start() {
    // Unit tests build modules; they must not write the real history.
    if cfg!(test) {
        return;
    }
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("nirubar-battery-history".into())
            .spawn(|| loop {
                let Some(battery) = crate::status::battery() else {
                    return;
                };
                if let Err(e) = record(&path(), now(), battery.percent) {
                    eprintln!("nirubar: cannot write the battery history: {e}");
                }
                std::thread::sleep(EVERY);
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skips_damaged_lines() {
        let s = parse("100 50\nbad\n200 101\n300 60 extra\n400 70\n\n");
        assert_eq!(s, vec![(100, 50), (400, 70)]);
    }

    #[test]
    fn prune_keeps_48_hours_in_order_and_bounded() {
        let now = 1_000_000;
        let s = prune(
            vec![
                (now - KEEP - 1, 10),
                (now - 10, 30),
                (now - 20, 20),
                (now - 10, 31),
                (now + 3600, 99),
            ],
            now,
        );
        assert_eq!(s, vec![(now - 20, 20), (now - 10, 30)]);
        let many: Vec<Sample> = (0..1000).map(|i| (now - 1000 + i, 50)).collect();
        let kept = prune(many, now);
        assert_eq!(kept.len(), MAX_LINES);
        assert_eq!(kept.last().unwrap().0, now - 1);
    }

    #[test]
    fn graph_has_one_cell_per_hour_and_spaces_for_gaps() {
        let now = 100 * 3600;
        assert_eq!(graph(&[], now), " ".repeat(24));
        let s = vec![
            (now - 23 * 3600 - 10, 0), // first hour
            (now - 30, 100),           // current hour
            (now - 60, 100),
            (now - 5 * 3600 - 1, 50),
        ];
        let g: Vec<char> = graph(&s, now).chars().collect();
        assert_eq!(g.len(), 24);
        assert_eq!(g[0], '▁');
        assert_eq!(g[23], '█');
        assert_eq!(g[18], '▅');
        assert_eq!(g.iter().filter(|c| **c == ' ').count(), 21);
    }

    #[test]
    fn record_is_atomic_and_skips_fresh_samples() {
        let dir = std::env::temp_dir().join(format!("nirubar-hist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("nirubar/battery-history");
        let now = 2_000_000;
        assert!(record(&file, now, 80).unwrap());
        assert!(!record(&file, now + 60, 79).unwrap());
        assert!(record(&file, now + 600, 78).unwrap());
        assert_eq!(load(&file, now + 600), vec![(now, 80), (now + 600, 78)]);
        assert_eq!(
            std::fs::read_dir(file.parent().unwrap()).unwrap().count(),
            1
        );
        // Two days later the old samples are gone.
        assert!(record(&file, now + 3 * 86_400, 50).unwrap());
        assert_eq!(load(&file, now + 3 * 86_400), vec![(now + 3 * 86_400, 50)]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
