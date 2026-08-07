mod real;
pub use real::probe;

use super::HostCapacity;

/// One reading of this machine, mapped without any syscall so every edge is testable: a probe that answered nothing, or nothing usable, yields `None` and a percentage falls back rather than booting a guessed size.
pub fn from_probe(cpus: Option<usize>, mem_bytes: Option<u64>) -> Option<HostCapacity> {
    const MIB: u64 = 1024 * 1024;
    let cpus = u8::try_from(cpus?).unwrap_or(u8::MAX);
    let mem_mib = usize::try_from(mem_bytes? / MIB).ok()?;
    (cpus >= 1 && mem_mib >= 1).then_some(HostCapacity { cpus, mem_mib })
}

/// The `MemTotal:` line of `/proc/meminfo`, in KiB.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn mem_total_kib(meminfo: &str) -> Option<u64> {
    meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kib| kib.parse::<u64>().ok())
}

/// Every cpu the kernel knows, from the `0-15`/`0,2-3` range list `/sys/devices/system/cpu/present` holds. Not `available_parallelism`, which honours this process's affinity and cgroup quota — the CLI and the service must read the same total or they size one run two ways.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn present_cpu_count(present: &str) -> Option<usize> {
    let mut total = 0usize;
    for part in present.trim().split(',') {
        let count = match part.split_once('-') {
            Some((first, last)) => {
                let first = first.parse::<usize>().ok()?;
                let last = last.parse::<usize>().ok()?;
                last.checked_sub(first)?.checked_add(1)?
            }
            None => {
                part.parse::<usize>().ok()?;
                1
            }
        };
        total = total.checked_add(count)?;
    }
    (total >= 1).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_probe_of_both_halves_becomes_a_capacity_in_whole_mib() {
        assert_eq!(
            from_probe(Some(10), Some(16 * 1024 * 1024 * 1024)),
            Some(HostCapacity {
                cpus: 10,
                mem_mib: 16384
            })
        );
    }

    #[test]
    fn a_half_the_probe_could_not_read_yields_no_capacity() {
        assert_eq!(from_probe(None, Some(1024 * 1024 * 1024)), None);
        assert_eq!(from_probe(Some(4), None), None);
        assert_eq!(from_probe(None, None), None);
    }

    #[test]
    fn a_reading_too_small_to_be_a_host_yields_no_capacity() {
        assert_eq!(
            from_probe(Some(0), Some(1024 * 1024 * 1024)),
            None,
            "a zero-core reading is a broken probe, not a host"
        );
        assert_eq!(
            from_probe(Some(4), Some(1024)),
            None,
            "under one whole MiB there is no share to take"
        );
    }

    #[test]
    fn a_core_count_beyond_a_byte_is_capped_rather_than_wrapped() {
        assert_eq!(
            from_probe(Some(4096), Some(1024 * 1024 * 1024)).map(|h| h.cpus),
            Some(u8::MAX),
            "a big machine must not wrap to a tiny core count"
        );
    }

    #[test]
    fn mem_total_is_read_from_the_meminfo_line_that_names_it() {
        let meminfo = "MemFree:         1000 kB\nMemTotal:       16384000 kB\n";
        assert_eq!(mem_total_kib(meminfo), Some(16_384_000));
        assert_eq!(mem_total_kib("MemFree: 10 kB\n"), None);
        assert_eq!(mem_total_kib("MemTotal: lots kB\n"), None);
        assert_eq!(mem_total_kib("MemTotal:\n"), None);
    }

    #[test]
    fn every_present_cpu_counts_whatever_shape_the_range_list_takes() {
        assert_eq!(present_cpu_count("0-15\n"), Some(16));
        assert_eq!(present_cpu_count("0"), Some(1));
        assert_eq!(present_cpu_count("0,2-3"), Some(3));
        assert_eq!(present_cpu_count("0-3,8-11"), Some(8));
    }

    #[test]
    fn a_range_list_that_makes_no_sense_yields_no_count() {
        assert_eq!(present_cpu_count(""), None);
        assert_eq!(present_cpu_count("lots"), None);
        assert_eq!(
            present_cpu_count("3-1"),
            None,
            "a reversed range is garbage"
        );
        assert_eq!(present_cpu_count("0-"), None);
        assert_eq!(present_cpu_count("0,,2"), None);
    }
}
