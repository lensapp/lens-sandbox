use std::io::Write;

use anyhow::{Context, Result};

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum Format {
    Table,
    Json,
}

#[derive(clap::Args, Debug, Clone)]
pub struct OutputArgs {
    #[arg(
        long,
        value_enum,
        default_value = "table",
        help = "Output format. json is experimental: its shape may change before v1.0."
    )]
    pub format: Format,
}

/// One row of a list verb, rendered as a human table column set or serialized as the JSON contract.
pub trait TableRow {
    const HEADERS: &'static [&'static str];
    fn cells(&self) -> Vec<String>;
}

pub fn emit<T: TableRow + serde::Serialize>(
    format: Format,
    rows: &[T],
    out: &mut dyn Write,
) -> Result<()> {
    match format {
        Format::Table => {
            let cells: Vec<Vec<String>> = rows.iter().map(TableRow::cells).collect();
            render_table(out, T::HEADERS, &cells).context("writing the table")
        }
        Format::Json => emit_object(&rows, out),
    }
}

/// Like `emit`, but a human reading an empty list gets prose where a script still gets `[]`.
pub fn emit_or_note<T: TableRow + serde::Serialize>(
    format: Format,
    rows: &[T],
    note: &str,
    out: &mut dyn Write,
) -> Result<()> {
    if rows.is_empty() && format == Format::Table {
        writeln!(out, "{note}").context("writing the empty-list note")?;
        return Ok(());
    }
    emit(format, rows, out)
}

/// One thing rather than a list: the JSON is a single object, and the table is the FIELD/VALUE summary of it that a reader can scan.
pub fn emit_fields<T: serde::Serialize>(
    format: Format,
    fields: &[(&str, String)],
    record: &T,
    out: &mut dyn Write,
) -> Result<()> {
    match format {
        Format::Table => {
            let rows: Vec<Vec<String>> = fields
                .iter()
                .map(|(field, value)| vec![(*field).to_string(), value.clone()])
                .collect();
            render_table(out, &["FIELD", "VALUE"], &rows).context("writing the table")
        }
        Format::Json => emit_object(record, out),
    }
}

pub fn emit_object<T: serde::Serialize>(value: &T, out: &mut dyn Write) -> Result<()> {
    let text = serde_json::to_string_pretty(value).context("serializing json output")?;
    writeln!(out, "{text}").context("writing json output")
}

pub fn render_table(
    out: &mut dyn Write,
    headers: &[&str],
    rows: &[Vec<String>],
) -> std::io::Result<()> {
    let columns = rows
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .max(headers.len());
    let mut widths = vec![0usize; columns];
    for (i, header) in headers.iter().enumerate() {
        widths[i] = header.len();
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }
    let header_cells: Vec<String> = headers.iter().map(|h| (*h).to_string()).collect();
    write_row(out, &header_cells, &widths)?;
    for row in rows {
        write_row(out, row, &widths)?;
    }
    Ok(())
}

fn write_row(out: &mut dyn Write, cells: &[String], widths: &[usize]) -> std::io::Result<()> {
    let last = cells.iter().rposition(|cell| !cell.is_empty()).unwrap_or(0);
    for (i, cell) in cells.iter().enumerate().take(last + 1) {
        if i == last {
            write!(out, "{cell}")?;
        } else {
            write!(
                out,
                "{:<width$}  ",
                cell,
                width = widths.get(i).copied().unwrap_or(0)
            )?;
        }
    }
    writeln!(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Sample {
        name: String,
        size_bytes: u64,
        held_by: Option<String>,
    }

    impl TableRow for Sample {
        const HEADERS: &'static [&'static str] = &["NAME", "SIZE"];

        fn cells(&self) -> Vec<String> {
            vec![self.name.clone(), format!("{} B", self.size_bytes)]
        }
    }

    fn sample() -> Sample {
        Sample {
            name: "one".into(),
            size_bytes: 1024,
            held_by: None,
        }
    }

    fn emitted(format: Format, rows: &[Sample]) -> String {
        let mut buf = Vec::new();
        emit(format, rows, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn the_table_format_renders_the_declared_headers_and_humanized_cells() {
        let text = emitted(Format::Table, &[sample()]);
        assert_eq!(text, "NAME  SIZE\none   1024 B\n");
    }

    #[test]
    fn the_json_format_emits_a_pretty_array_of_camel_case_rows() {
        let text = emitted(Format::Json, &[sample()]);
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed[0]["name"], "one");
        assert_eq!(
            parsed[0]["sizeBytes"], 1024,
            "byte counts stay raw integers for scripts: {text}"
        );
        assert!(
            text.contains('\n'),
            "the array is pretty-printed so a human can read it too: {text:?}"
        );
    }

    #[test]
    fn a_json_row_keeps_an_absent_field_as_an_explicit_null() {
        let text = emitted(Format::Json, &[sample()]);
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(
            parsed[0].get("heldBy").is_some(),
            "every key is present so `jq .heldBy` needs no guard: {text}"
        );
        assert!(parsed[0]["heldBy"].is_null(), "got: {text}");
    }

    #[test]
    fn the_json_format_emits_an_empty_array_when_there_is_nothing_to_list() {
        let text = emitted(Format::Json, &[]);
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            parsed,
            serde_json::json!([]),
            "an empty list is a valid document, not absent output: {text}"
        );
    }

    #[test]
    fn the_table_format_still_prints_the_header_when_there_is_nothing_to_list() {
        let text = emitted(Format::Table, &[]);
        assert_eq!(text, "NAME  SIZE\n");
    }

    fn noted(format: Format, rows: &[Sample]) -> String {
        let mut buf = Vec::new();
        emit_or_note(format, rows, "Nothing here.", &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn an_empty_list_reads_as_prose_for_a_human_and_as_an_empty_array_for_a_script() {
        assert_eq!(noted(Format::Table, &[]), "Nothing here.\n");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&noted(Format::Json, &[])).unwrap(),
            serde_json::json!([]),
            "prose would break a json consumer"
        );
    }

    #[test]
    fn a_non_empty_list_ignores_the_note_in_both_formats() {
        assert!(!noted(Format::Table, &[sample()]).contains("Nothing here."));
        assert!(!noted(Format::Json, &[sample()]).contains("Nothing here."));
    }

    #[test]
    fn a_single_object_is_emitted_as_one_pretty_json_document() {
        let mut buf = Vec::new();
        emit_object(&sample(), &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(parsed.is_object(), "not wrapped in an array: {text}");
        assert_eq!(parsed["sizeBytes"], 1024);
        assert!(text.ends_with('\n'), "shell-friendly trailing newline");
    }

    #[derive(Parser, Debug)]
    struct Harness {
        #[command(flatten)]
        output: OutputArgs,
    }

    #[test]
    fn the_format_flag_defaults_to_the_human_table() {
        let parsed = Harness::try_parse_from(["prog"]).unwrap();
        assert_eq!(parsed.output.format, Format::Table);
    }

    #[test]
    fn the_format_flag_accepts_json() {
        let parsed = Harness::try_parse_from(["prog", "--format", "json"]).unwrap();
        assert_eq!(parsed.output.format, Format::Json);
    }

    #[test]
    fn the_format_flag_rejects_a_streaming_format_a_list_verb_cannot_emit() {
        let err = Harness::try_parse_from(["prog", "--format", "jsonl"])
            .expect_err("jsonl belongs to event streams, not list verbs");
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
        assert!(
            err.to_string().contains("table") && err.to_string().contains("json"),
            "the error names what a list verb can emit: {err}"
        );
    }

    #[test]
    fn the_format_help_marks_json_as_experimental() {
        let help = Harness::command().render_help().to_string();
        assert!(
            help.contains("experimental"),
            "scripts must be warned the shape can change: {help}"
        );
    }

    fn rendered(headers: &[&str], rows: &[Vec<String>]) -> String {
        let mut buf = Vec::new();
        render_table(&mut buf, headers, rows).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn columns_pad_to_the_widest_cell_including_the_header() {
        let text = rendered(
            &["NAME", "VALUE"],
            &[
                vec!["a".into(), "1".into()],
                vec!["longername".into(), "2".into()],
            ],
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "NAME        VALUE");
        assert_eq!(lines[1], "a           1");
        assert_eq!(lines[2], "longername  2");
    }

    #[test]
    fn the_last_column_is_not_padded() {
        let text = rendered(&["A", "B"], &[vec!["x".into(), "y".into()]]);
        assert!(text.lines().all(|l| !l.ends_with(' ')), "got: {text:?}");
    }

    #[test]
    fn an_empty_last_cell_leaves_no_separator_dangling() {
        let text = rendered(
            &["VERDICT", "PATTERN", "DESCRIPTION"],
            &[
                vec!["allow".into(), "api.example.test".into(), String::new()],
                vec!["deny".into(), "evil.example".into(), "known bad".into()],
            ],
        );
        assert!(
            text.lines().all(|l| !l.ends_with(' ')),
            "a description-less rule is the common case: {text:?}"
        );
        assert!(
            text.contains("known bad"),
            "an empty cell must not truncate the row it sits in: {text:?}"
        );
    }

    #[test]
    fn an_empty_middle_cell_still_renders_what_follows_it() {
        let text = rendered(
            &["A", "B", "C"],
            &[vec!["a".into(), String::new(), "c".into()]],
        );
        assert_eq!(
            text,
            "A  B  C
a     c
"
        );
    }

    #[test]
    fn a_short_row_does_not_panic_on_missing_trailing_cells() {
        let text = rendered(&["A", "B", "C"], &[vec!["only-one".into()]]);
        assert!(text.contains("only-one"));
    }

    #[test]
    fn a_column_beyond_the_headers_is_still_sized_from_the_rows() {
        let text = rendered(&["A"], &[vec!["x".into(), "longer".into()]]);
        assert_eq!(text, "A\nx  longer\n");
    }
}
