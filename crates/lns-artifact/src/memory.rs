use std::fmt;

const MIB: u128 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

/// Reads a Docker-style memory size — a bare integer of MiB, or digits with a binary unit — into whole MiB.
pub fn parse_mib(spec: &str) -> Result<usize, ParseError> {
    let lower = spec.trim().to_ascii_lowercase();
    let digits_end = lower
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(lower.len());
    let (digits, unit) = lower.split_at(digits_end);
    let value: u128 = digits.parse().map_err(|_| {
        ParseError(format!(
            "invalid memory size `{spec}`: expected MiB, e.g. `512`, or `2g`"
        ))
    })?;
    let mib = match unit {
        "" | "m" | "mb" | "mi" | "mib" => value,
        "b" => value.div_ceil(MIB),
        "k" | "kb" | "ki" | "kib" => value.div_ceil(1024),
        "g" | "gb" | "gi" | "gib" => value.checked_mul(1024).ok_or_else(|| out_of_range(spec))?,
        _ => {
            return Err(ParseError(format!(
                "invalid memory size `{spec}`: unknown unit `{unit}` (use b, k, m, or g; `Mi`/`Gi` also work)"
            )));
        }
    };
    if mib == 0 {
        return Err(ParseError(format!(
            "invalid memory size `{spec}`: must be at least 1 MiB"
        )));
    }
    usize::try_from(mib).map_err(|_| out_of_range(spec))
}

fn out_of_range(spec: &str) -> ParseError {
    ParseError(format!("memory size `{spec}` is out of range"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_number_is_mib() {
        assert_eq!(parse_mib("512").unwrap(), 512);
        assert_eq!(parse_mib(" 640 ").unwrap(), 640);
    }

    #[test]
    fn every_spelling_of_a_unit_means_the_same_size() {
        for spec in ["512m", "512mb", "512mi", "512mib", "512M", "512MiB"] {
            assert_eq!(parse_mib(spec).unwrap(), 512, "spec: {spec}");
        }
        for spec in ["2g", "2gb", "2gi", "2gib", "2G", "2Gi", "2GiB"] {
            assert_eq!(parse_mib(spec).unwrap(), 2048, "spec: {spec}");
        }
        for spec in ["2048k", "2048kb", "2048ki", "2048KiB"] {
            assert_eq!(parse_mib(spec).unwrap(), 2, "spec: {spec}");
        }
    }

    #[test]
    fn a_size_below_a_whole_mib_rounds_up_so_the_guest_still_boots() {
        assert_eq!(parse_mib("1024k").unwrap(), 1);
        assert_eq!(parse_mib("1500k").unwrap(), 2);
        assert_eq!(parse_mib("1b").unwrap(), 1);
        assert_eq!(parse_mib("1048577b").unwrap(), 2);
    }

    #[test]
    fn a_zero_size_is_refused_because_no_guest_boots_on_it() {
        for spec in ["0", "0g", "0b", "0Gi"] {
            let err = parse_mib(spec).unwrap_err().to_string();
            assert!(err.contains("at least 1 MiB"), "spec {spec}: {err}");
        }
    }

    #[test]
    fn an_unknown_unit_is_named_back_to_the_author() {
        for (spec, unit) in [("12parsecs", "parsecs"), ("38gg", "gg"), ("4t", "t")] {
            let err = parse_mib(spec).unwrap_err().to_string();
            assert!(
                err.contains(&format!("unknown unit `{unit}`")),
                "spec {spec}: {err}"
            );
        }
    }

    #[test]
    fn a_unit_with_no_number_is_refused() {
        for spec in ["g", "", "Gi"] {
            let err = parse_mib(spec).unwrap_err().to_string();
            assert!(err.contains("expected MiB"), "spec {spec}: {err}");
        }
    }

    #[test]
    fn a_size_that_cannot_be_held_is_refused_rather_than_wrapped() {
        for spec in [
            format!("{}g", u64::MAX),
            format!("{}g", u128::MAX),
            format!("{}", u128::MAX),
        ] {
            let err = parse_mib(&spec).unwrap_err().to_string();
            assert!(err.contains("out of range"), "spec {spec}: {err}");
        }
    }
}
