//
//  MXBatteryStatusController.swift
//  MXBattery
//
//  Owns the NSStatusItem and keeps its image in sync with battery state and
//  the system appearance. Drop one of these into your AppDelegate (or wherever
//  the app's lifecycle lives) and call `update(state:)` from your battery poll.
//

import AppKit

public final class MXBatteryStatusController {

    public let statusItem: NSStatusItem

    private var appearanceObservation: NSKeyValueObservation?
    private var currentState: MXBatteryIcon.State = .disconnected

    public init() {
        self.statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        self.statusItem.behavior = .removalAllowed

        if let button = statusItem.button {
            button.imagePosition = .imageOnly
            button.image = MXBatteryIcon.image(for: currentState)

            // Re-render whenever the menu bar's effective appearance changes
            // (light ↔ dark ↔ HC). NSImage caches its bitmap at creation time,
            // so we replace the image entirely.
            appearanceObservation = button.observe(\.effectiveAppearance,
                                                   options: [.new]) { [weak self] _, _ in
                self?.refresh()
            }
        }
    }

    deinit {
        appearanceObservation?.invalidate()
        NSStatusBar.system.removeStatusItem(statusItem)
    }

    /// Update the displayed state. No-op if unchanged.
    public func update(state: MXBatteryIcon.State) {
        guard state != currentState else { return }
        currentState = state
        refresh()
    }

    /// Force a redraw — call after mutating `MXBatteryIcon.red/yellow/green/bright`.
    public func refresh() {
        guard let button = statusItem.button else { return }
        let render = {
            button.image = MXBatteryIcon.image(for: self.currentState)
        }
        if #available(macOS 11, *) {
            button.effectiveAppearance.performAsCurrentDrawingAppearance(render)
        } else {
            render()
        }
    }
}
