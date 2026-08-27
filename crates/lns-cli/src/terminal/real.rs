use std::fs::File;
use std::io::{BufRead, BufReader};

use super::Terminal;

pub struct RealTerminal {
    tty: Option<BufReader<File>>,
}

impl RealTerminal {
    pub fn open() -> Self {
        Self {
            tty: File::open("/dev/tty").ok().map(BufReader::new),
        }
    }
}

impl Terminal for RealTerminal {
    fn is_available(&self) -> bool {
        self.tty.is_some()
    }

    fn read_answer(&mut self) -> std::io::Result<String> {
        let Some(tty) = self.tty.as_mut() else {
            return Err(std::io::Error::other(
                "there is no controlling terminal to read an answer from",
            ));
        };
        let mut line = String::new();
        tty.read_line(&mut line)?;
        Ok(line)
    }
}
