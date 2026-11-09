//! Time zones for calendar events, without a time-zone crate.
//!
//! iCalendar events name their zone (`TZID=Europe/Stockholm`). The system
//! zone database already has every rule, so this module reads the compiled
//! TZif file for that name and, for instants past its last transition, the
//! POSIX rule in its footer. Modern "slim" zone files rely on that footer for
//! every future date, so both parts are needed.
//!
//! Display always uses the system zone through `chrono::Local`.

use chrono::{Datelike, NaiveDate, NaiveDateTime};
use std::collections::HashMap;

/// A zone's rules: transitions, then a POSIX rule for later instants.
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    /// (UTC instant, index into `types`), sorted by instant.
    transitions: Vec<(i64, usize)>,
    /// UTC offsets in seconds.
    types: Vec<i32>,
    rule: Option<Posix>,
}

#[derive(Clone, Debug, PartialEq)]
struct Posix {
    /// Standard offset east of UTC, in seconds.
    std: i32,
    dst: Option<Dst>,
}

#[derive(Clone, Debug, PartialEq)]
struct Dst {
    offset: i32,
    start: (Rule, i32),
    end: (Rule, i32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Rule {
    /// Mm.w.d: day d (0 = Sunday) of week w (5 = last) of month m.
    Month(u32, u32, u32),
    /// Jn: day 1..=365, February 29 never counted.
    Julian(u32),
    /// n: zero-based day of the year, leap days counted.
    Day(u32),
}

impl Zone {
    pub fn utc() -> Self {
        Self::fixed(0)
    }

    pub fn fixed(offset: i32) -> Self {
        Self {
            transitions: Vec::new(),
            types: vec![offset],
            rule: None,
        }
    }

    /// A zone from a POSIX TZ string such as `CET-1CEST,M3.5.0,M10.5.0/3`.
    #[cfg(test)]
    pub fn posix(text: &str) -> Option<Self> {
        let rule = parse_posix(text)?;
        Some(Self {
            transitions: Vec::new(),
            types: vec![rule.std],
            rule: Some(rule),
        })
    }

    /// Parse a compiled TZif file (versions 1 to 4).
    pub fn tzif(data: &[u8]) -> Option<Self> {
        let header = |at: usize| -> Option<[usize; 6]> {
            if data.get(at..at + 4)? != b"TZif" {
                return None;
            }
            let mut counts = [0usize; 6];
            for (i, count) in counts.iter_mut().enumerate() {
                let start = at + 20 + i * 4;
                let bytes: [u8; 4] = data.get(start..start + 4)?.try_into().ok()?;
                *count = u32::from_be_bytes(bytes) as usize;
            }
            Some(counts)
        };
        let version = *data.get(4)?;
        let [isut, isstd, leap, time, kinds, chars] = header(0)?;
        let v1_len = time * 5 + kinds * 6 + chars + leap * 8 + isstd + isut;
        let (start, width) = if version >= b'2' {
            (44 + v1_len, 8usize)
        } else {
            (0, 4usize)
        };
        let [isut, isstd, leap, time, kinds, chars] = header(start)?;
        let mut at = start + 44;
        let mut transitions = Vec::with_capacity(time);
        for i in 0..time {
            let raw = data.get(at + i * width..at + (i + 1) * width)?;
            let instant = if width == 8 {
                i64::from_be_bytes(raw.try_into().ok()?)
            } else {
                i64::from(i32::from_be_bytes(raw.try_into().ok()?))
            };
            transitions.push(instant);
        }
        at += time * width;
        let indices = data.get(at..at + time)?;
        at += time;
        let mut types = Vec::with_capacity(kinds);
        for i in 0..kinds {
            let raw: [u8; 4] = data.get(at + i * 6..at + i * 6 + 4)?.try_into().ok()?;
            types.push(i32::from_be_bytes(raw));
        }
        at += kinds * 6 + chars + leap * (width + 4) + isstd + isut;
        if types.is_empty() {
            return None;
        }
        let transitions = transitions
            .into_iter()
            .zip(indices.iter().map(|&i| usize::from(i)))
            .filter(|(_, i)| *i < types.len())
            .collect();
        let rule = if version >= b'2' {
            data.get(at..)
                .and_then(|rest| std::str::from_utf8(rest).ok())
                .and_then(|rest| rest.trim_matches('\n').lines().next())
                .and_then(parse_posix)
        } else {
            None
        };
        Some(Self {
            transitions,
            types,
            rule,
        })
    }

    /// Offset east of UTC, in seconds, at a UTC instant.
    pub fn offset_at(&self, utc: i64) -> i32 {
        if let Some(&(last, _)) = self.transitions.last() {
            if utc >= last {
                if let Some(rule) = &self.rule {
                    return rule.offset_at(utc);
                }
            }
        } else if let Some(rule) = &self.rule {
            return rule.offset_at(utc);
        }
        match self.transitions.partition_point(|(t, _)| *t <= utc) {
            0 => self.types[0],
            n => self.types[self.transitions[n - 1].1],
        }
    }

    /// The UTC instant of a wall-clock time in this zone. A time that falls in
    /// a spring-forward gap moves forward by the gap; a repeated autumn hour
    /// takes its first occurrence.
    pub fn to_utc(&self, wall: NaiveDateTime) -> i64 {
        let naive = wall.and_utc().timestamp();
        let first = naive - i64::from(self.offset_at(naive));
        let second = naive - i64::from(self.offset_at(first));
        let candidates = [
            naive - i64::from(self.offset_at(naive - 86_400)),
            first,
            second,
        ];
        // Prefer the earliest candidate that maps back to the same wall time.
        candidates
            .iter()
            .copied()
            .filter(|&t| t + i64::from(self.offset_at(t)) == naive)
            .min()
            .unwrap_or(candidates[0])
    }
}

impl Posix {
    fn offset_at(&self, utc: i64) -> i32 {
        let Some(dst) = &self.dst else {
            return self.std;
        };
        let year = chrono::DateTime::from_timestamp(utc + i64::from(self.std), 0)
            .map(|d| d.year())
            .unwrap_or(1970);
        let start = transition(year, dst.start) - i64::from(self.std);
        let end = transition(year, dst.end) - i64::from(dst.offset);
        let in_dst = if start < end {
            utc >= start && utc < end
        } else {
            !(utc >= end && utc < start)
        };
        if in_dst {
            dst.offset
        } else {
            self.std
        }
    }
}

/// Local seconds since the epoch of a rule's transition in `year`.
fn transition(year: i32, (rule, time): (Rule, i32)) -> i64 {
    let date = match rule {
        Rule::Month(month, week, weekday) => {
            let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_default();
            let lead = (weekday + 7 - first.weekday().num_days_from_sunday()) % 7;
            let mut day = 1 + lead + (week.saturating_sub(1)) * 7;
            let days = days_in_month(year, month);
            while day > days {
                day -= 7;
            }
            NaiveDate::from_ymd_opt(year, month, day).unwrap_or(first)
        }
        Rule::Julian(n) => {
            let base = NaiveDate::from_ymd_opt(year, 1, 1).unwrap_or_default();
            let mut ordinal = n.clamp(1, 365);
            if is_leap(year) && ordinal >= 60 {
                ordinal += 1;
            }
            base + chrono::Duration::days(i64::from(ordinal) - 1)
        }
        Rule::Day(n) => {
            NaiveDate::from_ymd_opt(year, 1, 1).unwrap_or_default()
                + chrono::Duration::days(i64::from(n.min(365)))
        }
    };
    date.and_hms_opt(0, 0, 0)
        .unwrap_or_default()
        .and_utc()
        .timestamp()
        + i64::from(time)
}

pub fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 30,
    }
}

/// Parse `[+-]hh[:mm[:ss]]` into seconds.
fn parse_hms(text: &str) -> Option<(i32, &str)> {
    let (sign, rest) = match text.as_bytes().first()? {
        b'-' => (-1, &text[1..]),
        b'+' => (1, &text[1..]),
        _ => (1, text),
    };
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == ':'))
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    let mut total = 0i32;
    for (i, part) in rest[..end].split(':').enumerate() {
        let n: i32 = part.parse().ok()?;
        total += n * [3600, 60, 1].get(i)?;
    }
    Some((sign * total, &rest[end..]))
}

fn parse_name(text: &str) -> Option<&str> {
    if let Some(rest) = text.strip_prefix('<') {
        let end = rest.find('>')?;
        return Some(&rest[end + 1..]);
    }
    let end = text
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(text.len());
    (end >= 3).then(|| &text[end..])
}

fn parse_rule(text: &str) -> Option<(Rule, i32)> {
    let (date, time) = match text.split_once('/') {
        Some((d, t)) => (d, parse_hms(t)?.0),
        None => (text, 7200),
    };
    let rule = if let Some(m) = date.strip_prefix('M') {
        let mut parts = m.split('.').map(|p| p.parse::<u32>().ok());
        let (month, week, day) = (parts.next()??, parts.next()??, parts.next()??);
        if !(1..=12).contains(&month) || !(1..=5).contains(&week) || day > 6 {
            return None;
        }
        Rule::Month(month, week, day)
    } else if let Some(j) = date.strip_prefix('J') {
        Rule::Julian(j.parse().ok()?)
    } else {
        Rule::Day(date.parse().ok()?)
    };
    Some((rule, time))
}

fn parse_posix(text: &str) -> Option<Posix> {
    let rest = parse_name(text.trim())?;
    let (west, rest) = parse_hms(rest)?;
    let std = -west;
    if rest.is_empty() {
        return Some(Posix { std, dst: None });
    }
    let rest = parse_name(rest)?;
    let (offset, rest) = match rest.chars().next() {
        Some(c) if c == '+' || c == '-' || c.is_ascii_digit() => {
            let (west, rest) = parse_hms(rest)?;
            (-west, rest)
        }
        _ => (std + 3600, rest),
    };
    let rest = rest.strip_prefix(',')?;
    let (start, end) = rest.split_once(',')?;
    Some(Posix {
        std,
        dst: Some(Dst {
            offset,
            start: parse_rule(start)?,
            end: parse_rule(end)?,
        }),
    })
}

/// Common Windows zone names used by Exchange and Outlook feeds.
fn windows_zone(name: &str) -> Option<&'static str> {
    Some(match name {
        "W. Europe Standard Time" => "Europe/Berlin",
        "Central Europe Standard Time" => "Europe/Budapest",
        "Central European Standard Time" => "Europe/Warsaw",
        "Romance Standard Time" => "Europe/Paris",
        "GMT Standard Time" => "Europe/London",
        "Greenwich Standard Time" => "Atlantic/Reykjavik",
        "FLE Standard Time" => "Europe/Helsinki",
        "E. Europe Standard Time" => "Europe/Chisinau",
        "Russian Standard Time" => "Europe/Moscow",
        "Eastern Standard Time" => "America/New_York",
        "Central Standard Time" => "America/Chicago",
        "Mountain Standard Time" => "America/Denver",
        "Pacific Standard Time" => "America/Los_Angeles",
        "Tokyo Standard Time" => "Asia/Tokyo",
        "China Standard Time" => "Asia/Shanghai",
        "India Standard Time" => "Asia/Kolkata",
        "AUS Eastern Standard Time" => "Australia/Sydney",
        "UTC" | "Coordinated Universal Time" => "UTC",
        _ => return None,
    })
}

/// A zone name that can safely be joined to a zone directory.
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < 64
        && !name.starts_with('/')
        && !name
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
}

fn load(name: &str) -> Option<Zone> {
    if matches!(name, "UTC" | "Etc/UTC" | "GMT" | "Etc/GMT" | "Z" | "Zulu") {
        return Some(Zone::utc());
    }
    if !safe_name(name) {
        return None;
    }
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("TZDIR") {
        dirs.push(std::path::PathBuf::from(dir));
    }
    for dir in [
        "/usr/share/zoneinfo",
        "/usr/lib/zoneinfo",
        "/usr/share/lib/zoneinfo",
    ] {
        dirs.push(dir.into());
    }
    dirs.iter().find_map(|dir| {
        let data = std::fs::read(dir.join(name)).ok()?;
        (data.len() < 1 << 20).then_some(())?;
        Zone::tzif(&data)
    })
}

/// Resolve a TZID as it appears in feeds: IANA names, names behind a vendor
/// prefix such as `/mozilla.org/20050126_1/Europe/Berlin`, quoted names and
/// common Windows names.
pub fn resolve(tzid: &str) -> Option<Zone> {
    let name = tzid.trim().trim_matches('"');
    if let Some(zone) = load(name) {
        return Some(zone);
    }
    if let Some(mapped) = windows_zone(name) {
        return load(mapped);
    }
    // The last two path components usually are the IANA name.
    let parts: Vec<&str> = name.split('/').filter(|p| !p.is_empty()).collect();
    for take in [3, 2] {
        if parts.len() > take {
            if let Some(zone) = load(&parts[parts.len() - take..].join("/")) {
                return Some(zone);
            }
        }
    }
    None
}

/// The zones an expansion needs, and how floating (zone-less) times map.
pub struct Zones {
    cache: HashMap<String, Option<Zone>>,
    /// `None` uses the system zone; tests pin one.
    local: Option<Zone>,
}

impl Zones {
    pub fn system() -> Self {
        Self {
            cache: HashMap::new(),
            local: None,
        }
    }

    #[cfg(test)]
    pub fn pinned(local: Zone) -> Self {
        Self {
            cache: HashMap::new(),
            local: Some(local),
        }
    }

    #[cfg(test)]
    pub fn insert(&mut self, name: &str, zone: Zone) {
        self.cache.insert(name.to_string(), Some(zone));
    }

    /// UTC instant of a wall time in `tzid`, or in local time when the zone is
    /// absent or unknown.
    pub fn wall_to_utc(&mut self, wall: NaiveDateTime, tzid: Option<&str>) -> i64 {
        if let Some(name) = tzid {
            let zone = self
                .cache
                .entry(name.to_string())
                .or_insert_with(|| resolve(name));
            if let Some(zone) = zone {
                return zone.to_utc(wall);
            }
        }
        self.local_to_utc(wall)
    }

    pub fn local_to_utc(&self, wall: NaiveDateTime) -> i64 {
        match &self.local {
            Some(zone) => zone.to_utc(wall),
            None => {
                use chrono::TimeZone;
                match chrono::Local.from_local_datetime(&wall) {
                    chrono::LocalResult::Single(t) => t.timestamp(),
                    chrono::LocalResult::Ambiguous(t, _) => t.timestamp(),
                    chrono::LocalResult::None => {
                        // A spring-forward gap: the hour after it.
                        chrono::Local
                            .from_local_datetime(&(wall + chrono::Duration::hours(1)))
                            .earliest()
                            .map(|t| t.timestamp() - 3600)
                            .unwrap_or_else(|| wall.and_utc().timestamp())
                    }
                }
            }
        }
    }

    /// Local wall time of a UTC instant.
    pub fn to_local(&self, utc: i64) -> NaiveDateTime {
        match &self.local {
            Some(zone) => chrono::DateTime::from_timestamp(utc + i64::from(zone.offset_at(utc)), 0)
                .unwrap_or_default()
                .naive_utc(),
            None => chrono::DateTime::from_timestamp(utc, 0)
                .unwrap_or_default()
                .with_timezone(&chrono::Local)
                .naive_local(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    fn utc(s: &str) -> i64 {
        at(s).and_utc().timestamp()
    }

    #[test]
    fn posix_rules_follow_european_daylight_time() {
        let cet = Zone::posix("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        assert_eq!(cet.offset_at(utc("2026-01-15 12:00")), 3600);
        assert_eq!(cet.offset_at(utc("2026-07-15 12:00")), 7200);
        // 2026-03-29 01:00 UTC is the switch; 2026-10-25 01:00 UTC back.
        assert_eq!(cet.offset_at(utc("2026-03-29 00:59")), 3600);
        assert_eq!(cet.offset_at(utc("2026-03-29 01:00")), 7200);
        assert_eq!(cet.offset_at(utc("2026-10-25 00:59")), 7200);
        assert_eq!(cet.offset_at(utc("2026-10-25 01:00")), 3600);
    }

    #[test]
    fn wall_times_convert_across_the_switch() {
        let cet = Zone::posix("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        assert_eq!(cet.to_utc(at("2026-11-09 09:00")), utc("2026-11-09 08:00"));
        assert_eq!(cet.to_utc(at("2026-06-01 09:00")), utc("2026-06-01 07:00"));
        // The repeated hour takes the first (summer) occurrence.
        assert_eq!(cet.to_utc(at("2026-10-25 02:30")), utc("2026-10-25 00:30"));
        // A time in the gap still lands in the right hour region.
        // A time in the spring-forward gap moves forward by the gap.
        assert_eq!(cet.to_utc(at("2026-03-29 02:30")), utc("2026-03-29 01:30"));
    }

    #[test]
    fn american_and_southern_rules() {
        let ny = Zone::posix("EST5EDT,M3.2.0,M11.1.0").unwrap();
        assert_eq!(ny.offset_at(utc("2026-07-01 12:00")), -4 * 3600);
        assert_eq!(ny.offset_at(utc("2026-12-01 12:00")), -5 * 3600);
        let sydney = Zone::posix("AEST-10AEDT,M10.1.0,M4.1.0/3").unwrap();
        assert_eq!(sydney.offset_at(utc("2026-01-15 00:00")), 11 * 3600);
        assert_eq!(sydney.offset_at(utc("2026-07-15 00:00")), 10 * 3600);
        let fixed = Zone::posix("<+0530>-5:30").unwrap();
        assert_eq!(fixed.offset_at(0), 5 * 3600 + 1800);
    }

    #[test]
    fn unsafe_zone_names_are_refused() {
        assert!(!safe_name("../../etc/passwd"));
        assert!(!safe_name("/etc/localtime"));
        assert!(safe_name("America/Argentina/Buenos_Aires"));
        assert!(resolve("../../etc/passwd").is_none());
        assert_eq!(resolve("UTC"), Some(Zone::utc()));
    }

    #[test]
    fn system_zone_file_parses_when_present() {
        let Ok(data) = std::fs::read("/usr/share/zoneinfo/Europe/Stockholm") else {
            return; // minimal containers have no zone database
        };
        let zone = Zone::tzif(&data).expect("TZif parses");
        assert_eq!(zone.offset_at(utc("2026-01-15 12:00")), 3600);
        assert_eq!(zone.offset_at(utc("2026-07-15 12:00")), 7200);
        assert_eq!(zone.offset_at(utc("2040-07-15 12:00")), 7200);
        assert_eq!(zone.offset_at(utc("1990-01-15 12:00")), 3600);
        assert!(resolve("/mozilla.org/20050126_1/Europe/Stockholm").is_some());
        assert!(resolve("W. Europe Standard Time").is_some());
    }
}
