use mxbattery::ipc::{send_open_prefs, IpcCommand, IpcServer};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_instance_lands_open_prefs() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("control.sock");
    let server = IpcServer::bind(sock.clone()).await.unwrap();

    let recv = tokio::spawn({
        let mut events = server.subscribe();
        async move {
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap()
        }
    });

    send_open_prefs(&sock).await.unwrap();
    let cmd = recv.await.unwrap();
    assert!(matches!(cmd, IpcCommand::OpenPrefs));
}

#[tokio::test]
async fn second_bind_fails_with_kind_already_running() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("control.sock");
    let _server1 = IpcServer::bind(sock.clone()).await.unwrap();
    let err = IpcServer::bind(sock.clone()).await.err().unwrap();
    assert!(matches!(err, mxbattery::ipc::IpcError::AlreadyRunning));
}
