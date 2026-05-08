# MXBattery

The Logitech MX Master 3 is a fantastic mouse but I was surprised once too often by the battery suddenly being empty. Curiously, I could see the battery level in the list of connected Bluetooth devices, although that number also seemed off sometimes. 

This is a small utility, which shows a macOS notification, by default when battery drops to 20% and another one at 5%. It also adds a menu bar icon showing the battery percentage, which can also be disabled.

The design of the tool is based on HID++ events and it's written in Rust, it uses very little resources. There is no polling: the daemon subscribes to BLE GATT notifications for the battery level and to spontaneous HID++ events for the charging state, so it sits at effectively 0 % CPU between events. The whole thing weighs about 2 MB on disk and 10–15 MB resident.

It only uses public CoreBluetooth APIs — no private frameworks, no `IOHID`, no Input Monitoring permission. All you have to grant on first launch is Bluetooth and Notifications.

![CI](https://github.com/cafca/mxbattery/actions/workflows/ci.yml/badge.svg)

## Usage

### Install

```bash
git clone https://github.com/cafca/mxbattery
cd mxbattery
./tools/make-app.sh
open target/release/MXBattery.app
```

The first launch prompts for Bluetooth and (on the first notification) Notifications permissions. Approve both.

To run automatically at login, register the LaunchAgent:

```bash
./target/release/MXBattery.app/Contents/MacOS/mxbattery install \
    "$(pwd)/target/release/MXBattery.app"
```

### Configure

Click the menu bar icon → **Preferences…**, or edit `~/Library/Application Support/MXBattery/config.toml` directly:

```toml
schema_version = 1

[device]
mode = "any-mx"   # specific | any-mx | any-logitech
# identifier = "798C4CBB-…"   # required when mode = "specific"

[thresholds]
warn = 20              # %; notification fires once per day below this
critical = 5           # %; notification re-fires every critical_period below this
rearm_hysteresis = 5   # %; battery must rise this far above a threshold to re-arm
warn_enabled = true
critical_enabled = true

[cadence]
warn_period = "24h"
critical_period = "30m"

[menubar]
enabled = true

[autostart]
enabled = false
```

Edits to the file are picked up by the running daemon within ~1 s via FSEvents — no restart needed.

### CLI

```
mxbattery                     run the daemon (used by launchd; can also be invoked directly)
mxbattery prefs               open or focus the preferences window
mxbattery read                print the last recorded battery + charging state
mxbattery install <APP>       install the LaunchAgent (pass the absolute path to MXBattery.app)
mxbattery uninstall           remove the LaunchAgent
mxbattery uninstall --purge   also delete config + state in ~/Library/Application Support/MXBattery
```

### Supported devices

Any Logitech BLE peripheral that exposes the standard Battery Service (`0x180F`) and the Logitech vendor GATT service (`00010000-0000-1000-8000-011F2000046D`). Confirmed working on the **MX Master 3 Mac**. Other MX-series devices should work; please open an issue if yours does not. Unifying-receiver devices are out of scope.

## Contributing

Contributions are welcome — but **please open an issue first** so we can agree on scope before any code is written. Drive-by PRs without prior discussion will likely be closed.

When filing a bug, please include:

- Your macOS version and Mac model.
- Device name and firmware version (`mxbattery read` plus the macOS Bluetooth menu).
- Daemon logs covering the issue, with `MXBATTERY_LOG=debug` set.

How to capture logs depends on how the daemon is running:

- **Installed via `mxbattery install` (launchd):** the agent writes to `~/Library/Application\ Support/MXBattery/daemon.log` and `daemon.err.log`. Set `EnvironmentVariables` → `MXBATTERY_LOG=debug` in `~/Library/LaunchAgents/com.vincentahrend.mxbattery-app.plist` and reload with `launchctl kickstart -k gui/$UID/com.vincentahrend.mxbattery-app`. Then tail `daemon.err.log`.
- **Running directly (terminal or `open`):** there is no log file by default — logs go to stderr. Capture with:

  ```bash
  killall mxbattery 2>/dev/null
  MXBATTERY_LOG=debug open --stderr /tmp/mxb.err target/release/MXBattery.app
  tail -F /tmp/mxb.err
  ```

The full design and implementation plan live under [`docs/superpowers/`](docs/superpowers/) for anyone who wants to understand the codebase before contributing.

## License

[MIT](LICENSE.md). © 2026 Vincent Ahrend.