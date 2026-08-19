use lns_artifact::resources::{DeclaredSize, ResourceOverrides, VmSize, resolve_declared};
use lns_artifact::spec::Resources;

pub fn resolve_size(
    sandbox: Option<&Resources>,
    overrides: &ResourceOverrides,
    defaults: VmSize,
    host: Option<lns_artifact::resources::HostCapacity>,
) -> VmSize {
    let (declared, ignored) = DeclaredSize::from_resources(sandbox, host);
    for field in ignored {
        crate::log::warn!(
            "resources.{field} is not a size this host can grant or read; using the default instead"
        );
    }
    resolve_declared(declared, overrides, defaults)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_artifact::resources::DEFAULT_VM_SIZE;
    use lns_artifact::spec::Quantity;

    #[derive(Default)]
    struct MessageCapture(std::sync::Mutex<Vec<String>>);

    struct MessageLayer(std::sync::Arc<MessageCapture>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for MessageLayer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct Message<'a>(&'a mut String);
            impl tracing::field::Visit for Message<'_> {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "message" {
                        *self.0 = format!("{value:?}");
                    }
                }
            }
            let mut message = String::new();
            event.record(&mut Message(&mut message));
            self.0.0.lock().unwrap().push(message);
        }
    }

    fn captured_messages(f: impl FnOnce()) -> Vec<String> {
        use tracing_subscriber::layer::SubscriberExt;
        let capture = std::sync::Arc::new(MessageCapture::default());
        let subscriber =
            tracing_subscriber::registry().with(MessageLayer(std::sync::Arc::clone(&capture)));
        tracing::subscriber::with_default(subscriber, f);
        capture.0.lock().unwrap().clone()
    }

    #[test]
    fn a_request_this_host_will_not_grant_is_said_out_loud_per_field() {
        let over_ceiling = Resources {
            cpu: Some(Quantity::Int(9000)),
            memory: Some(Quantity::Text("999999Gi".into())),
            disk: None,
        };
        let messages = captured_messages(|| {
            assert_eq!(
                resolve_size(
                    Some(&over_ceiling),
                    &ResourceOverrides::default(),
                    DEFAULT_VM_SIZE,
                    None,
                ),
                DEFAULT_VM_SIZE
            );
        });
        for field in ["resources.cpu", "resources.memory"] {
            assert!(
                messages
                    .iter()
                    .any(|m| m.contains(&format!("{field} is not a size this host can grant"))),
                "an ignored {field} must not be silent; got: {messages:?}"
            );
        }
    }

    #[test]
    fn a_size_the_host_grants_says_nothing() {
        let honoured = Resources {
            cpu: Some(Quantity::Int(3)),
            memory: Some(Quantity::Text("6Gi".into())),
            disk: None,
        };
        let messages = captured_messages(|| {
            assert_eq!(
                resolve_size(
                    Some(&honoured),
                    &ResourceOverrides::default(),
                    DEFAULT_VM_SIZE,
                    None,
                ),
                VmSize {
                    cpus: 3,
                    mem_mib: 6144
                }
            );
        });
        assert!(messages.is_empty(), "got: {messages:?}");
    }
}
