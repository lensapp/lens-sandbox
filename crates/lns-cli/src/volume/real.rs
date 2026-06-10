use std::path::PathBuf;

use lns_ipc::{Request, Response};

use super::VolumeService;
use crate::integration::LocalBoxFuture;

pub struct RealVolumeService {
    socket: PathBuf,
}

impl RealVolumeService {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
}

impl VolumeService for RealVolumeService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>> {
        Box::pin(async move { crate::service::real::send_request(&self.socket, &req).await })
    }
}
