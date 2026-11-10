//! Discord Gateway v10 WebSocket client.
//!
//! The Telegram plugin long-polls; Discord pushes events over a persistent
//! socket. This module owns it: connect, HELLO, heartbeat, IDENTIFY or RESUME,
//! then forward every dispatch to a channel. Session id and sequence survive
//! reconnects so a resume can replay whatever was missed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio::time::MissedTickBehavior;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// Public Gateway endpoint. `encoding=json` is required: only text frames are
/// parsed.
pub const DEFAULT_GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";

/// GUILDS | GUILD_MESSAGES | DIRECT_MESSAGES | MESSAGE_CONTENT (= 37377).
pub const INTENTS: u64 = 1 | (1 << 9) | (1 << 12) | (1 << 15);

const APP: &str = "neuxdot-discord-mcp";
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const HELLO_TIMEOUT: Duration = Duration::from_secs(30);

type GatewaySocket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type GatewaySink = SplitSink<GatewaySocket, Message>;
type GatewaySource = SplitStream<GatewaySocket>;

/// One dispatched Gateway event (op 0).
#[derive(Debug, Clone)]
pub struct GatewayEvent {
    /// Event name, for example `MESSAGE_CREATE`, `READY`, or `RESUMED`.
    pub event: String,
    /// The event's `d` object.
    pub data: Value,
}

/// The op-1 heartbeat, carrying the last sequence number seen (or null).
pub fn heartbeat_payload(seq: Option<i64>) -> Value {
    json!({ "op": 1, "d": seq })
}

/// The op-2 IDENTIFY, requesting exactly [`INTENTS`].
pub fn identify_payload(token: &str) -> Value {
    json!({
        "op": 2,
        "d": {
            "token": token,
            "intents": INTENTS,
            "properties": { "os": std::env::consts::OS, "browser": APP, "device": APP }
        }
    })
}

/// The op-6 RESUME, replaying a session from the last sequence number.
pub fn resume_payload(token: &str, session_id: &str, seq: Option<i64>) -> Value {
    json!({ "op": 6, "d": { "token": token, "session_id": session_id, "seq": seq } })
}

/// `d.heartbeat_interval` of an op-10 HELLO, in milliseconds.
pub fn hello_interval(payload: &Value) -> Option<u64> {
    if op_code(payload) != Some(10) {
        return None;
    }
    payload
        .get("d")
        .and_then(|d| d.get("heartbeat_interval"))
        .and_then(Value::as_u64)
}

/// True for an op-11 HEARTBEAT_ACK.
pub fn heartbeat_ack(payload: &Value) -> bool {
    op_code(payload) == Some(11)
}

/// `(t, d)` of an op-0 dispatch, or `None` for any other frame.
pub fn dispatch_event(payload: &Value) -> Option<(String, Value)> {
    if op_code(payload) != Some(0) {
        return None;
    }
    let event = payload.get("t")?.as_str()?.to_string();
    let data = payload.get("d")?.clone();
    Some((event, data))
}

/// The `op` field as a number.
pub fn op_code(payload: &Value) -> Option<u64> {
    payload.get("op").and_then(Value::as_u64)
}

/// `d.session_id`, present on READY and RESUMED.
pub fn session_id_of(payload: &Value) -> Option<String> {
    payload
        .get("d")
        .and_then(|d| d.get("session_id"))
        .and_then(Value::as_str)
        .map(String::from)
}

/// The `s` field, the sequence number used by heartbeats and resumes.
pub fn sequence_of(payload: &Value) -> Option<i64> {
    payload.get("s").and_then(Value::as_i64)
}

/// Close codes meaning "do not reconnect": the credentials or shard are wrong.
pub fn is_fatal_close(code: u16) -> bool {
    matches!(code, 4004 | 4010 | 4011 | 4012 | 4013 | 4014)
}

/// What one connection attempt means for the outer loop.
enum Outcome {
    /// The subscriber dropped `tx`.
    Stopped,
    /// A close code that must not be retried.
    Fatal,
    /// The connection ended and may be retried.
    Disconnected,
    /// READY or RESUMED was seen, so the backoff restarts at 1s.
    Healthy,
}

/// A frame read from the socket.
enum Inbound {
    Payload(Value),
    /// The connection is over; `Outcome` says whether it may be retried.
    End(Outcome),
}

/// Session state that survives a reconnect.
struct Session {
    id: String,
    seq: Option<i64>,
}

/// Connect, authenticate, and forward events until the connection ends.
/// Reconnects with exponential backoff (1s doubling to 60s), resuming the
/// session when one survives, and returns when `tx` is closed or the close
/// code is fatal.
pub async fn run_gateway(url: String, token: String, tx: mpsc::UnboundedSender<GatewayEvent>) {
    // An unset URL falls back to the public endpoint rather than failing.
    let url = if url.trim().is_empty() {
        DEFAULT_GATEWAY_URL.to_string()
    } else {
        url
    };
    let mut backoff = INITIAL_BACKOFF;
    let mut session: Option<Session> = None;

    loop {
        if tx.is_closed() {
            return;
        }
        match connect_once(&url, &token, &tx, &mut session).await {
            Outcome::Stopped | Outcome::Fatal => return,
            Outcome::Healthy => backoff = INITIAL_BACKOFF,
            Outcome::Disconnected => {}
        }
        // The channel may have closed while the attempt was ending.
        if tx.is_closed() {
            return;
        }
        tracing::warn!("discord gateway reconnecting in {:?}", backoff);
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// One connection attempt. `session` is read for RESUME and updated in place.
async fn connect_once(
    url: &str,
    token: &str,
    tx: &mpsc::UnboundedSender<GatewayEvent>,
    session: &mut Option<Session>,
) -> Outcome {
    let socket = match tokio_tungstenite::connect_async(url).await {
        Ok((socket, _response)) => socket,
        Err(e) => {
            tracing::warn!("discord gateway connect failed: {e}");
            return Outcome::Disconnected;
        }
    };

    let (mut sink, mut stream) = socket.split();

    // HELLO must arrive first; a silent or malformed handshake is a failure.
    let hello = match tokio::time::timeout(HELLO_TIMEOUT, read_inbound(&mut stream, tx)).await {
        Ok(Inbound::Payload(value)) => value,
        Ok(Inbound::End(outcome)) => return outcome,
        Err(_) => {
            tracing::warn!("discord gateway did not send HELLO");
            return Outcome::Disconnected;
        }
    };
    let Some(interval_ms) = hello_interval(&hello) else {
        tracing::warn!("discord gateway sent an unexpected first frame");
        return Outcome::Disconnected;
    };

    // IDENTIFY fresh, or RESUME the surviving session.
    let mut last_seq = session.as_ref().and_then(|state| state.seq);
    let auth = match session.as_ref() {
        Some(state) => resume_payload(token, &state.id, state.seq),
        None => identify_payload(token),
    };
    if !send_json(&mut sink, &auth).await {
        return Outcome::Disconnected;
    }

    let acked = Arc::new(AtomicBool::new(true));
    let (force_tx, mut force_rx) = watch::channel(false);
    let (seq_tx, seq_rx) = watch::channel(last_seq);
    let heartbeat = tokio::spawn(heartbeat_task(
        sink,
        interval_ms,
        Arc::clone(&acked),
        force_tx,
        seq_rx,
    ));

    // Ends on socket end, RECONNECT or INVALID_SESSION, a missed ACK, or a
    // closed subscriber channel.
    let mut healthy = false;
    let mut ended = Outcome::Disconnected;
    loop {
        let inbound = tokio::select! {
            biased;
            _ = force_rx.changed() => {
                tracing::warn!("discord gateway heartbeat stopped ACKing");
                break;
            }
            inbound = read_inbound(&mut stream, tx) => inbound,
        };

        let payload = match inbound {
            Inbound::Payload(payload) => payload,
            Inbound::End(outcome) => {
                ended = outcome;
                break;
            }
        };

        if let Some(seq) = sequence_of(&payload) {
            last_seq = Some(seq);
            seq_tx.send_replace(last_seq);
            if let Some(state) = session.as_mut() {
                state.seq = last_seq;
            }
        }

        match op_code(&payload) {
            Some(0) => {
                let Some((event, data)) = dispatch_event(&payload) else {
                    tracing::warn!("discord gateway dispatch carried no event name");
                    continue;
                };
                // READY opens a session; RESUMED confirms the resumed one.
                if matches!(event.as_str(), "READY" | "RESUMED") {
                    if let Some(id) = session_id_of(&payload) {
                        *session = Some(Session { id, seq: last_seq });
                    }
                    healthy = true;
                    tracing::info!(event, "discord gateway session established");
                }
                if tx.send(GatewayEvent { event, data }).is_err() {
                    ended = Outcome::Stopped;
                    break;
                }
            }
            Some(7) => {
                tracing::warn!("discord gateway requested a reconnect");
                break;
            }
            Some(9) => {
                // `d: false` means the session is gone; only `true` may resume.
                let resumable = payload.get("d").and_then(Value::as_bool).unwrap_or(false);
                if !resumable {
                    *session = None;
                }
                tracing::warn!("discord gateway invalidated the session (resumable: {resumable})");
                break;
            }
            _ if heartbeat_ack(&payload) => acked.store(true, Ordering::SeqCst),
            _ => {}
        }
    }

    heartbeat.abort();
    if healthy { Outcome::Healthy } else { ended }
}

/// Read one JSON payload, skipping frames the client handles itself.
async fn read_inbound(
    stream: &mut GatewaySource,
    tx: &mpsc::UnboundedSender<GatewayEvent>,
) -> Inbound {
    loop {
        let message = tokio::select! {
            biased;
            _ = tx.closed() => return Inbound::End(Outcome::Stopped),
            message = stream.next() => message,
        };

        match message {
            Some(Ok(Message::Text(text))) => match serde_json::from_str::<Value>(text.as_str()) {
                Ok(value) => return Inbound::Payload(value),
                Err(e) => tracing::warn!("discord gateway sent invalid JSON: {e}"),
            },
            Some(Ok(Message::Close(frame))) => {
                if let Some(frame) = frame {
                    let code = u16::from(frame.code);
                    if is_fatal_close(code) {
                        tracing::warn!("discord gateway closed fatally: {code} ({})", frame.reason);
                        return Inbound::End(Outcome::Fatal);
                    }
                    tracing::warn!("discord gateway closed: {code} ({})", frame.reason);
                } else {
                    tracing::warn!("discord gateway closed without a code");
                }
                return Inbound::End(Outcome::Disconnected);
            }
            Some(Ok(Message::Binary(_))) => {
                tracing::debug!("discord gateway sent an unexpected binary frame");
            }
            // Ping, pong, and raw frames need no handling: tungstenite answers
            // pings itself and the Gateway protocol adds nothing above them.
            Some(Ok(_)) => {}
            Some(Err(e)) => {
                tracing::warn!("discord gateway read failed: {e}");
                return Inbound::End(Outcome::Disconnected);
            }
            None => return Inbound::End(Outcome::Disconnected),
        }
    }
}

/// Serialize and write one frame. A write failure ends the connection.
async fn send_json(sink: &mut GatewaySink, payload: &Value) -> bool {
    match sink.send(Message::text(payload.to_string())).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("discord gateway write failed: {e}");
            false
        }
    }
}

/// Send op-1 heartbeats on the interval Discord chose. The first tick is
/// consumed because `interval` fires immediately; a missed ACK replaces the
/// connection.
async fn heartbeat_task(
    mut sink: GatewaySink,
    interval_ms: u64,
    acked: Arc<AtomicBool>,
    force_tx: watch::Sender<bool>,
    seq_rx: watch::Receiver<Option<i64>>,
) {
    let mut ticker = tokio::time::interval(Duration::from_millis(interval_ms.max(1)));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker.tick().await;

    loop {
        ticker.tick().await;
        if !acked.swap(false, Ordering::SeqCst) {
            tracing::warn!("discord gateway did not ACK the previous heartbeat");
            // The reader may already be gone; either way this connection dies.
            let _ = force_tx.send(true);
            return;
        }
        let seq = *seq_rx.borrow();
        if !send_json(&mut sink, &heartbeat_payload(seq)).await {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_carries_the_last_sequence_or_null() {
        let none = heartbeat_payload(None);
        assert_eq!(none["op"], 1);
        assert!(none["d"].is_null());

        let some = heartbeat_payload(Some(5));
        assert_eq!(some["op"], 1);
        assert_eq!(some["d"], 5);
    }

    #[test]
    fn identify_requests_the_configured_intents() {
        let payload = identify_payload("tok");
        assert_eq!(payload["op"], 2);
        assert_eq!(payload["d"]["token"], "tok");
        assert_eq!(payload["d"]["intents"], INTENTS);
        assert_eq!(INTENTS, 37377);
        assert!(payload["d"]["properties"].is_object());
    }

    #[test]
    fn resume_carries_session_and_sequence() {
        let payload = resume_payload("tok", "sess-1", Some(42));
        assert_eq!(payload["op"], 6);
        assert_eq!(payload["d"]["token"], "tok");
        assert_eq!(payload["d"]["session_id"], "sess-1");
        assert_eq!(payload["d"]["seq"], 42);
    }

    #[test]
    fn hello_interval_comes_only_from_hello() {
        let hello = json!({ "op": 10, "d": { "heartbeat_interval": 41250 } });
        assert_eq!(hello_interval(&hello), Some(41250));
        assert_eq!(hello_interval(&json!({ "op": 10 })), None);
        assert_eq!(hello_interval(&json!({ "op": 10, "d": {} })), None);
        assert_eq!(hello_interval(&json!({ "op": 0, "d": {} })), None);
    }

    #[test]
    fn dispatch_needs_op_zero_name_and_data() {
        let payload = json!({ "op": 0, "t": "MESSAGE_CREATE", "s": 7, "d": { "id": "1" } });
        let (event, data) = dispatch_event(&payload).unwrap();
        assert_eq!(event, "MESSAGE_CREATE");
        assert_eq!(data["id"], "1");
        assert_eq!(sequence_of(&payload), Some(7));

        assert!(dispatch_event(&json!({ "op": 11 })).is_none());
        assert!(dispatch_event(&json!({ "op": 0, "d": {} })).is_none());
        assert!(dispatch_event(&json!({ "op": 0, "t": "READY" })).is_none());
    }

    #[test]
    fn op_code_ack_and_session_fields_parse() {
        assert_eq!(op_code(&json!({ "op": 11 })), Some(11));
        assert!(op_code(&json!({ "op": "x" })).is_none());
        assert!(heartbeat_ack(&json!({ "op": 11 })));
        assert!(!heartbeat_ack(&json!({ "op": 10 })));

        let ready = json!({ "op": 0, "t": "READY", "s": 3, "d": { "session_id": "abc" } });
        assert_eq!(session_id_of(&ready).as_deref(), Some("abc"));
        assert_eq!(session_id_of(&json!({ "op": 11 })), None);
        assert_eq!(sequence_of(&json!({ "op": 11 })), None);
    }

    #[test]
    fn only_authentication_and_configuration_closes_are_fatal() {
        for code in [4004, 4010, 4011, 4012, 4013, 4014] {
            assert!(is_fatal_close(code), "{code} should be fatal");
        }
        for code in [1000, 1001, 1006, 4000, 4001, 4002, 4003, 4005, 4007, 4009] {
            assert!(!is_fatal_close(code), "{code} should be retried");
        }
    }
}
