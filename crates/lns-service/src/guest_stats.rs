use lns_ipc::{Response, RunStatsInfo};

pub const SAMPLE_MARKER: &str = "---LNS-STATS---";

const GUEST_BUSYBOX: &str = "/.lens/guest-tools/bin/busybox";

pub fn sample_argv() -> Vec<String> {
    vec![
        GUEST_BUSYBOX.to_string(),
        "sh".to_string(),
        "-c".to_string(),
        format!("cat /proc/stat /proc/meminfo; echo {SAMPLE_MARKER}; sleep 1; cat /proc/stat"),
    ]
}

pub fn response_from(captured: anyhow::Result<String>) -> Response {
    match captured {
        Ok(output) => match parse_samples(&output) {
            Ok(stats) => Response::RunStats { stats },
            Err(e) => Response::Error {
                message: format!("parsing guest stats failed: {e}"),
            },
        },
        Err(e) => Response::Error {
            message: format!("sampling guest stats failed: {e:#}"),
        },
    }
}

pub fn parse_samples(output: &str) -> Result<RunStatsInfo, String> {
    let (first, second) = output
        .split_once(SAMPLE_MARKER)
        .ok_or_else(|| format!("sample marker {SAMPLE_MARKER:?} not found"))?;
    let before = parse_cpu_line(first).ok_or("first sample is missing an aggregate cpu line")?;
    let after = parse_cpu_line(second).ok_or("second sample is missing an aggregate cpu line")?;
    let (mem_total_bytes, mem_available_bytes) =
        parse_meminfo(first).ok_or("MemTotal/MemAvailable not found in the first sample")?;
    Ok(RunStatsInfo {
        cpu_permille: cpu_permille(before, after),
        mem_used_bytes: mem_total_bytes.saturating_sub(mem_available_bytes),
        mem_total_bytes,
    })
}

#[derive(Debug, Clone, Copy)]
struct CpuSample {
    busy: u64,
    total: u64,
}

fn cpu_permille(before: CpuSample, after: CpuSample) -> u32 {
    let busy = after.busy.saturating_sub(before.busy);
    let total = after.total.saturating_sub(before.total);
    if total == 0 {
        return 0;
    }
    ((busy * 1000) / total) as u32
}

fn parse_cpu_line(text: &str) -> Option<CpuSample> {
    let line = text.lines().find(|l| l.starts_with("cpu "))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .map_while(|f| f.parse().ok())
        .collect();
    if fields.len() < 5 {
        return None;
    }
    let field = |i: usize| fields.get(i).copied().unwrap_or(0);
    let busy = field(0) + field(1) + field(2) + field(5) + field(6) + field(7);
    let idle = field(3) + field(4);
    Some(CpuSample {
        busy,
        total: busy + idle,
    })
}

fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let kb_of = |key: &str| {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    Some((kb_of("MemTotal:")? * 1024, kb_of("MemAvailable:")? * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_output(busy_a: u64, idle_a: u64, busy_b: u64, idle_b: u64) -> String {
        format!(
            "cpu  {busy_a} 0 0 {idle_a} 0 0 0 0 0 0\n\
             cpu0 1 0 0 1 0 0 0 0 0 0\n\
             MemTotal:         524288 kB\n\
             MemFree:          131072 kB\n\
             MemAvailable:     262144 kB\n\
             {SAMPLE_MARKER}\n\
             cpu  {busy_b} 0 0 {idle_b} 0 0 0 0 0 0\n"
        )
    }

    #[test]
    fn sample_argv_drives_the_bundled_busybox_not_the_image_shell() {
        let argv = sample_argv();
        assert_eq!(argv[0], "/.lens/guest-tools/bin/busybox");
        assert_eq!(argv[1], "sh");
        assert!(argv[3].contains("/proc/stat"));
        assert!(argv[3].contains("/proc/meminfo"));
        assert!(argv[3].contains(SAMPLE_MARKER));
    }

    #[test]
    fn parse_samples_computes_cpu_share_from_the_two_stat_snapshots() {
        let stats = parse_samples(&sample_output(100, 100, 150, 150)).unwrap();
        assert_eq!(stats.cpu_permille, 500, "50 busy of 100 jiffies is 50.0%");
    }

    #[test]
    fn parse_samples_reports_memory_used_as_total_minus_available() {
        let stats = parse_samples(&sample_output(0, 100, 0, 200)).unwrap();
        assert_eq!(stats.mem_total_bytes, 524_288 * 1024);
        assert_eq!(stats.mem_used_bytes, (524_288 - 262_144) * 1024);
    }

    #[test]
    fn an_idle_guest_reports_zero_cpu() {
        let stats = parse_samples(&sample_output(100, 100, 100, 200)).unwrap();
        assert_eq!(stats.cpu_permille, 0);
    }

    #[test]
    fn a_zero_jiffy_delta_reports_zero_cpu_instead_of_dividing_by_zero() {
        let stats = parse_samples(&sample_output(100, 100, 100, 100)).unwrap();
        assert_eq!(stats.cpu_permille, 0);
    }

    #[test]
    fn a_missing_marker_is_a_parse_error() {
        let err = parse_samples("cpu  1 2 3 4 5\n").unwrap_err();
        assert!(err.contains("marker"), "got: {err}");
    }

    #[test]
    fn a_missing_cpu_line_is_a_parse_error() {
        let err = parse_samples(&format!(
            "MemTotal: 1 kB\n{SAMPLE_MARKER}\ncpu  1 0 0 1 0\n"
        ))
        .unwrap_err();
        assert!(err.contains("first sample"), "got: {err}");
        let err = parse_samples(&format!(
            "cpu  1 0 0 1 0\nMemTotal: 1 kB\n{SAMPLE_MARKER}\n"
        ))
        .unwrap_err();
        assert!(err.contains("second sample"), "got: {err}");
    }

    #[test]
    fn missing_meminfo_keys_are_a_parse_error() {
        let err = parse_samples(&format!(
            "cpu  1 0 0 1 0\n{SAMPLE_MARKER}\ncpu  2 0 0 2 0\n"
        ))
        .unwrap_err();
        assert!(err.contains("MemTotal"), "got: {err}");
    }

    #[test]
    fn a_truncated_cpu_line_is_rejected() {
        let text = format!(
            "cpu  1 2 3\nMemTotal: 1 kB\nMemAvailable: 1 kB\n{SAMPLE_MARKER}\ncpu  1 0 0 1 0\n"
        );
        let err = parse_samples(&text).unwrap_err();
        assert!(err.contains("first sample"), "got: {err}");
    }

    #[test]
    fn counter_wraparound_clamps_to_zero_rather_than_underflowing() {
        let stats = parse_samples(&sample_output(200, 200, 100, 100)).unwrap();
        assert_eq!(stats.cpu_permille, 0);
    }

    #[test]
    fn response_from_wraps_parsed_stats() {
        let resp = response_from(Ok(sample_output(0, 0, 50, 50)));
        match resp {
            Response::RunStats { stats } => assert_eq!(stats.cpu_permille, 500),
            other => panic!("expected RunStats, got {other:?}"),
        }
    }

    #[test]
    fn response_from_surfaces_a_parse_failure() {
        let resp = response_from(Ok("garbage".to_string()));
        match resp {
            Response::Error { message } => {
                assert!(message.contains("parsing guest stats failed"), "{message}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn response_from_surfaces_a_capture_failure() {
        let resp = response_from(Err(anyhow::anyhow!("vsock unreachable")));
        match resp {
            Response::Error { message } => {
                assert!(message.contains("sampling guest stats failed"), "{message}");
                assert!(message.contains("vsock unreachable"), "{message}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }
}
