use std::path::{Path, PathBuf};

pub const BUNDLE_ID: &str = "com.vincentahrend.mxbattery-app";
pub const APP_SUPPORT_DIRNAME: &str = "MXBattery";

#[derive(Clone, Debug)]
pub struct Paths {
    root: PathBuf,
    home: PathBuf,
}

impl Paths {
    /// Resolve the standard locations using the user's HOME.
    pub fn standard() -> anyhow::Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("HOME not set"))?;
        let root = home
            .join("Library/Application Support")
            .join(APP_SUPPORT_DIRNAME);
        Ok(Self { root, home })
    }

    /// Override paths for tests. The given root is used as both the application-support
    /// directory and (synthetically) the home root for the LaunchAgent path.
    pub fn with_root(root: PathBuf) -> Self {
        Self {
            home: root.clone(),
            root,
        }
    }

    pub fn app_support_dir(&self) -> PathBuf {
        self.root.clone()
    }
    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }
    pub fn state_file(&self) -> PathBuf {
        self.root.join("state.json")
    }
    pub fn control_socket(&self) -> PathBuf {
        self.root.join("control.sock")
    }

    pub fn launch_agent_plist(&self) -> PathBuf {
        self.home
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", BUNDLE_ID))
    }

    /// Create the application-support directory if it does not exist.
    pub fn ensure_app_support_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)
    }
}

impl AsRef<Path> for Paths {
    fn as_ref(&self) -> &Path {
        &self.root
    }
}
