use crate::config::DeviceFilter;

pub const LOGITECH_VID: u16 = 0x046D;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceProbe {
    pub peripheral_identifier: String,
    pub model_number: Option<String>,
    pub pnp_vid: Option<u16>,
    pub pnp_pid: Option<u16>,
    pub has_logitech_vendor_service: bool,
}

pub fn matches(filter: &DeviceFilter, probe: &DeviceProbe) -> bool {
    match filter {
        DeviceFilter::Specific { identifier } => {
            identifier.eq_ignore_ascii_case(&probe.peripheral_identifier)
        }
        DeviceFilter::AnyMx => probe
            .model_number
            .as_deref()
            .map(|m| m.starts_with("MX "))
            .unwrap_or(false),
        DeviceFilter::AnyLogitech => {
            probe.has_logitech_vendor_service || probe.pnp_vid == Some(LOGITECH_VID)
        }
    }
}
