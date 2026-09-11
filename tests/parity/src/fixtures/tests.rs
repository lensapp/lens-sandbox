use super::*;
use pattern::{pattern_sha256, zeros_sha256};
use std::io::ErrorKind;

fn fixtures(sizes: Sizes) -> Fixtures {
    Fixtures::start(Ipv4Addr::LOCALHOST, 0, sizes).expect("fixtures bind on loopback")
}

fn small() -> Sizes {
    Sizes {
        source_bytes: 64 * 1024,
        bidirectional_bytes: 64 * 1024,
        half_close_reply_bytes: 4096,
        host_half_close_bytes: 8192,
        reset_after_bytes: 4096,
    }
}

fn connect(f: &Fixtures, role: Role) -> TcpStream {
    TcpStream::connect((f.bind(), f.port(role))).expect("connect to the fixture")
}

fn wait_for<T>(mut probe: impl FnMut() -> Option<T>) -> T {
    for _ in 0..600 {
        if let Some(value) = probe() {
            return value;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the fixture never recorded what the test waited for");
}

fn closed_record(f: &Fixtures, role: Role) -> ConnRecord {
    let port = f.port(role);
    wait_for(|| {
        f.report()
            .last_on(port)
            .filter(|record| record.closed_ms.is_some())
            .cloned()
    })
}

#[test]
fn the_sink_counts_and_hashes_every_byte_it_read() {
    let f = fixtures(small());
    let mut client = connect(&f, Role::Sink);
    client.write_all(&vec![0u8; 40_000]).unwrap();
    drop(client);

    let record = closed_record(&f, Role::Sink);
    assert_eq!(record.bytes_in, 40_000);
    assert_eq!(record.sha_in, Some(zeros_sha256(40_000)));
    assert!(record.saw_eof, "the sink must record the guest's EOF");
}

#[test]
fn the_source_sends_the_deterministic_pattern() {
    let f = fixtures(small());
    let mut client = connect(&f, Role::Source);
    let mut received = Vec::new();
    client.read_to_end(&mut received).unwrap();

    assert_eq!(received.len(), 64 * 1024);
    assert_eq!(received, pattern_chunk(0, 64 * 1024));
    let record = closed_record(&f, Role::Source);
    assert_eq!(record.sha_out, Some(pattern_sha256(64 * 1024)));
}

#[test]
fn the_bidirectional_fixture_counts_both_directions() {
    let f = fixtures(small());
    let mut client = connect(&f, Role::Bidirectional);
    let mut sender = client.try_clone().unwrap();
    let upload = std::thread::spawn(move || {
        sender.write_all(&vec![0u8; 20_000]).unwrap();
        sender.shutdown(std::net::Shutdown::Write).unwrap();
    });
    let mut received = Vec::new();
    client.read_to_end(&mut received).unwrap();
    upload.join().unwrap();

    let record = closed_record(&f, Role::Bidirectional);
    assert_eq!(record.bytes_in, 20_000);
    assert_eq!(record.sha_in, Some(zeros_sha256(20_000)));
    assert_eq!(received.len(), 64 * 1024);
    assert_eq!(record.sha_out, Some(pattern_sha256(64 * 1024)));
}

#[test]
fn the_half_close_fixture_replies_only_after_the_peer_stopped_writing() {
    let f = fixtures(small());
    let mut client = connect(&f, Role::HalfCloseReply);
    client.write_all(&vec![0u8; 1000]).unwrap();
    client.shutdown(std::net::Shutdown::Write).unwrap();

    let mut reply = Vec::new();
    client.read_to_end(&mut reply).unwrap();
    assert_eq!(reply.len(), 4096, "the reply follows the half close");

    let record = closed_record(&f, Role::HalfCloseReply);
    assert!(record.saw_eof);
    assert_eq!(record.bytes_in, 1000);
    assert_eq!(record.bytes_out, 4096);
}

#[test]
fn the_host_half_close_fixture_keeps_reading_after_it_stopped_writing() {
    let f = fixtures(small());
    let mut client = connect(&f, Role::HostHalfClose);
    let mut received = vec![0u8; 8192];
    client.read_exact(&mut received).unwrap();
    assert_eq!(
        client.read(&mut [0u8; 16]).unwrap(),
        0,
        "the client sees EOF"
    );

    client.write_all(&vec![0u8; 5000]).unwrap();
    client.shutdown(std::net::Shutdown::Write).unwrap();

    let record = closed_record(&f, Role::HostHalfClose);
    assert_eq!(record.bytes_out, 8192);
    assert_eq!(record.bytes_in, 5000);
    assert_eq!(record.sha_in, Some(zeros_sha256(5000)));
}

#[test]
fn the_reset_fixture_commits_what_it_sent_before_the_reset() {
    let f = fixtures(small());
    let mut client = connect(&f, Role::Reset);
    let mut received = Vec::new();
    let outcome = client.read_to_end(&mut received);

    let record = closed_record(&f, Role::Reset);
    assert!(
        record.reset_sent,
        "the fixture records that it sent a reset"
    );
    assert_eq!(record.bytes_out, 4096);
    match outcome {
        Err(err) => assert_eq!(err.kind(), ErrorKind::ConnectionReset),
        Ok(_) => assert!(
            received.len() <= 4096,
            "a peer that read everything before the reset never sees more than the fixture committed"
        ),
    }
}

#[test]
fn the_tcp_echo_returns_what_it_read() {
    let f = fixtures(small());
    let mut client = connect(&f, Role::Echo);
    client.write_all(b"parity").unwrap();
    let mut back = [0u8; 6];
    client.read_exact(&mut back).unwrap();
    assert_eq!(&back, b"parity");
}

#[test]
fn the_udp_echo_returns_an_empty_datagram_too() {
    let f = fixtures(small());
    let target = (f.bind(), f.port(Role::UdpEcho));
    let client = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    client.send_to(&[7u8; 100], target).unwrap();
    let mut buffer = [0u8; 200];
    assert_eq!(client.recv(&mut buffer).unwrap(), 100);

    client.send_to(&[], target).unwrap();
    assert_eq!(
        client.recv(&mut buffer).unwrap(),
        0,
        "an empty datagram is echoed as an empty datagram"
    );

    let lengths: Vec<usize> = wait_for(|| {
        let report = f.report();
        (report.datagrams.len() == 2).then(|| report.datagrams.iter().map(|d| d.len).collect())
    });
    assert_eq!(lengths, vec![100, 0]);
}

#[test]
fn the_witness_records_every_accept() {
    let f = fixtures(small());
    let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, f.port(Role::Witness))).unwrap();
    let mut body = String::new();
    client.read_to_string(&mut body).unwrap();
    assert!(body.ends_with("REACHED!"), "the witness answers: {body}");

    let accepts = wait_for(|| f.report().witness_accepts.eq(&1).then_some(1));
    assert_eq!(accepts, 1);
}

#[test]
fn the_report_names_the_bind_and_every_port_it_holds() {
    let f = fixtures(small());
    let report = f.report();
    assert_eq!(report.bind, "127.0.0.1");
    for role in [Role::Sink, Role::Source, Role::UdpEcho, Role::Witness] {
        assert!(
            report.ports.contains_key(role.as_str()),
            "the report must name the {} port",
            role.as_str()
        );
    }
}

#[test]
fn the_report_is_written_where_the_runner_reads_it() {
    let f = fixtures(small());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixtures.json");
    f.write_report(&path).unwrap();

    let parsed: FixtureReport = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
        .expect("the report parses as the schema the runner reads");
    assert_eq!(parsed.bind, "127.0.0.1");
}

#[test]
fn a_bind_address_a_guest_cannot_reach_is_refused() {
    for refused in [
        Ipv4Addr::new(127, 0, 0, 1),
        Ipv4Addr::UNSPECIFIED,
        Ipv4Addr::new(192, 168, 127, 5),
    ] {
        assert!(
            refuse_unsuitable_bind(refused).is_err(),
            "{refused} must be refused as a fixture bind"
        );
    }
    assert!(refuse_unsuitable_bind(Ipv4Addr::new(192, 168, 1, 50)).is_ok());
}

#[test]
fn consecutive_ports_follow_the_base_port() {
    assert_eq!(offset_port(47200, 0).unwrap(), 47200);
    assert_eq!(offset_port(47200, 8).unwrap(), 47208);
    assert_eq!(offset_port(0, 3).unwrap(), 0);
    assert!(offset_port(65535, 1).is_err());
}

#[test]
fn every_lan_fixture_is_a_destination_the_guests_definition_decides() {
    let f = fixtures(small());
    let destinations = f.guest_destinations();

    let expected: Vec<String> = {
        let report = f.report();
        let mut ports: Vec<u16> = report
            .ports
            .iter()
            .filter(|(role, _)| role.as_str() != Role::Witness.as_str())
            .map(|(_, port)| *port)
            .collect();
        ports.sort_unstable();
        ports
            .into_iter()
            .map(|port| format!("{}:{port}", f.bind()))
            .collect()
    };
    assert_eq!(destinations, expected);
    assert_eq!(destinations.len(), 8, "seven TCP fixtures and the UDP echo");

    let witness = format!("{}:{}", f.bind(), f.port(Role::Witness));
    assert!(
        !destinations.contains(&witness),
        "the witness binds the host's loopback; a case proves the guest cannot reach it"
    );
}

#[test]
fn what_the_fixtures_saw_since_a_mark_is_counted_on_both_directions() {
    let f = fixtures(small());
    let mark = f.report().connections.last().map(|c| c.id).unwrap_or(0);
    let mut client = connect(&f, Role::Sink);
    client.write_all(&vec![0u8; 1_000]).unwrap();
    drop(client);
    closed_record(&f, Role::Sink);

    let activity = f.activity_since(mark);
    assert_eq!(activity.connections, 1);
    assert_eq!(activity.bytes_in, 1_000);
    assert_eq!(activity.bytes_out, 0);
    assert_eq!(activity.bytes(), 1_000);

    let after = f.report().connections.last().map(|c| c.id).unwrap_or(0);
    assert_eq!(f.activity_since(after), Activity::default());
}
