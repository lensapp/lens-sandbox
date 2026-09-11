use super::http::request;
use super::report_server::Health;
use super::{
    Activity, FixtureReport, Fixtures, HARNESS_VERSION, Role, guest_destinations,
    refuse_unsuitable_bind, report_port,
};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Mutex;

/// Keeps the witness's own connections clear of the ids the remote hands out, so a mark from one side never counts a record from the other.
const WITNESS_ID_BASE: u64 = 1 << 32;

/// The fixtures on another machine, read over the report server's HTTP routes; the witness stays here, because it has to bind this host's loopback.
pub struct RemoteFixtures {
    endpoint: SocketAddrV4,
    bind: Ipv4Addr,
    ports: BTreeMap<String, u16>,
    version: String,
    witness: Fixtures,
    last: Mutex<FixtureReport>,
}

impl RemoteFixtures {
    pub fn connect(fixtures_at: SocketAddrV4) -> Result<Self> {
        let bind = *fixtures_at.ip();
        refuse_unsuitable_bind(bind)?;
        Self::open(
            SocketAddrV4::new(bind, report_port(fixtures_at.port())?),
            bind,
            fixtures_at.port(),
        )
    }

    fn open(endpoint: SocketAddrV4, bind: Ipv4Addr, witness_base_port: u16) -> Result<Self> {
        let health = health(endpoint)?;
        if health.harness_version != HARNESS_VERSION {
            bail!(
                "the fixtures at {endpoint} run harness version {}, this runner is {HARNESS_VERSION}; build the same revision on both machines",
                health.harness_version
            );
        }

        let witness = Fixtures::start_witness(witness_base_port)?;
        let mut ports = health.ports;
        ports.insert(
            Role::Witness.as_str().to_string(),
            witness.port(Role::Witness),
        );
        for role in Role::ALL {
            if !ports.contains_key(role.as_str()) {
                bail!(
                    "the fixtures at {endpoint} serve no {} fixture; they are another fixture set",
                    role.as_str()
                );
            }
        }

        Ok(Self {
            endpoint,
            bind,
            ports,
            version: health.harness_version,
            witness,
            last: Mutex::new(FixtureReport::default()),
        })
    }

    pub fn endpoint(&self) -> SocketAddrV4 {
        self.endpoint
    }

    pub fn bind(&self) -> Ipv4Addr {
        self.bind
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn port(&self, role: Role) -> u16 {
        self.ports[role.as_str()]
    }

    pub fn guest_destinations(&self) -> Vec<String> {
        guest_destinations(self.bind, &self.ports)
    }

    /// The remote's report with this host's witness folded in; a fetch that fails keeps the last one, and the next `reset` names the failure.
    pub fn report(&self) -> FixtureReport {
        match fetch_report(self.endpoint) {
            Ok(report) => {
                let merged = self.merge(report);
                self.remember(merged.clone());
                merged
            }
            Err(_) => self.remembered(),
        }
    }

    pub fn activity_since(&self, mark: u64) -> Activity {
        self.report().activity_since(mark)
    }

    pub fn reset(&self) -> Result<()> {
        self.witness.reset();
        let (status, body) = request(self.endpoint, "POST", "/reset")
            .with_context(|| format!("reset the fixtures at {}", self.endpoint))?;
        if status != 200 {
            bail!(
                "the fixtures at {} answered {status} to a reset: {body}",
                self.endpoint
            );
        }
        self.remember(FixtureReport::default());
        Ok(())
    }

    fn merge(&self, mut remote: FixtureReport) -> FixtureReport {
        let witness = self.witness.report();
        remote.bind = self.bind.to_string();
        remote.ports = self.ports.clone();
        remote.witness_accepts += witness.witness_accepts;
        remote
            .connections
            .extend(witness.connections.into_iter().map(|mut record| {
                record.id += WITNESS_ID_BASE;
                record
            }));
        remote
    }

    fn remember(&self, report: FixtureReport) {
        let mut guard = match self.last.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = report;
    }

    fn remembered(&self) -> FixtureReport {
        match self.last.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

pub fn health(endpoint: SocketAddrV4) -> Result<Health> {
    let (status, body) = request(endpoint, "GET", "/health")
        .with_context(|| format!("ask the fixtures at {endpoint} for their health"))?;
    if status != 200 {
        bail!("the fixtures at {endpoint} answered {status} to /health: {body}");
    }
    serde_json::from_str(&body)
        .with_context(|| format!("{endpoint} answers, but not as this harness's report server"))
}

fn fetch_report(endpoint: SocketAddrV4) -> Result<FixtureReport> {
    let (status, body) = request(endpoint, "GET", "/report")?;
    if status != 200 {
        bail!("the fixtures at {endpoint} answered {status} to /report");
    }
    Ok(serde_json::from_str(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::Sizes;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    fn served() -> (Fixtures, SocketAddrV4) {
        let fixtures = Fixtures::start(Ipv4Addr::LOCALHOST, 0, Sizes::default())
            .expect("the fixtures bind on loopback");
        let port = fixtures.serve_report(0).expect("the report server binds");
        (fixtures, SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
    }

    #[test]
    fn health_is_read_from_the_report_server_it_names() {
        let (_fixtures, endpoint) = served();
        let health = health(endpoint).unwrap();

        assert_eq!(health.harness_version, HARNESS_VERSION);
        assert!(health.ports.contains_key("sink"), "{health:?}");
    }

    /// A server that answers every route with one canned body, so a health a real fixture process would never serve can still be put to the runner.
    fn canned(health_body: String) -> SocketAddrV4 {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = match listener.local_addr().unwrap() {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(addr) => panic!("loopback bound as {addr}"),
        };
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut head = [0u8; 1024];
                let _ = stream.read(&mut head);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{health_body}",
                    health_body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        endpoint
    }

    fn remote(endpoint: SocketAddrV4) -> Result<RemoteFixtures> {
        RemoteFixtures::open(endpoint, Ipv4Addr::LOCALHOST, 0)
    }

    fn refused(opened: Result<RemoteFixtures>) -> String {
        match opened {
            Ok(_) => panic!("these fixtures must be refused"),
            Err(err) => format!("{err:#}"),
        }
    }

    #[test]
    fn the_runner_reads_the_remote_report_and_resets_it_between_two_cases() {
        let (fixtures, endpoint) = served();
        let remote = remote(endpoint).expect("the runner reaches the fixtures");

        assert_eq!(remote.port(Role::Sink), fixtures.port(Role::Sink));
        assert_eq!(remote.version(), HARNESS_VERSION);
        assert_eq!(remote.guest_destinations().len(), 8);

        remote.reset().expect("the case starts on cleared counters");
        let mut client = TcpStream::connect((fixtures.bind(), fixtures.port(Role::Sink))).unwrap();
        client.write_all(&[0u8; 256]).unwrap();
        drop(client);
        let activity = crate::cases::poll(Duration::from_secs(5), || {
            let activity = remote.activity_since(0);
            (activity.bytes_in == 256).then_some(activity)
        })
        .expect("the runner sees what the remote fixture read");
        assert_eq!(activity.connections, 1);

        remote.reset().expect("the next case starts cleared");
        assert_eq!(remote.report().connections.len(), 0);
        assert_eq!(remote.activity_since(0), Activity::default());
    }

    #[test]
    fn the_remote_report_carries_this_hosts_own_witness() {
        let (_fixtures, endpoint) = served();
        let remote = remote(endpoint).unwrap();
        let witness_port = remote.port(Role::Witness);

        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, witness_port)).unwrap();
        let mut body = String::new();
        client.read_to_string(&mut body).unwrap();

        let accepts = crate::cases::poll(Duration::from_secs(5), || {
            let report = remote.report();
            (report.witness_accepts == 1).then_some(report.witness_accepts)
        })
        .expect("the witness on this host is folded into the remote report");
        assert_eq!(accepts, 1);
        assert!(
            !remote
                .guest_destinations()
                .iter()
                .any(|destination| destination.ends_with(&witness_port.to_string())),
            "the witness is never a destination the definition grants"
        );
    }

    #[test]
    fn fixtures_built_from_another_revision_are_refused_before_a_case_runs() {
        let endpoint = canned(
            "{\"harness_version\":\"0.24.0\",\"bind\":\"192.168.1.77\",\"ports\":{},\"fixtures\":[]}"
                .to_string(),
        );
        let err = refused(remote(endpoint));

        assert!(err.contains("0.24.0"), "{err}");
        assert!(err.contains(HARNESS_VERSION), "{err}");
        assert!(err.contains("the same revision on both machines"), "{err}");
    }

    #[test]
    fn a_server_that_answers_something_else_is_not_taken_for_the_fixtures() {
        let endpoint = canned("<html>hello</html>".to_string());
        let err = refused(remote(endpoint));
        assert!(err.contains("not as this harness's report server"), "{err}");
    }

    #[test]
    fn a_fixture_set_that_is_missing_a_fixture_is_refused() {
        let endpoint = canned(format!(
            "{{\"harness_version\":\"{HARNESS_VERSION}\",\"bind\":\"192.168.1.77\",\"ports\":{{\"sink\":47200}},\"fixtures\":[\"sink\"]}}"
        ));
        let err = refused(remote(endpoint));
        assert!(err.contains("another fixture set"), "{err}");
    }

    #[test]
    fn a_reset_the_fixtures_never_answered_is_reported_rather_than_passed_over() {
        let (fixtures, endpoint) = served();
        let remote = remote(endpoint).unwrap();
        let seen = remote.report();
        fixtures.shutdown();

        let err = crate::cases::poll(Duration::from_secs(5), || {
            remote.reset().err().map(|err| format!("{err:#}"))
        })
        .expect("a dead report server fails the case rather than reading as quiet");
        assert!(err.contains(&endpoint.to_string()), "{err}");
        assert_eq!(remote.report(), seen, "the last report stands");
    }

    #[test]
    fn a_remote_on_an_address_no_guest_reaches_is_refused() {
        let err = refused(RemoteFixtures::connect(SocketAddrV4::new(
            Ipv4Addr::LOCALHOST,
            47200,
        )));
        assert!(err.contains("loopback"), "{err}");
    }

    #[test]
    fn a_report_server_that_does_not_answer_is_named_in_the_error() {
        let endpoint = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1);
        let err = format!("{:#}", health(endpoint).unwrap_err());
        assert!(err.contains("127.0.0.1:1"), "{err}");
    }
}
