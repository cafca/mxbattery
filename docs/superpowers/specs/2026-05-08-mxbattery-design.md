# MXBattery — design

## Context

The macOS Bluetooth menubar reports the battery percentage of the user's Logitech MX Master 3 Mac, but it lags reality and at times reports stale or wrong values (the Reddit "always shows 0% even after charging for hours" complaint is well-documented and reproduced during this design's exploration phase: device reported 100% over GATT while menubar still said 5%). Logitech's own Logi Options+ daemon has the data but is heavy and runs even when not needed. The user wants a small open-source alternative that is event-driven, near-zero idle cost, and ships native macOS notifications when the battery falls below a threshold.

The goal of this project is a single-binary Rust app, packaged as a `.app` bundle, that:

- Connects to a configured Logitech BLE mouse over CoreBluetooth.
- Reads battery percent from the standard Battery Service characteristic (`0x2A19`).
- Reads charging state from HID++ 2.0 over the device's vendor GATT characteristic.
- Receives battery and charging changes as push notifications (no polling).
- Fires native notifications at configurable warn / critical thresholds, with daily and N-minute cadences respectively, plus rise-hysteresis to re-arm.
- Optionally shows a menu bar icon (battery glyph quantized by level, with a charging overlay).
- Optionally shows a native AppKit preferences window.

## Feasibility evidence

Two probe binaries (`probe-b/main.swift`, `probe-b/main_hidpp.swift`) confirm the data path with public CoreBluetooth APIs only. No Input Monitoring TCC needed; only the standard "App wants to use Bluetooth" prompt. Probe-b also confirmed that HID++ events fire spontaneously on plug-in / unplug with sub-second latency, eliminating the need for periodic polling.

## Architecture

A single Rust binary `mxbattery`, packaged as `MXBattery.app`. The bundle is required so that macOS TCC associates the Bluetooth and notification permissions with a stable identity (`Info.plist` carries `CFBundleIdentifier`, `NSBluetoothAlwaysUsageDescription`, `LSUIElement=1`). The binary embeds an `NSApplication` so that CoreBluetooth delegate callbacks, AppKit menu/window events, and `UNUserNotificationCenter` callbacks all share the same run loop.

The app runs under a per-user LaunchAgent (`~/Library/LaunchAgents/com.vincentahrend.mxbattery-app.plist`) with `RunAtLoad` and `KeepAlive`. The launchd plist points at the Mach-O binary inside the bundle (`/Applications/MXBattery.app/Contents/MacOS/mxbattery`), so launchd treats it as a managed daemon while macOS still resolves the bundle identity for TCC.

```
launchd -> MXBattery.app/Contents/MacOS/mxbattery (LSUIElement)
            |
            +-- battery (CBCentralManager + vendor-char HID++ codec)
            |     |
            |     v
            |   reading_stream (BatteryReading events)
            |     |
            |     +-> notifier (cadence/hysteresis -> UNUserNotificationCenter)
            |     +-> menubar (NSStatusItem icon)
            |     +-> prefs_ui (live percent in window)
            |
            +-- config (~/Library/Application Support/MXBattery/config.toml, FSEvents-watched)
            +-- state  (~/Library/Application Support/MXBattery/state.json, atomic writes)
            +-- ipc    (~/Library/Application Support/MXBattery/control.sock for `mxbattery prefs`)
```

## Data model

```rust
enum ChargingState { Discharging, Recharging, ChargeInFinalState, ChargeComplete, Other(u8) }

struct BatteryReading {
    percent: u8,           // from BAS 0x2A19
    charging: ChargingState, // from HID++ feature 0x1000 events
    ts: Instant,
}

struct Config {
    device: DeviceFilter,
    thresholds: Thresholds,
    cadence: Cadence,
    menubar: MenubarConfig,
    autostart: bool,
}

enum DeviceFilter {
    Specific { address: String },           // BLE peripheral identifier (UUID on macOS)
    AnyMx,                                  // Device Information 0x2A24 starts with "MX "
    AnyLogitech,                            // PnP ID 0x2A50 VID == 0x046D, or vendor service present
}

struct Thresholds {
    warn: u8,                               // default 20
    critical: u8,                           // default 5; INVARIANT: critical < warn
    rearm_hysteresis: u8,                   // default 5
    warn_enabled: bool,                     // default true
    critical_enabled: bool,                 // default true
}

struct Cadence {
    warn_period: Duration,                  // default 24h ("once per day while below")
    critical_period: Duration,              // default 30 min
}

struct MenubarConfig { enabled: bool }

struct State {
    last_warn_notified_date: Option<NaiveDate>,
    last_critical_notified_at: Option<DateTime<Local>>,
    mute_until: Option<DateTime<Local>>,
    last_seen: Option<BatteryReading>,
}
```

## Components

All in one crate, separated by module. Modules communicate via `tokio::sync::watch` channels for "current reading" and `tokio::sync::mpsc` for "events to handle".

### `battery`

Owns one `CBCentralManager` (via `objc2-core-bluetooth`). On startup:

1. Wait for `CBCentralManager.state == .poweredOn`.
2. `retrieveConnectedPeripherals(withServices: [0x180F, 0x180A, vendorServiceUUID])`.
3. Filter the returned peripherals against the configured `DeviceFilter` (after reading `0x2A24` model number / `0x2A50` PnP for the `AnyMx` / `AnyLogitech` modes).
4. `connect`, then `discoverServices([0x180F, vendorServiceUUID])`, then `discoverCharacteristics`.
5. `setNotifyValue(true)` on `0x2A19` (battery percent) and on the vendor characteristic.
6. Read `0x2A19` once to seed percent.
7. Send one `getBatteryLevelStatus` HID++ frame to seed charging state.
8. Forward every push update as a `BatteryReading` on the watch channel.

On disconnect: emit a `BatteryReading::Disconnected` event (so the menu bar icon can dim), keep the central manager alive, and rely on macOS to call back when the peripheral reconnects.

### `hidpp`

Pure codec, no I/O. Two functions:

- `encode_get_feature(feature_id: u16, swid: u8) -> [u8; 18]` — emits `[root_feat_idx=0, fn|swid, hi, lo, 0…]`.
- `encode_get_battery_level_status(feat_idx: u8, swid: u8) -> [u8; 18]` — emits `[feat_idx, fn|swid, 0…]`.
- `decode(buf: &[u8]) -> Option<HidppFrame>` — returns `{feat_idx, fn, swid, params}`.

The MX Master 3 Mac's BLE pipe accepts 18-byte frames with `[feat_idx, fn|swid, params...]` and **no devIdx prefix**. `swid=0` on incoming frames marks unsolicited events (this is the device telling us a status changed). To stay future-compatible, the encoder is parameterised on a `FrameShape` enum (`NoDevIdx18`, `WithDevIdx19`, `WithReportId20`) so a different device can be supported by configuring or auto-probing the shape on connect. Auto-probe runs the same logic that probe-c used: try `NoDevIdx18` first; if the write returns `CBATTErrorDomain Code=13` ("invalid length"), step through alternates; if the write succeeds but no response arrives in 2s, also step.

Stateless and trivially unit-testable with hex fixtures captured from the probe.

### `device_filter`

Given a peripheral and the `DeviceFilter` config, decide whether to bind. Reads `0x2A24` (Model Number string) and `0x2A50` (PnP ID) once after connect for `AnyMx` / `AnyLogitech` checks. The `Specific` mode matches on the macOS-assigned peripheral identifier (the UUID-shaped value `798C4CBB-...` rather than the BD_ADDR, since macOS does not expose raw addresses). On first run with no device configured, the prefs UI shows a list of currently-connected Logitech BLE peripherals for the user to pick.

### `config`

TOML at `~/Library/Application Support/MXBattery/config.toml`:

```toml
schema_version = 1

[device]
mode = "specific"   # specific | any-mx | any-logitech
identifier = "798C4CBB-C8BE-9C3C-F286-195F08C01978"   # required when mode = "specific"

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
enabled = true
```

Loaded at startup. Watched via the `notify` crate (FSEvents). On change, the new config is parsed and swapped into an `arc_swap::ArcSwap<Config>`. Bad TOML triggers a fallback to the previous good config and emits a notification with the parse error so the user knows to fix it.

### `state`

JSON at `~/Library/Application Support/MXBattery/state.json`. Atomic write via `tempfile` + rename. Fields per the data model. Read at startup; written whenever a notification fires or `mute_until` changes.

### `notifier`

Pure decision function:

```rust
fn decide(reading: &BatteryReading, state: &State, cfg: &Config, now: DateTime<Local>) -> Option<NotificationKind>
```

Returns `Some(Warn)`, `Some(Critical)`, or `None`. Logic:

- If `now < mute_until`: return `None`.
- If `reading.charging` is anything but `Discharging`: never notify (charging means level will rise).
- Compute "below warn" and "below critical" against `reading.percent`.
- For warn (only if `warn_enabled`): if `percent <= warn` and (`last_warn_notified_date` is None or different from today's date), fire `Warn`.
- For critical (only if `critical_enabled`): if `percent <= critical` and (`last_critical_notified_at` is None or `now - last >= critical_period`), fire `Critical`.
- Re-arm hysteresis: when `percent` rises to `warn + rearm_hysteresis` or higher, clear `last_warn_notified_date`. Same for critical at `critical + rearm_hysteresis`.
- Critical wins over warn: if both would fire on the same reading (because critical < warn), only the `Critical` notification is posted.

Config invariant: `critical < warn`. Enforced at parse time (rejects the new config and falls back to the previous good one) and at the prefs UI (the warn stepper's minimum is dynamic, set to `critical + 1`; the critical stepper's maximum is `warn - 1`).

Side-effecting wrapper applies the decision: posts a `UNNotificationRequest` via `objc2-user-notifications` and updates state.

Critical alerts use the standard `UNNotificationCategory` (no `.criticalAlert` entitlement, since that requires an Apple-issued cert and is out of scope for an OSS project; documented as future work).

### `menubar`

`NSStatusItem` with a custom `NSImage` rendered from a small SF-Symbol-style battery glyph. Five quantized fill levels plus charging overlay:

| `percent` | fill | tint |
|---|---|---|
| 0–5 | 0/4 | systemRed |
| 6–20 | 1/4 | systemYellow |
| 21–60 | 2/4 | controlTextColor (auto light/dark) |
| 61–80 | 3/4 | controlTextColor |
| 81–100 | 4/4 | controlTextColor |

When `charging != Discharging`, overlay a small bolt glyph in the same template-image color so it inverts in dark mode automatically.

Click menu (constructed once, items toggled live):

- **Mute today** — toggles `mute_until` to next local midnight, or back to `None` if currently muted.
- **Preferences…** — opens prefs window (or focuses it).
- **Quit** — `NSApp.terminate`.

Menu bar item is hidden entirely when `config.menubar.enabled = false`. Toggling the config value at runtime adds/removes the `NSStatusItem` without restart.

### `prefs_ui`

`NSWindow` (titled, closable, not resizable). `NSStackView` form with the following rows:

- Device: dropdown of connected Logitech BLE peripherals + "Any MX" / "Any Logitech".
- Warn notifications: checkbox + stepper for threshold (range `critical + 1 … 99`). Stepper disabled when checkbox off.
- Critical notifications: checkbox + stepper for threshold (range `1 … warn - 1`). Stepper disabled when checkbox off.
- Re-arm hysteresis: stepper, 1–20.
- Critical re-notify period: stepper, 1–240 minutes.
- Show menu bar icon: checkbox.
- Start at login: checkbox (toggles the LaunchAgent).

Save is disabled (button greyed) whenever the form would violate the `critical < warn` invariant (this is normally prevented by the dynamic stepper ranges, but the guard catches direct text entry).

Save commits the new TOML (atomic write). The daemon picks it up via FSEvents within ~1s. Cancel discards. Close (red traffic light) is equivalent to Cancel.

The window is also opened from `applicationShouldHandleReopen` (so double-clicking the .app brings up prefs while the daemon is already running) and from the `mxbattery prefs` CLI subcommand (which sends "open prefs" over the control socket).

### `ipc`

Unix-domain socket at `~/Library/Application Support/MXBattery/control.sock`. Two purposes:

1. Single-instance lock: if a second process tries to bind the socket, it knows another instance is running and instead sends a one-line message before exiting.
2. CLI control: `mxbattery prefs` connects, sends `{"cmd": "open_prefs"}`, then exits. The running daemon reads the line and brings up the prefs window.

If the daemon is not running, the same `mxbattery prefs` invocation falls through to running the prefs UI in-process (single-shot mode) so the user can edit config without the daemon active.

### `launchd_install`

Subcommands:

- `mxbattery install` — copies `MXBattery.app` to `/Applications` (if invoked from elsewhere), writes the LaunchAgent plist, calls `launchctl bootstrap gui/$UID ~/Library/LaunchAgents/com.vincentahrend.mxbattery-app.plist`.
- `mxbattery uninstall` — `launchctl bootout`, removes the plist and (with `--purge`) the config + state directory.
- `mxbattery prefs` — see `ipc` above.
- (default, no subcommand) — runs the daemon. This is what the LaunchAgent invokes.

### `app`

`main`. Command parsing (`clap`), then dispatch. For the default daemon path: install signal handlers (SIGTERM clean shutdown), build `NSApplication` with `setActivationPolicy(.accessory)`, set the app delegate, instantiate config, state, battery, notifier, menubar, and prefs window. Wire the watch/mpsc channels. Run the AppKit run loop.

## Crate choices

| Crate | Purpose |
|---|---|
| `objc2`, `objc2-foundation`, `objc2-app-kit` | NSApplication, NSStatusItem, NSWindow, NSStackView, NSImage |
| `objc2-core-bluetooth` | CBCentralManager, CBPeripheral, CBCharacteristic |
| `objc2-user-notifications` | UNUserNotificationCenter |
| `tokio` | runtime, channels, signal handling |
| `serde`, `toml`, `serde_json` | config + state |
| `arc-swap` | hot-reloadable config |
| `notify` | FSEvents on config dir |
| `chrono` | Local midnight calculation, dates |
| `clap` | CLI parsing |
| `tracing`, `tracing-subscriber` | logging to launchd's stderr file |
| `tempfile` | atomic state writes |
| `dirs` | Library/Application Support path resolution |

Estimated release binary size: 3–6 MB stripped. Estimated steady-state memory: 8–15 MB RSS (dominated by Foundation/AppKit overhead). CPU at idle: effectively 0 — the daemon blocks on the run loop and only wakes on BLE callbacks (sub-second on plug change, otherwise rare).

## Bundle layout

```
MXBattery.app/
  Contents/
    Info.plist                # CFBundleIdentifier=com.vincentahrend.mxbattery-app, LSUIElement=1, NSBluetoothAlwaysUsageDescription
    MacOS/
      mxbattery               # the Rust binary
    Resources/
      AppIcon.icns
      menubar-battery-template.pdf  # template image used for the status item
```

Build script `tools/make-app.sh`:
1. `cargo build --release --target aarch64-apple-darwin`
2. Assemble bundle from a template
3. Copy binary, icons, plist
4. `codesign --force --sign - MXBattery.app` (ad-hoc for self-build; signing identity overridable via env var for users who have a Developer ID)

For x86_64 support, build a universal binary via `lipo`. Optional, can be added later.

## Distribution

- GitHub repo `MXBattery`, MIT-licensed.
- v1 (dogfood): ad-hoc-signed `.app`, user must right-click → Open on first run. The repo includes a `tools/make-app.sh` script anyone can run locally.
- v2: notarised universal `MXBattery.app.zip` on GitHub Releases. The author has a paid Apple Developer ID, so the build script is parameterised on `DEVELOPER_ID` / `NOTARY_KEYCHAIN_PROFILE` env vars; signing + notarisation flips on automatically when those are set. Until v2, the dogfood build is the only target.
- Homebrew cask later (post-v2): `brew install --cask mxbattery`.

## Testing

- Unit tests for `hidpp` (frame round-trip with hex fixtures from probe-c output).
- Unit tests for `notifier::decide` (table-driven across percent, charging, time, mute_until, last_notified_*).
- Unit tests for `state` atomic write (concurrent writers).
- Unit tests for `config` parsing including hot-reload of valid and malformed TOML.
- Integration test via a `MockCentralManager` trait that replays a recorded sequence of CoreBluetooth callbacks (so we can simulate connect / read 0x2A19 / receive a 0x1000 event without a real device).
- Manual end-to-end checklist (run on the user's machine):
  - First launch: TCC Bluetooth prompt appears; allow it; device shows in prefs picker.
  - Battery falls below 20% while discharging: warn notification fires once that day.
  - Battery falls below 5%: critical notification fires; second one fires 30 min later if still below.
  - Plug in: charging icon overlay appears within ~1s; no further notifications fire.
  - Charge above 25%: warn re-arms (verifiable by then unplugging and dropping below 20% again on a subsequent day).
  - Mute today: clicking the menu item silences notifications for the rest of the day; midnight resets it.
  - Disable menu bar in prefs: status item disappears immediately.
  - `mxbattery prefs` from a terminal while daemon is running: prefs window opens and focuses.
  - Quit from menu, reopen via `launchctl kickstart`: state and config preserved.

## Out of scope (v1 — dogfood)

- Critical-alert (DND-bypassing) notifications. The user has a paid Apple Developer account, but `.criticalAlert` additionally requires an Apple-approved entitlement application; deferred to a later version.
- Notarisation. Defer to v2; build script is parameterised so the flip is one env-var change.
- Logitech Unifying receiver support. This is BLE-only by design; receiver users need a code path through `hidraw`-equivalents that don't exist on macOS without IOHID.
- Multi-device monitoring. The architecture supports this (one `BatteryStream` per peripheral) but the prefs UI and notification dedup are designed for a single configured device. v1 ships single-device.
- Localisation.

## Open integration unknowns

- Whether all BLE-HID++ Logitech mice use the same 18-byte no-devIdx frame shape as the MX Master 3 Mac. The `hidpp` module's auto-probe handles divergence, but this needs verification when broadening "Any MX" / "Any Logitech" support.
- Whether macOS will prompt for Bluetooth TCC again when launchd starts the bundle vs. when the user double-clicks it. The probe verified that double-click works; first launch under launchd before user interaction may need a one-time "open the .app once" step in the install instructions.
- Whether `NSStatusItem` works correctly when the LaunchAgent starts at login *before* the user has fully logged in (some sessions report no menu bar host yet). If problematic, we add a brief retry loop in `menubar`.
