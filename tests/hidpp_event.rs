use mxbattery::battery::{handle_vendor_bytes, VendorOutcome};
use mxbattery::hidpp::FrameShape;
use mxbattery::state::ChargingState;

#[test]
fn dispatch_get_feature_response() {
    let bytes = hex::decode("000e0800010000000000000000000000000000").unwrap();
    let mut feat_1000_idx = None;
    let out = handle_vendor_bytes(FrameShape::NoDevIdx18, &bytes, 0xE, &mut feat_1000_idx);
    assert!(matches!(
        out,
        VendorOutcome::ResolvedBatteryStatusFeature(0x08)
    ));
    assert_eq!(feat_1000_idx, Some(0x08));
}

#[test]
fn dispatch_battery_event_charging() {
    let bytes = hex::decode("08000000010000000000000000000000000000").unwrap();
    let mut idx = Some(0x08);
    let out = handle_vendor_bytes(FrameShape::NoDevIdx18, &bytes, 0, &mut idx);
    assert!(matches!(
        out,
        VendorOutcome::Charging(ChargingState::Recharging)
    ));
}

#[test]
fn dispatch_battery_event_discharging() {
    let bytes = hex::decode("08006432000000000000000000000000000000").unwrap();
    let mut idx = Some(0x08);
    let out = handle_vendor_bytes(FrameShape::NoDevIdx18, &bytes, 0, &mut idx);
    assert!(matches!(
        out,
        VendorOutcome::Charging(ChargingState::Discharging)
    ));
}

#[test]
fn dispatch_unknown_swid_is_ignored() {
    let bytes = hex::decode("0a01ff0000000000000000000000000000000000").unwrap();
    let mut idx = Some(0x08);
    let out = handle_vendor_bytes(FrameShape::NoDevIdx18, &bytes, 0, &mut idx);
    assert!(matches!(out, VendorOutcome::Ignored));
}
