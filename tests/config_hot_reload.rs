use mxbattery::config::{Config, ConfigWatcher};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watcher_emits_on_save_and_keeps_old_on_bad_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    Config::default().save(&path).unwrap();

    let watcher = ConfigWatcher::start(path.clone()).unwrap();
    let initial = watcher.current();
    assert_eq!(initial.thresholds.warn, 20);

    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut next = Config::default();
    next.thresholds.warn = 15;
    next.save(&path).unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if watcher.current().thresholds.warn == 15 { break; }
        if tokio::time::Instant::now() >= deadline {
            panic!("watcher did not see new config in time");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    std::fs::write(&path, "schema_version = 1\n[thresholds]\nwarn = 5\ncritical = 10\nrearm_hysteresis = 5\nwarn_enabled = true\ncritical_enabled = true\n[device]\nmode = \"any-mx\"\n[cadence]\nwarn_period = \"24h\"\ncritical_period = \"30m\"\n[menubar]\nenabled = true\n[autostart]\nenabled = false\n").unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(watcher.current().thresholds.warn, 15, "kept previous good config");
}
