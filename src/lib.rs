//! MXBattery — Logitech BLE mouse battery monitor for macOS.

pub fn run() -> anyhow::Result<()> {
    println!("mxbattery v{}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
