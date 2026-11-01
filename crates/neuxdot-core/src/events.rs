use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum AgentEvent {
    #[serde(rename = "provider_selected")]
    ProviderSelected {
        agent_id: String,
        provider_id: String,
        provider_name: String,
        model: String,
        strategy: String,
    },

    #[serde(rename = "tool_search")]
    ToolSearch {
        agent_id: String,
        query: String,
        results_count: usize,
    },

    #[serde(rename = "tool_schema_fetched")]
    ToolSchemaFetched {
        agent_id: String,
        tool_name: String,
    },

    #[serde(rename = "tool_call_started")]
    ToolCallStarted {
        agent_id: String,
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    },

    #[serde(rename = "tool_call_completed")]
    ToolCallCompleted {
        agent_id: String,
        call_id: String,
        tool_name: String,
        result: serde_json::Value,
        success: bool,
    },

    #[serde(rename = "mcp_pushed_notification")]
    McpPushedNotification {
        agent_id: String,
        mcp_id: String,
        mcp_name: String,
        method: String,
        payload: serde_json::Value,
    },

    #[serde(rename = "turn_started")]
    TurnStarted {
        agent_id: String,
        turn_id: String,
    },

    #[serde(rename = "turn_chunk")]
    TurnChunk {
        agent_id: String,
        delta: String,
    },

    #[serde(rename = "turn_completed")]
    TurnCompleted {
        agent_id: String,
        turn_id: String,
        content: String,
    },

    #[serde(rename = "memory_updated")]
    MemoryUpdated {
        agent_id: String,
        updated_hash: String,
        char_count: usize,
    },

    #[serde(rename = "memory_compacted")]
    MemoryCompacted {
        agent_id: String,
        current_hash: String,
    },

    #[serde(rename = "memory_read")]
    MemoryRead {
        agent_id: String,
        current_hash: String,
        char_count: usize,
    },

    #[serde(rename = "agent_sleeping")]
    AgentSleeping {
        agent_id: String,
        duration: String,
        interruptible: bool,
    },

    #[serde(rename = "agent_woke_up")]
    AgentWokeUp {
        agent_id: String,
        reason: String,
    },

    #[serde(rename = "text_disallowed")]
    TextDisallowed {
        agent_id: String,
        error_message: String,
    },

    #[serde(rename = "error")]
    AgentError {
        agent_id: String,
        message: String,
    },

    #[serde(rename = "info")]
    AgentInfo {
        agent_id: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub agent_id: String,
    pub event: AgentEvent,
}

impl EventEnvelope {
    pub fn new(agent_id: impl Into<String>, event: AgentEvent) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            agent_id: agent_id.into(),
            event,
        }
    }
}
