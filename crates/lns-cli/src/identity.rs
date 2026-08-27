use lns_ipc::{Identity, Method};

const LNS_VERSION: &str = env!("CARGO_PKG_VERSION");

fn identity() -> &'static Identity {
    static ID: std::sync::OnceLock<Identity> = std::sync::OnceLock::new();
    ID.get_or_init(|| Identity::new(LNS_VERSION, &crate::platform::detect()))
}

/// The `User-Agent` every request this CLI makes must carry, so a registry sees `lns`, never the HTTP library underneath.
pub fn header(method: Method) -> &'static str {
    identity().header(method)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_header_names_this_cli_version_and_the_traffic_it_belongs_to() {
        for method in Method::ALL {
            let header = header(method);
            assert!(
                header.starts_with(&format!("{}/{LNS_VERSION} (os=", method.product())),
                "{header}"
            );
            assert!(
                header.ends_with(&format!("method={})", method.as_str())),
                "{header}"
            );
        }
    }

    #[test]
    fn a_header_is_built_once_and_handed_out_by_reference() {
        assert!(std::ptr::eq(
            header(Method::RegistryPush),
            header(Method::RegistryPush)
        ));
    }
}
