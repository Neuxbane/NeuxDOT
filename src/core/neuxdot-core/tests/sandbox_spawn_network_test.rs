//! End-to-end: a sandboxed stdio plugin keeps its network path **after**
//! `spawn_with_sandbox` returns.
//!
//! This is the regression that produced `Telegram polling failed: network
//! error: error sending request`, while the log simultaneously reported a
//! granted network allow-list. The allow-list proxy used to be a local
//! `_proxy_guard` inside `spawn_with_sandbox`; returning from that function
//! dropped it, aborting the proxy task and deleting its unix socket. The child
//! stayed alive with a `socat` relay pointing at a socket nobody was serving.
//!
//! The test spawns a real MCP stdio server under `bwrap --unshare-all`,
//! grants `127.0.0.1`, and then — from the *host* test process, after spawn has
//! returned — asks the plugin to fetch a local origin through its proxy. If the
//! guard is ever dropped too early, the fetch fails again.

use std::sync::Arc;

use neuxdot_core::db::schema::NetworkPermissions;
use neuxdot_core::mcp::client::McpTransport;
use neuxdot_core::mcp::sandbox::SandboxSpec;
use neuxdot_core::mcp::stdio::StdioTransport;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// An MCP stdio server that fetches `$PROBE_URL` through whatever proxy the
/// sandbox injects, and returns the body as a tool result.
const MCP_SERVER: &str = r#"
import json, os, subprocess, sys

def reply(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if "id" not in msg:
        continue  # notification
    mid, method = msg["id"], msg.get("method")

    if method == "initialize":
        reply({"jsonrpc": "2.0", "id": mid, "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "probe", "version": "0.0.0"}}})
    elif method == "tools/list":
        reply({"jsonrpc": "2.0", "id": mid, "result": {"tools": [
            {"name": "fetch", "description": "fetch the probe URL",
             "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "tools/call":
        url = os.environ.get("PROBE_URL", "")
        try:
            out = subprocess.run(
                ["curl", "-sS", "--max-time", "10", url],
                capture_output=True, text=True, timeout=15)
            text = out.stdout if out.returncode == 0 else f"curl failed rc={out.returncode}: {out.stderr}"
        except Exception as e:
            text = f"exception: {e}"
        reply({"jsonrpc": "2.0", "id": mid, "result": {
            "content": [{"type": "text", "text": text}], "isError": False}})
    else:
        reply({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "no method"}})
"#;

/// Local origin server; the sandbox can only reach it through the host proxy.
async fn origin_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind origin");
    let port = listener.local_addr().unwrap().port();
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

async fn spawn_probe(
    workspace: &std::path::Path,
    allow: Vec<String>,
    probe_url: &str,
) -> Arc<StdioTransport> {
    // One socket per test: two tests in the same process must not race for it.
    let tag = workspace
        .file_name()
        .map(|n| n.to_string_lossy().replace(|c: char| !c.is_ascii_alphanumeric(), "_"))
        .unwrap_or_else(|| "probe".to_string());
    std::fs::create_dir_all(workspace).expect("workspace");
    std::fs::write(workspace.join("mcp.py"), MCP_SERVER).expect("write server");

    let spec = SandboxSpec {
        workspace: workspace.to_path_buf(),
        // python3 lives under /usr (read-only bind), so it is reachable and
        // gets bound at PROGRAM_MOUNT like a real binary plugin.
        program: Some(std::path::PathBuf::from("/usr/bin/python3")),
        network: NetworkPermissions { allow },
        proxy_socket: Some(std::env::temp_dir().join(format!("neuxdot-e2e-{tag}.sock"))),
    };

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let transport = StdioTransport::spawn_with_sandbox(
        "/usr/bin/python3 -u /workspace/mcp.py",
        Some(&format!("PROBE_URL={probe_url}")),
        None,
        tx,
        &neuxdot_core::mcp::sandbox::Launch::Sandboxed(spec),
    )
    .await
    .expect("spawn sandboxed probe");
    transport
}

async fn fetch(transport: &StdioTransport) -> String {
    let res = transport
        .send_request(
            "tools/call",
            Some(json!({ "name": "fetch", "arguments": {} })),
        )
        .await
        .expect("tools/call");
    res["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn sandboxed_plugin_can_reach_an_allow_listed_host_after_spawn_returns() {
    let port = origin_server().await;
    let url = format!("http://127.0.0.1:{port}/probe");
    let workspace = std::env::temp_dir().join(format!("neuxdot-e2e-ws-{}", std::process::id()));

    let transport = spawn_probe(&workspace, vec!["127.0.0.1".to_string()], &url).await;

    // `spawn_probe` has RETURNED. If the proxy guard had been dropped with it,
    // the relay inside the sandbox would have no backend and this would fail
    // with a curl connection error instead of relaying ORIGIN.
    let body = fetch(&transport).await;
    assert!(
        body.contains("ORIGIN"),
        "sandboxed plugin could not reach the allow-listed origin after spawn returned: {body:?}"
    );

    let _ = transport.close().await;
    let _ = std::fs::remove_dir_all(&workspace);
}

#[tokio::test]
async fn sandboxed_plugin_is_still_denied_an_unlisted_host() {
    let port = origin_server().await;
    let url = format!("http://127.0.0.1:{port}/probe");
    let workspace = std::env::temp_dir().join(format!("neuxdot-e2e-deny-{}", std::process::id()));

    // Grant a host that is NOT the origin: the proxy must refuse.
    let transport = spawn_probe(&workspace, vec!["allowed.example".to_string()], &url).await;

    let body = fetch(&transport).await;
    assert!(
        !body.contains("ORIGIN"),
        "an off-list destination must never be relayed, got: {body:?}"
    );

    let _ = transport.close().await;
    let _ = std::fs::remove_dir_all(&workspace);
}
