use lns_openshell_spike::{
    Error,
    launch::real::{Child, ProcessIo},
};

pub async fn run(
    mut child: Child,
    io: ProcessIo,
    mut signals: crate::signals::Signals,
) -> Result<i32, Error> {
    if matches!(io, ProcessIo::Pty(_)) {
        return crate::terminal::real::run(child, io, signals).await;
    }
    let ProcessIo::Pipes {
        mut stdin,
        mut stdout,
        mut stderr,
    } = io
    else {
        return Err("expected workload pipes".into());
    };
    let input =
        tokio::spawn(async move { tokio::io::copy(&mut tokio::io::stdin(), &mut stdin).await });
    let output =
        tokio::spawn(async move { tokio::io::copy(&mut stdout, &mut tokio::io::stdout()).await });
    let errors =
        tokio::spawn(async move { tokio::io::copy(&mut stderr, &mut tokio::io::stderr()).await });
    let status = crate::signals::wait(&mut child, &mut signals).await?;
    input.abort();
    output.await??;
    errors.await??;
    Ok(crate::lifecycle::exit_code(
        status.exit_code(),
        status.signal(),
    ))
}
