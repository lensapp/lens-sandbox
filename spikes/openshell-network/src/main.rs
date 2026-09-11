use lns_openshell_spike::{Error, Gate};
use openshell_supervisor_network::opa::NetworkInput;
use tokio::sync::mpsc;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Error> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let gate = Gate::new(tx)?;
    let input = NetworkInput {
        host: "api.example.com".into(),
        port: 443,
        binary_path: "/usr/bin/curl".into(),
        binary_sha256: String::new(),
        ancestors: vec![],
        cmdline_paths: vec![],
    };
    let mut authorization = Box::pin(gate.authorize(&input));
    let pending = tokio::select! {
        result = &mut authorization => return Err(format!("request completed before approval: {result:?}").into()),
        pending = rx.recv() => pending.ok_or("approval channel closed")?,
    };
    pending.answer.send(true).map_err(|_| "request cancelled")?;
    if !authorization.await? {
        return Err("approved request was denied".into());
    }
    println!(
        "OpenShell policy engine: held request resumed after approval (no network connection opened)"
    );
    Ok(())
}
