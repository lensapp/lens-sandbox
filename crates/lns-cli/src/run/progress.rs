use std::io::Write;

const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const BAR_WIDTH: usize = 20;

pub struct ProgressRenderer {
    enabled: bool,
    active: Option<ActiveProgress>,
    spin: usize,
    rendered_width: usize,
}

struct ActiveProgress {
    verb: String,
    message: String,
    current: u64,
    total: u64,
}

impl ProgressRenderer {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            active: None,
            spin: 0,
            rendered_width: 0,
        }
    }

    pub fn update(
        &mut self,
        verb: &str,
        message: &str,
        current: u64,
        total: u64,
        writer: &mut impl Write,
    ) -> std::io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let active = ActiveProgress {
            verb: verb.to_string(),
            message: message.to_string(),
            current,
            total,
        };
        let line = progress_line(SPINNER_FRAMES[self.spin], &active);
        self.active = Some(active);
        self.write_line(&line, writer)
    }

    // `active` is only ever set by update(), so a disabled renderer never has a line to animate.
    pub fn tick(&mut self, writer: &mut impl Write) -> std::io::Result<()> {
        let next_spin = (self.spin + 1) % SPINNER_FRAMES.len();
        let line = match &self.active {
            Some(active) => progress_line(SPINNER_FRAMES[next_spin], active),
            None => return Ok(()),
        };
        self.spin = next_spin;
        self.write_line(&line, writer)
    }

    pub fn clear(&mut self, writer: &mut impl Write) -> std::io::Result<()> {
        self.active = None;
        if self.rendered_width == 0 {
            return Ok(());
        }
        let blank = " ".repeat(self.rendered_width);
        write!(writer, "\r{blank}\r")?;
        writer.flush()?;
        self.rendered_width = 0;
        Ok(())
    }

    fn write_line(&mut self, line: &str, writer: &mut impl Write) -> std::io::Result<()> {
        let width = line.chars().count();
        let pad = " ".repeat(self.rendered_width.saturating_sub(width));
        write!(writer, "\r{line}{pad}")?;
        writer.flush()?;
        self.rendered_width = width;
        Ok(())
    }
}

fn progress_line(spinner: char, p: &ActiveProgress) -> String {
    let phrase = p.verb.to_lowercase();
    if p.total > 0 {
        let capped = p.current.min(p.total);
        let ratio = capped as f64 / p.total as f64;
        let filled = (ratio * BAR_WIDTH as f64).round() as usize;
        let bar: String = "█".repeat(filled) + &"░".repeat(BAR_WIDTH - filled);
        let pct = (ratio * 100.0).round() as u64;
        let cur = fmt_bytes(capped);
        let tot = fmt_bytes(p.total);
        format!("{spinner} {phrase}  {bar}  {cur} / {tot}  {pct}%")
    } else if p.current > 0 {
        format!("{spinner} {phrase}  {}", fmt_bytes(p.current))
    } else if p.message.is_empty() {
        format!("{spinner} {phrase}…")
    } else {
        format!("{spinner} {phrase} {}…", p.message)
    }
}

fn fmt_bytes(n: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if n >= GIB {
        format!("{:.1} GiB", n as f64 / GIB as f64)
    } else if n >= MIB {
        format!("{:.1} MiB", n as f64 / MIB as f64)
    } else if n >= KIB {
        format!("{:.1} KiB", n as f64 / KIB as f64)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(buf: &[u8]) -> String {
        String::from_utf8(buf.to_vec()).unwrap()
    }

    #[test]
    fn disabled_renderer_writes_nothing_for_any_call() {
        let mut buf = Vec::new();
        let mut r = ProgressRenderer::new(false);
        r.update("Pulling", "", 5, 10, &mut buf).unwrap();
        r.tick(&mut buf).unwrap();
        r.clear(&mut buf).unwrap();
        let rendered = text(&buf);
        assert!(
            rendered.is_empty(),
            "non-tty output must stay machine-readable: {rendered:?}"
        );
    }

    #[test]
    fn determinate_update_renders_a_bar_with_sizes_and_percent() {
        let mut buf = Vec::new();
        let mut r = ProgressRenderer::new(true);
        r.update("Pulling", "", 26 * 1024 * 1024, 52 * 1024 * 1024, &mut buf)
            .unwrap();
        let s = text(&buf);
        assert!(s.starts_with('\r'), "line must overwrite in place: {s:?}");
        assert!(s.contains("pulling"), "{s:?}");
        assert!(s.contains(&"█".repeat(10)), "{s:?}");
        assert!(s.contains(&"░".repeat(10)), "{s:?}");
        assert!(s.contains("26.0 MiB / 52.0 MiB"), "{s:?}");
        assert!(s.contains("50%"), "{s:?}");
    }

    #[test]
    fn determinate_update_at_zero_and_full_render_empty_and_full_bars() {
        let mut buf = Vec::new();
        let mut r = ProgressRenderer::new(true);
        r.update("Pulling", "", 0, 100, &mut buf).unwrap();
        assert!(text(&buf).contains(&"░".repeat(BAR_WIDTH)));
        buf.clear();
        r.update("Pulling", "", 100, 100, &mut buf).unwrap();
        let s = text(&buf);
        assert!(s.contains(&"█".repeat(BAR_WIDTH)), "{s:?}");
        assert!(s.contains("100%"), "{s:?}");
    }

    #[test]
    fn overshoot_beyond_total_clamps_to_one_hundred_percent() {
        let mut buf = Vec::new();
        let mut r = ProgressRenderer::new(true);
        r.update("Pulling", "", 250, 100, &mut buf).unwrap();
        let s = text(&buf);
        assert!(s.contains("100%"), "{s:?}");
        assert!(s.contains("100 B / 100 B"), "{s:?}");
        assert!(!s.contains("250"), "{s:?}");
    }

    #[test]
    fn indeterminate_update_with_message_renders_a_spinner_phrase() {
        let mut buf = Vec::new();
        let mut r = ProgressRenderer::new(true);
        r.update("Booting", "microVM", 0, 0, &mut buf).unwrap();
        let s = text(&buf);
        assert!(s.contains("booting microVM…"), "{s:?}");
        assert!(!s.contains('█'), "{s:?}");
    }

    #[test]
    fn indeterminate_update_without_message_renders_just_the_phrase() {
        let mut buf = Vec::new();
        let mut r = ProgressRenderer::new(true);
        r.update("Assembling", "", 0, 0, &mut buf).unwrap();
        assert!(text(&buf).contains("assembling…"));
    }

    #[test]
    fn unknown_total_with_bytes_renders_a_byte_counter() {
        let mut buf = Vec::new();
        let mut r = ProgressRenderer::new(true);
        r.update("Pulling", "", 3 * 1024, 0, &mut buf).unwrap();
        let s = text(&buf);
        assert!(s.contains("pulling  3.0 KiB"), "{s:?}");
        assert!(!s.contains('%'), "{s:?}");
    }

    #[test]
    fn tick_advances_the_spinner_glyph_on_the_active_line() {
        let mut buf = Vec::new();
        let mut r = ProgressRenderer::new(true);
        r.update("Booting", "microVM", 0, 0, &mut buf).unwrap();
        let first = text(&buf).chars().nth(1).unwrap();
        buf.clear();
        r.tick(&mut buf).unwrap();
        let second = text(&buf).chars().nth(1).unwrap();
        assert_ne!(first, second, "the spinner must visibly animate");
        assert!(SPINNER_FRAMES.contains(&second));
    }

    #[test]
    fn tick_with_no_active_progress_writes_nothing() {
        let mut buf = Vec::new();
        let mut r = ProgressRenderer::new(true);
        r.tick(&mut buf).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn clear_blanks_the_rendered_line_and_returns_the_cursor() {
        let mut buf = Vec::new();
        let mut r = ProgressRenderer::new(true);
        r.update("Booting", "microVM", 0, 0, &mut buf).unwrap();
        let width = text(&buf).trim_start_matches('\r').chars().count();
        buf.clear();
        r.clear(&mut buf).unwrap();
        assert_eq!(text(&buf), format!("\r{}\r", " ".repeat(width)));
    }

    #[test]
    fn clear_before_any_render_writes_nothing() {
        let mut buf = Vec::new();
        let mut r = ProgressRenderer::new(true);
        r.clear(&mut buf).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn a_shorter_line_pads_over_the_previous_longer_one() {
        let mut buf = Vec::new();
        let mut r = ProgressRenderer::new(true);
        r.update("Pulling", "", 50, 100, &mut buf).unwrap();
        let long_width = text(&buf).trim_start_matches('\r').chars().count();
        buf.clear();
        r.update("Booting", "microVM", 0, 0, &mut buf).unwrap();
        let s = text(&buf);
        let short_width = "⠋ booting microVM…".chars().count();
        assert_eq!(
            s.chars().count(),
            1 + long_width.max(short_width),
            "stale glyphs from the longer line must be overwritten: {s:?}",
        );
        assert!(s.ends_with(&" ".repeat(long_width - short_width)));
    }

    #[test]
    fn fmt_bytes_picks_largest_unit_at_1024_boundary() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(1023), "1023 B");
        assert_eq!(fmt_bytes(1024), "1.0 KiB");
        assert_eq!(fmt_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(fmt_bytes(1024 * 1024 * 1024), "1.0 GiB");
    }
}
