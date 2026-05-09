# MXBattery menu bar icon — handoff

Drop-in package for the **Silhouette fill** menu bar icon: top-down mouse outline,
charge level rises inside the body, scroll-wheel hairline above the fill and
knocked out of the fill, charging bolt rendered in two treatments depending on
charge bucket.

## Files

```
handoff/
├── MXBatteryIcon.swift           ← drawing code (source of truth)
├── MXBatteryStatusController.swift ← NSStatusItem wiring + appearance observer
└── svg/                          ← 11 reference SVGs (regen from Swift if changed)
    ├── critical-05.svg              · critical-05-charging.svg
    ├── warning-20.svg               · warning-20-charging.svg
    ├── medium-60.svg                · medium-60-charging.svg
    ├── good-80.svg                  · good-80-charging.svg
    ├── full-100.svg                 · full-100-charging.svg
    └── disconnected.svg
```

The Swift drawing code is the source of truth. The SVGs are inspection-only
(opening them in a browser is the fastest way to eyeball the design); regenerate
them if you change `MXBatteryIcon.swift`.

## Buckets

The app reports a raw percent; the icon snaps to one of five buckets:

| Bucket    | Range   | Render | Resting fill        | Charging fill |
| ---       | ---     | ---    | ---                 | ---           |
| critical  | `< 5`   | 15%    | red                 | green + bolt  |
| warning   | `< 20`  | 32%    | yellow              | green + bolt  |
| medium    | `< 60`  | 50%    | label color         | green + bolt  |
| good      | `< 80`  | 70%    | label color         | green + bolt  |
| full      | `100`   | 100%   | label color         | green + bolt  |

Plus a **disconnected** state (silhouette dimmed, label-color slash on top) for
when no MX mouse is paired.

> Color is bucket-driven, not render-driven. A `<5` bucket stays red even though
> we draw a 15%-tall fill so it remains visible at small sizes.

## Charging bolt

- **`<5` and `<20`:** split treatment. The bolt is positive `currentColor` in
  the empty area above the fill, and a knockout in the fill below — so it stays
  legible against both the empty silhouette and the small green fill.
- **`<60`, `<80`, `100`:** solid bright bolt (`bright`, ~`oklch(0.97 0 0)`)
  drawn on top of the green fill. Same warm white in light and dark themes.

## Wheel hairline

The scroll-wheel mark is rendered in two pieces around the fill line:

- a positive `currentColor` line, clipped to the area above the fill;
- a knockout in the fill (via `.clear` blend mode).

So at any charge level the wheel reads cleanly: positive in the empty area,
negative inside the fill — including at 100% where the silhouette is fully
filled and the wheel is the only legibility detail.

## Integration

```swift
// AppDelegate.swift
import AppKit

final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusController: MXBatteryStatusController!

    func applicationDidFinishLaunching(_ notification: Notification) {
        statusController = MXBatteryStatusController()

        // Wire up your menu / popover here:
        // statusController.statusItem.menu = buildMenu()
        // or:
        // statusController.statusItem.button?.action = #selector(togglePopover(_:))
        // statusController.statusItem.button?.target = self
    }

    /// Call from your existing battery poll.
    func batteryDidUpdate(percent: Int, charging: Bool, connected: Bool) {
        statusController.update(state: connected
            ? .connected(percent: percent, charging: charging)
            : .disconnected)
    }
}
```

`MXBatteryStatusController` observes `effectiveAppearance` on the status
button and re-renders whenever the system flips light ↔ dark ↔ high-contrast.
You don't need to do anything for theme changes.

## Customizing colors

The four palette colors live as `static var`s on `MXBatteryIcon`. Override them
once at startup if you want different shades:

```swift
MXBatteryIcon.red    = NSColor(named: "BatteryRed")    ?? MXBatteryIcon.red
MXBatteryIcon.yellow = NSColor(named: "BatteryYellow") ?? MXBatteryIcon.yellow
MXBatteryIcon.green  = NSColor(named: "BatteryGreen")  ?? MXBatteryIcon.green
MXBatteryIcon.bright = NSColor(named: "BoltBright")    ?? MXBatteryIcon.bright
statusController.refresh()
```

The silhouette and wheel always use `NSColor.labelColor` so they track the menu
bar tint automatically.

## Sizing

The canvas is 16×22pt. AppKit handles retina scaling — `NSImage` is vector
internally because the drawing handler is invoked per backing scale. If you
want to render the icon somewhere other than the menu bar (e.g. a settings
popover), pass any `NSRect` to `MXBatteryIcon.draw(state:in:)`.

## SVGs

Each SVG uses `currentColor` for the silhouette/wheel and named oklch fills
for the colored stages. The viewBox is `0 0 16 22`. To preview them in your
browser:

```bash
open handoff/svg/medium-60-charging.svg
```

If you want the SVGs to auto-tint to your app's foreground color when embedded
in HTML (e.g. a help page), wrap them in an element with `color: <your color>`
— `currentColor` will inherit.
