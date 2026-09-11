use super::Children;
use std::io;
use std::time::Duration;
use tokio::signal::unix::{SignalKind, signal};

struct ProcessChildren;

impl Children for ProcessChildren {
    fn exited(&mut self) -> io::Result<Option<i32>> {
        loop {
            // SAFETY: siginfo_t accepts zero initialization and is exclusively borrowed by waitid.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: waitid writes only the initialized info buffer and does not reap with WNOWAIT.
            let result = unsafe {
                libc::waitid(
                    libc::P_ALL,
                    0,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result == 0 {
                // SAFETY: successful waitid populated the SIGCHLD union fields, or left the zero sentinel.
                let pid = unsafe { info.si_pid() };
                return Ok((pid > 0).then_some(pid));
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::ECHILD) => return Ok(None),
                _ => return Err(error),
            }
        }
    }

    fn managed(&self, pid: i32) -> bool {
        openshell_supervisor_process::managed_children::is_managed(pid)
    }

    fn reap(&mut self, pid: i32) -> io::Result<()> {
        // SAFETY: pid was observed by waitid and a null status pointer discards only this orphan's status.
        if unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) } >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ECHILD | libc::EINTR) => Ok(()),
            _ => Err(error),
        }
    }
}

pub fn start() -> io::Result<tokio::task::JoinHandle<io::Result<()>>> {
    // SAFETY: prctl marks this supervisor as the adoption point for its orphaned descendants.
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut changed = signal(SignalKind::child())?;
    Ok(tokio::spawn(async move {
        let mut retry = tokio::time::interval(Duration::from_secs(1));
        retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = changed.recv() => {},
                _ = retry.tick() => {},
            }
            super::drain(&mut ProcessChildren)?;
        }
    }))
}
