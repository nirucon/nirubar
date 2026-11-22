//! Battery of connected Bluetooth devices (headsets, mice, keyboards), read
//! from BlueZ's `org.bluez.Battery1` on the system bus: one object-manager
//! call on the module's worker whenever BlueZ signals a change (a device
//! connects, a battery level moves), no helper process and no polling.
//!
//! Hidden when no connected device reports a battery. At 15 % or less the
//! text turns red and one notification is shown per device and discharge;
//! the marker is cleared when the device is above 20 % again, so a bar on
//! each screen still notifies only once.

use crate::objects::{self, Objects};
use std::time::Duration;

const SERVICE: &str = "org.bluez";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";
pub const FIXTURE: &str = "NIRUBAR_BLUEZ_FIXTURE";
pub const LOW: u32 = 15;
const RECOVERED: u32 = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub address: String,
    pub name: String,
    pub percent: u32,
}

/// Connected devices with a battery, lowest first.
pub fn devices(objects: &Objects) -> Vec<Device> {
    let mut out: Vec<Device> = objects
        .values()
        .filter(|o| objects::flag(o, DEVICE, "Connected"))
        .filter_map(|o| {
            let percent = objects::number(o, BATTERY, "Percentage")? as u32;
            let name = [
                objects::text(o, DEVICE, "Alias"),
                objects::text(o, DEVICE, "Name"),
                objects::text(o, DEVICE, "Address"),
            ]
            .into_iter()
            .find(|n| !n.trim().is_empty())
            .unwrap_or("Bluetooth device")
            .to_string();
            Some(Device {
                address: objects::text(o, DEVICE, "Address").to_string(),
                name,
                percent: percent.min(100),
            })
        })
        .collect();
    out.sort_by(|a, b| a.percent.cmp(&b.percent).then(a.name.cmp(&b.name)));
    out
}

fn read(reader: &mut objects::Reader) -> Option<Vec<Device>> {
    if let Some(fixture) = objects::fixture(FIXTURE) {
        return Some(devices(&fixture));
    }
    // No BlueZ (no adapter, or not installed): nothing to show.
    Some(
        reader
            .objects(SERVICE, "/")
            .map(|o| devices(&o))
            .unwrap_or_default(),
    )
}

pub enum Monitor {
    Watched(objects::Watched<Vec<Device>>),
    Fixture(crate::module::Cached<Vec<Device>>),
}

fn sample(reader: &mut objects::Reader) -> Option<Vec<Device>> {
    let devices = read(reader)?;
    warn_low(&devices);
    Some(devices)
}

impl Monitor {
    pub fn new() -> Self {
        if std::env::var_os(FIXTURE).is_some() {
            let reader = std::sync::Mutex::new(objects::Reader::new());
            return Self::Fixture(crate::module::Cached::new(
                Duration::from_secs(1),
                move || sample(&mut *reader.lock().ok()?),
            ));
        }
        // BlueZ signals a battery change; the sample after at most a minute
        // is only a safety net for a missed signal.
        Self::Watched(objects::Watched::new(
            SERVICE,
            "/",
            Duration::from_secs(60),
            sample,
        ))
    }

    pub fn devices(&self) -> Vec<Device> {
        match self {
            Self::Watched(sample) => sample.get(),
            Self::Fixture(sample) => sample.get(),
        }
        .unwrap_or_default()
    }
}

fn marker(address: &str) -> std::path::PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let safe: String = address
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    dir.join(format!("nirubar-headset-low-{}-{safe}", unsafe {
        libc::getuid()
    }))
}

/// One notification per device and discharge, whichever bar sees it first.
fn warn_low(devices: &[Device]) {
    for d in devices {
        let path = marker(if d.address.is_empty() {
            &d.name
        } else {
            &d.address
        });
        if d.percent > RECOVERED {
            let _ = std::fs::remove_file(&path);
        } else if d.percent <= LOW
            && std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .is_ok()
            && !cfg!(test)
        {
            let _ = std::process::Command::new("notify-send")
                .args([
                    "-a",
                    "NIRUX",
                    "-i",
                    "battery-low",
                    &format!("{} battery low", d.name),
                    &format!("{} % left", d.percent),
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

pub fn text(devices: &[Device], icon: &str) -> String {
    match devices.first() {
        Some(d) => format!("{icon} {}%", d.percent),
        None => String::new(),
    }
}

pub fn low(devices: &[Device]) -> bool {
    devices.first().is_some_and(|d| d.percent <= LOW)
}

pub fn detail(devices: &[Device]) -> Vec<String> {
    let mut lines: Vec<String> = devices
        .iter()
        .map(|d| format!("{} · {}%", d.name, d.percent))
        .collect();
    if lines.is_empty() {
        lines.push("no connected device reports a battery".into());
    }
    lines.extend([
        String::new(),
        format!("red and one notification at {LOW} % or less"),
        "middle-click: Bluetooth devices".into(),
    ]);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connected_batteries_lowest_first() {
        let o = objects::parse_fixture(
            "/d/a\torg.bluez.Device1\tConnected\tb:true\n/d/a\torg.bluez.Device1\tAlias\ts:WH-1000XM4\n/d/a\torg.bluez.Battery1\tPercentage\tn:80\n\
/d/b\torg.bluez.Device1\tConnected\tb:true\n/d/b\torg.bluez.Device1\tName\ts:Mouse\n/d/b\torg.bluez.Battery1\tPercentage\tn:12\n\
/d/c\torg.bluez.Device1\tConnected\tb:false\n/d/c\torg.bluez.Battery1\tPercentage\tn:3\n\
/d/e\torg.bluez.Device1\tConnected\tb:true\n/d/e\torg.bluez.Device1\tAlias\ts:Speaker\n",
        );
        let d = devices(&o);
        assert_eq!(d.len(), 2);
        assert_eq!(text(&d, "hs"), "hs 12%");
        assert!(low(&d));
        assert_eq!(detail(&d)[1], "WH-1000XM4 · 80%");
        assert_eq!(text(&[], "hs"), "");
        assert!(!low(&d[1..]));
    }
}
