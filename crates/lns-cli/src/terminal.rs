mod real;
pub use real::RealTerminal;

/// Where `lns` reads an answer: the controlling terminal, so data piped into the process is never mistaken for one.
pub trait Terminal {
    fn is_available(&self) -> bool;
    fn read_answer(&mut self) -> std::io::Result<String>;
}

pub fn is_affirmative(answer: &str) -> bool {
    let answer = answer.trim();
    answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
}

/// The strict absent terminal: unlike `ScriptedTerminal::absent`, reading from it fails, so a caller that asks anyway cannot pass by seeing an empty answer.
#[cfg(test)]
pub(crate) struct NoTerminal;

#[cfg(test)]
impl Terminal for NoTerminal {
    fn is_available(&self) -> bool {
        false
    }

    fn read_answer(&mut self) -> std::io::Result<String> {
        Err(std::io::Error::other("there is no terminal to ask at"))
    }
}

#[cfg(test)]
pub(crate) struct ScriptedTerminal {
    answers: std::collections::VecDeque<String>,
    available: bool,
}

#[cfg(test)]
impl ScriptedTerminal {
    pub(crate) fn answering(answers: &[&str]) -> Self {
        Self {
            answers: answers.iter().map(|a| (*a).to_string()).collect(),
            available: true,
        }
    }

    pub(crate) fn answers_left(&self) -> usize {
        self.answers.len()
    }

    pub(crate) fn absent() -> Self {
        Self {
            answers: std::collections::VecDeque::new(),
            available: false,
        }
    }
}

#[cfg(test)]
impl Terminal for ScriptedTerminal {
    fn is_available(&self) -> bool {
        self.available
    }

    fn read_answer(&mut self) -> std::io::Result<String> {
        Ok(self.answers.pop_front().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_with_no_terminal_to_ask_at_never_gets_an_answer() {
        let mut terminal = NoTerminal;
        assert!(!terminal.is_available());
        let err = terminal.read_answer().unwrap_err();
        assert!(
            err.to_string().contains("no terminal to ask at"),
            "a caller that reads anyway must fail, never see a silent empty answer: {err}"
        );
    }

    #[test]
    fn y_and_yes_in_any_case_accept_and_nothing_else_does() {
        for yes in ["y", "Y", "yes", "YES", " yes \n"] {
            assert!(is_affirmative(yes), "{yes:?} must accept");
        }
        for no in ["", "\n", "n", "no", "ye", "yess", "sure"] {
            assert!(!is_affirmative(no), "{no:?} must not accept");
        }
    }
}
