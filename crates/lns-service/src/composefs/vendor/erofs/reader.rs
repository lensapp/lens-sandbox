//! EROFS image reading and parsing functionality.
//!
//! This vendored subset keeps only the alignment helper used by the writer.

/// Rounds up a value to the nearest multiple of `to`
pub fn round_up(n: usize, to: usize) -> usize {
    (n + to - 1) & !(to - 1)
}
