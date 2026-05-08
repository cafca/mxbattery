//! HID++ 2.0 frame codec for the Logitech BLE-HID++ pipe.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameShape {
    NoDevIdx18,
    WithDevIdx19,
    WithReportId20,
}

impl FrameShape {
    pub const fn frame_len(self) -> usize {
        match self {
            FrameShape::NoDevIdx18 => 18,
            FrameShape::WithDevIdx19 => 19,
            FrameShape::WithReportId20 => 20,
        }
    }

    const fn featidx_off(self) -> usize {
        match self {
            FrameShape::NoDevIdx18 => 0,
            FrameShape::WithDevIdx19 => 1,
            FrameShape::WithReportId20 => 2,
        }
    }
}

pub const ROOT_FEATURE_INDEX: u8 = 0x00;
pub const FN_GET_FEATURE: u8 = 0x00;
pub const FN_GET_BATTERY_LEVEL_STATUS: u8 = 0x00;
pub const FEATURE_BATTERY_STATUS: u16 = 0x1000;
pub const FEATURE_UNIFIED_BATTERY: u16 = 0x1004;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HidppFrame {
    pub feature_index: u8,
    pub function: u8,
    pub swid: u8,
    pub params: Vec<u8>,
}

impl HidppFrame {
    pub fn is_event(&self) -> bool { self.swid == 0 }
}

fn put_header(buf: &mut [u8], shape: FrameShape, feature_index: u8, function: u8, swid: u8) {
    let off = shape.featidx_off();
    if shape == FrameShape::WithReportId20 { buf[0] = 0x11; }
    if matches!(shape, FrameShape::WithDevIdx19 | FrameShape::WithReportId20) {
        buf[off - 1] = 0xFF;
    }
    buf[off] = feature_index;
    buf[off + 1] = (function << 4) | (swid & 0x0F);
}

pub fn encode_get_feature(shape: FrameShape, feature_id: u16, swid: u8) -> Vec<u8> {
    let mut buf = vec![0u8; shape.frame_len()];
    put_header(&mut buf, shape, ROOT_FEATURE_INDEX, FN_GET_FEATURE, swid);
    let off = shape.featidx_off();
    buf[off + 2] = (feature_id >> 8) as u8;
    buf[off + 3] = (feature_id & 0xFF) as u8;
    buf
}

pub fn encode_get_battery_level_status(shape: FrameShape, feature_index: u8, swid: u8) -> Vec<u8> {
    let mut buf = vec![0u8; shape.frame_len()];
    put_header(&mut buf, shape, feature_index, FN_GET_BATTERY_LEVEL_STATUS, swid);
    buf
}

pub fn decode(shape: FrameShape, buf: &[u8]) -> Option<HidppFrame> {
    let off = shape.featidx_off();
    if buf.len() < off + 2 { return None; }
    let feature_index = buf[off];
    let fn_swid = buf[off + 1];
    let function = fn_swid >> 4;
    let swid = fn_swid & 0x0F;
    let params = buf[off + 2..].to_vec();
    Some(HidppFrame { feature_index, function, swid, params })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChargingStateRaw {
    Discharging,
    Recharging,
    ChargeInFinalState,
    ChargeComplete,
    Other(u8),
}

impl ChargingStateRaw {
    pub fn from_byte(b: u8) -> Self {
        match b {
            0 => Self::Discharging,
            1 => Self::Recharging,
            2 => Self::ChargeInFinalState,
            3 => Self::ChargeComplete,
            other => Self::Other(other),
        }
    }
}

pub fn decode_battery_status(params: &[u8]) -> Option<(u8, u8, ChargingStateRaw)> {
    if params.len() < 3 { return None; }
    Some((params[0], params[1], ChargingStateRaw::from_byte(params[2])))
}
