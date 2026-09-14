#[derive(Debug, PartialEq, Eq)]
pub enum Callback {
    Ignore,
    Denied,
    Code(String),
}

pub fn authorization_matches(url: &str, redirect_uri: &str, state: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else {
        return false;
    };
    let pairs: Vec<_> = url.query_pairs().collect();
    let value = |key: &str| {
        let mut matches = pairs.iter().filter(|(name, _)| name == key);
        let (_, value) = matches.next()?;
        matches.next().is_none().then_some(value.as_ref())
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && value("redirect_uri") == Some(redirect_uri)
        && value("state") == Some(state)
        && !state.is_empty()
        && value("response_type") == Some("code")
        && value("code_challenge_method") == Some("S256")
        && value("code_challenge").is_some_and(|v| {
            v.len() == 43
                && v.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        })
}

pub fn callback(request: &str, expected_state: &str) -> Callback {
    let Some(line) = request.lines().next() else {
        return Callback::Ignore;
    };
    let parts: Vec<_> = line.split_whitespace().collect();
    if parts.len() != 3 || parts[0] != "GET" || !parts[2].starts_with("HTTP/1.") {
        return Callback::Ignore;
    }
    let Some(query) = parts[1].strip_prefix("/callback?") else {
        return Callback::Ignore;
    };
    let pairs: Vec<_> = form_urlencoded::parse(query.as_bytes()).collect();
    if ["state", "code", "error"]
        .iter()
        .any(|key| pairs.iter().filter(|(name, _)| name == key).count() > 1)
    {
        return Callback::Ignore;
    }
    let value = |key: &str| {
        let mut matches = pairs.iter().filter(|(name, _)| name == key);
        let (_, value) = matches.next()?;
        matches.next().is_none().then_some(value.as_ref())
    };
    if expected_state.is_empty() || value("state") != Some(expected_state) {
        return Callback::Ignore;
    }
    match (value("code"), value("error")) {
        (Some(code), None) if !code.is_empty() => Callback::Code(code.to_string()),
        (None, Some(_)) => Callback::Denied,
        _ => Callback::Ignore,
    }
}

pub struct Flows<T> {
    entries: std::collections::BTreeMap<String, Flow<T>>,
}

struct Flow<T> {
    connector: String,
    redirect_uri: String,
    state: String,
    expires: u64,
    listener: Option<T>,
    result: Option<Result<String, super::CallError>>,
}

impl<T> Default for Flows<T> {
    fn default() -> Self {
        Self {
            entries: Default::default(),
        }
    }
}

impl<T> Flows<T> {
    pub fn capacity(&mut self, connector: &str, now: u64) -> Result<(), super::CallError> {
        self.entries.retain(|_, flow| flow.expires > now);
        if self.entries.len() >= 8
            || self
                .entries
                .values()
                .filter(|flow| flow.connector == connector)
                .count()
                >= 2
        {
            Err(super::CallError::Refused("too many browser authorizations are already open; finish one or wait for it to expire".into()))
        } else {
            Ok(())
        }
    }

    pub fn insert(
        &mut self,
        connector: &str,
        session: &super::traits::BrowserSession,
        listener: T,
        expires: u64,
    ) {
        self.entries.insert(
            session.handle.clone(),
            Flow {
                connector: connector.into(),
                redirect_uri: session.redirect_uri.clone(),
                state: session.state.clone(),
                expires,
                listener: Some(listener),
                result: None,
            },
        );
    }

    pub fn open(
        &mut self,
        connector: &str,
        handle: &str,
        url: &str,
        now: u64,
    ) -> Result<(T, String, u64), super::CallError> {
        let flow = self.owned(connector, handle, now)?;
        if !authorization_matches(url, &flow.redirect_uri, &flow.state) {
            return Err(super::CallError::Refused(
                "browser authorization must retain its callback, state, and S256 PKCE challenge"
                    .into(),
            ));
        }
        let listener = flow.listener.take().ok_or_else(|| {
            super::CallError::Refused("that browser authorization was already opened".into())
        })?;
        Ok((listener, flow.state.clone(), flow.expires - now))
    }

    fn owned(
        &mut self,
        connector: &str,
        handle: &str,
        now: u64,
    ) -> Result<&mut Flow<T>, super::CallError> {
        self.entries
            .get_mut(handle)
            .filter(|flow| flow.connector == connector && flow.expires > now)
            .ok_or_else(|| {
                super::CallError::Refused("that browser authorization is no longer open".into())
            })
    }

    pub fn complete(&mut self, handle: &str, result: Result<String, super::CallError>) {
        if let Some(flow) = self.entries.get_mut(handle) {
            flow.result = Some(result);
        }
    }

    pub fn remove(&mut self, handle: &str) {
        self.entries.remove(handle);
    }

    pub fn poll(
        &mut self,
        connector: &str,
        handle: &str,
        now: u64,
    ) -> Result<Option<String>, super::CallError> {
        let flow = self.owned(connector, handle, now)?;
        let Some(result) = flow.result.take() else {
            return Ok(None);
        };
        self.entries.remove(handle);
        result.map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_matching_browser_callback_delivers_its_code() {
        assert_eq!(
            callback(
                "GET /callback?state=expected&code=authorization-code HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
                "expected"
            ),
            Callback::Code("authorization-code".into())
        );
    }

    #[test]
    fn unrelated_requests_cannot_complete_a_sign_in() {
        for request in [
            "",
            "GET",
            "GET /callback?state=expected&code=code&error=a&error=b HTTP/1.1",
            "GET /callback?state=expected&code=a&code=b&error=denied HTTP/1.1",
            "GET /callback?state=expected&code=code HTTP/2",
            "GET /callback?state=expected&code= HTTP/1.1",
            "GET /callback?state=expected HTTP/1.1",
            "GET /callback?state=expected&code=code&error=denied HTTP/1.1",
            "GET /callback?state=wrong&code=code HTTP/1.1\r\n\r\n",
            "GET /callback?code=code HTTP/1.1\r\n\r\n",
            "GET /favicon.ico?state=expected&code=code HTTP/1.1\r\n\r\n",
            "POST /callback?state=expected&code=code HTTP/1.1\r\n\r\n",
            "GET /callback?state=expected&state=wrong&code=code HTTP/1.1\r\n\r\n",
            "GET /callback?state=expected&code=first&code=second HTTP/1.1\r\n\r\n",
        ] {
            assert_eq!(callback(request, "expected"), Callback::Ignore);
        }
    }

    fn authorization_url() -> String {
        let mut url = reqwest::Url::parse("https://auth.example/authorize").unwrap();
        url.query_pairs_mut().extend_pairs([
            ("redirect_uri", "http://127.0.0.1:1234/callback"),
            ("state", "expected"),
            ("response_type", "code"),
            ("code_challenge_method", "S256"),
            (
                "code_challenge",
                "01234567890123456789012345678901234567-_abc",
            ),
        ]);
        url.to_string()
    }

    #[test]
    fn authorization_retains_the_host_owned_callback_and_state() {
        let url = authorization_url();
        assert!(authorization_matches(
            &url,
            "http://127.0.0.1:1234/callback",
            "expected"
        ));
        assert!(!authorization_matches(
            &url,
            "http://127.0.0.1:9999/callback",
            "expected"
        ));
        assert!(!authorization_matches(
            &url,
            "http://127.0.0.1:1234/callback",
            "other"
        ));
        assert!(!authorization_matches(
            &url,
            "http://127.0.0.1:1234/callback",
            ""
        ));
    }

    #[test]
    fn an_authorization_cannot_weaken_pkce_or_smuggle_ambiguous_parameters() {
        let url = authorization_url();
        for invalid in [
            "invalid".to_string(),
            "https://auth.example/authorize".to_string(),
            url.replace("https:", "http:"),
            url.replace("auth.example", "user@auth.example"),
            url.replace("auth.example", "user:password@auth.example"),
            format!("{url}#fragment"),
            url.replace("S256", "plain"),
            url.replace("response_type=code", "response_type=token"),
            url.replace("01234567890123456789012345678901234567-_abc", "short"),
            url.replace("-_abc", "%2Babc!"),
            format!("{url}&state=other"),
            format!("{url}&redirect_uri=http://evil.example"),
        ] {
            assert!(
                !authorization_matches(&invalid, "http://127.0.0.1:1234/callback", "expected"),
                "{invalid}"
            );
        }
    }

    #[test]
    fn denial_is_only_accepted_for_the_expected_state() {
        assert_eq!(
            callback(
                "GET /callback?state=expected&error=access_denied HTTP/1.1\r\n\r\n",
                "expected"
            ),
            Callback::Denied
        );
        assert_eq!(
            callback(
                "GET /callback?state=wrong&error=access_denied HTTP/1.1\r\n\r\n",
                "expected"
            ),
            Callback::Ignore
        );
    }
    fn registered() -> Flows<()> {
        let mut flows = Flows::default();
        flows.insert(
            "linear",
            &super::super::traits::BrowserSession {
                handle: "handle".into(),
                redirect_uri: "http://127.0.0.1:1234/callback".into(),
                state: "expected".into(),
            },
            (),
            1000,
        );
        flows
    }

    #[test]
    fn only_the_owner_can_open_and_consume_a_live_callback() {
        let mut flows = registered();
        assert!(
            flows
                .open("other", "handle", &authorization_url(), 0)
                .is_err()
        );
        assert!(
            flows
                .open("linear", "handle", "https://auth.example", 0)
                .is_err()
        );
        assert_eq!(
            flows
                .open("linear", "handle", &authorization_url(), 0)
                .unwrap(),
            ((), "expected".into(), 1000)
        );
        assert!(
            flows
                .open("linear", "handle", &authorization_url(), 0)
                .is_err()
        );
        assert!(flows.poll("other", "handle", 0).is_err());
        assert_eq!(flows.poll("linear", "handle", 0).unwrap(), None);
        flows.complete("handle", Ok("code".into()));
        assert!(flows.poll("other", "handle", 0).is_err());
        assert_eq!(
            flows.poll("linear", "handle", 0).unwrap(),
            Some("code".into())
        );
        assert!(flows.poll("linear", "handle", 0).is_err());
    }

    #[test]
    fn expired_and_removed_callbacks_cannot_deliver_a_code() {
        let mut flows = registered();
        flows.complete("handle", Ok("code".into()));
        assert!(flows.poll("linear", "handle", 1000).is_err());
        assert!(
            flows
                .open("linear", "handle", &authorization_url(), 1000)
                .is_err()
        );
        flows.remove("handle");
        flows.complete("handle", Ok("late-code".into()));
        assert!(flows.poll("linear", "handle", 0).is_err());
    }

    #[test]
    fn a_denial_is_consumed_once() {
        let mut flows = registered();
        flows.complete(
            "handle",
            Err(super::super::CallError::Failed("declined".into())),
        );
        assert!(
            matches!(flows.poll("linear", "handle", 0), Err(super::super::CallError::Failed(message)) if message == "declined")
        );
        assert!(flows.poll("linear", "handle", 0).is_err());
    }

    #[test]
    fn browser_sessions_have_per_connector_and_global_limits() {
        let mut flows = registered();
        assert!(flows.capacity("linear", 0).is_ok());
        for index in 0..7 {
            let owner = if index == 0 {
                "linear".into()
            } else {
                format!("provider-{index}")
            };
            assert!(flows.capacity(&owner, 0).is_ok());
            flows.insert(
                &owner,
                &super::super::traits::BrowserSession {
                    handle: format!("handle-{index}"),
                    redirect_uri: "callback".into(),
                    state: "state".into(),
                },
                (),
                1000,
            );
            assert!(flows.capacity("linear", 0).is_err());
        }
        assert!(flows.capacity("another", 0).is_err());
        assert!(flows.capacity("linear", 1000).is_ok());
    }
}
