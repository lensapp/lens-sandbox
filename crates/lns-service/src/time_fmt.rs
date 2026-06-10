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
