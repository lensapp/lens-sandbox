pub const MAX_DETACH_KEYS: usize = 5;

pub fn parse_detach_keys(s: &str) -> Result<Vec<u8>, DetachKeysError> {
    if s.is_empty() {
        return Err(DetachKeysError::Empty);
    }
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    if parts.len() > MAX_DETACH_KEYS {
        return Err(DetachKeysError::TooManyKeys(parts.len()));
    }
    let mut bytes = Vec::with_capacity(parts.len());
    for part in parts {
        bytes.push(parse_one_key(part)?);
    }
    Ok(bytes)
}

fn parse_one_key(key: &str) -> Result<u8, DetachKeysError> {
    let lower = key.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("ctrl-") {
        if rest.chars().count() != 1 {
            return Err(DetachKeysError::BadCtrlToken(key.to_string()));
        }
        let c = rest.chars().next().expect("len 1 verified above");
        return ctrl_byte(c).ok_or_else(|| DetachKeysError::BadCtrlToken(key.to_string()));
    }
    if key.chars().count() != 1 {
        return Err(DetachKeysError::BadKey(key.to_string()));
    }
    let c = key.chars().next().expect("len 1 verified above");
    if !c.is_ascii() {
        return Err(DetachKeysError::BadKey(key.to_string()));
    }
    Ok(c as u8)
}

fn ctrl_byte(c: char) -> Option<u8> {
    let upper = c.to_ascii_uppercase();
    match upper {
        'A'..='Z' => Some((upper as u8) - b'A' + 1),
        '@' => Some(0),
        '[' => Some(27),
        '\\' => Some(28),
        ']' => Some(29),
        '^' => Some(30),
        '_' => Some(31),
        _ => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DetachKeysError {
    Empty,
    TooManyKeys(usize),
    BadCtrlToken(String),
    BadKey(String),
}

impl std::fmt::Display for DetachKeysError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "--detach-keys cannot be empty"),
            Self::TooManyKeys(n) => write!(
                f,
                "--detach-keys accepts at most {MAX_DETACH_KEYS} keys, got {n}",
            ),
            Self::BadCtrlToken(s) => write!(f, "unrecognized ctrl- token in --detach-keys: {s}"),
            Self::BadKey(s) => write!(f, "unrecognized key in --detach-keys: {s}"),
        }
    }
}

impl std::error::Error for DetachKeysError {}

#[derive(Debug, PartialEq, Eq)]
pub enum FeedAction {
    Forward(u8),
    Hold,
    Flush(Vec<u8>),
    FlushAndForward(Vec<u8>, u8),
    Trigger,
}

pub struct DetachChordDetector {
    chord: Vec<u8>,
    matched: usize,
}

impl DetachChordDetector {
    pub fn new(chord: Vec<u8>) -> Self {
        assert!(!chord.is_empty(), "chord must not be empty");
        Self { chord, matched: 0 }
    }

    pub fn feed(&mut self, byte: u8) -> FeedAction {
        if byte == self.chord[self.matched] {
            self.matched += 1;
            if self.matched == self.chord.len() {
                self.matched = 0;
                return FeedAction::Trigger;
            }
            return FeedAction::Hold;
        }
        if self.matched == 0 {
            return FeedAction::Forward(byte);
        }
        let held = self.chord[..self.matched].to_vec();
        self.matched = 0;
        if byte == self.chord[0] {
            self.matched = 1;
            FeedAction::Flush(held)
        } else {
            FeedAction::FlushAndForward(held, byte)
        }
    }

    pub fn drain_for_eof(&mut self) -> Vec<u8> {
        let held = self.chord[..self.matched].to_vec();
        self.matched = 0;
        held
    }
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    #[test]
    fn parses_default_docker_chord() {
        assert_eq!(
            parse_detach_keys("ctrl-p,ctrl-q").unwrap(),
            vec![0x10, 0x11]
        );
    }

    #[test]
    fn parses_case_insensitive_ctrl_letters() {
        assert_eq!(
            parse_detach_keys("CTRL-P,Ctrl-Q").unwrap(),
            vec![0x10, 0x11]
        );
    }

    #[test]
    fn parses_single_char_keys() {
        assert_eq!(parse_detach_keys("q").unwrap(), vec![0x71]);
        assert_eq!(parse_detach_keys("a,b,c").unwrap(), vec![0x61, 0x62, 0x63]);
    }

    #[test]
    fn parses_mixed_ctrl_and_plain() {
        assert_eq!(parse_detach_keys("ctrl-a,d").unwrap(), vec![0x01, 0x64]);
    }

    #[test]
    fn parses_ctrl_symbols() {
        assert_eq!(parse_detach_keys("ctrl-@").unwrap(), vec![0x00]);
        assert_eq!(parse_detach_keys("ctrl-[").unwrap(), vec![0x1B]);
        assert_eq!(parse_detach_keys("ctrl-\\").unwrap(), vec![0x1C]);
        assert_eq!(parse_detach_keys("ctrl-]").unwrap(), vec![0x1D]);
        assert_eq!(parse_detach_keys("ctrl-^").unwrap(), vec![0x1E]);
        assert_eq!(parse_detach_keys("ctrl-_").unwrap(), vec![0x1F]);
    }

    #[test]
    fn detach_keys_error_display_renders_all_variants() {
        assert_eq!(
            DetachKeysError::Empty.to_string(),
            "--detach-keys cannot be empty",
        );
        assert_eq!(
            DetachKeysError::TooManyKeys(6).to_string(),
            "--detach-keys accepts at most 5 keys, got 6",
        );
        assert_eq!(
            DetachKeysError::BadCtrlToken("ctrl-pp".to_string()).to_string(),
            "unrecognized ctrl- token in --detach-keys: ctrl-pp",
        );
        assert_eq!(
            DetachKeysError::BadKey("hello".to_string()).to_string(),
            "unrecognized key in --detach-keys: hello",
        );
    }

    #[test]
    fn trims_whitespace_around_tokens() {
        assert_eq!(
            parse_detach_keys(" ctrl-p , ctrl-q ").unwrap(),
            vec![0x10, 0x11]
        );
    }

    #[test]
    fn rejects_empty_input() {
        assert_eq!(parse_detach_keys(""), Err(DetachKeysError::Empty));
    }

    #[test]
    fn rejects_more_than_max_keys() {
        assert_eq!(
            parse_detach_keys("a,b,c,d,e,f"),
            Err(DetachKeysError::TooManyKeys(6)),
        );
    }

    #[test]
    fn rejects_unknown_ctrl_token() {
        assert!(matches!(
            parse_detach_keys("ctrl-é"),
            Err(DetachKeysError::BadCtrlToken(_)),
        ));
        assert!(matches!(
            parse_detach_keys("ctrl-pp"),
            Err(DetachKeysError::BadCtrlToken(_)),
        ));
    }

    #[test]
    fn rejects_multi_char_plain_key() {
        assert!(matches!(
            parse_detach_keys("hello"),
            Err(DetachKeysError::BadKey(_)),
        ));
    }

    #[test]
    fn rejects_non_ascii_plain_key() {
        assert!(matches!(
            parse_detach_keys("ñ"),
            Err(DetachKeysError::BadKey(_)),
        ));
    }
}

#[cfg(test)]
mod chord_tests {
    use super::*;

    fn pq() -> DetachChordDetector {
        DetachChordDetector::new(vec![0x10, 0x11])
    }

    #[test]
    fn chord_prefix_then_trigger_fires_trigger() {
        let mut d = pq();
        assert_eq!(d.feed(0x10), FeedAction::Hold);
        assert_eq!(d.feed(0x11), FeedAction::Trigger);
    }

    #[test]
    fn chord_prefix_then_other_flushes_held_and_forwards() {
        let mut d = pq();
        assert_eq!(d.feed(0x10), FeedAction::Hold);
        assert_eq!(d.feed(b'x'), FeedAction::FlushAndForward(vec![0x10], b'x'));
        assert_eq!(d.feed(b'y'), FeedAction::Forward(b'y'));
    }

    #[test]
    fn chord_prefix_then_prefix_flushes_and_re_holds() {
        let mut d = pq();
        assert_eq!(d.feed(0x10), FeedAction::Hold);
        assert_eq!(d.feed(0x10), FeedAction::Flush(vec![0x10]));
        assert_eq!(d.feed(0x11), FeedAction::Trigger);
    }

    #[test]
    fn idle_forwards_non_prefix_bytes_including_ctrl_c() {
        let mut d = pq();
        assert_eq!(d.feed(b'a'), FeedAction::Forward(b'a'));
        assert_eq!(d.feed(b'\n'), FeedAction::Forward(b'\n'));
        assert_eq!(d.feed(0x03), FeedAction::Forward(0x03));
        assert_eq!(d.feed(0x04), FeedAction::Forward(0x04));
    }

    #[test]
    fn re_arms_after_trigger() {
        let mut d = pq();
        assert_eq!(d.feed(0x10), FeedAction::Hold);
        assert_eq!(d.feed(0x11), FeedAction::Trigger);
        assert_eq!(d.feed(0x10), FeedAction::Hold);
        assert_eq!(d.feed(0x11), FeedAction::Trigger);
    }

    #[test]
    fn drain_for_eof_returns_held_prefix_and_resets() {
        let mut d = pq();
        assert_eq!(d.feed(0x10), FeedAction::Hold);
        assert_eq!(d.drain_for_eof(), vec![0x10]);
        assert_eq!(d.feed(b'a'), FeedAction::Forward(b'a'));
    }

    #[test]
    fn drain_for_eof_idle_returns_empty() {
        let mut d = pq();
        assert_eq!(d.drain_for_eof(), Vec::<u8>::new());
    }

    #[test]
    fn single_byte_chord_triggers_immediately() {
        let mut d = DetachChordDetector::new(vec![b'q']);
        assert_eq!(d.feed(b'q'), FeedAction::Trigger);
        assert_eq!(d.feed(b'a'), FeedAction::Forward(b'a'));
    }

    #[test]
    fn three_byte_chord_full_sequence() {
        let mut d = DetachChordDetector::new(vec![b'a', b'b', b'c']);
        assert_eq!(d.feed(b'a'), FeedAction::Hold);
        assert_eq!(d.feed(b'b'), FeedAction::Hold);
        assert_eq!(d.feed(b'c'), FeedAction::Trigger);
    }

    #[test]
    fn three_byte_chord_break_mid_sequence() {
        let mut d = DetachChordDetector::new(vec![b'a', b'b', b'c']);
        assert_eq!(d.feed(b'a'), FeedAction::Hold);
        assert_eq!(d.feed(b'b'), FeedAction::Hold);
        assert_eq!(
            d.feed(b'd'),
            FeedAction::FlushAndForward(vec![b'a', b'b'], b'd'),
        );
        assert_eq!(d.feed(b'a'), FeedAction::Hold);
    }
}
