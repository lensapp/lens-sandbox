#[derive(Debug, Clone)]
pub struct CliRun {
    pub exit_code: i32,
    pub output: String,
}

/// The warn-level messages a verb logs, which reach the developer through the subscriber the real binary installs rather than through the writers a step hands in.
pub fn capture_warnings(emit: impl FnOnce()) -> Vec<String> {
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing_subscriber::layer::{Context as LayerContext, Layer};
    use tracing_subscriber::prelude::*;

    type Sink = Arc<Mutex<Vec<String>>>;
    struct CapturingLayer(Sink);
    impl<S: tracing::Subscriber> Layer<S> for CapturingLayer {
        fn on_event(&self, event: &tracing::Event<'_>, _: LayerContext<'_, S>) {
            struct V<'a>(&'a mut String);
            impl Visit for V<'_> {
                fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                    if field.name() == "message" {
                        *self.0 = format!("{value:?}");
                    }
                }
            }
            let mut message = String::new();
            event.record(&mut V(&mut message));
            self.0.lock().unwrap().push(message);
        }
    }

    let sink: Sink = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(CapturingLayer(sink.clone()));
    tracing::subscriber::with_default(subscriber, emit);
    sink.lock().unwrap().clone()
}

pub fn run_lns(args: &[&str]) -> CliRun {
    let mut argv: Vec<&str> = vec!["lns"];
    argv.extend(args);
    match lns_cli::command::try_get_matches_from(argv).map(|_| ()) {
        Ok(_matches) => CliRun {
            exit_code: 0,
            output: String::new(),
        },
        Err(e) => CliRun {
            exit_code: e.exit_code(),
            output: e.to_string(),
        },
    }
}
