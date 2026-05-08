use mxbattery::paths::Paths;
use std::path::PathBuf;

#[test]
fn paths_under_app_support() {
    let p = Paths::with_root(PathBuf::from("/tmp/mxb-test"));
    assert_eq!(p.config_file(), PathBuf::from("/tmp/mxb-test/config.toml"));
    assert_eq!(p.state_file(), PathBuf::from("/tmp/mxb-test/state.json"));
    assert_eq!(p.control_socket(), PathBuf::from("/tmp/mxb-test/control.sock"));
    assert_eq!(p.app_support_dir(), PathBuf::from("/tmp/mxb-test"));
}

#[test]
fn launch_agent_path_uses_bundle_id() {
    let p = Paths::with_root(PathBuf::from("/tmp/mxb-test"));
    let la = p.launch_agent_plist();
    assert!(la.ends_with("Library/LaunchAgents/com.vincentahrend.mxbattery-app.plist"));
}
