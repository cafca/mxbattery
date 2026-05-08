//! macOS menu bar icon for MXBattery.
//!
//! Draws a custom battery glyph as a template NSImage so it inverts
//! automatically in dark mode, and shows a bolt overlay when charging.
//! Also installs a click menu with Mute today / Preferences / Quit items.

use std::cell::Cell;

use block2::RcBlock;
use objc2::mutability::InteriorMutable;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::{declare_class, msg_send_id, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{CGFloat, MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};
use tokio::sync::mpsc::UnboundedSender;

use crate::state::ChargingState;

// ---------------------------------------------------------------------------
// Public command type
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub enum MenubarCommand {
    ToggleMuteToday,
    OpenPrefs,
    Quit,
}

// ---------------------------------------------------------------------------
// ObjC MenuTarget class
// ---------------------------------------------------------------------------

struct MenuTargetIvars {
    sender: Cell<Option<UnboundedSender<MenubarCommand>>>,
}

declare_class!(
    struct MenuTarget;

    unsafe impl ClassType for MenuTarget {
        type Super = NSObject;
        type Mutability = InteriorMutable;
        const NAME: &'static str = "MXBatteryMenuTarget";
    }

    impl DeclaredClass for MenuTarget {
        type Ivars = MenuTargetIvars;
    }

    unsafe impl MenuTarget {
        #[method(handleMuteToday:)]
        fn handle_mute_today(&self, _sender: *mut NSObject) {
            self.send(MenubarCommand::ToggleMuteToday);
        }

        #[method(handlePrefs:)]
        fn handle_prefs(&self, _sender: *mut NSObject) {
            self.send(MenubarCommand::OpenPrefs);
        }

        #[method(handleQuit:)]
        fn handle_quit(&self, _sender: *mut NSObject) {
            self.send(MenubarCommand::Quit);
        }
    }
);

impl MenuTarget {
    fn new(sender: UnboundedSender<MenubarCommand>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(MenuTargetIvars {
            sender: Cell::new(Some(sender)),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    fn send(&self, cmd: MenubarCommand) {
        tracing::info!(?cmd, "menubar: click");
        // Take temporarily, send, put back.
        let opt = self.ivars().sender.take();
        if let Some(ref tx) = opt {
            match tx.send(cmd) {
                Ok(()) => tracing::debug!("menubar: command dispatched"),
                Err(e) => tracing::warn!(?e, "menubar: send failed"),
            }
        } else {
            tracing::warn!("menubar: no sender set; click dropped");
        }
        self.ivars().sender.set(opt);
    }
}

// ---------------------------------------------------------------------------
// Public MenubarIcon struct
// ---------------------------------------------------------------------------

const ICON_W: CGFloat = 22.0;
const ICON_H: CGFloat = 14.0;

pub struct MenubarIcon {
    item: Retained<NSStatusItem>,
    /// Held alive so it doesn't get deallocated while the menu exists.
    _target: Retained<MenuTarget>,
    /// Strong ref to the "Mute today" menu item for dynamic label updates.
    mute_item: Retained<NSMenuItem>,
    /// Strong ref to the disabled status row showing current battery percent + state.
    status_item: Retained<NSMenuItem>,
}

impl MenubarIcon {
    pub fn install(mtm: MainThreadMarker) -> Self {
        let bar = unsafe { NSStatusBar::systemStatusBar() };
        let item: Retained<NSStatusItem> =
            unsafe { bar.statusItemWithLength(NSVariableStatusItemLength) };

        // Build a dummy target without a real channel (install replaces it
        // when set_command_channel is called).
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let target = MenuTarget::new(tx);

        let (menu, mute_item, status_item) = build_menu(&target, mtm);
        unsafe { item.setMenu(Some(&menu)) };

        // Render an initial placeholder icon so the status item is visible
        // before the first battery reading arrives. Without this the button
        // has no image and shows nothing.
        let img = render_icon(0, ChargingState::Unknown);
        unsafe {
            match item.button(mtm) {
                Some(button) => {
                    button.setImage(Some(&img));
                    tracing::info!("menubar: status item button installed with placeholder icon");
                }
                None => {
                    tracing::warn!("menubar: NSStatusItem.button(mtm) returned None — icon will not be visible");
                }
            }
        }

        Self {
            item,
            _target: target,
            mute_item,
            status_item,
        }
    }

    /// Wire the menu items to send commands on `sender`.
    pub fn set_command_channel(&self, sender: UnboundedSender<MenubarCommand>) {
        self._target.ivars().sender.set(Some(sender));
    }

    pub fn render(&self, percent: u8, charging: ChargingState, mtm: MainThreadMarker) {
        tracing::debug!(percent, ?charging, "menubar: rendering");
        let img = render_icon(percent, charging);
        unsafe {
            if let Some(button) = self.item.button(mtm) {
                button.setImage(Some(&img));
            } else {
                tracing::warn!("menubar: button(mtm) None during render");
            }
            self.status_item
                .setTitle(&NSString::from_str(&format_status(percent, charging)));
        }
    }

    /// Update the "Mute today" item label based on muted state.
    pub fn set_muted(&self, muted: bool) {
        let title = if muted { "Unmute" } else { "Mute today" };
        unsafe { self.mute_item.setTitle(&NSString::from_str(title)) };
    }

    pub fn remove(self) {
        let bar = unsafe { NSStatusBar::systemStatusBar() };
        unsafe { bar.removeStatusItem(&self.item) };
    }
}

// ---------------------------------------------------------------------------
// Menu construction
// ---------------------------------------------------------------------------

fn build_menu(
    target: &MenuTarget,
    mtm: MainThreadMarker,
) -> (Retained<NSMenu>, Retained<NSMenuItem>, Retained<NSMenuItem>) {
    let menu = unsafe { NSMenu::initWithTitle(mtm.alloc::<NSMenu>(), &NSString::from_str("")) };

    // Disabled informational row at the top showing the current reading.
    let status_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc::<NSMenuItem>(),
            &NSString::from_str("Battery: —"),
            None,
            &NSString::from_str(""),
        )
    };
    unsafe { status_item.setEnabled(false) };

    let separator = NSMenuItem::separatorItem(mtm);

    let mute_item = make_item("Mute today", sel!(handleMuteToday:), target, mtm);
    let prefs_item = make_item("Preferences\u{2026}", sel!(handlePrefs:), target, mtm);
    let quit_item = make_item("Quit", sel!(handleQuit:), target, mtm);

    menu.addItem(&status_item);
    menu.addItem(&separator);
    menu.addItem(&mute_item);
    menu.addItem(&prefs_item);
    menu.addItem(&quit_item);

    (menu, mute_item, status_item)
}

fn format_status(percent: u8, charging: ChargingState) -> String {
    let suffix = match charging {
        ChargingState::Recharging | ChargingState::ChargeInFinalState => " — charging",
        ChargingState::ChargeComplete => " — fully charged",
        ChargingState::Discharging => "",
        ChargingState::Unknown => " — connecting…",
    };
    format!("Battery: {}%{}", percent, suffix)
}

fn make_item(
    title: &str,
    action: objc2::runtime::Sel,
    target: &MenuTarget,
    mtm: MainThreadMarker,
) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc::<NSMenuItem>(),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(""),
        )
    };
    unsafe {
        item.setTarget(Some(target as &_ as &objc2::runtime::AnyObject));
    }
    item
}

// ---------------------------------------------------------------------------
// Icon rendering
// ---------------------------------------------------------------------------

fn render_icon(percent: u8, charging: ChargingState) -> Retained<NSImage> {
    let size = NSSize {
        width: ICON_W,
        height: ICON_H,
    };
    let block = RcBlock::new(move |_rect: NSRect| -> Bool {
        draw_battery(percent, charging);
        Bool::YES
    });
    let img: Retained<NSImage> =
        unsafe { NSImage::imageWithSize_flipped_drawingHandler(size, false, &block) };
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
    let body =
        unsafe { NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(body_rect, 2.0, 2.0) };
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
