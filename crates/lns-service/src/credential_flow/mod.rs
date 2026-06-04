//! Wire frames live in [`crate::approval_flow::protocol`] so a single `HostFrame` / `GuestFrame` carries both the network and credential flows.

pub mod detection;
pub mod notification;
pub mod providers;
pub mod registry;
pub mod session;
pub mod store;
pub mod watcher;
