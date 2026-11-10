//! Process-lifetime ownership of MCP resource subscriptions.
//!
//! ## Why this is not in the executor
//!
//! `AgentExecutor::setup_mcp_sessions` runs *inside* `run_turn`, so a session
//! (and therefore any `resources/subscribe` it sent) only exists while a turn
//! is running. That is useless for the actual job: a push subscription has to
//! be listening precisely when the agent is **idle**, otherwise the message it
//! was supposed to catch arrives with nobody listening.
//!
//! The old per-turn approach also started a fresh child process on every turn.
//! For a bot API that allows one `getUpdates` consumer (Telegram answers `409`
//! to a second one), the superseded processes kept polling and fought the new
//! turn's poller.
//!
//! This hub owns exactly one long-lived session per agent/plugin pair that has
//! at least one subscription. It is reconciled:
//!   * at boot, for every agent that already has subscriptions,
//!   * whenever an agent's subscription list changes,
//!   * whenever an agent's plugin bindings change.
//!
//! The executor no longer sends `resources/subscribe`; the hub is the single
//! owner. That removes the duplicate-poller `409`s.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use neuxdot_core::db::repos::Database;
use neuxdot_core::events::{AgentEvent, EventEnvelope};
use neuxdot_core::mcp::client::{McpClientSession, McpNotification};
use neuxdot_core::mcp::http::HttpTransport;
use neuxdot_core::mcp::stdio::StdioTransport;
use neuxdot_core::runtime::executor::AgentExecutor;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// A plugin push that should start an agent turn instead of merely landing in
/// the context queue. Two shapes are recognised:
///   * `role == "user"` — the chat plugin's console stream.
///   * an inbound chat message carrying `from` + text — Telegram-style update
///     summaries (`type: "message"`, `text: "..."`).
fn is_inbound_user_message(payload: &Value) -> bool {
    if payload.get("role").and_then(Value::as_str) == Some("user") {
        return true;
    }
    let has_sender = payload.get("from").is_some();
    let has_text = payload
        .get("text")
        .or_else(|| payload.get("caption"))
        .and_then(Value::as_str)
        .is_some_and(|t| !t.trim().is_empty());
    has_sender && has_text
}

/// Build the prompt used to wake an idle agent.
fn turn_prompt(plugin_name: &str, payload: &Value) -> String {
    if let Some(text) = payload.get("text").and_then(Value::as_str) {
        return format!(
            "Incoming message from {}: \"{}\". Respond to the sender using the {} tools.",
            plugin_name, text, plugin_name
        );
    }
    if let Some(caption) = payload.get("caption").and_then(Value::as_str) {
        return format!(
            "Incoming media message from {} with caption \"{}\". Respond using the {} tools.",
            plugin_name, caption, plugin_name
        );
    }
    format!("Incoming event from {}.", plugin_name)
}

struct Worker {
    /// Fingerprint of the config the worker was built from. A reconcile whose
    /// fingerprint matches is a no-op, so repeated saves do not churn processes.
    fingerprint: String,
    shutdown: oneshot::Sender<()>,
    handle: JoinHandle<()>,
}

pub struct SubscriptionHub {
    db: Arc<Database>,
    event_tx: tokio::sync::broadcast::Sender<EventEnvelope>,
    workers: Mutex<HashMap<String, Worker>>,
}

impl SubscriptionHub {
    pub fn new(
        db: Arc<Database>,
        event_tx: tokio::sync::broadcast::Sender<EventEnvelope>,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            event_tx,
            workers: Mutex::new(HashMap::new()),
        })
    }

    /// Reconcile one agent. Cheap and idempotent; safe to call after any write
    /// that could change what should be subscribed.
    pub async fn sync_agent(self: &Arc<Self>, agent_id: &str) {
        let subscriptions = match self.db.get_agent_subscriptions(agent_id).await {
            Ok(s) => s,
            Err(e) => {
                warn!("subscription hub: could not load subscriptions for {agent_id}: {e}");
                return;
            }
        };
        let servers = match self.db.get_agent_mcp_servers(agent_id).await {
            Ok(s) => s,
            Err(e) => {
                warn!("subscription hub: could not load plugins for {agent_id}: {e}");
                return;
            }
        };

        // Only push-capable plugins can own a stream. Custom tool / curl plugins
        // have no transport that can emit notifications.
        // Only push-capable plugins can own a stream. Custom tool / curl plugins
        // have no transport that can emit notifications.
        let mut streams: Vec<&neuxdot_core::db::schema::McpServer> = servers
            .iter()
            .filter(|s| s.is_active)
            .filter(|s| s.transport != "custom_tools" && s.transport != "curl")
            .collect();
        streams.sort_by(|a, b| a.id.cmp(&b.id));

        let fingerprint = worker_fingerprint(&streams, &subscriptions);
        let plugin_ids: Vec<String> = streams.iter().map(|s| s.id.clone()).collect();

        let mut workers = self.workers.lock().await;

        if subscriptions.is_empty() || plugin_ids.is_empty() {
            if let Some(worker) = workers.remove(agent_id) {
                let _ = worker.shutdown.send(());
                worker.handle.abort();
                info!("subscription hub: stopped stream for agent {agent_id}");
            }
            return;
        }

        if workers
            .get(agent_id)
            .is_some_and(|w| w.fingerprint == fingerprint && !w.handle.is_finished())
        {
            return;
        }

        if let Some(worker) = workers.remove(agent_id) {
            let _ = worker.shutdown.send(());
            worker.handle.abort();
        }

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let hub = Arc::clone(self);
        let owned_agent_id = agent_id.to_string();
        let handle = tokio::spawn(async move {
            run_worker(hub, owned_agent_id, servers, subscriptions, shutdown_rx).await;
        });

        workers.insert(
            agent_id.to_string(),
            Worker {
                fingerprint,
                shutdown: shutdown_tx,
                handle,
            },
        );
        info!("subscription hub: streaming for agent {agent_id}");
    }

    /// Boot-time reconcile: every agent that already has subscriptions should be
    /// streaming before the first user turn, not after it.
    pub async fn sync_all(self: &Arc<Self>) {
        let accounts = match self.db.list_accounts().await {
            Ok(a) => a,
            Err(e) => {
                warn!("subscription hub: cannot list accounts: {e}");
                return;
            }
        };
        for account in accounts {
            let agents = match self.db.list_agents(&account.id).await {
                Ok(a) => a,
                Err(e) => {
                    warn!(
                        "subscription hub: cannot list agents for {}: {e}",
                        account.id
                    );
                    continue;
                }
            };
            for agent in agents {
                self.sync_agent(&agent.id).await;
            }
        }
    }

    /// Stop all workers. Used on shutdown and by tests.
    pub async fn shutdown_all(&self) {
        let mut workers = self.workers.lock().await;
        for (_, worker) in workers.drain() {
            let _ = worker.shutdown.send(());
            worker.handle.abort();
        }
    }
}

/// Identity of the work a worker performs.
///
/// Must cover everything that changes what the spawned process *is* — the
/// command, env, transport, and the resolved grants. A fingerprint made only of
/// plugin ids (as this used to be) meant a permission edit left the old process
/// running under the old policy: granting a network allow-list had no effect
/// until the whole server restarted.
fn worker_fingerprint(
    streams: &[&neuxdot_core::db::schema::McpServer],
    subscriptions: &[String],
) -> String {
    let mut fingerprint = String::new();
    for s in streams {
        let perms = s.get_permissions();
        fingerprint.push_str(&format!(
            "{}|{}|{}|{}|{:?}|{}|{}#",
            s.id,
            s.transport,
            s.url_or_command,
            s.headers_or_env.as_deref().unwrap_or(""),
            perms.network.allow,
            perms.elevated,
            perms.process,
        ));
    }
    fingerprint.push('|');
    fingerprint.push_str(&subscriptions.join(","));
    fingerprint
}

async fn run_worker(
    hub: Arc<SubscriptionHub>,
    agent_id: String,
    servers: Vec<neuxdot_core::db::schema::McpServer>,
    subscriptions: Vec<String>,
    mut shutdown: oneshot::Receiver<()>,
) {
    // (plugin id, plugin name, notification) so the handler can attribute a
    // push to the plugin that actually emitted it.
    let (forward_tx, mut forward_rx) =
        mpsc::unbounded_channel::<(String, String, McpNotification)>();
    let mut keep_alive: Vec<Arc<McpClientSession>> = Vec::new();

    for server in &servers {
        if !server.is_active || server.transport == "custom_tools" || server.transport == "curl" {
            continue;
        }
        let (notif_tx, notif_rx) = mpsc::unbounded_channel::<McpNotification>();

        let transport: Arc<dyn neuxdot_core::mcp::client::McpTransport> =
            if server.transport == "stdio" || server.transport == "binary" {
                let launch = neuxdot_core::mcp::sandbox::SandboxSpec::launch_for(server);
                match StdioTransport::spawn_with_sandbox(
                    &server.url_or_command,
                    server.headers_or_env.as_deref(),
                    Some(agent_id.as_str()),
                    notif_tx,
                    &launch,
                )
                .await
                {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(
                        "subscription hub: failed to start plugin '{}' for agent {agent_id}: {e}",
                        server.name
                    );
                        continue;
                    }
                }
            } else {
                HttpTransport::new(
                    &server.url_or_command,
                    server.headers_or_env.as_deref(),
                    notif_tx,
                )
            };

        let session = Arc::new(McpClientSession {
            server_id: server.id.clone(),
            server_name: server.name.clone(),
            transport,
            notification_rx: Mutex::new(notif_rx),
        });

        if let Err(e) = session.initialize().await {
            warn!(
                "subscription hub: handshake failed for '{}' (agent {agent_id}): {e}",
                server.name
            );
        }

        // Ask the plugin which resources it actually exposes. Subscribing to a
        // URI a plugin does not own would just earn a `resource_not_found`.
        let available: HashSet<String> = session
            .transport
            .send_request("resources/list", None)
            .await
            .ok()
            .and_then(|v| {
                v.get("resources").and_then(Value::as_array).map(|list| {
                    list.iter()
                        .filter_map(|r| r.get("uri").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
            })
            .unwrap_or_default();

        for uri in &subscriptions {
            if available.contains(uri) {
                if let Err(e) = session
                    .transport
                    .send_request("resources/subscribe", Some(json!({ "uri": uri })))
                    .await
                {
                    warn!(
                        "subscription hub: subscribe to {uri} on '{}' failed: {e}",
                        server.name
                    );
                }
            }
        }

        // Re-fan every notification from this session into one channel so the
        // worker can handle them in a single place.
        let tx = forward_tx.clone();
        let worker = Arc::clone(&session);
        let plugin_id = server.id.clone();
        let plugin_name = server.name.clone();
        tokio::spawn(async move {
            // Hold the receiver for the lifetime of the session.
            let mut rx = worker.notification_rx.lock().await;
            while let Some(notif) = rx.recv().await {
                if tx
                    .send((plugin_id.clone(), plugin_name.clone(), notif))
                    .is_err()
                {
                    break;
                }
            }
        });

        keep_alive.push(session);
    }

    if keep_alive.is_empty() {
        warn!("subscription hub: no usable stream plugin for agent {agent_id}");
        return;
    }
    drop(forward_tx);

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("subscription hub: shutting down stream for agent {agent_id}");
                break;
            }
            received = forward_rx.recv() => {
                let Some((plugin_id, plugin_name, notif)) = received else { break };
                handle_notification(&hub, &agent_id, &plugin_id, &plugin_name, notif).await;
            }
        }
    }

    for session in &keep_alive {
        let _ = session.transport.close().await;
    }
}

async fn handle_notification(
    hub: &Arc<SubscriptionHub>,
    agent_id: &str,
    plugin_id: &str,
    plugin_name: &str,
    notif: McpNotification,
) {
    let params_str = notif.params.as_ref().map(|p| p.to_string());
    let plugin_name = plugin_name.to_string();

    // Persist first: the turn that consumes this may start immediately, and it
    // reads from `agent_pushed_messages`.
    let recorded = hub
        .db
        .save_pushed_message(
            agent_id,
            Some(plugin_id),
            &notif.method,
            params_str.as_deref(),
        )
        .await;
    if let Err(e) = recorded {
        warn!("subscription hub: failed to persist push for {agent_id}: {e}");
    }

    let payload = notif.params.clone().unwrap_or(Value::Null);
    let _ = hub.event_tx.send(EventEnvelope::new(
        agent_id.to_string(),
        AgentEvent::McpPushedNotification {
            agent_id: agent_id.to_string(),
            mcp_id: plugin_id.to_string(),
            mcp_name: plugin_name.clone(),
            method: notif.method.clone(),
            payload: payload.clone(),
        },
    ));

    if notif.method != "notifications/resources/updated" || !is_inbound_user_message(&payload) {
        return;
    }

    // Wake the agent. Without this a pushed message just sits in the queue until
    // some *future* turn happens to start, which is indistinguishable from the
    // feature being broken.
    let prompt = turn_prompt(&plugin_name, &payload);
    let db = Arc::clone(&hub.db);
    let event_tx = hub.event_tx.clone();
    let agent_id = agent_id.to_string();
    tokio::spawn(async move {
        let agent = match db.get_agent(&agent_id).await {
            Ok(Some(a)) => a,
            _ => return,
        };
        let models = db.get_agent_models(&agent.id).await.unwrap_or_default();
        let mcp_servers = db
            .get_agent_mcp_servers(&agent.id)
            .await
            .unwrap_or_default();
        let executor = AgentExecutor::new(Arc::clone(&db), agent, models, mcp_servers, event_tx);
        if let Err(e) = executor.run_turn(&prompt).await {
            warn!("subscription hub: agent turn for {agent_id} failed: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(perms: serde_json::Value) -> neuxdot_core::db::schema::McpServer {
        neuxdot_core::db::schema::McpServer {
            id: "p1".into(),
            account_id: "a1".into(),
            name: "Telegram".into(),
            alias: Some("telegram".into()),
            transport: "binary".into(),
            url_or_command: "./release/plugins/telegram".into(),
            headers_or_env: None,
            version: "1".into(),
            description: None,
            wasm_hash: None,
            wasm_binary: None,
            manifest_json: Some(json!({ "permissions": perms }).to_string()),
            permissions_json: None,
            is_active: true,
            storage_bytes: 0,
            created_at: None,
        }
    }

    fn subs() -> Vec<String> {
        vec!["extension://telegram/updates".to_string()]
    }

    /// Regression: the fingerprint used to be `plugin_ids|subscriptions`, so
    /// granting network did not change it and the already-running worker kept
    /// the old no-network sandbox. The Telegram poller then failed forever with
    /// a bare reqwest error despite the grant being saved.
    #[test]
    fn fingerprint_changes_when_network_is_granted() {
        let denied = server(json!({ "network": { "allow": [] } }));
        let allowed = server(json!({ "network": { "allow": ["api.telegram.org"] } }));
        assert_ne!(
            worker_fingerprint(&[&denied], &subs()),
            worker_fingerprint(&[&allowed], &subs()),
            "a network grant must restart the worker"
        );
    }

    #[test]
    fn fingerprint_changes_on_command_env_and_elevated() {
        let base = server(json!({}));
        let mut cmd = server(json!({}));
        cmd.url_or_command = "./release/plugins/telegram2".into();
        let mut env = server(json!({}));
        env.headers_or_env = Some("TELEGRAM_API_BASE=https://x".into());
        let elevated = server(json!({ "elevated": true }));

        let f = worker_fingerprint(&[&base], &subs());
        assert_ne!(f, worker_fingerprint(&[&cmd], &subs()));
        assert_ne!(f, worker_fingerprint(&[&env], &subs()));
        assert_ne!(f, worker_fingerprint(&[&elevated], &subs()));
    }

    #[test]
    fn fingerprint_is_stable_for_the_same_config() {
        let a = server(json!({ "network": { "allow": ["api.telegram.org"] } }));
        let b = server(json!({ "network": { "allow": ["api.telegram.org"] } }));
        assert_eq!(
            worker_fingerprint(&[&a], &subs()),
            worker_fingerprint(&[&b], &subs()),
            "an unchanged config must not churn the process"
        );
    }


    #[test]
    fn chat_console_user_events_wake_the_agent() {
        let payload = json!({
            "uri": "extension://chat/messages",
            "role": "user",
            "text": "hi there"
        });
        assert!(is_inbound_user_message(&payload));
    }

    #[test]
    fn inbound_telegram_message_wakes_the_agent() {
        // Shape emitted by built-in-plugin/telegram's summarize_update.
        let payload = json!({
            "uri": "extension://telegram/updates",
            "type": "message",
            "from": { "id": 8_535_924_912i64, "username": "neuxbanee" },
            "chat": { "id": 8_535_924_912i64, "type": "private" },
            "text": "Hello"
        });
        assert!(is_inbound_user_message(&payload));
    }

    #[test]
    fn caption_only_telegram_message_wakes_the_agent() {
        let payload = json!({
            "type": "message",
            "from": { "id": 1 },
            "caption": "look at this"
        });
        assert!(is_inbound_user_message(&payload));
    }

    #[test]
    fn outbound_echo_and_empty_events_do_not_wake_the_agent() {
        // Assistant-authored chat echo.
        assert!(!is_inbound_user_message(&json!({
            "role": "assistant",
            "text": "sent"
        })));
        // An update summary with no usable text.
        assert!(!is_inbound_user_message(&json!({
            "type": "message",
            "from": { "id": 1 },
            "text": "   "
        })));
        // A non-message update type.
        assert!(!is_inbound_user_message(&json!({
            "type": "other",
            "update_id": 5
        })));
    }

    #[test]
    fn prompt_quotes_the_incoming_text() {
        let prompt = turn_prompt("Telegram", &json!({ "text": "Hello" }));
        assert!(prompt.contains("Hello"));
        assert!(prompt.contains("Telegram"));
    }
}
