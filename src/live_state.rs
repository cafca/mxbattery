//! Shared live device state, published by the daemon glue task and
//! polled by the preferences window.
//!
//! Single-process design: the daemon and the prefs window run in the
//! same binary, so a global `ArcSwap` is sufficient. The
//! `mxbattery prefs` one-shot path leaves `daemon_running()` at `false`
//! and the snapshot at `default()`, so the UI shows
//! "Daemon not running".

use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwap;

use crate::state::ChargingState;

#[derive(Clone, Debug, Default)]
pub struct LiveDeviceState {
    pub connected: bool,
    pub device_name: Option<String>,
    pub identifier: Option<String>,
    pub last_percent: Option<u8>,
    pub charging: ChargingState,
}

static SNAPSHOT: OnceLock<Arc<ArcSwap<LiveDeviceState>>> = OnceLock::new();
static DAEMON_RUNNING: OnceLock<bool> = OnceLock::new();

pub fn handle() -> Arc<ArcSwap<LiveDeviceState>> {
    SNAPSHOT
        .get_or_init(|| Arc::new(ArcSwap::from_pointee(LiveDeviceState::default())))
        .clone()
}

pub fn mark_daemon_running() {
    let _ = DAEMON_RUNNING.set(true);
}

pub fn daemon_running() -> bool {
    *DAEMON_RUNNING.get().unwrap_or(&false)
}
