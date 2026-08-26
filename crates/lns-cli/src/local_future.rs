/// A boxed future that need not be `Send`: every CLI port is driven on one thread.
pub type LocalBoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + 'a>>;
