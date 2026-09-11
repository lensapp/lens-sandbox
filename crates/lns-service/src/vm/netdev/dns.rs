use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use hickory_proto::op::{Message, MessageType, ResponseCode};

pub const PORT: u16 = 53;

/// How long one upstream has to answer before the next one is asked.
pub const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(5);

/// The host's resolver list is read again this often, so a VPN coming up mid-run is picked up.
pub const REFRESH_AFTER: Duration = Duration::from_secs(30);

/// A read of the host's configuration has this long, `scutil --dns` included, before the list in hand is kept instead.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(2);

/// One resolver of the host's configuration: which servers to ask, and the domain suffix they answer for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub suffix: Option<String>,
    pub servers: Vec<SocketAddr>,
}

/// How a query reaches an upstream. UDP first; a truncated answer is asked again over TCP to the same server.
pub trait Upstream: Send + Sync + 'static {
    fn over_udp(
        &self,
        server: SocketAddr,
        query: Vec<u8>,
    ) -> BoxFuture<'static, std::io::Result<Vec<u8>>>;
    fn over_tcp(
        &self,
        server: SocketAddr,
        query: Vec<u8>,
    ) -> BoxFuture<'static, std::io::Result<Vec<u8>>>;
}

/// Where the host's resolver list comes from. A refresh reads the whole list again.
pub trait Sources: Send + Sync + 'static {
    fn scopes(&self) -> Vec<Scope>;
}

/// The host's resolvers, re-read on a timer and whenever an answer could not be had.
pub struct Resolvers {
    sources: Arc<dyn Sources>,
    refresh_after: Duration,
    cached: Arc<Mutex<Cached>>,
    refreshing: Arc<AtomicBool>,
}

struct Cached {
    scopes: Vec<Scope>,
    read_at: Instant,
}

impl Resolvers {
    pub fn new(sources: Arc<dyn Sources>, refresh_after: Duration) -> Self {
        let scopes = sources.scopes();
        Self {
            sources,
            refresh_after,
            cached: Arc::new(Mutex::new(Cached {
                scopes,
                read_at: Instant::now(),
            })),
            refreshing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The servers that answer for `name`: the longest matching scope, or the default one.
    pub fn servers_for(&self, name: &str) -> Vec<SocketAddr> {
        let (servers, old) = {
            let cached = self.cached.lock().expect("resolvers poisoned");
            (
                servers_for(&cached.scopes, name),
                cached.read_at.elapsed() >= self.refresh_after,
            )
        };
        if old {
            self.refresh();
        }
        servers
    }

    /// One read at a time, off the runtime's workers: a query is answered from the list in hand, never from the read.
    fn refresh(&self) {
        if self.refreshing.swap(true, Ordering::SeqCst) {
            return;
        }
        let sources = Arc::clone(&self.sources);
        let cached = Arc::clone(&self.cached);
        let refreshing = Arc::clone(&self.refreshing);
        tokio::spawn(async move {
            let read = tokio::task::spawn_blocking(move || sources.scopes());
            if let Ok(Ok(scopes)) = tokio::time::timeout(REFRESH_TIMEOUT, read).await {
                cached.lock().expect("resolvers poisoned").scopes = scopes;
            }
            cached.lock().expect("resolvers poisoned").read_at = Instant::now();
            refreshing.store(false, Ordering::SeqCst);
        });
    }

    /// After a query nobody answered, the next one reads the host's configuration again.
    pub fn stale(&self) {
        let mut cached = self.cached.lock().expect("resolvers poisoned");
        cached.read_at = Instant::now() - self.refresh_after;
    }
}

pub fn servers_for(scopes: &[Scope], name: &str) -> Vec<SocketAddr> {
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    let matched = scopes
        .iter()
        .filter_map(|scope| scope.suffix.as_ref().map(|suffix| (suffix, scope)))
        .filter(|(suffix, _)| covers(suffix, &name))
        .max_by_key(|(suffix, _)| suffix.len());
    match matched {
        Some((_, scope)) => scope.servers.clone(),
        None => scopes
            .iter()
            .filter(|scope| scope.suffix.is_none())
            .flat_map(|scope| scope.servers.iter().copied())
            .collect(),
    }
}

fn covers(suffix: &str, name: &str) -> bool {
    let suffix = suffix.trim_end_matches('.').to_ascii_lowercase();
    name == suffix || name.ends_with(&format!(".{suffix}"))
}

/// The name the query asks about, or `None` for bytes that are not a question this gateway relays.
pub fn question_name(query: &[u8]) -> Option<String> {
    let message = Message::from_vec(query).ok()?;
    if message.metadata.message_type != MessageType::Query {
        return None;
    }
    Some(message.queries.first()?.name().to_string())
}

/// The guest's query, asked of each server in turn. Nobody answering is a SERVFAIL, never silence.
pub async fn relay(query: &[u8], servers: &[SocketAddr], upstream: &dyn Upstream) -> Vec<u8> {
    let mut refusal = None;
    for server in servers {
        match ask(query, *server, upstream).await {
            Some(answer) if answers(&answer) => return answer,
            Some(refused) => refusal = Some(refused),
            None => {}
        }
    }
    refusal.unwrap_or_else(|| servfail(query))
}

/// SERVFAIL, REFUSED and NOTIMP are a server saying "not me", so the next one is asked; NXDOMAIN is an answer.
fn answers(reply: &[u8]) -> bool {
    Message::from_vec(reply).is_ok_and(|message| {
        !matches!(
            message.metadata.response_code,
            ResponseCode::ServFail | ResponseCode::Refused | ResponseCode::NotImp
        )
    })
}

async fn ask(query: &[u8], server: SocketAddr, upstream: &dyn Upstream) -> Option<Vec<u8>> {
    let over_udp = upstream.over_udp(server, query.to_vec());
    let answer = tokio::time::timeout(UPSTREAM_TIMEOUT, over_udp)
        .await
        .ok()?
        .ok()?;
    if !truncated(&answer) {
        return Some(answer);
    }
    let over_tcp = upstream.over_tcp(server, query.to_vec());
    tokio::time::timeout(UPSTREAM_TIMEOUT, over_tcp)
        .await
        .ok()?
        .ok()
}

fn truncated(answer: &[u8]) -> bool {
    Message::from_vec(answer).is_ok_and(|message| message.metadata.truncation)
}

pub fn servfail(query: &[u8]) -> Vec<u8> {
    let Ok(request) = Message::from_vec(query) else {
        return Vec::new();
    };
    let mut reply = Message::new(
        request.metadata.id,
        MessageType::Response,
        request.metadata.op_code,
    );
    reply.metadata.recursion_desired = request.metadata.recursion_desired;
    reply.metadata.recursion_available = true;
    reply.metadata.response_code = ResponseCode::ServFail;
    reply.queries = request.queries;
    reply.to_vec().unwrap_or_default()
}

pub fn scopes_of_resolv_conf(contents: &str) -> Vec<Scope> {
    let servers: Vec<SocketAddr> = contents
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter_map(|line| line.strip_prefix("nameserver"))
        .filter_map(|rest| rest.split_whitespace().next())
        .filter_map(|address| address.parse::<IpAddr>().ok())
        .map(|address| SocketAddr::new(address, PORT))
        .collect();
    if servers.is_empty() {
        return Vec::new();
    }
    vec![Scope {
        suffix: None,
        servers,
    }]
}

/// `scutil --dns` is the only place a split-DNS VPN's per-domain resolvers appear; `/etc/resolv.conf` holds none of them.
pub fn scopes_of_scutil(output: &str) -> Vec<Scope> {
    let mut scopes = Vec::new();
    let mut current: Option<Resolver> = None;
    for line in output.lines() {
        let line = line.trim();
        if line.contains("(for scoped queries)") {
            break;
        }
        if line.starts_with("resolver #") {
            push_scope(&mut scopes, current.take());
            current = Some(Resolver::default());
            continue;
        }
        if let Some(resolver) = current.as_mut() {
            read_field(resolver, line);
        }
    }
    push_scope(&mut scopes, current);
    scopes
}

/// One `resolver #n` block being read: its port is a line of its own and belongs to every nameserver it names.
#[derive(Default)]
struct Resolver {
    suffix: Option<String>,
    addresses: Vec<IpAddr>,
    port: Option<u16>,
}

fn read_field(resolver: &mut Resolver, line: &str) {
    let Some((key, value)) = line.split_once(':') else {
        return;
    };
    let (key, value) = (key.trim(), value.trim());
    if key == "domain" {
        resolver.suffix = Some(value.to_string());
    } else if key == "port" {
        resolver.port = value.parse().ok();
    } else if key.starts_with("nameserver")
        && let Ok(address) = value.parse::<IpAddr>()
    {
        resolver.addresses.push(address);
    }
}

fn push_scope(scopes: &mut Vec<Scope>, resolver: Option<Resolver>) {
    // A loopback nameserver here is a host-side socket this service opens itself, so the guest boundary does not judge it.
    let Some(resolver) = resolver.filter(|resolver| !resolver.addresses.is_empty()) else {
        return;
    };
    let port = resolver.port.unwrap_or(PORT);
    scopes.push(Scope {
        suffix: resolver.suffix,
        servers: resolver
            .addresses
            .into_iter()
            .map(|address| SocketAddr::new(address, port))
            .collect(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::{OpCode, Query};
    use hickory_proto::rr::{Name, RecordType};
    use std::sync::atomic::AtomicUsize;

    fn server(address: &str) -> SocketAddr {
        SocketAddr::new(address.parse().unwrap(), PORT)
    }

    fn scope(suffix: Option<&str>, servers: &[&str]) -> Scope {
        Scope {
            suffix: suffix.map(str::to_string),
            servers: servers.iter().map(|a| server(a)).collect(),
        }
    }

    fn question(name: &str, kind: RecordType) -> Vec<u8> {
        let mut message = Message::new(0x4242, MessageType::Query, OpCode::Query);
        message.metadata.recursion_desired = true;
        message.add_query(Query::query(Name::from_ascii(name).unwrap(), kind));
        message.to_vec().unwrap()
    }

    fn answer(query: &[u8], truncated: bool) -> Vec<u8> {
        let request = Message::from_vec(query).unwrap();
        let mut reply = Message::new(request.metadata.id, MessageType::Response, OpCode::Query);
        reply.metadata.truncation = truncated;
        reply.queries = request.queries;
        reply.to_vec().unwrap()
    }

    /// An upstream's own reply carrying a response code, marked authoritative so a test can tell it from the gateway's own.
    fn says(query: &[u8], code: ResponseCode) -> Vec<u8> {
        let request = Message::from_vec(query).unwrap();
        let mut reply = Message::new(request.metadata.id, MessageType::Response, OpCode::Query);
        reply.metadata.response_code = code;
        reply.metadata.authoritative = true;
        reply.queries = request.queries;
        reply.to_vec().unwrap()
    }

    struct FakeUpstream {
        udp: Mutex<Vec<SocketAddr>>,
        tcp: Mutex<Vec<SocketAddr>>,
        answering: Option<SocketAddr>,
        saying: Vec<(SocketAddr, ResponseCode)>,
        truncate_udp: bool,
    }

    impl FakeUpstream {
        fn answering(server: SocketAddr) -> Self {
            Self {
                udp: Mutex::new(Vec::new()),
                tcp: Mutex::new(Vec::new()),
                answering: Some(server),
                saying: Vec::new(),
                truncate_udp: false,
            }
        }

        fn silent() -> Self {
            Self {
                udp: Mutex::new(Vec::new()),
                tcp: Mutex::new(Vec::new()),
                answering: None,
                saying: Vec::new(),
                truncate_udp: false,
            }
        }

        fn reply_of(&self, server: SocketAddr, query: &[u8], truncate: bool) -> Option<Vec<u8>> {
            if let Some((_, code)) = self.saying.iter().find(|(named, _)| *named == server) {
                return Some(says(query, *code));
            }
            (self.answering == Some(server)).then(|| answer(query, truncate))
        }
    }

    impl Upstream for FakeUpstream {
        fn over_udp(
            &self,
            server: SocketAddr,
            query: Vec<u8>,
        ) -> BoxFuture<'static, std::io::Result<Vec<u8>>> {
            self.udp.lock().unwrap().push(server);
            let reply = self.reply_of(server, &query, self.truncate_udp);
            Box::pin(async move { reply.ok_or_else(|| std::io::Error::other("no answer")) })
        }

        fn over_tcp(
            &self,
            server: SocketAddr,
            query: Vec<u8>,
        ) -> BoxFuture<'static, std::io::Result<Vec<u8>>> {
            self.tcp.lock().unwrap().push(server);
            let reply = self.reply_of(server, &query, false);
            Box::pin(async move { reply.ok_or_else(|| std::io::Error::other("no answer")) })
        }
    }

    const PATIENCE: Duration = Duration::from_secs(5);

    async fn eventually(mut settled: impl FnMut() -> bool) {
        let until = Instant::now() + PATIENCE;
        while Instant::now() < until {
            if settled() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the refresh never landed");
    }

    /// A host whose configuration takes a while to read: the first read answers at once, every later one blocks.
    struct SlowSources {
        entered: AtomicUsize,
        finished: AtomicUsize,
        reading: Arc<tokio::sync::Semaphore>,
        gate: Mutex<std::sync::mpsc::Receiver<()>>,
        blocked_for: Duration,
        refreshed: Vec<Scope>,
    }

    impl SlowSources {
        fn new(
            gate: std::sync::mpsc::Receiver<()>,
            blocked_for: Duration,
            refreshed: Vec<Scope>,
        ) -> Self {
            Self {
                entered: AtomicUsize::new(0),
                finished: AtomicUsize::new(0),
                reading: Arc::new(tokio::sync::Semaphore::new(0)),
                gate: Mutex::new(gate),
                blocked_for,
                refreshed,
            }
        }

        async fn reading(&self) {
            let _ = self.reading.acquire().await.expect("the read starts");
        }

        fn reads(&self) -> (usize, usize) {
            (
                self.entered.load(Ordering::SeqCst),
                self.finished.load(Ordering::SeqCst),
            )
        }
    }

    impl Sources for SlowSources {
        fn scopes(&self) -> Vec<Scope> {
            if self.entered.fetch_add(1, Ordering::SeqCst) == 0 {
                return vec![scope(None, &["1.1.1.1"])];
            }
            self.reading.add_permits(1);
            let _ = self
                .gate
                .lock()
                .expect("gate poisoned")
                .recv_timeout(self.blocked_for);
            self.finished.fetch_add(1, Ordering::SeqCst);
            self.refreshed.clone()
        }
    }

    #[tokio::test]
    async fn a_burst_of_old_lists_is_read_again_once_and_waited_for_by_nobody() {
        let (release, gate) = std::sync::mpsc::channel();
        let sources = Arc::new(SlowSources::new(
            gate,
            PATIENCE,
            vec![scope(None, &["9.9.9.9"])],
        ));
        let resolvers = Resolvers::new(Arc::clone(&sources) as Arc<dyn Sources>, Duration::ZERO);

        for _ in 0..5 {
            assert_eq!(
                resolvers.servers_for("example.com"),
                vec![server("1.1.1.1")],
                "a query is answered from the list in hand, never from a read it waits for"
            );
        }

        sources.reading().await;
        assert_eq!(
            sources.reads(),
            (2, 0),
            "one read at construction and one refresh for the whole burst, still reading"
        );
        drop(release);
    }

    #[tokio::test]
    async fn a_host_configuration_that_will_not_be_read_leaves_the_list_in_hand() {
        let (release, gate) = std::sync::mpsc::channel();
        let sources = Arc::new(SlowSources::new(
            gate,
            REFRESH_TIMEOUT * 2,
            vec![scope(None, &["9.9.9.9"])],
        ));
        let resolvers = Resolvers::new(Arc::clone(&sources) as Arc<dyn Sources>, Duration::ZERO);

        assert_eq!(
            resolvers.servers_for("example.com"),
            vec![server("1.1.1.1")]
        );
        sources.reading().await;
        tokio::time::sleep(REFRESH_TIMEOUT + Duration::from_millis(200)).await;

        assert_eq!(
            resolvers.servers_for("example.com"),
            vec![server("1.1.1.1")],
            "a read that outlasts its own deadline changes nothing"
        );
        drop(release);
    }

    struct FixedSources(Mutex<Vec<Vec<Scope>>>);

    impl Sources for FixedSources {
        fn scopes(&self) -> Vec<Scope> {
            let mut reads = self.0.lock().unwrap();
            if reads.len() > 1 {
                reads.remove(0)
            } else {
                reads[0].clone()
            }
        }
    }

    fn sources() -> Arc<FixedSources> {
        Arc::new(FixedSources(Mutex::new(vec![
            vec![scope(None, &["1.1.1.1"])],
            vec![scope(None, &["9.9.9.9"])],
        ])))
    }

    #[tokio::test]
    async fn a_query_is_relayed_to_the_upstream_unchanged() {
        let upstream = FakeUpstream::answering(server("1.1.1.1"));
        let query = question("example.com.", RecordType::MX);

        let reply = relay(&query, &[server("1.1.1.1")], &upstream).await;

        let reply = Message::from_vec(&reply).expect("the upstream's own answer comes back");
        assert_eq!(reply.metadata.id, 0x4242);
        assert_eq!(reply.metadata.response_code, ResponseCode::NoError);
        assert_eq!(reply.queries[0].query_type(), RecordType::MX);
        assert_eq!(
            upstream.udp.lock().unwrap().as_slice(),
            &[server("1.1.1.1")]
        );
        assert!(
            upstream.tcp.lock().unwrap().is_empty(),
            "an answer that fits needs no second ask"
        );
    }

    #[tokio::test]
    async fn a_truncated_answer_is_asked_again_over_tcp_to_the_same_server() {
        let mut upstream = FakeUpstream::answering(server("1.1.1.1"));
        upstream.truncate_udp = true;
        let query = question("example.com.", RecordType::A);

        let reply = relay(&query, &[server("1.1.1.1")], &upstream).await;

        assert!(
            !Message::from_vec(&reply).unwrap().metadata.truncation,
            "the guest gets the whole answer, not the truncated one"
        );
        assert_eq!(
            upstream.tcp.lock().unwrap().as_slice(),
            &[server("1.1.1.1")]
        );
    }

    #[tokio::test]
    async fn a_truncated_answer_no_tcp_server_will_repeat_is_a_server_failure() {
        let mut upstream = FakeUpstream::answering(server("1.1.1.1"));
        upstream.truncate_udp = true;
        upstream.tcp = Mutex::new(Vec::new());
        let query = question("example.com.", RecordType::A);
        let servers = [server("1.1.1.1")];

        let reply = relay(&query, &servers, &TcpRefusing(upstream)).await;

        assert_eq!(
            Message::from_vec(&reply).unwrap().metadata.response_code,
            ResponseCode::ServFail
        );
    }

    struct TcpRefusing(FakeUpstream);

    impl Upstream for TcpRefusing {
        fn over_udp(
            &self,
            server: SocketAddr,
            query: Vec<u8>,
        ) -> BoxFuture<'static, std::io::Result<Vec<u8>>> {
            self.0.over_udp(server, query)
        }

        fn over_tcp(
            &self,
            _server: SocketAddr,
            _query: Vec<u8>,
        ) -> BoxFuture<'static, std::io::Result<Vec<u8>>> {
            Box::pin(async { Err(std::io::Error::other("the server refused TCP")) })
        }
    }

    #[tokio::test]
    async fn each_upstream_is_asked_in_turn_until_one_answers() {
        let upstream = FakeUpstream::answering(server("9.9.9.9"));
        let servers = [server("1.1.1.1"), server("8.8.8.8"), server("9.9.9.9")];
        let query = question("example.com.", RecordType::A);

        let reply = relay(&query, &servers, &upstream).await;

        assert_eq!(
            Message::from_vec(&reply).unwrap().metadata.response_code,
            ResponseCode::NoError
        );
        assert_eq!(upstream.udp.lock().unwrap().as_slice(), &servers);
    }

    #[tokio::test]
    async fn a_server_that_says_it_cannot_answer_sends_the_query_on_to_the_next() {
        let mut upstream = FakeUpstream::answering(server("9.9.9.9"));
        upstream.saying = vec![
            (server("1.1.1.1"), ResponseCode::ServFail),
            (server("8.8.8.8"), ResponseCode::Refused),
        ];
        let servers = [server("1.1.1.1"), server("8.8.8.8"), server("9.9.9.9")];
        let query = question("example.com.", RecordType::A);

        let reply = relay(&query, &servers, &upstream).await;

        let reply = Message::from_vec(&reply).unwrap();
        assert_eq!(
            reply.metadata.response_code,
            ResponseCode::NoError,
            "a healthy server later in the list still answers the guest"
        );
        assert!(
            !reply.metadata.authoritative,
            "the answer is the third server's own"
        );
        assert_eq!(upstream.udp.lock().unwrap().as_slice(), &servers);
    }

    #[tokio::test]
    async fn a_name_that_does_not_exist_is_an_answer_and_ends_the_search() {
        let mut upstream = FakeUpstream::answering(server("9.9.9.9"));
        upstream.saying = vec![(server("1.1.1.1"), ResponseCode::NXDomain)];
        let servers = [server("1.1.1.1"), server("9.9.9.9")];
        let query = question("nothing.example.com.", RecordType::A);

        let reply = relay(&query, &servers, &upstream).await;

        assert_eq!(
            Message::from_vec(&reply).unwrap().metadata.response_code,
            ResponseCode::NXDomain
        );
        assert_eq!(
            upstream.udp.lock().unwrap().as_slice(),
            &[server("1.1.1.1")],
            "a name nobody has is an answer, not a reason to ask elsewhere"
        );
    }

    #[tokio::test]
    async fn when_every_server_refuses_the_guest_gets_the_last_refusal_itself() {
        let mut upstream = FakeUpstream::silent();
        upstream.saying = vec![
            (server("1.1.1.1"), ResponseCode::ServFail),
            (server("8.8.8.8"), ResponseCode::NotImp),
        ];
        let servers = [server("1.1.1.1"), server("8.8.8.8")];
        let query = question("example.com.", RecordType::A);

        let reply = relay(&query, &servers, &upstream).await;

        let reply = Message::from_vec(&reply).unwrap();
        assert_eq!(reply.metadata.response_code, ResponseCode::NotImp);
        assert!(
            reply.metadata.authoritative,
            "the guest gets an upstream's own reply, not one the gateway made up"
        );
    }

    #[tokio::test]
    async fn a_query_no_upstream_answers_is_a_server_failure_not_silence() {
        let query = question("example.com.", RecordType::A);

        let reply = relay(&query, &[server("1.1.1.1")], &FakeUpstream::silent()).await;

        let reply = Message::from_vec(&reply).unwrap();
        assert_eq!(reply.metadata.response_code, ResponseCode::ServFail);
        assert_eq!(reply.metadata.id, 0x4242);
        assert_eq!(reply.queries[0].name().to_string(), "example.com.");
        assert!(reply.metadata.recursion_available);
        assert!(reply.metadata.recursion_desired);
    }

    #[tokio::test]
    async fn a_guest_with_no_resolver_configured_still_gets_an_answer() {
        let query = question("example.com.", RecordType::A);

        let reply = relay(&query, &[], &FakeUpstream::silent()).await;

        assert_eq!(
            Message::from_vec(&reply).unwrap().metadata.response_code,
            ResponseCode::ServFail
        );
    }

    #[test]
    fn bytes_that_are_not_a_question_name_nothing_to_relay() {
        assert_eq!(question_name(b"not dns at all"), None);

        let empty = Message::new(1, MessageType::Query, OpCode::Query);
        assert_eq!(
            question_name(&empty.to_vec().unwrap()),
            None,
            "a message with no question asks nothing"
        );

        let mut response = Message::new(2, MessageType::Response, OpCode::Query);
        response.add_query(Query::query(
            Name::from_ascii("example.com.").unwrap(),
            RecordType::A,
        ));
        assert_eq!(
            question_name(&response.to_vec().unwrap()),
            None,
            "the gateway relays questions, not answers"
        );

        assert_eq!(
            question_name(&question("example.com.", RecordType::A)),
            Some("example.com.".to_string())
        );
    }

    #[test]
    fn bytes_that_are_not_a_question_cannot_be_failed_either() {
        assert_eq!(servfail(b"not dns at all"), Vec::<u8>::new());
    }

    #[test]
    fn the_nameservers_of_resolv_conf_are_the_default_scope() {
        let scopes = scopes_of_resolv_conf(
            "# a comment\nsearch example.com\nnameserver 1.1.1.1\nnameserver 8.8.8.8\n",
        );
        assert_eq!(scopes, vec![scope(None, &["1.1.1.1", "8.8.8.8"])]);
    }

    #[test]
    fn a_resolv_conf_with_no_usable_nameserver_is_no_scope_at_all() {
        for contents in [
            "",
            "search example.com\n",
            "nameserver\n",
            "nameserver not-an-address\n",
            "#nameserver 1.1.1.1\n",
        ] {
            assert_eq!(scopes_of_resolv_conf(contents), Vec::new(), "{contents:?}");
        }
    }

    #[test]
    fn an_ipv6_nameserver_is_taken_as_written() {
        assert_eq!(
            scopes_of_resolv_conf("nameserver 2606:4700:4700::1111\n"),
            vec![scope(None, &["2606:4700:4700::1111"])]
        );
    }

    const SCUTIL: &str = "\
DNS configuration

resolver #1
  search domain[0] : example.com
  nameserver[0] : 192.168.1.1
  nameserver[1] : 192.168.1.2
  if_index : 5 (en0)
  flags    : Request A records, Request AAAA records

resolver #2
  domain   : corp.internal
  nameserver[0] : 10.0.0.53
  flags    : Supplemental, Request A records

resolver #3
  domain   : vpn.corp.internal
  nameserver[0] : not-an-address
  nameserver[1] : 10.1.0.53

resolver #4
  domain   : nothing.answers.this

DNS configuration (for scoped queries)

resolver #1
  nameserver[0] : 172.16.0.1
";

    #[test]
    fn scutil_names_the_resolvers_resolv_conf_does_not() {
        assert_eq!(
            scopes_of_scutil(SCUTIL),
            vec![
                scope(None, &["192.168.1.1", "192.168.1.2"]),
                scope(Some("corp.internal"), &["10.0.0.53"]),
                scope(Some("vpn.corp.internal"), &["10.1.0.53"]),
            ],
            "a resolver with no nameserver is no resolver, and the scoped-query section is not a domain"
        );
    }

    #[test]
    fn a_resolver_on_its_own_port_is_asked_there() {
        let scopes = scopes_of_scutil(
            "\
resolver #1
  domain   : corp.internal
  nameserver[0] : 127.0.0.1
  nameserver[1] : 10.0.0.53
  port     : 5353
  flags    : Supplemental, Request A records
",
        );

        assert_eq!(
            scopes,
            vec![Scope {
                suffix: Some("corp.internal".to_string()),
                servers: vec![
                    SocketAddr::new("127.0.0.1".parse().unwrap(), 5353),
                    SocketAddr::new("10.0.0.53".parse().unwrap(), 5353),
                ],
            }],
            "the port of a resolver belongs to every nameserver it names"
        );
    }

    #[test]
    fn a_resolver_whose_port_is_not_a_number_is_asked_on_53() {
        let scopes =
            scopes_of_scutil("resolver #1\n  nameserver[0] : 10.0.0.53\n  port : nonsense\n");

        assert_eq!(scopes, vec![scope(None, &["10.0.0.53"])]);
    }

    #[test]
    fn scutil_output_that_names_nothing_is_no_scope() {
        for output in [
            "",
            "DNS configuration\n",
            "  nameserver[0] : 1.1.1.1\n",
            "resolver #1\n  a line with no colon\n",
        ] {
            assert_eq!(scopes_of_scutil(output), Vec::new(), "{output:?}");
        }
    }

    #[test]
    fn the_longest_matching_domain_decides_which_servers_answer() {
        let scopes = scopes_of_scutil(SCUTIL);
        assert_eq!(
            servers_for(&scopes, "host.vpn.corp.internal."),
            vec![server("10.1.0.53")]
        );
        assert_eq!(
            servers_for(&scopes, "host.corp.internal"),
            vec![server("10.0.0.53")]
        );
        assert_eq!(
            servers_for(&scopes, "CORP.INTERNAL"),
            vec![server("10.0.0.53")],
            "a domain is matched without regard to case"
        );
        assert_eq!(
            servers_for(&scopes, "example.com"),
            vec![server("192.168.1.1"), server("192.168.1.2")],
            "a name no scope covers goes to the default resolvers"
        );
        assert_eq!(
            servers_for(&scopes, "notcorp.internal"),
            vec![server("192.168.1.1"), server("192.168.1.2")],
            "a suffix matches on a label boundary, not on characters"
        );
    }

    #[test]
    fn a_host_with_no_resolver_at_all_names_no_server() {
        assert_eq!(servers_for(&[], "example.com"), Vec::new());
    }

    #[tokio::test]
    async fn the_resolver_list_is_read_again_once_it_is_old() {
        let resolvers = Resolvers::new(sources(), Duration::ZERO);

        assert_eq!(
            resolvers.servers_for("example.com"),
            vec![server("1.1.1.1")],
            "the query in hand is answered from the list in hand"
        );

        eventually(|| resolvers.servers_for("example.com") == vec![server("9.9.9.9")]).await;
    }

    #[tokio::test]
    async fn a_fresh_resolver_list_is_not_read_again_for_every_query() {
        let resolvers = Resolvers::new(sources(), Duration::from_secs(3600));

        assert_eq!(
            resolvers.servers_for("example.com"),
            vec![server("1.1.1.1")]
        );
        assert_eq!(
            resolvers.servers_for("example.com"),
            vec![server("1.1.1.1")]
        );
    }

    #[tokio::test]
    async fn a_query_nobody_answered_makes_the_next_one_read_the_list_again() {
        let resolvers = Resolvers::new(sources(), Duration::from_secs(3600));
        assert_eq!(
            resolvers.servers_for("example.com"),
            vec![server("1.1.1.1")]
        );

        resolvers.stale();

        eventually(|| resolvers.servers_for("example.com") == vec![server("9.9.9.9")]).await;
    }
}
