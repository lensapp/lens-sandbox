use lns_audit::TimelineRow;

pub const KINDS: [&str; 8] = [
    "launch",
    "egress",
    "env",
    "volume",
    "bind",
    "approval",
    "connection",
    "credential",
];

#[derive(Debug, Default, Clone)]
pub struct Filters {
    pub kinds: Vec<String>,
    pub sandbox: String,
    pub search: String,
}

impl Filters {
    pub fn is_active(&self) -> bool {
        !self.kinds.is_empty() || !self.sandbox.trim().is_empty() || !self.search.trim().is_empty()
    }
}

pub fn row_matches(row: &TimelineRow, filters: &Filters) -> bool {
    if !filters.kinds.is_empty() && !filters.kinds.iter().any(|k| k == &row.kind) {
        return false;
    }
    let sandbox = filters.sandbox.trim();
    if !sandbox.is_empty() && !row.run.starts_with(sandbox) {
        return false;
    }
    let needle = filters.search.trim().to_lowercase();
    if !needle.is_empty() && !row_contains(row, &needle) {
        return false;
    }
    true
}

fn row_contains(row: &TimelineRow, needle: &str) -> bool {
    row.when.to_lowercase().contains(needle)
        || row.run.to_lowercase().contains(needle)
        || row.kind.to_lowercase().contains(needle)
        || row.detail.to_lowercase().contains(needle)
}

pub fn visible_indices(rows: &[TimelineRow], filters: &Filters) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| row_matches(row, filters))
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn row(run: &str, kind: &str, detail: &str) -> TimelineRow {
        TimelineRow {
            ts: "2026-06-29T14:00:00Z".into(),
            when: "2026-06-29 14:00:00".into(),
            run: run.into(),
            kind: kind.into(),
            detail: detail.into(),
            raw: Value::Null,
            integration: None,
        }
    }

    fn sample() -> Vec<TimelineRow> {
        vec![
            row("1a2b3c4d", "connection", "connect some-oauth"),
            row("1a2b3c4d", "egress", "GET api.example.test:443"),
            row("9e8d7c6b", "credential", "use some-provider"),
        ]
    }

    #[test]
    fn no_filters_shows_everything() {
        let filters = Filters::default();
        assert!(!filters.is_active());
        assert_eq!(visible_indices(&sample(), &filters), [0, 1, 2]);
    }

    #[test]
    fn the_kind_filter_keeps_only_that_kind() {
        let filters = Filters {
            kinds: vec!["egress".into()],
            ..Default::default()
        };
        assert!(filters.is_active());
        assert_eq!(visible_indices(&sample(), &filters), [1]);
    }

    #[test]
    fn multiple_selected_kinds_keep_any_matching_kind() {
        let filters = Filters {
            kinds: vec!["egress".into(), "credential".into()],
            ..Default::default()
        };
        assert_eq!(visible_indices(&sample(), &filters), [1, 2]);
    }

    #[test]
    fn the_sandbox_filter_prefix_matches_the_run_id() {
        let filters = Filters {
            sandbox: "1a2b".into(),
            ..Default::default()
        };
        assert!(filters.is_active());
        assert_eq!(visible_indices(&sample(), &filters), [0, 1]);
    }

    #[test]
    fn a_blank_sandbox_is_not_a_filter() {
        let filters = Filters {
            sandbox: "   ".into(),
            ..Default::default()
        };
        assert!(!filters.is_active());
        assert_eq!(visible_indices(&sample(), &filters).len(), 3);
    }

    #[test]
    fn search_is_a_case_insensitive_substring_over_every_column() {
        let filters = Filters {
            search: "SOME-PROVIDER".into(),
            ..Default::default()
        };
        assert_eq!(visible_indices(&sample(), &filters), [2]);

        let by_kind = Filters {
            search: "conn".into(),
            ..Default::default()
        };
        assert_eq!(visible_indices(&sample(), &by_kind), [0]);

        let by_when = Filters {
            search: "2026-06-29".into(),
            ..Default::default()
        };
        assert_eq!(visible_indices(&sample(), &by_when).len(), 3);
    }

    #[test]
    fn filters_compose_with_and_semantics() {
        let filters = Filters {
            kinds: vec!["egress".into()],
            sandbox: "1a2b".into(),
            search: "api.example".into(),
        };
        assert_eq!(visible_indices(&sample(), &filters), [1]);

        let contradictory = Filters {
            kinds: vec!["egress".into()],
            search: "some-oauth".into(),
            ..Default::default()
        };
        assert!(visible_indices(&sample(), &contradictory).is_empty());
    }
}
