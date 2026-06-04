use std::io::IsTerminal;

use anstyle::{AnsiColor, Style};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt as tsfmt;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

pub(crate) const TARGET: &str = "lns.log";

pub fn init(level: crate::cli::LogLevel) {
    let color = detect_color(
        std::env::var_os("NO_COLOR").is_some(),
        std::io::stderr().is_terminal(),
    );
    let level = cli_level_to_tracing(level);

    let log = tsfmt::layer()
        .event_format(LogFormat { color })
        .with_writer(std::io::stderr)
        .with_filter(filter_fn(move |m| {
            log_layer_accepts(m.target(), *m.level(), level)
        }));

    let trace = tsfmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(color)
        .with_filter(env_filter(level))
        .with_filter(filter_fn(|m| trace_layer_accepts(m.target())));

    let _ = tracing_subscriber::registry()
        .with(log)
        .with(trace)
        .try_init();
}

pub use ::tracing::debug;

fn detect_color(no_color_set: bool, stderr_is_terminal: bool) -> bool {
    !no_color_set && stderr_is_terminal
}

fn cli_level_to_tracing(level: crate::cli::LogLevel) -> Level {
    use crate::cli::LogLevel as L;
    match level {
        L::Error => Level::ERROR,
        L::Warn => Level::WARN,
        L::Info => Level::INFO,
        L::Debug => Level::DEBUG,
    }
}

fn log_layer_accepts(target: &str, event_level: Level, filter_level: Level) -> bool {
    target == TARGET && event_level <= filter_level
}

fn trace_layer_accepts(target: &str) -> bool {
    target != TARGET
}

#[macro_export]
macro_rules! __log_error {
    ($($arg:tt)+) => { ::tracing::error!(target: $crate::log::TARGET, $($arg)+) };
}

#[macro_export]
macro_rules! __log_warn {
    ($($arg:tt)+) => { ::tracing::warn!(target: $crate::log::TARGET, $($arg)+) };
}

#[macro_export]
macro_rules! __log_info {
    ($verb:expr, $($arg:tt)+) => {
        ::tracing::info!(target: $crate::log::TARGET, verb = $verb, $($arg)+)
    };
}

pub use crate::{__log_error as error, __log_info as info, __log_warn as warn};

fn env_filter(level: Level) -> EnvFilter {
    if let Ok(v) = std::env::var("LNS_LOG") {
        return EnvFilter::new(v);
    }
    if let Ok(v) = std::env::var("RUST_LOG") {
        return EnvFilter::new(v);
    }
    EnvFilter::new(level.to_string().to_lowercase())
}

struct LogFormat {
    color: bool,
}

impl LogFormat {
    fn style(&self, color: AnsiColor, bold: bool) -> Style {
        if !self.color {
            return Style::new();
        }
        let s = Style::new().fg_color(Some(color.into()));
        if bold { s.bold() } else { s }
    }

    fn write_info(
        &self,
        mut writer: Writer<'_>,
        v: &EventVisitor,
        extras: &str,
    ) -> std::fmt::Result {
        let verb = v.verb.as_deref().unwrap_or("");
        let s = self.style(AnsiColor::Green, true);
        writeln!(writer, "{s}{verb:>12}{s:#}  {}{extras}", v.message)
    }

    fn write_warn(
        &self,
        mut writer: Writer<'_>,
        v: &EventVisitor,
        extras: &str,
    ) -> std::fmt::Result {
        let s = self.style(AnsiColor::Yellow, false);
        writeln!(writer, "{s}warning:{s:#} {}{extras}", v.message)
    }

    fn write_error(
        &self,
        mut writer: Writer<'_>,
        v: &EventVisitor,
        extras: &str,
    ) -> std::fmt::Result {
        let s = self.style(AnsiColor::Red, true);
        writeln!(writer, "{s}error:{s:#} {}{extras}", v.message)
    }
}

impl<S, N> FormatEvent<S, N> for LogFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        let mut v = EventVisitor::default();
        event.record(&mut v);

        let level = *event.metadata().level();
        let extras = format_extras(&v.extras);
        match level {
            Level::INFO => self.write_info(writer, &v, &extras),
            Level::WARN => self.write_warn(writer, &v, &extras),
            Level::ERROR => self.write_error(writer, &v, &extras),
            _ => Ok(()),
        }
    }
}

#[derive(Default)]
struct EventVisitor {
    verb: Option<String>,
    message: String,
    extras: Vec<(String, String)>,
}

impl Visit for EventVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "verb" => self.verb = Some(value.to_string()),
            "message" => self.message = value.to_string(),
            other => self.extras.push((other.to_string(), value.to_string())),
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "message" => self.message = format!("{value:?}"),
            "verb" => {
                let s = format!("{value:?}");
                self.verb = Some(s.trim_matches('"').to_string());
            }
            other => {
                let s = format!("{value:?}");
                let s = s.trim_matches('"').to_string();
                self.extras.push((other.to_string(), s));
            }
        }
    }
}

fn format_extras(extras: &[(String, String)]) -> String {
    let mut out = String::new();
    for (k, v) in extras {
        out.push(' ');
        out.push_str(k);
        out.push('=');
        out.push_str(v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::subscriber::with_default;
    use tracing_subscriber::fmt::MakeWriter;

    #[test]
    fn detect_color_truth_table() {
        assert!(detect_color(false, true));
        assert!(!detect_color(true, true));
        assert!(!detect_color(false, false));
        assert!(!detect_color(true, false));
    }

    #[test]
    fn cli_level_maps_to_tracing_level() {
        use crate::cli::LogLevel as L;
        assert_eq!(cli_level_to_tracing(L::Error), Level::ERROR);
        assert_eq!(cli_level_to_tracing(L::Warn), Level::WARN);
        assert_eq!(cli_level_to_tracing(L::Info), Level::INFO);
        assert_eq!(cli_level_to_tracing(L::Debug), Level::DEBUG);
    }

    #[test]
    #[serial_test::serial(env)]
    fn env_filter_prefers_lns_log_over_rust_log() {
        let _lns = crate::test_env::EnvScope::set("LNS_LOG", "warn");
        let _rust = crate::test_env::EnvScope::set("RUST_LOG", "trace");
        assert_eq!(env_filter(Level::INFO).to_string(), "warn");
    }

    #[test]
    #[serial_test::serial(env)]
    fn env_filter_falls_back_to_rust_log_when_lns_log_unset() {
        let _lns = crate::test_env::EnvScope::unset("LNS_LOG");
        let _rust = crate::test_env::EnvScope::set("RUST_LOG", "debug");
        assert_eq!(env_filter(Level::INFO).to_string(), "debug");
    }

    #[test]
    fn log_layer_accepts_truth_table() {
        assert!(log_layer_accepts(TARGET, Level::INFO, Level::INFO));
        assert!(log_layer_accepts(TARGET, Level::WARN, Level::INFO));
        assert!(log_layer_accepts(TARGET, Level::ERROR, Level::INFO));
        assert!(!log_layer_accepts(TARGET, Level::DEBUG, Level::INFO));
        assert!(!log_layer_accepts(TARGET, Level::TRACE, Level::INFO));
        assert!(!log_layer_accepts("module::path", Level::INFO, Level::INFO));
        assert!(!log_layer_accepts(
            "module::path",
            Level::ERROR,
            Level::DEBUG
        ));
    }

    #[test]
    fn trace_layer_accepts_truth_table() {
        assert!(trace_layer_accepts("module::path"));
        assert!(trace_layer_accepts(""));
        assert!(!trace_layer_accepts(TARGET));
    }

    #[test]
    fn log_format_style_returns_blank_when_color_off() {
        let f = LogFormat { color: false };
        assert_eq!(f.style(AnsiColor::Red, true), Style::new());
        assert_eq!(f.style(AnsiColor::Green, false), Style::new());
    }

    #[test]
    fn log_format_style_emits_color_when_on() {
        let f = LogFormat { color: true };
        assert_ne!(f.style(AnsiColor::Red, true), Style::new());
        assert_ne!(f.style(AnsiColor::Green, false), Style::new());
    }

    #[test]
    #[serial_test::serial(env)]
    fn env_filter_falls_back_to_level_directive() {
        let _lns = crate::test_env::EnvScope::unset("LNS_LOG");
        let _rust = crate::test_env::EnvScope::unset("RUST_LOG");
        assert_eq!(env_filter(Level::WARN).to_string(), "warn");
        assert_eq!(env_filter(Level::DEBUG).to_string(), "debug");
    }

    #[derive(Clone)]
    struct VecWriter(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for VecWriter {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for VecWriter {
        type Writer = VecWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn capture(level: Level, color: bool, emit: impl FnOnce()) -> String {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = VecWriter(buf.clone());
        let layer = tsfmt::layer()
            .event_format(LogFormat { color })
            .with_writer(writer)
            .with_filter(filter_fn(move |m| {
                m.target() == TARGET && *m.level() <= level
            }));
        let subscriber = tracing_subscriber::registry().with(layer);
        with_default(subscriber, emit);
        String::from_utf8(buf.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn info_event_renders_verb_and_message() {
        let out = capture(Level::INFO, false, || {
            tracing::info!(target: TARGET, verb = "Booting", "microVM via Vz");
        });
        assert!(
            out.contains("     Booting  microVM via Vz"),
            "unexpected info render: {out:?}"
        );
    }

    #[test]
    fn record_str_captures_message_field() {
        let out = capture(Level::INFO, false, || {
            tracing::info!(target: TARGET, verb = "X", message = "via-str");
        });
        assert!(
            out.contains("via-str"),
            "record_str message arm did not capture: {out:?}"
        );
    }

    #[test]
    fn record_str_routes_unknown_string_field_to_extras() {
        let out = capture(Level::WARN, false, || {
            tracing::warn!(target: TARGET, message = "boom", reason = "denied");
        });
        assert!(
            out.contains("warning: boom reason=denied"),
            "record_str extras arm did not capture: {out:?}"
        );
    }

    #[test]
    #[serial_test::serial(global_subscriber)]
    fn init_installs_subscriber_idempotently() {
        super::init(crate::cli::LogLevel::Info);
        super::init(crate::cli::LogLevel::Debug);
    }

    #[test]
    fn warn_event_renders_warning_prefix() {
        let out = capture(Level::WARN, false, || {
            tracing::warn!(target: TARGET, "something off");
        });
        assert!(
            out.starts_with("warning: something off"),
            "unexpected warn render: {out:?}"
        );
    }

    #[test]
    fn log_info_macro_routes_through_tracing() {
        let out = capture(Level::INFO, false, || {
            crate::log::info!("Verb", "wired-via-macro");
        });
        assert!(
            out.contains("wired-via-macro"),
            "macro must dispatch: {out:?}"
        );
        assert!(out.contains("Verb"), "verb must render: {out:?}");
    }

    #[test]
    fn debug_event_is_suppressed_by_format_event_catch_all() {
        let out = capture(Level::DEBUG, false, || {
            tracing::debug!(target: TARGET, "debug-noise");
        });
        assert_eq!(out, "", "DEBUG must render to empty via the _ arm: {out:?}");
    }

    #[test]
    fn vec_writer_flush_is_a_noop() {
        use std::io::Write;
        let buf = Arc::new(Mutex::new(Vec::new()));
        let mut w = VecWriter(buf);
        w.flush().expect("flush is a no-op");
    }

    #[test]
    fn error_event_renders_error_prefix() {
        let out = capture(Level::ERROR, false, || {
            tracing::error!(target: TARGET, "boom");
        });
        assert!(
            out.starts_with("error: boom"),
            "unexpected error render: {out:?}"
        );
    }

    #[test]
    fn debug_event_is_suppressed_at_warn_level() {
        let out = capture(Level::WARN, false, || {
            tracing::debug!(target: TARGET, "noisy");
        });
        assert!(out.is_empty(), "debug should be filtered out: {out:?}");
    }

    #[test]
    fn info_event_with_debug_valued_verb_still_renders() {
        let out = capture(Level::INFO, false, || {
            let v: &dyn std::fmt::Debug = &"BootDbg";
            tracing::info!(target: TARGET, verb = ?v, "msg");
        });
        assert!(
            out.contains("     BootDbg  msg"),
            "unexpected ?verb render: {out:?}"
        );
    }

    #[test]
    fn warn_event_renders_structured_error_field() {
        let out = capture(Level::WARN, false, || {
            tracing::warn!(target: TARGET, error = %"boom: ENOENT", "agent stdio forward failed");
        });
        assert!(
            out.contains("warning: agent stdio forward failed error=boom: ENOENT"),
            "extras not rendered: {out:?}"
        );
    }

    #[test]
    fn debug_valued_string_extra_has_quotes_trimmed() {
        let out = capture(Level::WARN, false, || {
            let v: &dyn std::fmt::Debug = &"abc";
            tracing::warn!(target: TARGET, k = ?v, "msg");
        });
        assert!(
            out.contains("warning: msg k=abc"),
            "quote-stripped extra missing: {out:?}"
        );
        assert!(
            !out.contains("k=\"abc\""),
            "Debug quotes were not trimmed: {out:?}"
        );
    }

    #[test]
    fn info_event_renders_multiple_extras_in_record_order() {
        let out = capture(Level::INFO, false, || {
            tracing::info!(
                target: TARGET,
                verb = "Booting",
                a = %"1",
                b = %"2",
                "microVM via Vz",
            );
        });
        assert!(
            out.contains("     Booting  microVM via Vz a=1 b=2"),
            "ordered extras missing or wrong order: {out:?}"
        );
    }

    #[test]
    fn format_extras_empty_returns_empty_string() {
        assert_eq!(format_extras(&[]), "");
    }

    #[test]
    fn target_filter_routes_log_vs_trace_layers() {
        let log_buf = Arc::new(Mutex::new(Vec::new()));
        let trace_buf = Arc::new(Mutex::new(Vec::new()));
        let log_layer = tsfmt::layer()
            .event_format(LogFormat { color: false })
            .with_writer(VecWriter(log_buf.clone()))
            .with_filter(filter_fn(|m| {
                m.target() == TARGET && *m.level() <= Level::INFO
            }));
        let trace_layer = tsfmt::layer()
            .with_ansi(false)
            .with_writer(VecWriter(trace_buf.clone()))
            .with_filter(filter_fn(|m| m.target() != TARGET));
        let subscriber = tracing_subscriber::registry()
            .with(log_layer)
            .with(trace_layer);
        with_default(subscriber, || {
            tracing::info!(target: TARGET, verb = "X", "lns-side");
            tracing::info!("module-side");
        });
        let log = String::from_utf8(log_buf.lock().unwrap().clone()).unwrap();
        let trace = String::from_utf8(trace_buf.lock().unwrap().clone()).unwrap();
        assert!(log.contains("lns-side"), "log missing lns event: {log:?}");
        assert!(
            !log.contains("module-side"),
            "log leaked non-TARGET event: {log:?}",
        );
        assert!(
            trace.contains("module-side"),
            "trace missing module event: {trace:?}",
        );
        assert!(
            !trace.contains("lns-side"),
            "trace leaked TARGET event: {trace:?}",
        );
    }
}
