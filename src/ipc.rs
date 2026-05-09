use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

#[derive(Clone, Copy, Debug)]
pub enum IpcCommand {
    OpenPrefs,
}

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("daemon already running on this socket")]
    AlreadyRunning,
}

pub struct IpcServer {
    tx: broadcast::Sender<IpcCommand>,
    _path: PathBuf,
    _pid_path: PathBuf,
}

/// Files registered by the most recent `IpcServer::bind` so that a C-level
/// `atexit` handler can remove them even when `NSApplication::terminate` calls
/// `exit()` and bypasses Rust drop chains.
static CLEANUP_FILES: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();

extern "C" fn atexit_cleanup() {
    if let Some((sock, pid)) = CLEANUP_FILES.get() {
        let _ = std::fs::remove_file(sock);
        let _ = std::fs::remove_file(pid);
    }
}

fn pid_file_path(socket: &Path) -> PathBuf {
    let mut p = socket.to_path_buf();
    let stem = socket
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("control");
    p.set_file_name(format!("{stem}.pid"));
    p
}

fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill(pid, 0) just probes; no signal is delivered.
    unsafe { libc::kill(pid, 0) == 0 }
}

impl IpcServer {
    pub async fn bind(path: PathBuf) -> Result<Self, IpcError> {
        let pid_path = pid_file_path(&path);

        // Authoritative liveness check: if the PID file points to a live
        // process, treat the daemon as already running. Otherwise clean both
        // files and take over. We do not trust UnixStream::connect alone —
        // a dying daemon's listener can briefly accept connections after the
        // owning process is gone or about to be gone.
        if let Ok(content) = std::fs::read_to_string(&pid_path) {
            if let Ok(pid) = content.trim().parse::<i32>() {
                if pid_alive(pid) {
                    return Err(IpcError::AlreadyRunning);
                }
            }
        }

        let _ = std::fs::remove_file(&pid_path);
        let _ = std::fs::remove_file(&path);

        let listener = UnixListener::bind(&path)?;

        let my_pid = std::process::id();
        if let Err(e) = std::fs::write(&pid_path, my_pid.to_string()) {
            let _ = std::fs::remove_file(&path);
            return Err(IpcError::Io(e));
        }

        // Register the cleanup once per process. Subsequent calls are no-ops
        // because the OnceLock is already filled, but the same paths will be
        // reused (same Paths::standard() across the lifetime).
        let _ = CLEANUP_FILES.set((path.clone(), pid_path.clone()));
        // SAFETY: registering a no-arg extern "C" function with C atexit is
        // straightforward and safe. We don't care if libc::atexit returns
        // non-zero — at worst the cleanup is missed at exit().
        unsafe {
            libc::atexit(atexit_cleanup);
        }

        let (tx, _rx) = broadcast::channel(8);
        let tx2 = tx.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(?e, "accept");
                        continue;
                    }
                };
                let tx = tx2.clone();
                tokio::spawn(async move {
                    use tokio::io::AsyncReadExt;
                    let mut buf = String::new();
                    if stream.take(4096).read_to_string(&mut buf).await.is_ok()
                        && buf.trim() == "open_prefs"
                    {
                        let _ = tx.send(IpcCommand::OpenPrefs);
                    }
                });
            }
        });

        Ok(Self {
            tx,
            _path: path,
            _pid_path: pid_path,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<IpcCommand> {
        self.tx.subscribe()
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self._path);
        let _ = std::fs::remove_file(&self._pid_path);
    }
}

pub async fn send_open_prefs(path: &Path) -> Result<(), IpcError> {
    let mut s = UnixStream::connect(path).await?;
    s.write_all(b"open_prefs\n").await?;
    Ok(())
}
