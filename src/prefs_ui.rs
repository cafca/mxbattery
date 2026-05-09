//! Native AppKit preferences window for MXBattery.
//!
//! Provides a form-based UI for editing the application configuration.
//! The window is created once and re-shown on subsequent calls.

use std::cell::Cell;
use std::sync::Mutex;
use std::time::Duration;

use objc2::mutability::MainThreadOnly;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{declare_class, msg_send_id, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSButton,
    NSControlStateValueOff, NSControlStateValueOn, NSPopUpButton, NSStackView,
    NSStackViewDistribution, NSStackViewGravity, NSStepper, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDate, NSDefaultRunLoopMode, NSEdgeInsets, NSNotification,
    NSObject, NSObjectProtocol, NSPoint, NSRect, NSRunLoop, NSSize, NSString,
};

use crate::config::{Autostart, Cadence, Config, DeviceFilter, MenubarConfig, Thresholds};
use crate::paths::Paths;

// ---------------------------------------------------------------------------
// Form state held in Ivars
// ---------------------------------------------------------------------------

struct PrefsTargetIvars {
    // Device filter
    device_popup: Cell<*mut NSPopUpButton>,
    identifier_field: Cell<*mut NSTextField>,

    // Threshold checkboxes
    warn_enabled_check: Cell<*mut NSButton>,
    critical_enabled_check: Cell<*mut NSButton>,

    // Threshold steppers and display fields
    warn_stepper: Cell<*mut NSStepper>,
    warn_field: Cell<*mut NSTextField>,
    critical_stepper: Cell<*mut NSStepper>,
    critical_field: Cell<*mut NSTextField>,

    // Cadence (minutes)
    critical_period_stepper: Cell<*mut NSStepper>,
    critical_period_field: Cell<*mut NSTextField>,

    // Other flags
    menubar_check: Cell<*mut NSButton>,
    autostart_check: Cell<*mut NSButton>,

    // Save button (so we can toggle enabled)
    save_button: Cell<*mut NSButton>,

    // The window itself (not retained — the WindowCache holds the Retained)
    window: Cell<*mut NSWindow>,

    // Original warn_period from the config loaded at form-open time.
    // Preserved so Save doesn't clobber it with a hardcoded 24h value.
    original_warn_period: Mutex<Duration>,

    // Original rearm_hysteresis from the config. Not exposed in the UI;
    // power users edit it via config.toml directly.
    original_rearm_hysteresis: Cell<u8>,

    // Original autostart.enabled so Save can detect changes and call launchd.
    original_autostart_enabled: Cell<bool>,
}

// SAFETY: All raw pointers are only ever dereferenced on the main thread.
// The Cell wrappers ensure no aliased mutable access.
unsafe impl Send for PrefsTargetIvars {}
unsafe impl Sync for PrefsTargetIvars {}

// ---------------------------------------------------------------------------
// ObjC class declaration — MainThreadOnly so NSWindowDelegate is satisfied
// ---------------------------------------------------------------------------

declare_class!(
    struct PrefsTarget;

    unsafe impl ClassType for PrefsTarget {
        type Super = NSObject;
        type Mutability = MainThreadOnly;
        const NAME: &'static str = "MXBatteryPrefsTarget";
    }

    impl DeclaredClass for PrefsTarget {
        type Ivars = PrefsTargetIvars;
    }

    unsafe impl PrefsTarget {
        #[method(windowWillClose:)]
        fn window_will_close(&self, _notification: &NSNotification) {
            // Nothing to do — the cache keeps the window alive.
            // run_oneshot detects closure by checking isVisible.
        }

        #[method(onSavePressed:)]
        fn on_save_pressed(&self, _sender: *mut NSObject) {
            self.save_config();
            self.close_window();
        }

        #[method(onCancelPressed:)]
        fn on_cancel_pressed(&self, _sender: *mut NSObject) {
            self.close_window();
        }

        #[method(onDeviceChanged:)]
        fn on_device_changed(&self, _sender: *mut NSObject) {
            unsafe {
                let popup = &*self.ivars().device_popup.get();
                let id_field = &*self.ivars().identifier_field.get();
                // 0=AnyMX, 1=AnyLogitech, 2=Specific
                id_field.setHidden(popup.indexOfSelectedItem() != 2);
            }
        }

        #[method(onWarnEnabledChanged:)]
        fn on_warn_enabled_changed(&self, _sender: *mut NSObject) {
            self.update_enabled_states();
            self.update_save_state();
        }

        #[method(onCriticalEnabledChanged:)]
        fn on_critical_enabled_changed(&self, _sender: *mut NSObject) {
            self.update_enabled_states();
            self.update_save_state();
        }

        #[method(onThresholdChanged:)]
        fn on_threshold_changed(&self, sender: *mut NSObject) {
            self.sync_steppers_and_fields(sender);
            self.update_stepper_bounds();
            self.update_save_state();
        }
    }
);

impl PrefsTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let ivars = PrefsTargetIvars {
            device_popup: Cell::new(std::ptr::null_mut()),
            identifier_field: Cell::new(std::ptr::null_mut()),
            warn_enabled_check: Cell::new(std::ptr::null_mut()),
            critical_enabled_check: Cell::new(std::ptr::null_mut()),
            warn_stepper: Cell::new(std::ptr::null_mut()),
            warn_field: Cell::new(std::ptr::null_mut()),
            critical_stepper: Cell::new(std::ptr::null_mut()),
            critical_field: Cell::new(std::ptr::null_mut()),
            critical_period_stepper: Cell::new(std::ptr::null_mut()),
            critical_period_field: Cell::new(std::ptr::null_mut()),
            menubar_check: Cell::new(std::ptr::null_mut()),
            autostart_check: Cell::new(std::ptr::null_mut()),
            save_button: Cell::new(std::ptr::null_mut()),
            window: Cell::new(std::ptr::null_mut()),
            original_warn_period: Mutex::new(Duration::from_secs(24 * 60 * 60)),
            original_rearm_hysteresis: Cell::new(5),
            original_autostart_enabled: Cell::new(false),
        };
        let this = mtm.alloc::<Self>().set_ivars(ivars);
        unsafe { msg_send_id![super(this), init] }
    }

    fn close_window(&self) {
        let ptr = self.ivars().window.get();
        if !ptr.is_null() {
            unsafe { (*ptr).close() };
        }
    }

    fn save_config(&self) {
        let config = self.build_config();
        let prev_autostart = self.ivars().original_autostart_enabled.get();
        let new_autostart = config.autostart.enabled;
        match Paths::standard() {
            Ok(paths) => {
                if let Err(e) = config.save(&paths.config_file()) {
                    tracing::error!(?e, "failed to save config");
                    return;
                }
                // Call launchd if the autostart setting changed.
                //
                // We never run launchctl from inside the daemon process directly:
                // doing so creates a race where launchd schedules a new copy of the
                // daemon while the current one is still alive holding the IPC socket.
                // Instead we spawn a detached `mxbattery install` / `mxbattery uninstall`
                // child process and, when installing, terminate ourselves so launchd's
                // freshly bootstrapped copy can take over cleanly.
                if new_autostart != prev_autostart {
                    let exe = std::env::current_exe().ok();
                    if new_autostart {
                        let app_path = exe.as_ref().and_then(|e| {
                            e.parent()
                                .and_then(|p| p.parent())
                                .and_then(|p| p.parent())
                                .map(|p| p.to_path_buf())
                        });
                        match (exe.as_ref(), app_path) {
                            (Some(exe), Some(p))
                                if p.extension().and_then(|e| e.to_str()) == Some("app") =>
                            {
                                let app_str = p.to_string_lossy().into_owned();
                                match std::process::Command::new(exe)
                                    .args(["install", &app_str])
                                    .spawn()
                                {
                                    Ok(_) => {
                                        tracing::info!(
                                            "spawned launchd install helper; \
                                             terminating so launchd can take over"
                                        );
                                        // Update saved baseline so any re-entry doesn't
                                        // re-spawn — though we're about to exit anyway.
                                        self.ivars().original_autostart_enabled.set(true);
                                        if let Some(mtm) = MainThreadMarker::new() {
                                            let app = NSApplication::sharedApplication(mtm);
                                            unsafe { app.terminate(None) };
                                        }
                                        return;
                                    }
                                    Err(e) => {
                                        tracing::warn!(?e, "failed to spawn install helper")
                                    }
                                }
                            }
                            _ => tracing::warn!("running unbundled — skipping launchd install"),
                        }
                    } else if let Some(exe) = exe.as_ref() {
                        // Uninstall: detached child does the launchctl dance. If we are
                        // currently the LaunchAgent, bootout will SIGTERM us; if we are
                        // running standalone (e.g. via `open`), we keep running with the
                        // plist removed.
                        if let Err(e) = std::process::Command::new(exe).args(["uninstall"]).spawn()
                        {
                            tracing::warn!(?e, "failed to spawn uninstall helper");
                        }
                    }
                    self.ivars().original_autostart_enabled.set(new_autostart);
                }
            }
            Err(e) => tracing::error!(?e, "failed to resolve paths"),
        }
    }

    fn build_config(&self) -> Config {
        unsafe {
            let popup = &*self.ivars().device_popup.get();
            let device = match popup.indexOfSelectedItem() {
                2 => {
                    let id_field = &*self.ivars().identifier_field.get();
                    DeviceFilter::Specific {
                        identifier: id_field.stringValue().to_string(),
                    }
                }
                1 => DeviceFilter::AnyLogitech,
                _ => DeviceFilter::AnyMx,
            };

            let warn_enabled =
                (*self.ivars().warn_enabled_check.get()).state() == NSControlStateValueOn;
            let critical_enabled =
                (*self.ivars().critical_enabled_check.get()).state() == NSControlStateValueOn;

            let warn = (*self.ivars().warn_stepper.get()).doubleValue() as u8;
            let critical = (*self.ivars().critical_stepper.get()).doubleValue() as u8;
            let rearm_hysteresis = self.ivars().original_rearm_hysteresis.get();
            let period_mins = (*self.ivars().critical_period_stepper.get()).doubleValue() as u64;

            let menubar_enabled =
                (*self.ivars().menubar_check.get()).state() == NSControlStateValueOn;
            let autostart_enabled =
                (*self.ivars().autostart_check.get()).state() == NSControlStateValueOn;

            let warn_period = *self.ivars().original_warn_period.lock().unwrap();
            Config {
                schema_version: 1,
                device,
                thresholds: Thresholds {
                    warn,
                    critical,
                    rearm_hysteresis,
                    warn_enabled,
                    critical_enabled,
                },
                cadence: Cadence {
                    warn_period,
                    critical_period: Duration::from_secs(period_mins * 60),
                },
                menubar: MenubarConfig {
                    enabled: menubar_enabled,
                },
                autostart: Autostart {
                    enabled: autostart_enabled,
                },
            }
        }
    }

    /// Sync the paired (stepper, field) when one of them fires.
    fn sync_steppers_and_fields(&self, sender: *mut NSObject) {
        // Build list of (stepper_ptr, field_ptr) pairs
        let pairs = [
            (
                self.ivars().warn_stepper.get(),
                self.ivars().warn_field.get(),
            ),
            (
                self.ivars().critical_stepper.get(),
                self.ivars().critical_field.get(),
            ),
            (
                self.ivars().critical_period_stepper.get(),
                self.ivars().critical_period_field.get(),
            ),
        ];

        let sender_any = sender as *mut AnyObject;
        for (stepper_ptr, field_ptr) in pairs {
            if stepper_ptr.is_null() || field_ptr.is_null() {
                continue;
            }
            unsafe {
                let stepper_any = stepper_ptr as *mut AnyObject;
                let field_any = field_ptr as *mut AnyObject;
                if stepper_any == sender_any {
                    // Stepper changed → update text field
                    let val = (*stepper_ptr).doubleValue() as i64;
                    (*field_ptr).setStringValue(&NSString::from_str(&val.to_string()));
                } else if field_any == sender_any {
                    // Text field changed → update stepper (with bounds clamping)
                    let text = (*field_ptr).stringValue().to_string();
                    if let Ok(v) = text.parse::<f64>() {
                        let min = (*stepper_ptr).minValue();
                        let max = (*stepper_ptr).maxValue();
                        let clamped = v.clamp(min, max);
                        (*stepper_ptr).setDoubleValue(clamped);
                        if (v - clamped).abs() > 0.5 {
                            (*field_ptr)
                                .setStringValue(&NSString::from_str(&(clamped as i64).to_string()));
                        }
                    }
                }
            }
        }
    }

    /// Recompute stepper min/max so that critical < warn always holds.
    fn update_stepper_bounds(&self) {
        unsafe {
            let warn_s = &*self.ivars().warn_stepper.get();
            let crit_s = &*self.ivars().critical_stepper.get();
            let warn_val = warn_s.doubleValue();
            let crit_val = crit_s.doubleValue();

            crit_s.setMinValue(1.0);
            crit_s.setMaxValue((warn_val - 1.0).max(1.0));

            warn_s.setMinValue((crit_val + 1.0).min(99.0));
            warn_s.setMaxValue(99.0);
        }
    }

    /// Dim/enable warn and critical controls based on their checkbox state.
    fn update_enabled_states(&self) {
        unsafe {
            let warn_on = (*self.ivars().warn_enabled_check.get()).state() == NSControlStateValueOn;
            (*self.ivars().warn_stepper.get()).setEnabled(warn_on);
            (*self.ivars().warn_field.get()).setEnabled(warn_on);

            let crit_on =
                (*self.ivars().critical_enabled_check.get()).state() == NSControlStateValueOn;
            (*self.ivars().critical_stepper.get()).setEnabled(crit_on);
            (*self.ivars().critical_field.get()).setEnabled(crit_on);
        }
    }

    /// Disable Save if critical >= warn (invalid config).
    fn update_save_state(&self) {
        unsafe {
            let warn = (*self.ivars().warn_stepper.get()).doubleValue();
            let crit = (*self.ivars().critical_stepper.get()).doubleValue();
            (*self.ivars().save_button.get()).setEnabled(crit < warn);
        }
    }

    /// Populate all form controls from a `Config`.
    fn load_config(&self, cfg: &Config) {
        // Capture original values for use at save time.
        *self.ivars().original_warn_period.lock().unwrap() = cfg.cadence.warn_period;
        self.ivars()
            .original_autostart_enabled
            .set(cfg.autostart.enabled);

        unsafe {
            // Device
            let popup = &*self.ivars().device_popup.get();
            let id_field = &*self.ivars().identifier_field.get();
            match &cfg.device {
                DeviceFilter::AnyMx => {
                    popup.selectItemAtIndex(0);
                    id_field.setHidden(true);
                }
                DeviceFilter::AnyLogitech => {
                    popup.selectItemAtIndex(1);
                    id_field.setHidden(true);
                }
                DeviceFilter::Specific { identifier } => {
                    popup.selectItemAtIndex(2);
                    id_field.setHidden(false);
                    id_field.setStringValue(&NSString::from_str(identifier));
                }
            }

            let t = &cfg.thresholds;
            macro_rules! set_check {
                ($cell:expr, $val:expr) => {
                    (*$cell.get()).setState(if $val {
                        NSControlStateValueOn
                    } else {
                        NSControlStateValueOff
                    });
                };
            }
            set_check!(self.ivars().warn_enabled_check, t.warn_enabled);
            set_check!(self.ivars().critical_enabled_check, t.critical_enabled);

            macro_rules! set_stepper_field {
                ($sc:expr, $fc:expr, $val:expr) => {
                    (*$sc.get()).setDoubleValue($val);
                    (*$fc.get()).setStringValue(&NSString::from_str(&($val as i64).to_string()));
                };
            }
            set_stepper_field!(
                self.ivars().warn_stepper,
                self.ivars().warn_field,
                t.warn as f64
            );
            set_stepper_field!(
                self.ivars().critical_stepper,
                self.ivars().critical_field,
                t.critical as f64
            );
            self.ivars()
                .original_rearm_hysteresis
                .set(t.rearm_hysteresis);
            let period_mins = (cfg.cadence.critical_period.as_secs() / 60) as f64;
            set_stepper_field!(
                self.ivars().critical_period_stepper,
                self.ivars().critical_period_field,
                period_mins
            );

            set_check!(self.ivars().menubar_check, cfg.menubar.enabled);
            set_check!(self.ivars().autostart_check, cfg.autostart.enabled);
        }

        self.update_stepper_bounds();
        self.update_enabled_states();
        self.update_save_state();
    }
}

unsafe impl NSObjectProtocol for PrefsTarget {}
unsafe impl NSWindowDelegate for PrefsTarget {}

// ---------------------------------------------------------------------------
// UI helper builders
// ---------------------------------------------------------------------------

fn make_label(text: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    unsafe { NSTextField::labelWithString(&NSString::from_str(text), mtm) }
}

fn make_int_stepper(
    min: f64,
    max: f64,
    initial: f64,
    target: &PrefsTarget,
    action: objc2::runtime::Sel,
    mtm: MainThreadMarker,
) -> Retained<NSStepper> {
    let s = unsafe { NSStepper::new(mtm) };
    unsafe {
        s.setMinValue(min);
        s.setMaxValue(max);
        s.setIncrement(1.0);
        s.setValueWraps(false);
        s.setDoubleValue(initial);
        s.setTarget(Some(target as &_ as &AnyObject));
        s.setAction(Some(action));
    }
    s
}

fn make_int_field(
    initial: f64,
    target: &PrefsTarget,
    action: objc2::runtime::Sel,
    mtm: MainThreadMarker,
) -> Retained<NSTextField> {
    let frame = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: NSSize {
            width: 52.0,
            height: 22.0,
        },
    };
    let f = unsafe { NSTextField::initWithFrame(mtm.alloc::<NSTextField>(), frame) };
    unsafe {
        f.setStringValue(&NSString::from_str(&(initial as i64).to_string()));
        f.setEditable(true);
        f.setBordered(true);
        f.setBezeled(true);
        f.setTarget(Some(target as &_ as &AnyObject));
        f.setAction(Some(action));
    }
    f
}

fn make_checkbox(
    title: &str,
    on: bool,
    target: &PrefsTarget,
    action: objc2::runtime::Sel,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    let btn = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str(title),
            Some(target as &_ as &AnyObject),
            Some(action),
            mtm,
        )
    };
    unsafe {
        btn.setState(if on {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    }
    btn
}

/// Wrap an array of `&NSView`s in a horizontal `NSStackView`.
fn hstack(views: &[&NSView], spacing: f64, mtm: MainThreadMarker) -> Retained<NSStackView> {
    let retained: Vec<Retained<NSView>> = views
        .iter()
        .map(|v| unsafe { Retained::retain(*v as *const NSView as *mut NSView).unwrap() })
        .collect();
    let array = NSArray::from_id_slice(&retained);
    let stack = unsafe { NSStackView::stackViewWithViews(&array, mtm) };
    unsafe {
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        stack.setSpacing(spacing);
    }
    stack
}

/// Wrap an array of `&NSView`s in a vertical `NSStackView`.
fn vstack_with_insets(
    views: &[&NSView],
    spacing: f64,
    insets: NSEdgeInsets,
    mtm: MainThreadMarker,
) -> Retained<NSStackView> {
    let retained: Vec<Retained<NSView>> = views
        .iter()
        .map(|v| unsafe { Retained::retain(*v as *const NSView as *mut NSView).unwrap() })
        .collect();
    let array = NSArray::from_id_slice(&retained);
    let stack = unsafe { NSStackView::stackViewWithViews(&array, mtm) };
    unsafe {
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setSpacing(spacing);
        stack.setEdgeInsets(insets);
    }
    stack
}

// ---------------------------------------------------------------------------
// Window builder
// ---------------------------------------------------------------------------

fn build_prefs_window(
    cfg: &Config,
    mtm: MainThreadMarker,
) -> (Retained<NSWindow>, Retained<PrefsTarget>) {
    let target = PrefsTarget::new(mtm);

    // --- Device row ---
    let device_label = make_label("Device:", mtm);
    let popup = {
        let frame = NSRect {
            origin: NSPoint { x: 0.0, y: 0.0 },
            size: NSSize {
                width: 180.0,
                height: 26.0,
            },
        };
        unsafe {
            NSPopUpButton::initWithFrame_pullsDown(mtm.alloc::<NSPopUpButton>(), frame, false)
        }
    };
    unsafe {
        popup.addItemWithTitle(&NSString::from_str("Any MX"));
        popup.addItemWithTitle(&NSString::from_str("Any Logitech"));
        popup.addItemWithTitle(&NSString::from_str("Specific\u{2026}"));
        popup.setTarget(Some(&*target as &_ as &AnyObject));
        popup.setAction(Some(sel!(onDeviceChanged:)));
    }

    let id_frame = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: NSSize {
            width: 280.0,
            height: 22.0,
        },
    };
    let id_field = unsafe { NSTextField::initWithFrame(mtm.alloc::<NSTextField>(), id_frame) };
    unsafe {
        id_field.setPlaceholderString(Some(&NSString::from_str("Device identifier UUID")));
        id_field.setEditable(true);
        id_field.setBordered(true);
        id_field.setBezeled(true);
        id_field.setHidden(true);
    }

    let device_row = hstack(&[&*device_label, &*popup], 8.0, mtm);

    // --- Warn ---
    let warn_check = make_checkbox(
        "Warn enabled",
        cfg.thresholds.warn_enabled,
        &target,
        sel!(onWarnEnabledChanged:),
        mtm,
    );
    let warn_label = make_label("Warn threshold:", mtm);
    let warn_stepper = make_int_stepper(
        1.0,
        99.0,
        cfg.thresholds.warn as f64,
        &target,
        sel!(onThresholdChanged:),
        mtm,
    );
    let warn_field = make_int_field(
        cfg.thresholds.warn as f64,
        &target,
        sel!(onThresholdChanged:),
        mtm,
    );
    let warn_row = hstack(&[&*warn_label, &*warn_stepper, &*warn_field], 6.0, mtm);

    // --- Critical ---
    let crit_check = make_checkbox(
        "Critical enabled",
        cfg.thresholds.critical_enabled,
        &target,
        sel!(onCriticalEnabledChanged:),
        mtm,
    );
    let crit_label = make_label("Critical threshold:", mtm);
    let crit_stepper = make_int_stepper(
        1.0,
        99.0,
        cfg.thresholds.critical as f64,
        &target,
        sel!(onThresholdChanged:),
        mtm,
    );
    let crit_field = make_int_field(
        cfg.thresholds.critical as f64,
        &target,
        sel!(onThresholdChanged:),
        mtm,
    );
    let crit_row = hstack(&[&*crit_label, &*crit_stepper, &*crit_field], 6.0, mtm);

    // --- Critical re-notify period (minutes) ---
    let period_mins = (cfg.cadence.critical_period.as_secs() / 60) as f64;
    let period_label = make_label("Critical re-notify (min):", mtm);
    let period_stepper = make_int_stepper(
        1.0,
        240.0,
        period_mins,
        &target,
        sel!(onThresholdChanged:),
        mtm,
    );
    let period_field = make_int_field(period_mins, &target, sel!(onThresholdChanged:), mtm);
    let period_row = hstack(
        &[&*period_label, &*period_stepper, &*period_field],
        6.0,
        mtm,
    );

    // --- Other flags ---
    let menubar_check = make_checkbox(
        "Show menu bar icon",
        cfg.menubar.enabled,
        &target,
        sel!(onThresholdChanged:),
        mtm,
    );
    let autostart_check = make_checkbox(
        "Start at login",
        cfg.autostart.enabled,
        &target,
        sel!(onThresholdChanged:),
        mtm,
    );

    // --- Bottom button row (Cancel | Save, right-aligned) ---
    let cancel_btn = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str("Cancel"),
            Some(&*target as &_ as &AnyObject),
            Some(sel!(onCancelPressed:)),
            mtm,
        )
    };
    let save_btn = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str("Save"),
            Some(&*target as &_ as &AnyObject),
            Some(sel!(onSavePressed:)),
            mtm,
        )
    };
    // Spacer view
    let spacer = unsafe {
        NSView::initWithFrame(
            mtm.alloc::<NSView>(),
            NSRect {
                origin: NSPoint { x: 0.0, y: 0.0 },
                size: NSSize {
                    width: 1.0,
                    height: 1.0,
                },
            },
        )
    };
    let button_row = hstack(&[&*spacer, &*cancel_btn, &*save_btn], 8.0, mtm);
    unsafe {
        // Re-configure as gravity-areas so the spacer pushes buttons right
        button_row.setDistribution(NSStackViewDistribution::GravityAreas);
        button_row.addView_inGravity(&spacer, NSStackViewGravity::Leading);
        button_row.addView_inGravity(&cancel_btn, NSStackViewGravity::Trailing);
        button_row.addView_inGravity(&save_btn, NSStackViewGravity::Trailing);
    }

    // --- Stow refs in ivars ---
    target
        .ivars()
        .device_popup
        .set(Retained::as_ptr(&popup) as *mut _);
    target
        .ivars()
        .identifier_field
        .set(Retained::as_ptr(&id_field) as *mut _);
    target
        .ivars()
        .warn_enabled_check
        .set(Retained::as_ptr(&warn_check) as *mut _);
    target
        .ivars()
        .critical_enabled_check
        .set(Retained::as_ptr(&crit_check) as *mut _);
    target
        .ivars()
        .warn_stepper
        .set(Retained::as_ptr(&warn_stepper) as *mut _);
    target
        .ivars()
        .warn_field
        .set(Retained::as_ptr(&warn_field) as *mut _);
    target
        .ivars()
        .critical_stepper
        .set(Retained::as_ptr(&crit_stepper) as *mut _);
    target
        .ivars()
        .critical_field
        .set(Retained::as_ptr(&crit_field) as *mut _);
    target
        .ivars()
        .critical_period_stepper
        .set(Retained::as_ptr(&period_stepper) as *mut _);
    target
        .ivars()
        .critical_period_field
        .set(Retained::as_ptr(&period_field) as *mut _);
    target
        .ivars()
        .menubar_check
        .set(Retained::as_ptr(&menubar_check) as *mut _);
    target
        .ivars()
        .autostart_check
        .set(Retained::as_ptr(&autostart_check) as *mut _);
    target
        .ivars()
        .save_button
        .set(Retained::as_ptr(&save_btn) as *mut _);

    // Apply initial enabled/bound state (before loading config)
    target.update_stepper_bounds();
    target.update_enabled_states();
    target.update_save_state();

    // --- Outer vertical stack ---
    let insets = NSEdgeInsets {
        top: 16.0,
        left: 20.0,
        bottom: 16.0,
        right: 20.0,
    };
    let content_stack = vstack_with_insets(
        &[
            &*device_row,
            &*id_field,
            &*warn_check,
            &*warn_row,
            &*crit_check,
            &*crit_row,
            &*period_row,
            &*menubar_check,
            &*autostart_check,
            &*button_row,
        ],
        10.0,
        insets,
        mtm,
    );

    // --- NSWindow ---
    let style =
        NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;

    let content_rect = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: NSSize {
            width: 460.0,
            height: 480.0,
        },
    };
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc::<NSWindow>(),
            content_rect,
            style,
            NSBackingStoreType::NSBackingStoreBuffered,
            false,
        )
    };
    unsafe {
        window.setTitle(&NSString::from_str("MXBattery Preferences"));
        window.setReleasedWhenClosed(false);
        window.setContentView(Some(&*content_stack));
        window.center();
        window.setDelegate(Some(ProtocolObject::from_ref(&*target)));
    }
    target
        .ivars()
        .window
        .set(Retained::as_ptr(&window) as *mut _);

    // Re-load the passed config into the controls
    target.load_config(cfg);

    (window, target)
}

// ---------------------------------------------------------------------------
// Singleton cache via thread_local
//
// NSWindow/NSView etc. are MainThreadOnly and !Send/!Sync, so we keep them
// in a thread_local to avoid the Sync requirement on a static.
// ---------------------------------------------------------------------------

struct WindowCache {
    window: Retained<NSWindow>,
    _target: Retained<PrefsTarget>,
}

thread_local! {
    static CACHE: Cell<Option<WindowCache>> = const { Cell::new(None) };
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Handle to the preferences window.
pub struct PrefsWindow;

impl PrefsWindow {
    /// Show or focus the preferences window.  Loads `Config` from disk first.
    /// Must be called on the main thread.
    pub fn show_or_focus(mtm: MainThreadMarker) {
        let cfg = Paths::standard()
            .ok()
            .and_then(|p| Config::load(&p.config_file()).ok())
            .unwrap_or_default();

        // Activate the app first so the window comes to the foreground regardless
        // of which window currently has focus. Required for LSUIElement apps
        // because they don't auto-activate when their windows order front.
        let app = NSApplication::sharedApplication(mtm);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);

        CACHE.with(|cell| {
            // Take the option out
            let existing = cell.take();
            if let Some(cache) = existing {
                // Reload config and bring to front
                cache._target.load_config(&cfg);
                cache.window.makeKeyAndOrderFront(None);
                // Put it back
                cell.set(Some(cache));
            } else {
                // Build fresh
                let (window, target) = build_prefs_window(&cfg, mtm);
                window.makeKeyAndOrderFront(None);
                cell.set(Some(WindowCache {
                    window,
                    _target: target,
                }));
            }
        });
    }
}

/// One-shot mode for the `mxbattery prefs` CLI subcommand.
///
/// Builds an `NSApplication`, shows the prefs window, pumps the run loop
/// until the window is dismissed, then returns.
pub fn run_oneshot() -> anyhow::Result<()> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| anyhow::anyhow!("run_oneshot must be called on the main thread"))?;

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);

    PrefsWindow::show_or_focus(mtm);

    // Pump the run loop until the window is no longer visible.
    let run_loop = unsafe { NSRunLoop::mainRunLoop() };
    loop {
        let is_open = CACHE.with(|cell| {
            let opt = cell.take();
            let open = opt.as_ref().is_some_and(|c| c.window.isVisible());
            cell.set(opt);
            open
        });

        if !is_open {
            break;
        }

        // Run one iteration of the run loop with a short timeout
        let soon = unsafe { NSDate::dateWithTimeIntervalSinceNow(0.1) };
        unsafe {
            run_loop.runMode_beforeDate(NSDefaultRunLoopMode, &soon);
        }
    }

    Ok(())
}
