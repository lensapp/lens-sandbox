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
        let target = Target::of(&request.url).map_err(CallError::Refused)?;
        if !target.is_tls() {
            self.recorder.reached(&self.connector, &target.host, true);
            return Err(CallError::Refused(format!(
                "{} is not https: lns carries a mechanism's calls over TLS or not at all",
                request.url
            )));
        }
        if !self.bounds.allows(&target.host, target.port) {
            self.recorder.reached(&self.connector, &target.host, true);
            return Err(CallError::Refused(format!(
                "{} is not among the hosts this method declares",
                target.host
            )));
        }
        self.recorder.reached(&self.connector, &target.host, false);
        self.http.fetch(
            &HttpRequest {
                // The parse the bound was decided from, so no spelling can be read one way here and another on the wire.
                url: target.sending.to_string(),
                ..request.clone()
            },
            self.within(),
        )
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
        self.exec.run(argv, self.within())
    }

    /// How long one call has. The runtime's epoch cannot interrupt a host call already in flight, so the deadline travels with it (§3.2.6).
    fn within(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.bounds.call_seconds))
    }

    pub fn bytes(&self, count: u32) -> Vec<u8> {
        self.entropy.bytes(count.min(MAX_ENTROPY_BYTES))
    }
}

/// What one URL reaches, read by the parser that will send it: a bound decided from a second reading of the same string is one a spelling can be dressed past.
struct Target {
    sending: reqwest::Url,
    /// Spelled the way the `match` grammar spells a host, so an address literal is bare rather than bracketed and the two sides of a bound can be compared at all.
    host: String,
    port: u16,
}

impl Target {
    /// A bound holds against a host and a port, so anything lns cannot read one of is refused — and told apart, because "no port" is not a thing an author can act on by rereading the host.
    fn of(url: &str) -> Result<Self, String> {
        let sending = reqwest::Url::parse(url)
            .map_err(|e| format!("{url} is not a URL lns can read: {e}"))?;
        let host = sending
            .host_str()
            .ok_or_else(|| format!("{url} names no host lns can read"))?;
        let port = sending.port_or_known_default().ok_or_else(|| {
            format!(
                "{url} names no port, and lns cannot work out the one {}:// lands on",
                sending.scheme()
            )
        })?;
        Ok(Self {
            host: lns_policy::matching::unbracketed(host).to_string(),
            port,
            sending,
        })
    }

    fn is_tls(&self) -> bool {
        self.sending.scheme() == "https"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_as(target: &Target) -> (&str, u16, bool) {
        (&target.host, target.port, target.is_tls())
    }

    /// A `Target` that came back is reported as a refusal that says so, rather than through an arm that only runs when the test fails.
    fn refused(url: &str) -> String {
        Target::of(url)
            .err()
            .unwrap_or_else(|| format!("{url} was read as a target lns would reach"))
    }

    #[test]
    fn a_url_is_read_as_the_host_and_port_it_reaches_and_never_as_its_userinfo() {
        let target = Target::of("https://user:pw@auth.example.com:8443/a?b#c").expect("a target");
        assert_eq!(read_as(&target), ("auth.example.com", 8443, true));
    }

    #[test]
    fn an_https_url_naming_no_port_reaches_the_one_https_means() {
        let target = Target::of("HTTPS://auth.example.com/token").expect("a target");
        assert_eq!(read_as(&target), ("auth.example.com", 443, true));
    }

    #[test]
    fn a_backslash_ends_an_authority_here_exactly_as_it_does_on_the_wire() {
        // The one spelling a second reading of the string gets wrong: `rsplit('@')` reads the host after it, and the sender reads the host before.
        let target =
            Target::of(r"https://evil.example.com\@auth.example.com/token").expect("a target");
        assert_eq!(read_as(&target), ("evil.example.com", 443, true));
    }

    #[test]
    fn a_url_naming_no_host_is_refused_rather_than_read_as_the_empty_bound() {
        for (url, said) in [
            ("auth.example.com/token", "not a URL"),
            ("https://:443/token", "not a URL"),
            ("data:text/plain,x", "names no host"),
        ] {
            let refusal = refused(url);
            assert!(refusal.contains(said), "{url}: {refusal}");
        }
    }

    #[test]
    fn a_scheme_with_no_port_to_land_on_is_refused_rather_than_given_one() {
        // A bound holds against a host and a port, so a scheme lns cannot work a port out for is one it cannot hold a bound against — and the refusal says which half it could not read.
        let refusal = refused("foo://auth.example.com/token");
        assert!(refusal.contains("names no port"), "{refusal}");
        assert!(refusal.contains("foo://"), "{refusal}");
    }

    #[test]
    fn a_url_whose_host_is_only_a_path_segment_to_read_is_still_read_the_way_it_is_sent() {
        // `https:///token` reaches a host named `token`, so the bound is held against that and refuses it — a reader that called this hostless would disagree with the sender.
        let target = Target::of("https:///token").expect("the sender reads a host here");
        assert_eq!(read_as(&target), ("token", 443, true));
    }
}
