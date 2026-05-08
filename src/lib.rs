//! MXBattery — Logitech BLE mouse battery monitor for macOS.

pub mod paths;

pub fn run() -> anyhow::Result<()> {
    println!("mxbattery v{}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
