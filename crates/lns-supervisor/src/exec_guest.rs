use lns_openshell_spike::{
    Error,
    launch::real::{self as launch, Launch},
};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::os::fd::AsFd;

pub async fn run() -> Result<i32, Error> {
    let mut args = std::env::args().skip(2);
    let uid: u32 = args.next().ok_or("missing exec uid")?.parse()?;
    let gid: u32 = args.next().ok_or("missing exec gid")?.parse()?;
    let program = args.next().ok_or("missing exec command")?;
    let args = args.collect::<Vec<_>>();
    let namespace = crate::isolation::namespace()?;
    let policy = crate::openshell_guest::workload_policy(uid, gid);
    openshell_supervisor_process::sandbox::apply_supervisor_startup_hardening()
        .map_err(|error| error.to_string())?;
    let mut reaper = crate::reaper::real::start()?;
    let signals = crate::signals::Signals::new()?;
    let mut env = std::env::vars().collect::<HashMap<_, _>>();
    env.extend(
        openshell_supervisor_process::child_env::proxy_env_vars(lns_session::isolation::PROXY_URL)
            .map(|(key, value)| (key.to_string(), value)),
    );
    let cwd = std::env::current_dir()?;
    let (child, io) = launch::spawn(Launch {
        program: &program,
        args: &args,
        cwd: cwd.to_str().ok_or("non-UTF8 exec cwd")?,
        env: &env,
        uid,
        gid,
        terminal: std::io::stdin().is_terminal(),
        namespace: Some(namespace.as_fd()),
        policy: &policy,
    })?;
    tokio::select! {
        result = crate::workload::run(child, io, signals) => result,
        result = &mut reaper => Err(format!("exec orphan reaper exited: {result:?}").into()),
    }
}
