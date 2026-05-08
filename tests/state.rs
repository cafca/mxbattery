use chrono::{Local, NaiveDate, TimeZone};
use mxbattery::state::{BatteryReadingSnapshot, ChargingState, State};

#[test]
fn round_trip_through_json() {
    let s = State {
        last_warn_notified_date: Some(NaiveDate::from_ymd_opt(2026, 5, 8).unwrap()),
        last_critical_notified_at: Some(Local.with_ymd_and_hms(2026, 5, 8, 12, 0, 0).unwrap()),
        mute_until: None,
        last_seen: Some(BatteryReadingSnapshot {
            percent: 17,
            charging: ChargingState::Discharging,
        }),
    };
    let json = serde_json::to_string_pretty(&s).unwrap();
    let s2: State = serde_json::from_str(&json).unwrap();
    assert_eq!(s, s2);
}

#[test]
fn atomic_write_then_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let s = State::default();
    s.save(&path).unwrap();
    let r = State::load(&path).unwrap();
    assert_eq!(s, r);
}

#[test]
fn missing_file_yields_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("does-not-exist.json");
    let r = State::load(&path).unwrap();
    assert_eq!(r, State::default());
}
