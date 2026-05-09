//
//  MXBatteryIcon.swift
//  MXBattery
//
//  Source-of-truth drawing code for the menu bar icon.
//  Mirrors the SVGs in handoff/svg/ — if you change anything here, regenerate
//  the SVGs from this file (or vice versa).
//
//  Coordinate system: 16 wide × 22 tall (SVG y-down). The drawing helpers map
//  this onto the AppKit destination rect, so callers can use any size.
//

import AppKit

public enum MXBatteryIcon {

    // MARK: - Public API

    public enum State: Equatable {
        case connected(percent: Int, charging: Bool)
        case disconnected
    }

    /// Standard menu-bar canvas. macOS handles retina scaling automatically.
    public static let canvasSize = NSSize(width: 16, height: 22)

    /// Build an `NSImage` for a given state. Captures the *current* drawing
    /// appearance — re-create the image when the effective appearance changes
    /// (see `MXBatteryStatusController` for the wiring).
    public static func image(for state: State) -> NSImage {
        let img = NSImage(size: canvasSize, flipped: false) { rect in
            draw(state: state, in: rect)
            return true
        }
        img.isTemplate = false   // multi-color (green fill, bright bolt) — not a template
        return img
    }

    /// Draw the icon directly into the current `NSGraphicsContext`. Useful if
    /// you want to host the icon in a custom `NSView` instead of an `NSImage`.
    public static func draw(state: State, in rect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        ctx.saveGState()
        defer { ctx.restoreGState() }

        // Map SVG (y-down) coords onto the destination rect.
        let sx = rect.width / canvasSize.width
        let sy = rect.height / canvasSize.height
        ctx.translateBy(x: rect.minX, y: rect.maxY)
        ctx.scaleBy(x: sx, y: -sy)

        switch state {
        case .connected(let pct, let charging):
            drawConnected(percent: pct, charging: charging, ctx: ctx)
        case .disconnected:
            drawDisconnected(ctx: ctx)
        }
    }

    // MARK: - Bucketing

    public enum Bucket: Equatable {
        case critical   // < 5
        case warning    // < 20
        case medium     // < 60
        case good       // < 80
        case full       // 100

        public static func from(percent: Int) -> Bucket {
            switch percent {
            case ..<5:  return .critical
            case ..<20: return .warning
            case ..<60: return .medium
            case ..<80: return .good
            default:    return .full
            }
        }

        /// Visualization fill height (0..100). Decoupled from raw percent so
        /// `<5` is still legibly visible (15% drawn) and `<20` reads bigger
        /// than `<5` without overlapping the `<60` bucket visually.
        var renderPercent: CGFloat {
            switch self {
            case .critical: return 15
            case .warning:  return 32
            case .medium:   return 50
            case .good:     return 70
            case .full:     return 100
            }
        }

        var isLow: Bool { self == .critical || self == .warning }
    }

    // MARK: - Palette
    //
    // sRGB approximations of the oklch values used in the design canvas.
    // Override per-app by reassigning these statics before the first draw.

    public static var red    = NSColor(srgbRed: 0.831, green: 0.196, blue: 0.157, alpha: 1) // oklch(0.60 0.21 27)
    public static var yellow = NSColor(srgbRed: 0.984, green: 0.784, blue: 0.290, alpha: 1) // oklch(0.86 0.17 95)
    public static var green  = NSColor(srgbRed: 0.318, green: 0.706, blue: 0.435, alpha: 1) // oklch(0.66 0.16 148)
    public static var bright = NSColor(srgbRed: 0.965, green: 0.965, blue: 0.965, alpha: 1) // oklch(0.97 0 0)

    /// The silhouette + scroll-wheel ride on `NSColor.labelColor` so they
    /// follow the menu bar tint in light/dark/HC modes automatically.
    public static var foreground: NSColor { NSColor.labelColor }

    // MARK: - Drawing internals

    private static func drawConnected(percent: Int, charging: Bool, ctx: CGContext) {
        let bucket = Bucket.from(percent: percent)
        let fillH = 18 * bucket.renderPercent / 100
        let fy: CGFloat = 20 - fillH

        let fillColor: NSColor
        if charging {
            fillColor = green
        } else {
            switch bucket {
            case .critical: fillColor = red
            case .warning:  fillColor = yellow
            default:        fillColor = foreground
            }
        }

        let silhouette = silhouettePath()

        // 1. Fill clipped to silhouette, with wheel + (optional) bolt knocked out.
        ctx.saveGState()
        ctx.addPath(silhouette)
        ctx.clip()
        ctx.setFillColor(fillColor.cgColor)
        ctx.fill(CGRect(x: 2, y: fy, width: 12, height: fillH))
        // Knock the scroll-wheel out so it reads as negative space within the fill.
        ctx.setBlendMode(.clear)
        ctx.setLineWidth(1.4)
        ctx.setLineCap(.round)
        ctx.move(to: CGPoint(x: 8, y: 4.4))
        ctx.addLine(to: CGPoint(x: 8, y: 7.4))
        ctx.strokePath()
        if charging {
            ctx.addPath(boltPath(at: CGPoint(x: 5.5, y: 8.8)))
            ctx.fillPath()
        }
        ctx.setBlendMode(.normal)
        ctx.restoreGState()

        // 2. Silhouette outline.
        ctx.saveGState()
        ctx.addPath(silhouette)
        ctx.setStrokeColor(foreground.cgColor)
        ctx.setLineWidth(1.3)
        ctx.strokePath()
        ctx.restoreGState()

        // 3. Wheel as a positive line, only in the empty area above the fill.
        if fy > 0 {
            ctx.saveGState()
            ctx.clip(to: CGRect(x: 0, y: 0, width: canvasSize.width, height: fy))
            ctx.setStrokeColor(foreground.cgColor)
            ctx.setLineWidth(1.3)
            ctx.setLineCap(.round)
            ctx.move(to: CGPoint(x: 8, y: 4.4))
            ctx.addLine(to: CGPoint(x: 8, y: 7.4))
            ctx.strokePath()
            ctx.restoreGState()
        }

        // 4. Charging bolt.
        guard charging else { return }
        if bucket.isLow {
            // Split treatment: positive currentColor above the fill (knockout
            // below has already been done in step 1).
            if fy > 0 {
                ctx.saveGState()
                ctx.clip(to: CGRect(x: 0, y: 0, width: canvasSize.width, height: fy))
                ctx.setFillColor(foreground.cgColor)
                ctx.addPath(boltPath(at: CGPoint(x: 5.5, y: 8.8)))
                ctx.fillPath()
                ctx.restoreGState()
            }
        } else {
            // ≥20: solid bright bolt drawn on top of the green fill.
            ctx.setFillColor(bright.cgColor)
            ctx.addPath(boltPath(at: CGPoint(x: 5.5, y: 8.8)))
            ctx.fillPath()
        }
    }

    private static func drawDisconnected(ctx: CGContext) {
        let dim = foreground.withAlphaComponent(0.45)
        ctx.saveGState()
        ctx.addPath(silhouettePath())
        ctx.setStrokeColor(dim.cgColor)
        ctx.setLineWidth(1.3)
        ctx.strokePath()
        ctx.restoreGState()

        ctx.saveGState()
        ctx.setStrokeColor(dim.cgColor)
        ctx.setLineWidth(1.3)
        ctx.setLineCap(.round)
        ctx.move(to: CGPoint(x: 8, y: 4.4))
        ctx.addLine(to: CGPoint(x: 8, y: 7.4))
        ctx.strokePath()
        ctx.restoreGState()

        // Diagonal slash — full-strength label color so the disconnected
        // signal stays crisp against any menu bar background.
        ctx.saveGState()
        ctx.setStrokeColor(foreground.cgColor)
        ctx.setLineWidth(1.6)
        ctx.setLineCap(.round)
        ctx.move(to: CGPoint(x: 13.5, y: 3.5))
        ctx.addLine(to: CGPoint(x: 2.5, y: 18.5))
        ctx.strokePath()
        ctx.restoreGState()
    }

    private static func silhouettePath() -> CGPath {
        let p = CGMutablePath()
        p.move(to: CGPoint(x: 8, y: 2.4))
        p.addCurve(to: CGPoint(x: 13.6, y: 11),
                   control1: CGPoint(x: 12.2, y: 2.4),
                   control2: CGPoint(x: 13.6, y: 5.6))
        p.addCurve(to: CGPoint(x: 8, y: 19.6),
                   control1: CGPoint(x: 13.6, y: 16.6),
                   control2: CGPoint(x: 12.0, y: 19.6))
        p.addCurve(to: CGPoint(x: 2.4, y: 11),
                   control1: CGPoint(x: 4.0, y: 19.6),
                   control2: CGPoint(x: 2.4, y: 16.6))
        p.addCurve(to: CGPoint(x: 8, y: 2.4),
                   control1: CGPoint(x: 2.4, y: 5.6),
                   control2: CGPoint(x: 3.8, y: 2.4))
        p.closeSubpath()
        return p
    }

    private static func boltPath(at origin: CGPoint) -> CGPath {
        let p = CGMutablePath()
        let t = CGAffineTransform(translationX: origin.x, y: origin.y)
        p.move(to: CGPoint(x: 3.4, y: 0), transform: t)
        p.addLine(to: CGPoint(x: 0, y: 5.2), transform: t)
        p.addLine(to: CGPoint(x: 2.6, y: 5.2), transform: t)
        p.addLine(to: CGPoint(x: 1.2, y: 9.4), transform: t)
        p.addLine(to: CGPoint(x: 5, y: 4), transform: t)
        p.addLine(to: CGPoint(x: 2.4, y: 4), transform: t)
        p.addLine(to: CGPoint(x: 3.6, y: 0), transform: t)
        p.closeSubpath()
        return p
    }
}
