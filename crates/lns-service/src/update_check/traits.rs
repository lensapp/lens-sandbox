use super::UpdateStatus;
use anyhow::Result;
use std::future::Future;

pub(super) trait Clock: Send + Sync {
    fn now_unix(&self) -> u64;
}

pub(super) trait Fetcher: Send + Sync {
    fn get_manifest(
        &self,
        url: &str,
        user_agent: &str,
        install_id: &str,
    ) -> impl Future<Output = Result<Vec<u8>>> + Send;
}

pub(super) trait StateStore: Send + Sync {
    fn read_install_id(&self) -> Result<Option<String>>;
    fn write_install_id(&self, id: &str) -> Result<()>;
    fn write_status(&self, status: &UpdateStatus) -> Result<()>;
}
