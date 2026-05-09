//! macOS menu bar icon for MXBattery.
//!
//! Draws the "Silhouette fill" mouse glyph from `handoff/MXBatteryIcon.swift`
//! as a multi-color NSImage, and re-renders on light/dark appearance changes
//! via KVO on the status button's `effectiveAppearance`. Disconnected state
//! is not yet handled — see plan in /Users/pv/.claude/plans.
//!
//! Also installs a click menu with Mute today / Preferences / Quit items.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr;
use std::rc::Rc;

use block2::RcBlock;
use objc2::mutability::InteriorMutable;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{declare_class, msg_send, msg_send_id, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSAppearanceCustomization, NSBezierPath, NSColor, NSCompositingOperation, NSGraphicsContext,
    NSImage, NSLineCapStyle, NSMenu, NSMenuItem, NSStatusBar, NSStatusBarButton, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{
    CGFloat, MainThreadMarker, NSDictionary, NSKeyValueObservingOptions, NSObject, NSPoint, NSRect,
    NSSize, NSString,
};
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
// Appearance KVO observer
// ---------------------------------------------------------------------------
//
// Mirrors `MXBatteryStatusController.swift`: the status button's
// `effectiveAppearance` flips when the user toggles light/dark/HC, and we
// need to re-render so `labelColor` resolves to the new tint. Re-rendering
// is done inside `performAsCurrentDrawingAppearance:` so the dynamic colors
// pick the right value at draw time.

struct AppearanceObserverIvars {
    /// Last-rendered (percent, charging). Updated by `MenubarIcon::render` and
    /// read by the KVO callback to redraw on appearance flips alone.
    state: Rc<Cell<(u8, ChargingState)>>,
    /// Strong ref to the status button so the observer can drive a redraw.
    button: Retained<NSStatusBarButton>,
}

declare_class!(
    struct AppearanceObserver;

    unsafe impl ClassType for AppearanceObserver {
        type Super = NSObject;
        type Mutability = InteriorMutable;
        const NAME: &'static str = "MXBatteryAppearanceObserver";
    }

    impl DeclaredClass for AppearanceObserver {
        type Ivars = AppearanceObserverIvars;
    }

    unsafe impl AppearanceObserver {
        #[method(observeValueForKeyPath:ofObject:change:context:)]
        fn observe_value(
            &self,
            _key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSString, AnyObject>>,
            _context: *mut c_void,
        ) {
            let ivars = self.ivars();
            let (p, c) = ivars.state.get();
            redraw_button(&ivars.button, p, c);
        }
    }
);

impl AppearanceObserver {
    fn new(
        state: Rc<Cell<(u8, ChargingState)>>,
        button: Retained<NSStatusBarButton>,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(AppearanceObserverIvars { state, button });
        unsafe { msg_send_id![super(this), init] }
    }
}

// ---------------------------------------------------------------------------
// Public MenubarIcon struct
// ---------------------------------------------------------------------------

const ICON_W: CGFloat = 16.0;
const ICON_H: CGFloat = 22.0;

pub struct MenubarIcon {
    item: Retained<NSStatusItem>,
    /// Held alive while the menu exists.
    _target: Retained<MenuTarget>,
    /// Held alive so KVO callbacks keep firing; also used to remove the
    /// observer in `remove()`.
    appearance_observer: Retained<AppearanceObserver>,
    /// Strong ref to the status button so we can detach the observer.
    button: Retained<NSStatusBarButton>,
    /// Strong ref to the "Mute today" menu item for dynamic label updates.
    mute_item: Retained<NSMenuItem>,
    /// Strong ref to the disabled status row showing current battery percent + state.
    status_item: Retained<NSMenuItem>,
    /// Shared with the appearance observer so both paths redraw the same state.
    state: Rc<Cell<(u8, ChargingState)>>,
}

impl MenubarIcon {
    pub fn install(mtm: MainThreadMarker) -> Self {
        let bar = unsafe { NSStatusBar::systemStatusBar() };
        let item: Retained<NSStatusItem> =
            unsafe { bar.statusItemWithLength(NSVariableStatusItemLength) };

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let target = MenuTarget::new(tx);

        let (menu, mute_item, status_item) = build_menu(&target, mtm);
        unsafe { item.setMenu(Some(&menu)) };

        let state = Rc::new(Cell::new((0u8, ChargingState::Unknown)));

        // Install a placeholder icon and wire the appearance observer. Without
        // a button (e.g. status bar full) we skip both — render() will also
        // be a no-op in that case.
        let button = unsafe { item.button(mtm) };
        let (button, appearance_observer) = match button {
            Some(button) => {
                let (p, c) = state.get();
                redraw_button(&button, p, c);
                tracing::info!("menubar: status item button installed with placeholder icon");

                let observer = AppearanceObserver::new(Rc::clone(&state), button.clone());
                let key_path = NSString::from_str("effectiveAppearance");
                unsafe {
                    let observer_obj: &NSObject = observer.as_ref().as_ref();
                    let _: () = msg_send![
                        &button,
                        addObserver: observer_obj,
                        forKeyPath: &*key_path,
                        options: NSKeyValueObservingOptions::NSKeyValueObservingOptionNew,
                        context: ptr::null_mut::<c_void>(),
                    ];
                }
                (button, observer)
            }
            None => {
                tracing::warn!(
                    "menubar: NSStatusItem.button(mtm) returned None — icon will not be visible"
                );
                // Construct a dummy observer + button-less state so the struct
                // shape is consistent. We never add the observer in this branch.
                // SAFETY: We bail out before any AppKit interaction with this
                // unreachable branch by never invoking render(); remove() also
                // tolerates a missing button.
                let dummy_button: Retained<NSStatusBarButton> =
                    unsafe { msg_send_id![NSStatusBarButton::class(), new] };
                let observer = AppearanceObserver::new(Rc::clone(&state), dummy_button.clone());
                (dummy_button, observer)
            }
        };

        // Start hidden; main thread reveals via set_visible(true) once the
        // first reading arrives, and re-hides 5s after the last connected
        // peripheral drops.
        unsafe { item.setVisible(false) };

        Self {
            item,
            _target: target,
            appearance_observer,
            button,
            mute_item,
            status_item,
            state,
        }
    }

    /// Wire the menu items to send commands on `sender`.
    pub fn set_command_channel(&self, sender: UnboundedSender<MenubarCommand>) {
        self._target.ivars().sender.set(Some(sender));
    }

    pub fn render(&self, percent: u8, charging: ChargingState, mtm: MainThreadMarker) {
        tracing::debug!(percent, ?charging, "menubar: rendering");
        self.state.set((percent, charging));
        unsafe {
            if let Some(button) = self.item.button(mtm) {
                redraw_button(&button, percent, charging);
            } else {
                tracing::warn!("menubar: button(mtm) None during render");
            }
            self.status_item
                .setTitle(&NSString::from_str(&format_status(percent, charging)));
        }
    }

    /// Show or hide the menubar icon. Used to hide on mouse disconnect.
    pub fn set_visible(&self, visible: bool) {
        tracing::info!(visible, "menubar: set_visible");
        unsafe { self.item.setVisible(visible) };
    }

    /// Update the "Mute today" item label based on muted state.
    pub fn set_muted(&self, muted: bool) {
        let title = if muted { "Unmute" } else { "Mute today" };
        unsafe { self.mute_item.setTitle(&NSString::from_str(title)) };
    }

    pub fn remove(self) {
        // Detach KVO before the button goes away.
        let key_path = NSString::from_str("effectiveAppearance");
        unsafe {
            let observer_obj: &NSObject = self.appearance_observer.as_ref().as_ref();
            let _: () = msg_send![
                &self.button,
                removeObserver: observer_obj,
                forKeyPath: &*key_path,
            ];
        }
        let bar = unsafe { NSStatusBar::systemStatusBar() };
        unsafe { bar.removeStatusItem(&self.item) };
    }
}

impl Drop for MenubarIcon {
    fn drop(&mut self) {
        // If `remove()` wasn't called explicitly, still detach the observer to
        // avoid a dangling reference when the button is torn down.
        let key_path = NSString::from_str("effectiveAppearance");
        unsafe {
            let observer_obj: &NSObject = self.appearance_observer.as_ref().as_ref();
            let _: () = msg_send![
                &self.button,
                removeObserver: observer_obj,
                forKeyPath: &*key_path,
            ];
        }
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
// Bucket
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bucket {
    Critical, // < 5
    Warning,  // < 20
    Medium,   // < 60
    Good,     // < 80
    Full,     // 100
}

impl Bucket {
    fn from_percent(p: u8) -> Self {
        match p {
            0..=4 => Bucket::Critical,
            5..=19 => Bucket::Warning,
            20..=59 => Bucket::Medium,
            60..=79 => Bucket::Good,
            _ => Bucket::Full,
        }
    }

    /// Visualization fill height (0..100). Decoupled from raw percent so
    /// `<5` is still legibly visible.
    fn render_percent(self) -> CGFloat {
        match self {
            Bucket::Critical => 15.0,
            Bucket::Warning => 32.0,
            Bucket::Medium => 50.0,
            Bucket::Good => 70.0,
            Bucket::Full => 100.0,
        }
    }

    fn is_low(self) -> bool {
        matches!(self, Bucket::Critical | Bucket::Warning)
    }
}

fn is_charging(c: ChargingState) -> bool {
    !matches!(c, ChargingState::Discharging | ChargingState::Unknown)
}

// ---------------------------------------------------------------------------
// Palette (sRGB approximations of the oklch values from the design canvas)
// ---------------------------------------------------------------------------

fn color_red() -> Retained<NSColor> {
    unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.831, 0.196, 0.157, 1.0) }
}
fn color_yellow() -> Retained<NSColor> {
    unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.984, 0.784, 0.290, 1.0) }
}
#[allow(clippy::approx_constant)] // 0.318 is the design-canvas red channel, not 1/π
fn color_green() -> Retained<NSColor> {
    unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.318, 0.706, 0.435, 1.0) }
}
fn color_bright() -> Retained<NSColor> {
    unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.965, 0.965, 0.965, 1.0) }
}

// ---------------------------------------------------------------------------
// Icon rendering
// ---------------------------------------------------------------------------

/// Render an NSImage and stamp it on the button, wrapped in
/// `performAsCurrentDrawingAppearance:` so `labelColor` resolves under the
/// button's current appearance.
fn redraw_button(button: &NSStatusBarButton, percent: u8, charging: ChargingState) {
    let appearance = unsafe { button.effectiveAppearance() };
    let button_for_block = button.retain();
    let block = RcBlock::new(move || {
        let img = render_icon(percent, charging);
        unsafe { button_for_block.setImage(Some(&img)) };
    });
    unsafe { appearance.performAsCurrentDrawingAppearance(&block) };
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
    let img: Retained<NSImage> =
        unsafe { NSImage::imageWithSize_flipped_drawingHandler(size, true, &block) };
    unsafe { img.setTemplate(false) };
    img
}

/// Draw the silhouette + fill + wheel + (optional) bolt. Coordinates are SVG
/// y-down (0,0 top-left, 16×22 canvas). The flipped NSImage drawing context
/// matches that orientation, so values map 1:1 from `MXBatteryIcon.swift`.
fn draw_battery(percent: u8, charging: ChargingState) {
    let bucket = Bucket::from_percent(percent);
    let charging = is_charging(charging);

    let fill_h = 18.0 * bucket.render_percent() / 100.0;
    let fy = 20.0 - fill_h;

    let fill_color = if charging {
        color_green()
    } else {
        match bucket {
            Bucket::Critical => color_red(),
            Bucket::Warning => color_yellow(),
            _ => unsafe { NSColor::labelColor() },
        }
    };
    let label = unsafe { NSColor::labelColor() };

    let Some(ctx) = (unsafe { NSGraphicsContext::currentContext() }) else {
        return;
    };

    // 1. Silhouette-clipped fill, with wheel + (optional) bolt knocked out.
    unsafe {
        ctx.saveGraphicsState();
        let silhouette = silhouette_path();
        silhouette.addClip();

        fill_color.setFill();
        let fill_rect = NSRect::new(
            NSPoint { x: 2.0, y: fy },
            NSSize {
                width: 12.0,
                height: fill_h,
            },
        );
        let fill_path = NSBezierPath::bezierPathWithRect(fill_rect);
        fill_path.fill();

        ctx.setCompositingOperation(NSCompositingOperation::Clear);

        // Wheel knockout.
        let wheel = NSBezierPath::bezierPath();
        wheel.moveToPoint(NSPoint { x: 8.0, y: 4.4 });
        wheel.lineToPoint(NSPoint { x: 8.0, y: 7.4 });
        wheel.setLineWidth(1.4);
        wheel.setLineCapStyle(NSLineCapStyle::Round);
        wheel.stroke();

        if charging {
            let bolt = bolt_path(NSPoint { x: 5.5, y: 8.8 });
            bolt.fill();
        }

        ctx.restoreGraphicsState();
    }

    // 2. Silhouette outline.
    unsafe {
        ctx.saveGraphicsState();
        let silhouette = silhouette_path();
        label.setStroke();
        silhouette.setLineWidth(1.3);
        silhouette.stroke();
        ctx.restoreGraphicsState();
    }

    // 3. Wheel as a positive line, only in the empty area above the fill.
    if fy > 0.0 {
        unsafe {
            ctx.saveGraphicsState();
            NSBezierPath::clipRect(NSRect::new(
                NSPoint { x: 0.0, y: 0.0 },
                NSSize {
                    width: ICON_W,
                    height: fy,
                },
            ));
            label.setStroke();
            let wheel = NSBezierPath::bezierPath();
            wheel.moveToPoint(NSPoint { x: 8.0, y: 4.4 });
            wheel.lineToPoint(NSPoint { x: 8.0, y: 7.4 });
            wheel.setLineWidth(1.3);
            wheel.setLineCapStyle(NSLineCapStyle::Round);
            wheel.stroke();
            ctx.restoreGraphicsState();
        }
    }

    // 4. Charging bolt.
    if !charging {
        return;
    }
    if bucket.is_low() {
        if fy > 0.0 {
            unsafe {
                ctx.saveGraphicsState();
                NSBezierPath::clipRect(NSRect::new(
                    NSPoint { x: 0.0, y: 0.0 },
                    NSSize {
                        width: ICON_W,
                        height: fy,
                    },
                ));
                label.setFill();
                let bolt = bolt_path(NSPoint { x: 5.5, y: 8.8 });
                bolt.fill();
                ctx.restoreGraphicsState();
            }
        }
    } else {
        unsafe {
            color_bright().setFill();
            let bolt = bolt_path(NSPoint { x: 5.5, y: 8.8 });
            bolt.fill();
        }
    }
}

fn silhouette_path() -> Retained<NSBezierPath> {
    unsafe {
        let p = NSBezierPath::bezierPath();
        p.moveToPoint(NSPoint { x: 8.0, y: 2.4 });
        p.curveToPoint_controlPoint1_controlPoint2(
            NSPoint { x: 13.6, y: 11.0 },
            NSPoint { x: 12.2, y: 2.4 },
            NSPoint { x: 13.6, y: 5.6 },
        );
        p.curveToPoint_controlPoint1_controlPoint2(
            NSPoint { x: 8.0, y: 19.6 },
            NSPoint { x: 13.6, y: 16.6 },
            NSPoint { x: 12.0, y: 19.6 },
        );
        p.curveToPoint_controlPoint1_controlPoint2(
            NSPoint { x: 2.4, y: 11.0 },
            NSPoint { x: 4.0, y: 19.6 },
            NSPoint { x: 2.4, y: 16.6 },
        );
        p.curveToPoint_controlPoint1_controlPoint2(
            NSPoint { x: 8.0, y: 2.4 },
            NSPoint { x: 2.4, y: 5.6 },
            NSPoint { x: 3.8, y: 2.4 },
        );
        p.closePath();
        p
    }
}

fn bolt_path(origin: NSPoint) -> Retained<NSBezierPath> {
    unsafe {
        let p = NSBezierPath::bezierPath();
        let pts = [
            (3.4, 0.0),
            (0.0, 5.2),
            (2.6, 5.2),
            (1.2, 9.4),
            (5.0, 4.0),
            (2.4, 4.0),
            (3.6, 0.0),
        ];
        p.moveToPoint(NSPoint {
            x: origin.x + pts[0].0,
            y: origin.y + pts[0].1,
        });
        for (dx, dy) in &pts[1..] {
            p.lineToPoint(NSPoint {
                x: origin.x + dx,
                y: origin.y + dy,
            });
        }
        p.closePath();
        p
    }
}
