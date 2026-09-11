use lns_session::isolation::{LISTENER_FD, NAMESPACE_FD, SUPERVISOR, SUPERVISOR_FD};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::OnceLock;

static NETWORK: OnceLock<Result<Network, String>> = OnceLock::new();

pub struct Network {
    pub namespace: std::fs::File,
    pub listener: std::net::TcpListener,
    supervisor: std::fs::File,
}

pub fn active() -> bool {
    NETWORK.get().is_some()
}

pub fn enter() -> io::Result<()> {
    let fd = network()?.namespace.as_raw_fd();
    // SAFETY: only this dedicated forwarding thread enters the broker-owned workload namespace.
    if unsafe { libc::setns(fd, libc::CLONE_NEWNET) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn initialize() -> Result<(), String> {
    NETWORK
        .get_or_init(|| create().map_err(|e| e.to_string()))
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}

fn network() -> io::Result<&'static Network> {
    NETWORK
        .get()
        .ok_or_else(|| io::Error::other("workload network not initialized"))?
        .as_ref()
        .map_err(|error| io::Error::other(error.clone()))
}

pub fn handoff(primary: bool) -> io::Result<()> {
    let (real, effective, saved) = super::launcher_identity();
    // SAFETY: distinct real/effective UIDs prevent workload ptrace access across the privileged exec transition.
    if unsafe { libc::setresuid(real, effective, saved) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let network = network()?;
    let namespace = duplicate(network.namespace.as_raw_fd())?;
    let listener = duplicate(network.listener.as_raw_fd())?;
    let supervisor = duplicate(network.supervisor.as_raw_fd())?;
    for (source, target, flags) in [
        (namespace.as_raw_fd(), NAMESPACE_FD, 0),
        (supervisor.as_raw_fd(), SUPERVISOR_FD, libc::O_CLOEXEC),
    ] {
        // SAFETY: source is owned and target is a reserved descriptor in the post-fork child.
        if unsafe { libc::dup3(source, target, flags) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    if primary {
        // SAFETY: listener is owned and descriptor four is reserved in this post-fork child.
        if unsafe { libc::dup3(listener.as_raw_fd(), LISTENER_FD, 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
    } else {
        // SAFETY: the exec launcher does not receive the privileged proxy listener.
        unsafe { libc::close(LISTENER_FD) };
    }
    Ok(())
}

fn duplicate(fd: i32) -> io::Result<OwnedFd> {
    // SAFETY: fcntl duplicates a borrowed live descriptor above the reserved range.
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 6) };
    if copy < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: copy is newly allocated and ownership transfers once.
    Ok(unsafe { OwnedFd::from_raw_fd(copy) })
}

fn pin_supervisor() -> io::Result<std::fs::File> {
    let mut source = std::fs::File::open(SUPERVISOR)?;
    // SAFETY: memfd_create borrows a static nul-terminated name and returns an owned descriptor.
    let fd = unsafe {
        libc::memfd_create(
            c"lns-supervisor".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is newly allocated and ownership transfers once.
    let mut pinned = unsafe { std::fs::File::from_raw_fd(fd) };
    std::io::copy(&mut source, &mut pinned)?;
    super::seal(fd, |fd, seals| {
        // SAFETY: fcntl seals the owned memfd after copying and before exposing it to children.
        if unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })?;
    Ok(pinned)
}

fn create() -> io::Result<Network> {
    std::thread::spawn(create_on_thread)
        .join()
        .map_err(|_| io::Error::other("script network setup thread panicked"))?
}

fn create_on_thread() -> io::Result<Network> {
    // SAFETY: unshare changes only this dedicated thread's network namespace.
    if unsafe { libc::unshare(libc::CLONE_NEWNET) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: socket returns a new owned descriptor or a negative error sentinel.
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is newly allocated and ownership is transferred exactly once.
    let socket = unsafe { OwnedFd::from_raw_fd(fd) };
    super::configure_loopback(|name, address| {
        configure_interface(socket.as_raw_fd(), name, address)
    })?;
    let listener = std::net::TcpListener::bind("10.200.0.1:3128")?;
    listener.set_nonblocking(true)?;
    Ok(Network {
        supervisor: pin_supervisor()?,
        namespace: std::fs::File::open("/proc/thread-self/ns/net")?,
        listener,
    })
}

fn configure_interface(fd: i32, name: &str, address: Option<[u8; 4]>) -> io::Result<()> {
    // SAFETY: an all-zero ifreq is valid initial storage for interface ioctls.
    let mut request: libc::ifreq = unsafe { std::mem::zeroed() };
    for (target, source) in request.ifr_name.iter_mut().zip(name.bytes()) {
        *target = source as libc::c_char;
    }
    let operation = if let Some(address) = address {
        let address = libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: 0,
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes(address),
            },
            sin_zero: [0; 8],
        };
        // SAFETY: sockaddr_in and sockaddr have identical sizes and the union accepts sockaddr storage.
        request.ifr_ifru.ifru_addr =
            unsafe { std::mem::transmute::<libc::sockaddr_in, libc::sockaddr>(address) };
        libc::SIOCSIFADDR
    } else {
        request.ifr_ifru.ifru_flags = (libc::IFF_UP | libc::IFF_LOOPBACK) as libc::c_short;
        libc::SIOCSIFFLAGS
    };
    // SAFETY: request holds the initialized union member required by operation and fd is a live socket.
    if unsafe { libc::ioctl(fd, operation as _, &request) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
