use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub schema_version: u32,
    pub device: DeviceFilter,
    pub thresholds: Thresholds,
    pub cadence: Cadence,
    pub menubar: MenubarConfig,
    pub autostart: Autostart,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum DeviceFilter {
    Specific { identifier: String },
    AnyMx,
    AnyLogitech,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    pub warn: u8,
    pub critical: u8,
    pub rearm_hysteresis: u8,
    #[serde(default = "yes")]
    pub warn_enabled: bool,
    #[serde(default = "yes")]
    pub critical_enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Cadence {
    #[serde(with = "humantime_serde")]
    pub warn_period: Duration,
    #[serde(with = "humantime_serde")]
    pub critical_period: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MenubarConfig {
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Autostart {
    pub enabled: bool,
}

fn yes() -> bool { true }

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            device: DeviceFilter::AnyMx,
            thresholds: Thresholds {
                warn: 20,
                critical: 5,
                rearm_hysteresis: 5,
                warn_enabled: true,
                critical_enabled: true,
            },
            cadence: Cadence {
                warn_period: Duration::from_secs(24 * 60 * 60),
                critical_period: Duration::from_secs(30 * 60),
            },
            menubar: MenubarConfig { enabled: true },
            autostart: Autostart { enabled: false },
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invariant violated: {0}")]
    InvariantViolated(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl Config {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.thresholds.critical >= self.thresholds.warn {
            return Err(ConfigError::InvariantViolated(format!(
                "critical ({}) must be < warn ({})",
                self.thresholds.critical, self.thresholds.warn
            )));
        }
        if self.thresholds.warn > 99 || self.thresholds.critical == 0 {
            return Err(ConfigError::InvariantViolated(
                "warn must be 1..=99, critical must be >=1".into(),
            ));
        }
        Ok(())
    }

    pub fn load(path: &std::path::Path) -> Result<Self, ConfigError> {
        let body = std::fs::read_to_string(path)?;
        let cfg: Self = toml::from_str(&body)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), ConfigError> {
        self.validate()?;
        let body = toml::to_string_pretty(self).expect("serialise");
        let dir = path.parent().unwrap();
        std::fs::create_dir_all(dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        std::io::Write::write_all(&mut tmp, body.as_bytes())?;
        tmp.persist(path).map_err(|e| ConfigError::Io(e.error))?;
        Ok(())
    }
}

use arc_swap::ArcSwap;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use std::path::PathBuf;
use std::sync::Arc;

pub struct ConfigWatcher {
    current: Arc<ArcSwap<Config>>,
    _watcher: RecommendedWatcher,
    _path: PathBuf,
}

impl ConfigWatcher {
    pub fn start(path: PathBuf) -> Result<Self, ConfigError> {
        let initial = Config::load(&path)?;
        let current = Arc::new(ArcSwap::from_pointee(initial));
        let cur = current.clone();
        let watch_path = std::fs::canonicalize(&path).unwrap_or(path.clone());
        let dir = path.parent().unwrap().to_path_buf();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else { return };
            if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)) {
                return;
            }
            if !event.paths.iter().any(|p| p == &watch_path) { return; }
            match Config::load(&watch_path) {
                Ok(c) => {
                    tracing::info!("config reloaded");
                    cur.store(Arc::new(c));
                }
                Err(e) => tracing::warn!(?e, "ignoring bad config; keeping previous"),
            }
        })?;
        watcher.watch(&dir, RecursiveMode::NonRecursive)?;
        Ok(Self { current, _watcher: watcher, _path: path })
    }

    pub fn current(&self) -> Arc<Config> { self.current.load_full() }

    pub fn handle(&self) -> Arc<ArcSwap<Config>> { self.current.clone() }
}

impl From<notify::Error> for ConfigError {
    fn from(e: notify::Error) -> Self { ConfigError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)) }
}
