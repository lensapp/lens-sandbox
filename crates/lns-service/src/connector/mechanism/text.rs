//! Words a connector supplies from code nobody can read, rendered on lns's own lines (`docs/sandbox-spec.md` §3.2.6).
//!
//! An ask's message and labels are one place they arrive; the ledger line naming
//! what a component reached or ran is another, and `lns audit` prints that. One
//! rule covers both, because text that could redraw the line lns drew it on could
//! forge whichever account surrounds it.

/// Nothing that could move a cursor, clear a screen, start a line of its own, or reorder or hide the words beside it survives — nor [`MARK`]'s ellipsis, which is lns's alone to write.
pub(crate) fn scrubbed(words: &str) -> String {
    words
        .chars()
        .map(|c| if lnss_own(c) { ' ' } else { c })
        .collect()
}

/// The ellipsis [`MARK`] is built from. A component that could write one could write a whole name that reads as one lns shortened, which is the forgery [`cut`] exists to prevent, so it is reserved rather than rendered.
const RESERVED: char = '…';

/// What lns adds where it shortened a component's words, so a cut name cannot read as a whole one (§3.2.6).
pub(crate) const MARK: &str = " …(cut)";

/// [`scrubbed`], and where that is longer than the ceiling, cut and marked. For text with no connect left to fail — a runtime's own error, a ledger entry — where refusing would lose the account entirely. A ceiling too narrow for the mark yields the mark alone: what lns could not fit is still not passed off as whole.
pub(crate) fn cut(words: &str, ceiling: usize) -> String {
    let mut said = scrubbed(words);
    if said.len() <= ceiling {
        return said;
    }
    // `login.example.com.attacker.example` shortened to `login.example.com` would be a host the component never named, in the one record that answers for it.
    let mut at = ceiling.saturating_sub(MARK.len()).min(said.len());
    while !said.is_char_boundary(at) {
        at -= 1;
    }
    said.truncate(at);
    said.push_str(MARK);
    said
}

/// Whether one character is lns's to write and not the connector's: everything Unicode assigns to Cc, Cf, Zl or Zp, each of which is a way to redraw the line lns surrounds this text with, and the one mark lns reserves for a cut.
fn lnss_own(c: char) -> bool {
    c == RESERVED
        || c.is_control()
        || DRAWS_ELSEWHERE
            .iter()
            .any(|(first, last)| (*first..=*last).contains(&c))
}

/// Every Cf, Zl and Zp range in Unicode 17.0 — the separators, the bidi marks, overrides and isolates that reverse what follows them, and the zero-width, annotation and tag characters that hide it — ZWNJ and ZWJ included, which does mangle a Persian word or an emoji family, because either one can hide a word boundary in the middle of a disclosure; Cc is `char::is_control` and is not repeated here.
const DRAWS_ELSEWHERE: &[(char, char)] = &[
    ('\u{00ad}', '\u{00ad}'),
    ('\u{0600}', '\u{0605}'),
    ('\u{061c}', '\u{061c}'),
    ('\u{06dd}', '\u{06dd}'),
    ('\u{070f}', '\u{070f}'),
    ('\u{0890}', '\u{0891}'),
    ('\u{08e2}', '\u{08e2}'),
    ('\u{180e}', '\u{180e}'),
    ('\u{200b}', '\u{200f}'),
    ('\u{2028}', '\u{202e}'),
    ('\u{2060}', '\u{2064}'),
    ('\u{2066}', '\u{206f}'),
    ('\u{feff}', '\u{feff}'),
    ('\u{fff9}', '\u{fffb}'),
    ('\u{110bd}', '\u{110bd}'),
    ('\u{110cd}', '\u{110cd}'),
    ('\u{13430}', '\u{1343f}'),
    ('\u{1bca0}', '\u{1bca3}'),
    ('\u{1d173}', '\u{1d17a}'),
    ('\u{e0001}', '\u{e0001}'),
    ('\u{e0020}', '\u{e007f}'),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_lands_on_a_character_boundary_rather_than_inside_one() {
        // Truncating inside a multi-byte character would panic, and a component chooses where its characters fall.
        let cut = cut(&"é".repeat(40), 20);
        assert!(cut.ends_with(MARK), "{cut}");
        assert!(cut.len() <= 20, "{} bytes", cut.len());
    }

    #[test]
    fn a_shortened_name_says_it_was_shortened_rather_than_reading_as_a_whole_one() {
        // A prefix of a hostile host is a plausible host, and the ledger is the only thing that answers for what the component reached.
        let cut = cut("login.example.com.attacker.example", 30);

        assert!(cut.starts_with("login.example.com"), "{cut}");
        assert!(
            cut.ends_with(MARK),
            "without this the entry reads as a host the component never named: {cut}"
        );
    }

    #[test]
    fn a_ceiling_narrower_than_the_marker_still_says_the_words_were_cut() {
        let cut = cut("some-very-long-name", 2);
        assert_eq!(
            cut, MARK,
            "what lns could not fit is still not passed off as whole"
        );
    }

    #[test]
    fn text_within_the_ceiling_is_returned_whole() {
        assert_eq!(cut("claude", 256), "claude");
    }

    #[test]
    fn a_component_cannot_write_the_mark_that_says_lns_shortened_something() {
        // A whole name carrying the mark would read as one lns cut, which is the forgery the mark exists to prevent — the same harm as a cut that reads as whole.
        let said = cut("login.example.com …(cut)", 256);

        assert_eq!(said, "login.example.com  (cut)");
        assert!(!said.ends_with(MARK), "{said}");
    }
}
