//! macOS menu bar icon for MXBattery.
//!
//! Draws a custom battery glyph as a template NSImage so it inverts
//! automatically in dark mode, and shows a bolt overlay when charging.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{
    NSBezierPath, NSColor, NSImage, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{CGFloat, MainThreadMarker, NSPoint, NSRect, NSSize};

use crate::state::ChargingState;

const ICON_W: CGFloat = 22.0;
const ICON_H: CGFloat = 14.0;

pub struct MenubarIcon {
    item: Retained<NSStatusItem>,
}

impl MenubarIcon {
    pub fn install(_mtm: MainThreadMarker) -> Self {
        let bar = unsafe { NSStatusBar::systemStatusBar() };
        let item: Retained<NSStatusItem> =
            unsafe { bar.statusItemWithLength(NSVariableStatusItemLength) };
        Self { item }
    }

    pub fn render(&self, percent: u8, charging: ChargingState, mtm: MainThreadMarker) {
        let img = render_icon(percent, charging);
        unsafe {
            if let Some(button) = self.item.button(mtm) {
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
    let size = NSSize {
        width: ICON_W,
        height: ICON_H,
    };
    let block = RcBlock::new(move |_rect: NSRect| -> Bool {
        draw_battery(percent, charging);
        Bool::YES
    });
    let img: Retained<NSImage> = unsafe {
        NSImage::imageWithSize_flipped_drawingHandler(size, false, &block)
    };
    unsafe { img.setTemplate(true) };
    img
}

fn draw_battery(percent: u8, charging: ChargingState) {
    // Outer body outline
    let body_rect = NSRect::new(
        NSPoint { x: 1.0, y: 2.0 },
        NSSize {
            width: ICON_W - 4.0,
            height: ICON_H - 4.0,
        },
    );
    let body = unsafe {
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(body_rect, 2.0, 2.0)
    };
    unsafe {
        body.setLineWidth(1.0);
        NSColor::controlTextColor().setStroke();
        body.stroke();
    }

    // Terminal nub on the right
    let nub_rect = NSRect::new(
        NSPoint {
            x: ICON_W - 3.0,
            y: 5.0,
        },
        NSSize {
            width: 1.5,
            height: 4.0,
        },
    );
    let nub = unsafe { NSBezierPath::bezierPathWithRect(nub_rect) };
    unsafe {
        NSColor::controlTextColor().setFill();
        nub.fill();
    }

    // Fill level
    let (fill_frac, color) = bucket(percent);
    if fill_frac > 0.0 {
        unsafe { color.setFill() };
        let inner_w = (ICON_W - 6.0) * fill_frac;
        let fill_rect = NSRect::new(
            NSPoint { x: 2.0, y: 3.0 },
            NSSize {
                width: inner_w,
                height: ICON_H - 6.0,
            },
        );
        let fill = unsafe { NSBezierPath::bezierPathWithRect(fill_rect) };
        unsafe { fill.fill() };
    }

    // Bolt overlay when charging
    if !matches!(
        charging,
        ChargingState::Discharging | ChargingState::Unknown
    ) {
        draw_bolt();
    }
}

fn bucket(p: u8) -> (CGFloat, Retained<NSColor>) {
    unsafe {
        match p {
            0..=5 => (0.05, NSColor::systemRedColor()),
            6..=20 => (0.25, NSColor::systemYellowColor()),
            21..=60 => (0.50, NSColor::controlTextColor()),
            61..=80 => (0.75, NSColor::controlTextColor()),
            _ => (1.00, NSColor::controlTextColor()),
        }
    }
}

fn draw_bolt() {
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
        for p in &pts[1..] {
            path.lineToPoint(*p);
        }
        path.closePath();
        NSColor::controlTextColor().setFill();
        path.fill();
    }
}
