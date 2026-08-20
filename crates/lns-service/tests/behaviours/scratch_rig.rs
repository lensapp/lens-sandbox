use std::path::PathBuf;

use lns_service::run::scratch::{self, RealScratchFs};

#[derive(Debug)]
pub struct ScratchRig {
    pub cache_root: PathBuf,
    pub data_root: PathBuf,
    _tmp: tempfile::TempDir,
    pub exited: Vec<String>,
    pub running: Vec<String>,
    pub reclaimed: Option<u64>,
    next_id: u32,
}

impl ScratchRig {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        Self {
            cache_root: tmp.path().join("cache"),
            data_root: tmp.path().join("data"),
            _tmp: tmp,
            exited: Vec::new(),
            running: Vec::new(),
            reclaimed: None,
            next_id: 1,
        }
    }

    fn alloc_id(&mut self) -> String {
        let id = format!("{:032x}", self.next_id);
        self.next_id += 1;
        id
    }

    fn make_scratch_dir(&self, run_id: &str) {
        let dir = self.run_dir(run_id);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        std::fs::write(dir.join("upper.img"), vec![7u8; 4096]).expect("upper.img");
        std::fs::write(dir.join("console.log"), b"guest boot log").expect("console.log");
    }

    pub fn run_dir(&self, run_id: &str) -> PathBuf {
        self.cache_root.join("runs").join(run_id)
    }

    pub fn add_exited_run(&mut self) -> String {
        let id = self.alloc_id();
        self.make_scratch_dir(&id);
        self.exited.push(id.clone());
        id
    }

    pub fn add_running_run(&mut self) -> String {
        let id = self.alloc_id();
        self.make_scratch_dir(&id);
        self.running.push(id.clone());
        id
    }

    pub fn audit_path(&self, run_id: &str) -> PathBuf {
        self.data_root.join("runs").join(run_id).join("audit.jsonl")
    }

    pub fn add_audit_chain(&self, run_id: &str) {
        let path = self.audit_path(run_id);
        std::fs::create_dir_all(path.parent().expect("audit dir")).expect("audit dir");
        std::fs::write(path, b"{\"event\":\"run_launched\"}\n").expect("audit chain");
    }

    pub fn reclaim_upper(&self, run_id: &str) {
        scratch::reclaim_upper_with(&RealScratchFs, &self.cache_root, run_id);
    }

    pub fn remove(&self, run_id: &str) {
        scratch::remove_dir_with(&RealScratchFs, &self.cache_root, run_id);
    }

    pub fn prune_exited(&mut self) {
        let reclaimed = scratch::prune_with(&RealScratchFs, &self.cache_root, &self.exited);
        self.reclaimed = Some(reclaimed);
    }

    pub fn sweep(&self) {
        scratch::sweep_orphans_with(&RealScratchFs, &self.cache_root, &self.running);
    }
}
