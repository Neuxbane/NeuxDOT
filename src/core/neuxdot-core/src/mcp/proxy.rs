//! The only route out of a sandboxed plugin: a CONNECT/HTTP allow-list proxy.
//!
//! A sandboxed plugin has no network namespace of its own, so it cannot open a
//! route to anywhere. [`crate::mcp::sandbox`] binds a unix socket into the
//! sandbox and relays `127.0.0.1:RELAY_PORT` to it, so every outbound
//! connection a well-behaved HTTP client makes arrives here.
//!
//! The proxy does two things and nothing else:
//!
//! 1. Parse the requested destination (`CONNECT host:port` or an absolute-form
//!    request line for plain HTTP).
//! 2. Ask [`NetworkPermissions::allows_host`] whether the manifest granted it.
//!    Refuse with `403` and a named reason when it did not.
//!
//! It is deliberately not a caching proxy, not a TLS interceptor and not
//! authenticated: it lives in the host's process, is bound to a unix socket
//! that only the sandbox can reach, and forwards bytes without inspecting them.
//!
//! ## What it does not stop
//!
//! A plugin that ignores `HTTP_PROXY` and speaks raw TCP cannot reach anything:
//! the sandbox has no route except loopback, and loopback is where this proxy
//! listens. It also cannot reach the proxy's *targets* directly, because it
//! never gets the host network stack unless the manifest said `["*"]`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixListener};
use tracing::{debug, info, warn};

use crate::db::schema::NetworkPermissions;

/// Cap on a request head the proxy will buffer before giving up. A CONNECT line
/// is tiny; anything larger is either a client bug or abuse.
const MAX_HEAD_BYTES: usize = 8 * 1024;

/// How long a client may take to send its request head.
const HEAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long the upstream connection may take to establish.
const UPSTREAM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// A running allow-list proxy for one plugin.
pub struct AllowListProxy {
    socket: PathBuf,
    handle: tokio::task::JoinHandle<()>,
}

impl AllowListProxy {
    /// Start the proxy on `socket`, enforcing `allow`.
    ///
    /// An existing socket file is removed first: a crashed previous run leaves
    /// a stale inode behind and `bind` would fail with `EADDRINUSE`.
    pub async fn start(socket: PathBuf, allow: NetworkPermissions) -> std::io::Result<Arc<Self>> {
        if Path::new(&socket).exists() {
            std::fs::remove_file(&socket)?;
        }
        if let Some(parent) = socket.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let listener = UnixListener::bind(&socket)?;
        let socket_for_task = socket.clone();
        let rules_for_log = if allow.is_unrestricted() {
            "unrestricted".to_string()
        } else {
            allow.allow.len().to_string()
        };
        let allow = Arc::new(allow);

        let handle = tokio::spawn(async move {
            loop {
                let (client, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(e) => {
                        warn!("sandbox proxy: accept failed: {e}");
                        // A transient accept error must not kill the proxy.
                        continue;
                    }
                };
                let allow = Arc::clone(&allow);
                tokio::spawn(async move {
                    if let Err(e) = handle_client(client, allow).await {
                        debug!("sandbox proxy: connection ended: {e}");
                    }
                });
            }
        });

        info!(
            "sandbox proxy listening on {} with {} destination rule(s)",
            socket_for_task.display(),
            rules_for_log
        );

        Ok(Arc::new(Self { socket, handle }))
    }

    /// The socket the sandbox must bind.
    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

impl Drop for AllowListProxy {
    fn drop(&mut self) {
        self.handle.abort();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// The destination the client asked for.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    /// `CONNECT host:port` — tunnel bytes through unchanged.
    Tunnel { host: String, port: u16 },
    /// `GET http://host/path HTTP/1.1` — absolute-form plain HTTP.
    Plain { host: String, port: u16 },
}

impl Target {
    fn host(&self) -> &str {
        match self {
            Target::Tunnel { host, .. } | Target::Plain { host, .. } => host,
        }
    }

    fn port(&self) -> u16 {
        match self {
            Target::Tunnel { port, .. } | Target::Plain { port, .. } => *port,
        }
    }
}

/// Parse the destination out of a request head.
///
/// Returns `None` when the head is not a proxy request this proxy can police —
/// the caller answers `400` rather than guessing.
fn parse_target(head: &str) -> Option<Target> {
    let first_line = head.lines().next()?.trim();
    let mut parts = first_line.split_whitespace();
    let method = parts.next()?;
    let request_target = parts.next()?;

    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = split_host_port(request_target, 443)?;
        return Some(Target::Tunnel { host, port });
    }

    // Absolute-form: the request target carries the scheme and authority.
    let rest = request_target
        .strip_prefix("http://")
        .or_else(|| request_target.strip_prefix("https://"))?;
    let authority = rest.split('/').next()?;
    let default_port = if request_target.starts_with("https://") {
        443
    } else {
        80
    };
    let (host, port) = split_host_port(authority, default_port)?;
    Some(Target::Plain { host, port })
}

/// Split `host:port`, tolerating bracketed IPv6 literals.
fn split_host_port(authority: &str, default_port: u16) -> Option<(String, u16)> {
    let authority = authority.trim();
    if authority.is_empty() {
        return None;
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = match tail.strip_prefix(':') {
            Some(raw) => raw.parse::<u16>().ok()?,
            None => default_port,
        };
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        // A bare IPv6 address with no brackets and no port is ambiguous; refuse
        // rather than silently connecting somewhere unintended.
        Some((host, port)) if !host.contains(':') => {
            let port = port.parse::<u16>().ok()?;
            Some((host.to_string(), port))
        }
        Some(_) => None,
        None => Some((authority.to_string(), default_port)),
    }
}

async fn handle_client(
    mut client: tokio::net::UnixStream,
    allow: Arc<NetworkPermissions>,
) -> std::io::Result<()> {
    let head = match read_head(&mut client).await? {
        Some(head) => head,
        None => return Ok(()),
    };

    let Some(target) = parse_target(&head) else {
        let _ = client
            .write_all(
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await;
        return Ok(());
    };

    if !allow.allows_host(target.host()) {
        let body = format!(
            "destination '{}' is not in this plugin's network allow-list",
            target.host()
        );
        let response = format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        warn!(
            "sandbox proxy refused host {} (not in {:?})",
            target.host(),
            allow.allow
        );
        let _ = client.write_all(response.as_bytes()).await;
        return Ok(());
    }

    let upstream_addr = match resolve(&target).await {
        Some(addr) => addr,
        None => {
            let _ = client
                .write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
            return Ok(());
        }
    };

    let mut upstream =
        match tokio::time::timeout(UPSTREAM_TIMEOUT, TcpStream::connect(upstream_addr)).await {
            Ok(Ok(stream)) => stream,
            _ => {
                let _ = client
                .write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
                return Ok(());
            }
        };

    match target {
        Target::Tunnel { .. } => {
            client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?;
        }
        Target::Plain { .. } => {
            // Replay the head to the origin, rewritten to origin-form so the
            // origin server sees a normal request rather than a proxy one.
            let rewritten = rewrite_to_origin_form(&head);
            upstream.write_all(rewritten.as_bytes()).await?;
        }
    }

    // Bidirectional copy until either side closes.
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
    Ok(())
}

/// Read up to the end of the request head (`\r\n\r\n`).
async fn read_head(client: &mut tokio::net::UnixStream) -> std::io::Result<Option<String>> {
    let mut buf = Vec::with_capacity(1024);
    let read = tokio::time::timeout(HEAD_TIMEOUT, async {
        loop {
            let mut chunk = [0u8; 1024];
            let n = client.read(&mut chunk).await?;
            if n == 0 {
                return Ok::<usize, std::io::Error>(buf.len());
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() >= MAX_HEAD_BYTES {
                return Ok(buf.len());
            }
        }
    })
    .await;

    match read {
        Ok(Ok(0)) => Ok(None),
        Ok(Ok(_)) => Ok(Some(String::from_utf8_lossy(&buf).to_string())),
        Ok(Err(e)) => Err(e),
        Err(_) => Ok(None),
    }
}

/// `GET http://host/path HTTP/1.1` -> `GET /path HTTP/1.1`.
fn rewrite_to_origin_form(head: &str) -> String {
    let mut out = String::with_capacity(head.len());
    let mut lines = head.split("\r\n");
    if let Some(first) = lines.next() {
        let mut parts = first.split_whitespace();
        if let (Some(method), Some(target), Some(version)) =
            (parts.next(), parts.next(), parts.next())
        {
            let path = target
                .strip_prefix("http://")
                .or_else(|| target.strip_prefix("https://"))
                .and_then(|rest| rest.find('/').map(|i| &rest[i..]))
                .unwrap_or("/");
            out.push_str(&format!("{method} {path} {version}\r\n"));
        } else {
            out.push_str(first);
            out.push_str("\r\n");
        }
    }
    for line in lines {
        out.push_str(line);
        out.push_str("\r\n");
    }
    out
}

/// Resolve a target to a socket address, honouring the port from the request.
async fn resolve(target: &Target) -> Option<SocketAddr> {
    let host = target.host();
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Some(SocketAddr::new(ip, target.port()));
    }
    // `lookup_host` needs the port embedded.
    let authority = format!("{host}:{}", target.port());
    tokio::net::lookup_host(authority)
        .await
        .ok()
        .and_then(|mut addrs| addrs.next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connect_tunnels() {
        let head = "CONNECT api.example.com:443 HTTP/1.1\r\nHost: api.example.com:443\r\n\r\n";
        assert_eq!(
            parse_target(head),
            Some(Target::Tunnel {
                host: "api.example.com".into(),
                port: 443
            })
        );
    }

    #[test]
    fn parses_plain_absolute_form() {
        let head = "GET http://api.example.com/v1/x HTTP/1.1\r\nHost: api.example.com\r\n\r\n";
        assert_eq!(
            parse_target(head),
            Some(Target::Plain {
                host: "api.example.com".into(),
                port: 80
            })
        );
        let https = "GET https://api.example.com/v1/x HTTP/1.1\r\n\r\n";
        assert_eq!(
            parse_target(https),
            Some(Target::Plain {
                host: "api.example.com".into(),
                port: 443
            })
        );
    }

    #[test]
    fn parses_bracketed_ipv6() {
        assert_eq!(
            parse_target("CONNECT [2001:db8::1]:8443 HTTP/1.1\r\n\r\n"),
            Some(Target::Tunnel {
                host: "2001:db8::1".into(),
                port: 8443
            })
        );
    }

    #[test]
    fn rejects_junk_instead_of_guessing() {
        assert_eq!(parse_target(""), None);
        assert_eq!(parse_target("GARBAGE\r\n\r\n"), None);
        assert_eq!(
            parse_target("CONNECT no-port HTTP/1.1\r\n\r\n"),
            Some(Target::Tunnel {
                host: "no-port".into(),
                port: 443
            })
        );
        // Unbracketed IPv6 with a port is ambiguous.
        assert_eq!(
            parse_target("CONNECT 2001:db8::1:443 HTTP/1.1\r\n\r\n"),
            None
        );
    }

    #[test]
    fn rewrites_absolute_form_to_origin_form() {
        let head = "GET http://api.example.com/v1/x?a=1 HTTP/1.1\r\nHost: api.example.com\r\n\r\n";
        let out = rewrite_to_origin_form(head);
        assert!(out.starts_with("GET /v1/x?a=1 HTTP/1.1\r\n"));
        assert!(out.contains("Host: api.example.com"));
        // A target with no path still gets "/".
        assert!(
            rewrite_to_origin_form("GET http://h HTTP/1.1\r\n\r\n").starts_with("GET / HTTP/1.1")
        );
    }

    #[test]
    fn allow_list_gates_every_destination() {
        let allow = NetworkPermissions {
            allow: vec!["api.example.com".into(), "10.0.0.0/8".into()],
        };
        assert!(allow.allows_host("api.example.com"));
        assert!(!allow.allows_host("evil.example.com"));
        assert!(!allow.allows_host("169.254.169.254"));
        assert!(allow.allows_host("10.5.5.5"));

        let denied = NetworkPermissions::default();
        assert!(!denied.allows_host("api.example.com"));
    }
}
