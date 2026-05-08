//! Main daemon run loop — NSApplication wiring and event glue.
//!
//! Called by `lib::run()` on macOS.  Must be invoked on the main thread.

use std::ptr::NonNull;
use std::sync::Arc;

use arc_swap::ArcSwap;
use block2::RcBlock;
use objc2_foundation::{MainThreadMarker, NSRunLoop, NSTimer};

use crate::battery::{cb::CbBackend, BatteryBackend, BatteryEvent};
use crate::config::ConfigWatcher;
use crate::ipc::IpcServer;
use crate::menubar::MenubarIcon;
use crate::notifier::{clear_armed_if_rose, decide, post, NotificationKind};
use crate::paths::Paths;
use crate::state::{BatteryReadingSnapshot, ChargingState, State};

/// Messages from the tokio task to the main run loop.
enum MainMsg {
    Render { percent: u8, charging: ChargingState },
    OpenPrefs,
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

    let menubar = MenubarIcon::install(mtm);

    // Set up a channel for the tokio task to send messages to the main thread.
    let (main_tx, main_rx) = tokio::sync::mpsc::unbounded_channel::<MainMsg>();

    // Set up the NSApplication and policy.
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Accessory);

    // Glue task: pump backend + ipc events and bridge to main thread.
    let cfg_handle_for_task = cfg_handle.clone();
    let paths_for_task = paths.clone();
    let main_tx_bat = main_tx.clone();
    let main_tx_ipc = main_tx.clone();
    rt.spawn(async move {
        let mut last_percent: u8 = 0;
        let mut last_charging = ChargingState::Unknown;
        loop {
            tokio::select! {
                ev = bat_rx.recv() => {
                    let Ok(ev) = ev else { continue };
                    match ev {
                        BatteryEvent::Connected { .. } => {}
                        BatteryEvent::Disconnected => {}
                        BatteryEvent::Percent(p) => {
                            last_percent = p;
                            handle_reading(
                                &cfg_handle_for_task,
                                &paths_for_task,
                                &mut state,
                                last_percent,
                                last_charging,
                            );
                            let _ = main_tx_bat.send(MainMsg::Render {
                                percent: last_percent,
                                charging: last_charging,
                            });
                        }
                        BatteryEvent::Charging(c) => {
                            last_charging = c;
                            handle_reading(
                                &cfg_handle_for_task,
                                &paths_for_task,
                                &mut state,
                                last_percent,
                                last_charging,
                            );
                            let _ = main_tx_bat.send(MainMsg::Render {
                                percent: last_percent,
                                charging: last_charging,
                            });
                        }
                    }
                }
                cmd = ipc_rx.recv() => {
                    let Ok(_cmd) = cmd else { continue };
                    let _ = main_tx_ipc.send(MainMsg::OpenPrefs);
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
    let menubar_ptr: *const MenubarIcon = &menubar;
    // SAFETY: `menubar` outlives the timer because we only call `app.run()` while
    // both are alive, and the timer is invalidated when `app.run()` returns (the
    // run loop is torn down).
    let timer_block = {
        let main_rx = Arc::clone(&main_rx);
        // SAFETY: menubar_ptr points to a stack local that lives for the rest
        // of `run_daemon` (past the NSApp run loop).
        RcBlock::new(move |_timer: NonNull<NSTimer>| {
            let mtm_inner = match MainThreadMarker::new() {
                Some(m) => m,
                None => return,
            };
            let mut rx = main_rx.lock().unwrap();
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    MainMsg::Render { percent, charging } => {
                        // SAFETY: menubar_ptr is valid (see above).
                        unsafe { (*menubar_ptr).render(percent, charging, mtm_inner) };
                    }
                    MainMsg::OpenPrefs => {
                        crate::prefs_ui::PrefsWindow::show_or_focus(mtm_inner);
                    }
                }
            }
        })
    };

    let timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.1, true, &timer_block)
    };
    // Add the timer to the common run loop modes so it keeps firing even when
    // the user is interacting with menus.
    let run_loop = unsafe { NSRunLoop::mainRunLoop() };
    unsafe {
        run_loop.addTimer_forMode(
            &timer,
            objc2_foundation::NSRunLoopCommonModes,
        );
    }

    unsafe { app.run() };

    // Invalidate the timer after the run loop exits so it doesn't fire again.
    unsafe { timer.invalidate() };

    drop(menubar);
    Ok(())
}

fn handle_reading(
    cfg_handle: &Arc<ArcSwap<crate::config::Config>>,
    paths: &Paths,
    state: &mut State,
    percent: u8,
    charging: ChargingState,
) {
    let cfg = cfg_handle.load_full();
    let snap = BatteryReadingSnapshot { percent, charging };
    clear_armed_if_rose(&snap, state, &cfg);
    state.last_seen = Some(snap);
    if let Some(kind) = decide(&snap, state, &cfg, chrono::Local::now()) {
        post(kind, percent, "MX Master 3 Mac");
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
