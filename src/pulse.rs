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

/// Used percentage of one mounted filesystem, using `statvfs`.
fn filesystem_percent(path: &Path) -> Option<u8> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    // `statvfs` is in libc, which Rust already links on Linux. The struct is
    // defined here rather than pulled in as a dependency.
    #[repr(C)]
    struct StatVfs {
        block_size: u64,
        fragment_size: u64,
        blocks: u64,
        free_blocks: u64,
        available_blocks: u64,
        files: u64,
        free_files: u64,
        available_files: u64,
        filesystem_id: u64,
        mount_flags: u64,
        maximum_name_length: u64,
    }
    extern "C" {
        fn statvfs(path: *const std::os::raw::c_char, buf: *mut StatVfs) -> std::os::raw::c_int;
    }
    let mut buf = StatVfs {
        block_size: 0,
        fragment_size: 0,
        blocks: 0,
        free_blocks: 0,
        available_blocks: 0,
        files: 0,
        free_files: 0,
        available_files: 0,
        filesystem_id: 0,
        mount_flags: 0,
        maximum_name_length: 0,
    };
    // SAFETY: `c` is a valid NUL-terminated path and `buf` points to a
    // correctly sized struct that lives for the call.
    let rc = unsafe { statvfs(c.as_ptr(), &mut buf) };
    if rc != 0 || buf.blocks == 0 {
        return None;
    }
    let used = buf.blocks.saturating_sub(buf.free_blocks);
    Some(((used.saturating_mul(100)) / buf.blocks).min(100) as u8)
}

/// Take one sample, given the counters read at the same moment.
pub fn sample(now: &CpuCounters, previous: &CpuCounters) -> Sample {
    Sample {
        cpu: now.percentage_since(previous),
        memory: memory_percent().unwrap_or(0),
        disk: disk_percent().unwrap_or(0),
    }
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
