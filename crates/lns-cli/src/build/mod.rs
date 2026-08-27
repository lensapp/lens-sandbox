pub(crate) mod push;
pub(crate) mod push_auth;

use lns_ipc::Method;
use oci_client::client::ClientConfig;

/// Every registry client a push builds, so neither the transport nor the identity is left to the HTTP library's defaults.
pub(crate) fn push_client_config(registry: &str) -> ClientConfig {
    ClientConfig {
        protocol: lns_artifact::client_protocol_for(registry),
        user_agent: crate::identity::header(Method::RegistryPush),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oci_client::client::ClientProtocol;

    #[test]
    fn a_push_client_identifies_itself_as_lns_pushing() {
        let config = push_client_config("hub.lns.run");
        assert!(
            config.user_agent.starts_with("lns/"),
            "{}",
            config.user_agent
        );
        assert!(
            config.user_agent.ends_with("method=registry-push)"),
            "{}",
            config.user_agent
        );
    }

    #[test]
    fn a_push_client_keeps_the_registry_transport_rule() {
        assert!(matches!(
            push_client_config("localhost:5000").protocol,
            ClientProtocol::Http
        ));
        assert!(matches!(
            push_client_config("hub.lns.run").protocol,
            ClientProtocol::Https
        ));
    }
}
