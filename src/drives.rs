//! Removable drives: USB sticks, SD cards and external disks, from UDisks2.
//!
//! The bar samples UDisks2's object manager every few seconds on the
//! module's own worker (one D-Bus call, no helper process) and shows a mark
//! only while something removable is attached. Mounting, opening and ejecting
//! run in a separate `nirubar --drives ...` process started by an explicit
//! click, so a slow or busy drive never holds up the bar.
//!
//! NIRUX does not auto-mount, so nothing is fought here: a drive is mounted
//! when you ask to open it (UDisks2 mounts it under `/media/$USER`), and
//! files on it stay where the file manager put them.

use crate::objects::{self, Interfaces, Objects, Prop};
use std::time::Duration;

const SERVICE: &str = "org.freedesktop.UDisks2";
const ROOT: &str = "/org/freedesktop/UDisks2";
const BLOCK: &str = "org.freedesktop.UDisks2.Block";
const FILESYSTEM: &str = "org.freedesktop.UDisks2.Filesystem";
const DRIVE: &str = "org.freedesktop.UDisks2.Drive";
/// Environment variable naming a fixture file used instead of the bus.
pub const FIXTURE: &str = "NIRUBAR_DRIVES_FIXTURE";

/// One filesystem on a removable drive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// UDisks2 block object, used for mount and unmount.
    pub block: String,
    /// The drive object it belongs to, used to power the drive off.
    pub drive: String,
    /// `/dev/sdb1`.
    pub device: String,
    /// The filesystem label, or the drive's model when it has none.
    pub label: String,
    pub size: u64,
    pub mount: Option<String>,
    pub can_power_off: bool,
    pub ejectable: bool,
}

/// The removable filesystems in `objects`, sorted by device name.
pub fn volumes(objects: &Objects) -> Vec<Volume> {
    // A drive with any filesystem the system owns (in /etc/fstab or
    // /etc/crypttab, or mounted outside /media) is left out as a whole, so
    // "eject all" can never power off a backup disk that is always attached.
    let system_drives: std::collections::BTreeSet<&str> = objects
        .values()
        .filter(|o| o.contains_key(FILESYSTEM) && system_owned(o))
        .map(|o| objects::text(o, BLOCK, "Drive"))
        .collect();
    let mut out = Vec::new();
    for (path, object) in objects {
        if !object.contains_key(FILESYSTEM) {
            continue;
        }
        if objects::flag(object, BLOCK, "HintIgnore") || objects::flag(object, BLOCK, "HintSystem")
        {
            continue;
        }
        if system_drives.contains(objects::text(object, BLOCK, "Drive")) {
            continue;
        }
        let mount = mount_point(object);
        let drive_path = objects::text(object, BLOCK, "Drive");
        let Some(drive) = objects.get(drive_path) else {
            continue;
        };
        if !removable(drive) {
            continue;
        }
        let device = device_name(object);
        let label = [
            objects::text(object, BLOCK, "IdLabel").to_string(),
            objects::text(object, BLOCK, "HintName").to_string(),
            model(drive),
            device.trim_start_matches("/dev/").to_string(),
        ]
        .into_iter()
        .find(|s| !s.trim().is_empty())
        .unwrap_or_default();
        out.push(Volume {
            block: path.clone(),
            drive: drive_path.to_string(),
            device,
            label,
            size: objects::number(object, BLOCK, "Size").unwrap_or(0),
            mount,
            can_power_off: objects::flag(drive, DRIVE, "CanPowerOff"),
            ejectable: objects::flag(drive, DRIVE, "Ejectable"),
        });
    }
    out.sort_by(|a, b| a.device.cmp(&b.device));
    out
}

fn system_owned(object: &Interfaces) -> bool {
    let configured = matches!(
        objects::prop(object, BLOCK, "Configuration"),
        Some(Prop::Count(n)) if *n > 0
    );
    let mounted_elsewhere = mount_point(object)
        .is_some_and(|m| !m.starts_with("/media/") && !m.starts_with("/run/media/"));
    configured || mounted_elsewhere
}

/// A drive the user can take out: UDisks2 marks USB sticks and card readers
/// removable; an external disk on USB or Thunderbolt is not always marked,
/// so its bus counts too.
fn removable(drive: &Interfaces) -> bool {
    objects::flag(drive, DRIVE, "Removable")
        || objects::flag(drive, DRIVE, "MediaRemovable")
        || objects::flag(drive, DRIVE, "Ejectable")
        || matches!(
            objects::text(drive, DRIVE, "ConnectionBus"),
            "usb" | "sdio" | "ieee1394"
        )
}

fn model(drive: &Interfaces) -> String {
    let vendor = objects::text(drive, DRIVE, "Vendor").trim();
    let model = objects::text(drive, DRIVE, "Model").trim();
    format!("{vendor} {model}").trim().to_string()
}

fn bytes_text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn device_name(object: &Interfaces) -> String {
    match objects::prop(object, BLOCK, "PreferredDevice")
        .or_else(|| objects::prop(object, BLOCK, "Device"))
    {
        Some(Prop::Bytes(list)) if !list.is_empty() => bytes_text(&list[0]),
        // UDisks2 sends `ay`, which reduces to a list of single bytes.
        Some(Prop::Text(t)) => t.clone(),
        _ => String::new(),
    }
}

fn mount_point(object: &Interfaces) -> Option<String> {
    match objects::prop(object, FILESYSTEM, "MountPoints") {
        Some(Prop::Bytes(list)) => list.iter().map(|m| bytes_text(m)).find(|m| !m.is_empty()),
        _ => None,
    }
}

/// Free bytes on a mounted filesystem.
pub fn free_bytes(mount: &str) -> Option<u64> {
    let path = std::ffi::CString::new(mount).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

pub fn size_text(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb >= 100.0 {
        format!("{gb:.0} GB")
    } else if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / 1e6)
    }
}

/// Sample the drives now.
pub fn read(reader: &mut objects::Reader) -> Option<Vec<Volume>> {
    if let Some(fixture) = objects::fixture(FIXTURE) {
        return Some(volumes(&fixture));
    }
    reader.objects(SERVICE, ROOT).map(|o| volumes(&o))
}

/// The module's background sampler.
pub struct Monitor {
    sample: crate::module::Cached<Vec<Volume>>,
}

impl Monitor {
    pub fn new() -> Self {
        let reader = std::sync::Mutex::new(objects::Reader::new());
        Self {
            sample: crate::module::Cached::new(Duration::from_secs(3), move || {
                let mut reader = reader.lock().ok()?;
                read(&mut reader)
            }),
        }
    }

    pub fn volumes(&self) -> Vec<Volume> {
        self.sample.get().unwrap_or_default()
    }
}

/// The bar text: nothing, the one drive's label, or a count.
pub fn text(volumes: &[Volume], icon: &str) -> String {
    match volumes {
        [] => String::new(),
        [one] => {
            let label: String = one.label.chars().take(12).collect();
            format!("{icon} {label}")
        }
        many => format!("{icon} {}", many.len()),
    }
}

pub fn detail(volumes: &[Volume]) -> Vec<String> {
    let mut lines = Vec::new();
    if volumes.is_empty() {
        lines.push("no removable drive".into());
    }
    for v in volumes {
        let place = match &v.mount {
            Some(m) => match free_bytes(m) {
                Some(free) => format!("{m} · {} free", size_text(free)),
                None => m.clone(),
            },
            None => "not mounted".into(),
        };
        lines.push(format!("{} · {} · {place}", v.label, size_text(v.size)));
    }
    lines.extend([
        String::new(),
        "right-click: eject all safely".into(),
        "middle-click: open in the file manager".into(),
    ]);
    lines
}

// ---------------------------------------------------------------------------
// `nirubar --drives list|open|eject`: the actions, in their own process.

fn notify(summary: &str, body: &str) {
    let _ = std::process::Command::new("notify-send")
        .args(["-a", "NIRUX", "-i", "drive-removable-media", summary, body])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

struct Bus {
    runtime: tokio::runtime::Runtime,
    connection: zbus::Connection,
}

impl Bus {
    fn connect() -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let connection = runtime
            .block_on(zbus::Connection::system())
            .map_err(|e| format!("system bus: {e}"))?;
        Ok(Self {
            runtime,
            connection,
        })
    }

    /// Call `method` on `path`, waiting at most `seconds` (a polkit prompt
    /// may be open meanwhile).
    fn call<R>(
        &self,
        path: &str,
        iface: &str,
        method: &str,
        body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
        seconds: u64,
    ) -> Result<R, String>
    where
        R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    {
        self.runtime.block_on(async {
            let proxy = zbus::Proxy::new(&self.connection, SERVICE, path, iface)
                .await
                .map_err(|e| e.to_string())?;
            match tokio::time::timeout(Duration::from_secs(seconds), proxy.call(method, body)).await
            {
                Ok(Ok(reply)) => Ok(reply),
                Ok(Err(e)) => Err(short_error(&e.to_string())),
                Err(_) => Err("no answer from UDisks2".into()),
            }
        })
    }
}

/// UDisks2 errors carry the D-Bus name first; people need the reason.
fn short_error(text: &str) -> String {
    if text.contains("target is busy") || text.contains("DeviceBusy") {
        return "it is in use: close the files and programs using it".into();
    }
    if text.contains("NotAuthorized") {
        return "not allowed".into();
    }
    text.rsplit(": ").next().unwrap_or(text).trim().to_string()
}

/// With a fixture, record the action next to it instead of touching drives.
fn fixture_log(line: &str) -> bool {
    let Some(path) = std::env::var_os(FIXTURE) else {
        return false;
    };
    use std::io::Write;
    let mut log = std::path::PathBuf::from(path).into_os_string();
    log.push(".actions");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    {
        let _ = writeln!(f, "{line}");
    }
    println!("{line}");
    true
}

type Options<'a> = std::collections::HashMap<&'a str, zbus::zvariant::Value<'a>>;

fn current() -> Result<Vec<Volume>, String> {
    let mut reader = objects::Reader::new();
    read(&mut reader).ok_or_else(|| "UDisks2 is not available".into())
}

fn mount(bus: &Bus, v: &Volume) -> Result<String, String> {
    if let Some(m) = &v.mount {
        return Ok(m.clone());
    }
    bus.call::<String>(&v.block, FILESYSTEM, "Mount", &(Options::new(),), 120)
}

fn open() -> i32 {
    let volumes = match current() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("nirubar: {e}");
            return 1;
        }
    };
    let Some(first) = volumes.first() else {
        eprintln!("nirubar: no removable drive");
        return 1;
    };
    if fixture_log(&format!("open {}", first.device)) {
        return 0;
    }
    let result = Bus::connect().and_then(|bus| mount(&bus, first));
    match result {
        Ok(path) => {
            let _ = std::process::Command::new("xdg-open")
                .arg(&path)
                .stdin(std::process::Stdio::null())
                .spawn();
            println!("{path}");
            0
        }
        Err(e) => {
            notify(&format!("Could not open {}", first.label), &e);
            eprintln!("nirubar: {e}");
            1
        }
    }
}

fn eject() -> i32 {
    let volumes = match current() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("nirubar: {e}");
            return 1;
        }
    };
    if volumes.is_empty() {
        println!("no removable drive");
        return 0;
    }
    let names: Vec<&str> = volumes.iter().map(|v| v.device.as_str()).collect();
    if fixture_log(&format!("eject {}", names.join(" "))) {
        return 0;
    }
    let bus = match Bus::connect() {
        Ok(bus) => bus,
        Err(e) => {
            eprintln!("nirubar: {e}");
            return 1;
        }
    };
    let mut drives: Vec<&str> = volumes.iter().map(|v| v.drive.as_str()).collect();
    drives.dedup();
    let mut status = 0;
    for drive in drives {
        let parts: Vec<&Volume> = volumes.iter().filter(|v| v.drive == drive).collect();
        let label = parts[0].label.clone();
        // Unmounting flushes what is still being written.
        let unmounted = parts
            .iter()
            .filter(|v| v.mount.is_some())
            .try_for_each(|v| {
                bus.call::<()>(&v.block, FILESYSTEM, "Unmount", &(Options::new(),), 120)
            });
        if let Err(e) = unmounted {
            notify(&format!("{label} is not ejected"), &e);
            eprintln!("nirubar: {label}: {e}");
            status = 1;
            continue;
        }
        let first = parts[0];
        let off = if first.can_power_off {
            bus.call::<()>(drive, DRIVE, "PowerOff", &(Options::new(),), 30)
        } else if first.ejectable {
            bus.call::<()>(drive, DRIVE, "Eject", &(Options::new(),), 30)
        } else {
            Ok(())
        };
        // A card reader that refuses power-off is still safe once unmounted.
        let _ = off;
        notify(&format!("{label} can be removed"), "");
        println!("{label} can be removed");
    }
    status
}

/// `nirubar --drives [list|open|eject]`.
pub fn cli(args: &[String]) -> i32 {
    match args.first().map(String::as_str).unwrap_or("list") {
        "list" => match current() {
            Ok(volumes) => {
                for v in volumes {
                    println!(
                        "{}\t{}\t{}\t{}",
                        v.device,
                        v.label,
                        size_text(v.size),
                        v.mount.as_deref().unwrap_or("-")
                    );
                }
                0
            }
            Err(e) => {
                eprintln!("nirubar: {e}");
                1
            }
        },
        "open" => open(),
        "eject" => eject(),
        _ => {
            eprintln!("Usage: nirubar --drives [list|open|eject]");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIX: &str = "\
/b/sdb1\torg.freedesktop.UDisks2.Block\tDrive\ts:/d/stick
/b/sdb1\torg.freedesktop.UDisks2.Block\tPreferredDevice\ty:/dev/sdb1
/b/sdb1\torg.freedesktop.UDisks2.Block\tIdLabel\ts:KINGSTON
/b/sdb1\torg.freedesktop.UDisks2.Block\tSize\tn:16000000000
/b/sdb1\torg.freedesktop.UDisks2.Filesystem\tMountPoints\ty:/media/niru/KINGSTON
/d/stick\torg.freedesktop.UDisks2.Drive\tRemovable\tb:true
/d/stick\torg.freedesktop.UDisks2.Drive\tCanPowerOff\tb:true
/b/nvme\torg.freedesktop.UDisks2.Block\tDrive\ts:/d/nvme
/b/nvme\torg.freedesktop.UDisks2.Block\tHintSystem\tb:true
/b/nvme\torg.freedesktop.UDisks2.Filesystem\tMountPoints\ty:/
/d/nvme\torg.freedesktop.UDisks2.Drive\tConnectionBus\ts:
/b/sdc1\torg.freedesktop.UDisks2.Block\tDrive\ts:/d/disk
/b/sdc1\torg.freedesktop.UDisks2.Block\tPreferredDevice\ty:/dev/sdc1
/b/sdc1\torg.freedesktop.UDisks2.Block\tSize\tn:2000000000000
/b/sdc1\torg.freedesktop.UDisks2.Filesystem\tMountPoints\ty:
/d/disk\torg.freedesktop.UDisks2.Drive\tConnectionBus\ts:usb
/d/disk\torg.freedesktop.UDisks2.Drive\tVendor\ts:WD
/d/disk\torg.freedesktop.UDisks2.Drive\tModel\ts:Elements
/b/loop0\torg.freedesktop.UDisks2.Block\tDrive\ts:/
/b/sdd1\torg.freedesktop.UDisks2.Block\tDrive\ts:/d/backup
/b/sdd1\torg.freedesktop.UDisks2.Block\tConfiguration\tc:1
/b/sdd1\torg.freedesktop.UDisks2.Filesystem\tMountPoints\ty:/mnt/backup-usb
/b/sde1\torg.freedesktop.UDisks2.Block\tDrive\ts:/d/backup
/b/sde1\torg.freedesktop.UDisks2.Filesystem\tMountPoints\ty:/srv/data
/d/backup\torg.freedesktop.UDisks2.Drive\tConnectionBus\ts:usb
/b/loop0\torg.freedesktop.UDisks2.Filesystem\tMountPoints\ty:/snap
";

    #[test]
    fn only_removable_filesystems_are_listed() {
        let v = volumes(&objects::parse_fixture(FIX));
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].device, "/dev/sdb1");
        assert_eq!(v[0].label, "KINGSTON");
        assert_eq!(v[0].mount.as_deref(), Some("/media/niru/KINGSTON"));
        assert!(v[0].can_power_off);
        assert_eq!(v[1].label, "WD Elements");
        assert_eq!(v[1].mount, None);
        assert_eq!(text(&v, "usb"), "usb 2");
        assert_eq!(text(&v[..1], "usb"), "usb KINGSTON");
        assert_eq!(text(&[], "usb"), "");
        let lines = detail(&v);
        assert!(lines[0].starts_with("KINGSTON · 16.0 GB · /media/niru/KINGSTON"));
        assert_eq!(lines[1], "WD Elements · 2000 GB · not mounted");
    }

    #[test]
    fn sizes_and_errors_read_well() {
        assert_eq!(size_text(512_000_000), "512 MB");
        assert_eq!(size_text(64_000_000_000), "64.0 GB");
        assert_eq!(
            short_error(
                "org.freedesktop.UDisks2.Error.DeviceBusy: Error unmounting: target is busy"
            ),
            "it is in use: close the files and programs using it"
        );
    }
}
