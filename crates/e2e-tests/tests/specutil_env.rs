#[path = "specutil/mod.rs"]
mod specutil;

use specutil::command_for;
use std::ffi::OsStr;

#[test]
fn every_lns_the_harness_spawns_opts_out_of_the_update_check() {
    let cmd = command_for("/bin/true");

    let opt_out = cmd
        .get_envs()
        .find(|(k, _)| *k == OsStr::new(lns_ipc::NO_UPDATE_CHECK_ENV));

    assert_eq!(
        opt_out.map(|(_, v)| v),
        Some(Some(OsStr::new("1"))),
        "the harness must not reach get.lns.run or mint an install id; envs={:?}",
        cmd.get_envs().collect::<Vec<_>>()
    );
}
