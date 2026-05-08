use mxbattery::config::DeviceFilter;
use mxbattery::device_filter::{matches, DeviceProbe};

fn mx_master_3() -> DeviceProbe {
    DeviceProbe {
        peripheral_identifier: "798C4CBB-C8BE-9C3C-F286-195F08C01978".into(),
        model_number: Some("MX Master 3".into()),
        pnp_vid: Some(0x046D),
        pnp_pid: Some(0xB023),
        has_logitech_vendor_service: true,
    }
}

fn airpods_pro() -> DeviceProbe {
    DeviceProbe {
        peripheral_identifier: "30821691-5D68-0000-0000-000000000000".into(),
        model_number: Some("AirPods Pro".into()),
        pnp_vid: Some(0x004C),
        pnp_pid: Some(0x2014),
        has_logitech_vendor_service: false,
    }
}

#[test]
fn specific_matches_by_identifier() {
    let p = mx_master_3();
    let f = DeviceFilter::Specific {
        identifier: p.peripheral_identifier.clone(),
    };
    assert!(matches(&f, &p));
}

#[test]
fn any_mx_matches_model_number_prefix() {
    assert!(matches(&DeviceFilter::AnyMx, &mx_master_3()));
    assert!(!matches(&DeviceFilter::AnyMx, &airpods_pro()));
}

#[test]
fn any_logitech_matches_vendor_service_or_pnp_vid() {
    assert!(matches(&DeviceFilter::AnyLogitech, &mx_master_3()));
    assert!(!matches(&DeviceFilter::AnyLogitech, &airpods_pro()));
}
