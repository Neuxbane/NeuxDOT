use neuxdot_core::db::repos::Database;
use neuxdot_core::events::EventEnvelope;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub event_tx: broadcast::Sender<EventEnvelope>,
}
