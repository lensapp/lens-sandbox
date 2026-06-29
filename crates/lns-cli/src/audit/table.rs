use std::io::Write;

pub fn render_table(
    out: &mut dyn Write,
    headers: &[&str],
    rows: &[Vec<String>],
) -> std::io::Result<()> {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.len());
            }
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
    let last = cells.len().saturating_sub(1);
    for (i, cell) in cells.iter().enumerate() {
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
    fn a_short_row_does_not_panic_on_missing_trailing_cells() {
        let text = rendered(&["A", "B", "C"], &[vec!["only-one".into()]]);
        assert!(text.contains("only-one"));
    }
}
