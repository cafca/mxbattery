use crate::state::ChargingState;
use tokio::sync::broadcast;

#[cfg(target_os = "macos")]
pub mod cb;
pub mod mock;

#[derive(Clone, Debug)]
pub enum BatteryEvent {
    Connected { name: String },
    Percent(u8),
    Charging(ChargingState),
    Disconnected,
}

pub trait BatteryBackend: Send + Sync + 'static {
    fn subscribe(&self) -> broadcast::Receiver<BatteryEvent>;
}
