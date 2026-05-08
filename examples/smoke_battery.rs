use mxbattery::battery::{cb::CbBackend, BatteryBackend};
use mxbattery::config::DeviceFilter;
use objc2_foundation::MainThreadMarker;

fn main() {
    let mtm = MainThreadMarker::new().expect("main thread");
    let backend = CbBackend::start(DeviceFilter::AnyMx, mtm);
    let mut rx = backend.subscribe();
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    std::thread::spawn(move || loop {
        if let Ok(ev) = rx.blocking_recv() {
            println!("{:?}", ev);
        }
    });
    unsafe {
        app.run();
    }
}
