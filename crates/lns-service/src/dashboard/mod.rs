pub mod approvals;
mod filter;
#[cfg(not(target_os = "macos"))]
mod format;
pub mod live;
pub(crate) mod sandboxes;

pub use filter::{Filters, KINDS, visible_indices};

pub use lns_ipc::DashboardSandbox as Sandbox;

#[cfg(not(target_os = "macos"))]
mod desktop;
#[cfg(not(target_os = "macos"))]
pub use desktop::*;
