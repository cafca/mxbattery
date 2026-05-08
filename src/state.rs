use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChargingState {
    #[default]
    Unknown,
    Discharging,
    Recharging,
    ChargeInFinalState,
    ChargeComplete,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BatteryReadingSnapshot {
    pub percent: u8,
    pub charging: ChargingState,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub last_warn_notified_date: Option<NaiveDate>,
    #[serde(default)]
    pub last_critical_notified_at: Option<DateTime<Local>>,
    #[serde(default)]
    pub mute_until: Option<DateTime<Local>>,
    #[serde(default)]
    pub last_seen: Option<BatteryReadingSnapshot>,
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("json: {0}")] Json(#[from] serde_json::Error),
}

impl State {
    pub fn load(path: &Path) -> Result<Self, StateError> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(serde_json::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), StateError> {
        let body = serde_json::to_vec_pretty(self)?;
        let dir = path.parent().unwrap();
        std::fs::create_dir_all(dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        std::io::Write::write_all(&mut tmp, &body)?;
        tmp.persist(path).map_err(|e| StateError::Io(e.error))?;
        Ok(())
    }
}
