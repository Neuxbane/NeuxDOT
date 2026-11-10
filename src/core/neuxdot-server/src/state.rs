use neuxdot_core::db::repos::Database;
use neuxdot_core::events::EventEnvelope;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::subscriptions::SubscriptionHub;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub event_tx: broadcast::Sender<EventEnvelope>,
    /// Process-lifetime MCP push subscriptions. See `crate::subscriptions`.
    pub subscriptions: Arc<SubscriptionHub>,
}
