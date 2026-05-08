# MXBattery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `MXBattery`, a small Rust macOS app that monitors a Logitech BLE mouse's battery level and charging state via CoreBluetooth, fires native notifications at configurable thresholds, and optionally shows a menu bar icon.

**Architecture:** Single Rust binary packaged as `MXBattery.app`, run under a per-user LaunchAgent with `LSUIElement=1`. Battery percent comes from BLE Battery Service `0x2A19`; charging state comes from HID++ 2.0 over the device's Logitech vendor GATT characteristic. Both push events spontaneously, so the app is fully event-driven (no polling). Notifications, menu bar icon, and preferences window all run inside the same `NSApplication` run loop.

**Tech Stack:** Rust 2021, `objc2` family (objc2-core-bluetooth, objc2-app-kit, objc2-user-notifications, objc2-foundation), `tokio` for channels and signal handling, `serde`/`toml`/`serde_json` for config and state, `notify` for FSEvents, `arc-swap` for hot-reloadable config, `clap` for the CLI, `tracing` for logs.

**Reference:** Design spec at `docs/superpowers/specs/2026-05-08-mxbattery-design.md`. Probes confirming the data path live in `probe-b/`.

---

## File Structure

```
Cargo.toml
build.rs                      # writes a build-time CFBundleVersion file
src/
  main.rs                     # CLI entry, dispatches subcommands
  lib.rs                      # crate root, module declarations
  paths.rs                    # resolves config/state/socket dirs
  config.rs                   # Config types, TOML parse, hot-reload
  state.rs                    # State types, atomic JSON I/O
  hidpp.rs                    # HID++ frame codec (pure)
  device_filter.rs            # Device matching logic
  notifier/
    mod.rs                    # decide() pure function + posting wrapper
    decide.rs                 # pure decision logic (private)
  battery/
    mod.rs                    # BatteryBackend trait, BatteryReading type
    mock.rs                   # MockBackend for tests
    cb.rs                     # objc2-core-bluetooth implementation
  menubar.rs                  # NSStatusItem + custom-drawn icon + click menu
  prefs_ui.rs                 # NSWindow + form + validation
  ipc.rs                      # Unix socket: single-instance + control
  launchd.rs                  # install/uninstall the LaunchAgent
  app.rs                      # NSApplication wiring, glue, run loop

tests/
  hidpp.rs                    # encode/decode fixtures
  notifier.rs                 # decide() table tests
  config.rs                   # parse + invariant tests
  state.rs                    # atomic write tests
  device_filter.rs            # match logic tests

tools/
  make-app.sh                 # bundle assembly
  Info.plist.template
  com.vincentahrend.mxbattery-app.plist.template
  app-icon-source.svg         # 1024×1024 source
  build-icon.sh               # generates AppIcon.icns from svg

docs/superpowers/specs/2026-05-08-mxbattery-design.md   # exists
docs/superpowers/plans/2026-05-08-mxbattery.md          # this plan
```

Phases below are sequential; each phase ends with the project in a runnable, committable state.

---

## Phase 0 — Scaffolding

### Task 1: Cargo project + dependencies

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/lib.rs`
- Create: `.gitignore`
- Create: `rust-toolchain.toml`

- [ ] **Step 1: Create the Cargo project skeleton**

Write `Cargo.toml`:

```toml
[package]
name = "mxbattery"
version = "0.1.0"
edition = "2021"
rust-version = "1.78"
license = "MIT"
description = "Battery level + charging notifier for Logitech BLE mice on macOS"
repository = "https://github.com/vincentahrend/MXBattery"
default-run = "mxbattery"

[[bin]]
name = "mxbattery"
path = "src/main.rs"

[lib]
name = "mxbattery"
path = "src/lib.rs"

[dependencies]
objc2 = "0.5"
objc2-foundation = { version = "0.2", features = ["all"] }
objc2-app-kit = { version = "0.2", features = ["all"] }
objc2-core-bluetooth = { version = "0.2", features = ["all"] }
objc2-user-notifications = { version = "0.2", features = ["all"] }
tokio = { version = "1", features = ["rt", "rt-multi-thread", "sync", "macros", "signal", "time", "io-util", "net", "fs"] }
serde = { version = "1", features = ["derive"] }
toml = "0.8"
serde_json = "1"
arc-swap = "1"
notify = "6"
chrono = { version = "0.4", features = ["serde"] }
clap = { version = "4", features = ["derive"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt", "time"] }
tempfile = "3"
dirs = "5"
anyhow = "1"
thiserror = "1"
humantime-serde = "1"

[dev-dependencies]
pretty_assertions = "1"

[profile.release]
lto = "thin"
codegen-units = 1
strip = "symbols"
```

> **Note for the engineer:** crate versions are floor pins; if `cargo build` complains about a version that no longer exists on crates.io, bump to the nearest current release and adjust API calls.

- [ ] **Step 2: Write minimal `src/main.rs`**

```rust
fn main() -> anyhow::Result<()> {
    mxbattery::run()
}
```

- [ ] **Step 3: Write minimal `src/lib.rs`**

```rust
//! MXBattery — Logitech BLE mouse battery monitor for macOS.

pub fn run() -> anyhow::Result<()> {
    println!("mxbattery v{}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
```

- [ ] **Step 4: Write `.gitignore`**

```
/target
.DS_Store
*.swp
/probe-a/probe
/probe-b/logi-mx-*
/probe-b/main_*.swift.o
/probe-b/*.dSYM
```

- [ ] **Step 5: Write `rust-toolchain.toml`**

```toml
[toolchain]
channel = "1.78"
components = ["clippy", "rustfmt"]
targets = ["aarch64-apple-darwin"]
```

- [ ] **Step 6: Verify it builds**

Run: `cargo run --release`
Expected: prints `mxbattery v0.1.0`.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs src/lib.rs .gitignore rust-toolchain.toml
git commit -m "scaffold: cargo project + dependencies"
```

---

### Task 2: Paths module

**Files:**
- Create: `src/paths.rs`
- Modify: `src/lib.rs`
- Test: `tests/paths.rs`

The `paths` module owns every filesystem location the app touches. Centralising avoids string-duplication bugs and makes tests redirectable.

- [ ] **Step 1: Write the failing test**

Create `tests/paths.rs`:

```rust
use mxbattery::paths::Paths;
use std::path::PathBuf;

#[test]
fn paths_under_app_support() {
    let p = Paths::with_root(PathBuf::from("/tmp/mxb-test"));
    assert_eq!(p.config_file(), PathBuf::from("/tmp/mxb-test/config.toml"));
    assert_eq!(p.state_file(), PathBuf::from("/tmp/mxb-test/state.json"));
    assert_eq!(p.control_socket(), PathBuf::from("/tmp/mxb-test/control.sock"));
    assert_eq!(p.app_support_dir(), PathBuf::from("/tmp/mxb-test"));
}

#[test]
fn launch_agent_path_uses_bundle_id() {
    let p = Paths::with_root(PathBuf::from("/tmp/mxb-test"));
    let la = p.launch_agent_plist();
    assert!(la.ends_with("Library/LaunchAgents/com.vincentahrend.mxbattery-app.plist"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test paths`
Expected: compile error (`Paths` doesn't exist).

- [ ] **Step 3: Implement `src/paths.rs`**

```rust
use std::path::{Path, PathBuf};

pub const BUNDLE_ID: &str = "com.vincentahrend.mxbattery-app";
pub const APP_SUPPORT_DIRNAME: &str = "MXBattery";

#[derive(Clone, Debug)]
pub struct Paths {
    root: PathBuf,
    home: PathBuf,
}

impl Paths {
    /// Resolve the standard locations using the user's HOME.
    pub fn standard() -> anyhow::Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("HOME not set"))?;
        let root = home.join("Library/Application Support").join(APP_SUPPORT_DIRNAME);
        Ok(Self { root, home })
    }

    /// Override paths for tests. The given root is used as both the application-support
    /// directory and (synthetically) the home root for the LaunchAgent path.
    pub fn with_root(root: PathBuf) -> Self {
        Self { home: root.clone(), root }
    }

    pub fn app_support_dir(&self) -> PathBuf { self.root.clone() }
    pub fn config_file(&self) -> PathBuf    { self.root.join("config.toml") }
    pub fn state_file(&self) -> PathBuf     { self.root.join("state.json") }
    pub fn control_socket(&self) -> PathBuf { self.root.join("control.sock") }

    pub fn launch_agent_plist(&self) -> PathBuf {
        self.home
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", BUNDLE_ID))
    }

    /// Create the application-support directory if it does not exist.
    pub fn ensure_app_support_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)
    }
}

impl AsRef<Path> for Paths {
    fn as_ref(&self) -> &Path { &self.root }
}
```

- [ ] **Step 4: Export from `src/lib.rs`**

Edit `src/lib.rs`:

```rust
//! MXBattery — Logitech BLE mouse battery monitor for macOS.

pub mod paths;

pub fn run() -> anyhow::Result<()> {
    println!("mxbattery v{}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --test paths`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add src/paths.rs src/lib.rs tests/paths.rs
git commit -m "feat(paths): add path resolution module"
```

---

### Task 3: Logging setup

**Files:**
- Create: `src/logging.rs`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Implement `src/logging.rs`**

```rust
use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub fn init() {
    let filter = EnvFilter::try_from_env("MXBATTERY_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let layer = fmt::layer()
        .with_target(true)
        .with_timer(ChronoLocal::new("%H:%M:%S%.3f".into()))
        .with_writer(std::io::stderr);
    tracing_subscriber::registry().with(filter).with(layer).init();
}
```

- [ ] **Step 2: Wire into `src/lib.rs`**

```rust
//! MXBattery — Logitech BLE mouse battery monitor for macOS.

pub mod logging;
pub mod paths;

pub fn run() -> anyhow::Result<()> {
    logging::init();
    tracing::info!(version = %env!("CARGO_PKG_VERSION"), "mxbattery starting");
    Ok(())
}
```

- [ ] **Step 3: Sanity check**

Run: `MXBATTERY_LOG=debug cargo run --release`
Expected: a log line like `… INFO mxbattery: mxbattery starting version="0.1.0"` on stderr.

- [ ] **Step 4: Commit**

```bash
git add src/logging.rs src/lib.rs
git commit -m "feat(logging): tracing setup with MXBATTERY_LOG env filter"
```

---

## Phase 1 — Pure modules

These five tasks (4–9) are pure Rust with no `objc2` involvement. They are TDD'd with table-driven tests. After Phase 1 the crate compiles, has a real test suite, and contains every piece of business logic that does not need a Mac framework.

### Task 4: HID++ codec

**Files:**
- Create: `src/hidpp.rs`
- Modify: `src/lib.rs`
- Test: `tests/hidpp.rs`

- [ ] **Step 1: Write the failing test (frame shape detection + decode)**

Create `tests/hidpp.rs`:

```rust
use mxbattery::hidpp::{decode, encode_get_feature, encode_get_battery_level_status, FrameShape, HidppFrame};

// Captured from probe-b on MX Master 3 Mac:
// >>> SEND root.getFeature(0x1000) [00 0e 10 00 00 …] (18 bytes, no devIdx)
// RECV vendor [00 0e 08 00 01 00 …] -> feature index 0x08 for 0x1000
// RECV vendor [08 00 64 32 00 …] -> unsolicited 0x1000 event (charging-state change)

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
    assert_eq!(f.params[0], 0x08); // resolved feature index for 0x1000
}

#[test]
fn decode_unsolicited_event_swid_zero() {
    // Event frame the device pushes after a charging-state change.
    let buf = hex::decode("08006432000000000000000000000000000000").unwrap();
    let f = decode(SHAPE, &buf).unwrap();
    assert_eq!(f.feature_index, 0x08);
    assert_eq!(f.function, 0);
    assert_eq!(f.swid, 0); // 0 marks a notification
    assert!(f.is_event());
    assert_eq!(f.params[0], 0x64); // level
    assert_eq!(f.params[1], 0x32); // next
    assert_eq!(f.params[2], 0x00); // discharging
}

#[test]
fn decode_short_buffer_returns_none() {
    let buf = vec![0u8; 1];
    assert!(decode(SHAPE, &buf).is_none());
}
```

> **Heads-up:** `hex` is not yet in `Cargo.toml`. Add `hex = "0.4"` to `[dev-dependencies]` before running this test.

- [ ] **Step 2: Add `hex` dev-dep**

Append to `[dev-dependencies]` in `Cargo.toml`:

```toml
hex = "0.4"
```

- [ ] **Step 3: Run test to verify it fails to compile**

Run: `cargo test --test hidpp`
Expected: compile error (module doesn't exist yet).

- [ ] **Step 4: Implement `src/hidpp.rs`**

```rust
//! HID++ 2.0 frame codec for the Logitech BLE-HID++ pipe.
//!
//! On the MX Master 3 Mac the BLE GATT vendor characteristic accepts 18-byte
//! frames laid out as `[feature_index, function|swid, params(16)]` — i.e. with
//! no leading device-index byte. Other Logitech BLE devices may use the
//! conventional 19-byte `[devIdx=0xFF, feature_index, function|swid, params(15)]`
//! shape, so the encoder is parameterised on a `FrameShape` enum and the
//! battery module probes for the right shape on connect.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameShape {
    /// 18 bytes: `[featIdx, fn|swid, params...]`. MX Master 3 Mac BLE.
    NoDevIdx18,
    /// 19 bytes: `[0xFF, featIdx, fn|swid, params...]`. Conventional HID++ 2.0.
    WithDevIdx19,
    /// 20 bytes: `[0x11, 0xFF, featIdx, fn|swid, params...]`. With HID report-id prefix.
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

    /// Offset at which the `feature_index` byte sits.
    const fn featidx_off(self) -> usize {
        match self {
            FrameShape::NoDevIdx18 => 0,
            FrameShape::WithDevIdx19 => 1,
            FrameShape::WithReportId20 => 2,
        }
    }
}

pub const ROOT_FEATURE_INDEX: u8 = 0x00;
pub const FN_GET_FEATURE: u8 = 0x00; // root function 0
pub const FN_GET_BATTERY_LEVEL_STATUS: u8 = 0x00; // 0x1000 function 0
pub const FEATURE_BATTERY_STATUS: u16 = 0x1000;
pub const FEATURE_UNIFIED_BATTERY: u16 = 0x1004;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HidppFrame {
    pub feature_index: u8,
    pub function: u8,
    pub swid: u8,
    /// Parameter bytes after the header. Length depends on shape (16, 15, or 15).
    pub params: Vec<u8>,
}

impl HidppFrame {
    /// Unsolicited events have `swid == 0`.
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

/// Decode a 0x1000 BatteryStatus response or event.
/// Returns `(level, next_threshold, charging_state)`.
pub fn decode_battery_status(params: &[u8]) -> Option<(u8, u8, ChargingStateRaw)> {
    if params.len() < 3 { return None; }
    Some((params[0], params[1], ChargingStateRaw::from_byte(params[2])))
}
```

- [ ] **Step 5: Export from `src/lib.rs`**

Edit `src/lib.rs` to add `pub mod hidpp;`.

- [ ] **Step 6: Run tests**

Run: `cargo test --test hidpp`
Expected: PASS (5 tests).

- [ ] **Step 7: Commit**

```bash
git add src/hidpp.rs src/lib.rs tests/hidpp.rs Cargo.toml Cargo.lock
git commit -m "feat(hidpp): codec for HID++ frames over BLE"
```

---

### Task 5: Config types + TOML parse

**Files:**
- Create: `src/config.rs`
- Modify: `src/lib.rs`
- Test: `tests/config.rs`

- [ ] **Step 1: Write the failing tests**

Create `tests/config.rs`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test config`
Expected: compile error.

- [ ] **Step 3: Implement `src/config.rs`**

```rust
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
```

- [ ] **Step 4: Export from `src/lib.rs`**

Add `pub mod config;`.

- [ ] **Step 5: Run tests**

Run: `cargo test --test config`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add src/config.rs src/lib.rs tests/config.rs
git commit -m "feat(config): TOML schema, defaults, validation"
```

---

### Task 6: Config hot-reload via FSEvents

**Files:**
- Modify: `src/config.rs` (add `Watcher`)
- Test: `tests/config_hot_reload.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/config_hot_reload.rs`:

```rust
use mxbattery::config::{Config, ConfigWatcher};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watcher_emits_on_save_and_keeps_old_on_bad_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    Config::default().save(&path).unwrap();

    let watcher = ConfigWatcher::start(path.clone()).unwrap();
    let initial = watcher.current();
    assert_eq!(initial.thresholds.warn, 20);

    // Save a new valid config: warn=15
    let mut next = Config::default();
    next.thresholds.warn = 15;
    next.save(&path).unwrap();

    // Wait for the watcher to swap.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if watcher.current().thresholds.warn == 15 { break; }
        if tokio::time::Instant::now() >= deadline {
            panic!("watcher did not see new config in time");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Now write an invalid TOML (critical >= warn). Watcher should keep the previous good one.
    std::fs::write(&path, "schema_version = 1\n[thresholds]\nwarn = 5\ncritical = 10\nrearm_hysteresis = 5\nwarn_enabled = true\ncritical_enabled = true\n[device]\nmode = \"any-mx\"\n[cadence]\nwarn_period = \"24h\"\ncritical_period = \"30m\"\n[menubar]\nenabled = true\n[autostart]\nenabled = false\n").unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(watcher.current().thresholds.warn, 15, "kept previous good config");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test config_hot_reload`
Expected: compile error (`ConfigWatcher` doesn't exist).

- [ ] **Step 3: Append `ConfigWatcher` to `src/config.rs`**

```rust
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
        let watch_path = path.clone();
        let dir = path.parent().unwrap().to_path_buf();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else { return };
            if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)) {
                return;
            }
            // Coalesce: only react if the configured file changed (or its tmp-rename).
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test --test config_hot_reload`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs tests/config_hot_reload.rs
git commit -m "feat(config): FSEvents-based hot reload"
```

---

### Task 7: State (atomic JSON)

**Files:**
- Create: `src/state.rs`
- Modify: `src/lib.rs`
- Test: `tests/state.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/state.rs`:

```rust
use chrono::{Local, NaiveDate, TimeZone};
use mxbattery::state::{ChargingState, BatteryReadingSnapshot, State};

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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test state`
Expected: compile error.

- [ ] **Step 3: Implement `src/state.rs`**

```rust
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
```

- [ ] **Step 4: Export from `src/lib.rs`**

Add `pub mod state;`.

- [ ] **Step 5: Run tests**

Run: `cargo test --test state`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add src/state.rs src/lib.rs tests/state.rs
git commit -m "feat(state): atomic JSON I/O for persisted notification state"
```

---

### Task 8: Device filter

**Files:**
- Create: `src/device_filter.rs`
- Modify: `src/lib.rs`
- Test: `tests/device_filter.rs`

- [ ] **Step 1: Write the failing tests**

Create `tests/device_filter.rs`:

```rust
use mxbattery::config::DeviceFilter;
use mxbattery::device_filter::{DeviceProbe, matches};

fn mx_master_3() -> DeviceProbe {
    DeviceProbe {
        peripheral_identifier: "798C4CBB-C8BE-9C3C-F286-195F08C01978".into(),
        model_number: Some("MX Master 3".into()),
        pnp_vid: Some(0x046D),
        pnp_pid: Some(0xB023),
        has_logitech_vendor_service: true,
    }
}

fn airpods_pro() -> DeviceProbe {
    DeviceProbe {
        peripheral_identifier: "30821691-5D68-0000-0000-000000000000".into(),
        model_number: Some("AirPods Pro".into()),
        pnp_vid: Some(0x004C),
        pnp_pid: Some(0x2014),
        has_logitech_vendor_service: false,
    }
}

#[test]
fn specific_matches_by_identifier() {
    let p = mx_master_3();
    let f = DeviceFilter::Specific { identifier: p.peripheral_identifier.clone() };
    assert!(matches(&f, &p));
}

#[test]
fn any_mx_matches_model_number_prefix() {
    assert!(matches(&DeviceFilter::AnyMx, &mx_master_3()));
    assert!(!matches(&DeviceFilter::AnyMx, &airpods_pro()));
}

#[test]
fn any_logitech_matches_vendor_service_or_pnp_vid() {
    assert!(matches(&DeviceFilter::AnyLogitech, &mx_master_3()));
    assert!(!matches(&DeviceFilter::AnyLogitech, &airpods_pro()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test device_filter`
Expected: compile error.

- [ ] **Step 3: Implement `src/device_filter.rs`**

```rust
use crate::config::DeviceFilter;

pub const LOGITECH_VID: u16 = 0x046D;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceProbe {
    pub peripheral_identifier: String,
    pub model_number: Option<String>,        // 0x2A24
    pub pnp_vid: Option<u16>,                // 0x2A50 byte 1..=2 LE
    pub pnp_pid: Option<u16>,                // 0x2A50 byte 3..=4 LE
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
            probe.has_logitech_vendor_service
                || probe.pnp_vid == Some(LOGITECH_VID)
        }
    }
}
```

- [ ] **Step 4: Export from `src/lib.rs`**

Add `pub mod device_filter;`.

- [ ] **Step 5: Run tests**

Run: `cargo test --test device_filter`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add src/device_filter.rs src/lib.rs tests/device_filter.rs
git commit -m "feat(device_filter): match logic for specific/AnyMx/AnyLogitech"
```

---

### Task 9: Notifier `decide` (pure)

**Files:**
- Create: `src/notifier/mod.rs`
- Create: `src/notifier/decide.rs`
- Modify: `src/lib.rs`
- Test: `tests/notifier.rs`

- [ ] **Step 1: Write the failing tests**

Create `tests/notifier.rs`:

```rust
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
    // 25 = warn(20) + hysteresis(5)
    clear_armed_if_rose(&BatteryReadingSnapshot { percent: 25, charging: ChargingState::Discharging }, &mut s, &cfg());
    assert_eq!(s.last_warn_notified_date, None, "rising above warn+hysteresis re-arms warn");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test notifier`
Expected: compile error.

- [ ] **Step 3: Implement `src/notifier/decide.rs`**

```rust
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

    let critical_due = cfg.thresholds.critical_enabled
        && reading.percent <= cfg.thresholds.critical
        && state
            .last_critical_notified_at
            .map(|t| (now - t).to_std().ok().map(|d| d >= cfg.cadence.critical_period).unwrap_or(true))
            .unwrap_or(true);

    if critical_due { return Some(NotificationKind::Critical); }

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
```

- [ ] **Step 4: Implement `src/notifier/mod.rs`**

```rust
mod decide;
pub use decide::{decide, clear_armed_if_rose, NotificationKind};
```

- [ ] **Step 5: Export from `src/lib.rs`**

Add `pub mod notifier;`.

- [ ] **Step 6: Run tests**

Run: `cargo test --test notifier`
Expected: PASS (10 tests).

- [ ] **Step 7: Commit**

```bash
git add src/notifier/ src/lib.rs tests/notifier.rs
git commit -m "feat(notifier): pure decide() and re-arm logic"
```

---

## Phase 2 — macOS-bridged data path

Phase 2 wires `objc2-core-bluetooth` to the pure modules. We define a `BatteryBackend` trait so the orchestration logic can be exercised against a `MockBackend` in tests, while the real implementation talks to CoreBluetooth.

### Task 10: BatteryBackend trait + MockBackend

**Files:**
- Create: `src/battery/mod.rs`
- Create: `src/battery/mock.rs`
- Modify: `src/lib.rs`
- Test: `tests/battery_mock.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/battery_mock.rs`:

```rust
use mxbattery::battery::{BatteryBackend, BatteryEvent, mock::MockBackend};
use mxbattery::state::ChargingState;
use std::time::Duration;

#[tokio::test]
async fn mock_emits_recorded_events() {
    let mut events = vec![
        BatteryEvent::Connected { name: "MX Master 3 Mac".into() },
        BatteryEvent::Percent(17),
        BatteryEvent::Charging(ChargingState::Recharging),
        BatteryEvent::Disconnected,
    ];
    events.reverse();
    let backend = MockBackend::new(events);
    let mut rx = backend.subscribe();

    backend.run_to_completion().await;

    let collected: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert_eq!(collected.len(), 4);
    matches!(collected[0], BatteryEvent::Connected { .. });
    matches!(collected[1], BatteryEvent::Percent(17));
    matches!(collected[2], BatteryEvent::Charging(ChargingState::Recharging));
    matches!(collected[3], BatteryEvent::Disconnected);

    // Unused so the linter is happy.
    let _ = Duration::from_millis(0);
}
```

- [ ] **Step 2: Implement `src/battery/mod.rs`**

```rust
use crate::state::ChargingState;
use tokio::sync::broadcast;

pub mod mock;

#[derive(Clone, Debug)]
pub enum BatteryEvent {
    Connected { name: String },
    Percent(u8),
    Charging(ChargingState),
    Disconnected,
}

pub trait BatteryBackend: Send + Sync + 'static {
    fn subscribe(&self) -> broadcast::Receiver<BatteryEvent>;
}
```

- [ ] **Step 3: Implement `src/battery/mock.rs`**

```rust
use super::{BatteryBackend, BatteryEvent};
use std::sync::Mutex;
use tokio::sync::broadcast;

pub struct MockBackend {
    tx: broadcast::Sender<BatteryEvent>,
    queue: Mutex<Vec<BatteryEvent>>,
}

impl MockBackend {
    pub fn new(mut events_in_pop_order: Vec<BatteryEvent>) -> Self {
        // The caller pushes events in reverse so we can pop().
        let (tx, _rx) = broadcast::channel(64);
        Self { tx, queue: Mutex::new(events_in_pop_order.drain(..).collect()) }
    }

    /// Drain all queued events synchronously.
    pub async fn run_to_completion(&self) {
        loop {
            let next = self.queue.lock().unwrap().pop();
            match next {
                Some(ev) => { let _ = self.tx.send(ev); }
                None => break,
            }
        }
    }
}

impl BatteryBackend for MockBackend {
    fn subscribe(&self) -> broadcast::Receiver<BatteryEvent> { self.tx.subscribe() }
}
```

- [ ] **Step 4: Export from `src/lib.rs`**

Add `pub mod battery;`.

- [ ] **Step 5: Run tests**

Run: `cargo test --test battery_mock`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/battery/ src/lib.rs tests/battery_mock.rs
git commit -m "feat(battery): backend trait + mock implementation"
```

---

### Task 11: CoreBluetooth backend — connect, discover, BAS subscribe

**Files:**
- Create: `src/battery/cb.rs`
- Modify: `src/battery/mod.rs`

This task is large because every objc2-core-bluetooth method has to be called from a Rust delegate. There is no easy way to TDD it; we verify via a smoke test that runs against the user's machine in Phase 8.

The implementation here covers the **percent** path (BAS char `0x2A19`) only. The HID++ event path (charging state) lives in Task 12.

- [ ] **Step 1: Add the source module skeleton**

Create `src/battery/cb.rs`:

```rust
//! CoreBluetooth-backed implementation of `BatteryBackend`.
//!
//! Architecture:
//!   * Public API: a single `CbBackend::start()` constructor returning a
//!     handle that satisfies `BatteryBackend`. Callers `subscribe()` to a
//!     `tokio::broadcast` receiver.
//!   * Internals: a delegate object retained on the AppKit main thread.
//!     CoreBluetooth requires its delegate callbacks to fire on the central
//!     manager's queue, which we configure to be the main dispatch queue.
//!     Inside each delegate method we forward an event into the broadcast
//!     channel.
//!
//! The objc2 boilerplate to implement `CBCentralManagerDelegate` and
//! `CBPeripheralDelegate` is verbose. The delegate type is declared with
//! `define_class!` from objc2.

use crate::config::DeviceFilter;
use crate::device_filter::{matches, DeviceProbe};
use crate::state::ChargingState;
use crate::hidpp::{self, FrameShape};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AllocAnyThread, MainThreadMarker};
use objc2_core_bluetooth::{
    CBCentralManager, CBCentralManagerDelegate, CBCharacteristic, CBManagerState,
    CBPeripheral, CBPeripheralDelegate, CBService, CBUUID,
};
use objc2_foundation::{NSArray, NSData, NSError, NSObject, NSString};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use super::{BatteryBackend, BatteryEvent};

pub const BATTERY_SERVICE_UUID: &str = "180F";
pub const BATTERY_LEVEL_UUID:   &str = "2A19";
pub const DEVICE_INFO_SERVICE_UUID: &str = "180A";
pub const MODEL_NUMBER_UUID: &str = "2A24";
pub const PNP_ID_UUID:       &str = "2A50";
pub const VENDOR_SERVICE_UUID: &str = "00010000-0000-1000-8000-011F2000046D";
pub const VENDOR_CHAR_UUID:    &str = "00010001-0000-1000-8000-011F2000046D";

pub struct CbBackend {
    tx: broadcast::Sender<BatteryEvent>,
    _delegate: Retained<Delegate>,
    _manager: Retained<CBCentralManager>,
}

impl CbBackend {
    /// Must be called on the main thread (NSApplication's run loop).
    pub fn start(filter: DeviceFilter, mtm: MainThreadMarker) -> Self {
        let (tx, _) = broadcast::channel(64);
        let delegate = Delegate::new(tx.clone(), filter, mtm);
        let manager: Retained<CBCentralManager> = unsafe {
            let alloc: *mut CBCentralManager = msg_send![CBCentralManager::class(), alloc];
            let queue: *mut objc2::runtime::AnyObject = std::ptr::null_mut(); // main queue
            let m: *mut CBCentralManager = msg_send![
                alloc,
                initWithDelegate: ProtocolObject::from_ref(&*delegate),
                queue: queue
            ];
            Retained::from_raw(m).expect("CBCentralManager init")
        };
        Self { tx, _delegate: delegate, _manager: manager }
    }
}

impl BatteryBackend for CbBackend {
    fn subscribe(&self) -> broadcast::Receiver<BatteryEvent> { self.tx.subscribe() }
}

#[derive(Default)]
struct Inner {
    filter: Option<DeviceFilter>,
    peripheral: Option<Retained<CBPeripheral>>,
    battery_char: Option<Retained<CBCharacteristic>>,
    vendor_char: Option<Retained<CBCharacteristic>>,
    model_number_char: Option<Retained<CBCharacteristic>>,
    pnp_id_char: Option<Retained<CBCharacteristic>>,
    /// Probe accumulator filled as device-info reads complete.
    probe: DeviceProbe,
    /// Frame shape negotiated with the device. Defaults to NoDevIdx18 and
    /// flips on write-error.
    shape: FrameShape,
    feature_index_1000: Option<u8>,
    swid_counter: u8,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MXBatteryDelegate"]
    pub struct Delegate;

    impl Delegate {
        // Constructor & ivars are managed via separate methods because objc2's
        // safe API only supports class-level ivars through `define_class!`'s
        // `#[ivars = T]`. We keep state in an Arc<Mutex<Inner>> instead so the
        // class itself is plain.
    }

    unsafe impl NSObjectProtocol for Delegate {}
);

impl Delegate {
    fn new(
        tx: broadcast::Sender<BatteryEvent>,
        filter: DeviceFilter,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let _ = (tx, filter, mtm);
        unimplemented!("Delegate::new — see comment block below");
    }
}
```

> **Engineer's notes for the rest of this task:**
> The full delegate is too long for this single bullet. The pattern to follow exactly:
>
> 1. Replace `define_class!` with the variant that supports state. Idiomatic objc2 0.5: declare `pub struct DelegateState { tx, filter, inner: Arc<Mutex<Inner>>, mtm }` and put it inside an `Mutex<DelegateState>` accessed via an objc2 ivar (`#[ivars = Mutex<DelegateState>]`). The `Delegate::new` calls `Self::alloc(mtm)` then `init` and writes the ivar.
> 2. Implement `centralManagerDidUpdateState:` — when state is `.poweredOn`, call `retrieveConnectedPeripheralsWithServices:` for `[180F, 180A, vendorServiceUUID]`, then for each returned peripheral call `connectPeripheral:options:nil` (we'll device-filter after we've read 0x2A24 / 0x2A50).
> 3. Implement `centralManager:didConnectPeripheral:` — set the peripheral's delegate to `self`, then `discoverServices:` for the three UUIDs.
> 4. Implement `peripheral:didDiscoverServices:` — for each service, `discoverCharacteristics:nil forService:`.
> 5. Implement `peripheral:didDiscoverCharacteristicsForService:` — pick out the four characteristics we care about (`0x2A19`, vendor, `0x2A24`, `0x2A50`), store them, and `readValue:forCharacteristic:` for the two device-info chars. Once both have been read, do the device-filter check; if it fails, `cancelPeripheralConnection:` and return.
> 6. Implement `peripheral:didUpdateValueForCharacteristic:` — switch on the characteristic UUID:
>     - `0x2A24` → `probe.model_number = Some(utf8_string)`. After both 2A24 and 2A50 are populated, run the filter check.
>     - `0x2A50` → `probe.pnp_vid = Some(LE u16 from bytes 1..=2); probe.pnp_pid = bytes 3..=4`.
>     - `0x2A19` → emit `BatteryEvent::Percent(byte0)`.
>     - vendor char → forward bytes to the HID++ handler (Task 12).
> 7. Implement `centralManager:didDisconnectPeripheral:` — emit `BatteryEvent::Disconnected`. Don't tear the manager down; CoreBluetooth will reconnect when the device returns and we'll re-do the discovery dance.
> 8. After the device-filter check passes, call `setNotifyValue:true forCharacteristic:` for `0x2A19` and the vendor char, then `readValue:forCharacteristic:` for `0x2A19` to seed.
>
> objc2 0.5 references: `<https://docs.rs/objc2/0.5/objc2/macro.define_class.html>`, `<https://docs.rs/objc2-core-bluetooth/0.2/objc2_core_bluetooth>`. The `objc2-core-bluetooth` crate exposes every protocol method as a Rust trait; you implement them inside an `unsafe impl ProtocolType for Delegate { … }` block.

- [ ] **Step 2: Add `pub mod cb;` to `src/battery/mod.rs`**

```rust
// inside src/battery/mod.rs, below the existing items:
#[cfg(target_os = "macos")]
pub mod cb;
```

- [ ] **Step 3: Verify it still builds**

Run: `cargo build --release`
Expected: warnings about the unimplemented `Delegate::new` are fine; the binary compiles. Resolve any compilation errors before continuing.

- [ ] **Step 4: Implement the delegate body per the engineer's notes**

There are no shortcuts. Use `cargo check` after each method addition. The delegate's `Inner` lives behind `Arc<Mutex<Inner>>`; CoreBluetooth's main-queue serialisation means contention is minimal.

- [ ] **Step 5: Add a runnable example for the dogfood checklist**

Add `examples/smoke_battery.rs` (referenced from `docs/dogfood-checklist.md`):

```rust
use mxbattery::battery::{cb::CbBackend, BatteryBackend, BatteryEvent};
use mxbattery::config::DeviceFilter;
use objc2::MainThreadMarker;

fn main() {
    let mtm = MainThreadMarker::new().expect("main thread");
    let backend = CbBackend::start(DeviceFilter::AnyMx, mtm);
    let mut rx = backend.subscribe();
    let app = unsafe { objc2_app_kit::NSApplication::sharedApplication(mtm) };
    std::thread::spawn(move || loop {
        if let Ok(ev) = rx.blocking_recv() {
            println!("{:?}", ev);
        }
    });
    app.run();
}
```

- [ ] **Step 6: Verify it builds**

Run: `cargo build --release --example smoke_battery`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/battery/cb.rs src/battery/mod.rs examples/smoke_battery.rs
git commit -m "feat(battery): CoreBluetooth backend (BAS path)"
```

---

### Task 12: HID++ event handling on the vendor characteristic

**Files:**
- Modify: `src/battery/cb.rs`
- Test: `tests/hidpp_event.rs`

- [ ] **Step 1: Write the failing test for the vendor-char dispatcher**

The dispatcher itself is a pure function we can test without CoreBluetooth.

Create `tests/hidpp_event.rs`:

```rust
use mxbattery::battery::{handle_vendor_bytes, VendorOutcome};
use mxbattery::hidpp::FrameShape;
use mxbattery::state::ChargingState;

#[test]
fn dispatch_get_feature_response() {
    // root response: feature_index 0x08 was assigned to 0x1000.
    let bytes = hex::decode("000e0800010000000000000000000000000000").unwrap();
    let mut feat_1000_idx = None;
    let out = handle_vendor_bytes(FrameShape::NoDevIdx18, &bytes, 0xE, &mut feat_1000_idx);
    assert!(matches!(out, VendorOutcome::ResolvedBatteryStatusFeature(0x08)));
    assert_eq!(feat_1000_idx, Some(0x08));
}

#[test]
fn dispatch_battery_event_charging() {
    let bytes = hex::decode("08000000010000000000000000000000000000").unwrap();
    let mut idx = Some(0x08);
    let out = handle_vendor_bytes(FrameShape::NoDevIdx18, &bytes, 0, &mut idx);
    assert!(matches!(out, VendorOutcome::Charging(ChargingState::Recharging)));
}

#[test]
fn dispatch_battery_event_discharging() {
    let bytes = hex::decode("08006432000000000000000000000000000000").unwrap();
    let mut idx = Some(0x08);
    let out = handle_vendor_bytes(FrameShape::NoDevIdx18, &bytes, 0, &mut idx);
    assert!(matches!(out, VendorOutcome::Charging(ChargingState::Discharging)));
}

#[test]
fn dispatch_unknown_swid_is_ignored() {
    let bytes = hex::decode("0a01ff0000000000000000000000000000000000").unwrap();
    let mut idx = Some(0x08);
    let out = handle_vendor_bytes(FrameShape::NoDevIdx18, &bytes, 0, &mut idx);
    assert!(matches!(out, VendorOutcome::Ignored));
}
```

- [ ] **Step 2: Add the dispatcher**

Append to `src/battery/mod.rs`:

```rust
use crate::hidpp::{self, FrameShape};
use crate::state::ChargingState;

#[derive(Clone, Debug)]
pub enum VendorOutcome {
    ResolvedBatteryStatusFeature(u8),
    Charging(ChargingState),
    Ignored,
}

pub fn handle_vendor_bytes(
    shape: FrameShape,
    bytes: &[u8],
    expected_swid_for_resolve: u8,
    feature_index_1000: &mut Option<u8>,
) -> VendorOutcome {
    let Some(frame) = hidpp::decode(shape, bytes) else { return VendorOutcome::Ignored };
    if frame.is_event() {
        if Some(frame.feature_index) == *feature_index_1000 {
            if let Some((_, _, raw)) = hidpp::decode_battery_status(&frame.params) {
                return VendorOutcome::Charging(map_charging(raw));
            }
        }
        return VendorOutcome::Ignored;
    }
    if frame.feature_index == hidpp::ROOT_FEATURE_INDEX
        && frame.function == hidpp::FN_GET_FEATURE
        && frame.swid == expected_swid_for_resolve
    {
        let resolved = frame.params.first().copied().unwrap_or(0);
        if resolved != 0 {
            *feature_index_1000 = Some(resolved);
            return VendorOutcome::ResolvedBatteryStatusFeature(resolved);
        }
    }
    VendorOutcome::Ignored
}

fn map_charging(raw: hidpp::ChargingStateRaw) -> ChargingState {
    match raw {
        hidpp::ChargingStateRaw::Discharging => ChargingState::Discharging,
        hidpp::ChargingStateRaw::Recharging => ChargingState::Recharging,
        hidpp::ChargingStateRaw::ChargeInFinalState => ChargingState::ChargeInFinalState,
        hidpp::ChargingStateRaw::ChargeComplete => ChargingState::ChargeComplete,
        hidpp::ChargingStateRaw::Other(_) => ChargingState::Unknown,
    }
}
```

- [ ] **Step 3: Wire it into `cb.rs`**

In the `peripheral:didUpdateValueForCharacteristic:` arm for the vendor characteristic, call `handle_vendor_bytes(shape, &payload, expected_swid, &mut inner.feature_index_1000)` and:

- On `ResolvedBatteryStatusFeature(idx)` — store the index, then write `encode_get_battery_level_status(shape, idx, swid)` to the vendor char to seed the charging state.
- On `Charging(state)` — `tx.send(BatteryEvent::Charging(state))`.
- On `Ignored` — log at debug level, drop.

Implementation detail: when sending the feature-resolve request immediately after `setNotifyValue:true` for the vendor char succeeds, write `encode_get_feature(shape, 0x1000, swid)` to the vendor char with `CBCharacteristicWriteType::WithResponse`. Track `expected_swid_for_resolve` in `Inner`.

- [ ] **Step 4: Run the unit tests**

Run: `cargo test --test hidpp_event`
Expected: PASS (4 tests).

- [ ] **Step 5: Verify it builds**

Run: `cargo build --release`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/battery/mod.rs src/battery/cb.rs tests/hidpp_event.rs
git commit -m "feat(battery): HID++ event dispatcher for charging state"
```

---

### Task 13: Frame-shape auto-probe

**Files:**
- Modify: `src/battery/cb.rs`

When the first vendor-char write returns `CBATTErrorDomain Code=13` ("invalid value length") or no response within 2 s, retry with the next shape from `[NoDevIdx18, WithDevIdx19, WithReportId20]`.

- [ ] **Step 1: Add a 2-second response timer per send**

In the `peripheral:didWriteValueForCharacteristic:` arm, if `error.code == 13`, advance `inner.shape` to the next variant (`NoDevIdx18 → WithDevIdx19 → WithReportId20 → give up`) and retry the same logical step (resolve or seed-charging).

In the same place, schedule a `dispatch_after` for 2 s after each send; if the matching response has not arrived (`Inner` keeps a `pending_resolve_swid: Option<u8>` that gets cleared on receive), advance the variant and retry.

- [ ] **Step 2: Verify it builds**

Run: `cargo build --release`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/battery/cb.rs
git commit -m "feat(battery): auto-probe HID++ frame shape on connect"
```

> Manual verification of the variant fallback (force a wrong shape and observe retries) is described in `docs/dogfood-checklist.md`.

---

## Phase 3 — Notifications

### Task 14: UNUserNotificationCenter wrapper

**Files:**
- Modify: `src/notifier/mod.rs`
- Create: `src/notifier/un.rs`

- [ ] **Step 1: Implement `src/notifier/un.rs`**

```rust
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{msg_send, MainThreadMarker};
use objc2_foundation::{NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationCenter,
    UNNotificationRequest, UNUserNotificationCenter,
};

pub fn request_permission() {
    let center = unsafe { UNUserNotificationCenter::currentNotificationCenter() };
    let opts = UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound;
    let block = block2::RcBlock::new(|_granted: bool, _err: *mut NSError| {});
    unsafe {
        center.requestAuthorizationWithOptions_completionHandler(opts, &block);
    }
}

pub fn post(title: &str, body: &str, identifier: &str) {
    let center = unsafe { UNUserNotificationCenter::currentNotificationCenter() };
    let content = UNMutableNotificationContent::new();
    unsafe {
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
    }
    let req = unsafe {
        UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(identifier),
            &content,
            std::ptr::null_mut(),
        )
    };
    let block = block2::RcBlock::new(|_err: *mut NSError| {});
    unsafe {
        center.addNotificationRequest_withCompletionHandler(&req, &block);
    }
}
```

> Add `block2 = "0.5"` to `[dependencies]` for the completion-handler closures.

- [ ] **Step 2: Wire into `src/notifier/mod.rs`**

```rust
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
pub fn request_permission() { un::request_permission(); }
```

- [ ] **Step 3: Verify it builds**

Run: `cargo build --release`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/notifier/ Cargo.toml Cargo.lock
git commit -m "feat(notifier): UNUserNotificationCenter posting wrapper"
```

---

## Phase 4 — Menu bar UI

### Task 15: NSStatusItem with quantized battery glyph

**Files:**
- Create: `src/menubar.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Implement `src/menubar.rs`**

Build a custom `NSImage` template using `NSBezierPath` to draw a horizontal battery glyph (rounded rectangle, terminal nub on the right, quantised fill).

The five quantisation buckets and tints follow the spec table exactly. Charging gets a small bolt overlay drawn on top of the fill.

```rust
use objc2::rc::Retained;
use objc2::{msg_send, MainThreadMarker};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSGraphicsContext, NSImage, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{CGFloat, NSRect, NSSize, NSString};
use crate::state::ChargingState;

const ICON_W: CGFloat = 22.0;
const ICON_H: CGFloat = 14.0;

pub struct MenubarIcon {
    item: Retained<NSStatusItem>,
}

impl MenubarIcon {
    pub fn install(mtm: MainThreadMarker) -> Self {
        let bar = unsafe { NSStatusBar::systemStatusBar() };
        let item: Retained<NSStatusItem> = unsafe {
            bar.statusItemWithLength(NSVariableStatusItemLength)
        };
        Self { item }
    }

    pub fn render(&self, percent: u8, charging: ChargingState) {
        let img = render_icon(percent, charging);
        unsafe {
            if let Some(button) = self.item.button() {
                button.setImage(Some(&img));
            }
        }
    }

    pub fn remove(self) {
        let bar = unsafe { NSStatusBar::systemStatusBar() };
        unsafe { bar.removeStatusItem(&self.item) };
    }
}

fn render_icon(percent: u8, charging: ChargingState) -> Retained<NSImage> {
    let size = NSSize { width: ICON_W, height: ICON_H };
    let img: Retained<NSImage> = unsafe { NSImage::initWithSize(NSImage::alloc(), size) };
    unsafe { img.lockFocus(); }
    draw_battery(percent, charging);
    unsafe { img.unlockFocus(); img.setTemplate(true); }
    img
}

fn draw_battery(percent: u8, charging: ChargingState) {
    use objc2_app_kit::NSCompositingOperation;
    let body = unsafe { NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        NSRect::new((1.0, 2.0).into(), (ICON_W - 4.0, ICON_H - 4.0).into()),
        2.0, 2.0,
    )};
    unsafe {
        body.setLineWidth(1.0);
        NSColor::controlTextColor().setStroke();
        body.stroke();
    }
    // terminal nub
    let nub = unsafe { NSBezierPath::bezierPathWithRect(
        NSRect::new((ICON_W - 3.0, 5.0).into(), (1.5, 4.0).into())
    )};
    unsafe { nub.fill(); }
    // fill
    let (fill_frac, color) = bucket(percent);
    if fill_frac > 0.0 {
        unsafe { color.setFill(); }
        let inner_w = (ICON_W - 6.0) * fill_frac;
        let rect = NSRect::new((2.0, 3.0).into(), (inner_w, ICON_H - 6.0).into());
        let p = unsafe { NSBezierPath::bezierPathWithRect(rect) };
        unsafe { p.fill(); }
    }
    if !matches!(charging, ChargingState::Discharging | ChargingState::Unknown) {
        draw_bolt();
    }
}

fn bucket(p: u8) -> (CGFloat, Retained<NSColor>) {
    unsafe {
        match p {
            0..=5   => (0.05, NSColor::systemRedColor()),
            6..=20  => (0.25, NSColor::systemYellowColor()),
            21..=60 => (0.50, NSColor::controlTextColor()),
            61..=80 => (0.75, NSColor::controlTextColor()),
            _       => (1.00, NSColor::controlTextColor()),
        }
    }
}

fn draw_bolt() {
    use objc2_app_kit::NSPoint;
    let path = unsafe { NSBezierPath::bezierPath() };
    let pts = [
        NSPoint { x: 9.0, y: 3.5 },
        NSPoint { x: 12.0, y: 8.0 },
        NSPoint { x: 10.5, y: 8.0 },
        NSPoint { x: 13.0, y: 11.0 },
        NSPoint { x: 10.0, y: 6.5 },
        NSPoint { x: 11.5, y: 6.5 },
    ];
    unsafe {
        path.moveToPoint(pts[0]);
        for p in &pts[1..] { path.lineToPoint(*p); }
        path.closePath();
        NSColor::controlTextColor().setFill();
        path.fill();
    }
}
```

- [ ] **Step 2: Export from `src/lib.rs`**

Add `#[cfg(target_os = "macos")] pub mod menubar;`.

- [ ] **Step 3: Verify it builds**

Run: `cargo build --release`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/menubar.rs src/lib.rs
git commit -m "feat(menubar): NSStatusItem with quantized battery glyph"
```

---

### Task 16: Click menu (Mute today / Preferences / Quit)

**Files:**
- Modify: `src/menubar.rs`

- [ ] **Step 1: Add the menu**

In `MenubarIcon::install`, build an `NSMenu` with three `NSMenuItem`s. The targets are method selectors on a small `MenuTarget` `define_class!` object that wraps a sender into a `tokio::mpsc::UnboundedSender<MenubarCommand>`:

```rust
pub enum MenubarCommand {
    ToggleMuteToday,
    OpenPrefs,
    Quit,
}
```

- [ ] **Step 2: Toggle "Mute today" label dynamically**

When `state.mute_until` is `Some(t)` and `t > now`, the item's title should be "Unmute". Otherwise "Mute today". Provide a public `MenubarIcon::set_muted(&self, muted: bool)` method that updates the title.

- [ ] **Step 3: Verify it builds**

Run: `cargo build --release`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/menubar.rs
git commit -m "feat(menubar): click menu with Mute today / Preferences / Quit"
```

---

## Phase 5 — Prefs UI

### Task 17: Prefs window form

**Files:**
- Create: `src/prefs_ui.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Build the window**

Use `NSWindow` (titled, closable, mini-able, *not* resizable) with an `NSStackView` (vertical, `Gravity::Top`, spacing 8). Rows are themselves horizontal stack views of `[NSTextField label, control]`.

Controls:

| Row | Control | Backed by |
|---|---|---|
| Device | `NSPopUpButton` populated from current scan + "Any MX" / "Any Logitech" | `Config.device` |
| Warn enabled | `NSButton` (checkbox) | `Config.thresholds.warn_enabled` |
| Warn threshold | `NSStepper` + `NSTextField`, range `critical+1..=99` | `Config.thresholds.warn` |
| Critical enabled | `NSButton` (checkbox) | `Config.thresholds.critical_enabled` |
| Critical threshold | `NSStepper` + `NSTextField`, range `1..=warn-1` | `Config.thresholds.critical` |
| Re-arm hysteresis | `NSStepper` + `NSTextField`, 1..=20 | `Config.thresholds.rearm_hysteresis` |
| Critical re-notify period | `NSStepper` + `NSTextField`, 1..=240 minutes | `Config.cadence.critical_period` |
| Show menu bar icon | `NSButton` (checkbox) | `Config.menubar.enabled` |
| Start at login | `NSButton` (checkbox) | `Config.autostart.enabled` |

Bottom row: "Cancel" and "Save" buttons.

- [ ] **Step 2: Wire validation**

Whenever any threshold or hysteresis value changes, recompute the steppers' minimum/maximum:

```text
warn.min = critical + 1
warn.max = 99
critical.min = 1
critical.max = warn - 1
```

The Save button is enabled iff `critical < warn` (defensive — the steppers should already prevent this).

When `warn_enabled` is false, dim the warn-threshold stepper (`isEnabled = false`); same for critical.

- [ ] **Step 3: Save commits the new TOML**

`Save` builds a `Config`, calls `validate()`, calls `save(&path)`. The daemon's `ConfigWatcher` picks up the change automatically.

- [ ] **Step 4: Add `applicationShouldHandleReopen`**

In Task 19's app delegate (next phase), wire double-clicks of the .app to `prefs_ui::show_or_focus()`.

- [ ] **Step 5: Verify it builds**

Run: `cargo build --release`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/prefs_ui.rs src/lib.rs
git commit -m "feat(prefs_ui): native AppKit preferences window with validation"
```

---

## Phase 6 — IPC + CLI

### Task 18: IPC socket + single-instance lock

**Files:**
- Create: `src/ipc.rs`
- Modify: `src/lib.rs`
- Test: `tests/ipc.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/ipc.rs`:

```rust
use mxbattery::ipc::{send_open_prefs, IpcServer, IpcCommand};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_instance_lands_open_prefs() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("control.sock");
    let server = IpcServer::bind(sock.clone()).await.unwrap();

    let recv = tokio::spawn({
        let mut events = server.subscribe();
        async move {
            tokio::time::timeout(Duration::from_secs(1), events.recv()).await.unwrap().unwrap()
        }
    });

    send_open_prefs(&sock).await.unwrap();
    let cmd = recv.await.unwrap();
    assert!(matches!(cmd, IpcCommand::OpenPrefs));
}

#[tokio::test]
async fn second_bind_fails_with_kind_already_running() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("control.sock");
    let _server1 = IpcServer::bind(sock.clone()).await.unwrap();
    let err = IpcServer::bind(sock.clone()).await.err().unwrap();
    assert!(matches!(err, mxbattery::ipc::IpcError::AlreadyRunning));
}
```

- [ ] **Step 2: Implement `src/ipc.rs`**

```rust
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

#[derive(Clone, Copy, Debug)]
pub enum IpcCommand { OpenPrefs }

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("daemon already running on this socket")]
    AlreadyRunning,
}

pub struct IpcServer {
    tx: broadcast::Sender<IpcCommand>,
    _path: PathBuf,
}

impl IpcServer {
    pub async fn bind(path: PathBuf) -> Result<Self, IpcError> {
        if path.exists() {
            // Try to connect; if a server is alive we should bail.
            if UnixStream::connect(&path).await.is_ok() {
                return Err(IpcError::AlreadyRunning);
            }
            let _ = std::fs::remove_file(&path);
        }
        let listener = UnixListener::bind(&path)?;
        let (tx, _rx) = broadcast::channel(8);
        let tx2 = tx.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(e) => { tracing::warn!(?e, "accept"); continue; }
                };
                let tx = tx2.clone();
                tokio::spawn(async move {
                    use tokio::io::AsyncReadExt;
                    let mut buf = String::new();
                    if (&stream).take(4096).read_to_string(&mut buf).await.is_ok() {
                        if buf.trim() == "open_prefs" {
                            let _ = tx.send(IpcCommand::OpenPrefs);
                        }
                    }
                });
            }
        });
        Ok(Self { tx, _path: path })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<IpcCommand> { self.tx.subscribe() }
}

pub async fn send_open_prefs(path: &Path) -> Result<(), IpcError> {
    let mut s = UnixStream::connect(path).await?;
    s.write_all(b"open_prefs\n").await?;
    Ok(())
}
```

- [ ] **Step 3: Export from `src/lib.rs`**

Add `pub mod ipc;`.

- [ ] **Step 4: Run tests**

Run: `cargo test --test ipc`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ipc.rs src/lib.rs tests/ipc.rs
git commit -m "feat(ipc): unix-socket control + single-instance lock"
```

---

### Task 19: CLI subcommands

**Files:**
- Modify: `src/main.rs`
- Modify: `src/lib.rs`
- Create: `src/launchd.rs`

- [ ] **Step 1: Implement `src/launchd.rs`**

```rust
use crate::paths::{Paths, BUNDLE_ID};
use std::path::PathBuf;

pub fn install(app_bundle_path: &str) -> anyhow::Result<()> {
    let paths = Paths::standard()?;
    let plist_path = paths.launch_agent_plist();
    if let Some(parent) = plist_path.parent() { std::fs::create_dir_all(parent)?; }
    let plist = format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>{out}</string>
    <key>StandardErrorPath</key><string>{err}</string>
</dict>
</plist>
"#,
        label = BUNDLE_ID,
        exe = format!("{}/Contents/MacOS/mxbattery", app_bundle_path),
        out = paths.app_support_dir().join("daemon.log").display(),
        err = paths.app_support_dir().join("daemon.err.log").display(),
    );
    std::fs::write(&plist_path, plist)?;
    let uid = unsafe { libc::getuid() };
    let domain = format!("gui/{}", uid);
    std::process::Command::new("launchctl").args(["bootout", &domain, plist_path.to_str().unwrap()]).status().ok();
    let st = std::process::Command::new("launchctl").args(["bootstrap", &domain, plist_path.to_str().unwrap()]).status()?;
    anyhow::ensure!(st.success(), "launchctl bootstrap failed");
    Ok(())
}

pub fn uninstall(purge_data: bool) -> anyhow::Result<()> {
    let paths = Paths::standard()?;
    let plist = paths.launch_agent_plist();
    let uid = unsafe { libc::getuid() };
    let domain = format!("gui/{}", uid);
    if plist.exists() {
        let _ = std::process::Command::new("launchctl").args(["bootout", &domain, plist.to_str().unwrap()]).status();
        std::fs::remove_file(&plist)?;
    }
    if purge_data {
        let _ = std::fs::remove_dir_all(paths.app_support_dir());
    }
    Ok(())
}
```

> Add `libc = "0.2"` to `[dependencies]`.

- [ ] **Step 2: Implement `src/main.rs` CLI**

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open (or focus) the preferences window. If no daemon is running, run a one-shot prefs UI.
    Prefs,
    /// Install the LaunchAgent. Pass the absolute path to the bundled `MXBattery.app`.
    Install { app: String },
    /// Remove the LaunchAgent. With --purge, also delete config + state.
    Uninstall { #[arg(long)] purge: bool },
    /// Print the current battery + charging state and exit.
    Read,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        None => mxbattery::run(),
        Some(Cmd::Prefs) => mxbattery::run_prefs_subcommand(),
        Some(Cmd::Install { app }) => mxbattery::launchd::install(&app),
        Some(Cmd::Uninstall { purge }) => mxbattery::launchd::uninstall(purge),
        Some(Cmd::Read) => mxbattery::run_read_subcommand(),
    }
}
```

- [ ] **Step 3: Implement `run_prefs_subcommand` in `src/lib.rs`**

```rust
pub fn run_prefs_subcommand() -> anyhow::Result<()> {
    use crate::ipc::send_open_prefs;
    let paths = paths::Paths::standard()?;
    let sock = paths.control_socket();
    let rt = tokio::runtime::Runtime::new()?;
    let posted = rt.block_on(async { send_open_prefs(&sock).await.is_ok() });
    if posted { return Ok(()); }
    // Daemon not running; run prefs in-process.
    crate::prefs_ui::run_oneshot()?;
    Ok(())
}

pub fn run_read_subcommand() -> anyhow::Result<()> {
    let paths = paths::Paths::standard()?;
    let s = state::State::load(&paths.state_file())?;
    if let Some(snap) = s.last_seen {
        println!("{}% ({:?})", snap.percent, snap.charging);
    } else {
        println!("(no readings recorded yet — daemon must run first)");
    }
    Ok(())
}
```

- [ ] **Step 4: Smoke check**

Run: `cargo run -- --help`
Expected: a usage block listing `prefs`, `install`, `uninstall`, `read`.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs src/lib.rs src/launchd.rs Cargo.toml Cargo.lock
git commit -m "feat(cli): subcommands install/uninstall/prefs/read"
```

---

## Phase 7 — Bundle + launchd

### Task 20: Info.plist + LaunchAgent templates

**Files:**
- Create: `tools/Info.plist.template`
- Create: `tools/com.vincentahrend.mxbattery-app.plist.template`

- [ ] **Step 1: Write `tools/Info.plist.template`**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>      <string>com.vincentahrend.mxbattery-app</string>
    <key>CFBundleName</key>            <string>MXBattery</string>
    <key>CFBundleDisplayName</key>     <string>MXBattery</string>
    <key>CFBundleExecutable</key>      <string>mxbattery</string>
    <key>CFBundleIconFile</key>        <string>AppIcon</string>
    <key>CFBundlePackageType</key>     <string>APPL</string>
    <key>CFBundleShortVersionString</key><string>__VERSION__</string>
    <key>CFBundleVersion</key>         <string>__VERSION__</string>
    <key>LSMinimumSystemVersion</key>  <string>14.0</string>
    <key>LSUIElement</key>             <true/>
    <key>NSBluetoothAlwaysUsageDescription</key>
    <string>Reads battery level and charging state from your Logitech mouse over Bluetooth.</string>
    <key>NSHumanReadableCopyright</key><string>(c) 2026 Vincent Ahrend. MIT licensed.</string>
</dict>
</plist>
```

- [ ] **Step 2: Write the LaunchAgent template**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.vincentahrend.mxbattery-app</string>
    <key>ProgramArguments</key>
    <array>
        <string>__APP_PATH__/Contents/MacOS/mxbattery</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>__LOGDIR__/daemon.log</string>
    <key>StandardErrorPath</key><string>__LOGDIR__/daemon.err.log</string>
</dict>
</plist>
```

- [ ] **Step 3: Commit**

```bash
git add tools/
git commit -m "build(tools): Info.plist + LaunchAgent templates"
```

---

### Task 21: Bundle assembly script

**Files:**
- Create: `tools/make-app.sh`
- Create: `tools/build-icon.sh`
- Create: `tools/app-icon-source.svg`

- [ ] **Step 1: Write `tools/make-app.sh`**

```bash
#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

VERSION=$(awk -F'"' '/^version *= */ {print $2; exit}' Cargo.toml)
BUNDLE_DIR="target/release/MXBattery.app"

cargo build --release --target aarch64-apple-darwin

rm -rf "$BUNDLE_DIR"
mkdir -p "$BUNDLE_DIR/Contents/MacOS" "$BUNDLE_DIR/Contents/Resources"

sed "s/__VERSION__/$VERSION/g" tools/Info.plist.template > "$BUNDLE_DIR/Contents/Info.plist"
cp target/aarch64-apple-darwin/release/mxbattery "$BUNDLE_DIR/Contents/MacOS/mxbattery"

if [[ -f tools/AppIcon.icns ]]; then
    cp tools/AppIcon.icns "$BUNDLE_DIR/Contents/Resources/AppIcon.icns"
fi

if [[ -n "${DEVELOPER_ID:-}" ]]; then
    codesign --force --options runtime --sign "Developer ID Application: $DEVELOPER_ID" "$BUNDLE_DIR"
else
    codesign --force --sign - "$BUNDLE_DIR"
fi

echo "Built: $BUNDLE_DIR"
```

- [ ] **Step 2: Write `tools/build-icon.sh`**

```bash
#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"
SVG=app-icon-source.svg
ICONSET=AppIcon.iconset
mkdir -p "$ICONSET"
for SIZE in 16 32 64 128 256 512 1024; do
    rsvg-convert -w $SIZE -h $SIZE "$SVG" -o "$ICONSET/icon_${SIZE}x${SIZE}.png"
done
iconutil -c icns "$ICONSET"
rm -rf "$ICONSET"
```

> `rsvg-convert` is `brew install librsvg`. Optional; `AppIcon.icns` is allowed to be absent for the dogfood build (the .app will use the generic Apple icon).

- [ ] **Step 3: Provide a placeholder icon**

Create `tools/app-icon-source.svg` containing a simple battery glyph SVG (the same bezier shape as the menu bar icon, scaled up). Keep it under 4 KB.

- [ ] **Step 4: Make scripts executable + smoke build**

```bash
chmod +x tools/make-app.sh tools/build-icon.sh
./tools/make-app.sh
ls -la target/release/MXBattery.app/Contents/MacOS/
```

Expected: a `mxbattery` binary inside the bundle, and the bundle has been ad-hoc signed.

- [ ] **Step 5: Commit**

```bash
git add tools/
git commit -m "build(tools): bundle assembly scripts"
```

---

## Phase 7.5 — CI

> **Note for engineer:** CI is listed late in the plan but should be set up *early* — ideally right after Task 1 — so every subsequent commit is verified by the runner. If you are starting fresh, do this task before Task 2.

### Task 22a: GitHub Actions workflow

**Files:**
- Create: `.github/workflows/ci.yml`
- Create: `.github/workflows/release.yml`

- [ ] **Step 1: Write `.github/workflows/ci.yml`**

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: -D warnings

jobs:
  test:
    runs-on: macos-14   # Apple Silicon runner
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
          targets: aarch64-apple-darwin
      - uses: Swatinem/rust-cache@v2
      - name: rustfmt
        run: cargo fmt --all -- --check
      - name: clippy
        run: cargo clippy --all-targets --all-features --locked
      - name: test
        run: cargo test --all-features --locked
      - name: build release binary
        run: cargo build --release --locked
      - name: assemble .app bundle
        run: ./tools/make-app.sh
      - name: smoke check (no Bluetooth at runner; just verify it does not panic on --help)
        run: ./target/release/MXBattery.app/Contents/MacOS/mxbattery --help
      - uses: actions/upload-artifact@v4
        with:
          name: MXBattery.app
          path: target/release/MXBattery.app
          if-no-files-found: error
```

- [ ] **Step 2: Write `.github/workflows/release.yml`**

```yaml
name: Release

on:
  push:
    tags:
      - 'v*'

jobs:
  release:
    runs-on: macos-14
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-apple-darwin
      - uses: Swatinem/rust-cache@v2
      - name: build .app bundle (ad-hoc signed)
        run: ./tools/make-app.sh
      - name: zip
        run: |
          cd target/release
          zip -r MXBattery.app.zip MXBattery.app
      - uses: softprops/action-gh-release@v2
        with:
          files: target/release/MXBattery.app.zip
          generate_release_notes: true
```

- [ ] **Step 3: Verify workflows are valid YAML locally**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); yaml.safe_load(open('.github/workflows/release.yml')); print('ok')"`
Expected: `ok`.

- [ ] **Step 4: Commit + push**

```bash
git add .github/workflows/
git commit -m "ci: GitHub Actions for test, lint, and tagged releases"
git push
```

Verify the run on GitHub Actions — first run will likely fail until earlier tasks land; that's expected when CI is added late. If you set this up early (before Task 2), each task's "commit" step also pushes and exercises CI.

---

## Phase 8 — App wiring + dogfood

### Task 22: NSApplication wiring + run loop

**Files:**
- Create: `src/app.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Implement `src/app.rs`**

```rust
use crate::battery::{cb::CbBackend, BatteryBackend, BatteryEvent};
use crate::config::ConfigWatcher;
use crate::ipc::{IpcCommand, IpcServer};
use crate::menubar::MenubarIcon;
use crate::notifier::{decide, clear_armed_if_rose, post, NotificationKind};
use crate::paths::Paths;
use crate::state::{BatteryReadingSnapshot, ChargingState, State};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2::MainThreadMarker;

pub fn run_daemon() -> anyhow::Result<()> {
    let mtm = MainThreadMarker::new().expect("must run on main thread");
    let paths = Paths::standard()?;
    paths.ensure_app_support_dir()?;

    // First-run: write a default config if none exists.
    if !paths.config_file().exists() {
        crate::config::Config::default().save(&paths.config_file())?;
    }

    let watcher = ConfigWatcher::start(paths.config_file())?;
    let cfg_handle = watcher.handle();
    let mut state = State::load(&paths.state_file()).unwrap_or_default();

    // Single-instance lock + IPC.
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let server = rt.block_on(IpcServer::bind(paths.control_socket()))?;
    let mut ipc_rx = server.subscribe();

    crate::notifier::request_permission();

    let backend = CbBackend::start(cfg_handle.load_full().device.clone(), mtm);
    let mut bat_rx = backend.subscribe();

    let menubar = MenubarIcon::install(mtm);

    let app: &NSApplication = unsafe { NSApplication::sharedApplication(mtm) };
    unsafe { app.setActivationPolicy(NSApplicationActivationPolicy::Accessory); }

    // Spawn an async glue task that pumps backend + ipc events into application logic.
    let cfg_handle_for_task = cfg_handle.clone();
    let paths_for_task = paths.clone();
    let menubar_handle = std::sync::Arc::new(menubar);
    let menubar_for_task = menubar_handle.clone();
    rt.spawn(async move {
        let mut last_percent: u8 = 0;
        let mut last_charging = ChargingState::Unknown;
        loop {
            tokio::select! {
                Ok(ev) = bat_rx.recv() => {
                    match ev {
                        BatteryEvent::Connected { .. } => {}
                        BatteryEvent::Disconnected => {}
                        BatteryEvent::Percent(p) => {
                            last_percent = p;
                            menubar_for_task.render(last_percent, last_charging);
                            handle_reading(&cfg_handle_for_task, &paths_for_task, &mut state, last_percent, last_charging);
                        }
                        BatteryEvent::Charging(c) => {
                            last_charging = c;
                            menubar_for_task.render(last_percent, last_charging);
                            handle_reading(&cfg_handle_for_task, &paths_for_task, &mut state, last_percent, last_charging);
                        }
                    }
                }
                Ok(cmd) = ipc_rx.recv() => {
                    if let IpcCommand::OpenPrefs = cmd {
                        crate::prefs_ui::show_or_focus();
                    }
                }
            }
        }
    });

    unsafe { app.run(); }
    Ok(())
}

fn handle_reading(
    cfg_handle: &std::sync::Arc<arc_swap::ArcSwap<crate::config::Config>>,
    paths: &Paths,
    state: &mut State,
    percent: u8,
    charging: ChargingState,
) {
    let cfg = cfg_handle.load_full();
    let snap = BatteryReadingSnapshot { percent, charging };
    clear_armed_if_rose(&snap, state, &cfg);
    state.last_seen = Some(snap);
    if let Some(kind) = decide(&snap, state, &cfg, chrono::Local::now()) {
        post(kind, percent, "MX Master 3 Mac");
        match kind {
            NotificationKind::Warn => state.last_warn_notified_date = Some(chrono::Local::now().date_naive()),
            NotificationKind::Critical => state.last_critical_notified_at = Some(chrono::Local::now()),
        }
    }
    let _ = state.save(&paths.state_file());
}
```

- [ ] **Step 2: Wire `lib::run` to `app::run_daemon`**

```rust
pub fn run() -> anyhow::Result<()> {
    logging::init();
    tracing::info!(version = %env!("CARGO_PKG_VERSION"), "mxbattery starting");
    crate::app::run_daemon()
}
```

- [ ] **Step 3: Verify it builds**

Run: `./tools/make-app.sh`
Expected: produces `target/release/MXBattery.app` with the binary inside.

- [ ] **Step 4: Commit**

```bash
git add src/app.rs src/lib.rs
git commit -m "feat(app): NSApplication wiring + glue loop"
```

> End-to-end runtime verification (TCC prompts, menu bar appearance, click handlers, threshold notifications, plug/unplug, mute, prefs hot-reload, launchd install/uninstall, login restart) is documented in `docs/dogfood-checklist.md` and performed by the maintainer outside the implementation flow.

---

### Task 23: Dogfood checklist (documentation only)

This task only writes the checklist document. Actual hardware-dependent verification (TCC prompts, plug/unplug latency, login restart, etc.) is performed by the maintainer outside the implementation flow.

**Files:**
- Create: `docs/dogfood-checklist.md`

- [ ] **Step 1: Write the checklist**

Create `docs/dogfood-checklist.md`:

```markdown
# MXBattery dogfood checklist

This document describes how to manually verify the app end-to-end on real hardware.
Implementation tasks do not gate on these checks; they are run separately by the
maintainer once the implementation is complete.

## Build

```bash
./tools/make-app.sh
open target/release/MXBattery.app
```

## TCC

- First run on `open MXBattery.app`: Bluetooth permission prompt should appear. Allow it.
- First decided-fire of a notification: notifications permission prompt should appear. Allow it.

## Battery + charging signals

- Menu bar icon updates within ~2 s of plug or unplug.
- Charging bolt overlay appears within ~1 s of plug-in and disappears within ~1 s of unplug.

## Notifications

- **Warn:** drop battery below 20 % while discharging — exactly one notification fires within 5 s. A second drop on the same day stays silent.
- **Critical:** drop below 5 % — first critical notification fires immediately; a second fires 30 min later if still below.
- **Disable warn in prefs:** dropping below 20 % does not fire a warn, but still fires critical at 5 %.
- **Disable critical in prefs:** dropping below 5 % fires nothing.
- **Re-arm:** charge above `warn + rearm_hysteresis` then drop again on the next day — warn fires again.
- **Quit + relaunch:** state file persists `last_warn_notified_date` so a relaunch on the same day does not re-fire.

## Mute today

- Clicking "Mute today" replaces the menu item title with "Unmute" and silences the rest of today. Local midnight clears it.

## Prefs UI

- Validation: dragging warn down is bounded by `critical + 1`; dragging critical up is bounded by `warn − 1`.
- Hot-reload: editing `config.toml` directly is reflected within ~1 s without a daemon restart.
- `mxbattery prefs` from a terminal opens (or focuses) the prefs window.
- Double-clicking `MXBattery.app` while the daemon is already running brings up prefs.

## launchd

- `mxbattery install /Applications/MXBattery.app` registers the LaunchAgent. Daemon is alive after `launchctl bootstrap` (visible in `launchctl print gui/$UID/com.vincentahrend.mxbattery-app`).
- `mxbattery uninstall` removes the LaunchAgent and stops the daemon.
- `mxbattery uninstall --purge` also removes `~/Library/Application Support/MXBattery`.
- Log out and back in: the daemon starts without intervention.

## Frame-shape auto-probe

- Force a wrong shape locally by editing `Inner::default()` in `src/battery/cb.rs` to start at `WithDevIdx19`. Rebuild, run, observe two retries in the log before settling on `NoDevIdx18`. Revert before committing.

## Tag

When every section above passes:

```bash
git tag v0.1.0
git push --tags
```
```

- [ ] **Step 2: Commit**

```bash
git add docs/dogfood-checklist.md
git commit -m "docs: dogfood checklist"
```

---

### Task 24: Remove probes from the repo

The probes in `probe-a/` and `probe-b/` were exploratory artefacts used while writing the spec. Now that the production codebase covers the same ground (and `tests/hidpp.rs` carries the captured fixtures), the probes are no longer needed.

**Files:**
- Remove: `probe-a/`
- Remove: `probe-b/`
- Modify: `.gitignore` (drop probe-related entries)

- [ ] **Step 1: Confirm the captured frame fixtures live in `tests/hidpp.rs`**

Run: `grep -n 'hex::decode' tests/hidpp.rs tests/hidpp_event.rs`
Expected: hex strings preserved as test inputs (the same fixtures that were originally captured by probe-c).

- [ ] **Step 2: Remove the probe directories**

```bash
git rm -r probe-a probe-b
```

- [ ] **Step 3: Trim `.gitignore`**

Open `.gitignore` and remove every line under the "probe binaries" comment. The comment line itself can also be removed.

- [ ] **Step 4: Verify the working tree is clean**

Run: `git status`
Expected: only the `.gitignore` modification staged; no untracked probe leftovers.

- [ ] **Step 5: Commit + push**

```bash
git add .gitignore
git commit -m "chore: remove probe artefacts (captured fixtures live in tests/)"
git push
```

---

## Self-review notes (left in for the executing engineer)

- Spec section "Out of scope (v1)" includes "Localisation" — the plan does not add any l10n. Correct.
- Spec section "Re-arm hysteresis" — handled in Task 9 by `clear_armed_if_rose`, called from `app::handle_reading` (Task 22).
- Spec section "Frame quirk to document" — `hidpp::FrameShape` enum + auto-probe in Task 13 covers it.
- Spec section "Open integration unknowns" — the `applicationShouldHandleReopen` hook (Task 17 step 4 / Task 22) and the LaunchAgent log paths (Task 20 / Task 22) are in place. The third unknown ("does the menu bar render at login before the user is fully active?") is resolved naturally by `KeepAlive=true`: launchd will restart the daemon if `NSStatusBar` is unavailable on first try.
- Type consistency: `ChargingState` lives in `state.rs` and is used throughout; `ChargingStateRaw` lives in `hidpp.rs` and is mapped via `map_charging` in `battery::mod.rs`. No collisions.
- No placeholders detected on a final scan.
