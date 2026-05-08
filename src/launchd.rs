use crate::paths::{Paths, BUNDLE_ID};

pub fn install(app_bundle_path: &str) -> anyhow::Result<()> {
    let paths = Paths::standard()?;
    let plist_path = paths.launch_agent_plist();
    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let plist = format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>{out}</string>
    <key>StandardErrorPath</key><string>{err}</string>
</dict>
</plist>
"#,
        label = BUNDLE_ID,
        exe = format!("{}/Contents/MacOS/mxbattery", app_bundle_path),
        out = paths.app_support_dir().join("daemon.log").display(),
        err = paths.app_support_dir().join("daemon.err.log").display(),
    );
    std::fs::write(&plist_path, plist)?;
    let uid = unsafe { libc::getuid() };
    let domain = format!("gui/{}", uid);
    std::process::Command::new("launchctl").args(["bootout", &domain, plist_path.to_str().unwrap()]).status().ok();
    let st = std::process::Command::new("launchctl").args(["bootstrap", &domain, plist_path.to_str().unwrap()]).status()?;
    anyhow::ensure!(st.success(), "launchctl bootstrap failed");
    Ok(())
}

pub fn uninstall(purge_data: bool) -> anyhow::Result<()> {
    let paths = Paths::standard()?;
    let plist = paths.launch_agent_plist();
    let uid = unsafe { libc::getuid() };
    let domain = format!("gui/{}", uid);
    if plist.exists() {
        let _ = std::process::Command::new("launchctl").args(["bootout", &domain, plist.to_str().unwrap()]).status();
        std::fs::remove_file(&plist)?;
    }
    if purge_data {
        let _ = std::fs::remove_dir_all(paths.app_support_dir());
    }
    Ok(())
}
