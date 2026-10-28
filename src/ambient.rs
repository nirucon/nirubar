//! Additions that make the bar useful without being loud.
//!
//! Three small modules share one idea: say something only when it matters, and
//! always explain yourself when clicked.
//!
//! * **now playing artwork** is not possible in a text bar, but the media
//!   module already scrolls; what is missing is a *player* that is silent when
//!   nothing plays.
//! * **uptime** answers "how long has this machine been up", and doubles as a
//!   memory of when it last came back.
//! * **sun** shows when the sun sets, from the same latitude the weather
//!   module uses, so a walk can be planned from the bar.
//!
//! Everything is computed locally: no network call, no API key.

use std::fs;

/// How long the machine has been up, as a short phrase.
pub fn uptime_text() -> Option<String> {
    let raw = fs::read_to_string("/proc/uptime").ok()?;
    let seconds: f64 = raw.split_whitespace().next()?.parse().ok()?;
    Some(short_duration(seconds as u64))
}

/// "3d 4h", "5h 12m", "9m". Two units is plenty for a glance.
pub fn short_duration(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}

/// When the sun rises and sets, in local clock time.
///
/// The solar position is the simplified NOAA algorithm: accurate to a couple of
/// minutes between the tropics and the Arctic circle, which is all a bar needs.
/// Returns `None` inside the polar day and night, where there is no event.
pub fn sun_times(latitude: f64, longitude: f64) -> Option<(String, String)> {
    let (rise, set) = solar_events(latitude, longitude)?;
    Some((clock(rise), clock(set)))
}

fn clock(hours: f64) -> String {
    let total = (hours.rem_euclid(24.0) * 60.0).round() as i64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// Sunrise and sunset as hours after local midnight (solar noon corrected for
/// the equation of time and the longitude).
fn solar_events(latitude: f64, longitude: f64) -> Option<(f64, f64)> {
    let today = chrono::Local::now().date_naive();
    solar_events_on_day(latitude, longitude, chrono::Datelike::ordinal(&today) as f64)
}

fn solar_events_on_day(latitude: f64, longitude: f64, day: f64) -> Option<(f64, f64)> {
    // Fraction of the year, 0 at the start, from the day number.
    let gamma = 2.0 * std::f64::consts::PI / 365.0 * (day - 1.0);
    // Equation of time, in minutes.
    let eqtime = 229.18
        * (0.000075 + 0.001868 * gamma.cos()
            - 0.032077 * gamma.sin()
            - 0.014615 * (2.0 * gamma).cos()
            - 0.040849 * (2.0 * gamma).sin());
    // Solar declination, in radians.
    let decl = 0.006918 - 0.399912 * gamma.cos() + 0.070257 * gamma.sin()
        - 0.006758 * (2.0 * gamma).cos()
        + 0.000907 * (2.0 * gamma).sin()
        - 0.002697 * (3.0 * gamma).cos()
        + 0.00148 * (3.0 * gamma).sin();
    let lat = latitude.to_radians();
    // 90.833 degrees: the sun's radius plus refraction at the horizon.
    let zenith = 90.833_f64.to_radians();
    let cos_ha = (zenith.cos() / (lat.cos() * decl.cos())) - lat.tan() * decl.tan();
    if !(-1.0..=1.0).contains(&cos_ha) {
        return None;
    }
    let ha = cos_ha.acos().to_degrees();
    // Minutes from UTC midnight.
    let noon = 720.0 - 4.0 * longitude - eqtime;
    let rise = (noon - 4.0 * ha) / 60.0;
    let set = (noon + 4.0 * ha) / 60.0;
    // `noon` is UTC; the bar shows local time.
    let offset = local_utc_offset_hours();
    Some((rise + offset, set + offset))
}

/// The current local offset from UTC, in hours.
fn local_utc_offset_hours() -> f64 {
    use chrono::{Local, Offset};
    f64::from(Local::now().offset().fix().local_minus_utc()) / 3600.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(short_duration(59), "0m");
        assert_eq!(short_duration(9 * 60), "9m");
        assert_eq!(short_duration(5 * 3_600 + 12 * 60), "5h 12m");
        assert_eq!(short_duration(3 * 86_400 + 4 * 3_600), "3d 4h");
    }

    #[test]
    fn the_sun_rises_before_it_sets_away_from_the_poles() {
        let (rise, set) = sun_times(59.3, 18.0).expect("Stockholm has sun in this season");
        assert!(rise < set, "rise {rise} should be before set {set}");
    }

    #[test]
    fn above_the_arctic_circle_there_are_fixed_polar_days() {
        // 172 is midsummer and 355 is polar night at Longyearbyen. The result
        // must not depend on the date the test happens to run.
        let lat = 78.2;
        assert!(solar_events_on_day(lat, 15.6, 172.0).is_none());
        assert!(solar_events_on_day(lat, 15.6, 355.0).is_none());
        assert!(solar_events_on_day(lat, 15.6, 80.0).is_some());
    }

    #[test]
    fn uptime_is_readable() {
        assert!(uptime_text().is_some());
    }
}
