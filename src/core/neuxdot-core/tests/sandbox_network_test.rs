//! The network allow-list is a security boundary, so it is tested as one: the
//! proxy is started for real, spoken to over its unix socket, and asked to
//! reach both a permitted and a forbidden destination.
//!
//! No network access is required — the "permitted" origin is a local TCP
//! listener that the test starts itself, which also proves the proxy actually
//! tunnels bytes rather than merely answering `200`.

use std::time::Duration;

use neuxdot_core::db::schema::NetworkPermissions;
use neuxdot_core::mcp::proxy::AllowListProxy;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UnixStream};

/// Bind a throwaway local origin server and return its port.
async fn origin_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind origin");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nORIGIN")
                    .await;
                let _ = stream.shutdown().await;
            });
        }
    });
    port
}

async fn send(socket: &std::path::Path, request: &str) -> String {
    let mut stream = UnixStream::connect(socket).await.expect("connect proxy");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).to_string()
}

#[tokio::test]
async fn proxy_refuses_destinations_outside_the_allow_list() {
    let port = origin_server().await;
    let socket = std::env::temp_dir().join("neuxdot-proxy-test-refuse.sock");
    let proxy = AllowListProxy::start(
        socket.clone(),
        NetworkPermissions {
            allow: vec!["allowed.example".to_string()],
        },
    )
    .await
    .expect("start proxy");

    // 127.0.0.1 is a valid, LIVE destination, but not on the list.
    let response = send(
        proxy.socket(),
        &format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
    )
    .await;

    assert!(
        response.starts_with("HTTP/1.1 403"),
        "expected the proxy to refuse an off-list destination, got: {response}"
    );
    assert!(
        response.contains("not in this plugin's network allow-list"),
        "refusal must name the reason, got: {response}"
    );

    drop(proxy);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn proxy_tunnels_to_an_allow_listed_destination() {
    let port = origin_server().await;
    let socket = std::env::temp_dir().join("neuxdot-proxy-test-allow.sock");
    let proxy = AllowListProxy::start(
        socket.clone(),
        NetworkPermissions {
            allow: vec!["127.0.0.1".to_string()],
        },
    )
    .await
    .expect("start proxy");

    // A permitted origin: the proxy must answer 200 and faithfully relay bytes.
    let mut stream = UnixStream::connect(proxy.socket()).await.expect("connect");
    stream
        .write_all(
            format!("GET http://127.0.0.1:{port}/probe HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                .as_bytes(),
        )
        .await
        .expect("write");
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut out)).await;
    let response = String::from_utf8_lossy(&out).to_string();

    assert!(
        response.contains("ORIGIN"),
        "expected the origin body to be relayed through the proxy, got: {response}"
    );

    drop(proxy);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn proxy_refuses_when_the_policy_grants_nothing() {
    let port = origin_server().await;
    let socket = std::env::temp_dir().join("neuxdot-proxy-test-deny.sock");
    let proxy = AllowListProxy::start(socket.clone(), NetworkPermissions::default())
        .await
        .expect("start proxy");

    let response = send(
        proxy.socket(),
        &format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n"),
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 403"),
        "an empty allow-list must refuse everything, got: {response}"
    );

    drop(proxy);
    let _ = std::fs::remove_file(&socket);
}
