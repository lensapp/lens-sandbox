use anyhow::Result;

#[derive(Clone, PartialEq, Eq)]
pub enum Callback {
    Ignore,
    Denied,
    Failed,
    UnsupportedIssuer,
    Code(String),
}

impl std::fmt::Debug for Callback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Ignore => "Ignore",
            Self::Denied => "Denied",
            Self::Failed => "Failed",
            Self::UnsupportedIssuer => "UnsupportedIssuer",
            Self::Code(_) => "Code(<redacted>)",
        })
    }
}

pub struct Lease {
    owner: String,
    path: String,
    state: String,
    expires: u64,
    consumed: bool,
}

impl Lease {
    pub fn new(owner: &str, path: &str, state: &str, expires: u64) -> Self {
        Self {
            owner: owner.into(),
            path: path.into(),
            state: state.into(),
            expires,
            consumed: false,
        }
    }
    pub fn live(&self, owner: &str, now: u64) -> Result<()> {
        if self.owner != owner || self.expired(now) || self.consumed {
            anyhow::bail!("OAuth callback is no longer open");
        }
        Ok(())
    }
    pub fn accept(&mut self, owner: &str, now: u64, request: &str) -> Result<Callback> {
        self.live(owner, now)?;
        let result = callback(request, &self.path, &self.state);
        self.consumed = !matches!(result, Callback::Ignore);
        Ok(result)
    }
    pub fn expired(&self, now: u64) -> bool {
        self.expires <= now
    }
}

pub trait Browser: Send + Sync {
    fn prepare(
        &self,
        owner: &str,
        path: &str,
        port: Option<u16>,
        state: &str,
        expires: u64,
    ) -> Result<(String, String)>;
    fn open(&self, url: &str) -> Result<()>;
    fn poll(&self, owner: &str, handle: &str, now: u64) -> Result<Callback>;
    fn cancel(&self, handle: &str);
    fn sweep(&self, now: u64);
}

pub fn callback(request: &str, path: &str, state: &str) -> Callback {
    if request.len() > 8192 || state.is_empty() {
        return Callback::Ignore;
    }
    let Some(line) = request.lines().next() else {
        return Callback::Ignore;
    };
    let parts: Vec<_> = line.split_whitespace().collect();
    if parts.len() != 3 || parts[0] != "GET" || !matches!(parts[2], "HTTP/1.0" | "HTTP/1.1") {
        return Callback::Ignore;
    }
    let Some((received_path, query)) = parts[1].split_once('?') else {
        return Callback::Ignore;
    };
    if received_path != path || query.contains('#') {
        return Callback::Ignore;
    }
    let mut pairs = std::collections::BTreeMap::new();
    for (name, value) in form_urlencoded::parse(query.as_bytes()) {
        if pairs.insert(name, value).is_some() {
            return Callback::Ignore;
        }
    }
    let value = |key: &str| pairs.get(key).map(|v| v.as_ref());
    if value("state") != Some(state) {
        return Callback::Ignore;
    }
    if value("iss").is_some() {
        return Callback::UnsupportedIssuer;
    }
    match (value("code"), value("error")) {
        (Some(code), None)
            if !code.is_empty() && code.bytes().all(|b| (0x20..=0x7e).contains(&b)) =>
        {
            Callback::Code(code.into())
        }
        (None, Some("access_denied")) => Callback::Denied,
        (
            None,
            Some(
                "invalid_request"
                | "unauthorized_client"
                | "unsupported_response_type"
                | "invalid_scope"
                | "server_error"
                | "temporarily_unavailable",
            ),
        ) => Callback::Failed,
        _ => Callback::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_matching_issuer_callback_fails_immediately_and_consumes_its_lease() {
        let mut lease = Lease::new("owner", "/callback", "expected", 100);
        let result = lease
            .accept(
                "owner",
                0,
                "GET /callback?state=expected&code=private&iss=https://issuer.example HTTP/1.1",
            )
            .unwrap();
        assert_eq!(result, Callback::UnsupportedIssuer);
        assert!(!format!("{result:?}").contains("issuer.example"));
        assert!(lease.live("owner", 1).is_err());
    }

    #[test]
    fn an_authorization_server_error_is_terminal_only_for_its_matching_state() {
        for error in [
            "invalid_request",
            "unauthorized_client",
            "unsupported_response_type",
            "invalid_scope",
            "server_error",
            "temporarily_unavailable",
        ] {
            assert_eq!(
                callback(
                    &format!("GET /callback?state=expected&error={error} HTTP/1.1"),
                    "/callback",
                    "expected"
                ),
                Callback::Failed
            );
        }
        assert_eq!(
            callback(
                "GET /callback?state=wrong&error=server_error HTTP/1.1",
                "/callback",
                "expected"
            ),
            Callback::Ignore
        );
    }

    #[test]
    fn callback_lease_rejects_wrong_owner_deadline_replay_and_cross_operation_codes() {
        let request = "GET /callback?state=a&code=private HTTP/1.1";
        let mut first = Lease::new("owner-a", "/callback", "a", 100);
        let mut second = Lease::new("owner-b", "/callback", "b", 100);
        assert!(first.accept("owner-b", 0, request).is_err());
        assert_eq!(
            second.accept("owner-b", 0, request).unwrap(),
            Callback::Ignore
        );
        assert_eq!(
            first.accept("owner-a", 99, request).unwrap(),
            Callback::Code("private".into())
        );
        assert!(first.accept("owner-a", 99, request).is_err());
        assert!(second.accept("owner-b", 100, request).is_err());
        assert!(!second.expired(99));
        assert!(second.expired(100));
    }

    #[test]
    fn callback_requires_exact_path_state_and_unambiguous_code() {
        assert_eq!(
            callback(
                "GET /oauth/callback?state=expected&code=private HTTP/1.1\r\n\r\n",
                "/oauth/callback",
                "expected"
            ),
            Callback::Code("private".into())
        );
        assert_eq!(
            callback(
                "GET /callback?state=expected&error=access_denied HTTP/1.1",
                "/callback",
                "expected"
            ),
            Callback::Denied
        );
        for request in [
            "GET /callback?state=wrong&code=a HTTP/1.1",
            "POST /callback?state=expected&code=a HTTP/1.1",
            "GET /wrong?state=expected&code=a HTTP/1.1",
            "GET /callback?state=expected&state=expected&code=a HTTP/1.1",
            "GET /callback?state=expected&code=a&code=b HTTP/1.1",
            "GET /callback?state=expected&code=a&error=x HTTP/1.1",
            "GET /callback?state=expected&error=access_denied&error=x HTTP/1.1",
            "GET /callback?state=expected&code= HTTP/1.1",
            "GET /callback?state=wrong&code=a&iss=https://other.example HTTP/1.1",
            "GET /callback?state=expected&code=a&extra=1&extra=2 HTTP/1.1",
            "",
            "GET /callback HTTP/1.1",
            "GET /callback?state=expected&code=a HTTP/2",
            "GET /callback?state=expected&code=a HTTP/1.1 extra",
        ] {
            assert_eq!(
                callback(request, "/callback", "expected"),
                Callback::Ignore,
                "{request}"
            );
        }
        assert_eq!(
            callback("GET /callback?state=&code=a HTTP/1.1", "/callback", ""),
            Callback::Ignore
        );
        for (value, rendered) in [
            (Callback::Ignore, "Ignore"),
            (Callback::Denied, "Denied"),
            (Callback::Failed, "Failed"),
            (Callback::Code("private".into()), "Code(<redacted>)"),
        ] {
            assert_eq!(format!("{value:?}"), rendered);
        }
    }
}
