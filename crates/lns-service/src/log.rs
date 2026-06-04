use anstyle::{AnsiColor, Style};
use lns_ipc::{LogLevel, Response, WireFrame};
use std::io::IsTerminal;
use tokio::sync::mpsc::Sender;
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

pub const TARGET: &str = "lns.log";

#[derive(Clone)]
pub(crate) struct RunFrameTx(pub Sender<WireFrame>);

pub fn attach_to_run_span(tx: Sender<WireFrame>) {
    let span = tracing::Span::current();
    span.with_subscriber(|(id, dispatch)| {
        if let Some(reg) = dispatch.downcast_ref::<tracing_subscriber::Registry>()
            && let Some(s) = reg.span(id)
        {
            s.extensions_mut().insert(RunFrameTx(tx.clone()));
        }
    });
}

pub fn init() {
    let stderr_is_tty = std::io::stderr().is_terminal();
    let color = detect_color(std::env::var_os("NO_COLOR").is_some(), stderr_is_tty);

    let log = tsfmt::layer()
        .event_format(LogFormat { color })
        .with_writer(std::io::stderr)
        .with_filter(local_log_filter(stderr_is_tty));

    let trace = tsfmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(color)
        .with_filter(env_filter())
        .with_filter(filter_fn(|m| m.target() != TARGET));

    let _ = tracing_subscriber::registry()
        .with(log)
        .with(trace)
        .with(FrameForwardLayer)
        .try_init();
}

pub use ::tracing::debug;

fn detect_color(no_color_set: bool, stderr_is_terminal: bool) -> bool {
    !no_color_set && stderr_is_terminal
}

fn local_log_layer_accepts(target: &str, in_run_scope: bool, stderr_is_tty: bool) -> bool {
    target == TARGET && (!in_run_scope || !stderr_is_tty)
}

fn local_log_filter<S>(stderr_is_tty: bool) -> LocalLogFilter<S> {
    LocalLogFilter {
        stderr_is_tty,
        _marker: std::marker::PhantomData,
    }
}

struct LocalLogFilter<S> {
    stderr_is_tty: bool,
    _marker: std::marker::PhantomData<fn(S)>,
}

impl<S> tracing_subscriber::layer::Filter<S> for LocalLogFilter<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn enabled(
        &self,
        meta: &tracing::Metadata<'_>,
        _cx: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        meta.is_span() || meta.target() == TARGET
    }

    fn event_enabled(
        &self,
        event: &Event<'_>,
        cx: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        local_log_layer_accepts(
            event.metadata().target(),
            event_in_run_scope(event, cx),
            self.stderr_is_tty,
        )
    }
}

fn event_in_run_scope<S>(event: &Event<'_>, cx: &tracing_subscriber::layer::Context<'_, S>) -> bool
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let Some(scope) = cx.event_scope(event) else {
        return false;
    };
    scope
        .from_root()
        .any(|span| span.extensions().get::<RunFrameTx>().is_some())
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

fn env_filter() -> EnvFilter {
    if let Ok(v) = std::env::var("LNS_LOG") {
        return EnvFilter::new(v);
    }
    if let Ok(v) = std::env::var("RUST_LOG") {
        return EnvFilter::new(v);
    }
    EnvFilter::new("info")
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
}

impl<S, N> FormatEvent<S, N> for LogFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        let mut v = EventVisitor::default();
        event.record(&mut v);

        let level = *event.metadata().level();
        let extras = format_extras(&v.extras);
        match level {
            Level::INFO => {
                let verb = v.verb.as_deref().unwrap_or("");
                let s = self.style(AnsiColor::Green, true);
                writeln!(writer, "{s}{verb:>12}{s:#}  {}{extras}", v.message)
            }
            Level::WARN => {
                let s = self.style(AnsiColor::Yellow, false);
                writeln!(writer, "{s}warning:{s:#} {}{extras}", v.message)
            }
            Level::ERROR => {
                let s = self.style(AnsiColor::Red, true);
                writeln!(writer, "{s}error:{s:#} {}{extras}", v.message)
            }
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

// `try_send` rather than `send` because `on_event` is synchronous; log frames are dropped on full rather than blocking the emitter.
struct FrameForwardLayer;

impl<S> Layer<S> for FrameForwardLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != TARGET {
            return;
        }
        let level = match *event.metadata().level() {
            Level::ERROR => LogLevel::Error,
            Level::WARN => LogLevel::Warn,
            Level::INFO => LogLevel::Info,
            Level::DEBUG => LogLevel::Debug,
            _ => return,
        };

        let Some(scope) = ctx.event_scope(event) else {
            return;
        };
        let Some(tx) = scope
            .from_root()
            .find_map(|span| span.extensions().get::<RunFrameTx>().cloned())
        else {
            return;
        };

        let mut v = EventVisitor::default();
        event.record(&mut v);
        let mut message = v.message;
        message.push_str(&format_extras(&v.extras));
        let wire = WireFrame::Json(Response::RunLog {
            level,
            verb: v.verb,
            message,
        });
        let _ = tx.0.try_send(wire);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::subscriber::with_default;
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone)]
    struct TestBuf(Arc<Mutex<Vec<u8>>>);

    impl TestBuf {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(Vec::new())))
        }
        fn contents(&self) -> Vec<u8> {
            self.0.lock().unwrap().clone()
        }
        fn text(&self) -> String {
            String::from_utf8(self.contents()).unwrap()
        }
    }

    impl std::io::Write for TestBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for TestBuf {
        type Writer = Self;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn event_visitor_captures_str_extras_in_record_order() {
        use std::sync::{Arc, Mutex};
        struct Capture(Arc<Mutex<Option<EventVisitor>>>);
        impl<S: Subscriber> Layer<S> for Capture {
            fn on_event(&self, event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
                let mut v = EventVisitor::default();
                event.record(&mut v);
                *self.0.lock().unwrap() = Some(v);
            }
        }
        let slot = Arc::new(Mutex::new(None));
        let layer = Capture(slot.clone());
        let subscriber = tracing_subscriber::registry().with(layer);
        with_default(subscriber, || {
            tracing::warn!(target: TARGET, error = "boom", path = "/tmp/x", "agent stdio");
        });
        let v = slot.lock().unwrap().take().expect("event captured");
        assert_eq!(v.message, "agent stdio");
        assert_eq!(
            v.extras,
            vec![
                ("error".to_string(), "boom".to_string()),
                ("path".to_string(), "/tmp/x".to_string()),
            ],
            "extras must preserve record order and exclude verb/message",
        );
    }

    #[test]
    fn format_extras_renders_leading_space_pairs_and_empty_on_no_extras() {
        assert_eq!(format_extras(&[]), "");
        let extras = vec![
            ("error".to_string(), "boom".to_string()),
            ("path".to_string(), "/tmp/x".to_string()),
        ];
        assert_eq!(format_extras(&extras), " error=boom path=/tmp/x");
    }

    #[test]
    fn event_visitor_trims_debug_quotes_on_string_extras() {
        use std::sync::{Arc, Mutex};
        struct Capture(Arc<Mutex<Option<EventVisitor>>>);
        impl<S: Subscriber> Layer<S> for Capture {
            fn on_event(&self, event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
                let mut v = EventVisitor::default();
                event.record(&mut v);
                *self.0.lock().unwrap() = Some(v);
            }
        }
        let slot = Arc::new(Mutex::new(None));
        let subscriber = tracing_subscriber::registry().with(Capture(slot.clone()));
        with_default(subscriber, || {
            let v: &dyn std::fmt::Debug = &"abc";
            tracing::warn!(target: TARGET, k = ?v, "msg");
        });
        let captured = slot.lock().unwrap().take().expect("event captured");
        assert_eq!(captured.extras, vec![("k".to_string(), "abc".to_string())]);
    }

    #[tokio::test]
    async fn frame_forward_layer_inlines_extras_into_run_log_message() {
        use tokio::sync::mpsc;
        let subscriber = tracing_subscriber::registry().with(FrameForwardLayer);
        let (tx, mut rx) = mpsc::channel::<WireFrame>(4);
        with_default(subscriber, || {
            let span = tracing::info_span!("run", run_id = 1u32);
            tracing::dispatcher::get_default(|dispatch| {
                if let Some(reg) = dispatch.downcast_ref::<tracing_subscriber::Registry>()
                    && let Some(id) = span.id()
                    && let Some(s) = reg.span(&id)
                {
                    s.extensions_mut().insert(RunFrameTx(tx.clone()));
                }
            });
            let _g = span.enter();
            tracing::warn!(target: TARGET, error = %"boom", "agent stdio forward failed");
        });
        let frame = rx.try_recv().expect("RunLog frame emitted");
        match frame {
            WireFrame::Json(Response::RunLog { level, message, .. }) => {
                assert!(matches!(level, lns_ipc::LogLevel::Warn));
                assert_eq!(message, "agent stdio forward failed error=boom");
            }
            other => panic!("expected WireFrame::Json(RunLog), got {other:?}"),
        }
    }

    #[test]
    fn local_log_layer_accepts_truth_table() {
        assert!(
            local_log_layer_accepts(TARGET, false, true),
            "non-run service log renders to the service's own stderr/log",
        );
        assert!(
            !local_log_layer_accepts(TARGET, true, true),
            "on a shared tty, run-scoped events are forwarded to the CLI, not echoed locally",
        );
        assert!(
            local_log_layer_accepts(TARGET, true, false),
            "off a tty (service.log), run-scoped events must still render so the log stays complete",
        );
        assert!(
            local_log_layer_accepts(TARGET, false, false),
            "non-run service log still renders to service.log when stderr is not a tty",
        );
        assert!(
            !local_log_layer_accepts("other::module", false, true),
            "non-TARGET events are handled by the trace layer, not the human log layer",
        );
        assert!(!local_log_layer_accepts("other::module", true, true));
        assert!(!local_log_layer_accepts("other::module", true, false));
    }

    fn local_filter_subscriber(
        buf: &TestBuf,
        stderr_is_tty: bool,
    ) -> impl Subscriber + for<'a> LookupSpan<'a> {
        let local = tsfmt::layer()
            .event_format(LogFormat { color: false })
            .with_writer(buf.clone())
            .with_filter(local_log_filter(stderr_is_tty));
        tracing_subscriber::registry()
            .with(local)
            .with(FrameForwardLayer)
    }

    #[tokio::test]
    async fn run_scoped_event_forwards_a_frame_but_emits_no_local_bytes_on_a_shared_tty() {
        use tokio::sync::mpsc;

        let buf = TestBuf::new();
        let subscriber = local_filter_subscriber(&buf, true);
        let (tx, mut rx) = mpsc::channel::<WireFrame>(4);
        with_default(subscriber, || {
            let span = tracing::info_span!("run", run_id = 1u32);
            attach_tx_to_span(&span, tx);
            let _g = span.enter();
            tracing::info!(target: TARGET, verb = "Booted", "microVM");
        });
        assert!(
            rx.try_recv().is_ok(),
            "run-scoped event must produce a RunLog frame to the CLI",
        );
        let local_bytes = buf.contents();
        assert!(
            local_bytes.is_empty(),
            "run-scoped event leaked to the shared controlling terminal: {:?}",
            String::from_utf8_lossy(&local_bytes),
        );
    }

    #[tokio::test]
    async fn run_scoped_event_still_renders_to_service_log_when_stderr_is_not_a_tty() {
        use tokio::sync::mpsc;

        let buf = TestBuf::new();
        let subscriber = local_filter_subscriber(&buf, false);
        let (tx, mut rx) = mpsc::channel::<WireFrame>(4);
        with_default(subscriber, || {
            let span = tracing::info_span!("run", run_id = 1u32);
            attach_tx_to_span(&span, tx);
            let _g = span.enter();
            tracing::info!(target: TARGET, verb = "Booted", "microVM");
        });
        assert!(
            rx.try_recv().is_ok(),
            "run-scoped event must still be forwarded as a frame to the CLI",
        );
        assert!(
            buf.text().contains("Booted  microVM"),
            "detached service.log must keep run-scoped lines: {:?}",
            buf.text(),
        );
    }

    #[test]
    fn event_in_a_non_run_span_still_renders_locally() {
        let buf = TestBuf::new();
        let local = tsfmt::layer()
            .event_format(LogFormat { color: false })
            .with_writer(buf.clone())
            .with_filter(local_log_filter(true));
        let subscriber = tracing_subscriber::registry().with(local);
        with_default(subscriber, || {
            let span = tracing::info_span!("housekeeping");
            let _g = span.enter();
            tracing::info!(target: TARGET, verb = "Pruned", "old caches");
        });
        assert!(
            buf.text().contains("Pruned  old caches"),
            "a span without a RunFrameTx must not suppress the local render: {:?}",
            buf.text(),
        );
    }

    #[test]
    fn non_run_event_renders_to_the_local_human_layer() {
        let buf = TestBuf::new();
        let local = tsfmt::layer()
            .event_format(LogFormat { color: false })
            .with_writer(buf.clone())
            .with_filter(local_log_filter(true));
        let subscriber = tracing_subscriber::registry().with(local);
        with_default(subscriber, || {
            tracing::info!(target: TARGET, verb = "Starting", "lns-service");
            tracing::info!(target: "other::module", "trace-only noise");
        });
        let rendered = buf.text();
        assert!(
            rendered.contains("Starting  lns-service"),
            "non-run service log must still render locally: {rendered:?}",
        );
        assert!(
            !rendered.contains("trace-only noise"),
            "non-TARGET events must not reach the human log layer: {rendered:?}",
        );
    }

    #[test]
    fn detect_color_truth_table() {
        assert!(!detect_color(true, true), "NO_COLOR set wins over TTY");
        assert!(
            !detect_color(true, false),
            "NO_COLOR set + no TTY → no color"
        );
        assert!(!detect_color(false, false), "no TTY → no color");
        assert!(detect_color(false, true), "TTY + no NO_COLOR → color");
    }

    #[test]
    fn log_format_style_returns_blank_when_color_disabled() {
        let f = LogFormat { color: false };
        assert_eq!(f.style(AnsiColor::Green, true), Style::new());
        assert_eq!(f.style(AnsiColor::Red, false), Style::new());
    }

    #[test]
    fn log_format_style_distinguishes_bold_when_color_enabled() {
        let f = LogFormat { color: true };
        let bold = f.style(AnsiColor::Green, true);
        let plain = f.style(AnsiColor::Green, false);
        assert_ne!(bold, plain);
        assert_ne!(bold, Style::new());
        assert_ne!(plain, Style::new());
    }

    fn render_with_log_format(emit: impl FnOnce()) -> String {
        let buf = TestBuf::new();
        std::io::Write::flush(&mut buf.clone()).unwrap();
        let layer = tsfmt::layer()
            .event_format(LogFormat { color: false })
            .with_writer(buf.clone())
            .with_filter(filter_fn(|m| m.target() == TARGET));
        let subscriber = tracing_subscriber::registry().with(layer);
        with_default(subscriber, emit);
        buf.text()
    }

    #[test]
    fn log_format_renders_info_line_with_right_justified_verb() {
        let out = render_with_log_format(|| {
            tracing::info!(target: TARGET, verb = "Installing", "hello");
        });
        assert!(out.contains("Installing  hello"), "got: {out:?}");
    }

    #[test]
    fn log_format_renders_warn_line_with_warning_prefix() {
        let out = render_with_log_format(|| {
            tracing::warn!(target: TARGET, "soft fail");
        });
        assert!(out.contains("warning: soft fail"), "got: {out:?}");
    }

    #[test]
    fn log_format_renders_error_line_with_error_prefix() {
        let out = render_with_log_format(|| {
            tracing::error!(target: TARGET, "broken");
        });
        assert!(out.contains("error: broken"), "got: {out:?}");
    }

    #[test]
    fn log_format_drops_debug_via_fallthrough_arm() {
        let out = render_with_log_format(|| {
            tracing::debug!(target: TARGET, "noisy");
        });
        assert!(!out.contains("noisy"), "got: {out:?}");
    }

    #[test]
    #[serial_test::serial(env)]
    fn env_filter_prefers_lns_log_over_rust_log() {
        let _lns = crate::test_env::EnvVarGuard::set("LNS_LOG", "warn");
        let _rust = crate::test_env::EnvVarGuard::set("RUST_LOG", "trace");
        assert_eq!(env_filter().to_string(), "warn");
    }

    #[test]
    #[serial_test::serial(env)]
    fn env_filter_falls_back_to_rust_log_when_lns_log_unset() {
        let _lns = crate::test_env::EnvVarGuard::unset("LNS_LOG");
        let _rust = crate::test_env::EnvVarGuard::set("RUST_LOG", "debug");
        assert_eq!(env_filter().to_string(), "debug");
    }

    #[test]
    #[serial_test::serial(env)]
    fn env_filter_defaults_to_info_when_neither_env_set() {
        let _lns = crate::test_env::EnvVarGuard::unset("LNS_LOG");
        let _rust = crate::test_env::EnvVarGuard::unset("RUST_LOG");
        assert_eq!(env_filter().to_string(), "info");
    }

    #[test]
    fn attach_to_run_span_inserts_tx_into_current_span_extensions() {
        use tokio::sync::mpsc;
        let (tx, _rx) = mpsc::channel::<WireFrame>(4);
        let subscriber = tracing_subscriber::registry();
        with_default(subscriber, || {
            let span = tracing::info_span!("run");
            let _g = span.enter();
            attach_to_run_span(tx);
            let mut has_tx = false;
            tracing::Span::current().with_subscriber(|(id, dispatch)| {
                if let Some(reg) = dispatch.downcast_ref::<tracing_subscriber::Registry>()
                    && let Some(s) = reg.span(id)
                {
                    has_tx = s.extensions().get::<RunFrameTx>().is_some();
                }
            });
            assert!(has_tx, "RunFrameTx must be installed in the run span");
        });
    }

    #[test]
    #[serial_test::serial(env)]
    fn init_runs_without_panicking() {
        let _lns = crate::test_env::EnvVarGuard::unset("LNS_LOG");
        let _rust = crate::test_env::EnvVarGuard::unset("RUST_LOG");
        init();
        init();
    }

    #[test]
    fn event_visitor_strips_debug_quotes_from_verb() {
        use std::sync::{Arc, Mutex};
        struct Capture(Arc<Mutex<Option<EventVisitor>>>);
        impl<S: Subscriber> Layer<S> for Capture {
            fn on_event(&self, event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
                let mut v = EventVisitor::default();
                event.record(&mut v);
                *self.0.lock().unwrap() = Some(v);
            }
        }
        let slot = Arc::new(Mutex::new(None));
        let subscriber = tracing_subscriber::registry().with(Capture(slot.clone()));
        with_default(subscriber, || {
            let v: &dyn std::fmt::Debug = &"install";
            tracing::info!(target: TARGET, verb = ?v, "msg");
        });
        let captured = slot.lock().unwrap().take().expect("event captured");
        assert_eq!(captured.verb.as_deref(), Some("install"));
    }

    fn attach_tx_to_span(span: &tracing::Span, tx: tokio::sync::mpsc::Sender<WireFrame>) {
        tracing::dispatcher::get_default(|dispatch| {
            if let Some(reg) = dispatch.downcast_ref::<tracing_subscriber::Registry>()
                && let Some(id) = span.id()
                && let Some(s) = reg.span(&id)
            {
                s.extensions_mut().insert(RunFrameTx(tx.clone()));
            }
        });
    }

    #[tokio::test]
    async fn frame_forward_layer_ignores_events_with_other_targets() {
        use tokio::sync::mpsc;
        let subscriber = tracing_subscriber::registry().with(FrameForwardLayer);
        let (tx, mut rx) = mpsc::channel::<WireFrame>(4);
        with_default(subscriber, || {
            let span = tracing::info_span!("run");
            attach_tx_to_span(&span, tx);
            let _g = span.enter();
            tracing::warn!(target: "other::module", "not for the cli");
        });
        assert!(
            rx.try_recv().is_err(),
            "non-TARGET events must not produce a RunLog frame",
        );
    }

    #[tokio::test]
    async fn frame_forward_layer_drops_events_without_span_scope() {
        use tokio::sync::mpsc;
        let subscriber = tracing_subscriber::registry().with(FrameForwardLayer);
        let (_tx, mut rx) = mpsc::channel::<WireFrame>(4);
        with_default(subscriber, || {
            tracing::warn!(target: TARGET, "no span");
        });
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn frame_forward_layer_drops_events_when_no_tx_attached() {
        use tokio::sync::mpsc;
        let subscriber = tracing_subscriber::registry().with(FrameForwardLayer);
        let (_tx, mut rx) = mpsc::channel::<WireFrame>(4);
        with_default(subscriber, || {
            let span = tracing::info_span!("run");
            let _g = span.enter();
            tracing::warn!(target: TARGET, "no tx");
        });
        assert!(rx.try_recv().is_err());
    }

    fn collect_run_log_levels(emit: impl FnOnce()) -> Vec<LogLevel> {
        use tokio::sync::mpsc;
        let subscriber = tracing_subscriber::registry().with(FrameForwardLayer);
        let (tx, mut rx) = mpsc::channel::<WireFrame>(4);
        with_default(subscriber, || {
            let span = tracing::info_span!("run");
            attach_tx_to_span(&span, tx);
            let _g = span.enter();
            emit();
        });
        let mut out = Vec::new();
        while let Ok(WireFrame::Json(Response::RunLog { level, .. })) = rx.try_recv() {
            out.push(level);
        }
        out
    }

    #[tokio::test]
    async fn frame_forward_layer_emits_error_frame() {
        let levels = collect_run_log_levels(|| tracing::error!(target: TARGET, "e"));
        assert_eq!(levels.len(), 1);
        assert!(matches!(levels[0], LogLevel::Error));
    }

    #[tokio::test]
    async fn frame_forward_layer_emits_info_frame() {
        let levels = collect_run_log_levels(|| tracing::info!(target: TARGET, "i"));
        assert_eq!(levels.len(), 1);
        assert!(matches!(levels[0], LogLevel::Info));
    }

    #[tokio::test]
    async fn frame_forward_layer_emits_debug_frame() {
        let levels = collect_run_log_levels(|| tracing::debug!(target: TARGET, "d"));
        assert_eq!(levels.len(), 1);
        assert!(matches!(levels[0], LogLevel::Debug));
    }

    #[tokio::test]
    async fn frame_forward_layer_drops_trace_frames() {
        let levels = collect_run_log_levels(|| tracing::trace!(target: TARGET, "t"));
        assert!(levels.is_empty(), "TRACE must not produce a RunLog");
    }
}
