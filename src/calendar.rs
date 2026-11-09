//! Read-only calendar events for the clock's calendar popup.
//!
//! Feeds are iCalendar (ICS) URLs, such as Google Calendar's "secret address
//! in iCal format". Those URLs are credentials: anyone who has one can read the
//! calendar. They live in their own file, `~/.config/nirubar/calendars.toml`,
//! which is refused unless only its owner can read it (mode 0600). The URL is
//! never logged, printed, shown or put on a command line; curl reads it from
//! its standard input.
//!
//! Fetching happens on a background thread through the bounded probe runner.
//! Parsed events are cached in `~/.cache/nirubar/calendar/` (mode 0600) so
//! they still show offline, and the draw loop only ever reads memory.

use crate::ics::{self, Event, Occurrence, Span};
use crate::popup::{Bullet, Decor, Link, Mark};
use crate::probe;
use crate::tz::Zones;
use chrono::{Datelike, Duration as Days, NaiveDate};
use serde::{Deserialize, Serialize};
use std::{
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// One configured feed. `url` is a secret; `Debug` leaves it out.
#[derive(Clone)]
pub struct Feed {
    pub name: String,
    url: String,
    pub colour: Option<u32>,
    pub open: Option<String>,
}

impl std::fmt::Debug for Feed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Feed")
            .field("name", &self.name)
            .field("url", &"<redacted>")
            .field("colour", &self.colour)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub feeds: Vec<Feed>,
    pub refresh: Duration,
    pub days_ahead: u32,
    pub show_in_bar: bool,
    /// Why calendars.toml or one of its feeds was not used.
    pub problems: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            feeds: Vec::new(),
            refresh: Duration::from_secs(15 * 60),
            days_ahead: 7,
            show_in_bar: false,
            problems: Vec::new(),
        }
    }
}

fn config_base() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
        .join("nirubar")
}

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        })
        .join("nirubar")
        .join("calendar")
}

const SECRETS: &str = "~/.config/nirubar/calendars.toml";
const GOOGLE_OPEN: &str = "https://calendar.google.com/calendar/r/day/{yyyy}/{m}/{d}";

/// Read the non-secret `[calendar]` options and the feeds file.
pub fn settings() -> Settings {
    let base = config_base();
    let mut settings = Settings::default();
    if let Some(table) = std::fs::read_to_string(base.join("config.toml"))
        .ok()
        .and_then(|t| t.parse::<toml::Value>().ok())
        .and_then(|v| v.get("calendar").cloned())
    {
        apply_options(&mut settings, &table);
    }
    read_feeds(&base.join("calendars.toml"), &mut settings);
    settings
}

/// What identifies one version of a file: device, inode, size, mode, mtime
/// and ctime (a `chmod 600` changes the mode and ctime). `None` when missing.
type FileStamp = Option<(u64, u64, u64, u32, i64, i64, i64, i64)>;

fn file_stamp(path: &Path) -> FileStamp {
    let m = std::fs::metadata(path).ok()?;
    Some((
        m.dev(),
        m.ino(),
        m.size(),
        m.mode(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}

/// Both files the calendar reads: the `[calendar]` options and the feeds.
fn stamps() -> [FileStamp; 2] {
    let base = config_base();
    [
        file_stamp(&base.join("config.toml")),
        file_stamp(&base.join("calendars.toml")),
    ]
}

/// The calendar plus the files it came from, so an edited, added, replaced
/// or `chmod`-ed `calendars.toml` takes effect without restarting the bar.
pub struct Watch {
    calendar: Option<Calendar>,
    stamps: [FileStamp; 2],
}

impl Watch {
    pub fn load() -> Self {
        // Stamp first: a write that lands while the file is being read
        // changes the stamp again and is picked up by the next check.
        let stamps = stamps();
        Self {
            calendar: Calendar::new(settings()),
            stamps,
        }
    }

    pub fn calendar(&mut self) -> Option<&mut Calendar> {
        self.calendar.as_mut()
    }

    /// Re-read the settings when either file changed since the last look.
    /// A few `stat` calls; the bar runs it every few seconds, never per frame.
    /// Returns true when the calendar was re-read.
    pub fn check_files(&mut self) -> bool {
        let now = stamps();
        if now == self.stamps {
            return false;
        }
        self.stamps = now;
        self.reread(settings());
        true
    }

    fn reread(&mut self, settings: Settings) {
        match self.calendar.as_mut() {
            Some(_) if settings.feeds.is_empty() && settings.problems.is_empty() => {
                self.calendar = None;
            }
            Some(calendar) => calendar.apply(settings),
            None => self.calendar = Calendar::new(settings),
        }
    }
}

fn apply_options(settings: &mut Settings, table: &toml::Value) {
    if let Some(n) = table.get("refresh_minutes").and_then(|v| v.as_integer()) {
        settings.refresh = Duration::from_secs((n.clamp(5, 24 * 60) as u64) * 60);
    }
    if let Some(n) = table.get("days_ahead").and_then(|v| v.as_integer()) {
        settings.days_ahead = n.clamp(0, 31) as u32;
    }
    if let Some(b) = table.get("show_in_bar").and_then(|v| v.as_bool()) {
        settings.show_in_bar = b;
    }
}

/// Refuse a secrets file that anyone but its owner could read.
fn secure(path: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{SECRETS}: {}", e.kind()))?;
    let mode = meta.mode() & 0o777;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if meta.uid() != uid {
        return Err(format!("{SECRETS} is not owned by you; calendars are off"));
    }
    if mode & 0o077 != 0 {
        return Err(format!(
            "{SECRETS} has mode {mode:04o}; calendars stay off until: chmod 600 {SECRETS}"
        ));
    }
    if !meta.is_file() {
        return Err(format!("{SECRETS} is not a regular file"));
    }
    Ok(())
}

/// A feed URL in the form curl will fetch, or why it is refused. Plain http
/// is accepted only on this machine (tests and local servers).
fn feed_url(raw: &str) -> Result<String, &'static str> {
    let raw = raw.trim();
    if raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("URL contains spaces or control characters");
    }
    let url = if let Some(rest) = raw.strip_prefix("webcal://") {
        format!("https://{rest}")
    } else {
        raw.to_string()
    };
    if url.starts_with("https://") {
        return Ok(url);
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let host = rest.split(['/', '?']).next().unwrap_or_default();
        let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host);
        let name = if host.starts_with('[') {
            host.split(']')
                .next()
                .unwrap_or_default()
                .trim_start_matches('[')
        } else {
            host.split(':').next().unwrap_or_default()
        };
        if matches!(name, "127.0.0.1" | "localhost" | "::1") {
            return Ok(url);
        }
    }
    Err("URL must start with https:// (or webcal://)")
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .and_then(|r| r.split(['/', '?']).next())
        .unwrap_or_default()
}

fn read_feeds(path: &Path, settings: &mut Settings) {
    if !path.exists() {
        return;
    }
    if let Err(problem) = secure(path) {
        settings.problems.push(problem);
        return;
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            settings.problems.push(format!("{SECRETS}: {}", e.kind()));
            return;
        }
    };
    let value = match text.parse::<toml::Value>() {
        Ok(v) => v,
        Err(e) => {
            // The parser quotes the offending line, which may hold a URL.
            let line = e.span().map(|s| text[..s.start].lines().count().max(1));
            settings.problems.push(format!(
                "{SECRETS}: not valid TOML{}",
                line.map(|l| format!(" (line {l})")).unwrap_or_default()
            ));
            return;
        }
    };
    let Some(list) = value.get("calendar").and_then(|v| v.as_array()) else {
        settings
            .problems
            .push(format!("{SECRETS}: no [[calendar]] entries"));
        return;
    };
    for (i, entry) in list.iter().enumerate() {
        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(40).collect::<String>())
            .unwrap_or_else(|| format!("calendar {}", i + 1));
        let Some(raw) = entry.get("url").and_then(|v| v.as_str()) else {
            settings.problems.push(format!("{name}: no url"));
            continue;
        };
        let url = match feed_url(raw) {
            Ok(u) => u,
            Err(why) => {
                settings.problems.push(format!("{name}: {why}"));
                continue;
            }
        };
        let colour = entry
            .get("color")
            .or_else(|| entry.get("colour"))
            .and_then(|v| v.as_str())
            .and_then(|c| crate::config::Theme::pixel(c.trim().trim_start_matches('#')).ok());
        let open = entry
            .get("open")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|o| o.starts_with("https://") || o.starts_with("http://"))
            .or_else(|| (host_of(&url) == "calendar.google.com").then(|| GOOGLE_OPEN.to_string()));
        settings.feeds.push(Feed {
            name,
            url,
            colour,
            open,
        });
    }
}

/// Lines for `nirubar --check-config`. Never includes a URL.
pub fn check_lines() -> Vec<String> {
    let settings = settings();
    let mut out = Vec::new();
    if settings.feeds.is_empty() && settings.problems.is_empty() {
        out.push(format!("Calendars: none ({SECRETS} not present)"));
    } else if !settings.feeds.is_empty() {
        let names: Vec<&str> = settings.feeds.iter().map(|f| f.name.as_str()).collect();
        out.push(format!(
            "Calendars: {} ({}), refresh {} min, {} days ahead{}",
            names.len(),
            names.join(", "),
            settings.refresh.as_secs() / 60,
            settings.days_ahead,
            if settings.show_in_bar {
                ", next event in the bar"
            } else {
                ""
            }
        ));
    }
    for problem in &settings.problems {
        out.push(format!("Calendars: WARNING {problem}"));
    }
    out
}

/// Remove anything from an error that could be part of a feed URL.
fn redact(text: &str, url: &str) -> String {
    let mut out = text.replace(url, "<calendar url>");
    for piece in url.split(['/', '?', '&', '=', '#']) {
        if piece.len() >= 6 && !piece.contains(':') && piece != host_of(url) {
            out = out.replace(piece, "…");
        }
    }
    let out = out.trim().replace('\n', " ");
    out.chars().take(120).collect()
}

/// Escape a value for a curl config file's quoted string.
fn curl_quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn fetch(url: &str) -> Result<Vec<Event>, String> {
    let protocols = if url.starts_with("http://") {
        "=http"
    } else {
        "=https"
    };
    let config = format!("url = \"{}\"\n", curl_quote(url));
    let out = probe::run_with_input(
        "curl",
        &[
            "-q",
            "-fsS",
            "--globoff",
            "--proto",
            protocols,
            "--proto-redir",
            "=https",
            "-L",
            "--max-redirs",
            "3",
            "--max-time",
            "20",
            "--max-filesize",
            "16777216",
            "-K",
            "-",
        ],
        &[],
        Duration::from_secs(25),
        16 * 1024 * 1024,
        config.as_bytes(),
    )
    .ok_or("request timed out or curl is unavailable")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(redact(
            if err.trim().is_empty() {
                "download failed"
            } else {
                &err
            },
            url,
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    if !text.contains("BEGIN:VCALENDAR") {
        return Err("response is not an iCalendar feed".into());
    }
    let before = chrono::Local::now().date_naive() - Days::days(400);
    Ok(ics::prune(ics::parse(&text), before))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// FNV-1a, so a cache made for another URL is not shown for this one.
fn fingerprint(url: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[derive(Serialize, Deserialize)]
struct Cache {
    schema: u32,
    fetched: u64,
    key: String,
    events: Vec<Event>,
}

fn cache_file(index: usize, feed: &Feed) -> PathBuf {
    let safe: String = feed
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(24)
        .collect();
    cache_dir().join(format!("{index}-{safe}.json"))
}

fn read_cache(index: usize, feed: &Feed) -> Option<(u64, Vec<Event>)> {
    let text = std::fs::read_to_string(cache_file(index, feed)).ok()?;
    let cache: Cache = serde_json::from_str(&text).ok()?;
    (cache.schema == 1 && cache.key == fingerprint(&feed.url))
        .then_some((cache.fetched, cache.events))
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().ok_or(std::io::ErrorKind::NotFound)?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let tmp = path.with_extension(format!("new.{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

fn write_cache(index: usize, feed: &Feed, fetched: u64, events: &[Event]) {
    let cache = Cache {
        schema: 1,
        fetched,
        key: fingerprint(&feed.url),
        events: events.to_vec(),
    };
    if let Ok(text) = serde_json::to_vec(&cache) {
        let _ = write_private(&cache_file(index, feed), &text);
    }
}

/// One bar per monitor shares the cache: whoever takes this lock fetches, the
/// others pick the result up from disk.
fn take_lock() -> Option<PathBuf> {
    let path = cache_dir().join("fetch.lock");
    let dir = cache_dir();
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .ok()?;
    let fresh = |p: &Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < Duration::from_secs(120))
    };
    if path.exists() && !fresh(&path) {
        let _ = std::fs::remove_file(&path);
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .ok()?;
    Some(path)
}

/// Fetch `todo`, or, while another bar holds the fetch lock, wait for it and
/// take what it cached. Feeds still missing after that are fetched here.
fn fetch_or_wait(mut todo: Vec<(usize, Feed)>, refresh: u64) -> Results {
    let mut results = Vec::new();
    let give_up = Instant::now() + Duration::from_secs(90);
    loop {
        // Whatever another bar has fetched meanwhile.
        todo.retain(|(i, feed)| match read_cache(*i, feed) {
            Some((fetched, events)) if !Some(fetched).is_none_or_old(refresh) => {
                results.push((*i, Ok((fetched, events))));
                false
            }
            _ => true,
        });
        if todo.is_empty() {
            return results;
        }
        if let Some(lock) = take_lock() {
            for (i, feed) in todo {
                let fetched = now_secs();
                let result = fetch(&feed.url).map(|events| {
                    write_cache(i, &feed, fetched, &events);
                    (fetched, events)
                });
                results.push((i, result));
            }
            let _ = std::fs::remove_file(lock);
            return results;
        }
        if Instant::now() >= give_up {
            return results; // the next round tries again
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

struct FeedState {
    fetched: Option<u64>,
    events: Vec<Event>,
    error: Option<String>,
}

type Results = Vec<(usize, Result<(u64, Vec<Event>), String>)>;

pub struct Calendar {
    settings: Settings,
    feeds: Vec<FeedState>,
    loaded: bool,
    rx: Option<Receiver<Results>>,
    last_try: Option<Instant>,
    zones: Zones,
    bar: Option<(Instant, String)>,
}

/// Theme colours a calendar without its own colour takes, in order.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub accent: u32,
    pub urgent: u32,
    pub foreground: u32,
}

impl Calendar {
    /// `None` when nothing is configured and there is nothing to report.
    pub fn new(settings: Settings) -> Option<Self> {
        if settings.feeds.is_empty() && settings.problems.is_empty() {
            return None;
        }
        Some(Self {
            feeds: settings
                .feeds
                .iter()
                .map(|_| FeedState {
                    fetched: None,
                    events: Vec::new(),
                    error: None,
                })
                .collect(),
            settings,
            loaded: false,
            rx: None,
            last_try: None,
            zones: Zones::system(),
            bar: None,
        })
    }

    pub fn show_in_bar(&self) -> bool {
        self.settings.show_in_bar
    }

    /// Take new settings from a re-read file. Feeds whose URL is unchanged
    /// keep their events; new or changed feeds are fetched at once.
    pub fn apply(&mut self, settings: Settings) {
        self.load_cache();
        let same_feeds = settings.feeds.len() == self.settings.feeds.len()
            && settings
                .feeds
                .iter()
                .zip(&self.settings.feeds)
                .all(|(new, old)| new.url == old.url && new.name == old.name);
        if !same_feeds {
            let mut old: Vec<(String, FeedState)> = self
                .settings
                .feeds
                .iter()
                .map(|f| f.url.clone())
                .zip(std::mem::take(&mut self.feeds))
                .collect();
            let mut feeds = Vec::with_capacity(settings.feeds.len());
            for (i, feed) in settings.feeds.iter().enumerate() {
                let kept = old
                    .iter()
                    .position(|(url, _)| *url == feed.url)
                    .map(|at| old.swap_remove(at).1);
                let state = match kept {
                    Some(state) => {
                        // The cache file is named by position and name.
                        if let Some(fetched) = state.fetched {
                            if read_cache(i, feed).is_none() {
                                write_cache(i, feed, fetched, &state.events);
                            }
                        }
                        state
                    }
                    None => match read_cache(i, feed) {
                        Some((fetched, events)) => FeedState {
                            fetched: Some(fetched),
                            events,
                            error: None,
                        },
                        None => FeedState {
                            fetched: None,
                            events: Vec::new(),
                            error: None,
                        },
                    },
                };
                feeds.push(state);
            }
            self.feeds = feeds;
            // Results in flight are numbered for the old list.
            self.rx = None;
            // Fetch what is new now, not after the retry pause.
            self.last_try = None;
        }
        self.settings = settings;
        self.bar = None;
    }

    fn load_cache(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        for (i, feed) in self.settings.feeds.iter().enumerate() {
            if let Some((fetched, events)) = read_cache(i, feed) {
                self.feeds[i].fetched = Some(fetched);
                self.feeds[i].events = events;
            }
        }
    }

    fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        match rx.try_recv() {
            Ok(results) => {
                for (i, result) in results {
                    let Some(state) = self.feeds.get_mut(i) else {
                        continue;
                    };
                    match result {
                        Ok((fetched, events)) => {
                            state.fetched = Some(fetched);
                            state.events = events;
                            state.error = None;
                        }
                        Err(e) => state.error = Some(e),
                    }
                }
                self.rx = None;
                self.bar = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.rx = None,
        }
    }

    fn due(&self) -> bool {
        let refresh = self.settings.refresh.as_secs();
        self.feeds.iter().any(|f| f.fetched.is_none_or_old(refresh))
    }

    fn start_fetch(&mut self) {
        if self.rx.is_some() || self.settings.feeds.is_empty() || !self.due() {
            return;
        }
        let failing = self.feeds.iter().any(|f| f.error.is_some());
        let wait = if failing { 120 } else { 20 };
        if self.last_try.is_some_and(|t| t.elapsed().as_secs() < wait) {
            return;
        }
        self.last_try = Some(Instant::now());
        // Another bar may have refreshed the shared cache meanwhile.
        let refresh = self.settings.refresh.as_secs();
        for (i, feed) in self.settings.feeds.iter().enumerate() {
            if let Some((fetched, events)) = read_cache(i, feed) {
                if self.feeds[i].fetched.map_or(true, |f| fetched > f) {
                    self.feeds[i].fetched = Some(fetched);
                    self.feeds[i].events = events;
                    self.bar = None;
                }
            }
        }
        if !self.due() {
            return;
        }
        let todo: Vec<(usize, Feed)> = self
            .settings
            .feeds
            .iter()
            .enumerate()
            .filter(|(i, _)| self.feeds[*i].fetched.is_none_or_old(refresh))
            .map(|(i, f)| (i, f.clone()))
            .collect();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetch_or_wait(todo, refresh));
        });
        self.rx = Some(rx);
    }

    /// Cheap per-frame upkeep: no I/O beyond the first cache read.
    pub fn tick(&mut self) {
        self.load_cache();
        self.poll();
        self.start_fetch();
    }

    fn colour(&self, index: usize, palette: &Palette) -> u32 {
        self.settings
            .feeds
            .get(index)
            .and_then(|f| f.colour)
            .unwrap_or(match index % 3 {
                0 => palette.accent,
                1 => palette.urgent,
                _ => palette.foreground,
            })
    }

    fn occurrences(&mut self, from: NaiveDate, to: NaiveDate) -> Vec<Occurrence> {
        let mut all = Vec::new();
        for (i, state) in self.feeds.iter().enumerate() {
            all.extend(ics::expand(&state.events, i, from, to, &mut self.zones));
        }
        let zones = &self.zones;
        all.sort_by_key(|o| (o.span.sort_key(zones), o.calendar));
        all
    }

    fn open_url(&self, calendar: usize, day: NaiveDate) -> Option<String> {
        let template = self.settings.feeds.get(calendar)?.open.as_deref()?;
        Some(open_url(template, day))
    }

    /// The next event for the bar, refreshed at most once a minute.
    pub fn bar_text(&mut self) -> String {
        if let Some((at, text)) = &self.bar {
            if at.elapsed() < Duration::from_secs(60) {
                return text.clone();
            }
        }
        let now = chrono::Local::now();
        let today = now.date_naive();
        let now_utc = now.timestamp();
        let list = self.occurrences(today, today + Days::days(2));
        let text = list
            .iter()
            .find_map(|o| match o.span {
                Span::Timed(start, _) if start >= now_utc && start - now_utc <= 12 * 3600 => {
                    let local = self.zones.to_local(start);
                    let prefix = if local.date() == today { "" } else { "tmrw " };
                    Some(format!(
                        "{prefix}{} {}",
                        local.format("%H:%M"),
                        truncate(&o.summary, 22)
                    ))
                }
                _ => None,
            })
            .unwrap_or_default();
        self.bar = Some((Instant::now(), text.clone()));
        text
    }

    /// Add markers, the agenda and click targets to the month popup.
    pub fn decorate(
        &mut self,
        year: i32,
        month: u32,
        lines: &mut Vec<String>,
        decor: &mut Decor,
        palette: &Palette,
    ) {
        let today = chrono::Local::now().date_naive();
        let now = chrono::Local::now().timestamp();
        self.decorate_at(year, month, today, now, lines, decor, palette);
    }

    #[allow(clippy::too_many_arguments)]
    fn decorate_at(
        &mut self,
        year: i32,
        month: u32,
        today: NaiveDate,
        now: i64,
        lines: &mut Vec<String>,
        decor: &mut Decor,
        palette: &Palette,
    ) {
        let Some(header) = lines.iter().position(|l| l.starts_with("Wk  Mon")) else {
            return;
        };
        let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(today);
        let (ny, nm) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        let next = NaiveDate::from_ymd_opt(ny, nm, 1).unwrap_or(first + Days::days(31));
        let lead = first.weekday().num_days_from_monday() as usize;
        let month_list = self.occurrences(first, next);
        let mut day = first;
        while day < next {
            let cell = lead + day.day() as usize - 1;
            let line = header + 1 + cell / 7;
            let col = 4 + (cell % 7) * 4;
            let mut colours = Vec::new();
            let mut link_calendar = None;
            for o in &month_list {
                if o.span.overlaps(day, day + Days::days(1), &self.zones) {
                    let c = self.colour(o.calendar, palette);
                    if !colours.contains(&c) && colours.len() < 3 {
                        colours.push(c);
                    }
                    if link_calendar.is_none() && self.open_url(o.calendar, day).is_some() {
                        link_calendar = Some(o.calendar);
                    }
                }
            }
            if !colours.is_empty() {
                decor.marks.push(Mark {
                    line,
                    col: col + 1,
                    cols: 2,
                    colours,
                });
            }
            let target = link_calendar.or_else(|| {
                (0..self.settings.feeds.len()).find(|&i| self.open_url(i, day).is_some())
            });
            if let Some(url) = target.and_then(|i| self.open_url(i, day)) {
                decor.links.push(Link {
                    line,
                    col,
                    cols: 4,
                    url,
                });
            }
            day += Days::days(1);
        }

        // The agenda goes after the grid, before the navigation hint.
        let mut at = header + 1;
        while lines.get(at).is_some_and(|l| !l.is_empty()) {
            at += 1;
        }
        let mut agenda: Vec<Row> = Vec::new();
        for problem in &self.settings.problems {
            agenda.extend(wrap(problem, 38).into_iter().map(Row::Text));
        }
        let end = today + Days::days(i64::from(self.settings.days_ahead) + 1);
        let list = self.occurrences(today, end);
        let mut rows = 0;
        let mut hidden = 0;
        let mut day = today;
        while day < end {
            let todays: Vec<&Occurrence> = list
                .iter()
                .filter(|o| o.span.overlaps(day, day + Days::days(1), &self.zones))
                .filter(|o| match o.span {
                    // Timed events show on their first day, or as "until" today.
                    Span::Timed(start, stop) => {
                        let local = self.zones.to_local(start).date();
                        local == day || (day == today && local < day && stop > now)
                    }
                    Span::AllDay(..) => true,
                })
                .collect();
            if !todays.is_empty() {
                if rows >= MAX_ROWS - 1 {
                    hidden += todays.len();
                    day += Days::days(1);
                    continue;
                }
                rows += 1;
                let label = match (day - today).num_days() {
                    0 => format!("Today · {}", day.format("%a %-d %b")),
                    1 => format!("Tomorrow · {}", day.format("%a %-d %b")),
                    _ => day.format("%A %-d %B").to_string(),
                };
                let link = todays.iter().find_map(|o| self.open_url(o.calendar, day));
                agenda.push(Row::Day(label, link));
                for o in todays {
                    if rows >= MAX_ROWS {
                        hidden += 1;
                        continue;
                    }
                    rows += 1;
                    let time = match o.span {
                        Span::AllDay(..) => "all day".to_string(),
                        Span::Timed(start, stop) => {
                            if self.zones.to_local(start).date() < day {
                                format!("→{}", self.zones.to_local(stop).format("%H:%M"))
                            } else {
                                self.zones.to_local(start).format("%H:%M").to_string()
                            }
                        }
                    };
                    let mut title = o.summary.clone();
                    if title.is_empty() {
                        title = "(no title)".into();
                    }
                    if !o.location.is_empty()
                        && title.chars().count() + o.location.chars().count() < 24
                    {
                        title = format!("{title} · {}", o.location);
                    }
                    let text = format!("   {time:<7} {}", truncate(&title, 27));
                    let colour = self.colour(o.calendar, palette);
                    let url = self.open_url(o.calendar, day);
                    agenda.push(Row::Event(text, colour, url));
                }
            }
            day += Days::days(1);
        }
        if hidden > 0 {
            agenda.push(Row::Text(format!("   +{hidden} more")));
        }
        if list.is_empty() && self.settings.problems.is_empty() {
            let span = match self.settings.days_ahead {
                0 => "today".to_string(),
                1 => "today or tomorrow".to_string(),
                n => format!("in the next {n} days"),
            };
            agenda.push(Row::Text(format!("No events {span}")));
        }
        let status = self.status(now as u64);
        if let Some(status) = status {
            agenda.extend(wrap(&status, 38).into_iter().map(Row::Text));
        }
        agenda.insert(0, Row::Text(String::new()));
        for (offset, row) in agenda.into_iter().enumerate() {
            let line = at + offset;
            let text = match row {
                Row::Text(text) => text,
                Row::Day(text, url) => {
                    if let Some(url) = url {
                        decor.links.push(Link {
                            line,
                            col: 0,
                            cols: 40,
                            url,
                        });
                    }
                    text
                }
                Row::Event(text, colour, url) => {
                    decor.bullets.push(Bullet {
                        line,
                        col: 1,
                        colour,
                    });
                    if let Some(url) = url {
                        decor.links.push(Link {
                            line,
                            col: 0,
                            cols: 40,
                            url,
                        });
                    }
                    text
                }
            };
            lines.insert(line, text);
        }
    }

    /// Offline or failure state, if any.
    fn status(&self, now: u64) -> Option<String> {
        let mut parts = Vec::new();
        for (i, state) in self.feeds.iter().enumerate() {
            let name = &self.settings.feeds[i].name;
            match (&state.error, state.fetched) {
                (Some(e), None) => parts.push(format!("{name}: {e}")),
                (Some(_), Some(at)) => parts.push(format!(
                    "{name}: offline, from {} min ago",
                    now.saturating_sub(at) / 60
                )),
                (None, None) if self.rx.is_some() => parts.push(format!("{name}: loading…")),
                _ => {}
            }
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    }
}

/// Agenda rows (day headings and events), so the panel fits a 768 px screen.
const MAX_ROWS: usize = 16;

/// One agenda row: plain text, a day heading, or an event with its colour.
enum Row {
    Text(String),
    Day(String, Option<String>),
    Event(String, u32, Option<String>),
}

trait Stale {
    fn is_none_or_old(&self, refresh: u64) -> bool;
}

impl Stale for Option<u64> {
    fn is_none_or_old(&self, refresh: u64) -> bool {
        self.map_or(true, |t| now_secs().saturating_sub(t) >= refresh)
    }
}

/// Break text into lines of at most `width` characters, at spaces when it can.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let word = truncate(word, width);
        let needed = line.chars().count() + usize::from(!line.is_empty()) + word.chars().count();
        if needed > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines.truncate(4);
    lines
}

pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Fill `{yyyy}`, `{mm}`, `{m}`, `{dd}` and `{d}` in an open template.
pub fn open_url(template: &str, day: NaiveDate) -> String {
    template
        .replace("{yyyy}", &format!("{:04}", day.year()))
        .replace("{mm}", &format!("{:02}", day.month()))
        .replace("{m}", &day.month().to_string())
        .replace("{dd}", &format!("{:02}", day.day()))
        .replace("{d}", &day.day().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tz::Zone;

    const SECRET: &str =
        "https://calendar.google.com/calendar/ical/someone%40example.invalid/private-0123456789abcdef/basic.ics";

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nirubar-cal-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str, mode: u32) {
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_readable_secrets_file_is_refused_without_showing_urls() {
        let dir = temp("mode");
        let path = dir.join("calendars.toml");
        write(
            &path,
            &format!("[[calendar]]\nname = \"Privat\"\nurl = \"{SECRET}\"\n"),
            0o644,
        );
        let mut settings = Settings::default();
        read_feeds(&path, &mut settings);
        assert!(settings.feeds.is_empty());
        assert_eq!(settings.problems.len(), 1);
        assert!(
            settings.problems[0].contains("0644"),
            "{:?}",
            settings.problems
        );
        assert!(settings.problems[0].contains("chmod 600"));

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut settings = Settings::default();
        read_feeds(&path, &mut settings);
        assert!(settings.problems.is_empty(), "{:?}", settings.problems);
        assert_eq!(settings.feeds.len(), 1);
        let feed = &settings.feeds[0];
        assert_eq!(feed.name, "Privat");
        assert_eq!(
            feed.open.as_deref(),
            Some(GOOGLE_OPEN),
            "Google feeds open Google"
        );
        assert!(
            !format!("{feed:?}").contains("private-"),
            "Debug redacts the URL"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bad_entries_are_reported_by_name_only() {
        let dir = temp("bad");
        let path = dir.join("calendars.toml");
        write(
            &path,
            "[[calendar]]\nname = \"Plain\"\nurl = \"http://example.invalid/secret-token-123/x.ics\"\n\
             [[calendar]]\nname = \"Local\"\nurl = \"http://127.0.0.1:8080/x.ics\"\ncolor = \"#7aa2f7\"\n\
             open = \"https://example.invalid/day/{yyyy}-{mm}-{dd}\"\n\
             [[calendar]]\nurl = \"webcal://example.invalid/feed.ics\"\n",
            0o600,
        );
        let mut settings = Settings::default();
        read_feeds(&path, &mut settings);
        assert_eq!(settings.problems.len(), 1);
        assert!(settings.problems[0].starts_with("Plain:"));
        assert!(!settings.problems[0].contains("secret-token"));
        assert_eq!(settings.feeds.len(), 2);
        assert_eq!(settings.feeds[0].colour, Some(0x7aa2f7));
        assert_eq!(settings.feeds[1].name, "calendar 3");
        assert!(settings.feeds[1].url.starts_with("https://"));
        assert_eq!(settings.feeds[1].open, None);

        write(&path, &format!("[[calendar]\nurl = \"{SECRET}\"\n"), 0o600);
        let mut settings = Settings::default();
        read_feeds(&path, &mut settings);
        assert!(settings.problems[0].contains("not valid TOML"));
        assert!(!settings.problems[0].contains("private-"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn errors_are_redacted() {
        let err = format!("curl: (22) The requested URL returned error: 404 for {SECRET}");
        let clean = redact(&err, SECRET);
        assert!(!clean.contains("private-0123456789abcdef"), "{clean}");
        assert!(!clean.contains("someone"), "{clean}");
        let partial = redact("bad path /private-0123456789abcdef/basic.ics", SECRET);
        assert!(!partial.contains("0123456789abcdef"), "{partial}");
        assert_eq!(curl_quote("a\"b\\c"), "a\\\"b\\\\c");
    }

    #[test]
    fn options_have_bounds() {
        let mut settings = Settings::default();
        assert_eq!(settings.refresh, Duration::from_secs(15 * 60));
        assert_eq!(settings.days_ahead, 7);
        assert!(!settings.show_in_bar);
        let table: toml::Value = "refresh_minutes = 1\ndays_ahead = 99\nshow_in_bar = true"
            .parse()
            .unwrap();
        apply_options(&mut settings, &table);
        assert_eq!(settings.refresh, Duration::from_secs(5 * 60));
        assert_eq!(settings.days_ahead, 31);
        assert!(settings.show_in_bar);
    }

    #[test]
    fn open_templates_fill_the_date() {
        let day = NaiveDate::from_ymd_opt(2026, 3, 7).unwrap();
        assert_eq!(
            open_url(GOOGLE_OPEN, day),
            "https://calendar.google.com/calendar/r/day/2026/3/7"
        );
        assert_eq!(open_url("x/{yyyy}-{mm}-{dd}", day), "x/2026-03-07");
    }

    #[test]
    fn the_popup_gets_markers_an_agenda_and_links() {
        let feed = Feed {
            name: "Privat".into(),
            url: "https://calendar.google.com/x.ics".into(),
            colour: Some(0x7aa2f7),
            open: Some(GOOGLE_OPEN.into()),
        };
        let settings = Settings {
            feeds: vec![feed],
            ..Settings::default()
        };
        let mut cal = Calendar::new(settings).unwrap();
        cal.loaded = true;
        cal.zones = Zones::pinned(Zone::posix("CET-1CEST,M3.5.0,M10.5.0/3").unwrap());
        cal.feeds[0].fetched = Some(now_secs());
        cal.feeds[0].events = ics::parse(include_str!("../tests/fixtures/calendar/basic.ics"));
        let (title, mut lines) = crate::popup::calendar_for(2026, 11, "%Y-%m-%d");
        assert!(title.contains("November"));
        let mut decor = Decor::default();
        let palette = Palette {
            accent: 1,
            urgent: 2,
            foreground: 3,
        };
        let today = NaiveDate::from_ymd_opt(2026, 11, 9).unwrap();
        let now = Zones::pinned(Zone::posix("CET-1CEST,M3.5.0,M10.5.0/3").unwrap())
            .local_to_utc(today.and_hms_opt(7, 0, 0).unwrap());
        cal.decorate_at(2026, 11, today, now, &mut lines, &mut decor, &palette);
        let header = lines.iter().position(|l| l.starts_with("Wk  Mon")).unwrap();
        // November 2026 starts on a Sunday: the 9th is Monday of row 3.
        let ninth = decor
            .marks
            .iter()
            .find(|m| m.line == header + 3 && m.col == 5)
            .expect("a marker under the 9th");
        assert_eq!(ninth.colours, vec![0x7aa2f7]);
        // 13-15 all-day trip: Fri, Sat, Sun of the same row.
        assert!(decor
            .marks
            .iter()
            .any(|m| m.line == header + 3 && m.col == 4 + 6 * 4 + 1));
        assert!(
            !decor.marks.iter().any(|m| m.line == header + 2),
            "nothing in week 45"
        );
        let text = lines.join("\n");
        assert!(text.contains("Today · Mon 9 Nov"), "{text}");
        assert!(text.contains("09:00   Standup"), "{text}");
        assert!(text.contains("14:30   Tandläkare"), "{text}");
        assert!(text.contains("all day Fjällresa"), "{text}");
        assert!(text.contains("15:00   A summary"), "{text}");
        assert!(!text.contains("Cancelled"), "{text}");
        assert_eq!(lines.last().map(String::as_str), Some("←/→ change month"));
        let standup = lines.iter().position(|l| l.contains("Standup")).unwrap();
        assert!(decor
            .bullets
            .iter()
            .any(|b| b.line == standup && b.colour == 0x7aa2f7));
        assert_eq!(
            decor.link_at(standup, 10),
            Some("https://calendar.google.com/calendar/r/day/2026/11/9")
        );
        assert_eq!(
            decor.link_at(header + 4, 4 + 2 * 4 + 1),
            Some("https://calendar.google.com/calendar/r/day/2026/11/18"),
            "every day cell opens its date"
        );
    }

    #[test]
    fn unreadable_problems_show_in_the_popup() {
        let settings = Settings {
            problems: vec!["~/.config/nirubar/calendars.toml has mode 0644".into()],
            ..Settings::default()
        };
        let mut cal = Calendar::new(settings).unwrap();
        cal.loaded = true;
        let (_, mut lines) = crate::popup::calendar_for(2026, 11, "%Y-%m-%d");
        let mut decor = Decor::default();
        let palette = Palette {
            accent: 1,
            urgent: 2,
            foreground: 3,
        };
        let today = NaiveDate::from_ymd_opt(2026, 11, 9).unwrap();
        cal.decorate_at(2026, 11, today, 0, &mut lines, &mut decor, &palette);
        assert!(lines.iter().any(|l| l.contains("0644")));
        assert!(decor.links.is_empty());
    }

    fn feed(name: &str, url: &str) -> Feed {
        Feed {
            name: name.into(),
            url: url.into(),
            colour: None,
            open: None,
        }
    }

    #[test]
    fn new_settings_keep_known_feeds_and_fetch_new_ones_at_once() {
        // Loopback URLs that no cache on this machine matches; nothing here
        // fetches, and with no fetch time recorded nothing is written.
        let a = "http://127.0.0.1:9/unit-a.ics";
        let b = "http://127.0.0.1:9/unit-b.ics";
        let mut cal = Calendar::new(Settings {
            feeds: vec![feed("A", a)],
            ..Settings::default()
        })
        .unwrap();
        cal.loaded = true;
        cal.feeds[0].events = ics::parse(include_str!("../tests/fixtures/calendar/basic.ics"));
        let known = cal.feeds[0].events.len();
        assert!(known > 0);
        cal.last_try = Some(Instant::now());
        let (_tx, rx) = mpsc::channel();
        cal.rx = Some(rx);

        // Only an option changed: nothing is dropped or refetched.
        cal.apply(Settings {
            feeds: vec![feed("A", a)],
            days_ahead: 3,
            ..Settings::default()
        });
        assert_eq!(cal.settings.days_ahead, 3);
        assert!(cal.rx.is_some() && cal.last_try.is_some());

        // A calendar was added in front: A keeps its events at its new place,
        // the new one is due and the retry pause is lifted.
        cal.apply(Settings {
            feeds: vec![feed("B", b), feed("A", a)],
            ..Settings::default()
        });
        assert_eq!(cal.feeds.len(), 2);
        assert!(cal.feeds[0].events.is_empty());
        assert_eq!(cal.feeds[1].events.len(), known);
        assert!(cal.rx.is_none(), "results for the old list are dropped");
        assert!(cal.last_try.is_none(), "the new feed is fetched at once");
        assert!(cal.due());

        // Removed again.
        cal.apply(Settings {
            feeds: vec![feed("A", a)],
            ..Settings::default()
        });
        assert_eq!(cal.feeds.len(), 1);
        assert_eq!(cal.feeds[0].events.len(), known);
    }

    #[test]
    fn the_stamp_changes_with_content_mode_and_replacement() {
        let dir = temp("stamp");
        let path = dir.join("calendars.toml");
        assert_eq!(file_stamp(&path), None);
        write(&path, "[[calendar]]\n", 0o600);
        let first = file_stamp(&path);
        assert!(first.is_some());
        assert_eq!(file_stamp(&path), first, "unchanged file, same stamp");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let chmod = file_stamp(&path);
        assert_ne!(chmod, first, "chmod is a change");
        // An editor that writes a new file and renames it over the old one.
        let tmp = dir.join("calendars.toml.new");
        write(&tmp, "[[calendar]]\n", 0o644);
        std::fs::rename(&tmp, &path).unwrap();
        assert_ne!(file_stamp(&path), chmod, "a new inode is a change");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cache_files_are_private() {
        let dir = temp("cache");
        let path = dir.join("sub").join("0-privat.json");
        write_private(&path, b"{}").unwrap();
        let mode = std::fs::metadata(&path).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dir_mode = std::fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        let _ = std::fs::remove_dir_all(dir);
    }
}
