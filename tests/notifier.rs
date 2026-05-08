use chrono::{Local, TimeZone};
use mxbattery::config::Config;
use mxbattery::notifier::{decide, NotificationKind};
use mxbattery::state::{BatteryReadingSnapshot, ChargingState, State};

fn cfg() -> Config { Config::default() }
fn now() -> chrono::DateTime<Local> { Local.with_ymd_and_hms(2026, 5, 8, 14, 0, 0).unwrap() }

fn discharging(p: u8) -> BatteryReadingSnapshot {
    BatteryReadingSnapshot { percent: p, charging: ChargingState::Discharging }
}

#[test]
fn at_30_no_notification() {
    let s = State::default();
    let r = decide(&discharging(30), &s, &cfg(), now());
    assert!(r.is_none());
}

#[test]
fn at_20_warn_fires_once_per_day() {
    let mut s = State::default();
    let r = decide(&discharging(20), &s, &cfg(), now()).unwrap();
    assert_eq!(r, NotificationKind::Warn);
    s.last_warn_notified_date = Some(now().date_naive());
    let r2 = decide(&discharging(20), &s, &cfg(), now());
    assert!(r2.is_none(), "second call same day should be silent");
}

#[test]
fn at_5_critical_wins_over_warn() {
    let s = State::default();
    let r = decide(&discharging(5), &s, &cfg(), now()).unwrap();
    assert_eq!(r, NotificationKind::Critical);
}

#[test]
fn critical_re_fires_after_period() {
    let mut s = State::default();
    s.last_critical_notified_at = Some(now() - chrono::Duration::minutes(35));
    let r = decide(&discharging(4), &s, &cfg(), now()).unwrap();
    assert_eq!(r, NotificationKind::Critical);
}

#[test]
fn critical_silent_within_period() {
    let mut s = State::default();
    s.last_critical_notified_at = Some(now() - chrono::Duration::minutes(10));
    let r = decide(&discharging(4), &s, &cfg(), now());
    assert!(r.is_none());
}

#[test]
fn charging_silences_everything() {
    let s = State::default();
    let r = decide(
        &BatteryReadingSnapshot { percent: 4, charging: ChargingState::Recharging },
        &s, &cfg(), now(),
    );
    assert!(r.is_none());
}

#[test]
fn mute_until_silences_everything() {
    let mut s = State::default();
    s.mute_until = Some(now() + chrono::Duration::hours(1));
    let r = decide(&discharging(4), &s, &cfg(), now());
    assert!(r.is_none());
}

#[test]
fn warn_disabled_skips_warn_but_critical_still_fires() {
    let mut c = cfg();
    c.thresholds.warn_enabled = false;
    let s = State::default();
    assert!(decide(&discharging(15), &s, &c, now()).is_none());
    assert_eq!(decide(&discharging(4), &s, &c, now()).unwrap(), NotificationKind::Critical);
}

#[test]
fn critical_disabled_skips_critical_but_warn_still_fires() {
    let mut c = cfg();
    c.thresholds.critical_enabled = false;
    let s = State::default();
    assert_eq!(decide(&discharging(4), &s, &c, now()).unwrap(), NotificationKind::Warn);
}

#[test]
fn warn_re_arms_after_rise_above_threshold_plus_hysteresis() {
    use mxbattery::notifier::clear_armed_if_rose;
    let mut s = State::default();
    s.last_warn_notified_date = Some(now().date_naive());
    clear_armed_if_rose(&BatteryReadingSnapshot { percent: 25, charging: ChargingState::Discharging }, &mut s, &cfg());
    assert_eq!(s.last_warn_notified_date, None, "rising above warn+hysteresis re-arms warn");
}
