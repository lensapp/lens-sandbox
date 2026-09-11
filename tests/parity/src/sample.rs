use crate::result::Sample;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const INTERVAL: Duration = Duration::from_secs(5);

pub fn rss_kib(pid: u32) -> Option<u64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    parse_rss(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_rss(output: &str) -> Option<u64> {
    output.split_whitespace().next()?.parse().ok()
}

pub fn open_fds(pid: u32) -> Option<u64> {
    if let Ok(output) = Command::new("lsof")
        .args(["-p", &pid.to_string(), "-Fn"])
        .output()
        && output.status.success()
    {
        return Some(count_lsof_descriptors(&String::from_utf8_lossy(
            &output.stdout,
        )));
    }
    let dir = std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
    Some(dir.filter(|entry| entry.is_ok()).count() as u64)
}

pub fn count_lsof_descriptors(output: &str) -> u64 {
    output
        .lines()
        .filter(|line| line.starts_with('f'))
        .filter(|line| line[1..].chars().all(|c| c.is_ascii_digit()))
        .count() as u64
}

pub struct Sampler {
    samples: Arc<Mutex<Vec<Sample>>>,
    stop: Arc<AtomicBool>,
}

impl Sampler {
    pub fn start(pid: u32, start: Instant) -> Self {
        let samples = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_samples = Arc::clone(&samples);
        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                let sample = Sample {
                    at_ms: start.elapsed().as_millis() as u64,
                    rss_kib: rss_kib(pid),
                    open_fds: open_fds(pid),
                };
                if let Ok(mut guard) = thread_samples.lock() {
                    guard.push(sample);
                }
                sleep_until_stop(&thread_stop, INTERVAL);
            }
        });
        Self { samples, stop }
    }

    pub fn take(&self) -> Vec<Sample> {
        self.samples
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop();
    }
}

fn sleep_until_stop(stop: &AtomicBool, total: Duration) {
    let step = Duration::from_millis(100);
    let mut slept = Duration::ZERO;
    while slept < total && !stop.load(Ordering::SeqCst) {
        std::thread::sleep(step);
        slept += step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_resident_size_is_the_first_field_ps_prints() {
        assert_eq!(parse_rss("  123456\n"), Some(123_456));
        assert_eq!(parse_rss("123456 7\n"), Some(123_456));
        assert_eq!(parse_rss(""), None);
        assert_eq!(parse_rss("no such process\n"), None);
    }

    #[test]
    fn only_the_descriptor_lines_of_lsof_are_counted() {
        let output = "p4242\nfcwd\nftxt\nf0\nn/dev/null\nf1\nn/dev/null\nf12\nn/tmp/x\n";
        assert_eq!(count_lsof_descriptors(output), 3);
        assert_eq!(count_lsof_descriptors(""), 0);
    }

    #[test]
    fn a_sampler_records_the_process_it_watches_and_stops_when_told() {
        let sampler = Sampler::start(std::process::id(), Instant::now());
        for _ in 0..100 {
            if !sampler.take().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let samples = sampler.take();
        assert!(!samples.is_empty(), "the sampler took no sample");

        sampler.stop();
        std::thread::sleep(Duration::from_millis(300));
        let after_stop = sampler.take().len();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            sampler.take().len(),
            after_stop,
            "a stopped sampler is idle"
        );
    }
}
