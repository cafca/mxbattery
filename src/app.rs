//! Main daemon run loop — NSApplication wiring and event glue.
//!
//! Called by `lib::run()` on macOS.  Must be invoked on the main thread.

use std::ptr::NonNull;
use std::sync::Arc;

use arc_swap::ArcSwap;
use block2::RcBlock;
use objc2::mutability::MainThreadOnly;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{declare_class, msg_send_id, ClassType, DeclaredClass};
use objc2_app_kit::NSApplicationDelegate;
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSRunLoop, NSTimer};

use crate::battery::{cb::CbBackend, BatteryBackend, BatteryEvent};
use crate::config::ConfigWatcher;
use crate::ipc::IpcServer;
use crate::menubar::{MenubarCommand, MenubarIcon};
use crate::notifier::{clear_armed_if_rose, decide, post, NotificationKind};
use crate::paths::Paths;
use crate::state::{BatteryReadingSnapshot, ChargingState, State};

/// Messages from the tokio task to the main run loop.
enum MainMsg {
    Render {
        percent: u8,
        charging: ChargingState,
    },
    SetVisible(bool),
    OpenPrefs,
    SetMuted(bool),
    SetMenubarEnabled(bool),
    Quit,
}

fn next_local_midnight(now: chrono::DateTime<chrono::Local>) -> chrono::DateTime<chrono::Local> {
    let next_day = (now + chrono::Duration::days(1)).date_naive();
    next_day
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .unwrap()
}

declare_class!(
    /// NSApp delegate. We only implement `applicationShouldHandleReopen:` so
    /// that double-clicking MXBattery.app while the daemon is already running
    /// (the LSUIElement re-launch path) opens the prefs window instead of
    /// being silently dropped.
    struct AppDelegate;

    unsafe impl ClassType for AppDelegate {
        type Super = NSObject;
        type Mutability = MainThreadOnly;
        const NAME: &'static str = "MXBatteryAppDelegate";
    }

    impl DeclaredClass for AppDelegate {
        type Ivars = ();
    }

    unsafe impl AppDelegate {
        #[method(applicationShouldHandleReopen:hasVisibleWindows:)]
        fn application_should_handle_reopen(
            &self,
            _sender: *mut NSObject,
            _has_visible: bool,
        ) -> bool {
            tracing::info!("app: reopen requested — opening prefs");
            if let Some(mtm) = MainThreadMarker::new() {
                crate::prefs_ui::PrefsWindow::show_or_focus(mtm);
            }
            true
        }
    }

    unsafe impl NSObjectProtocol for AppDelegate {}
    unsafe impl NSApplicationDelegate for AppDelegate {}
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(());
        unsafe { msg_send_id![super(this), init] }
    }
}

pub fn run_daemon() -> anyhow::Result<()> {
    let mtm = MainThreadMarker::new().expect("must run on main thread");
    let paths = Paths::standard()?;
    paths.ensure_app_support_dir()?;

    if !paths.config_file().exists() {
        crate::config::Config::default().save(&paths.config_file())?;
    }

    let watcher = ConfigWatcher::start(paths.config_file())?;
    let cfg_handle = watcher.handle();
    let mut state = State::load(&paths.state_file()).unwrap_or_default();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let server = rt.block_on(IpcServer::bind(paths.control_socket()))?;
    let mut ipc_rx = server.subscribe();

    crate::notifier::request_permission();

    let backend = CbBackend::start(cfg_handle.load_full().device.clone(), mtm);
    let mut bat_rx = backend.subscribe();

    // Set up a channel for the tokio task to send messages to the main thread.
    let (main_tx, main_rx) = tokio::sync::mpsc::unbounded_channel::<MainMsg>();

    // Wire the menubar click items to send commands back into the tokio task.
    // The same sender is reused if the menubar is reinstalled later.
    let (menubar_tx, mut menubar_rx) = tokio::sync::mpsc::unbounded_channel::<MenubarCommand>();

    // Install the menubar only if config asks for it; we may add/remove it
    // dynamically later when the user toggles the preference.
    let initial_menubar_enabled = cfg_handle.load_full().menubar.enabled;
    let menubar: Option<MenubarIcon> = if initial_menubar_enabled {
        let mb = MenubarIcon::install(mtm);
        mb.set_command_channel(menubar_tx.clone());
        Some(mb)
    } else {
        None
    };

    // Set up the NSApplication and policy.
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Accessory);

    // Install an NSApp delegate so re-launching MXBattery.app while the daemon
    // is running opens the prefs window. Held in a local so it isn't dropped
    // while NSApp holds a weak reference to it.
    let app_delegate = AppDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*app_delegate)));

    // Glue task: pump backend + ipc events and bridge to main thread.
    let cfg_handle_for_task = cfg_handle.clone();
    let paths_for_task = paths.clone();
    let main_tx_bat = main_tx.clone();
    let main_tx_ipc = main_tx.clone();
    let main_tx_menu = main_tx.clone();
    let main_tx_cfg = main_tx.clone();
    rt.spawn(async move {
        let mut last_percent: u8 = 0;
        let mut last_charging = ChargingState::Unknown;
        let mut device_name = String::from("Logitech mouse");
        let mut last_menubar_enabled = cfg_handle_for_task.load_full().menubar.enabled;
        let mut cfg_tick = tokio::time::interval(std::time::Duration::from_millis(500));
        cfg_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Connection / visibility tracking. We hide the menubar icon when no
        // mouse is connected, with a 5-second grace period to avoid flicker
        // on transient BLE drops.
        let mut connected_count: usize = 0;
        let mut have_reading = false;
        let mut visible = false;
        let mut hide_at: Option<tokio::time::Instant> = None;
        const HIDE_GRACE: std::time::Duration = std::time::Duration::from_secs(5);
        loop {
            tokio::select! {
                _ = cfg_tick.tick() => {
                    let cur = cfg_handle_for_task.load_full().menubar.enabled;
                    if cur != last_menubar_enabled {
                        last_menubar_enabled = cur;
                        let _ = main_tx_cfg.send(MainMsg::SetMenubarEnabled(cur));
                    }
                }
                _ = async {
                    match hide_at {
                        Some(t) => tokio::time::sleep_until(t).await,
                        None => std::future::pending().await,
                    }
                } => {
                    hide_at = None;
                    if visible {
                        visible = false;
                        let _ = main_tx_bat.send(MainMsg::SetVisible(false));
                    }
                }
                ev = bat_rx.recv() => {
                    let Ok(ev) = ev else { continue };
                    match ev {
                        BatteryEvent::Connected { name } => {
                            device_name = name;
                            connected_count += 1;
                            hide_at = None;
                            tracing::info!(connected_count, have_reading, visible, "app: BatteryEvent::Connected");
                            if have_reading && !visible {
                                visible = true;
                                let _ = main_tx_bat.send(MainMsg::SetVisible(true));
                            }
                        }
                        BatteryEvent::Disconnected => {
                            connected_count = connected_count.saturating_sub(1);
                            tracing::info!(connected_count, visible, "app: BatteryEvent::Disconnected");
                            if connected_count == 0 && visible {
                                hide_at = Some(tokio::time::Instant::now() + HIDE_GRACE);
                            }
                        }
                        BatteryEvent::Percent(p) => {
                            last_percent = p;
                            have_reading = true;
                            hide_at = None;
                            handle_reading(
                                &cfg_handle_for_task,
                                &paths_for_task,
                                &mut state,
                                last_percent,
                                last_charging,
                                &device_name,
                            );
                            let _ = main_tx_bat.send(MainMsg::Render {
                                percent: last_percent,
                                charging: last_charging,
                            });
                            if !visible {
                                visible = true;
                                let _ = main_tx_bat.send(MainMsg::SetVisible(true));
                            }
                        }
                        BatteryEvent::Charging(c) => {
                            last_charging = c;
                            have_reading = true;
                            hide_at = None;
                            handle_reading(
                                &cfg_handle_for_task,
                                &paths_for_task,
                                &mut state,
                                last_percent,
                                last_charging,
                                &device_name,
                            );
                            let _ = main_tx_bat.send(MainMsg::Render {
                                percent: last_percent,
                                charging: last_charging,
                            });
                            if !visible {
                                visible = true;
                                let _ = main_tx_bat.send(MainMsg::SetVisible(true));
                            }
                        }
                    }
                }
                cmd = ipc_rx.recv() => {
                    let Ok(_cmd) = cmd else { continue };
                    let _ = main_tx_ipc.send(MainMsg::OpenPrefs);
                }
                cmd = menubar_rx.recv() => {
                    let Some(cmd) = cmd else { continue };
                    match cmd {
                        MenubarCommand::OpenPrefs => {
                            let _ = main_tx_menu.send(MainMsg::OpenPrefs);
                        }
                        MenubarCommand::ToggleMuteToday => {
                            let now = chrono::Local::now();
                            state.mute_until = match state.mute_until {
                                Some(t) if t > now => None,
                                _ => Some(next_local_midnight(now)),
                            };
                            let _ = state.save(&paths_for_task.state_file());
                            let muted = state.mute_until.is_some_and(|t| t > now);
                            let _ = main_tx_menu.send(MainMsg::SetMuted(muted));
                        }
                        MenubarCommand::Quit => {
                            let _ = main_tx_menu.send(MainMsg::Quit);
                        }
                    }
                }
            }
        }
    });

    // Drain main-thread messages via a recurring NSTimer (0.1 s).
    //
    // The timer fires on the main run loop (same thread NSApp::run() processes
    // events on), so all AppKit calls inside are safe.
    //
    // We wrap `main_rx` in an Arc<Mutex> so it can be moved into the block
    // which must be `'static` (RcBlock captures by value).
    let main_rx = Arc::new(std::sync::Mutex::new(main_rx));
    // The timer block owns the menubar slot so it can install/remove on the
    // main thread when the preference toggles. RefCell + Option models the
    // present/absent states; everything inside runs on the main run loop.
    let menubar_slot = std::rc::Rc::new(std::cell::RefCell::new(menubar));
    // Track the most recent battery reading so we can render after a fresh install.
    let last_reading = std::rc::Rc::new(std::cell::Cell::new((0u8, ChargingState::Unknown)));
    // Track whether the icon should currently be visible, so a config-toggle
    // re-install can restore the right visibility (hidden when no mouse).
    let visible_state = std::rc::Rc::new(std::cell::Cell::new(false));
    let timer_block = {
        let main_rx = Arc::clone(&main_rx);
        let menubar_slot = std::rc::Rc::clone(&menubar_slot);
        let menubar_tx = menubar_tx.clone();
        let last_reading = std::rc::Rc::clone(&last_reading);
        let visible_state = std::rc::Rc::clone(&visible_state);
        RcBlock::new(move |_timer: NonNull<NSTimer>| {
            let mtm_inner = match MainThreadMarker::new() {
                Some(m) => m,
                None => return,
            };
            let mut rx = main_rx.lock().unwrap();
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    MainMsg::Render { percent, charging } => {
                        last_reading.set((percent, charging));
                        if let Some(mb) = menubar_slot.borrow().as_ref() {
                            mb.render(percent, charging, mtm_inner);
                        }
                    }
                    MainMsg::SetVisible(v) => {
                        visible_state.set(v);
                        if let Some(mb) = menubar_slot.borrow().as_ref() {
                            mb.set_visible(v);
                        }
                    }
                    MainMsg::OpenPrefs => {
                        tracing::info!("app: dispatching OpenPrefs to PrefsWindow on main thread");
                        crate::prefs_ui::PrefsWindow::show_or_focus(mtm_inner);
                    }
                    MainMsg::SetMuted(muted) => {
                        if let Some(mb) = menubar_slot.borrow().as_ref() {
                            mb.set_muted(muted);
                        }
                    }
                    MainMsg::SetMenubarEnabled(enabled) => {
                        let mut slot = menubar_slot.borrow_mut();
                        match (enabled, slot.is_some()) {
                            (true, false) => {
                                tracing::info!("menubar: install (config toggled on)");
                                let mb = MenubarIcon::install(mtm_inner);
                                mb.set_command_channel(menubar_tx.clone());
                                let (p, c) = last_reading.get();
                                mb.render(p, c, mtm_inner);
                                mb.set_visible(visible_state.get());
                                *slot = Some(mb);
                            }
                            (false, true) => {
                                tracing::info!("menubar: remove (config toggled off)");
                                if let Some(mb) = slot.take() {
                                    mb.remove();
                                }
                            }
                            _ => {}
                        }
                    }
                    MainMsg::Quit => {
                        tracing::info!("app: terminating on user request");
                        let app = objc2_app_kit::NSApplication::sharedApplication(mtm_inner);
                        unsafe { app.terminate(None) };
                    }
                }
            }
        })
    };

    let timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.1, true, &timer_block) };
    // Add the timer to the common run loop modes so it keeps firing even when
    // the user is interacting with menus.
    let run_loop = unsafe { NSRunLoop::mainRunLoop() };
    unsafe {
        run_loop.addTimer_forMode(&timer, objc2_foundation::NSRunLoopCommonModes);
    }

    unsafe { app.run() };

    // Invalidate the timer after the run loop exits so it doesn't fire again.
    unsafe { timer.invalidate() };

    // Drop the menubar slot (and the menubar inside, if any).
    drop(menubar_slot);
    drop(app_delegate);
    Ok(())
}

fn handle_reading(
    cfg_handle: &Arc<ArcSwap<crate::config::Config>>,
    paths: &Paths,
    state: &mut State,
    percent: u8,
    charging: ChargingState,
    device_name: &str,
) {
    let cfg = cfg_handle.load_full();
    let snap = BatteryReadingSnapshot { percent, charging };
    clear_armed_if_rose(&snap, state, &cfg);
    state.last_seen = Some(snap);
    if let Some(kind) = decide(&snap, state, &cfg, chrono::Local::now()) {
        post(kind, percent, device_name);
        match kind {
            NotificationKind::Warn => {
                state.last_warn_notified_date = Some(chrono::Local::now().date_naive());
            }
            NotificationKind::Critical => {
                state.last_critical_notified_at = Some(chrono::Local::now());
            }
        }
    }
    let _ = state.save(&paths.state_file());
}
