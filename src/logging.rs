use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub fn init() {
    let filter = EnvFilter::try_from_env("MXBATTERY_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let layer = fmt::layer()
        .with_target(true)
        .with_timer(ChronoLocal::new("%H:%M:%S%.3f".into()))
        .with_writer(std::io::stderr);
    tracing_subscriber::registry().with(filter).with(layer).init();
}
