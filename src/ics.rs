//! A small iCalendar (RFC 5545) reader for read-only calendar feeds.
//!
//! It reads what a calendar popup needs: VEVENT start, end, summary,
//! location, status, recurrence rules, exceptions and overridden instances.
//! Everything else in a feed (alarms, attendees, free/busy) is skipped.

use crate::tz::{days_in_month, Zones};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Weekday};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A start or end value as the feed wrote it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum When {
    /// `VALUE=DATE`: an all-day value.
    Date(NaiveDate),
    /// A UTC date-time (`...Z`).
    Utc(NaiveDateTime),
    /// A wall time in a named zone, or floating local time when `None`.
    Wall(NaiveDateTime, Option<String>),
}

const STAMP: &str = "%Y%m%dT%H%M%S";

impl From<When> for String {
    fn from(w: When) -> String {
        match w {
            When::Date(d) => format!("D{}", d.format("%Y%m%d")),
            When::Utc(t) => format!("U{}", t.format(STAMP)),
            When::Wall(t, None) => format!("F{}", t.format(STAMP)),
            When::Wall(t, Some(z)) => format!("Z{}|{z}", t.format(STAMP)),
        }
    }
}

impl TryFrom<String> for When {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        let bad = || format!("bad cached time {s:?}");
        let rest = s.get(1..).ok_or_else(bad)?;
        let time = |t: &str| NaiveDateTime::parse_from_str(t, STAMP).map_err(|_| bad());
        match s.as_bytes().first() {
            Some(b'D') => NaiveDate::parse_from_str(rest, "%Y%m%d")
                .map(When::Date)
                .map_err(|_| bad()),
            Some(b'U') => time(rest).map(When::Utc),
            Some(b'F') => time(rest).map(|t| When::Wall(t, None)),
            Some(b'Z') => {
                let (t, z) = rest.split_once('|').ok_or_else(bad)?;
                Ok(When::Wall(time(t)?, Some(z.to_string())))
            }
            _ => Err(bad()),
        }
    }
}

impl When {
    fn is_date(&self) -> bool {
        matches!(self, When::Date(_))
    }

    /// The wall time the value is expressed in (midnight for a date).
    fn wall(&self) -> NaiveDateTime {
        match self {
            When::Date(d) => d.and_time(NaiveTime::MIN),
            When::Utc(t) | When::Wall(t, _) => *t,
        }
    }

    /// The same kind of value at another wall time.
    fn with_wall(&self, wall: NaiveDateTime) -> When {
        match self {
            When::Date(_) => When::Date(wall.date()),
            When::Utc(_) => When::Utc(wall),
            When::Wall(_, z) => When::Wall(wall, z.clone()),
        }
    }

    fn utc(&self, zones: &mut Zones) -> i64 {
        match self {
            When::Date(d) => zones.local_to_utc(d.and_time(NaiveTime::MIN)),
            When::Utc(t) => t.and_utc().timestamp(),
            When::Wall(t, z) => zones.wall_to_utc(*t, z.as_deref()),
        }
    }
}

/// One VEVENT, reduced to what the popup shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub uid: String,
    pub summary: String,
    #[serde(default)]
    pub location: String,
    pub start: When,
    #[serde(default)]
    pub end: Option<When>,
    /// Seconds, from DURATION, when DTEND is absent.
    #[serde(default)]
    pub duration: Option<i64>,
    #[serde(default)]
    pub rrule: Option<String>,
    #[serde(default)]
    pub exdates: Vec<When>,
    #[serde(default)]
    pub rdates: Vec<When>,
    #[serde(default)]
    pub recurrence_id: Option<When>,
    /// Only kept for a cancelled override, which removes one instance.
    #[serde(default)]
    pub cancelled: bool,
}

/// Undo RFC 5545 line folding: a line that starts with a space or tab
/// continues the previous one.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix([' ', '\t']) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    lines
}

/// A content line: `NAME;PARAM=VALUE;...:value`.
struct Line {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Line {
    fn param(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

fn parse_line(line: &str) -> Option<Line> {
    let mut in_quote = false;
    let mut split = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            ':' if !in_quote => {
                split = Some(i);
                break;
            }
            _ => {}
        }
    }
    let split = split?;
    let (head, value) = (&line[..split], &line[split + 1..]);
    let mut parts = Vec::new();
    let mut current = String::new();
    in_quote = false;
    for c in head.chars() {
        match c {
            '"' => in_quote = !in_quote,
            ';' if !in_quote => parts.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    parts.push(current);
    let name = parts.first()?.trim().to_ascii_uppercase();
    let params = parts[1..]
        .iter()
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.trim().to_ascii_uppercase(), v.to_string()))
        })
        .collect();
    Some(Line {
        name,
        params,
        value: value.to_string(),
    })
}

/// TEXT unescaping: `\\`, `\;`, `\,` and `\n`.
pub fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push(' '),
            Some(other) => out.push(other),
            None => {}
        }
    }
    // A popup row is one line; collapse runs of whitespace.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value.get(..8)?, "%Y%m%d").ok()
}

/// Parse one DATE or DATE-TIME value with the line's VALUE and TZID.
fn parse_when(value: &str, tzid: Option<&str>, date_only: bool) -> Option<When> {
    let value = value.trim();
    if date_only || value.len() == 8 {
        return parse_date(value).map(When::Date);
    }
    let (body, utc) = match value.strip_suffix(['Z', 'z']) {
        Some(body) => (body, true),
        None => (value, false),
    };
    // Seconds are optional in the wild (`T0900`).
    let wall = NaiveDateTime::parse_from_str(body, STAMP)
        .or_else(|_| NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M"))
        .ok()?;
    Some(if utc {
        When::Utc(wall)
    } else {
        When::Wall(wall, tzid.map(|z| z.trim_matches('"').to_string()))
    })
}

fn parse_values(line: &Line) -> Vec<When> {
    let date_only = line
        .param("VALUE")
        .is_some_and(|v| v.eq_ignore_ascii_case("DATE"));
    if line
        .param("VALUE")
        .is_some_and(|v| v.eq_ignore_ascii_case("PERIOD"))
    {
        return line
            .value
            .split(',')
            .filter_map(|v| parse_when(v.split('/').next()?, line.param("TZID"), false))
            .collect();
    }
    line.value
        .split(',')
        .filter_map(|v| parse_when(v, line.param("TZID"), date_only))
        .collect()
}

/// `[+-]P[nW][nD][T[nH][nM][nS]]` in seconds.
pub fn parse_duration(value: &str) -> Option<i64> {
    let value = value.trim();
    let (sign, rest) = match value.as_bytes().first()? {
        b'-' => (-1, &value[1..]),
        b'+' => (1, &value[1..]),
        _ => (1, value),
    };
    let rest = rest.strip_prefix(['P', 'p'])?;
    let mut total = 0i64;
    let mut number = String::new();
    let mut time = false;
    for c in rest.chars() {
        match c.to_ascii_uppercase() {
            'T' => time = true,
            d if d.is_ascii_digit() => number.push(d),
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                total += n * match (unit, time) {
                    ('W', _) => 7 * 86_400,
                    ('D', _) => 86_400,
                    ('H', true) => 3_600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
        }
    }
    number.is_empty().then_some(sign * total)
}

/// Read every VEVENT in a feed. Cancelled events are dropped, except a
/// cancelled override, which is kept so it can remove its instance.
pub fn parse(text: &str) -> Vec<Event> {
    let mut events = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut current: Option<Event> = None;
    let mut cancelled = false;
    for raw in unfold(text) {
        let Some(line) = parse_line(&raw) else {
            continue;
        };
        match line.name.as_str() {
            "BEGIN" => {
                let kind = line.value.trim().to_ascii_uppercase();
                if kind == "VEVENT" && stack.last().is_some_and(|k| k == "VCALENDAR") {
                    current = Some(Event {
                        uid: String::new(),
                        summary: String::new(),
                        location: String::new(),
                        start: When::Date(NaiveDate::MIN),
                        end: None,
                        duration: None,
                        rrule: None,
                        exdates: Vec::new(),
                        rdates: Vec::new(),
                        recurrence_id: None,
                        cancelled: false,
                    });
                    cancelled = false;
                }
                stack.push(kind);
                continue;
            }
            "END" => {
                let kind = line.value.trim().to_ascii_uppercase();
                if let Some(pos) = stack.iter().rposition(|k| *k == kind) {
                    stack.truncate(pos);
                }
                if kind == "VEVENT" {
                    if let Some(mut event) = current.take() {
                        let started = event.start != When::Date(NaiveDate::MIN);
                        if started && (!cancelled || event.recurrence_id.is_some()) {
                            event.cancelled = cancelled;
                            events.push(event);
                        }
                    }
                }
                continue;
            }
            _ => {}
        }
        if stack.last().map(String::as_str) != Some("VEVENT") {
            continue;
        }
        let Some(event) = current.as_mut() else {
            continue;
        };
        let first = || parse_values(&line).into_iter().next();
        match line.name.as_str() {
            "UID" => event.uid = line.value.trim().to_string(),
            "SUMMARY" => event.summary = unescape(&line.value),
            "LOCATION" => event.location = unescape(&line.value),
            "STATUS" => cancelled = line.value.trim().eq_ignore_ascii_case("CANCELLED"),
            "DTSTART" => {
                if let Some(w) = first() {
                    event.start = w;
                }
            }
            "DTEND" => event.end = first(),
            "DURATION" => event.duration = parse_duration(&line.value),
            "RRULE" => event.rrule = Some(line.value.trim().to_string()),
            "EXDATE" => event.exdates.extend(parse_values(&line)),
            "RDATE" => event.rdates.extend(parse_values(&line)),
            "RECURRENCE-ID" => event.recurrence_id = first(),
            _ => {}
        }
    }
    events
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Debug)]
struct Rrule {
    freq: Freq,
    interval: u32,
    count: Option<u32>,
    until: Option<When>,
    /// (ordinal, weekday); ordinal 0 means every such weekday.
    by_day: Vec<(i32, Weekday)>,
    by_month_day: Vec<i32>,
    by_month: Vec<u32>,
    by_set_pos: Vec<i32>,
    week_start: Weekday,
}

fn weekday(code: &str) -> Option<Weekday> {
    Some(match code.to_ascii_uppercase().as_str() {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

/// Parse an RRULE. `None` means "not understood": the event is then shown
/// at its first instance only, rather than at invented times.
fn parse_rrule(text: &str) -> Option<Rrule> {
    let mut rule = Rrule {
        freq: Freq::Daily,
        interval: 1,
        count: None,
        until: None,
        by_day: Vec::new(),
        by_month_day: Vec::new(),
        by_month: Vec::new(),
        by_set_pos: Vec::new(),
        week_start: Weekday::Mon,
    };
    let mut freq = None;
    for part in text.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        let list = || value.split(',').map(str::trim).filter(|v| !v.is_empty());
        match key.trim().to_ascii_uppercase().as_str() {
            "FREQ" => {
                freq = Some(match value.trim().to_ascii_uppercase().as_str() {
                    "DAILY" => Freq::Daily,
                    "WEEKLY" => Freq::Weekly,
                    "MONTHLY" => Freq::Monthly,
                    "YEARLY" => Freq::Yearly,
                    _ => return None,
                })
            }
            "INTERVAL" => rule.interval = value.trim().parse().ok().filter(|n| *n > 0)?,
            "COUNT" => rule.count = Some(value.trim().parse().ok()?),
            "UNTIL" => rule.until = Some(parse_when(value, None, false)?),
            "BYDAY" => {
                for v in list() {
                    let split = v.len().checked_sub(2)?;
                    let ordinal = if split == 0 {
                        0
                    } else {
                        v[..split].parse::<i32>().ok()?
                    };
                    rule.by_day.push((ordinal, weekday(&v[split..])?));
                }
            }
            "BYMONTHDAY" => {
                for v in list() {
                    rule.by_month_day.push(v.parse().ok()?);
                }
            }
            "BYMONTH" => {
                for v in list() {
                    rule.by_month
                        .push(v.parse().ok().filter(|m| (1..=12).contains(m))?);
                }
            }
            "BYSETPOS" => {
                for v in list() {
                    rule.by_set_pos.push(v.parse().ok()?);
                }
            }
            "WKST" => rule.week_start = weekday(value.trim())?,
            // Sub-day expansion is not something a day calendar shows.
            "BYHOUR" | "BYMINUTE" | "BYSECOND" | "BYWEEKNO" | "BYYEARDAY" => return None,
            _ => {}
        }
    }
    rule.freq = freq?;
    Some(rule)
}

fn add_months(year: i32, month: u32, delta: i64) -> (i32, u32) {
    let index = i64::from(year) * 12 + i64::from(month) - 1 + delta;
    (index.div_euclid(12) as i32, index.rem_euclid(12) as u32 + 1)
}

/// Days of `weekday` in a month or year span, filtered by ordinal.
fn nth_weekdays(days: &[NaiveDate], ordinal: i32, weekday: Weekday) -> Vec<NaiveDate> {
    let matching: Vec<NaiveDate> = days
        .iter()
        .copied()
        .filter(|d| d.weekday() == weekday)
        .collect();
    match ordinal {
        0 => matching,
        n if n > 0 => matching.get(n as usize - 1).copied().into_iter().collect(),
        n => matching
            .len()
            .checked_sub(n.unsigned_abs() as usize)
            .and_then(|i| matching.get(i).copied())
            .into_iter()
            .collect(),
    }
}

fn month_days(year: i32, month: u32) -> Vec<NaiveDate> {
    (1..=days_in_month(year, month))
        .filter_map(|d| NaiveDate::from_ymd_opt(year, month, d))
        .collect()
}

fn month_day(year: i32, month: u32, day: i32) -> Option<NaiveDate> {
    let days = days_in_month(year, month) as i32;
    let day = if day < 0 { days + 1 + day } else { day };
    (1..=days)
        .contains(&day)
        .then(|| NaiveDate::from_ymd_opt(year, month, day as u32))
        .flatten()
}

/// Candidate days in one month, from BYDAY and BYMONTHDAY.
fn month_candidates(rule: &Rrule, year: i32, month: u32, start: NaiveDate) -> Vec<NaiveDate> {
    let days = month_days(year, month);
    let by_month_day: Vec<NaiveDate> = rule
        .by_month_day
        .iter()
        .filter_map(|&d| month_day(year, month, d))
        .collect();
    if !rule.by_day.is_empty() {
        let mut out: Vec<NaiveDate> = rule
            .by_day
            .iter()
            .flat_map(|&(n, wd)| nth_weekdays(&days, n, wd))
            .collect();
        if !rule.by_month_day.is_empty() {
            out.retain(|d| by_month_day.contains(d));
        }
        out
    } else if !rule.by_month_day.is_empty() {
        by_month_day
    } else {
        month_day(year, month, start.day() as i32)
            .into_iter()
            .collect()
    }
}

/// The candidate days of period `k` of a rule, sorted, before COUNT/UNTIL.
fn period(rule: &Rrule, start: NaiveDate, k: i64) -> Vec<NaiveDate> {
    let step = k * i64::from(rule.interval);
    let mut days = match rule.freq {
        Freq::Daily => {
            let day = start + Duration::days(step);
            let ok = (rule.by_day.is_empty()
                || rule.by_day.iter().any(|(_, w)| *w == day.weekday()))
                && (rule.by_month_day.is_empty()
                    || rule
                        .by_month_day
                        .iter()
                        .any(|&d| month_day(day.year(), day.month(), d) == Some(day)));
            if ok {
                vec![day]
            } else {
                Vec::new()
            }
        }
        Freq::Weekly => {
            let back = (7 + start.weekday().num_days_from_monday()
                - rule.week_start.num_days_from_monday())
                % 7;
            let week = start - Duration::days(i64::from(back)) + Duration::days(7 * step);
            let wanted: Vec<Weekday> = if rule.by_day.is_empty() {
                vec![start.weekday()]
            } else {
                rule.by_day.iter().map(|(_, w)| *w).collect()
            };
            (0..7)
                .map(|i| week + Duration::days(i))
                .filter(|d| wanted.contains(&d.weekday()))
                .collect()
        }
        Freq::Monthly => {
            let (year, month) = add_months(start.year(), start.month(), step);
            month_candidates(rule, year, month, start)
        }
        Freq::Yearly => {
            let year = start.year() + step as i32;
            if !rule.by_month.is_empty() {
                let mut out = Vec::new();
                for &month in &rule.by_month {
                    if rule.by_day.is_empty() && rule.by_month_day.is_empty() {
                        out.extend(month_day(year, month, start.day() as i32));
                    } else {
                        out.extend(month_candidates(rule, year, month, start));
                    }
                }
                out
            } else if !rule.by_day.is_empty() {
                let all: Vec<NaiveDate> = (1..=12).flat_map(|m| month_days(year, m)).collect();
                rule.by_day
                    .iter()
                    .flat_map(|&(n, wd)| nth_weekdays(&all, n, wd))
                    .collect()
            } else if !rule.by_month_day.is_empty() {
                (1..=12)
                    .flat_map(|m| month_candidates(rule, year, m, start))
                    .collect()
            } else {
                // February 29 recurs only in leap years.
                NaiveDate::from_ymd_opt(year, start.month(), start.day())
                    .into_iter()
                    .collect()
            }
        }
    };
    if !rule.by_month.is_empty() {
        days.retain(|d| rule.by_month.contains(&d.month()));
    }
    days.sort();
    days.dedup();
    if !rule.by_set_pos.is_empty() {
        let n = days.len() as i32;
        let picked: Vec<NaiveDate> = rule
            .by_set_pos
            .iter()
            .filter_map(|&p| {
                let index = if p > 0 { p - 1 } else { n + p };
                (0..n).contains(&index).then(|| days[index as usize])
            })
            .collect();
        days = picked;
        days.sort();
        days.dedup();
    }
    days
}

/// How many whole periods lie between `start` and `day`, rounded down.
fn periods_before(rule: &Rrule, start: NaiveDate, day: NaiveDate) -> i64 {
    let interval = i64::from(rule.interval);
    let raw = match rule.freq {
        Freq::Daily => (day - start).num_days() / interval,
        Freq::Weekly => (day - start).num_days() / (7 * interval),
        Freq::Monthly => {
            (i64::from(day.year() - start.year()) * 12 + i64::from(day.month())
                - i64::from(start.month()))
                / interval
        }
        Freq::Yearly => i64::from(day.year() - start.year()) / interval,
    };
    (raw - 1).max(0)
}

/// What the popup shows: one instance of an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub span: Span,
    pub summary: String,
    pub location: String,
    /// Index of the calendar in the configuration.
    pub calendar: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Span {
    /// All-day, from the first day to the day after the last.
    AllDay(NaiveDate, NaiveDate),
    /// UTC seconds, start inclusive and end exclusive.
    Timed(i64, i64),
}

impl Span {
    /// Does this span touch local day `[from, to)`?
    pub fn overlaps(&self, from: NaiveDate, to: NaiveDate, zones: &Zones) -> bool {
        match *self {
            Span::AllDay(a, b) => a < to && b > from,
            Span::Timed(a, b) => {
                let lo = zones.local_to_utc(from.and_time(NaiveTime::MIN));
                let hi = zones.local_to_utc(to.and_time(NaiveTime::MIN));
                a < hi && (b > lo || (a == b && a >= lo))
            }
        }
    }

    pub fn sort_key(&self, zones: &Zones) -> i64 {
        match *self {
            Span::AllDay(a, _) => zones.local_to_utc(a.and_time(NaiveTime::MIN)) - 1,
            Span::Timed(a, _) => a,
        }
    }
}

/// An instance identity used to match EXDATE and RECURRENCE-ID values.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum Key {
    Day(NaiveDate),
    At(i64),
}

fn keys(when: &When, zones: &mut Zones) -> Key {
    match when {
        When::Date(d) => Key::Day(*d),
        other => Key::At(other.utc(zones)),
    }
}

fn span_of(event: &Event, start: &When, zones: &mut Zones) -> Span {
    let base = event.start.wall();
    let length = match (&event.end, event.duration) {
        (Some(end), _) if end.is_date() == event.start.is_date() => {
            Some(end.wall() - base).filter(|d| *d >= Duration::zero())
        }
        (Some(end), _) => Some(Duration::seconds(end.utc(zones) - event.start.utc(zones)))
            .filter(|d| *d >= Duration::zero()),
        (None, Some(seconds)) if seconds >= 0 => Some(Duration::seconds(seconds)),
        _ => None,
    };
    match start {
        When::Date(d) => {
            let days = length.map(|l| l.num_days()).unwrap_or(1).max(1);
            Span::AllDay(*d, *d + Duration::days(days))
        }
        other => {
            let from = other.utc(zones);
            let end = match (&event.end, length) {
                // Same zone: keep the wall-clock length (a 1 h meeting stays 1 h
                // wall time across a DST change, as calendar apps show it).
                (Some(end), Some(length)) if end.is_date() == event.start.is_date() => {
                    start.with_wall(start.wall() + length).utc(zones)
                }
                (_, Some(length)) => from + length.num_seconds(),
                _ => from,
            };
            Span::Timed(from, end.max(from))
        }
    }
}

/// Safety bound on recurrence periods walked per event.
const MAX_PERIODS: i64 = 20_000;

/// Expand events into the instances that touch local days `[from, to)`.
pub fn expand(
    events: &[Event],
    calendar: usize,
    from: NaiveDate,
    to: NaiveDate,
    zones: &mut Zones,
) -> Vec<Occurrence> {
    let mut overridden: HashMap<&str, HashSet<Key>> = HashMap::new();
    for event in events {
        if let Some(id) = &event.recurrence_id {
            let key = keys(id, zones);
            overridden
                .entry(event.uid.as_str())
                .or_default()
                .insert(key);
        }
    }
    let mut out = Vec::new();
    let mut push = |span: Span, event: &Event, zones: &Zones| {
        if span.overlaps(from, to, zones) {
            out.push(Occurrence {
                span,
                summary: event.summary.clone(),
                location: event.location.clone(),
                calendar,
            });
        }
    };
    for event in events {
        if event.cancelled {
            continue;
        }
        if event.recurrence_id.is_some() {
            let span = span_of(event, &event.start, zones);
            push(span, event, zones);
            continue;
        }
        let skip: HashSet<Key> = event
            .exdates
            .iter()
            .map(|w| keys(w, zones))
            .chain(
                overridden
                    .get(event.uid.as_str())
                    .into_iter()
                    .flatten()
                    .copied(),
            )
            .collect();
        let excluded = |when: &When, zones: &mut Zones| {
            let key = keys(when, zones);
            skip.contains(&key) || skip.contains(&Key::Day(when.wall().date()))
        };
        for extra in &event.rdates {
            let when = if extra.is_date() == event.start.is_date() {
                extra.clone()
            } else {
                event
                    .start
                    .with_wall(extra.wall().date().and_time(event.start.wall().time()))
            };
            if !excluded(&when, zones) {
                let span = span_of(event, &when, zones);
                push(span, event, zones);
            }
        }
        let rule = event.rrule.as_deref().and_then(parse_rrule);
        let Some(rule) = rule else {
            if !excluded(&event.start, zones) {
                let span = span_of(event, &event.start, zones);
                push(span, event, zones);
            }
            continue;
        };
        let first = event.start.wall();
        let time = first.time();
        let start_day = first.date();
        let until = rule.until.as_ref().map(|u| match u {
            When::Date(d) => Key::Day(*d),
            // A floating UNTIL is in the event's own zone.
            When::Wall(t, None) => Key::At(event.start.with_wall(*t).utc(zones)),
            other => Key::At(other.utc(zones)),
        });
        let mut k = if rule.count.is_none() {
            // Without COUNT, nothing before the window matters.
            periods_before(&rule, start_day, from - Duration::days(62))
        } else {
            0
        };
        let mut seen = 0u32;
        let mut walked = 0;
        'periods: while walked < MAX_PERIODS {
            walked += 1;
            let days = period(&rule, start_day, k);
            k += 1;
            for day in days {
                let wall = day.and_time(time);
                if wall < first {
                    continue;
                }
                let when = event.start.with_wall(wall);
                match until {
                    Some(Key::Day(d)) if day > d => break 'periods,
                    Some(Key::At(t)) if when.utc(zones) > t => break 'periods,
                    _ => {}
                }
                if let Some(count) = rule.count {
                    if seen >= count {
                        break 'periods;
                    }
                }
                seen += 1;
                if day >= to {
                    break 'periods;
                }
                if !excluded(&when, zones) {
                    let span = span_of(event, &when, zones);
                    push(span, event, zones);
                }
            }
            // A period beyond the window cannot add anything.
            if period_start_after(&rule, start_day, k, to) {
                break;
            }
        }
    }
    out.sort_by_key(|o| (o.span.sort_key(zones), o.summary.clone()));
    out
}

fn period_start_after(rule: &Rrule, start: NaiveDate, k: i64, to: NaiveDate) -> bool {
    let step = k * i64::from(rule.interval);
    let begin = match rule.freq {
        Freq::Daily => start + Duration::days(step),
        Freq::Weekly => start + Duration::days(7 * step - 6),
        Freq::Monthly => {
            let (y, m) = add_months(start.year(), start.month(), step);
            NaiveDate::from_ymd_opt(y, m, 1).unwrap_or(start)
        }
        Freq::Yearly => NaiveDate::from_ymd_opt(start.year() + step as i32, 1, 1).unwrap_or(start),
    };
    begin >= to
}

/// Events whose every instance ended long ago are not worth caching.
pub fn prune(events: Vec<Event>, before: NaiveDate) -> Vec<Event> {
    events
        .into_iter()
        .filter(|e| {
            if e.rrule.is_some() || !e.rdates.is_empty() {
                return true;
            }
            let end = e.end.as_ref().unwrap_or(&e.start).wall().date();
            end >= before
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tz::Zone;

    const BASIC: &str = include_str!("../tests/fixtures/calendar/basic.ics");
    const RECURRING: &str = include_str!("../tests/fixtures/calendar/recurring.ics");

    fn zones() -> Zones {
        let cet = Zone::posix("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        let mut zones = Zones::pinned(cet.clone());
        zones.insert("Europe/Stockholm", cet);
        zones.insert(
            "America/New_York",
            Zone::posix("EST5EDT,M3.2.0,M11.1.0").unwrap(),
        );
        zones
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn utc(s: &str) -> i64 {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M")
            .unwrap()
            .and_utc()
            .timestamp()
    }

    fn named<'a>(list: &'a [Occurrence], summary: &str) -> Vec<&'a Occurrence> {
        list.iter().filter(|o| o.summary == summary).collect()
    }

    #[test]
    fn folding_escaping_and_components() {
        let events = parse(BASIC);
        assert_eq!(
            events.len(),
            5,
            "the cancelled event is dropped: {events:#?}"
        );
        let dentist = events.iter().find(|e| e.uid.starts_with("tz-1")).unwrap();
        assert_eq!(dentist.summary, "Tandläkare, kontroll; ta med kort");
        assert_eq!(dentist.location, "Storgatan 1 Stockholm");
        let folded = events.iter().find(|e| e.uid.starts_with("folded")).unwrap();
        assert_eq!(folded.summary, "A summary that is folded across two lines");
        assert_eq!(folded.duration, Some(5400));
        assert!(matches!(&folded.start, When::Wall(_, Some(z)) if z == "America/New_York"));
    }

    #[test]
    fn utc_zoned_floating_and_all_day_times() {
        let mut zones = zones();
        let list = expand(
            &parse(BASIC),
            0,
            day("2026-11-09"),
            day("2026-11-17"),
            &mut zones,
        );
        assert_eq!(list.len(), 5);
        let span = |name: &str| named(&list, name)[0].span;
        assert_eq!(
            span("Standup"),
            Span::Timed(utc("2026-11-09 08:00"), utc("2026-11-09 08:30"))
        );
        assert_eq!(
            span("Tandläkare, kontroll; ta med kort"),
            Span::Timed(utc("2026-11-10 13:30"), utc("2026-11-10 14:30"))
        );
        assert_eq!(
            span("A summary that is folded across two lines"),
            Span::Timed(utc("2026-11-11 14:00"), utc("2026-11-11 15:30"))
        );
        assert_eq!(
            span("Floating dinner"),
            Span::Timed(utc("2026-11-12 18:00"), utc("2026-11-12 20:00"))
        );
        assert_eq!(
            span("Fjällresa"),
            Span::AllDay(day("2026-11-13"), day("2026-11-16"))
        );
        // Sorted by start.
        assert_eq!(list[0].summary, "Standup");
        assert!(span("Fjällresa").overlaps(day("2026-11-15"), day("2026-11-16"), &zones));
        assert!(!span("Fjällresa").overlaps(day("2026-11-16"), day("2026-11-17"), &zones));
    }

    #[test]
    fn recurrence_rules_exceptions_and_overrides() {
        let mut zones = zones();
        let list = expand(
            &parse(RECURRING),
            0,
            day("2026-11-01"),
            day("2026-12-01"),
            &mut zones,
        );
        let daily = named(&list, "Daily five");
        assert_eq!(daily.len(), 5);
        assert_eq!(
            daily[0].span,
            Span::Timed(utc("2026-11-02 06:00"), utc("2026-11-02 06:15"))
        );
        assert_eq!(
            daily[4].span,
            Span::Timed(utc("2026-11-06 06:00"), utc("2026-11-06 06:15"))
        );

        let gym: Vec<Span> = named(&list, "Gym").iter().map(|o| o.span).collect();
        assert_eq!(
            gym,
            vec![
                Span::Timed(utc("2026-11-02 08:00"), utc("2026-11-02 09:00")),
                Span::Timed(utc("2026-11-04 08:00"), utc("2026-11-04 09:00")),
                // 9th moved, 11th EXDATE, 16th cancelled override.
                Span::Timed(utc("2026-11-18 08:00"), utc("2026-11-18 09:00")),
            ]
        );
        let moved = named(&list, "Gym (moved)");
        assert_eq!(moved.len(), 1);
        assert_eq!(
            moved[0].span,
            Span::Timed(utc("2026-11-09 10:30"), utc("2026-11-09 11:30"))
        );

        let fika = named(&list, "Fredagsfika");
        assert_eq!(fika.len(), 1);
        assert_eq!(
            fika[0].span,
            Span::Timed(utc("2026-11-27 15:00"), utc("2026-11-27 16:00"))
        );

        assert!(
            named(&list, "Month end").is_empty(),
            "COUNT=6 ended in October"
        );
        assert_eq!(
            named(&list, "Födelsedag")[0].span,
            Span::AllDay(day("2026-11-14"), day("2026-11-15"))
        );
        let biweekly: Vec<u32> = named(&list, "Every other week")
            .iter()
            .map(|o| match o.span {
                Span::Timed(t, _) => chrono::DateTime::from_timestamp(t, 0).unwrap().day(),
                _ => 0,
            })
            .collect();
        assert_eq!(biweekly, vec![1, 3, 15, 17, 29]);
        assert_eq!(
            named(&list, "Thanksgiving")[0].span,
            Span::AllDay(day("2026-11-26"), day("2026-11-27"))
        );
        assert_eq!(named(&list, "Unsupported rule shows once").len(), 1);
    }

    #[test]
    fn wall_time_is_kept_across_daylight_saving() {
        let mut zones = zones();
        let list = expand(
            &parse(RECURRING),
            0,
            day("2026-10-01"),
            day("2026-12-01"),
            &mut zones,
        );
        let starts: Vec<Span> = named(&list, "Across the switch")
            .iter()
            .map(|o| o.span)
            .collect();
        assert_eq!(
            starts,
            vec![
                Span::Timed(utc("2026-10-19 06:00"), utc("2026-10-19 07:00")),
                Span::Timed(utc("2026-10-26 07:00"), utc("2026-10-26 08:00")),
                Span::Timed(utc("2026-11-02 07:00"), utc("2026-11-02 08:00")),
            ]
        );
        let month_end: Vec<NaiveDate> = expand(
            &parse(RECURRING),
            0,
            day("2026-01-01"),
            day("2027-01-01"),
            &mut zones,
        )
        .into_iter()
        .filter(|o| o.summary == "Month end")
        .map(|o| match o.span {
            Span::AllDay(a, _) => a,
            _ => NaiveDate::MIN,
        })
        .collect();
        let expected: Vec<NaiveDate> = [
            "2026-01-31",
            "2026-03-31",
            "2026-05-31",
            "2026-07-31",
            "2026-08-31",
            "2026-10-31",
        ]
        .iter()
        .map(|d| day(d))
        .collect();
        assert_eq!(month_end, expected, "months without a 31st are skipped");
    }

    #[test]
    fn a_long_running_rule_is_found_far_from_its_start() {
        let mut zones = zones();
        let list = expand(
            &parse(RECURRING),
            0,
            day("2046-11-01"),
            day("2046-12-01"),
            &mut zones,
        );
        assert_eq!(
            named(&list, "Födelsedag")[0].span,
            Span::AllDay(day("2046-11-14"), day("2046-11-15"))
        );
        assert_eq!(named(&list, "Fredagsfika").len(), 1);
        assert!(named(&list, "Daily five").is_empty());
    }

    #[test]
    fn cached_times_round_trip() {
        let events = parse(RECURRING);
        let json = serde_json::to_string(&events).unwrap();
        let back: Vec<Event> = serde_json::from_str(&json).unwrap();
        assert_eq!(events, back);
    }

    #[test]
    fn durations_and_bad_input() {
        assert_eq!(parse_duration("P1D"), Some(86_400));
        assert_eq!(parse_duration("PT1H30M"), Some(5_400));
        assert_eq!(parse_duration("-PT15M"), Some(-900));
        assert_eq!(parse_duration("P1W"), Some(604_800));
        assert_eq!(parse_duration("1H"), None);
        assert!(parse("not a calendar").is_empty());
        assert!(parse(
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nSUMMARY:no start\nEND:VEVENT\nEND:VCALENDAR"
        )
        .is_empty());
        assert!(parse_rrule("FREQ=SECONDLY").is_none());
        assert!(parse_rrule("FREQ=DAILY;BYHOUR=9").is_none());
    }
}
