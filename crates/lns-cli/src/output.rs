use std::borrow::Cow;
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

/// Every list verb states its own empty case, so a human reading nothing gets prose where a script still gets `[]`.
pub fn emit<T: TableRow + serde::Serialize>(
    format: Format,
    rows: &[T],
    empty_note: &str,
    out: &mut dyn Write,
) -> Result<()> {
    match format {
        Format::Table if rows.is_empty() => {
            writeln!(out, "{empty_note}").context("writing the empty-list note")
        }
        Format::Table => {
            let cells: Vec<Vec<String>> = rows.iter().map(TableRow::cells).collect();
            render_table(out, T::HEADERS, &cells).context("writing the table")
        }
        Format::Json => emit_object(&rows, out),
    }
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
        widths[i] = display_width(header);
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(display_width(&display_cell(cell)));
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
        let cell = display_cell(cell);
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

/// A cell as a terminal may safely receive it: every control character escaped, because a column
/// carries whatever the record underneath it holds and some of those legitimately contain one —
/// `lns-policy` separates a composed mixin from its workload with a NUL, precisely because no path
/// or OCI reference can contain one. Written raw, a single such cell makes `grep` call the whole
/// stream binary and refuse to match it, and an ESC in a cell is a sequence the terminal obeys
/// rather than shows. Escaping here rather than at each call site means a table added later is safe
/// without knowing to ask.
///
/// Only control characters are escaped, and uniformly as `\xNN`: a backslash a path really contains
/// is left as itself rather than doubled, since the point is a stream that survives a pipe, not a
/// round-trippable encoding. `--format json` needs none of this — serde escapes controls already.
fn display_cell(cell: &str) -> Cow<'_, str> {
    if !cell.chars().any(char::is_control) {
        return Cow::Borrowed(cell);
    }
    let mut escaped = String::with_capacity(cell.len());
    for ch in cell.chars() {
        if ch.is_control() {
            escaped.push_str(&format!("\\x{:02x}", ch as u32));
        } else {
            escaped.push(ch);
        }
    }
    Cow::Owned(escaped)
}

/// Column widths count `char`s because the padding below is applied in `char`s: measuring the same
/// cell in bytes pushed every row holding a non-ASCII name out of line. A `char` is still not a
/// terminal column for wide or combining scripts, which would need a unicode-width table to get
/// right; this keeps the two halves of the calculation agreeing on one unit.
fn display_width(cell: &str) -> usize {
    cell.chars().count()
}

/// The size column every table renders through, in the binary units the JSON carries raw.
pub fn format_bytes(n: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if n >= GIB {
        format!("{:.1} GiB", n as f64 / GIB as f64)
    } else if n >= MIB {
        format!("{:.1} MiB", n as f64 / MIB as f64)
    } else if n >= KIB {
        format!("{:.1} KiB", n as f64 / KIB as f64)
    } else {
        format!("{n} B")
    }
}

/// The prune candidates ride with the question on stderr, so a piped stdout still shows what's on the line.
pub async fn announce_prune_candidates<E: tokio::io::AsyncWriteExt + Unpin>(
    names: &[String],
    err: &mut E,
) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    let mut block = String::from("Would remove:\n");
    for name in names {
        block.push_str("  ");
        block.push_str(name);
        block.push('\n');
    }
    err.write_all(block.as_bytes()).await?;
    err.flush().await?;
    Ok(())
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
        emit(format, rows, "Nothing here.", &mut buf).unwrap();
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
    fn the_table_format_says_there_is_nothing_to_list_instead_of_printing_a_bare_header() {
        assert_eq!(emitted(Format::Table, &[]), "Nothing here.\n");
    }

    #[test]
    fn a_non_empty_list_ignores_the_note_in_both_formats() {
        assert!(!emitted(Format::Table, &[sample()]).contains("Nothing here."));
        assert!(!emitted(Format::Json, &[sample()]).contains("Nothing here."));
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

    /// A workload key composed with a mixin carries a NUL, and one of those on stdout is what makes
    /// `grep` answer "binary file matches" instead of the line the user asked for.
    #[test]
    fn no_control_character_reaches_the_stream_from_a_cell() {
        let text = rendered(
            &["WORKLOAD", "VERDICT"],
            &[vec!["def:/w\u{0}mixin".into(), "allow".into()]],
        );
        assert!(
            !text.chars().any(|c| c.is_control() && c != '\n'),
            "a table a pipe can carry has no control bytes in it: {text:?}"
        );
        assert!(
            text.contains("def:/w\\x00mixin"),
            "the byte is shown, not dropped: {text:?}"
        );
    }

    #[test]
    fn an_escape_sequence_in_a_cell_is_shown_rather_than_obeyed() {
        let text = rendered(&["NAME"], &[vec!["\u{1b}[31mred".into()]]);
        assert_eq!(
            text, "NAME\n\\x1b[31mred\n",
            "a record must not be able to colour or reposition the terminal it prints on"
        );
    }

    #[test]
    fn an_escaped_cell_is_measured_as_it_prints_not_as_it_was_stored() {
        let text = rendered(
            &["NAME", "N"],
            &[
                vec!["a\u{0}b".into(), "1".into()],
                vec!["wwwww".into(), "2".into()],
            ],
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[1], "a\\x00b  1",
            "the escape widened the cell past the column: {text:?}"
        );
        assert_eq!(lines[2], "wwwww   2");
    }

    #[test]
    fn columns_line_up_when_a_cell_is_not_ascii() {
        let text = rendered(
            &["NAME", "N"],
            &[
                vec!["äöü".into(), "1".into()],
                vec!["abc".into(), "2".into()],
            ],
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[1].chars().count(),
            lines[2].chars().count(),
            "a multi-byte name is three columns wide, not six: {text:?}"
        );
    }

    #[test]
    fn a_clean_cell_is_passed_through_untouched() {
        assert!(
            matches!(display_cell("ordinary/path:1.0"), Cow::Borrowed(_)),
            "the common row must not pay for the rare one"
        );
    }
}
