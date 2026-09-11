use lns_session::SignalKind;
use std::io;
use tokio::signal::unix::{Signal, SignalKind as UnixSignal, signal};

pub struct Signals {
    interrupt: Signal,
    terminate: Signal,
    quit: Signal,
    hangup: Signal,
}

impl Signals {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            interrupt: signal(UnixSignal::interrupt())?,
            terminate: signal(UnixSignal::terminate())?,
            quit: signal(UnixSignal::quit())?,
            hangup: signal(UnixSignal::hangup())?,
        })
    }

    pub async fn next(&mut self) -> SignalKind {
        tokio::select! {
            _ = self.interrupt.recv() => SignalKind::Int,
            _ = self.terminate.recv() => SignalKind::Term,
            _ = self.quit.recv() => SignalKind::Quit,
            _ = self.hangup.recv() => SignalKind::Hup,
        }
    }
}

pub fn forward(group: u32, signal: SignalKind) -> io::Result<()> {
    crate::lifecycle::forward_signal(group, signal, |target, signal| {
        // SAFETY: target is a validated negative process group and kill consumes scalar arguments.
        if unsafe { libc::kill(target, signal) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    })
}

pub async fn wait(
    child: &mut lns_openshell_spike::launch::real::Child,
    signals: &mut Signals,
) -> io::Result<openshell_supervisor_process::process::ProcessStatus> {
    loop {
        tokio::select! {
            status = child.wait() => return status,
            signal = signals.next() => forward(child.pid(), signal)?,
        }
    }
}
