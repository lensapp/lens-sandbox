use crate::E2eWorld;
use cucumber::{given, then, when};
use lns_ipc::{OAuthProgress, Request, Response};
use std::io::Write;

fn request(world: &E2eWorld, request: Request) -> Response {
    let mut socket =
        std::os::unix::net::UnixStream::connect(world.service_socket.as_ref().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    socket
        .write_all(&lns_ipc::encode_frame(&request).unwrap())
        .unwrap();
    lns_ipc::decode_frame(&mut socket).unwrap()
}

#[given("a native device connector using an isolated loopback provider")]
fn install(world: &mut E2eWorld) {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let document = serde_json::json!({"apiVersion":"lns.run/v1","kind":"connector","name":"native-demo","spec":{"serves":["api.example.test"],"methods":[{"name":"sign-in","auth":{"kind":"oauth_device","clientId":"test-public-client","deviceAuthorizationEndpoint":format!("https://127.0.0.1:{port}/device"),"tokenEndpoint":format!("https://127.0.0.1:{port}/token"),"verificationHosts":["example.test"]},"credentials":[{"envVar":"API_TOKEN","placeholder":"LNSPLACEHOLDER0000000000","injections":[{"kind":"bearer_header","domain":"api.example.test"}]}]}]}});
    let path = world.home.as_ref().unwrap().path().join("native.yaml");
    std::fs::write(&path, document.to_string()).unwrap();
    assert!(matches!(
        request(
            world,
            Request::InstallConnector {
                source: path.to_str().unwrap().into()
            }
        ),
        Response::ConnectorInstalled { .. }
    ));
    world.native_provider = Some(listener);
}

#[when("I start native OAuth over the service socket")]
fn start(world: &mut E2eWorld) {
    let response = request(
        world,
        Request::BeginConnect {
            name: "native-demo".into(),
            method: "sign-in".into(),
            connection: "work".into(),
        },
    );
    let Response::ConnectorPending {
        session,
        progress: OAuthProgress::Starting { .. },
    } = response
    else {
        panic!("native authorization did not start: {response:?}")
    };
    world.native_pending = Some(session);
}

#[then("repeated native OAuth status requests share one background operation")]
fn status(world: &mut E2eWorld) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match world.native_provider.as_ref().unwrap().accept() {
            Ok((stream, _)) => {
                world.native_transport = Some(stream);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the service never started its background provider request"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => panic!("loopback provider failed: {error}"),
        }
    }
    for _ in 0..12 {
        let response = request(
            world,
            Request::ConnectStatus {
                session: world.native_pending.clone().unwrap(),
            },
        );
        assert!(matches!(
            response,
            Response::ConnectorPending {
                progress: OAuthProgress::Starting { .. },
                ..
            }
        ));
    }
    assert_eq!(
        world
            .native_provider
            .as_ref()
            .unwrap()
            .accept()
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[when("I cancel native OAuth over the service socket")]
fn cancel(world: &mut E2eWorld) {
    assert!(matches!(
        request(
            world,
            Request::CancelConnect {
                session: world.native_pending.clone().unwrap()
            }
        ),
        Response::ConnectorPending {
            progress: OAuthProgress::Canceled,
            ..
        }
    ));
    world.native_transport = None;
    world.native_provider = None;
}

#[then("native OAuth reports cancellation and keeps no connection")]
fn canceled(world: &mut E2eWorld) {
    assert!(matches!(
        request(
            world,
            Request::ConnectStatus {
                session: world.native_pending.clone().unwrap()
            }
        ),
        Response::ConnectorPending {
            progress: OAuthProgress::Canceled,
            ..
        }
    ));
    let Response::ConnectorList { connectors } = request(world, Request::ListConnectors) else {
        panic!("expected connector list")
    };
    assert_eq!(connectors.len(), 1);
    assert!(connectors[0].connections.is_empty());
    world.shutdown_service();
    super::service::start_service_with(world, &[("LNS_HEADLESS", "1")]);
    let Response::ConnectorList { connectors } = request(world, Request::ListConnectors) else {
        panic!("expected persisted connector list")
    };
    assert_eq!(connectors.len(), 1);
    assert!(connectors[0].connections.is_empty());
}

#[when("the native loopback callback adapter receives a validated authorization response")]
fn callback_adapter(world: &mut E2eWorld) {
    use lns_service::connector::mechanism::oauth::browser::Callback;
    use lns_service::connector::mechanism::traits::Mechanisms;
    use std::io::Read;
    let mechanisms = lns_service::connector::mechanism::real::RealMechanisms::new().unwrap();
    let method = serde_json::from_value(serde_json::json!({"name":"browser","auth":{"kind":"oauth_authorization_code","clientId":"test-public","authorizationEndpoint":"https://auth.example/authorize","tokenEndpoint":"https://auth.example/token","redirect":{"kind":"loopback"}}})).unwrap();
    let prepared = mechanisms.for_method("provider", &method, None).unwrap();
    let browser = &prepared.mechanism.native().unwrap().browser;
    let occupied = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let error = browser
        .prepare(
            "owner",
            "/callback",
            Some(occupied.local_addr().unwrap().port()),
            "private-state",
            100,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("close the application using that port")
    );
    let (handle, uri) = browser
        .prepare("owner", "/oauth/callback", None, "private-state", 100)
        .unwrap();
    assert!(uri.starts_with("http://127.0.0.1:"));
    let address = uri
        .strip_prefix("http://")
        .unwrap()
        .split('/')
        .next()
        .unwrap();
    let mut socket = std::net::TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    socket.write_all(b"GET /oauth/callback?state=private-state&code=private-code HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
    assert!(browser.poll("wrong-owner", &handle, 1).is_err());
    assert_eq!(
        browser.poll("owner", &handle, 1).unwrap(),
        Callback::Code("private-code".into())
    );
    assert!(browser.poll("owner", &handle, 1).is_err());
    let mut page = String::new();
    socket.read_to_string(&mut page).unwrap();
    assert!(!page.contains("private-code"));
    assert!(!page.contains("private-state"));
    assert!(page.contains("history.replaceState"));
    let (canceled, uri) = browser
        .prepare("owner", "/callback", None, "state", 100)
        .unwrap();
    let address = uri
        .strip_prefix("http://")
        .unwrap()
        .split('/')
        .next()
        .unwrap();
    browser.cancel(&canceled);
    assert!(std::net::TcpStream::connect(address).is_err());
    world.result = Some(crate::specutil::CliResult {
        exit_code: 0,
        stdout: "callback consumed and listener released".into(),
        stderr: String::new(),
    });
}

#[then("the callback is consumed once and its completion page contains no authorization values")]
fn callback_result(world: &mut E2eWorld) {
    assert_eq!(
        world.result.as_ref().unwrap().stdout,
        "callback consumed and listener released"
    );
}
