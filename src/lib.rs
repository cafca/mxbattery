//! MXBattery — Logitech BLE mouse battery monitor for macOS.

pub mod config;
pub mod device_filter;
pub mod hidpp;
pub mod logging;
pub mod paths;
pub mod state;

pub fn run() -> anyhow::Result<()> {
    logging::init();
    tracing::info!(version = %env!("CARGO_PKG_VERSION"), "mxbattery starting");
    Ok(())
}
