use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd};
use std::process::Stdio;

use openshell_core::policy::SandboxPolicy;
use openshell_supervisor_process::{managed_children, process, sandbox};
pub use process::{ProcessIo, ProcessStatus};

pub struct Launch<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub cwd: &'a str,
    pub env: &'a HashMap<String, String>,
    pub uid: u32,
    pub gid: u32,
    pub terminal: bool,
    pub namespace: Option<BorrowedFd<'a>>,
    pub policy: &'a SandboxPolicy,
}

pub struct Child {
    child: tokio::process::Child,
    pid: u32,
}

impl Child {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub async fn wait(&mut self) -> io::Result<ProcessStatus> {
        let status = self.child.wait().await?;
        managed_children::unregister(self.pid);
        Ok(status.into())
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        managed_children::unregister(self.pid);
    }
}

pub fn spawn(spec: Launch<'_>) -> io::Result<(Child, ProcessIo)> {
    super::validate_identity(spec.uid, spec.namespace.is_some())?;
    let mut command = super::command(spec.program, spec.args, spec.cwd, spec.env);
    let terminal = if spec.terminal {
        Some(open_terminal()?)
    } else {
        None
    };
    if let Some((_, slave)) = &terminal {
        command
            .stdin(slave.try_clone()?)
            .stdout(slave.try_clone()?)
            .stderr(slave.try_clone()?);
    } else {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    }
    let mut prepared = Some(sandbox::linux::prepare(spec.policy, Some(spec.cwd)).map_err(other)?);
    let terminal_fd = terminal.as_ref().map(|(_, slave)| slave.as_raw_fd());
    let namespace = spec.namespace.map(|fd| fd.as_raw_fd());
    let uid = spec.uid;
    let gid = spec.gid;
    let policy = spec.policy.clone();
    // SAFETY: descriptors live through spawn; the child performs namespace/identity setup and OpenShell's pre-exec enforcement before exec.
    unsafe {
        command.pre_exec(move || {
            establish_session(terminal_fd)?;
            if let Some(fd) = namespace
                && libc::setns(fd, libc::CLONE_NEWNET) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if uid == 0 {
                super::root::apply(gid)?;
            } else {
                process::drop_privileges_with_identity(
                    &policy,
                    process::ResolvedProcessIdentity::new(Some(uid), Some(gid)),
                )
                .map_err(other)?;
            }
            process::harden_child_process().map_err(other)?;
            sandbox::linux::enforce(
                prepared
                    .take()
                    .ok_or_else(|| io::Error::other("sandbox already consumed"))?,
            )
            .map_err(other)
        });
    }
    let mut child = command.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("spawn returned no PID"))?;
    managed_children::register(pid);
    let io = if let Some((master, _)) = terminal {
        ProcessIo::Pty(master)
    } else {
        ProcessIo::Pipes {
            stdin: child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("missing stdin pipe"))?,
            stdout: child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("missing stdout pipe"))?,
            stderr: child
                .stderr
                .take()
                .ok_or_else(|| io::Error::other("missing stderr pipe"))?,
        }
    };
    Ok((Child { child, pid }, io))
}

fn other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn establish_session(terminal: Option<i32>) -> io::Result<()> {
    // SAFETY: these scalar operations apply only to the post-fork child and its live terminal descriptor.
    unsafe {
        if let Some(fd) = terminal {
            if libc::setsid() < 0 || libc::ioctl(fd, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
        } else if libc::setpgid(0, 0) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn open_terminal() -> io::Result<(std::fs::File, std::fs::File)> {
    let mut master = -1;
    let mut slave = -1;
    // SAFETY: openpty initializes two descriptor outputs; null optional parameters select the default terminal settings.
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful openpty returned two distinct owned descriptors.
    let files = unsafe {
        (
            std::fs::File::from_raw_fd(master),
            std::fs::File::from_raw_fd(slave),
        )
    };
    for fd in [master, slave] {
        // SAFETY: both descriptors are live and F_SETFD consumes a scalar close-on-exec flag.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(files)
}
