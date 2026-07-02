pub(crate) fn rfc3339_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    rfc3339_from_unix(secs)
}

pub(crate) fn rfc3339_from_unix(secs: u64) -> String {
    let (year, month, day, hour, minute, second) = unix_to_ymdhms(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

pub(crate) fn unix_from_rfc3339(ts: &str) -> u64 {
    let b = ts.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[19] != b'Z' {
        return 0;
    }
    let field = |range: std::ops::Range<usize>| ts.get(range).and_then(|s| s.parse::<u64>().ok());
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        field(0..4),
        field(5..7),
        field(8..10),
        field(11..13),
        field(14..16),
        field(17..19),
    ) else {
        return 0;
    };
    if year == 0
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return 0;
    }
    ymdhms_to_unix(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second
}

fn ymdhms_to_unix(year: u64, month: u64, day: u64) -> u64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y / 400;
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn unix_to_ymdhms(secs: u64) -> (i32, u8, u8, u8, u8, u8) {
    let days = (secs / 86_400) as i64;
    let day_of_year_secs = secs % 86_400;
    let hour = (day_of_year_secs / 3600) as u8;
    let minute = ((day_of_year_secs % 3600) / 60) as u8;
    let second = (day_of_year_secs % 60) as u8;

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = (y + i64::from(m <= 2)) as i32;
    (year, m, d, hour, minute, second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_to_ymdhms_pinned_known_timestamps() {
        assert_eq!(unix_to_ymdhms(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(unix_to_ymdhms(1_677_591_907), (2023, 2, 28, 13, 45, 7));
        assert_eq!(unix_to_ymdhms(951_825_600), (2000, 2, 29, 12, 0, 0));
        assert_eq!(unix_to_ymdhms(1_704_067_200), (2024, 1, 1, 0, 0, 0));
    }

    #[test]
    fn rfc3339_from_unix_pins_the_epoch_and_a_modern_timestamp() {
        assert_eq!(rfc3339_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_from_unix(1_704_067_200), "2024-01-01T00:00:00Z");
    }

    #[test]
    fn unix_from_rfc3339_round_trips_every_stamp_rfc3339_from_unix_produces() {
        for secs in [
            0,
            951_825_600,
            1_677_591_907,
            1_704_067_200,
            1_780_000_000,
            4_102_444_800,
        ] {
            assert_eq!(
                unix_from_rfc3339(&rfc3339_from_unix(secs)),
                secs,
                "round trip for {secs}"
            );
        }
    }

    #[test]
    fn unix_from_rfc3339_yields_zero_for_a_malformed_stamp() {
        assert_eq!(unix_from_rfc3339(""), 0);
        assert_eq!(unix_from_rfc3339("2026-06-29 14:00:00Z"), 0, "space, not T");
        assert_eq!(unix_from_rfc3339("2026-06-29T14:00:00"), 0, "no Z");
        assert_eq!(
            unix_from_rfc3339("20x6-06-29T14:00:00Z"),
            0,
            "non-numeric year"
        );
    }

    #[test]
    fn unix_from_rfc3339_yields_zero_for_out_of_range_fields_without_panicking() {
        for ts in [
            "0000-06-15T12:00:00Z", // year 0 would underflow year - 1
            "2024-06-00T12:00:00Z", // day 0 would underflow day - 1
            "2024-00-15T12:00:00Z", // month 0
            "2024-13-15T12:00:00Z", // month 13
            "2024-06-15T24:00:00Z", // hour 24
            "2024-06-15T12:60:00Z", // minute 60
            "2024-06-15T12:00:60Z", // second 60
        ] {
            assert_eq!(unix_from_rfc3339(ts), 0, "{ts} must be rejected, not panic");
        }
    }

    #[test]
    fn rfc3339_now_format_is_iso_with_z_suffix() {
        let s = rfc3339_now();
        assert_eq!(s.len(), 20, "got: {s}");
        let bytes = s.as_bytes();
        assert_eq!(bytes[4], b'-');
        assert_eq!(bytes[7], b'-');
        assert_eq!(bytes[10], b'T');
        assert_eq!(bytes[13], b':');
        assert_eq!(bytes[16], b':');
        assert_eq!(bytes[19], b'Z');
        for (i, c) in s.chars().enumerate() {
            if [4, 7, 10, 13, 16, 19].contains(&i) {
                continue;
            }
            assert!(c.is_ascii_digit(), "non-digit at {i} in {s}");
        }
    }
}
