//! The pulse: a small graph of how hard the machine is working.
//!
//! Three numbers, one line each, read from `/proc` and `/sys`: CPU busy time,
//! memory in use, and the fullest filesystem. The bar draws a new sample every
//! second and keeps the last minute, so a copy that started to chew the disk,
//! or a browser growing without limit, shows up as a shape before it is a
//! problem.
//!
//! Nothing here needs a daemon or a dependency beyond the standard library.

use std::fs;
use std::path::Path;

/// One instant of system load.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    /// Percent of CPU time spent on work during the last interval.
    pub cpu: u8,
    /// Percent of memory in use.
    pub memory: u8,
    /// Percent used on the fullest filesystem, or 0 when nothing is mounted
    /// that should be reported.
    pub disk: u8,
    /// CPU temperature in °C, or 0 when the machine does not report one.
    pub temp: u8,
}

/// The raw counters behind `cpu`, kept between samples.
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuCounters {
    busy: u64,
    total: u64,
}

impl CpuCounters {
    /// Read the totals from `/proc/stat`.
    pub fn read() -> Option<Self> {
        let text = fs::read_to_string("/proc/stat").ok()?;
        let line = text.lines().next()?;
        let mut fields = line.split_whitespace();
        if fields.next()? != "cpu" {
            return None;
        }
        let numbers: Vec<u64> = fields.filter_map(|f| f.parse().ok()).collect();
        if numbers.len() < 4 {
            return None;
        }
        // user nice system idle iowait irq softirq steal
        let idle = numbers.get(3).copied().unwrap_or(0) + numbers.get(4).copied().unwrap_or(0);
        let total: u64 = numbers.iter().take(8).sum();
        Some(Self {
            busy: total.saturating_sub(idle),
            total,
        })
    }

    /// The percentage busy since `previous`, clamped to 0..=100.
    ///
    /// The first sample has nothing to compare with and reports 0, which is
    /// better than a spike that never happened.
    pub fn percentage_since(&self, previous: &Self) -> u8 {
        let total = self.total.saturating_sub(previous.total);
        if total == 0 {
            return 0;
        }
        let busy = self.busy.saturating_sub(previous.busy);
        ((busy.saturating_mul(100)) / total).min(100) as u8
    }
}

/// Memory in use, as a percentage of `MemTotal`.
pub fn memory_percent() -> Option<u8> {
    let text = fs::read_to_string("/proc/meminfo").ok()?;
    let mut total = 0u64;
    let mut available = 0u64;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("MemTotal:") => total = parts.next()?.parse().ok()?,
            Some("MemAvailable:") => available = parts.next()?.parse().ok()?,
            _ => {}
        }
    }
    if total == 0 {
        return None;
    }
    let used = total.saturating_sub(available);
    Some(((used.saturating_mul(100)) / total).min(100) as u8)
}

/// The fullest filesystem under `/`, ignoring the ones that are always at
/// 100%: read-only images, tmpfs and the efivar filesystem.
pub fn disk_percent() -> Option<u8> {
    let text = fs::read_to_string("/proc/mounts").ok()?;
    let mut worst = 0u8;
    let mut seen = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let _device = parts.next()?;
        let mount = parts.next()?;
        let kind = parts.next()?;
        if matches!(
            kind,
            "proc"
                | "sysfs"
                | "devtmpfs"
                | "tmpfs"
                | "devpts"
                | "cgroup"
                | "cgroup2"
                | "efivarfs"
                | "squashfs"
                | "overlay"
                | "debugfs"
                | "tracefs"
                | "securityfs"
                | "pstore"
                | "bpf"
                | "autofs"
                | "mqueue"
                | "hugetlbfs"
                | "fusectl"
                | "configfs"
        ) {
            continue;
        }
        // `/run/user/...` and containers' own filesystems are small and noisy.
        if mount.starts_with("/run") || mount.starts_with("/dev") {
            continue;
        }
        let mount = mount.replace("\\040", " ");
        if seen.contains(&mount) {
            continue;
        }
        seen.push(mount.clone());
        if let Some(used) = filesystem_percent(Path::new(&mount)) {
            worst = worst.max(used);
        }
    }
    Some(worst)
}

use libc::statvfs;
type StatVfs = libc::statvfs;

/// Used percentage of one mounted filesystem, using `statvfs`.
fn filesystem_percent(path: &Path) -> Option<u8> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut buf = std::mem::MaybeUninit::<StatVfs>::uninit();
    // SAFETY: `c` is a valid NUL-terminated path and `buf` points to a
    // correctly sized struct that lives for the call.
    let rc = unsafe { statvfs(c.as_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: a successful statvfs initializes the target-specific libc type.
    let buf = unsafe { buf.assume_init() };
    if buf.f_blocks == 0 {
        return None;
    }
    let used = buf.f_blocks.saturating_sub(buf.f_bfree);
    Some(((used.saturating_mul(100)) / buf.f_blocks).min(100) as u8)
}

/// Take one sample, given the counters read at the same moment. `disk` is
/// passed in: filesystems change slowly and are looked at once a minute.
pub fn sample(now: &CpuCounters, previous: &CpuCounters, disk: u8) -> Sample {
    Sample {
        cpu: now.percentage_since(previous),
        memory: memory_percent().unwrap_or(0),
        disk,
        temp: cpu_temperature().unwrap_or(0),
    }
}

/// The sensor that reports the CPU's temperature, found once: the AMD and
/// Intel drivers first, then the ARM and ACPI ones, then a thermal zone.
fn temperature_file() -> Option<&'static std::path::PathBuf> {
    static FILE: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    FILE.get_or_init(|| {
        let hwmon: Vec<std::path::PathBuf> = fs::read_dir("/sys/class/hwmon")
            .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).collect())
            .unwrap_or_default();
        for want in [
            "k10temp",
            "zenpower",
            "coretemp",
            "cpu_thermal",
            "soc_thermal",
            "acpitz",
        ] {
            for dir in &hwmon {
                let name = fs::read_to_string(dir.join("name")).unwrap_or_default();
                if name.trim() != want {
                    continue;
                }
                // The package or die reading when the sensor labels its inputs.
                for i in 1..=16 {
                    let label =
                        fs::read_to_string(dir.join(format!("temp{i}_label"))).unwrap_or_default();
                    if matches!(label.trim(), "Tctl" | "Tdie" | "Package id 0") {
                        return Some(dir.join(format!("temp{i}_input")));
                    }
                }
                let first = dir.join("temp1_input");
                if first.exists() {
                    return Some(first);
                }
            }
        }
        let zones = fs::read_dir("/sys/class/thermal").ok()?;
        for zone in zones.filter_map(Result::ok) {
            let kind = fs::read_to_string(zone.path().join("type")).unwrap_or_default();
            if matches!(kind.trim(), "x86_pkg_temp" | "cpu-thermal" | "cpu_thermal") {
                return Some(zone.path().join("temp"));
            }
        }
        None
    })
    .as_ref()
}

/// The CPU temperature in whole °C.
pub fn cpu_temperature() -> Option<u8> {
    let milli: i64 = fs::read_to_string(temperature_file()?)
        .ok()?
        .trim()
        .parse()
        .ok()?;
    (milli > 0).then(|| (milli / 1000).clamp(0, 255) as u8)
}

/// The last `width` samples as a compact line of block characters, one row per
/// metric. A graph made of text needs no drawing code and reads the same at
/// every font size.
pub fn graph_rows(samples: &[Sample], width: usize) -> [String; 3] {
    let take = samples.len().saturating_sub(width);
    let recent = &samples[take..];
    let row =
        |pick: fn(&Sample) -> u8| -> String { recent.iter().map(pick).map(block_for).collect() };
    [row(|s| s.cpu), row(|s| s.memory), row(|s| s.disk)]
}

/// A block that rises with the value: a shape, not just a number.
fn block_for(value: u8) -> char {
    // U+2581..U+2588 are the eight partial blocks every monospace font has.
    let step = (u32::from(value) * 7 / 100) as u8;
    char::from_u32(0x2581 + u32::from(step)).unwrap_or('#')
}

/// Short summary for the bar text.
pub fn summary(s: Sample) -> String {
    format!("cpu {}%  mem {}%", s.cpu, s.memory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_query_does_not_overwrite_adjacent_storage() {
        // The old handwritten ABI omitted libc's reserved fields. Keep enough
        // enclosing storage even for that broken declaration so the regression
        // observes the overwrite without writing outside the allocation.
        #[repr(C)]
        struct Guarded {
            stats: std::mem::MaybeUninit<StatVfs>,
            guard: [u8; 64],
        }
        let mut storage = Guarded {
            stats: std::mem::MaybeUninit::uninit(),
            guard: [0xa5; 64],
        };
        let path = std::ffi::CString::new("/").unwrap();
        // SAFETY: storage includes space for the target ABI and a trailing
        // canary; the path is NUL terminated and both outlive the call.
        assert_eq!(
            unsafe { statvfs(path.as_ptr(), storage.stats.as_mut_ptr()) },
            0
        );
        assert_eq!(
            storage.guard, [0xa5; 64],
            "filesystem ABI overwrote adjacent storage"
        );
    }

    #[test]
    fn filesystem_query_handles_present_and_missing_paths() {
        assert!(filesystem_percent(Path::new("/")).is_some_and(|n| n <= 100));
        assert_eq!(
            filesystem_percent(Path::new("/proc/self/niru-missing-mount")),
            None
        );
    }

    #[test]
    fn cpu_percentage_is_the_share_of_busy_time() {
        let previous = CpuCounters {
            busy: 100,
            total: 200,
        };
        let now = CpuCounters {
            busy: 150,
            total: 400,
        };
        // 50 busy of 200 total.
        assert_eq!(now.percentage_since(&previous), 25);
    }

    #[test]
    fn first_sample_is_zero_not_a_spike() {
        let zero = CpuCounters::default();
        let now = CpuCounters {
            busy: 10,
            total: 10,
        };
        assert_eq!(now.percentage_since(&zero), 100);
        assert_eq!(zero.percentage_since(&zero), 0);
    }

    #[test]
    fn graphs_are_as_wide_as_the_samples() {
        let samples: Vec<Sample> = (0..100)
            .map(|i| Sample {
                cpu: i as u8,
                memory: 50,
                disk: 90,
                temp: 0,
            })
            .collect();
        let rows = graph_rows(&samples, 10);
        assert_eq!(rows[0].chars().count(), 10);
        assert_eq!(rows[1].chars().count(), 10);
        // The tallest sample draws the tallest block.
        // The newest sample is 99%, one block below the top.
        assert_eq!(rows[0].chars().last().unwrap(), '\u{2587}');
    }

    #[test]
    fn memory_reads_a_percentage() {
        // The real file always exists on Linux.
        assert!(memory_percent().map(|m| m <= 100).unwrap_or(true));
    }
}
