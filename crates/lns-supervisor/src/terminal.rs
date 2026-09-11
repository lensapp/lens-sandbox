#[cfg(target_os = "linux")]
pub mod real;

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Pipes,
    Terminal,
}

impl Mode {
    pub fn from_stdin(is_terminal: bool) -> Self {
        if is_terminal {
            Self::Terminal
        } else {
            Self::Pipes
        }
    }
}

pub fn raw_settings(mut original: libc::termios) -> libc::termios {
    // SAFETY: cfmakeraw only transforms the initialized, exclusively borrowed termios value.
    unsafe { libc::cfmakeraw(&mut original) };
    original
}

pub fn dimensions(columns: u16, rows: u16) -> (u32, u32) {
    if columns == 0 || rows == 0 {
        (80, 24)
    } else {
        (u32::from(columns), u32::from(rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_terminal_reaches_the_workload() {
        assert_eq!(Mode::from_stdin(true), Mode::Terminal);
    }

    #[test]
    fn redirected_stdin_remains_a_pipe() {
        assert_eq!(Mode::from_stdin(false), Mode::Pipes);
    }

    #[test]
    fn broker_line_discipline_does_not_consume_or_echo_workload_keys() {
        // SAFETY: all-zero termios is valid data for this in-memory transformation.
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        original.c_iflag = libc::ICRNL | libc::IXON;
        original.c_oflag = libc::OPOST;
        original.c_lflag = libc::ECHO | libc::ICANON | libc::ISIG | libc::IEXTEN;
        let raw = raw_settings(original);
        assert_eq!(
            raw.c_lflag & (libc::ECHO | libc::ICANON | libc::ISIG | libc::IEXTEN),
            0
        );
        assert_eq!(raw.c_iflag & (libc::ICRNL | libc::IXON), 0);
        assert_eq!(raw.c_oflag & libc::OPOST, 0);
        assert_eq!(raw.c_cc[libc::VMIN], 1);
        assert_eq!(raw.c_cc[libc::VTIME], 0);
    }

    #[test]
    fn missing_broker_dimensions_use_a_usable_default() {
        for size in [(0, 0), (0, 24), (80, 0)] {
            assert_eq!(dimensions(size.0, size.1), (80, 24));
        }
        assert_eq!(dimensions(111, 37), (111, 37));
    }
}
