use std::io::Cursor;
use std::sync::{Arc, Mutex};

use lns_service::composefs::descriptor::BuiltDescriptor;
use tempfile::TempDir;

#[derive(Debug)]
pub struct AssemblyRig {
    pub dir: TempDir,
    pub layers: Vec<Vec<u8>>,
    pub events: Arc<Mutex<Vec<(u64, u64)>>>,
    pub built: Option<BuiltDescriptor>,
    pub rebuilt: Option<BuiltDescriptor>,
}

impl AssemblyRig {
    pub fn with_layer_sizes(sizes: &[usize]) -> Self {
        let layers = sizes
            .iter()
            .enumerate()
            .map(|(i, size)| tar_layer_of(*size, &format!("file-{i}")))
            .collect();
        Self {
            dir: TempDir::new().expect("create tempdir"),
            layers,
            events: Arc::new(Mutex::new(Vec::new())),
            built: None,
            rebuilt: None,
        }
    }

    pub fn layer_digests(&self) -> Vec<String> {
        (0..self.layers.len())
            .map(|i| format!("sha256:layer-{i}"))
            .collect()
    }
}

/// A plain tar archive of exactly `total` bytes: one 512-byte header, one content run, one 1024-byte end-of-archive marker.
fn tar_layer_of(total: usize, name: &str) -> Vec<u8> {
    let content_len = total - 512 - 1024;
    let content = vec![b'x'; content_len];
    let mut bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(Cursor::new(&mut bytes));
        let mut header = tar::Header::new_ustar();
        header.set_path(name).expect("tar path");
        header.set_size(content_len as u64);
        header.set_mode(0o644);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder
            .append(&header, Cursor::new(&content[..]))
            .expect("append tar entry");
        builder.finish().expect("finish tar");
    }
    assert_eq!(
        bytes.len(),
        total,
        "tar fixture must be exactly {total} bytes"
    );
    bytes
}
