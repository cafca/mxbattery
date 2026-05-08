mod decide;
#[cfg(target_os = "macos")]
mod un;

pub use decide::{decide, clear_armed_if_rose, NotificationKind};

#[cfg(target_os = "macos")]
pub fn post(kind: NotificationKind, percent: u8, device_name: &str) {
    let (title, body, id) = match kind {
        NotificationKind::Warn => (
            "Mouse battery low".to_string(),
            format!("{} is at {}%. Consider charging soon.", device_name, percent),
            format!("mxbattery.warn.{}", chrono::Local::now().date_naive()),
        ),
        NotificationKind::Critical => (
            "Mouse battery critical".to_string(),
            format!("{} is at {}%. Charge now.", device_name, percent),
            format!("mxbattery.critical.{}", chrono::Local::now().timestamp()),
        ),
    };
    un::post(&title, &body, &id);
}

#[cfg(target_os = "macos")]
pub fn request_permission() {
    un::request_permission();
}
