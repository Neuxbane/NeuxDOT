use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;

use crate::state::AppState;

#[derive(Deserialize)]
pub struct WsQuery {
    pub agent_id: Option<String>,
}

pub async fn ws_events_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, query.agent_id, state))
}

async fn handle_ws(socket: WebSocket, filter_agent_id: Option<String>, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.event_tx.subscribe();

    let mut send_task = tokio::spawn(async move {
        while let Ok(envelope) = rx.recv().await {
            // If an agent filter was specified, only send matching events
            if let Some(ref target_id) = filter_agent_id {
                if &envelope.agent_id != target_id {
                    continue;
                }
            }

            if let Ok(json_str) = serde_json::to_string(&envelope) {
                if sender.send(Message::Text(json_str)).await.is_err() {
                    break;
                }
            }
        }
    });

    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Close(_) = msg {
                break;
            }
        }
    });

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };
}
