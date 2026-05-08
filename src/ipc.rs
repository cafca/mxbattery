use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

#[derive(Clone, Copy, Debug)]
pub enum IpcCommand { OpenPrefs }

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("daemon already running on this socket")]
    AlreadyRunning,
}

pub struct IpcServer {
    tx: broadcast::Sender<IpcCommand>,
    _path: PathBuf,
}

impl IpcServer {
    pub async fn bind(path: PathBuf) -> Result<Self, IpcError> {
        if path.exists() {
            if UnixStream::connect(&path).await.is_ok() {
                return Err(IpcError::AlreadyRunning);
            }
            let _ = std::fs::remove_file(&path);
        }
        let listener = UnixListener::bind(&path)?;
        let (tx, _rx) = broadcast::channel(8);
        let tx2 = tx.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(e) => { tracing::warn!(?e, "accept"); continue; }
                };
                let tx = tx2.clone();
                tokio::spawn(async move {
                    use tokio::io::AsyncReadExt;
                    let mut buf = String::new();
                    if stream.take(4096).read_to_string(&mut buf).await.is_ok() {
                        if buf.trim() == "open_prefs" {
                            let _ = tx.send(IpcCommand::OpenPrefs);
                        }
                    }
                });
            }
        });
        Ok(Self { tx, _path: path })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<IpcCommand> { self.tx.subscribe() }
}

pub async fn send_open_prefs(path: &Path) -> Result<(), IpcError> {
    let mut s = UnixStream::connect(path).await?;
    s.write_all(b"open_prefs\n").await?;
    Ok(())
}
