use super::install::{DirEntryView, Fs, WritableFile};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(super) struct Faults {
    pub fail_create_dir_all: Option<io::ErrorKind>,
    pub fail_remove_file: Option<io::ErrorKind>,
    pub fail_create_new: Option<io::ErrorKind>,
    pub fail_write: Option<io::ErrorKind>,
    pub fail_sync: Option<io::ErrorKind>,
    pub fail_rename: Option<io::ErrorKind>,
    pub fail_read: Option<io::ErrorKind>,
    pub fail_read_dir: Option<io::ErrorKind>,
}

#[derive(Default)]
struct State {
    files: BTreeMap<PathBuf, Vec<u8>>,
    extra_entries: BTreeMap<PathBuf, Vec<(OsString, u64)>>,
    sync_calls: Vec<PathBuf>,
}

struct Inner {
    state: Mutex<State>,
    faults: Mutex<Faults>,
}

#[derive(Clone)]
pub(super) struct InMemoryFs(Arc<Inner>);

impl InMemoryFs {
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            state: Mutex::new(State::default()),
            faults: Mutex::new(Faults::default()),
        }))
    }

    pub fn as_arc(&self) -> Arc<dyn Fs> {
        Arc::new(self.clone())
    }

    pub fn inject(&self, mut configure: impl FnMut(&mut Faults)) {
        configure(&mut self.0.faults.lock().unwrap());
    }

    pub fn put(&self, path: &Path, bytes: &[u8]) {
        self.0
            .state
            .lock()
            .unwrap()
            .files
            .insert(path.to_path_buf(), bytes.to_vec());
    }

    pub fn put_dir_entry(&self, dir: &Path, name: OsString, size: u64) {
        self.0
            .state
            .lock()
            .unwrap()
            .extra_entries
            .entry(dir.to_path_buf())
            .or_default()
            .push((name, size));
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.0.state.lock().unwrap().files.contains_key(path)
    }

    pub fn read_bytes(&self, path: &Path) -> Option<Vec<u8>> {
        self.0.state.lock().unwrap().files.get(path).cloned()
    }

    pub fn sync_calls(&self) -> Vec<PathBuf> {
        self.0.state.lock().unwrap().sync_calls.clone()
    }
}

fn take<T>(slot: &mut Option<T>) -> Option<T> {
    slot.take()
}

impl Fs for InMemoryFs {
    fn create_dir_all(&self, _p: &Path) -> io::Result<()> {
        if let Some(kind) = take(&mut self.0.faults.lock().unwrap().fail_create_dir_all) {
            return Err(io::Error::from(kind));
        }
        Ok(())
    }
    fn remove_file(&self, p: &Path) -> io::Result<()> {
        if let Some(kind) = take(&mut self.0.faults.lock().unwrap().fail_remove_file) {
            return Err(io::Error::from(kind));
        }
        let mut st = self.0.state.lock().unwrap();
        if st.files.remove(p).is_some() {
            Ok(())
        } else {
            Err(io::Error::from(io::ErrorKind::NotFound))
        }
    }
    fn create_new(&self, p: &Path) -> io::Result<Box<dyn WritableFile>> {
        if let Some(kind) = take(&mut self.0.faults.lock().unwrap().fail_create_new) {
            return Err(io::Error::from(kind));
        }
        let mut st = self.0.state.lock().unwrap();
        if st.files.contains_key(p) {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        st.files.insert(p.to_path_buf(), Vec::new());
        Ok(Box::new(InMemoryFile {
            inner: self.0.clone(),
            path: p.to_path_buf(),
        }))
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        if let Some(kind) = take(&mut self.0.faults.lock().unwrap().fail_rename) {
            return Err(io::Error::from(kind));
        }
        let mut st = self.0.state.lock().unwrap();
        let bytes = st
            .files
            .remove(from)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        st.files.insert(to.to_path_buf(), bytes);
        Ok(())
    }
    fn is_file(&self, p: &Path) -> bool {
        self.0.state.lock().unwrap().files.contains_key(p)
    }
    fn is_dir(&self, p: &Path) -> bool {
        let st = self.0.state.lock().unwrap();
        st.files.keys().any(|k| k.parent() == Some(p)) || st.extra_entries.contains_key(p)
    }
    fn read(&self, p: &Path) -> io::Result<Vec<u8>> {
        if let Some(kind) = take(&mut self.0.faults.lock().unwrap().fail_read) {
            return Err(io::Error::from(kind));
        }
        self.0
            .state
            .lock()
            .unwrap()
            .files
            .get(p)
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
    fn read_dir_entries(&self, p: &Path) -> io::Result<Vec<DirEntryView>> {
        if let Some(kind) = take(&mut self.0.faults.lock().unwrap().fail_read_dir) {
            return Err(io::Error::from(kind));
        }
        let st = self.0.state.lock().unwrap();
        let mut out = Vec::new();
        for (path, bytes) in &st.files {
            if path.parent() == Some(p)
                && let Some(name) = path.file_name()
            {
                out.push(DirEntryView {
                    name: name.to_os_string(),
                    path: path.clone(),
                    size: bytes.len() as u64,
                });
            }
        }
        if let Some(extras) = st.extra_entries.get(p) {
            for (name, size) in extras {
                let path = p.join(name);
                out.push(DirEntryView {
                    name: name.clone(),
                    path,
                    size: *size,
                });
            }
        }
        Ok(out)
    }
}

struct InMemoryFile {
    inner: Arc<Inner>,
    path: PathBuf,
}

impl Write for InMemoryFile {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if let Some(kind) = take(&mut self.inner.faults.lock().unwrap().fail_write) {
            return Err(io::Error::from(kind));
        }
        self.inner
            .state
            .lock()
            .unwrap()
            .files
            .get_mut(&self.path)
            .expect("fake invariant: file removed while handle open")
            .extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl WritableFile for InMemoryFile {
    fn sync_all(&self) -> io::Result<()> {
        if let Some(kind) = take(&mut self.inner.faults.lock().unwrap().fail_sync) {
            return Err(io::Error::from(kind));
        }
        self.inner
            .state
            .lock()
            .unwrap()
            .sync_calls
            .push(self.path.clone());
        Ok(())
    }
}
