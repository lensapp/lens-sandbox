use std::path::PathBuf;

use lns_ipc::{Request, Response};

use super::ImageService;
use crate::integration::LocalBoxFuture;

pub struct RealImageService {
    socket: PathBuf,
}

impl RealImageService {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
}

impl ImageService for RealImageService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>> {
        Box::pin(async move { crate::service::real::send_request(&self.socket, &req).await })
    }
}
