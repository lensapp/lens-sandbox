use lns_ipc::{Request, Response};
use std::time::Instant;

pub async fn run_one_shot(request: &Request, started_at: Instant) -> Response {
    lns_service::ipc::handle_request(request, started_at).await
}
