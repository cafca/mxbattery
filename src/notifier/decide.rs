use crate::config::Config;
use crate::state::{BatteryReadingSnapshot, ChargingState, State};
use chrono::{DateTime, Local};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationKind { Warn, Critical }

pub fn decide(
    reading: &BatteryReadingSnapshot,
    state: &State,
    cfg: &Config,
    now: DateTime<Local>,
) -> Option<NotificationKind> {
    if let Some(until) = state.mute_until {
        if now < until { return None; }
    }
    if reading.charging != ChargingState::Discharging {
        return None;
    }

    // Check if we're in critical territory
    if cfg.thresholds.critical_enabled && reading.percent <= cfg.thresholds.critical {
        let critical_due = state
            .last_critical_notified_at
            .map(|t| {
                let elapsed = now - t;
                let elapsed_secs = elapsed.num_seconds() as u64;
                let period_secs = cfg.cadence.critical_period.as_secs();
                elapsed_secs >= period_secs
            })
            .unwrap_or(true);

        if critical_due { return Some(NotificationKind::Critical); }
        // If we're critical but not due, suppress all notifications
        if state.last_critical_notified_at.is_some() { return None; }
    }

    let warn_due = cfg.thresholds.warn_enabled
        && reading.percent <= cfg.thresholds.warn
        && state
            .last_warn_notified_date
            .map(|d| d != now.date_naive())
            .unwrap_or(true);

    if warn_due { Some(NotificationKind::Warn) } else { None }
}

pub fn clear_armed_if_rose(
    reading: &BatteryReadingSnapshot,
    state: &mut State,
    cfg: &Config,
) {
    let warn_re = cfg.thresholds.warn.saturating_add(cfg.thresholds.rearm_hysteresis);
    if reading.percent >= warn_re {
        state.last_warn_notified_date = None;
    }
    let crit_re = cfg.thresholds.critical.saturating_add(cfg.thresholds.rearm_hysteresis);
    if reading.percent >= crit_re {
        state.last_critical_notified_at = None;
    }
}
