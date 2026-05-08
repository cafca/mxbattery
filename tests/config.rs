use mxbattery::config::{Config, DeviceFilter, ConfigError};

const VALID: &str = r#"
schema_version = 1

[device]
mode = "specific"
identifier = "798C4CBB-C8BE-9C3C-F286-195F08C01978"

[thresholds]
warn = 20
critical = 5
rearm_hysteresis = 5
warn_enabled = true
critical_enabled = true

[cadence]
warn_period = "24h"
critical_period = "30m"

[menubar]
enabled = true

[autostart]
enabled = false
"#;

#[test]
fn parses_a_valid_config() {
    let cfg: Config = toml::from_str(VALID).unwrap();
    assert!(matches!(cfg.device, DeviceFilter::Specific { .. }));
    assert_eq!(cfg.thresholds.warn, 20);
    assert_eq!(cfg.thresholds.critical, 5);
    assert_eq!(cfg.cadence.critical_period, std::time::Duration::from_secs(30 * 60));
    assert_eq!(cfg.cadence.warn_period,    std::time::Duration::from_secs(24 * 60 * 60));
    assert!(cfg.thresholds.warn_enabled);
    assert!(cfg.thresholds.critical_enabled);
}

#[test]
fn rejects_critical_ge_warn() {
    let bad = VALID.replace("critical = 5", "critical = 30");
    let cfg: Config = toml::from_str(&bad).unwrap();
    let err = cfg.validate().unwrap_err();
    assert!(matches!(err, ConfigError::InvariantViolated(_)));
}

#[test]
fn defaults_are_sensible() {
    let c = Config::default();
    assert_eq!(c.thresholds.warn, 20);
    assert_eq!(c.thresholds.critical, 5);
    assert!(c.thresholds.warn_enabled);
    assert!(c.thresholds.critical_enabled);
    assert_eq!(c.cadence.warn_period.as_secs(), 24 * 60 * 60);
    assert_eq!(c.cadence.critical_period.as_secs(), 30 * 60);
    assert!(c.menubar.enabled);
    assert!(matches!(c.device, DeviceFilter::AnyMx));
    c.validate().unwrap();
}

#[test]
fn round_trips_through_toml() {
    let c = Config::default();
    let s = toml::to_string(&c).unwrap();
    let c2: Config = toml::from_str(&s).unwrap();
    assert_eq!(c, c2);
}
