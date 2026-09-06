//! Every bound here is decided from the method that declared the mechanism and never from the mechanism, so a component cannot widen its own reach.

use std::sync::Arc;

use super::traits::{Entropy, Exec, Http, Recorder};
use super::{Bounds, CallError, ExecOutput, HttpRequest, HttpResponse};

/// The most entropy one call may draw. A component needs a PKCE verifier or a state parameter, not a keystream.
const MAX_ENTROPY_BYTES: u32 = 1024;

/// Cheap to clone, because a component runtime needs one per call.
#[derive(Clone)]
pub struct Host {
    connector: String,
    bounds: Bounds,
    http: Arc<dyn Http>,
    exec: Arc<dyn Exec>,
    entropy: Arc<dyn Entropy>,
    recorder: Arc<dyn Recorder>,
}

impl Host {
    pub fn new(
        connector: &str,
        bounds: Bounds,
        http: Arc<dyn Http>,
        exec: Arc<dyn Exec>,
        entropy: Arc<dyn Entropy>,
        recorder: Arc<dyn Recorder>,
    ) -> Self {
        Self {
            connector: connector.to_string(),
            bounds,
            http,
            exec,
            entropy,
            recorder,
        }
    }

    pub fn bounds(&self) -> &Bounds {
        &self.bounds
    }

    /// Reach one host the method declared, over TLS. Anything else is refused before the request is built, so nothing leaves the machine (§3.2.6).
    pub fn fetch(&self, request: &HttpRequest) -> Result<HttpResponse, CallError> {
        let target = Target::of(&request.url).ok_or_else(|| {
            CallError::Refused(format!("{} names no host lns can read", request.url))
        })?;
        if !target.is_tls {
            self.recorder.reached(&self.connector, target.host, true);
            return Err(CallError::Refused(format!(
                "{} is not https: lns carries a mechanism's calls over TLS or not at all",
                request.url
            )));
        }
        if !self.bounds.allows(target.host, target.port) {
            self.recorder.reached(&self.connector, target.host, true);
            return Err(CallError::Refused(format!(
                "{} is not among the hosts this method declares",
                target.host
            )));
        }
        self.recorder.reached(&self.connector, target.host, false);
        self.http.fetch(request)
    }

    /// Run a program on the machine with the user's own access. Available only where the method declared it (§3.2.6).
    pub fn run(&self, argv: &[String]) -> Result<ExecOutput, CallError> {
        let program = argv
            .first()
            .ok_or_else(|| CallError::Refused("a program to run was not named".to_string()))?;
        if !self.bounds.exec {
            self.recorder.ran(&self.connector, program, true);
            return Err(CallError::Refused(
                "this method does not declare host execution".to_string(),
            ));
        }
        self.recorder.ran(&self.connector, program, false);
        self.exec.run(argv)
    }

    pub fn bytes(&self, count: u32) -> Vec<u8> {
        self.entropy.bytes(count.min(MAX_ENTROPY_BYTES))
    }
}

/// The port an https URL reaches when it names none.
const HTTPS: &str = "443";

/// What one URL reaches, as the bound reads it: no userinfo, and the port it lands on whether or not the URL spelled it out.
struct Target<'a> {
    is_tls: bool,
    host: &'a str,
    port: &'a str,
}

impl<'a> Target<'a> {
    /// `None` where there is nothing lns could match a bound against, which is a refusal rather than a default.
    fn of(url: &'a str) -> Option<Self> {
        let (scheme, rest) = url.split_once("://")?;
        let authority = rest
            .split(['/', '?', '#'])
            .next()
            .filter(|part| !part.is_empty())?;
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let (host, port) = lns_policy::matching::split_destination(host_port);
        (!host.is_empty()).then_some(Self {
            is_tls: scheme.eq_ignore_ascii_case("https"),
            host,
            port: port.unwrap_or(HTTPS),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_is_read_as_the_host_and_port_it_reaches_and_never_as_its_userinfo() {
        let target = Target::of("https://user:pw@auth.example.com:8443/a?b#c").expect("a target");
        assert_eq!(
            (target.host, target.port, target.is_tls),
            ("auth.example.com", "8443", true)
        );
    }

    #[test]
    fn an_https_url_naming_no_port_reaches_the_one_https_means() {
        let target = Target::of("HTTPS://auth.example.com/token").expect("a target");
        assert_eq!(
            (target.host, target.port, target.is_tls),
            ("auth.example.com", "443", true)
        );
    }

    #[test]
    fn a_url_naming_no_host_is_read_as_none_rather_than_as_the_empty_bound() {
        for url in [
            "auth.example.com/token",
            "https:///token",
            "https://:443/token",
        ] {
            assert!(Target::of(url).is_none(), "{url}");
        }
    }
}
