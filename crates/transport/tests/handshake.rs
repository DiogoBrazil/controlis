use std::sync::{Arc, Mutex};

use protocol::{ControlMessage, Hello, Role, VideoCodec, PROTOCOL_VERSION};
use transport::{connect_viewer, HostIdentity, HostListener};

fn localhost() -> std::net::SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

async fn hello(role: Role) -> ControlMessage {
    ControlMessage::Hello(Hello {
        protocol_version: PROTOCOL_VERSION,
        app_version: "test".into(),
        role,
        supported_codecs: vec![VideoCodec::JpegTiles],
    })
}

#[tokio::test]
async fn viewer_and_host_exchange_control_messages() {
    let identity = HostIdentity::generate().unwrap();
    let listener = HostListener::bind(localhost(), &identity).unwrap();
    let addr = listener.local_addr().unwrap();

    let host = tokio::spawn(async move {
        let conn = listener
            .accept()
            .await
            .expect("endpoint open")
            .expect("handshake ok");
        let mut control = conn.accept_control().await.expect("accept control");
        let received = control.recv().await.expect("host recv");
        control
            .send(&ControlMessage::Pong { nonce: 1 })
            .await
            .expect("host send");
        // Keep the connection alive until the viewer has read the reply.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        received
    });

    let seen = Arc::new(Mutex::new(None));
    let conn = connect_viewer(addr, None, seen.clone()).await.unwrap();
    let mut control = conn.open_control().await.unwrap();
    control.send(&hello(Role::Viewer).await).await.unwrap();
    let reply = control.recv().await.unwrap();

    assert_eq!(reply, ControlMessage::Pong { nonce: 1 });
    assert_eq!(host.await.unwrap(), hello(Role::Viewer).await);
    // First contact recorded the host fingerprint for trust-on-first-use.
    assert!(seen.lock().unwrap().is_some());
}

#[tokio::test]
async fn pinned_fingerprint_mismatch_is_rejected() {
    let identity = HostIdentity::generate().unwrap();
    let listener = HostListener::bind(localhost(), &identity).unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        // Accept and hold; the handshake should fail before control is exchanged.
        let _ = listener.accept().await;
    });

    // Pin a fingerprint that is not the host's.
    let wrong = HostIdentity::generate().unwrap().fingerprint().clone();
    let seen = Arc::new(Mutex::new(None));
    let result = connect_viewer(addr, Some(wrong), seen).await;
    assert!(result.is_err(), "mismatched pin must abort the handshake");
}
