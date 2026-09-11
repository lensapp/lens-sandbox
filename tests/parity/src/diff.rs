use crate::result::{Measure, RunResult};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq)]
pub enum Difference {
    MissingCase {
        case: String,
        present_in: String,
    },
    Status {
        case: String,
        left: String,
        right: String,
    },
    Value {
        case: String,
        key: String,
        left: Option<String>,
        right: Option<String>,
    },
}

impl Difference {
    #[cfg(test)]
    pub fn case(&self) -> &str {
        match self {
            Difference::MissingCase { case, .. }
            | Difference::Status { case, .. }
            | Difference::Value { case, .. } => case,
        }
    }
}

impl std::fmt::Display for Difference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Difference::MissingCase { case, present_in } => {
                write!(f, "{case}: only {present_in} ran it")
            }
            Difference::Status { case, left, right } => {
                write!(f, "{case}: status {left} -> {right}")
            }
            Difference::Value {
                case,
                key,
                left,
                right,
            } => {
                let left = left.as_deref().unwrap_or("(absent)");
                let right = right.as_deref().unwrap_or("(absent)");
                write!(f, "{case}: {key} {left} -> {right}")
            }
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct DiffReport {
    pub unexpected: Vec<Difference>,
    pub expected: Vec<Difference>,
}

impl DiffReport {
    pub fn agrees(&self) -> bool {
        self.unexpected.is_empty()
    }
}

pub fn diff(left: &RunResult, right: &RunResult) -> DiffReport {
    let names: BTreeSet<&str> = left
        .cases
        .iter()
        .chain(right.cases.iter())
        .map(|c| c.name.as_str())
        .collect();

    let mut report = DiffReport::default();
    for name in names {
        let expected = left.expects_difference(name) || right.expects_difference(name);
        for difference in case_differences(left, right, name) {
            if expected {
                report.expected.push(difference);
            } else {
                report.unexpected.push(difference);
            }
        }
    }
    report
}

fn case_differences(left: &RunResult, right: &RunResult, name: &str) -> Vec<Difference> {
    let (Some(a), Some(b)) = (left.case(name), right.case(name)) else {
        let present_in = if left.case(name).is_some() {
            left.backend.name.clone()
        } else {
            right.backend.name.clone()
        };
        return vec![Difference::MissingCase {
            case: name.to_string(),
            present_in,
        }];
    };

    let mut differences = Vec::new();
    if a.status != b.status {
        differences.push(Difference::Status {
            case: name.to_string(),
            left: a.status.as_str().to_string(),
            right: b.status.as_str().to_string(),
        });
    }

    let keys: BTreeSet<&str> = a
        .measures
        .keys()
        .chain(b.measures.keys())
        .map(String::as_str)
        .collect();
    for key in keys {
        let left_value = a.measures.get(key);
        let right_value = b.measures.get(key);
        if !comparable(key) || measures_agree(left_value, right_value) {
            continue;
        }
        differences.push(Difference::Value {
            case: name.to_string(),
            key: key.to_string(),
            left: left_value.map(Measure::to_string),
            right: right_value.map(Measure::to_string),
        });
    }
    differences
}

fn comparable(key: &str) -> bool {
    !(key.starts_with("duration") || key.starts_with("throughput") || key.ends_with("_ms"))
}

fn measures_agree(left: Option<&Measure>, right: Option<&Measure>) -> bool {
    match (left, right) {
        (Some(Measure::Float(a)), Some(Measure::Float(b))) => (a - b).abs() < f64::EPSILON,
        (a, b) => a == b,
    }
}

pub fn render(report: &DiffReport, left: &RunResult, right: &RunResult) -> String {
    let mut out = format!("{} vs {}\n", describe(left), describe(right));
    if report.unexpected.is_empty() {
        out.push_str("no unexpected difference\n");
    } else {
        out.push_str("unexpected differences:\n");
        for difference in &report.unexpected {
            out.push_str(&format!("  {difference}\n"));
        }
    }
    if !report.expected.is_empty() {
        out.push_str("expected differences (named in the backend's TOML):\n");
        for difference in &report.expected {
            out.push_str(&format!("  {difference}\n"));
        }
    }
    out
}

fn describe(result: &RunResult) -> String {
    format!("{} ({})", result.backend.name, result.lns_version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::{BackendRecord, CaseResult, HostFacts, RunResult, SCHEMA_VERSION, Status};

    fn result(name: &str, cases: Vec<CaseResult>, expected: &[&str]) -> RunResult {
        RunResult {
            schema_version: SCHEMA_VERSION,
            harness_revision: "abc1234".into(),
            backend: BackendRecord {
                name: name.into(),
                env: Default::default(),
                expectations: Default::default(),
                expected_differences: expected.iter().map(|s| s.to_string()).collect(),
            },
            binaries: vec![],
            lns_version: "lns 0.25.0".into(),
            service_pid: None,
            images: vec![],
            host: HostFacts::default(),
            started_unix_ms: 0,
            finished_unix_ms: 0,
            cases,
            samples: vec![],
        }
    }

    fn case(name: &str, status: Status, measures: &[(&str, Measure)]) -> CaseResult {
        let mut case = CaseResult::new(name);
        case.status = status;
        for (key, value) in measures {
            case.measures.insert(key.to_string(), value.clone());
        }
        case
    }

    #[test]
    fn two_runs_that_measured_the_same_agree() {
        let cases = vec![case(
            "upload-100m",
            Status::Pass,
            &[("bytes", Measure::Int(104_857_600))],
        )];
        let report = diff(
            &result("netstack", cases.clone(), &[]),
            &result("vmnet", cases, &[]),
        );

        assert!(report.agrees());
        assert!(report.expected.is_empty());
    }

    #[test]
    fn a_status_that_changed_is_an_unexpected_difference() {
        let left = result("netstack", vec![case("udp-echo", Status::Fail, &[])], &[]);
        let right = result("vmnet", vec![case("udp-echo", Status::Pass, &[])], &[]);
        let report = diff(&left, &right);

        assert!(!report.agrees());
        assert_eq!(
            report.unexpected[0],
            Difference::Status {
                case: "udp-echo".into(),
                left: "fail".into(),
                right: "pass".into(),
            }
        );
        assert_eq!(
            report.unexpected[0].to_string(),
            "udp-echo: status fail -> pass"
        );
    }

    #[test]
    fn a_measured_value_that_changed_is_an_unexpected_difference() {
        let left = result(
            "netstack",
            vec![case(
                "upload-100m",
                Status::Pass,
                &[("sink_sha256", Measure::Text("aa".into()))],
            )],
            &[],
        );
        let right = result(
            "vmnet",
            vec![case(
                "upload-100m",
                Status::Pass,
                &[("sink_sha256", Measure::Text("bb".into()))],
            )],
            &[],
        );
        let report = diff(&left, &right);

        assert_eq!(report.unexpected.len(), 1);
        assert_eq!(
            report.unexpected[0].to_string(),
            "upload-100m: sink_sha256 aa -> bb"
        );
    }

    #[test]
    fn a_measurement_only_one_run_took_is_a_difference_too() {
        let left = result(
            "netstack",
            vec![case("lease-and-resolver", Status::Pass, &[])],
            &[],
        );
        let right = result(
            "vmnet",
            vec![case(
                "lease-and-resolver",
                Status::Pass,
                &[("nameserver", Measure::Text("192.168.64.1".into()))],
            )],
            &[],
        );
        let report = diff(&left, &right);

        assert_eq!(
            report.unexpected[0],
            Difference::Value {
                case: "lease-and-resolver".into(),
                key: "nameserver".into(),
                left: None,
                right: Some("192.168.64.1".into()),
            }
        );
        assert!(report.unexpected[0].to_string().contains("(absent)"));
    }

    #[test]
    fn a_difference_the_backend_expects_does_not_fail_the_diff() {
        let left = result(
            "netstack",
            vec![case(
                "lease-and-resolver",
                Status::Pass,
                &[("address", Measure::Text("192.168.127.2".into()))],
            )],
            &["lease-and-resolver"],
        );
        let right = result(
            "vmnet",
            vec![case(
                "lease-and-resolver",
                Status::Pass,
                &[("address", Measure::Text("192.168.64.3".into()))],
            )],
            &[],
        );
        let report = diff(&left, &right);

        assert!(
            report.agrees(),
            "an expected difference keeps the diff green"
        );
        assert_eq!(report.expected.len(), 1);
        assert_eq!(report.expected[0].case(), "lease-and-resolver");
    }

    #[test]
    fn a_case_only_one_run_took_is_reported_and_names_the_run_that_took_it() {
        let left = result(
            "netstack",
            vec![case("kill-mid-transfer", Status::Pass, &[])],
            &[],
        );
        let right = result("vmnet", vec![], &[]);
        let report = diff(&left, &right);

        assert_eq!(
            report.unexpected[0],
            Difference::MissingCase {
                case: "kill-mid-transfer".into(),
                present_in: "netstack".into(),
            }
        );
        assert_eq!(
            report.unexpected[0].to_string(),
            "kill-mid-transfer: only netstack ran it"
        );

        let swapped = diff(&result("vmnet", vec![], &[]), &left);
        assert_eq!(
            swapped.unexpected[0],
            Difference::MissingCase {
                case: "kill-mid-transfer".into(),
                present_in: "netstack".into(),
            }
        );
    }

    #[test]
    fn timings_and_throughput_are_recorded_but_never_diffed() {
        let left = result(
            "netstack",
            vec![case(
                "download-100m",
                Status::Pass,
                &[
                    ("throughput_mib_s", Measure::Float(91.0)),
                    ("connect_ms", Measure::Int(12)),
                    ("duration_setup", Measure::Int(3)),
                ],
            )],
            &[],
        );
        let right = result(
            "vmnet",
            vec![case(
                "download-100m",
                Status::Pass,
                &[
                    ("throughput_mib_s", Measure::Float(43.0)),
                    ("connect_ms", Measure::Int(900)),
                    ("duration_setup", Measure::Int(30)),
                ],
            )],
            &[],
        );

        assert!(diff(&left, &right).agrees());
    }

    #[test]
    fn two_floats_that_measured_the_same_agree() {
        let left = result(
            "netstack",
            vec![case("x", Status::Pass, &[("ratio", Measure::Float(1.5))])],
            &[],
        );
        let right = result(
            "vmnet",
            vec![case("x", Status::Pass, &[("ratio", Measure::Float(1.5))])],
            &[],
        );
        assert!(diff(&left, &right).agrees());

        let other = result(
            "vmnet",
            vec![case("x", Status::Pass, &[("ratio", Measure::Float(2.5))])],
            &[],
        );
        assert!(!diff(&left, &other).agrees());
    }

    #[test]
    fn the_rendered_diff_names_both_runs_and_every_difference() {
        let left = result("netstack", vec![case("udp-echo", Status::Fail, &[])], &[]);
        let right = result("vmnet", vec![case("udp-echo", Status::Pass, &[])], &[]);
        let report = diff(&left, &right);
        let text = render(&report, &left, &right);

        assert!(
            text.contains("netstack (lns 0.25.0) vs vmnet (lns 0.25.0)"),
            "{text}"
        );
        assert!(text.contains("udp-echo: status fail -> pass"), "{text}");
    }

    #[test]
    fn a_clean_diff_says_so_and_still_lists_the_expected_differences() {
        let left = result(
            "netstack",
            vec![case(
                "lease-and-resolver",
                Status::Pass,
                &[("address", Measure::Text("a".into()))],
            )],
            &["lease-and-resolver"],
        );
        let right = result(
            "vmnet",
            vec![case(
                "lease-and-resolver",
                Status::Pass,
                &[("address", Measure::Text("b".into()))],
            )],
            &[],
        );
        let text = render(&diff(&left, &right), &left, &right);

        assert!(text.contains("no unexpected difference"), "{text}");
        assert!(text.contains("expected differences"), "{text}");
    }
}
