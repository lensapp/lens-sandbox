//! Vendored composefs-rs encoder subset.
//!
//! This module intentionally keeps the upstream namespace shape for the EROFS
//! writer while omitting Linux-only fs-verity ioctl and mount/runtime pieces.

#![allow(dead_code, unused_imports, clippy::all)]

pub mod erofs;
pub mod fsverity;
pub mod generic_tree;
pub mod tree;

/// Maximum symlink target length in bytes.
///
/// Copied from composefs-rs v0.4.0 `src/lib.rs`; the writer enforces this
/// bound before emitting inline symlink content.
pub const SYMLINK_MAX: usize = 1024;
