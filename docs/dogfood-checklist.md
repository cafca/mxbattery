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
