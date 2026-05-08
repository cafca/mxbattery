use mxbattery::hidpp::{decode, encode_get_battery_level_status, encode_get_feature, FrameShape};

const SHAPE: FrameShape = FrameShape::NoDevIdx18;

#[test]
fn encode_get_feature_18b_no_devidx() {
    let f = encode_get_feature(SHAPE, 0x1000, 0xE);
    assert_eq!(f.len(), 18);
    assert_eq!(&f[..4], &[0x00, 0x0E, 0x10, 0x00]);
    assert!(f[4..].iter().all(|b| *b == 0));
}

#[test]
fn encode_get_battery_level_status_18b_no_devidx() {
    let f = encode_get_battery_level_status(SHAPE, 0x08, 0xF);
    assert_eq!(f.len(), 18);
    assert_eq!(&f[..2], &[0x08, 0x0F]);
    assert!(f[2..].iter().all(|b| *b == 0));
}

#[test]
fn decode_get_feature_response() {
    let buf = hex::decode("000e0800010000000000000000000000000000").unwrap();
    let f = decode(SHAPE, &buf).unwrap();
    assert_eq!(f.feature_index, 0x00);
    assert_eq!(f.function, 0);
    assert_eq!(f.swid, 0xE);
    assert_eq!(f.params[0], 0x08);
}

#[test]
fn decode_unsolicited_event_swid_zero() {
    let buf = hex::decode("08006432000000000000000000000000000000").unwrap();
    let f = decode(SHAPE, &buf).unwrap();
    assert_eq!(f.feature_index, 0x08);
    assert_eq!(f.function, 0);
    assert_eq!(f.swid, 0);
    assert!(f.is_event());
    assert_eq!(f.params[0], 0x64);
    assert_eq!(f.params[1], 0x32);
    assert_eq!(f.params[2], 0x00);
}

#[test]
fn decode_short_buffer_returns_none() {
    let buf = vec![0u8; 1];
    assert!(decode(SHAPE, &buf).is_none());
}
