//! MXBattery — Logitech BLE mouse battery monitor for macOS.

#[cfg(target_os = "macos")]
pub mod app;
pub mod battery;
pub mod config;
pub mod device_filter;
pub mod hidpp;
pub mod ipc;
pub mod launchd;
pub mod logging;
#[cfg(target_os = "macos")]
pub mod menubar;
#[cfg(target_os = "macos")]
pub mod prefs_ui;
pub mod notifier;
pub mod paths;
pub mod state;

pub fn run() -> anyhow::Result<()> {
    logging::init();
    tracing::info!(version = %env!("CARGO_PKG_VERSION"), "mxbattery starting");
    #[cfg(target_os = "macos")]
    {
        crate::app::run_daemon()?;
    }
    Ok(())
}

pub fn run_prefs_subcommand() -> anyhow::Result<()> {
    let paths = paths::Paths::standard()?;
    let sock = paths.control_socket();
    let rt = tokio::runtime::Runtime::new()?;
    let posted = rt.block_on(async { ipc::send_open_prefs(&sock).await.is_ok() });
    if posted {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        crate::prefs_ui::run_oneshot()?;
    }
    Ok(())
}

pub fn run_read_subcommand() -> anyhow::Result<()> {
    let paths = paths::Paths::standard()?;
    let s = state::State::load(&paths.state_file())?;
    if let Some(snap) = s.last_seen {
        println!("{}% ({:?})", snap.percent, snap.charging);
    } else {
        println!("(no readings recorded yet — daemon must run first)");
    }
    Ok(())
}
