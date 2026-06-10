use crate::E2eWorld;
use crate::specutil::{run_cli_with_env, service_binary};
use cucumber::given;
use std::ffi::OsString;

#[given("the Lens Sandbox service is running in that home")]
fn service_running_in_home(world: &mut E2eWorld) {
    world.ensure_service_dir();
    let home = world
        .home
        .as_ref()
        .expect("Given a clean lns cache home first");
    let envs: Vec<(&str, OsString)> = vec![
        (
            "LNS_SOCKET_PATH",
            world.service_socket.clone().unwrap().into(),
        ),
        ("LNS_SERVICE_BIN", service_binary().into()),
        ("HOME", home.path().into()),
        ("XDG_CACHE_HOME", home.path().join(".cache").into()),
    ];
    let result = run_cli_with_env(["service", "start"], envs);
    assert!(
        result.exit_code == 0,
        "lns service start failed: stdout={:?} stderr={:?}",
        result.stdout,
        result.stderr
    );
}
