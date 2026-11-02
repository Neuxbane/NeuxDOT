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
        /// Transcript row id holding the full (untruncated) result, so the UI
        /// can fetch more via history_inspect.
        #[serde(skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
        /// True when `result` was head/body/tail truncated for the event stream.
        #[serde(default)]
        truncated: bool,
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
    /// Human-readable header shared with tool responses:
    /// `2026-11-02T07:49:33.124Z ctx:30%`.
    #[serde(default)]
    pub context: String,
}

impl EventEnvelope {
    pub fn new(agent_id: impl Into<String>, event: AgentEvent) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            agent_id: agent_id.into(),
            event,
            context: String::new(),
        }
    }

    /// Attaches the `timestamp ctx:N%` header used by tool responses.
    pub fn with_context(mut self, current_chars: usize, max_chars: usize) -> Self {
        self.context = crate::runtime::meta_tools::context_header(current_chars, max_chars);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_carries_real_context_pressure() {
        // Mirrors what a spawned MCP notification task reads from the shared
        // gauge: a real percentage, not the old hard-coded 0%.
        let live = std::sync::atomic::AtomicUsize::new(90_000);
        let current = live.load(std::sync::atomic::Ordering::Relaxed);

        let envelope = EventEnvelope::new(
            "agent-1",
            AgentEvent::McpPushedNotification {
                agent_id: "agent-1".into(),
                mcp_id: "mcp-1".into(),
                mcp_name: "Computer".into(),
                method: "notifications/resources/updated".into(),
                payload: serde_json::json!({ "uri": "terminal://completed" }),
            },
        )
        .with_context(current, 300_000);

        assert!(envelope.context.contains("ctx:30%"), "got {}", envelope.context);
        assert!(envelope.context.ends_with("Z ctx:30%"));
        // The gauge updates are visible to the task on its next read.
        live.store(150_000, std::sync::atomic::Ordering::Relaxed);
        let next = EventEnvelope::new("agent-1", AgentEvent::McpPushedNotification {
            agent_id: "agent-1".into(),
            mcp_id: "mcp-1".into(),
            mcp_name: "Computer".into(),
            method: "m".into(),
            payload: serde_json::Value::Null,
        })
        .with_context(live.load(std::sync::atomic::Ordering::Relaxed), 300_000);
        assert!(next.context.ends_with("ctx:50%"), "got {}", next.context);
    }
}
