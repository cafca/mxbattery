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

use crate::hidpp::{self, FrameShape};

#[derive(Clone, Debug)]
pub enum VendorOutcome {
    ResolvedBatteryStatusFeature(u8),
    Charging(ChargingState),
    Ignored,
}

pub fn handle_vendor_bytes(
    shape: FrameShape,
    bytes: &[u8],
    expected_swid_for_resolve: u8,
    feature_index_1000: &mut Option<u8>,
) -> VendorOutcome {
    let Some(frame) = hidpp::decode(shape, bytes) else { return VendorOutcome::Ignored };
    if frame.is_event() {
        if Some(frame.feature_index) == *feature_index_1000 {
            if let Some((_, _, raw)) = hidpp::decode_battery_status(&frame.params) {
                return VendorOutcome::Charging(map_charging(raw));
            }
        }
        return VendorOutcome::Ignored;
    }
    if frame.feature_index == hidpp::ROOT_FEATURE_INDEX
        && frame.function == hidpp::FN_GET_FEATURE
        && frame.swid == expected_swid_for_resolve
    {
        let resolved = frame.params.first().copied().unwrap_or(0);
        if resolved != 0 {
            *feature_index_1000 = Some(resolved);
            return VendorOutcome::ResolvedBatteryStatusFeature(resolved);
        }
    }
    VendorOutcome::Ignored
}

fn map_charging(raw: hidpp::ChargingStateRaw) -> ChargingState {
    match raw {
        hidpp::ChargingStateRaw::Discharging => ChargingState::Discharging,
        hidpp::ChargingStateRaw::Recharging => ChargingState::Recharging,
        hidpp::ChargingStateRaw::ChargeInFinalState => ChargingState::ChargeInFinalState,
        hidpp::ChargingStateRaw::ChargeComplete => ChargingState::ChargeComplete,
        hidpp::ChargingStateRaw::Other(_) => ChargingState::Unknown,
    }
}
