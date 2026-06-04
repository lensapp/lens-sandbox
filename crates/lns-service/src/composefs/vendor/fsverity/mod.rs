//! Linux fs-verity hash value support.
//!
//! This vendored subset keeps the hash value trait and algorithm metadata used
//! by the EROFS writer, and intentionally omits the Linux ioctl implementation.

mod hashvalue;

pub use hashvalue::{
    Algorithm, AlgorithmParseError, DEFAULT_LG_BLOCKSIZE, FsVerityHashValue, Sha256HashValue,
    Sha512HashValue,
};
