pub fn split_args(cmd_line: &str) -> Vec<String> {
    let trimmed = match cmd_line.strip_prefix("lns") {
        Some(rest) if rest.is_empty() || rest.starts_with(' ') => rest.trim_start_matches(' '),
        _ => cmd_line,
    };
    let mut scanner = ArgScanner::default();
    let mut out: Vec<String> = Vec::new();
    for ch in trimmed.chars() {
        scanner.feed(ch, &mut out);
    }
    scanner.flush(&mut out);
    out
}

#[derive(Default)]
struct ArgScanner {
    current: String,
    quote_char: Option<char>,
}

impl ArgScanner {
    fn feed(&mut self, ch: char, out: &mut Vec<String>) {
        if self.try_open_quote(ch) || self.try_close_quote(ch) {
            return;
        }
        if ch == ' ' && self.quote_char.is_none() {
            self.flush(out);
            return;
        }
        self.current.push(ch);
    }

    fn flush(&mut self, out: &mut Vec<String>) {
        if !self.current.is_empty() {
            out.push(std::mem::take(&mut self.current));
        }
    }

    fn try_open_quote(&mut self, ch: char) -> bool {
        if self.quote_char.is_some() || !self.current.is_empty() || (ch != '"' && ch != '\'') {
            return false;
        }
        self.quote_char = Some(ch);
        true
    }

    fn try_close_quote(&mut self, ch: char) -> bool {
        if self.quote_char != Some(ch) {
            return false;
        }
        self.quote_char = None;
        true
    }
}
