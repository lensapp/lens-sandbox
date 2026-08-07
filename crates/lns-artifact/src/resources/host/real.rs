use super::{HostCapacity, from_probe};

/// What this host reports, or `None` when either half is unreadable.
pub fn probe() -> Option<HostCapacity> {
    from_probe(total_cpus(), total_memory_bytes())
}

#[cfg(target_os = "linux")]
fn total_cpus() -> Option<usize> {
    let present = std::fs::read_to_string("/sys/devices/system/cpu/present").ok()?;
    super::present_cpu_count(&present)
}

#[cfg(target_os = "linux")]
fn total_memory_bytes() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    super::mem_total_kib(&meminfo).map(|kib| kib * 1024)
}

/// `available_parallelism` is the total logical core count on macOS; it is not on Linux, which is why only this arm uses it.
#[cfg(target_os = "macos")]
fn total_cpus() -> Option<usize> {
    std::thread::available_parallelism().ok().map(|n| n.get())
}

#[cfg(target_os = "macos")]
fn total_memory_bytes() -> Option<u64> {
    let out = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    String::from_utf8(out.stdout).ok()?.trim().parse().ok()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn total_cpus() -> Option<usize> {
    None
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn total_memory_bytes() -> Option<u64> {
    None
}
