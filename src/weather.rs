//! Weather summary and panel backed by Open-Meteo.
//!
//! The bar never performs the request while painting. A stale cache is shown
//! immediately; a background thread refreshes it through the bounded probe
//! runner, and failures back off instead of retrying every frame.

use crate::probe;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug)]
pub struct Settings {
    /// True when a location was deliberately configured. This keeps the valid
    /// coordinates `0,0` distinct from "no location selected".
    pub configured: bool,
    pub location: String,
    pub latitude: f64,
    pub longitude: f64,
    pub refresh: Duration,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Api {
    #[serde(default)]
    pub current: Current,
    #[serde(default)]
    pub hourly: Hourly,
    #[serde(default)]
    pub daily: Daily,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Current {
    #[serde(default)]
    pub temperature_2m: Option<f64>,
    #[serde(default)]
    pub apparent_temperature: Option<f64>,
    #[serde(default)]
    pub precipitation: Option<f64>,
    #[serde(default)]
    pub weather_code: Option<u32>,
    #[serde(default)]
    pub wind_speed_10m: Option<f64>,
    #[serde(default)]
    pub wind_gusts_10m: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Hourly {
    #[serde(default)]
    pub time: Vec<String>,
    #[serde(default)]
    pub temperature_2m: Vec<f64>,
    #[serde(default)]
    pub precipitation_probability: Vec<Option<f64>>,
    #[serde(default)]
    pub precipitation: Vec<f64>,
    #[serde(default)]
    pub weather_code: Vec<u32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Daily {
    #[serde(default)]
    pub sunrise: Vec<String>,
    #[serde(default)]
    pub sunset: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Cache {
    schema: u32,
    fetched: u64,
    latitude: f64,
    longitude: f64,
    json: String,
}

fn cache_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        });
    base.join("nirubar").join("weather-v1.json")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Read `~/.config/nirubar/config.toml`, then fall back to the legacy shared
/// top-level coordinates. The own file is authoritative when it exists.
pub fn settings(legacy_latitude: f64, legacy_longitude: f64) -> Settings {
    let mut settings = Settings {
        configured: legacy_latitude != 0.0 || legacy_longitude != 0.0,
        location: String::new(),
        latitude: legacy_latitude,
        longitude: legacy_longitude,
        refresh: Duration::from_secs(30 * 60),
    };
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    let Ok(text) = std::fs::read_to_string(base.join("nirubar/config.toml")) else {
        return settings;
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return settings;
    };
    let Some(weather) = value.get("weather").and_then(|v| v.as_table()) else {
        return settings;
    };
    settings.configured = true;
    if let Some(s) = weather.get("location").and_then(|v| v.as_str()) {
        settings.location = s.to_string();
    }
    if let Some(n) = weather.get("latitude").and_then(|v| v.as_float()) {
        settings.latitude = n;
    }
    if let Some(n) = weather.get("longitude").and_then(|v| v.as_float()) {
        settings.longitude = n;
    }
    if let Some(n) = weather.get("refresh_minutes").and_then(|v| v.as_integer()) {
        if n > 0 {
            settings.refresh = Duration::from_secs((n as u64).min(24 * 60) * 60);
        }
    }
    settings
}

pub struct Weather {
    settings: Settings,
    cache: Option<(u64, Api)>,
    loaded: bool,
    rx: Option<Receiver<(u64, Result<Api, String>)>>,
    last_try: Option<Instant>,
    error: Option<String>,
}

impl Weather {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            cache: None,
            loaded: false,
            rx: None,
            last_try: None,
            error: None,
        }
    }

    fn enabled(&self) -> bool {
        self.settings.configured
    }

    fn load_cache(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let Ok(text) = std::fs::read_to_string(cache_path()) else {
            return;
        };
        let Ok(cache) = serde_json::from_str::<Cache>(&text) else {
            return;
        };
        if cache.schema != 1
            || (cache.latitude - self.settings.latitude).abs() > 1e-6
            || (cache.longitude - self.settings.longitude).abs() > 1e-6
        {
            return;
        }
        if let Ok(api) = serde_json::from_str::<Api>(&cache.json) {
            self.cache = Some((cache.fetched, api));
        }
    }

    fn save_cache(&self, fetched: u64, api: &Api) -> Option<()> {
        let json = serde_json::to_string(api).ok()?;
        let cache = Cache {
            schema: 1,
            fetched,
            latitude: self.settings.latitude,
            longitude: self.settings.longitude,
            json,
        };
        let text = serde_json::to_string(&cache).ok()?;
        let path = cache_path();
        std::fs::create_dir_all(path.parent()?).ok()?;
        let tmp = path.with_extension("json.new");
        std::fs::write(&tmp, text).ok()?;
        std::fs::rename(tmp, path).ok()?;
        Some(())
    }

    fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        match rx.try_recv() {
            Ok((fetched, Ok(api))) => {
                self.save_cache(fetched, &api);
                self.cache = Some((fetched, api));
                self.error = None;
                self.rx = None;
                self.last_try = Some(Instant::now());
            }
            Ok((_, Err(e))) => {
                self.error = Some(e);
                self.rx = None;
                self.last_try = Some(Instant::now());
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.error = Some("weather worker stopped".into());
                self.rx = None;
                self.last_try = Some(Instant::now());
            }
        }
    }

    fn start_fetch(&mut self) {
        if !self.enabled() || self.rx.is_some() {
            return;
        }
        let age = self
            .cache
            .as_ref()
            .map(|(t, _)| now_secs().saturating_sub(*t));
        if age.is_some_and(|a| a < self.settings.refresh.as_secs()) {
            return;
        }
        let wait = if self.error.is_some() { 60 } else { 5 };
        if self.last_try.is_some_and(|t| t.elapsed().as_secs() < wait) {
            return;
        }
        self.last_try = Some(Instant::now());
        let (tx, rx) = mpsc::channel();
        let latitude = self.settings.latitude;
        let longitude = self.settings.longitude;
        std::thread::spawn(move || {
            let fetched = now_secs();
            let result = fetch(latitude, longitude);
            let _ = tx.send((fetched, result));
        });
        self.rx = Some(rx);
    }

    fn ensure(&mut self) {
        self.load_cache();
        self.poll();
        self.start_fetch();
    }

    pub fn summary(&mut self) -> String {
        if !self.enabled() {
            return String::new();
        }
        self.ensure();
        let Some((_, api)) = &self.cache else {
            return "weather --".into();
        };
        let Some(temp) = api.current.temperature_2m else {
            return "weather --".into();
        };
        let condition = condition(api.current.weather_code.unwrap_or(0), true);
        format!("{condition} {:.0}°", temp)
    }

    pub fn panel(&mut self) -> (String, Vec<String>) {
        if !self.enabled() {
            return (
                "Weather".into(),
                vec![
                    "no weather location configured".into(),
                    String::new(),
                    "set [weather] location/coordinates in ~/.config/nirubar/config.toml".into(),
                ],
            );
        }
        self.ensure();
        let mut lines = Vec::new();
        if let Some((fetched, api)) = &self.cache {
            let c = &api.current;
            let name = if self.settings.location.is_empty() {
                format!(
                    "{:.2}, {:.2}",
                    self.settings.latitude, self.settings.longitude
                )
            } else {
                self.settings.location.clone()
            };
            lines.push(name);
            if let Some(t) = c.temperature_2m {
                lines.push(format!(
                    "{}  {:.0}°C{}",
                    condition(c.weather_code.unwrap_or(0), false),
                    t,
                    c.apparent_temperature
                        .map(|a| format!("  feels {a:.0}°"))
                        .unwrap_or_default()
                ));
            }
            if c.wind_speed_10m.is_some() || c.wind_gusts_10m.is_some() {
                lines.push(format!(
                    "wind {:.0} m/s{}",
                    c.wind_speed_10m.unwrap_or(0.0),
                    c.wind_gusts_10m
                        .map(|g| format!("  gusts {g:.0}"))
                        .unwrap_or_default()
                ));
            }
            if let Some(p) = c.precipitation {
                lines.push(format!("precipitation {p:.1} mm"));
            }
            lines.push(String::new());
            lines.push("next hours".into());
            let now = chrono::Local::now().naive_local();
            let mut shown = 0;
            for (i, time) in api.hourly.time.iter().enumerate() {
                let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(time, "%Y-%m-%dT%H:%M")
                else {
                    continue;
                };
                if parsed < now {
                    continue;
                }
                let temp = api.hourly.temperature_2m.get(i).copied().unwrap_or(0.0);
                let prob = api
                    .hourly
                    .precipitation_probability
                    .get(i)
                    .copied()
                    .flatten()
                    .unwrap_or(0.0);
                lines.push(format!(
                    "{}  {:.0}°  rain {:.0}%",
                    parsed.format("%H:%M"),
                    temp,
                    prob
                ));
                shown += 1;
                if shown == 6 {
                    break;
                }
            }
            if shown == 0 {
                lines.push("no hourly forecast in the response".into());
            }
            if let (Some(rise), Some(set)) = (api.daily.sunrise.first(), api.daily.sunset.first()) {
                lines.push(String::new());
                lines.push(format!(
                    "sunrise {}  sunset {}",
                    rise.split('T').nth(1).unwrap_or(rise),
                    set.split('T').nth(1).unwrap_or(set)
                ));
            }
            lines.push(String::new());
            let age = now_secs().saturating_sub(*fetched);
            lines.push(format!(
                "updated {} min ago{}",
                age / 60,
                if self.error.is_some() {
                    "  (stale)"
                } else {
                    ""
                }
            ));
        } else {
            lines.push("no weather data yet".into());
        }
        if let Some(e) = &self.error {
            lines.push(format!("last error: {e}"));
        }
        ("Weather".into(), lines)
    }
}

fn fetch(latitude: f64, longitude: f64) -> Result<Api, String> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={latitude}&longitude={longitude}\
         &current=temperature_2m,apparent_temperature,precipitation,weather_code,wind_speed_10m,wind_gusts_10m\
         &hourly=temperature_2m,precipitation_probability,precipitation,weather_code\
         &daily=sunrise,sunset&timezone=auto&forecast_days=2"
    );
    let out = probe::run(
        "curl",
        &[
            "-fsS",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--max-time",
            "12",
            "--max-filesize",
            "262144",
            &url,
        ],
        &[],
        Duration::from_secs(15),
        256 * 1024,
    )
    .ok_or("weather request timed out or curl is unavailable")?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    serde_json::from_slice::<Api>(&out.stdout).map_err(|e| format!("invalid weather response: {e}"))
}

fn condition(code: u32, short: bool) -> &'static str {
    match code {
        0 => {
            if short {
                "clear"
            } else {
                "clear sky"
            }
        }
        1 => {
            if short {
                "mostly clear"
            } else {
                "mainly clear"
            }
        }
        2 => "partly cloudy",
        3 => "overcast",
        45 | 48 => "fog",
        51 | 53 | 55 => "drizzle",
        56 | 57 => "freezing drizzle",
        61 | 63 | 65 => "rain",
        66 | 67 => "freezing rain",
        71 | 73 | 75 | 77 => "snow",
        80..=82 => "showers",
        85 | 86 => "snow showers",
        95 => "thunderstorm",
        96 | 99 => "thunderstorm with hail",
        _ => "weather",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "current": {"temperature_2m": 9.4, "apparent_temperature": 7.1, "precipitation": 0.2,
                  "weather_code": 3, "wind_speed_10m": 4.2, "wind_gusts_10m": 7.0},
      "hourly": {"time": ["2026-10-19T14:00", "2026-10-19T15:00"],
                 "temperature_2m": [9.0, 8.5], "precipitation_probability": [20, 35],
                 "precipitation": [0.0, 0.2], "weather_code": [3, 61]},
      "daily": {"sunrise": ["2026-10-19T07:44"], "sunset": ["2026-10-19T17:31"]}
    }"#;

    #[test]
    fn fixture_parses_and_formats() {
        let api: Api = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(api.current.temperature_2m, Some(9.4));
        assert_eq!(condition(3, true), "overcast");
        let settings = Settings {
            configured: true,
            location: "Ljungskile".into(),
            latitude: 58.2,
            longitude: 11.9,
            refresh: Duration::from_secs(1800),
        };
        let mut weather = Weather::new(settings);
        weather.cache = Some((now_secs(), api));
        assert_eq!(weather.summary(), "overcast 9°");
        let (_, lines) = weather.panel();
        assert!(lines.iter().any(|l| l.contains("Ljungskile")));
        assert!(lines.iter().any(|l| l.contains("sunrise")));
    }

    #[test]
    fn configured_zero_coordinates_are_not_missing() {
        let settings = Settings {
            configured: true,
            location: "gulf".into(),
            latitude: 0.0,
            longitude: 0.0,
            refresh: Duration::from_secs(1800),
        };
        let mut weather = Weather::new(settings);
        weather.loaded = true;
        weather.cache = Some((
            now_secs(),
            Api {
                current: Current {
                    temperature_2m: Some(27.0),
                    ..Current::default()
                },
                ..Api::default()
            },
        ));
        assert!(weather.summary().contains("27"));
    }

    #[test]
    fn a_bad_response_is_an_error_not_a_panic() {
        assert!(serde_json::from_slice::<Api>(b"{not json").is_err());
    }

    #[test]
    #[ignore = "requires a live Open-Meteo request"]
    fn live_open_meteo_response_parses() {
        let api = fetch(58.2, 11.9).expect("live weather request");
        assert!(api.current.temperature_2m.is_some());
        assert!(!api.hourly.time.is_empty());
    }

    #[test]
    fn cache_schema_is_versioned() {
        let c = Cache {
            schema: 1,
            fetched: 0,
            latitude: 0.0,
            longitude: 0.0,
            json: "{}".into(),
        };
        assert!(serde_json::to_string(&c).unwrap().contains("\"schema\":1"));
    }
}
