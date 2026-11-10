use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::{info, warn};

use super::meta_tools::{
    calculate_memory_hash, get_meta_tools_definitions, interpolate_prompt, parse_duration_string,
    render_response_hooks, MAX_MEMORY_CHARS,
};
use super::router::ProviderRouter;
use super::timer_manager::global_timer_manager;
use super::turn_queue::global_turn_queue;
use crate::db::repos::Database;
use crate::db::schema::{ActiveModelTarget, Agent, AgentPushedMessage, McpServer};
use crate::events::{AgentEvent, EventEnvelope};
use crate::mcp::client::{McpClientSession, McpNotification, McpTool};
use crate::mcp::custom_tools::CustomToolsTransport;
use crate::mcp::http::HttpTransport;
use crate::mcp::stdio::StdioTransport;

/// How many consecutive turns a model may produce *no tool call* while output
/// text is disallowed before the turn is abandoned.
///
/// This is a liveness guard, not a work cap. The turn loop itself is unbounded
/// (an agent may run for as long as it keeps calling tools and sleeping), but a
/// model that answers with prose to a tool-only agent would otherwise be re-fed
/// the same rejection forever, burning tokens with zero progress. Any tool call
/// — including `sleep` — resets the counter, so genuine long-horizon work is
/// never truncated.
const MAX_CONSECUTIVE_TEXT_REJECTIONS: u32 = 5;

#[derive(Debug, Clone)]
pub struct ToolRegistryEntry {
    pub server_id: String,
    pub server_alias: String,
    pub original_tool_name: String,
    pub tool: McpTool,
}

/// Kills every MCP child process spawned for one turn, on every exit path.
///
/// `McpTransport::close` is async, so the kill is dispatched from `Drop` onto
/// the runtime. Killing the child is what actually ends the process; the
/// blocking reader then sees EOF, drops the notification sender, and the
/// per-session listener task exits on its own.
struct SessionGuard {
    transports: Vec<Arc<dyn crate::mcp::client::McpTransport>>,
}

impl SessionGuard {
    fn new(sessions: &HashMap<String, Arc<McpClientSession>>) -> Self {
        Self {
            transports: sessions
                .values()
                .map(|s| Arc::clone(&s.transport))
                .collect(),
        }
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        for transport in self.transports.drain(..) {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = transport.close().await;
                });
            }
        }
    }
}

/// Compose the model-facing tool name for a plugin tool.
///
/// The host namespaces every plugin tool by its alias, but most plugins already
/// prefix their own tools with that same domain (`telegram_*`, `control_*`).
/// Naively doing `format!("{alias}.{tool}")` then produces `control.control_whoami`
/// and `telegram.telegram_send_message` — stuttering, ambiguous to read, and a
/// collision hazard for `resolve_tool`'s bare-name fallback.
///
/// The alias prefix is redundant when the tool already starts with `<alias>_`,
/// so it is stripped in that case only. A tool that genuinely starts with the
/// alias but is not namespaced by it (e.g. plugin `a`, tool `a_b` where the
/// plugin means the literal name) still resolves, because `resolve_tool`
/// matches `original_tool_name` as well.
pub fn compose_tool_name(alias: &str, tool: &str) -> String {
    let prefix = format!("{alias}_");
    if let Some(stripped) = tool.strip_prefix(&prefix) {
        if !stripped.is_empty() {
            return format!("{alias}.{stripped}");
        }
    }
    format!("{alias}.{tool}")
}

pub fn resolve_tool<'a>(
    registry: &'a HashMap<String, ToolRegistryEntry>,
    name: &str,
) -> Result<&'a ToolRegistryEntry, String> {
    // 1. Direct exact match (e.g. "telegram.chat_send")
    if let Some(entry) = registry.get(name) {
        return Ok(entry);
    }

    // 2. Delimiter variants (e.g. "telegram.chat_send", "telegram__chat_send", "telegram:chat_send", "telegram_chat_send")
    if let Some((prefix, suffix)) = name
        .split_once('.')
        .or_else(|| name.split_once("__"))
        .or_else(|| name.split_once(':'))
        .or_else(|| name.split_once('_'))
    {
        let candidate_dot = format!("{}.{}", prefix, suffix);
        if let Some(entry) = registry.get(&candidate_dot) {
            return Ok(entry);
        }
        let candidate_under = format!("{}__{}", prefix, suffix);
        if let Some(entry) = registry.get(&candidate_under) {
            return Ok(entry);
        }
    }

    // 3. Match by original_tool_name (e.g. "chat_send")
    let matches: Vec<(&String, &ToolRegistryEntry)> = registry
        .iter()
        .filter(|(_, entry)| entry.original_tool_name == name)
        .collect();

    match matches.len() {
        1 => Ok(matches[0].1),
        0 => Err(format!("Tool '{}' not found in active MCP servers", name)),
        _ => {
            let mut candidates: Vec<String> = matches.iter().map(|(k, _)| (*k).clone()).collect();
            candidates.sort();
            Err(format!(
                "Ambiguous tool '{}'. Multiple MCP servers provide this tool: please specify with the server alias (e.g. '{}' or '{}')",
                name,
                candidates[0],
                candidates[1]
            ))
        }
    }
}

pub fn sanitize_chat_messages(messages: Vec<Value>) -> Vec<Value> {
    // Ids answered anywhere later in the stream. Used so a group is never
    // "closed" with a synthetic reply while the real reply still follows —
    // that would produce duplicate responses for the same tool_call_id.
    let mut answered_anywhere: std::collections::HashSet<String> = std::collections::HashSet::new();
    for msg in &messages {
        if msg["role"].as_str() == Some("tool") {
            if let Some(tid) = msg["tool_call_id"].as_str() {
                if !tid.is_empty() {
                    answered_anywhere.insert(tid.to_string());
                }
            }
        }
    }

    let mut sanitized: Vec<Value> = Vec::with_capacity(messages.len());
    let mut active_tool_call_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let mut tool_id_to_name: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut seen_tool_replies: std::collections::HashSet<String> = std::collections::HashSet::new();

    for mut msg in messages {
        let role = msg["role"].as_str().unwrap_or("").to_string();

        if role == "assistant" {
            // Close the previous grouping before starting a new assistant turn.
            close_tool_call_group(
                &mut sanitized,
                &mut active_tool_call_ids,
                &mut tool_id_to_name,
                &answered_anywhere,
                "superseded by a newer assistant message",
            );

            if let Some(tool_calls) = msg.get("tool_calls").and_then(|tc| tc.as_array()) {
                for tc in tool_calls {
                    if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                        active_tool_call_ids.insert(id.to_string());
                        if let Some(fn_name) = tc
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(|n| n.as_str())
                        {
                            tool_id_to_name.insert(id.to_string(), fn_name.to_string());
                        }
                    }
                }
            }
            sanitized.push(msg);
        } else if role == "tool" {
            let tid = msg
                .get("tool_call_id")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();

            if !tid.is_empty()
                && active_tool_call_ids.contains(&tid)
                && seen_tool_replies.insert(tid.clone())
            {
                // Ensure name is present for Gemini / OpenAI proxy compliance
                if msg.get("name").is_none() || msg["name"].as_str().unwrap_or("").is_empty() {
                    if let Some(name) = tool_id_to_name.get(&tid) {
                        msg["name"] = Value::String(name.clone());
                    }
                }
                active_tool_call_ids.remove(&tid);
                sanitized.push(msg);
                if active_tool_call_ids.is_empty() {
                    tool_id_to_name.clear();
                }
            } else if seen_tool_replies.contains(&tid) {
                // Duplicate reply (e.g. a delayed real reply after a repair).
                // Providers reject responses that answer a call twice.
                warn!(
                    "Sanitizing duplicate tool message (id: '{}') into system context",
                    tid
                );
                let content = msg["content"].as_str().unwrap_or("");
                sanitized.push(json!({
                    "role": "system",
                    "content": format!("[Previous Tool Context]: {}", content)
                }));
            } else {
                warn!(
                    "Sanitizing orphaned tool message (id: '{}') into system context",
                    tid
                );
                let content = msg["content"].as_str().unwrap_or("");
                sanitized.push(json!({
                    "role": "system",
                    "content": format!("[Previous Tool Context]: {}", content)
                }));
            }
        } else {
            // "user" or "system" resets active tool call expectation
            close_tool_call_group(
                &mut sanitized,
                &mut active_tool_call_ids,
                &mut tool_id_to_name,
                &answered_anywhere,
                "a non-tool message interrupted the tool call",
            );
            sanitized.push(msg);
        }
    }

    close_tool_call_group(
        &mut sanitized,
        &mut active_tool_call_ids,
        &mut tool_id_to_name,
        &answered_anywhere,
        "end of conversation",
    );

    sanitized
}

/// Emits synthetic `tool` replies for pending tool calls so the immediately
/// preceding assistant `tool_calls` message is fully answered. Ids that are
/// answered later in the stream are skipped (that reply is out of order and
/// will be converted to context on arrival instead of being duplicated here).
fn close_tool_call_group(
    sanitized: &mut Vec<Value>,
    pending: &mut std::collections::HashSet<String>,
    tool_id_to_name: &mut std::collections::HashMap<String, String>,
    answered_anywhere: &std::collections::HashSet<String>,
    reason: &str,
) {
    if pending.is_empty() {
        return;
    }
    let mut ids: Vec<String> = pending.drain().collect();
    ids.sort();
    for id in ids {
        if answered_anywhere.contains(&id) {
            // A late reply exists; leave the group alone so we don't duplicate.
            warn!(
                "Tool call '{}' is answered out of order ({}); leaving as-is",
                id, reason
            );
            continue;
        }
        warn!(
            "Synthesizing missing tool response for unanswered tool_call '{}' ({})",
            id, reason
        );
        let mut tool_msg = json!({
            "role": "tool",
            "tool_call_id": id,
            "content": json!({ "error": format!("Tool call was not executed: {}", reason) }).to_string()
        });
        if let Some(name) = tool_id_to_name.remove(&id) {
            tool_msg["name"] = Value::String(name);
        }
        sanitized.push(tool_msg);
    }
    tool_id_to_name.clear();
}

/// Collects the tool-call ids of an `assistant` message (empty if none).
fn assistant_tool_call_ids(msg: &Value) -> Vec<String> {
    msg.get("tool_calls")
        .and_then(|tc| tc.as_array())
        .map(|calls| {
            calls
                .iter()
                .filter_map(|c| c.get("id").and_then(|i| i.as_str()).map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Answers, in-memory, every tool call of `assistant_msg` that has no reply in
/// `messages` yet. Used when the executor skips a call (compaction / aborted
/// iteration): OpenAI and Gemini both require one `tool` message per
/// `tool_call_id` before the next non-tool message can appear.
fn append_missing_tool_responses(
    messages: &mut Vec<Value>,
    assistant_msg: &Value,
    skip_id: Option<&str>,
    reason: &str,
) {
    let ids = assistant_tool_call_ids(assistant_msg);
    if ids.is_empty() {
        return;
    }

    let mut names = std::collections::HashMap::new();
    if let Some(calls) = assistant_msg.get("tool_calls").and_then(|tc| tc.as_array()) {
        for call in calls {
            if let (Some(id), Some(name)) = (
                call.get("id").and_then(|i| i.as_str()),
                call.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str()),
            ) {
                names.insert(id.to_string(), name.to_string());
            }
        }
    }

    for id in ids {
        if Some(id.as_str()) == skip_id {
            continue;
        }
        let already_answered = messages.iter().any(|m| {
            m.get("role").and_then(|r| r.as_str()) == Some("tool")
                && m.get("tool_call_id").and_then(|t| t.as_str()) == Some(id.as_str())
        });
        if already_answered {
            continue;
        }

        warn!(
            "Emitting synthetic tool response for unanswered tool_call '{}' ({})",
            id, reason
        );
        let mut tool_msg = json!({
            "role": "tool",
            "tool_call_id": id,
            "content": json!({ "error": format!("Tool call was not executed: {}", reason) }).to_string()
        });
        if let Some(name) = names.get(&id) {
            tool_msg["name"] = Value::String(name.clone());
        }
        messages.push(tool_msg);
    }
}

/// Normalises persisted history into a transcript that is valid for *both*
/// OpenAI-style (adjacency) and Gemini-style (positional per-turn) tool calls.
///
/// Rules, applied in one pass:
///   * an `assistant` message with `tool_calls` opens a group; it is followed
///     immediately by one `tool` reply per call id, in call order;
///   * a `tool` reply is consumed by the *current* group if its id belongs
///     there; if it belongs to an earlier group (out-of-order lane), the
///     earlier group is now closed and this reply is an orphan → system note;
///   * ids still missing when the group closes get a synthetic error reply so
///     the assistant message is never left unanswered;
///   * duplicate replies for an already answered id become system notes.
///
/// Non tool/system/user roles and `system` rows are preserved in place.
pub fn normalize_history_transcript(history: &[crate::db::schema::AgentHistoryItem]) -> Vec<Value> {
    use std::collections::{HashMap, HashSet};

    let mut out: Vec<Value> = Vec::with_capacity(history.len());
    // Open group: ordered ids plus the names needed for the `name` field.
    let mut open_ids: Vec<String> = Vec::new();
    let mut open_names: HashMap<String, String> = HashMap::new();
    let mut answered: HashSet<String> = HashSet::new();
    // System notes about orphaned/duplicate replies. They must not be emitted
    // between an assistant message and its pending replies, so they are held
    // until the open group is closed and only then appended.
    let mut deferred_notes: Vec<Value> = Vec::new();
    // System rows that arrived while a tool-call group was still open.
    let mut deferred_system: Vec<Value> = Vec::new();

    fn close_group(
        out: &mut Vec<Value>,
        open_ids: &mut Vec<String>,
        open_names: &mut HashMap<String, String>,
        answered: &mut HashSet<String>,
        recovered: &HashMap<String, String>,
    ) {
        for id in open_ids.iter() {
            if answered.contains(id) {
                continue;
            }
            let content = match recovered.get(id) {
                Some(real) => {
                    warn!(
                        "History repair: reattaching recovered real reply for '{}'",
                        id
                    );
                    real.clone()
                }
                None => {
                    warn!(
                        "History repair: unanswered tool_call '{}' — inserting synthetic response",
                        id
                    );
                    json!({
                        "error": "This tool call was not executed because the turn was interrupted."
                    })
                    .to_string()
                }
            };
            let mut tool_msg = json!({
                "role": "tool",
                "tool_call_id": id,
                "content": content
            });
            if let Some(name) = open_names.get(id) {
                tool_msg["name"] = Value::String(name.clone());
            }
            out.push(tool_msg);
        }
        open_ids.clear();
        open_names.clear();
        answered.clear();
    }

    fn flush_notes(
        out: &mut Vec<Value>,
        deferred_notes: &mut Vec<Value>,
        deferred_system: &mut Vec<Value>,
    ) {
        out.append(deferred_notes);
        out.append(deferred_system);
    }

    // Index of *real* replies per tool_call_id (ignoring synthetic ones this
    // repair itself produced in an earlier run). If a real reply exists
    // anywhere later in the history, the synthetic placeholder is garbage and
    // must be removed rather than preserved.
    const SYNTHETIC_MARKER: &str =
        "This tool call was not executed because the turn was interrupted.";
    let mut real_reply_ids: HashSet<String> = HashSet::new();
    for item in history {
        if item.role == "tool" {
            if let Some(tid) = item.tool_call_id.as_deref() {
                if !tid.is_empty() && !item.content.contains(SYNTHETIC_MARKER) {
                    real_reply_ids.insert(tid.to_string());
                }
            }
        }
    }
    // A previous repair run may have demoted the REAL reply to
    // "[Previous Tool Context]: ..." system context while leaving the
    // synthetic placeholder as the adjacent `tool` row. Detect that too: if
    // the group's real answer survives as context, the placeholder must go.
    let has_demoted_reply = history
        .iter()
        .any(|h| h.role == "system" && h.content.starts_with("[Previous Tool Context]:"));
    // The demoted notes in order of appearance; they are matched positionally
    // against groups whose synthetic placeholder was dropped.
    // Pair each demoted "[Previous Tool Context]" note with the synthetic
    // placeholder it replaced. The synthetic row is dropped and its group is
    // answered by the demoted note, reattached AT THE SYNTHETIC ROW'S POSITION
    // (which is adjacent to the assistant message). This repairs histories
    // corrupted by an earlier repair run without inventing new errors.
    let synthetic_ids: Vec<String> = history
        .iter()
        .filter(|h| h.role == "tool" && h.content.contains(SYNTHETIC_MARKER))
        .filter_map(|h| h.tool_call_id.clone())
        .collect();

    let mut demoted_notes: Vec<String> = history
        .iter()
        .filter(|h| h.role == "system" && h.content.starts_with("[Previous Tool Context]:"))
        .map(|h| {
            h.content
                .trim_start_matches("[Previous Tool Context]:")
                .trim()
                .to_string()
        })
        .collect();

    // tool_call_id -> real content recovered from a demoted note
    let mut recovered: HashMap<String, String> = HashMap::new();
    // Only treat a demoted note as the real answer when the history is in the
    // *corrupted* shape: a compaction summary written between the assistant
    // message and its reply (compact ran twice in one turn). Without that
    // marker the note is simply historical context, and re-using it would
    // re-open an already-answered group (breaking idempotency).
    let has_double_compaction = {
        let summaries: Vec<usize> = history
            .iter()
            .enumerate()
            .filter(|(_, h)| {
                h.role == "system" && h.content.starts_with("### [Compaction Summary]")
            })
            .map(|(i, _)| i)
            .collect();
        summaries.windows(2).any(|w| w[1] - w[0] <= 3)
    };
    if has_demoted_reply && has_double_compaction {
        for tid in synthetic_ids.iter() {
            // Only repair when no real `tool` row exists for this id.
            if real_reply_ids.contains(tid) || demoted_notes.is_empty() {
                continue;
            }
            recovered.insert(tid.clone(), demoted_notes.remove(0));
        }
    }
    let consumed_notes: HashSet<String> = recovered.values().cloned().collect();

    for item in history {
        let mut msg = json!({ "role": item.role, "content": item.content });

        match item.role.as_str() {
            "assistant" => {
                close_group(
                    &mut out,
                    &mut open_ids,
                    &mut open_names,
                    &mut answered,
                    &recovered,
                );
                flush_notes(&mut out, &mut deferred_notes, &mut deferred_system);

                if let Some(raw) = item.tool_calls_json.as_deref() {
                    if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
                        if let Some(calls) = parsed.as_array() {
                            if !calls.is_empty() {
                                msg["tool_calls"] = Value::Array(calls.clone());
                                for call in calls {
                                    if let Some(id) = call.get("id").and_then(|i| i.as_str()) {
                                        open_ids.push(id.to_string());
                                        let name = call
                                            .get("function")
                                            .and_then(|f| f.get("name"))
                                            .and_then(|n| n.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        if !name.is_empty() {
                                            open_names.insert(id.to_string(), name);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                out.push(msg);
            }
            "tool" => {
                let tid = item.tool_call_id.clone().unwrap_or_default();
                // A synthetic placeholder is dropped only when we recovered a
                // real reply for it. A plain `has_demoted_reply` elsewhere in
                // history is not evidence about THIS id, and dropping on that
                // basis deletes a legitimate answer (non-idempotent).
                if item.content.contains(SYNTHETIC_MARKER)
                    && (real_reply_ids.contains(&tid) || recovered.contains_key(&tid))
                {
                    warn!(
                        "History repair: dropping stale synthetic reply for '{}'",
                        tid
                    );
                    answered.insert(tid.clone());
                    // Emit the recovered real answer in its place (adjacent to
                    // the assistant message) so the group stays answered;
                    // otherwise the group is answered later by close_group.
                    if let Some(real) = recovered.get(&tid) {
                        let mut tool_msg = json!({
                            "role": "tool",
                            "tool_call_id": tid,
                            "content": real,
                        });
                        if let Some(name) = open_names.get(&tid) {
                            tool_msg["name"] = Value::String(name.clone());
                        }
                        out.push(tool_msg);
                    }
                    continue;
                }
                if !tid.is_empty()
                    && open_ids.iter().any(|id| *id == tid)
                    && !answered.contains(&tid)
                {
                    let mut tool_msg = json!({
                        "role": "tool",
                        "tool_call_id": tid,
                        "content": item.content,
                    });
                    if let Some(name) = open_names.get(&tid) {
                        tool_msg["name"] = Value::String(name.clone());
                    }
                    answered.insert(tid);
                    out.push(tool_msg);
                } else {
                    warn!(
                        "History repair: orphaned/duplicate tool reply '{}' → system context",
                        tid
                    );
                    deferred_notes.push(json!({
                        "role": "system",
                        "content": format!("[Previous Tool Context]: {}", item.content)
                    }));
                }
            }
            // A `system` row does NOT close an open tool-call group:
            // compaction legitimately writes `[assistant(compact)] [tool]
            // [summary system]`, and pushes/repairs can land mid-group.
            // Defer the row until the group is answered so the assistant
            // message stays immediately followed by its replies.
            // A "[Previous Tool Context]" note that was reattached as a real
            // tool reply above must not also appear as its own row.
            "system"
                if item.content.starts_with("[Previous Tool Context]:")
                    && consumed_notes.contains(
                        item.content
                            .trim_start_matches("[Previous Tool Context]:")
                            .trim(),
                    ) =>
            {
                continue;
            }
            "system" if !open_ids.is_empty() => {
                // Running `compact` twice in one turn produced duplicate
                // summary rows; keep only the first.
                let dup = deferred_system
                    .iter()
                    .chain(out.iter().rev().take(1))
                    .any(|prev| {
                        prev.get("content").and_then(|c| c.as_str()) == msg["content"].as_str()
                    });
                if !dup {
                    deferred_system.push(msg);
                }
            }
            _ => {
                close_group(
                    &mut out,
                    &mut open_ids,
                    &mut open_names,
                    &mut answered,
                    &recovered,
                );
                flush_notes(&mut out, &mut deferred_notes, &mut deferred_system);
                out.push(msg);
            }
        }
    }

    close_group(
        &mut out,
        &mut open_ids,
        &mut open_names,
        &mut answered,
        &recovered,
    );
    flush_notes(&mut out, &mut deferred_notes, &mut deferred_system);
    out
}

/// True when rewriting `normalized` over `history` would change anything
/// (row counts or payloads), so a healthy history is never rewritten.
fn normalized_changed(
    history: &[crate::db::schema::AgentHistoryItem],
    normalized: &[Value],
) -> bool {
    if history.len() != normalized.len() {
        return true;
    }
    history.iter().zip(normalized).any(|(item, msg)| {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
        let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
        if item.role != role || item.content != content {
            return true;
        }
        let stored_calls = item
            .tool_calls_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
        let wanted_calls = msg.get("tool_calls").cloned();
        if stored_calls != wanted_calls {
            return true;
        }
        let stored_tid = item.tool_call_id.clone();
        let wanted_tid = msg
            .get("tool_call_id")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string());
        stored_tid != wanted_tid
    })
}

pub struct AgentExecutor {
    pub db: Arc<Database>,
    pub agent: Agent,
    pub models: Vec<ActiveModelTarget>,
    pub mcp_servers: Vec<McpServer>,
    pub event_tx: broadcast::Sender<EventEnvelope>,
}

impl AgentExecutor {
    pub fn new(
        db: Arc<Database>,
        agent: Agent,
        models: Vec<ActiveModelTarget>,
        mcp_servers: Vec<McpServer>,
        event_tx: broadcast::Sender<EventEnvelope>,
    ) -> Self {
        Self {
            db,
            agent,
            models,
            mcp_servers,
            event_tx,
        }
    }

    /// Records one row in the append-only transcript. Failures are logged but
    /// never abort a turn: the working context must stay usable even if the
    /// archival write fails.
    async fn record_transcript(
        &self,
        role: &str,
        content: &str,
        tool_calls_json: Option<&str>,
        tool_call_id: Option<&str>,
        origin: &str,
        turn: i64,
    ) -> Option<String> {
        match self
            .db
            .append_transcript(
                &self.agent.id,
                role,
                content,
                tool_calls_json,
                tool_call_id,
                origin,
                turn,
                None,
            )
            .await
        {
            Ok(item) => Some(item.id),
            Err(e) => {
                warn!("Failed to append transcript row ({}): {}", role, e);
                None
            }
        }
    }

    // -------------------------------------------------------------------------
    // Keyed memory graph and skills
    // -------------------------------------------------------------------------

    /// Shared write path for `memoryWrite` and `skillWrite`.
    ///
    /// Both are the same operation with a different `type`; keeping one
    /// implementation means the hash-CAS and link-derivation rules cannot drift
    /// between them.
    async fn write_memory_node(
        &self,
        raw_key: &str,
        value: &str,
        node_type: &str,
        description: &str,
        expected_hash: Option<&str>,
    ) -> (Value, bool) {
        let Some(key) = crate::db::repos::normalize_memory_key(raw_key) else {
            return (
                json!({
                    "error": {
                        "code": "invalid_key",
                        "message": format!(
                            "'{raw_key}' is not a usable memory key: it must be 1-{} characters, a single line, and must not contain '[', ']' or '|'.",
                            crate::db::repos::MEMORY_KEY_MAX_CHARS
                        ),
                        "retryable": true,
                        "hint": "Use a canonical lowercase noun phrase, e.g. 'postgres primary'."
                    }
                }),
                false,
            );
        };

        let value_chars = value.chars().count();
        if value.trim().is_empty() {
            return (
                json!({
                    "error": {
                        "code": "empty_value",
                        "message": "A memory node cannot be empty. Use memoryDelete to remove one.",
                        "retryable": true
                    }
                }),
                false,
            );
        }
        if value_chars > crate::db::repos::MEMORY_VALUE_MAX_CHARS {
            return (
                json!({
                    "error": {
                        "code": "value_too_long",
                        "message": format!(
                            "The value is {value_chars} characters; the limit is {}.",
                            crate::db::repos::MEMORY_VALUE_MAX_CHARS
                        ),
                        "retryable": true,
                        "hint": "Split it into several linked nodes instead of one huge one."
                    }
                }),
                false,
            );
        }

        // Optimistic concurrency: an update must prove it read the current node.
        let existing = match self.db.get_memory_node(&self.agent.id, &key).await {
            Ok(node) => node,
            Err(e) => {
                return (
                    json!({ "error": { "code": "storage_error", "message": format!("Could not read the existing node: {e}"), "retryable": true } }),
                    false,
                );
            }
        };
        match (&existing, expected_hash) {
            (Some(node), None) => {
                return (
                    json!({
                        "error": {
                            "code": "hash_required",
                            "message": format!("Memory node '{key}' already exists; pass its expectedHash to replace it."),
                            "retryable": true,
                            "hint": format!("Call memoryRead(key=\"{key}\") to obtain hash {} and decide whether to overwrite.", node.hash)
                        }
                    }),
                    false,
                );
            }
            (Some(node), Some(provided)) if provided != node.hash => {
                return (
                    json!({
                        "error": {
                            "code": "stale_hash",
                            "message": format!("Hash mismatch for '{key}': provided '{provided}' but the node is at '{}'. Nothing was written.", node.hash),
                            "retryable": true,
                            "hint": format!("Call memoryRead(key=\"{key}\") to see the current content, then re-apply your change.")
                        }
                    }),
                    false,
                );
            }
            _ => {}
        }

        let hash = crate::runtime::meta_tools::calculate_memory_hash(value);
        let description = description.trim();
        if let Err(e) = self
            .db
            .upsert_memory_node(&self.agent.id, &key, value, node_type, description, &hash)
            .await
        {
            return (
                json!({ "error": { "code": "storage_error", "message": format!("Memory was not saved: {e}"), "retryable": true } }),
                false,
            );
        }

        let links = crate::db::repos::extract_wikilinks(value);
        // The graph view and the prompt manifest are both derived, so tell the
        // UI a node changed.
        self.emit(AgentEvent::MemoryUpdated {
            agent_id: self.agent.id.clone(),
            updated_hash: hash.clone(),
            char_count: value_chars,
        });

        (
            json!({
                "success": true,
                "key": key,
                "type": node_type,
                "hash": hash,
                "character_count": value_chars,
                "links": links,
                "replaced": existing.is_some(),
            }),
            true,
        )
    }

    async fn memory_write(&self, args: &Value) -> (Value, bool) {
        let key = args["key"].as_str().unwrap_or("");
        let value = args["value"].as_str().unwrap_or("");
        let node_type = args["type"].as_str().unwrap_or("concept");
        let description = args["description"].as_str().unwrap_or("");
        let expected = args["expectedHash"].as_str();
        self.write_memory_node(key, value, node_type, description, expected)
            .await
    }

    async fn memory_read(&self, args: &Value) -> (Value, bool) {
        let raw_key = args["key"].as_str().unwrap_or("");
        let Some(key) = crate::db::repos::normalize_memory_key(raw_key) else {
            return (
                json!({ "error": { "code": "invalid_key", "message": format!("'{raw_key}' is not a valid memory key."), "retryable": true } }),
                false,
            );
        };

        let views = match self.db.memory_nodes_with_links(&self.agent.id).await {
            Ok(views) => views,
            Err(e) => {
                return (
                    json!({ "error": { "code": "storage_error", "message": format!("Could not read memory: {e}"), "retryable": true } }),
                    false,
                );
            }
        };

        match views.into_iter().find(|v| v.key == key) {
            Some(view) => (
                json!({
                    "key": view.key,
                    "type": view.r#type,
                    "description": view.description,
                    "value": view.value,
                    "hash": view.hash,
                    "updated_at": view.updated_at,
                    "links": view.outbound,
                    "backlinks": view.backlinks,
                }),
                true,
            ),
            None => (
                json!({
                    "error": {
                        "code": "not_found",
                        "message": format!("No memory node '{key}'."),
                        "retryable": true,
                        "hint": "Use memoryList or memorySearch to find the right key."
                    }
                }),
                false,
            ),
        }
    }

    async fn memory_search(&self, args: &Value) -> (Value, bool) {
        let query = args["query"].as_str().unwrap_or("");
        let limit = args["limit"].as_u64().unwrap_or(10) as usize;
        match self
            .db
            .search_memory_nodes(&self.agent.id, query, limit)
            .await
        {
            Ok(nodes) => {
                let results: Vec<Value> = nodes
                    .iter()
                    .map(|n| {
                        json!({
                            "key": n.key,
                            "type": n.r#type,
                            "description": n.description,
                            "hash": n.hash,
                            "preview": n.value.chars().take(160).collect::<String>(),
                        })
                    })
                    .collect();
                (
                    json!({ "query": query, "count": results.len(), "results": results }),
                    true,
                )
            }
            Err(e) => (
                json!({ "error": { "code": "storage_error", "message": format!("Search failed: {e}"), "retryable": true } }),
                false,
            ),
        }
    }

    async fn memory_list(&self, args: &Value) -> (Value, bool) {
        let wanted_type = args["type"].as_str().map(|t| t.trim().to_lowercase());
        match self.db.memory_nodes_with_links(&self.agent.id).await {
            Ok(views) => {
                let nodes: Vec<Value> = views
                    .iter()
                    .filter(|v| {
                        wanted_type
                            .as_deref()
                            .map(|t| v.r#type.eq_ignore_ascii_case(t))
                            .unwrap_or(true)
                    })
                    .map(|v| {
                        json!({
                            "key": v.key,
                            "type": v.r#type,
                            "description": v.description,
                            "hash": v.hash,
                            "links": v.outbound.len(),
                            "backlinks": v.backlinks.len(),
                        })
                    })
                    .collect();
                (json!({ "count": nodes.len(), "nodes": nodes }), true)
            }
            Err(e) => (
                json!({ "error": { "code": "storage_error", "message": format!("Could not list memory: {e}"), "retryable": true } }),
                false,
            ),
        }
    }

    async fn memory_delete(&self, args: &Value) -> (Value, bool) {
        let raw_key = args["key"].as_str().unwrap_or("");
        let Some(key) = crate::db::repos::normalize_memory_key(raw_key) else {
            return (
                json!({ "error": { "code": "invalid_key", "message": format!("'{raw_key}' is not a valid memory key."), "retryable": true } }),
                false,
            );
        };
        let Some(expected) = args["expectedHash"]
            .as_str()
            .filter(|h| !h.trim().is_empty())
        else {
            return (
                json!({
                    "error": {
                        "code": "hash_required",
                        "message": "memoryDelete requires expectedHash so a stale read cannot delete newer content.",
                        "retryable": true,
                        "hint": format!("Call memoryRead(key=\"{key}\") to obtain the current hash.")
                    }
                }),
                false,
            );
        };

        let existing = match self.db.get_memory_node(&self.agent.id, &key).await {
            Ok(node) => node,
            Err(e) => {
                return (
                    json!({ "error": { "code": "storage_error", "message": format!("Could not read the node: {e}"), "retryable": true } }),
                    false,
                );
            }
        };
        let Some(node) = existing else {
            return (
                json!({ "error": { "code": "not_found", "message": format!("No memory node '{key}'."), "retryable": true } }),
                false,
            );
        };
        if expected != node.hash {
            return (
                json!({
                    "error": {
                        "code": "stale_hash",
                        "message": format!("Hash mismatch for '{key}': provided '{expected}' but the node is at '{}'. Nothing was deleted.", node.hash),
                        "retryable": true
                    }
                }),
                false,
            );
        }

        match self.db.delete_memory_node(&self.agent.id, &key).await {
            Ok(deleted) => (
                json!({ "success": true, "key": key, "deleted": deleted }),
                true,
            ),
            Err(e) => (
                json!({ "error": { "code": "storage_error", "message": format!("Delete failed: {e}"), "retryable": true } }),
                false,
            ),
        }
    }

    async fn skill_write(&self, args: &Value) -> (Value, bool) {
        let name = args["name"].as_str().unwrap_or("");
        let instructions = args["instructions"].as_str().unwrap_or("");
        let description = args["description"].as_str().unwrap_or("");
        let expected = args["expectedHash"].as_str();
        if description.trim().is_empty() {
            return (
                json!({
                    "error": {
                        "code": "missing_description",
                        "message": "A skill needs a description: it is what memorySearch matches when deciding whether to load the skill.",
                        "retryable": true
                    }
                }),
                false,
            );
        }
        self.write_memory_node(name, instructions, "skill", description, expected)
            .await
    }

    async fn skill_list(&self) -> (Value, bool) {
        match self.db.memory_nodes_with_links(&self.agent.id).await {
            Ok(views) => {
                let skills: Vec<Value> = views
                    .iter()
                    .filter(|v| v.r#type.eq_ignore_ascii_case("skill"))
                    .map(|v| {
                        json!({
                            "name": v.key,
                            "description": v.description,
                            "hash": v.hash,
                            "character_count": v.value.chars().count(),
                        })
                    })
                    .collect();
                (
                    json!({
                        "count": skills.len(),
                        "skills": skills,
                        "note": "Load a skill with memoryRead(key=<name>) before following it."
                    }),
                    true,
                )
            }
            Err(e) => (
                json!({ "error": { "code": "storage_error", "message": format!("Could not list skills: {e}"), "retryable": true } }),
                false,
            ),
        }
    }

    /// Per-agent head/body/tail truncation config for large payloads.
    fn truncation_config(&self) -> crate::runtime::meta_tools::TruncationConfig {
        crate::runtime::meta_tools::TruncationConfig::default()
    }

    /// Applies head/body/tail truncation to an event payload. Returns the
    /// (possibly truncated) JSON plus whether anything was elided.
    fn truncate_event_payload(&self, payload: &Value) -> (Value, bool) {
        let serialized = payload.to_string();
        let truncated = self.truncate_for_context(&serialized, None);
        if truncated == serialized {
            (payload.clone(), false)
        } else {
            (Value::String(truncated), true)
        }
    }

    /// Truncates `content` for the working context / event stream, embedding
    /// the `response_id` so the full text stays reachable via
    /// `history_inspect`.
    fn truncate_for_context(&self, content: &str, response_id: Option<&str>) -> String {
        self.truncate_for_context_with(content, response_id, self.truncation_config())
    }

    /// Same as [`Self::truncate_for_context`] but with an explicit config, used
    /// for per-call `head`/`body`/`tail` overrides on `call_tool`.
    fn truncate_for_context_with(
        &self,
        content: &str,
        response_id: Option<&str>,
        cfg: crate::runtime::meta_tools::TruncationConfig,
    ) -> String {
        let truncated = crate::runtime::meta_tools::truncate_middle(content, cfg);
        match response_id {
            Some(id) if truncated != content => format!(
                "{}\n[full content: history_inspect(id=\"{}\", from=.., to=..)]",
                truncated, id
            ),
            _ => truncated,
        }
    }

    /// Total characters currently in the working context.
    fn measure_messages(messages: &[Value]) -> usize {
        messages
            .iter()
            .map(|m| {
                m["content"]
                    .as_str()
                    .map(|s| s.chars().count())
                    .unwrap_or(0)
                    + m.get("tool_calls")
                        .map(|tc| tc.to_string().chars().count())
                        .unwrap_or(0)
            })
            .sum()
    }

    fn emit(&self, event: AgentEvent) {
        let envelope = EventEnvelope::new(&self.agent.id, event);
        let _ = self.event_tx.send(envelope);
    }

    /// Emits an event stamped with the current context pressure.
    fn emit_with_context(&self, event: AgentEvent, messages: &[Value]) {
        let current: usize = messages
            .iter()
            .map(|m| {
                m["content"]
                    .as_str()
                    .map(|s| s.chars().count())
                    .unwrap_or(0)
                    + m.get("tool_calls")
                        .map(|tc| tc.to_string().chars().count())
                        .unwrap_or(0)
            })
            .sum();
        let envelope = EventEnvelope::new(&self.agent.id, event)
            .with_context(current, self.agent.max_history_chars());
        let _ = self.event_tx.send(envelope);
    }

    fn format_pushed_message(&self, msg: &AgentPushedMessage) -> String {
        let server_name = msg
            .mcp_id
            .as_deref()
            .and_then(|id| {
                self.mcp_servers
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| s.name.as_str())
            })
            .or_else(|| msg.mcp_id.as_deref())
            .unwrap_or("System MCP");

        let params_str = msg.params_json.as_deref().unwrap_or("{}");
        let parsed: serde_json::Value =
            serde_json::from_str(params_str).unwrap_or(serde_json::Value::Null);

        let uri_str = parsed.get("uri").and_then(|u| u.as_str());

        let time_str = msg
            .created_at
            .map(|t| t.format("%l:%M %p").to_string().trim().to_string())
            .unwrap_or_else(|| "just now".to_string());

        if let Some(uri) = uri_str {
            format!(
                "- [Source: {}] [Resource: {}] [Event: {}] ({}): {}\n",
                server_name, uri, msg.method, time_str, params_str
            )
        } else {
            format!(
                "- [Source: {}] [Event: {}] ({}): {}\n",
                server_name, msg.method, time_str, params_str
            )
        }
    }

    /// Initializes MCP sessions and sets up background listener for pushed information
    pub async fn setup_mcp_sessions(
        &self,
        // Live history size in chars, shared with the notification tasks so
        // push events can report real context pressure.
        current_chars: Arc<std::sync::atomic::AtomicUsize>,
    ) -> (
        HashMap<String, Arc<McpClientSession>>,
        HashMap<String, ToolRegistryEntry>,
        Vec<String>,
    ) {
        let mut sessions = HashMap::new();
        let mut tool_registry = HashMap::new();
        // Plugins whose `tools/list` failed this turn. `search_tools` must not
        // report these as "no matches": an empty result caused by a dead or
        // still-starting plugin is a different answer from "no such tool".
        let mut unreachable: Vec<String> = Vec::new();

        for server in &self.mcp_servers {
            if !server.is_active {
                continue;
            }
            let (notif_tx, notif_rx) = mpsc::unbounded_channel::<McpNotification>();

            let transport: Option<Arc<dyn crate::mcp::client::McpTransport>> = if server.transport
                == "stdio"
                || server.transport == "binary"
            {
                let launch = crate::mcp::sandbox::SandboxSpec::launch_for(server);
                match StdioTransport::spawn_with_sandbox(
                    &server.url_or_command,
                    server.headers_or_env.as_deref(),
                    Some(self.agent.id.as_str()),
                    notif_tx,
                    &launch,
                )
                .await
                {
                    Ok(t) => Some(t),
                    Err(e) => {
                        self.emit(AgentEvent::AgentError {
                            agent_id: self.agent.id.clone(),
                            message: format!("Failed to start MCP server '{}': {}", server.name, e),
                        });
                        None
                    }
                }
            } else if server.transport == "custom_tools" || server.transport == "curl" {
                match CustomToolsTransport::new(&server.id, &server.url_or_command, notif_tx) {
                    Ok(t) => Some(Arc::new(t)),
                    Err(e) => {
                        self.emit(AgentEvent::AgentError {
                            agent_id: self.agent.id.clone(),
                            message: format!(
                                "Failed to load custom tools for '{}': {}",
                                server.name, e
                            ),
                        });
                        None
                    }
                }
            } else {
                Some(HttpTransport::new(
                    &server.url_or_command,
                    server.headers_or_env.as_deref(),
                    notif_tx,
                ))
            };

            if let Some(transport) = transport {
                let session = Arc::new(McpClientSession {
                    server_id: server.id.clone(),
                    server_name: server.name.clone(),
                    transport,
                    notification_rx: Mutex::new(notif_rx),
                });

                if let Err(e) = session.initialize().await {
                    warn!("MCP init handshake failed for {}: {}", server.name, e);
                }

                // Push subscriptions are owned by the server's long-lived
                // `SubscriptionHub`, not by a turn. Subscribing here too would
                // start a second consumer for the same bot (Telegram answers
                // 409) and the stream would die with this turn anyway.

                // Granular Tool Permissions: filter out disabled tools from discovery
                let tool_perms = self
                    .db
                    .get_agent_tool_permissions(&self.agent.id)
                    .await
                    .unwrap_or_default();
                let disabled_tools: std::collections::HashSet<String> = tool_perms
                    .into_iter()
                    .filter(|p| p.mcp_id == server.id && !p.is_enabled)
                    .map(|p| p.tool_name)
                    .collect();

                match session.list_tools().await {
                    Ok(tools) => {
                        let server_alias = server.get_alias();
                        for tool in tools {
                            if tool.is_app_only() {
                                continue;
                            }
                            if disabled_tools.contains(&tool.name) {
                                info!(
                                    "Tool '{}' from MCP server '{}' is disabled for agent '{}' - hidden from discovery",
                                    tool.name, server.name, self.agent.name
                                );
                                continue;
                            }
                            let unique_tool_name = compose_tool_name(&server_alias, &tool.name);
                            tool_registry.insert(
                                unique_tool_name,
                                ToolRegistryEntry {
                                    server_id: server.id.clone(),
                                    server_alias: server_alias.clone(),
                                    original_tool_name: tool.name.clone(),
                                    tool,
                                },
                            );
                        }
                    }
                    Err(e) => {
                        warn!("Failed to list tools from {}: {}", server.name, e);
                        unreachable.push(format!("{}: {}", server.name, e));
                    }
                }

                let agent_id = self.agent.id.clone();
                let mcp_id = server.id.clone();
                let mcp_name = server.name.clone();
                let event_tx = self.event_tx.clone();
                let db = Arc::clone(&self.db);
                let session_worker = Arc::clone(&session);
                let live_chars = Arc::clone(&current_chars);
                let max_chars = self.agent.max_history_chars();

                tokio::spawn(async move {
                    let mut rx = session_worker.notification_rx.lock().await;
                    while let Some(notif) = rx.recv().await {
                        info!(
                            "MCP server '{}' pushed information: method={}",
                            mcp_name, notif.method
                        );

                        let params_str = notif.params.as_ref().map(|p| p.to_string());
                        let _ = db
                            .save_pushed_message(
                                &agent_id,
                                Some(&mcp_id),
                                &notif.method,
                                params_str.as_deref(),
                            )
                            .await;

                        // Event stream must not carry unbounded payloads: keep a
                        // head/body/tail view (the full JSON is already stored
                        // in agent_pushed_messages and the transcript).
                        let payload_val = notif.params.unwrap_or(Value::Null);
                        let payload_raw = payload_val.to_string();
                        let payload_truncated = crate::runtime::meta_tools::truncate_middle(
                            &payload_raw,
                            crate::runtime::meta_tools::TruncationConfig::default(),
                        );
                        let payload_val = if payload_truncated == payload_raw {
                            payload_val
                        } else {
                            Value::String(payload_truncated)
                        };
                        // Report the real live context pressure, read from the
                        // shared cell the executor keeps up to date.
                        let current = live_chars.load(std::sync::atomic::Ordering::Relaxed);
                        let envelope = EventEnvelope::new(
                            &agent_id,
                            AgentEvent::McpPushedNotification {
                                agent_id: agent_id.clone(),
                                mcp_id: mcp_id.clone(),
                                mcp_name: mcp_name.clone(),
                                method: notif.method.clone(),
                                payload: payload_val,
                            },
                        )
                        .with_context(current, max_chars);
                        let _ = event_tx.send(envelope);
                    }
                });

                sessions.insert(server.id.clone(), session);
            }
        }

        (sessions, tool_registry, unreachable)
    }

    /// Offer a message to this agent.
    ///
    /// Exactly one turn runs per agent. If a turn is already active the message
    /// is queued and merged with any other arrivals into a single batch, which
    /// runs as the next turn when the current one yields. A sleeping agent is
    /// woken so the batch is not stuck behind the timer, but the running turn
    /// is never duplicated — that duplication is what made two `sleep` calls
    /// cancel each other forever.
    pub async fn run_turn(&self, user_input: &str) -> Result<String, String> {
        let Some((token, _guard)) =
            global_turn_queue().acquire(&self.agent.id, user_input.to_string())
        else {
            info!(
                "agent {} is busy; queued the incoming message for the next batch",
                self.agent.id
            );
            // Additive only: the active turn keeps running to completion (and
            // any sleep it is in runs its full duration). The batch is handled
            // afterwards by the runner, never as a second concurrent turn.
            return Ok(String::new());
        };

        // This caller owns the agent. Drain the queue until nothing is left.
        // `_guard` releases the agent on drop, including on unwind.
        let mut last: Result<String, String> = Ok(String::new());
        loop {
            let Some(batch) = global_turn_queue().take_batch(&self.agent.id, token) else {
                break;
            };
            last = self.run_turn_inner(&batch).await;
        }
        last
    }

    /// Execute one agent turn using persistent running history and enforcing output text settings.
    async fn run_turn_inner(&self, user_input: &str) -> Result<String, String> {
        let turn_id = uuid::Uuid::new_v4().to_string();
        // Monotonic per-agent turn number, persisted in the transcript so
        // exports can group rows into turns for distillation.
        let turn_number = self
            .db
            .get_agent_turn_counter(&self.agent.id)
            .await
            .unwrap_or(0)
            + 1;
        self.emit(AgentEvent::TurnStarted {
            agent_id: self.agent.id.clone(),
            turn_id: turn_id.clone(),
        });

        // Cancel any active sleep timer for this agent so it doesn't retrigger or conflict
        global_timer_manager().cancel_timer(&self.agent.id, "new turn started");

        // Shared live context pressure; updated as the transcript grows and
        // read by the spawned MCP notification tasks.
        let live_chars = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (sessions, tool_registry, unreachable_servers) =
            self.setup_mcp_sessions(Arc::clone(&live_chars)).await;

        // Every turn spawns its own plugin processes. Nothing else reaps them:
        // the notification listener holds a session `Arc` (so the transport is
        // never dropped) and the child is not `kill_on_drop`. Because
        // `run_turn` returns early on many error paths (no provider, provider
        // failure, cancellation), cleanup has to be tied to scope, not to the
        // end of the function. Leaked children keep long-polling and fight the
        // next turn for a single-consumer API such as Telegram's getUpdates.
        let _session_guard = SessionGuard::new(&sessions);

        let mut allowed_tools: Vec<String> = tool_registry.keys().cloned().collect();
        allowed_tools.sort();

        // Fetch current memory
        let current_mem = self
            .db
            .get_agent_memory(&self.agent.id)
            .await
            .unwrap_or_else(|_| crate::db::schema::AgentMemory {
                agent_id: self.agent.id.clone(),
                content: String::new(),
                hash: calculate_memory_hash(""),
                updated_at: None,
            });

        // Load existing running history from SQLite
        let mut db_history = self
            .db
            .get_agent_history(&self.agent.id)
            .await
            .unwrap_or_default();

        let max_history_chars = self.agent.max_history_chars();
        if db_history.is_empty() {
            let base_template = self
                .agent
                .system_prompt
                .as_deref()
                .unwrap_or("You are {{agent.name}}, an autonomous AI assistant.");

            let system_text = interpolate_prompt(
                base_template,
                &self.agent.id,
                &self.agent.name,
                &self.agent.strategy,
                &current_mem.content,
                &current_mem.hash,
                &allowed_tools,
                max_history_chars,
            );

            let initial_sys = self
                .db
                .append_agent_history(
                    &self.agent.id,
                    "system",
                    &system_text,
                    None,
                    None,
                    Some(base_template),
                )
                .await
                .map_err(|e| e.to_string())?;

            // The base system prompt appears exactly once in the transcript.
            self.record_transcript("system", &system_text, None, None, "system", turn_number)
                .await;

            db_history.push(initial_sys);
        } else {
            // The conversation already has a base instruction row. If the agent's
            // `system_prompt` changed since that row was rendered (the user
            // picked a different Agent Instructions preset, or edited the text by
            // hand), re-render it *in place*.
            //
            // Without this the old instruction silently wins forever: row 0 is
            // written once and only rebuilt at compaction, so a new preset would
            // appear selected in the UI while the model kept obeying the old text.
            let base_template = self
                .agent
                .system_prompt
                .as_deref()
                .unwrap_or("You are {{agent.name}}, an autonomous AI assistant.");

            let base_row = db_history
                .iter()
                .enumerate()
                .find(|(_, h)| h.role == "system")
                .map(|(idx, h)| (idx, h.source_template.clone(), h.tool_calls_json.clone()));

            if let Some((idx, stored_template, _)) = base_row {
                // Rows written before `source_template` existed have no
                // fingerprint; leave those alone rather than risk clobbering a
                // distillation/compaction summary row.
                let changed = stored_template
                    .as_deref()
                    .map(|stored| stored != base_template)
                    .unwrap_or(false);

                if changed {
                    let rendered = interpolate_prompt(
                        base_template,
                        &self.agent.id,
                        &self.agent.name,
                        &self.agent.strategy,
                        &current_mem.content,
                        &current_mem.hash,
                        &allowed_tools,
                        max_history_chars,
                    );

                    match self
                        .db
                        .replace_base_system_prompt(&self.agent.id, &rendered, base_template)
                        .await
                    {
                        Ok(true) => {
                            db_history[idx].content = rendered;
                            self.record_transcript(
                                "system",
                                &db_history[idx].content,
                                None,
                                None,
                                "instruction_refresh",
                                turn_number,
                            )
                            .await;
                        }
                        Ok(false) => {}
                        Err(e) => warn!("Failed to refresh base system prompt: {}", e),
                    }
                }
            }
        }

        // Repair interrupted turns left in history. A previous turn may have
        // died (crash, shutdown, provider error, a `compact` in a multi-call
        // turn) leaving an `assistant` message whose `tool_calls` were never
        // answered, replies that arrive after a later assistant message, or
        // duplicates. Those survive into the next request and providers reject
        // them ("insufficient tool messages following tool_calls", Gemini's
        // "functionResponse turn ... order").
        //
        // We normalise the transcript into canonical form: every assistant
        // tool-call group is immediately followed by exactly one reply per id,
        // in id order, before any other assistant message; replies that were
        // misplaced (answered later) are moved up; unknown/duplicate replies
        // become system context. The result is rewritten only if it differs.
        let normalized = normalize_history_transcript(&db_history);
        let mut messages_override: Option<Vec<Value>> = None;
        if normalized_changed(&db_history, &normalized) {
            match self
                .db
                .replace_agent_history(&self.agent.id, &normalized)
                .await
            {
                Ok(()) => {
                    warn!(
                        "Repaired inconsistent tool-call history for agent '{}' ({} rows)",
                        self.agent.id,
                        normalized.len()
                    );
                    db_history = self
                        .db
                        .get_agent_history(&self.agent.id)
                        .await
                        .unwrap_or_else(|_| {
                            messages_override = Some(normalized.clone());
                            Vec::new()
                        });
                }
                Err(e) => {
                    // Fall back to the repaired transcript in memory only; the
                    // outgoing request is still valid for this turn.
                    warn!("Failed to persist repaired history: {}", e);
                    messages_override = Some(normalized);
                }
            }
        }

        // Fetch and inject any unconsumed Push MCP updates / Subscriptions
        let unconsumed_pushed = self
            .db
            .list_unconsumed_pushed_messages(&self.agent.id)
            .await
            .unwrap_or_default();

        if !unconsumed_pushed.is_empty() {
            // Insert into working context, append to the immutable transcript
            // and mark the events consumed in ONE transaction, so a crash can
            // never leave an event both consumed and absent from the context.
            match self
                .db
                .consume_pushed_into_history(
                    &self.agent.id,
                    |msg| self.format_pushed_message(msg),
                    turn_number,
                )
                .await
            {
                Ok(Some(pushed_item)) => {
                    info!(
                        "Injected {} unconsumed MCP push event(s) for agent '{}'",
                        unconsumed_pushed.len(),
                        self.agent.id
                    );
                    db_history.push(pushed_item);
                }
                Ok(None) => {}
                Err(e) => warn!("Failed to inject pushed events: {}", e),
            }
        }

        // Append user message to database history
        let user_item = self
            .db
            .append_agent_history(&self.agent.id, "user", user_input, None, None, None)
            .await
            .map_err(|e| e.to_string())?;
        db_history.push(user_item);
        self.record_transcript("user", user_input, None, None, "turn", turn_number)
            .await;

        // Convert DB history items to OpenAI messages format and sanitize.
        // If the repaired transcript could not be persisted, use it directly
        // for this turn so the request is still well formed.
        let raw_messages: Vec<Value> = match messages_override {
            Some(overridden) => {
                let mut out = overridden;
                out.push(json!({ "role": "user", "content": user_input }));
                out
            }
            None => db_history
                .iter()
                .map(|item| {
                    let mut obj = json!({
                        "role": item.role,
                        "content": item.content,
                    });
                    if let Some(ref tc_str) = item.tool_calls_json {
                        if let Ok(tc_val) = serde_json::from_str::<Value>(tc_str) {
                            obj["tool_calls"] = tc_val;
                        }
                    }
                    if let Some(ref tid) = item.tool_call_id {
                        obj["tool_call_id"] = Value::String(tid.clone());
                    }
                    obj
                })
                .collect(),
        };
        let mut messages = sanitize_chat_messages(raw_messages);
        live_chars.store(
            Self::measure_messages(&messages),
            std::sync::atomic::Ordering::Relaxed,
        );

        let router = ProviderRouter::new(self.models.clone(), &self.agent.strategy);
        let meta_tools = get_meta_tools_definitions();

        let mut final_response = String::new();
        let mut turn_superseded = false;
        let mut iteration: u64 = 0;
        // Counts consecutive model replies that contained no tool call while
        // output text was disallowed. Reset by any tool call.
        let mut text_rejections: u32 = 0;
        // Set when the agent finished a sleep and queued work is waiting: the
        // natural end of this turn, so the batch can run next.
        let mut yield_to_batch = false;

        // A turn is deliberately UNBOUNDED.
        //
        // The agent is built for long horizons: it paces itself with `sleep`
        // (which blocks inside the turn and is interruptible), reacts to push
        // events, and decides for itself when it is done. A fixed iteration
        // cap silently truncated that work mid-thought — the turn just fell out
        // of the loop and reported success with whatever partial text it had.
        //
        // The loop ends only when:
        //   * the agent returns a final answer (and output text is allowed),
        //   * a newer turn supersedes this one (`turn_superseded`), or
        //   * a fatal error returns early.
        //
        // There is no iteration limit. A long-running turn is observable via
        // the periodic progress log below and cancellable by sending the agent
        // a new message, which sets `turn_superseded`.
        loop {
            iteration += 1;
            if turn_superseded || yield_to_batch {
                break;
            }
            if iteration % 100 == 0 {
                info!(
                    "agent {} turn {} still running after {} iterations",
                    self.agent.id, turn_number, iteration
                );
            }
            // Publish live context pressure for event headers (MCP push tasks
            // read this from a detached spawned task).
            live_chars.store(
                Self::measure_messages(&messages),
                std::sync::atomic::Ordering::Relaxed,
            );

            let agent_id_for_event = self.agent.id.clone();
            let strategy_name = self.agent.strategy.clone();
            let event_tx_clone = self.event_tx.clone();

            let sanitized_for_call = sanitize_chat_messages(messages.clone());
            let completion_result = router
                .execute_chat_completion(
                    sanitized_for_call,
                    Some(meta_tools.clone()),
                    move |target| {
                        let envelope = EventEnvelope::new(
                            &agent_id_for_event,
                            AgentEvent::ProviderSelected {
                                agent_id: agent_id_for_event.clone(),
                                provider_id: target.provider_id.clone(),
                                provider_name: target.provider_name.clone(),
                                model: target.model_id.clone(),
                                strategy: strategy_name.clone(),
                            },
                        );
                        let _ = event_tx_clone.send(envelope);
                    },
                )
                .await;

            let (response_json, _chosen_provider) = match completion_result {
                Ok(res) => res,
                Err(e) => {
                    let err_msg = format!("LLM completion failed: {}", e);
                    self.emit(AgentEvent::AgentError {
                        agent_id: self.agent.id.clone(),
                        message: err_msg.clone(),
                    });
                    return Err(err_msg);
                }
            };

            let choice = response_json["choices"]
                .as_array()
                .and_then(|arr| arr.first())
                .ok_or_else(|| "Empty choices array from LLM".to_string())?;

            let message = &choice["message"];
            let tool_calls = message.get("tool_calls").and_then(|tc| tc.as_array());
            let has_tool_calls = tool_calls.map(|tc| !tc.is_empty()).unwrap_or(false);
            let content_str = message["content"].as_str().unwrap_or("").trim();

            // Check Output Text Permission when no tool calls are made
            if !has_tool_calls {
                if !self.agent.allow_output_text {
                    let error_msg = self.agent.output_text_error_message.as_deref().unwrap_or(
                        "Output Text is not allowed, consider use correct tool or sleep.",
                    );

                    self.emit(AgentEvent::TextDisallowed {
                        agent_id: self.agent.id.clone(),
                        error_message: error_msg.to_string(),
                    });

                    // Append assistant message, then feed rejection error message back to the LLM
                    messages.push(message.clone());
                    let user_rejection = json!({
                        "role": "user",
                        "content": error_msg
                    });
                    messages.push(user_rejection);

                    let _ = self
                        .db
                        .append_agent_history(
                            &self.agent.id,
                            "assistant",
                            content_str,
                            None,
                            None,
                            None,
                        )
                        .await;
                    self.record_transcript(
                        "assistant",
                        content_str,
                        None,
                        None,
                        "turn",
                        turn_number,
                    )
                    .await;

                    let _ = self
                        .db
                        .append_agent_history(&self.agent.id, "user", error_msg, None, None, None)
                        .await;
                    self.record_transcript("user", error_msg, None, None, "turn", turn_number)
                        .await;

                    text_rejections += 1;
                    if text_rejections >= MAX_CONSECUTIVE_TEXT_REJECTIONS {
                        // The model is not cooperating with the tool-only
                        // policy. Stop loudly instead of looping forever.
                        let msg = format!(
                            "Aborting turn after {} consecutive replies with no tool call. \
                             This agent is tool-only: call a tool, or `sleep` if there is \
                             nothing to do.",
                            text_rejections
                        );
                        warn!("agent {}: {}", self.agent.id, msg);
                        self.emit(AgentEvent::TurnCompleted {
                            agent_id: self.agent.id.clone(),
                            turn_id: turn_id.clone(),
                            content: msg.clone(),
                        });
                        return Err(msg);
                    }

                    continue;
                } else {
                    final_response = content_str.to_string();
                    messages.push(message.clone());
                    let _ = self
                        .db
                        .append_agent_history(
                            &self.agent.id,
                            "assistant",
                            content_str,
                            None,
                            None,
                            None,
                        )
                        .await;
                    self.record_transcript(
                        "assistant",
                        content_str,
                        None,
                        None,
                        "turn",
                        turn_number,
                    )
                    .await;
                    break;
                }
            }

            // Otherwise, assistant made tool calls. Real progress: any tool
            // call (including `sleep`) clears the no-progress counter.
            text_rejections = 0;
            messages.push(message.clone());
            let tool_calls_json_str = tool_calls.map(|tc| Value::Array(tc.clone()).to_string());
            let _ = self
                .db
                .append_agent_history(
                    &self.agent.id,
                    "assistant",
                    content_str,
                    tool_calls_json_str.as_deref(),
                    None,
                    None,
                )
                .await;
            self.record_transcript(
                "assistant",
                content_str,
                tool_calls_json_str.as_deref(),
                None,
                "turn",
                turn_number,
            )
            .await;

            if let Some(tool_calls) = tool_calls {
                for call in tool_calls {
                    let call_id = call["id"].as_str().unwrap_or("call_0").to_string();
                    let fn_name = call["function"]["name"].as_str().unwrap_or("");
                    let fn_args_str = call["function"]["arguments"].as_str().unwrap_or("{}");
                    // Models routinely emit slightly malformed argument JSON.
                    // Turning that into `{}` re-attributes the failure to the
                    // tool ("missing 'query'"), so the model retries the same
                    // broken call forever. Repair it instead, and when the
                    // repair fails say *that*.
                    let fn_args = match serde_json::from_str::<Value>(fn_args_str) {
                        Ok(value) if value.is_object() => value,
                        Ok(value) => {
                            // A model sometimes double-encodes the object as a
                            // string; unwrap that one level.
                            match value
                                .as_str()
                                .and_then(|inner| serde_json::from_str::<Value>(inner).ok())
                            {
                                Some(inner) if inner.is_object() => inner,
                                _ => json!({}),
                            }
                        }
                        Err(_) => json!({}),
                    };
                    let fn_args_parse_error = fn_args_str.trim() != "{}"
                        && serde_json::from_str::<Value>(fn_args_str).is_err();

                    if turn_superseded {
                        let err_val = json!({
                            "error": "This tool call was not executed because the turn was interrupted."
                        });
                        let (event_payload, event_truncated) =
                            self.truncate_event_payload(&err_val);
                        self.emit_with_context(
                            AgentEvent::ToolCallCompleted {
                                agent_id: self.agent.id.clone(),
                                call_id: call_id.clone(),
                                tool_name: fn_name.to_string(),
                                result: event_payload,
                                success: false,
                                response_id: None,
                                truncated: event_truncated,
                            },
                            &messages,
                        );
                        let tool_content_str = err_val.to_string();
                        let _ = self
                            .db
                            .append_agent_history(
                                &self.agent.id,
                                "tool",
                                &tool_content_str,
                                None,
                                Some(&call_id),
                                None,
                            )
                            .await;
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "name": fn_name,
                            "content": tool_content_str
                        }));
                        continue;
                    }

                    self.emit(AgentEvent::ToolCallStarted {
                        agent_id: self.agent.id.clone(),
                        call_id: call_id.clone(),
                        tool_name: fn_name.to_string(),
                        arguments: fn_args.clone(),
                    });

                    let mut was_compacted = false;
                    let mut response_id: Option<String> = None;
                    // Per-call truncation override supplied to `call_tool`.
                    let mut truncation_override: Option<
                        crate::runtime::meta_tools::TruncationConfig,
                    > = None;
                    // A call whose arguments could not be parsed is answered
                    // immediately with an actionable error. Dispatching it with
                    // `{}` would blame the tool for a malformed request and
                    // teach the model the wrong cause.
                    let (result_value, success) = if fn_args_parse_error {
                        (
                            json!({
                                "error": {
                                    "code": "invalid_arguments",
                                    "message": format!(
                                        "The arguments for '{fn_name}' were not valid JSON, so the call was not executed."
                                    ),
                                    "retryable": true,
                                    "hint": "Re-emit the tool call with a JSON object as its arguments. Do not double-encode the object as a string."
                                }
                            }),
                            false,
                        )
                    } else {
                        match fn_name {
                            "search_tools" => {
                                let query = fn_args["query"].as_str().unwrap_or("").to_lowercase();
                                let limit = fn_args["limit"].as_u64().unwrap_or(10) as usize;

                                let mut matches = vec![];
                                for (unique_name, entry) in &tool_registry {
                                    let desc = entry
                                        .tool
                                        .description
                                        .as_deref()
                                        .unwrap_or("")
                                        .to_lowercase();
                                    if unique_name.to_lowercase().contains(&query)
                                        || desc.contains(&query)
                                        || entry.original_tool_name.to_lowercase().contains(&query)
                                    {
                                        matches.push(json!({
                                            "name": unique_name,
                                            "server_alias": entry.server_alias,
                                            "original_name": entry.original_tool_name,
                                            "description": entry.tool.description
                                        }));
                                    }
                                    if matches.len() >= limit {
                                        break;
                                    }
                                }

                                self.emit(AgentEvent::ToolSearch {
                                    agent_id: self.agent.id.clone(),
                                    query: query.clone(),
                                    results_count: matches.len(),
                                });

                                // Distinguish "no such tool" from "a plugin
                                // failed to enumerate". A bare empty list made
                                // a transient plugin failure look like the tool
                                // did not exist, which is what caused the agent
                                // to retry identical searches.
                                let mut out = json!({ "tools": matches });
                                if !unreachable_servers.is_empty() {
                                    out["plugins_unavailable"] = json!(unreachable_servers);
                                    out["note"] = json!(
                                        "Some plugins could not be queried this turn;                                          their tools may exist but are not shown.                                          Retry, or inspect them with `get_tool_schema`."
                                    );
                                }
                                (out, true)
                            }

                            "get_tool_schema" => {
                                let name = fn_args["name"].as_str().unwrap_or("");
                                self.emit(AgentEvent::ToolSchemaFetched {
                                    agent_id: self.agent.id.clone(),
                                    tool_name: name.to_string(),
                                });

                                match resolve_tool(&tool_registry, name) {
                                    Ok(entry) => {
                                        let unique_name = format!(
                                            "{}.{}",
                                            entry.server_alias, entry.original_tool_name
                                        );
                                        (
                                            json!({
                                                "name": unique_name,
                                                "original_name": entry.original_tool_name,
                                                "server_alias": entry.server_alias,
                                                "description": entry.tool.description,
                                                "inputSchema": entry.tool.input_schema
                                            }),
                                            true,
                                        )
                                    }
                                    Err(err) => (json!({ "error": err }), false),
                                }
                            }

                            "call_tool" => {
                                let name = fn_args["name"].as_str().unwrap_or("");
                                let arguments =
                                    fn_args.get("arguments").cloned().unwrap_or(json!({}));

                                // Optional per-call head/body/tail overrides. Any
                                // omitted segment falls back to the agent default.
                                let default_cfg = self.truncation_config();
                                let pick = |key: &str, fallback: usize| -> usize {
                                    fn_args
                                        .get(key)
                                        .and_then(|v| v.as_u64())
                                        .map(|v| v as usize)
                                        .unwrap_or(fallback)
                                };
                                if fn_args.get("head").is_some()
                                    || fn_args.get("body").is_some()
                                    || fn_args.get("tail").is_some()
                                {
                                    truncation_override =
                                        Some(crate::runtime::meta_tools::TruncationConfig {
                                            head: pick("head", default_cfg.head),
                                            body: pick("body", default_cfg.body),
                                            tail: pick("tail", default_cfg.tail),
                                        });
                                }

                                match resolve_tool(&tool_registry, name) {
                                    Ok(entry) => {
                                        // Arguments are forwarded exactly as the model
                                        // supplied them. The runtime must not know any
                                        // plugin's schema; tools that need the caller
                                        // read the host-stamped `_sender_*` metadata that
                                        // the HTTP proxy adds.
                                        //
                                        // Agent-initiated calls have no authenticated
                                        // human behind them, so nothing is stamped here
                                        // and plugins must treat a missing sender as
                                        // "the agent itself".
                                        if let Some(session) = sessions.get(&entry.server_id) {
                                            let mut final_args = arguments;
                                            if let Some(map) = final_args.as_object_mut() {
                                                map.entry("agent_id".to_string())
                                                    .or_insert_with(|| json!(self.agent.id));
                                            }
                                            match session
                                                .call_tool(&entry.original_tool_name, final_args)
                                                .await
                                            {
                                                Ok(res) => (res, true),
                                                Err(e) => {
                                                    (json!({ "error": e.to_string() }), false)
                                                }
                                            }
                                        } else {
                                            (
                                                json!({ "error": "MCP server session not active" }),
                                                false,
                                            )
                                        }
                                    }
                                    Err(err) => (json!({ "error": err }), false),
                                }
                            }

                            "history_search" => {
                                let pattern = fn_args["regex"].as_str().unwrap_or("");
                                let limit = fn_args["limit"].as_u64().unwrap_or(20) as usize;

                                if pattern.is_empty() {
                                    (json!({ "error": "regex must not be empty" }), false)
                                } else {
                                    match self
                                        .db
                                        .search_transcript(&self.agent.id, pattern, limit)
                                        .await
                                    {
                                        Ok(hits) => {
                                            let count = hits.len();
                                            (json!({ "matches": hits, "count": count }), true)
                                        }
                                        Err(e) => (
                                            json!({ "error": format!("history_search failed: {}", e) }),
                                            false,
                                        ),
                                    }
                                }
                            }

                            "history_inspect" => {
                                let id = fn_args["id"].as_str().unwrap_or("");
                                let from = fn_args["from"].as_u64().unwrap_or(0) as usize;
                                let to = fn_args["to"]
                                    .as_u64()
                                    .map(|t| t as usize)
                                    .unwrap_or(from.saturating_add(2000));

                                if id.is_empty() {
                                    (json!({ "error": "id must not be empty" }), false)
                                } else {
                                    match self
                                        .db
                                        .inspect_transcript(&self.agent.id, id, from, to)
                                        .await
                                    {
                                        Ok(Some((total, slice))) => (
                                            json!({
                                                "id": id,
                                                "from": from,
                                                "to": to,
                                                "total": total,
                                                "content": slice,
                                            }),
                                            true,
                                        ),
                                        Ok(None) => (
                                            json!({ "error": format!("No history entry with id '{}'", id) }),
                                            false,
                                        ),
                                        Err(e) => (
                                            json!({ "error": format!("history_inspect failed: {}", e) }),
                                            false,
                                        ),
                                    }
                                }
                            }

                            "list_servers" => {
                                let list: Vec<Value> = self
                                    .mcp_servers
                                    .iter()
                                    .map(|s| {
                                        json!({
                                            "id": s.id,
                                            "name": s.name,
                                            "transport": s.transport,
                                            "active": sessions.contains_key(&s.id)
                                        })
                                    })
                                    .collect();
                                (json!({ "servers": list }), true)
                            }

                            "setMemory" | "set_memory" | "instructionsWrite" | "instructions_write" => {
                                let content_str = fn_args["content"].as_str().unwrap_or("");
                                let char_count = content_str.chars().count();

                                if char_count > MAX_MEMORY_CHARS {
                                    (
                                        json!({
                                            "error": format!(
                                                "Memory limit exceeded: new content has {} characters, exceeding the 5,000 character maximum limit. Rejected.",
                                                char_count
                                            )
                                        }),
                                        false,
                                    )
                                } else {
                                    let provided_hash = fn_args
                                        .get("currentHash")
                                        .or_else(|| fn_args.get("current_hash"))
                                        .and_then(|h| h.as_str())
                                        .unwrap_or("");

                                    let mem =
                                        self.db.get_agent_memory(&self.agent.id).await.unwrap();

                                    if mem.hash != provided_hash {
                                        (
                                            json!({
                                                "error": format!(
                                                    "Hash mismatch: provided currentHash '{}' does not match actual memory hash '{}'. The memory may have been modified. Please call instructionsRead() to inspect the current untruncated content and hash.",
                                                    provided_hash, mem.hash
                                                )
                                            }),
                                            false,
                                        )
                                    } else {
                                        let new_hash = calculate_memory_hash(content_str);
                                        // A discarded write result here would report
                                        // success for memory that was never stored --
                                        // the exact silent-degradation shape that makes
                                        // an agent distrust a correct call. Propagate.
                                        match self
                                            .db
                                            .set_agent_memory(
                                                &self.agent.id,
                                                content_str,
                                                &new_hash,
                                            )
                                            .await
                                        {
                                            Ok(_) => {
                                                self.emit(AgentEvent::MemoryUpdated {
                                                    agent_id: self.agent.id.clone(),
                                                    updated_hash: new_hash.clone(),
                                                    char_count,
                                                });

                                                (
                                                    json!({
                                                        "success": true,
                                                        "updated_hash": new_hash,
                                                        "character_count": char_count
                                                    }),
                                                    true,
                                                )
                                            }
                                            Err(e) => (
                                                json!({
                                                    "error": format!(
                                                        "Memory was not saved: {e}. Nothing was stored; retry the call."
                                                    )
                                                }),
                                                false,
                                            ),
                                        }
                                    }
                                }
                            }

                            "editMemory" | "edit_memory" | "instructionsEdit" | "instructions_edit" => {
                                let search = fn_args["search"].as_str().unwrap_or("");
                                let replace = fn_args["replace"].as_str().unwrap_or("");
                                let provided_hash = fn_args
                                    .get("currentHash")
                                    .or_else(|| fn_args.get("current_hash"))
                                    .and_then(|h| h.as_str())
                                    .unwrap_or("");

                                let mem = self.db.get_agent_memory(&self.agent.id).await.unwrap();

                                if mem.hash != provided_hash {
                                    (
                                        json!({
                                            "error": format!(
                                                "Hash mismatch: provided currentHash '{}' does not match actual memory hash '{}'. The memory may have been modified. Please call instructionsRead() to inspect the current untruncated content and hash.",
                                                provided_hash, mem.hash
                                            )
                                        }),
                                        false,
                                    )
                                } else if !mem.content.contains(search) {
                                    (
                                        json!({
                                            "error": format!(
                                                "Search segment '{}' not found in current memory.",
                                                search
                                            )
                                        }),
                                        false,
                                    )
                                } else {
                                    let new_content = mem.content.replace(search, replace);
                                    let char_count = new_content.chars().count();

                                    if char_count > MAX_MEMORY_CHARS {
                                        (
                                            json!({
                                                "error": format!(
                                                    "Memory limit exceeded: resulting content has {} characters, exceeding the 5,000 character maximum limit. Rejected.",
                                                    char_count
                                                )
                                            }),
                                            false,
                                        )
                                    } else {
                                        let new_hash = calculate_memory_hash(&new_content);
                                        // Same rule as setMemory: never report a
                                        // write that did not happen.
                                        match self
                                            .db
                                            .set_agent_memory(
                                                &self.agent.id,
                                                &new_content,
                                                &new_hash,
                                            )
                                            .await
                                        {
                                            Ok(_) => {
                                                self.emit(AgentEvent::MemoryUpdated {
                                                    agent_id: self.agent.id.clone(),
                                                    updated_hash: new_hash.clone(),
                                                    char_count,
                                                });

                                                (
                                                    json!({
                                                        "success": true,
                                                        "updated_hash": new_hash,
                                                        "character_count": char_count
                                                    }),
                                                    true,
                                                )
                                            }
                                            Err(e) => (
                                                json!({
                                                    "error": format!(
                                                        "Memory edit was not saved: {e}. Nothing changed; retry the call."
                                                    )
                                                }),
                                                false,
                                            ),
                                        }
                                    }
                                }
                            }

                            "readMemory" | "read_memory" | "instructionsRead" | "instructions_read" => {
                                // Never panic a tool call: a DB hiccup must surface
                                // as a retryable error, not a killed turn.
                                match self.db.get_agent_memory(&self.agent.id).await {
                                    Ok(mem) => {
                                        let char_count = mem.content.chars().count();

                                        self.emit(AgentEvent::MemoryRead {
                                            agent_id: self.agent.id.clone(),
                                            current_hash: mem.hash.clone(),
                                            char_count,
                                        });

                                        (
                                            json!({
                                                "content": mem.content,
                                                "current_hash": mem.hash,
                                                "character_count": char_count
                                            }),
                                            true,
                                        )
                                    }
                                    Err(e) => (
                                        json!({
                                            "error": format!(
                                                "Could not read memory: {e}. This is a host storage error; retry the call."
                                            )
                                        }),
                                        false,
                                    ),
                                }
                            }

                            "memoryWrite" | "memory_write" | "memoryNodeWrite" | "memory_node_write" => self.memory_write(&fn_args).await,

                            "memoryRead" | "memory_read" | "memoryNodeRead" | "memory_node_read" => self.memory_read(&fn_args).await,

                            "memorySearch" | "memory_search" | "memoryNodeSearch" | "memory_node_search" => self.memory_search(&fn_args).await,

                            "memoryList" | "memory_list" | "memoryNodeList" | "memory_node_list" => self.memory_list(&fn_args).await,

                            "memoryDelete" | "memory_delete" | "memoryNodeDelete" | "memory_node_delete" => self.memory_delete(&fn_args).await,

                            "skillWrite" | "skill_write" => self.skill_write(&fn_args).await,

                            "skillList" | "skill_list" => self.skill_list().await,

                            "compact" => {
                                let mem = match self.db.get_agent_memory(&self.agent.id).await {
                                    Ok(mem) => mem,
                                    Err(e) => {
                                        return Err(format!(
                                            "compaction aborted: could not read agent memory: {e}"
                                        ));
                                    }
                                };

                                // Compaction is the designated context-rebuild
                                // point: re-render the system instruction from the
                                // agent's CURRENT system_prompt (which is where a
                                // selected instruction preset lands), memory and
                                // tool list, then install it as the new row 0.
                                // This is the only place a changed preset becomes
                                // live for an already-running agent, and it costs
                                // no cache hit because history is rebuilt anyway.
                                let base_template = self.agent.system_prompt.as_deref().unwrap_or(
                                    "You are {{agent.name}}, an autonomous AI assistant.",
                                );

                                let rendered_instruction = interpolate_prompt(
                                    base_template,
                                    &self.agent.id,
                                    &self.agent.name,
                                    &self.agent.strategy,
                                    &mem.content,
                                    &mem.hash,
                                    &allowed_tools,
                                    max_history_chars,
                                );

                                let summary = format!(
                                    "Earlier turns were compacted by the agent.\n\
                                 Memory hash: {}\n\
                                 Memory:\n{}\n\
                                 You must continue by calling a tool. If you have\
                                 nothing to do right now, call `sleep`.",
                                    mem.hash, mem.content
                                );

                                let tool_reply = json!({
                                "success": true,
                                "message": "Running context compacted into a summary; original system prompt preserved.",
                                "current_hash": mem.hash
                            })
                            .to_string();

                                // Build the entire tool-call group up front: the
                                // assistant message may carry sibling calls, and
                                // every one of them must be answered adjacently.
                                let skipped_notice = |name: &str| {
                                    json!({
                                    "error": format!(
                                        "Tool call '{}' was skipped: history was compacted by this turn's earlier tool call.",
                                        name
                                    )
                                })
                                .to_string()
                                };

                                let mut group_calls: Vec<Value> = Vec::new();
                                let mut replies: Vec<(String, String)> = Vec::new();
                                for sibling in tool_calls.iter() {
                                    let sib_id =
                                        sibling["id"].as_str().unwrap_or("call_0").to_string();
                                    let sib_name = sibling["function"]["name"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    let sib_content = if sib_id == call_id {
                                        tool_reply.clone()
                                    } else {
                                        skipped_notice(&sib_name)
                                    };
                                    group_calls.push(sibling.clone());
                                    replies.push((sib_id, sib_content));
                                }
                                if group_calls.is_empty() {
                                    group_calls.push(call.clone());
                                    replies.push((call_id.clone(), tool_reply.clone()));
                                }

                                let assistant_group = json!({
                                    "role": "assistant",
                                    "content": "",
                                    "tool_calls": group_calls
                                });

                                // Persist atomically in provider-valid order:
                                // [base] [assistant] [tool replies] [summary]
                                let _ = self
                                    .db
                                    .compact_agent_history(
                                        &self.agent.id,
                                        &rendered_instruction,
                                        &summary,
                                        Some(&assistant_group),
                                        &replies,
                                    )
                                    .await;

                                // Archived, never deleted: distillation keeps the
                                // full pre-compaction sequence.
                                self.record_transcript(
                                    "system",
                                    &format!("### [Compaction Summary]\n{}", summary),
                                    None,
                                    None,
                                    "compaction",
                                    turn_number,
                                )
                                .await;

                                // In-memory context mirrors the persisted order:
                                // row 0 is the freshly rendered instruction.
                                let mut group_msgs: Vec<Value> = vec![json!({
                                    "role": "system",
                                    "content": rendered_instruction
                                })];
                                group_msgs.push(assistant_group);
                                for (cid, content) in replies.iter() {
                                    group_msgs.push(json!({
                                        "role": "tool",
                                        "tool_call_id": cid,
                                        "content": content
                                    }));
                                }
                                group_msgs.push(json!({
                                    "role": "system",
                                    "content": format!("### [Compaction Summary]\n{}", summary)
                                }));

                                messages = group_msgs;

                                was_compacted = true;

                                self.emit(AgentEvent::MemoryCompacted {
                                    agent_id: self.agent.id.clone(),
                                    current_hash: mem.hash.clone(),
                                });

                                (
                                    json!({
                                        "success": true,
                                        "message": "Running history compacted to system prompt, memory, and currentHash.",
                                        "current_hash": mem.hash
                                    }),
                                    true,
                                )
                            }

                            "sleep" => {
                                let duration_str = fn_args
                                    .get("duration")
                                    .or_else(|| fn_args.get("time"))
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("");

                                let interruptible = fn_args
                                    .get("interruptible")
                                    .and_then(|i| i.as_bool())
                                    .unwrap_or(false);

                                match parse_duration_string(duration_str) {
                                    Ok(dur) => {
                                        self.emit(AgentEvent::AgentSleeping {
                                            agent_id: self.agent.id.clone(),
                                            duration: duration_str.to_string(),
                                            interruptible,
                                        });

                                        let (cancel_tx, mut cancel_rx) =
                                            tokio::sync::oneshot::channel::<String>();
                                        // Strictly enforce single timer: cancelling any prior timer for this agent
                                        global_timer_manager().register_timer(
                                            &self.agent.id,
                                            cancel_tx,
                                            dur.as_secs(),
                                        );

                                        let start_time = std::time::Instant::now();
                                        let was_interrupted: bool;
                                        let wake_reason: String;

                                        tokio::select! {
                                            _ = tokio::time::sleep(dur) => {
                                                global_timer_manager().clear_timer(&self.agent.id);
                                                was_interrupted = false;
                                                wake_reason = "timer_completed".to_string();
                                            }
                                            Ok(reason) = &mut cancel_rx => {
                                                was_interrupted = true;
                                                wake_reason = format!("interrupted: {}", reason);
                                                if reason == "new turn started" {
                                                    turn_superseded = true;
                                                }
                                            }
                                        }

                                        let elapsed_secs = start_time.elapsed().as_secs();

                                        // `sleep` is the agent's own declaration
                                        // that it has nothing to do right now, so
                                        // it is the natural boundary for a queued
                                        // batch: finish this turn and let the
                                        // runner start the next one with the
                                        // messages that arrived meanwhile. The
                                        // turn itself is never interrupted.
                                        if !was_interrupted
                                            && global_turn_queue()
                                                .pending_len(&self.agent.id)
                                                > 0
                                        {
                                            yield_to_batch = true;
                                        }

                                        self.emit(AgentEvent::AgentWokeUp {
                                            agent_id: self.agent.id.clone(),
                                            reason: wake_reason.clone(),
                                        });

                                        (
                                            json!({
                                                "success": true,
                                                "duration": duration_str,
                                                "slept_seconds": elapsed_secs,
                                                "interrupted": was_interrupted,
                                                "timer_cancelled": was_interrupted,
                                                "wake_reason": wake_reason
                                            }),
                                            true,
                                        )
                                    }
                                    Err(err_str) => (json!({ "error": err_str }), false),
                                }
                            }

                            other => (
                                json!({ "error": format!("Unknown meta-tool '{}'", other) }),
                                false,
                            ),
                        }
                    };

                    // Emit only once the transcript row exists so the event can
                    // carry `response_id`; the payload is truncated for the
                    // event stream, exactly like the working context.
                    let (event_payload, event_truncated) =
                        self.truncate_event_payload(&result_value);
                    self.emit_with_context(
                        AgentEvent::ToolCallCompleted {
                            agent_id: self.agent.id.clone(),
                            call_id: call_id.clone(),
                            tool_name: fn_name.to_string(),
                            result: event_payload,
                            success,
                            response_id: response_id.clone(),
                            truncated: event_truncated,
                        },
                        &messages,
                    );

                    if was_compacted {
                        // Compaction replaced `messages`; make sure every other
                        // tool call of the same assistant turn is acknowledged.
                        append_missing_tool_responses(
                            &mut messages,
                            message,
                            Some(call_id.as_str()),
                            "history was compacted before this tool call was executed",
                        );
                    } else {
                        let current_chars: usize = messages
                            .iter()
                            .map(|m| {
                                let content_len = m["content"]
                                    .as_str()
                                    .map(|s| s.chars().count())
                                    .unwrap_or(0);
                                let tc_len = m
                                    .get("tool_calls")
                                    .map(|tc| tc.to_string().chars().count())
                                    .unwrap_or(0);
                                content_len + tc_len
                            })
                            .sum();

                        let enabled_hooks = self.agent.get_response_hooks();
                        let rendered_hooks =
                            render_response_hooks(&enabled_hooks, current_chars, max_history_chars);

                        // Full-fidelity text: goes to the append-only
                        // transcript, which is the source for distillation and
                        // for `history_inspect`.
                        let full_content_str = if !rendered_hooks.is_empty() {
                            format!(
                                "{}\n\n[Response Hook]\n{}",
                                result_value,
                                rendered_hooks.join("\n")
                            )
                        } else {
                            result_value.to_string()
                        };

                        response_id = self
                            .record_transcript(
                                "tool",
                                &full_content_str,
                                None,
                                Some(&call_id),
                                "tool",
                                turn_number,
                            )
                            .await;

                        // Working context keeps only a head/body/tail view so a
                        // single huge result cannot flood the window.
                        let truncated = self.truncate_for_context_with(
                            &full_content_str,
                            response_id.as_deref(),
                            truncation_override.unwrap_or_else(|| self.truncation_config()),
                        );

                        // Every tool response carries the UTC timestamp and the
                        // current context pressure so the agent can pace itself.
                        let tool_content_str = crate::runtime::meta_tools::with_context_header(
                            &truncated,
                            current_chars,
                            max_history_chars,
                        );

                        let _ = self
                            .db
                            .append_agent_history(
                                &self.agent.id,
                                "tool",
                                &tool_content_str,
                                None,
                                Some(&call_id),
                                None,
                            )
                            .await;

                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "name": fn_name,
                            "content": tool_content_str
                        }));

                        // Keep the shared gauge accurate right after the
                        // context grew, so a push generated now reports the
                        // true pressure rather than the previous iteration's.
                        live_chars.store(
                            Self::measure_messages(&messages),
                            std::sync::atomic::Ordering::Relaxed,
                        );

                        // Enforce max_running_history_chars limit
                        let mut total_chars: usize = messages
                            .iter()
                            .map(|m| {
                                let content_len = m["content"]
                                    .as_str()
                                    .map(|s| s.chars().count())
                                    .unwrap_or(0);
                                let tc_len = m
                                    .get("tool_calls")
                                    .map(|tc| tc.to_string().chars().count())
                                    .unwrap_or(0);
                                content_len + tc_len
                            })
                            .sum();

                        if total_chars > max_history_chars {
                            let _ = self
                                .db
                                .prune_agent_history_to_limit(&self.agent.id, max_history_chars)
                                .await;

                            // Trim whole atomic groups from the front (keeping the
                            // leading system message). Never remove an assistant
                            // `tool_calls` message without also removing every
                            // `tool` reply that belongs to it — a half removed
                            // group is rejected by the provider.
                            loop {
                                if messages.len() <= 1 || total_chars <= max_history_chars {
                                    break;
                                }

                                let mut group_len = 1usize;
                                let first = &messages[1];
                                if first.get("role").and_then(|r| r.as_str()) == Some("assistant") {
                                    let ids = assistant_tool_call_ids(first);
                                    if !ids.is_empty() {
                                        let pending: std::collections::HashSet<&str> =
                                            ids.iter().map(|s| s.as_str()).collect();
                                        while 1 + group_len < messages.len() {
                                            let follower = &messages[1 + group_len];
                                            let is_member =
                                                follower.get("role").and_then(|r| r.as_str())
                                                    == Some("tool")
                                                    && follower
                                                        .get("tool_call_id")
                                                        .and_then(|t| t.as_str())
                                                        .map(|id| pending.contains(id))
                                                        .unwrap_or(false);
                                            if !is_member {
                                                break;
                                            }
                                            group_len += 1;
                                        }
                                    }
                                }

                                for _ in 0..group_len {
                                    if messages.len() <= 1 {
                                        break;
                                    }
                                    let removed = messages.remove(1);
                                    let rem_len = removed["content"]
                                        .as_str()
                                        .map(|s| s.chars().count())
                                        .unwrap_or(0);
                                    let rem_tc = removed
                                        .get("tool_calls")
                                        .map(|tc| tc.to_string().chars().count())
                                        .unwrap_or(0);
                                    total_chars = total_chars.saturating_sub(rem_len + rem_tc);
                                }
                            }
                        }
                    }
                }
            }

            if turn_superseded {
                info!(
                    "Turn for agent {} was superseded by a newer turn; ending turn loop",
                    self.agent.id
                );
                break;
            }
        }

        self.emit(AgentEvent::TurnCompleted {
            agent_id: self.agent.id.clone(),
            turn_id,
            content: final_response.clone(),
        });

        Ok(final_response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_tool_name_strips_a_redundant_alias_prefix() {
        // Plugin `control` exposing `control_whoami` used to render as
        // `control.control_whoami`; the model then had to guess the shape.
        assert_eq!(compose_tool_name("control", "control_whoami"), "control.whoami");
        assert_eq!(
            compose_tool_name("telegram", "telegram_send_message"),
            "telegram.send_message"
        );
        // Not prefixed: leave the name alone.
        assert_eq!(compose_tool_name("chat", "send"), "chat.send");
        // A tool whose name merely starts with the alias but is not the
        // `<alias>_` prefix must not be truncated.
        assert_eq!(compose_tool_name("a", "ab"), "a.ab");
        // Degenerate: alias echoed exactly -> keep it rather than produce "x.".
        assert_eq!(compose_tool_name("x", "x_"), "x.x_");
    }

    #[test]
    fn test_resolve_tool_exact_and_fallback() {
        let mut registry = HashMap::new();
        registry.insert(
            "telegram.chat_send".to_string(),
            ToolRegistryEntry {
                server_id: "srv_tg".into(),
                server_alias: "telegram".into(),
                original_tool_name: "chat_send".into(),
                tool: McpTool {
                    name: "chat_send".into(),
                    description: Some("Send telegram message".into()),
                    input_schema: None,
                    meta: None,
                },
            },
        );

        // 1. Direct exact match with dot notation
        let resolved = resolve_tool(&registry, "telegram.chat_send").unwrap();
        assert_eq!(resolved.server_id, "srv_tg");
        assert_eq!(resolved.original_tool_name, "chat_send");

        // 2. Delimiter variants (e.g. double underscore, single underscore)
        let resolved_variant1 = resolve_tool(&registry, "telegram__chat_send").unwrap();
        assert_eq!(resolved_variant1.server_id, "srv_tg");

        let resolved_variant2 = resolve_tool(&registry, "telegram_chat_send").unwrap();
        assert_eq!(resolved_variant2.server_id, "srv_tg");

        // 3. Fallback when unambiguous
        let resolved_fallback = resolve_tool(&registry, "chat_send").unwrap();
        assert_eq!(resolved_fallback.server_id, "srv_tg");
    }

    #[test]
    fn test_resolve_tool_ambiguous_conflict() {
        let mut registry = HashMap::new();
        registry.insert(
            "telegram.chat_send".to_string(),
            ToolRegistryEntry {
                server_id: "srv_tg".into(),
                server_alias: "telegram".into(),
                original_tool_name: "chat_send".into(),
                tool: McpTool {
                    name: "chat_send".into(),
                    description: Some("Telegram".into()),
                    input_schema: None,
                    meta: None,
                },
            },
        );
        registry.insert(
            "discord.chat_send".to_string(),
            ToolRegistryEntry {
                server_id: "srv_dc".into(),
                server_alias: "discord".into(),
                original_tool_name: "chat_send".into(),
                tool: McpTool {
                    name: "chat_send".into(),
                    description: Some("Discord".into()),
                    input_schema: None,
                    meta: None,
                },
            },
        );

        // Calling exact alias works
        assert_eq!(
            resolve_tool(&registry, "telegram.chat_send")
                .unwrap()
                .server_id,
            "srv_tg"
        );
        assert_eq!(
            resolve_tool(&registry, "discord.chat_send")
                .unwrap()
                .server_id,
            "srv_dc"
        );

        // Calling naked chat_send should fail with ambiguous error
        let err = resolve_tool(&registry, "chat_send").unwrap_err();
        assert!(err.contains("Ambiguous tool 'chat_send'"));
        assert!(err.contains("discord.chat_send"));
        assert!(err.contains("telegram.chat_send"));
    }

    #[test]
    fn test_sanitize_chat_messages_orphaned_tools() {
        // Simulating the exact state that caused Gemini error:
        // [system, tool (call_43399), tool (call_43402), user]
        let raw = vec![
            json!({ "role": "system", "content": "You are Neuxbane." }),
            json!({ "role": "tool", "tool_call_id": "call_43399", "content": "{\"compacted\": true}" }),
            json!({ "role": "tool", "tool_call_id": "call_43402", "content": "chat sent" }),
            json!({ "role": "user", "content": "hello" }),
        ];

        let sanitized = sanitize_chat_messages(raw);
        assert_eq!(sanitized.len(), 4);
        assert_eq!(sanitized[0]["role"], "system");
        // Orphaned tool calls MUST be converted to system context to avoid Gemini functionResponse error
        assert_eq!(sanitized[1]["role"], "system");
        assert!(sanitized[1]["content"]
            .as_str()
            .unwrap()
            .contains("compacted"));
        assert_eq!(sanitized[2]["role"], "system");
        assert_eq!(sanitized[3]["role"], "user");
    }

    #[test]
    fn test_sanitize_chat_messages_valid_tool_call() {
        // Valid tool call: assistant has tool_calls matching tool message
        let raw = vec![
            json!({ "role": "user", "content": "hello" }),
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_123",
                    "type": "function",
                    "function": { "name": "search_tools", "arguments": "{}" }
                }]
            }),
            json!({ "role": "tool", "tool_call_id": "call_123", "content": "result" }),
        ];

        let sanitized = sanitize_chat_messages(raw);
        assert_eq!(sanitized.len(), 3);
        assert_eq!(sanitized[1]["role"], "assistant");
        assert_eq!(sanitized[2]["role"], "tool");
        // Ensure "name" is injected from assistant tool_call
        assert_eq!(sanitized[2]["name"], "search_tools");
    }

    /// Asserts both provider contracts at once:
    ///   * OpenAI adjacency: every assistant `tool_calls` message is followed
    ///     by exactly one `tool` message per call id, before any non-tool row;
    ///   * Gemini turn order: no id is answered twice, and every `tool` row
    ///     belongs to the group it is adjacent to.
    fn assert_valid_tool_transcript(messages: &[Value]) {
        let mut answered: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut i = 0;
        while i < messages.len() {
            let msg = &messages[i];
            let ids = if msg["role"].as_str() == Some("assistant") {
                assistant_tool_call_ids(msg)
            } else {
                Vec::new()
            };

            if ids.is_empty() {
                assert_ne!(
                    msg["role"].as_str(),
                    Some("tool"),
                    "orphaned tool message at index {} (id {:?}); transcript: {}",
                    i,
                    msg["tool_call_id"],
                    serde_json::to_string_pretty(messages).unwrap()
                );
                i += 1;
                continue;
            }

            // Walk the contiguous tool replies that follow.
            let mut group: Vec<String> = Vec::new();
            let mut j = i + 1;
            while j < messages.len() && messages[j]["role"].as_str() == Some("tool") {
                group.push(
                    messages[j]["tool_call_id"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                );
                j += 1;
            }

            for id in &ids {
                assert!(
                    group.iter().any(|g| g == id),
                    "assistant tool_call '{}' at index {} has no adjacent tool response; transcript: {}",
                    id,
                    i,
                    serde_json::to_string_pretty(messages).unwrap()
                );
            }
            for (k, gid) in group.iter().enumerate() {
                assert!(
                    ids.iter().any(|id| id == gid),
                    "tool reply '{}' at index {} is not part of the adjacent assistant group ({:?})",
                    gid,
                    i + 1 + k,
                    ids
                );
                assert!(
                    answered.insert(gid.clone()),
                    "tool_call_id '{}' answered more than once; transcript: {}",
                    gid,
                    serde_json::to_string_pretty(messages).unwrap()
                );
            }
            i = j;
        }
    }

    fn hist(
        role: &str,
        content: &str,
        tool_calls: Option<&str>,
        tool_call_id: Option<&str>,
    ) -> crate::db::schema::AgentHistoryItem {
        crate::db::schema::AgentHistoryItem {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: "agent".into(),
            role: role.into(),
            content: content.into(),
            tool_calls_json: tool_calls.map(|s| s.to_string()),
            tool_call_id: tool_call_id.map(|s| s.to_string()),
            source_template: None,
            created_at: None,
        }
    }

    #[test]
    fn test_normalize_recovers_real_reply_masked_by_synthetic() {
        // Exact corrupted shape produced by running `compact` twice in one
        // turn: summary written BEFORE the assistant message, a synthetic
        // "interrupted" placeholder as the tool reply, a duplicate summary,
        // and the REAL reply demoted to "[Previous Tool Context]".
        let calls = r#"[{"id":"call_compact","type":"function","function":{"name":"compact","arguments":"{}"}}]"#;
        let history = vec![
            hist("system", "You are Nexura.", None, None),
            hist("system", "### [Compaction Summary]\nold", None, None),
            hist("assistant", "", Some(calls), None),
            hist(
                "tool",
                "{\"error\":\"This tool call was not executed because the turn was interrupted.\"}",
                None,
                Some("call_compact"),
            ),
            hist("system", "### [Compaction Summary]\nold", None, None),
            hist(
                "system",
                "[Previous Tool Context]: {\"success\":true}",
                None,
                None,
            ),
            hist("user", "hello?", None, None),
        ];

        let normalized = normalize_history_transcript(&history);
        assert_valid_tool_transcript(&normalized);

        // The bogus placeholder is gone; the real reply is attached to the
        // compact call, adjacent to its assistant message.
        let answered = normalized
            .iter()
            .find(|m| m["tool_call_id"] == "call_compact")
            .expect("compact call must be answered");
        assert!(
            answered["content"].as_str().unwrap().contains("success"),
            "expected the recovered real reply, got {:?}",
            answered["content"]
        );
        assert!(
            !normalized.iter().any(|m| m["content"]
                .as_str()
                .unwrap_or("")
                .contains("turn was interrupted")),
            "stale synthetic reply must be removed"
        );
    }

    #[test]
    fn test_normalize_keeps_synthetic_when_reply_truly_missing() {
        // Genuinely interrupted turn (no real reply anywhere): the synthetic
        // answer is correct and must be kept so the transcript stays valid.
        let calls = r#"[{"id":"call_gone","type":"function","function":{"name":"sleep","arguments":"{}"}}]"#;
        let history = vec![
            hist("system", "sys", None, None),
            hist("assistant", "", Some(calls), None),
            hist("user", "next", None, None),
        ];
        let normalized = normalize_history_transcript(&history);
        assert_valid_tool_transcript(&normalized);
        let answered = normalized
            .iter()
            .find(|m| m["tool_call_id"] == "call_gone")
            .expect("must be answered");
        assert!(answered["content"]
            .as_str()
            .unwrap()
            .contains("turn was interrupted"));
    }

    #[test]
    fn test_normalize_reorders_late_tool_reply() {
        // Reproduces the live DB: assistant(call_bid) is answered only after
        // two later assistant turns (the sleep call was interrupted by an MCP
        // push, so its reply landed late). Gemini pairs positionally per turn
        // and rejects this with "functionResponse turn ... order".
        let calls =
            r#"[{"id":"call_a","type":"function","function":{"name":"sleep","arguments":"{}"}}]"#;
        let calls_b = r#"[{"id":"call_b","type":"function","function":{"name":"computer.list","arguments":"{}"}}]"#;
        let calls_c = r#"[{"id":"call_c","type":"function","function":{"name":"computer.terminal","arguments":"{}"}}]"#;
        let history = vec![
            hist("system", "sys", None, None),
            hist("user", "go", None, None),
            hist("assistant", "", Some(calls), None),
            hist("assistant", "", Some(calls_b), None),
            hist("tool", "listed", None, Some("call_b")),
            hist("assistant", "", Some(calls_c), None),
            hist("tool", "ran", None, Some("call_c")),
            hist("tool", "slept late", None, Some("call_a")),
        ];

        let normalized = normalize_history_transcript(&history);
        assert_valid_tool_transcript(&normalized);

        // Each group is closed before the next assistant turn, so a reply that
        // only arrives later cannot be moved back without reordering turns.
        // The group is answered synthetically and the late real reply becomes
        // context, preserving the information without breaking turn order.
        assert_eq!(normalized[2]["tool_calls"][0]["id"], "call_a");
        assert_eq!(normalized[3]["tool_call_id"], "call_a");
        assert!(normalized[3]["content"]
            .as_str()
            .unwrap()
            .contains("interrupted"));
        let late = normalized
            .iter()
            .find(|m| m["content"].as_str().unwrap_or("").contains("slept late"));
        assert!(
            late.is_some(),
            "late reply content must be preserved as context"
        );
    }

    #[test]
    fn test_normalize_is_idempotent_and_repairs_live_shapes() {
        // Shape from agent b1246cf6: two consecutive assistant tool_calls rows
        // with the earlier call answered afterwards.
        let sleep_calls = r#"[{"id":"call_sleep","type":"function","function":{"name":"sleep","arguments":"{}"}}]"#;
        let list_calls = r#"[{"id":"call_list","type":"function","function":{"name":"computer.terminal","arguments":"{}"}}]"#;
        let history = vec![
            hist("system", "sys", None, None),
            hist("assistant", "", Some(sleep_calls), None),
            hist("assistant", "", Some(list_calls), None),
            hist("tool", "slept", None, Some("call_sleep")),
            hist("tool", "failed", None, Some("call_list")),
            hist("user", "next", None, None),
        ];

        let once = normalize_history_transcript(&history);
        assert_valid_tool_transcript(&once);

        let as_items: Vec<_> = once
            .iter()
            .map(|m| {
                hist(
                    m["role"].as_str().unwrap_or("system"),
                    m["content"].as_str().unwrap_or(""),
                    m.get("tool_calls").map(|tc| tc.to_string()).as_deref(),
                    m.get("tool_call_id").and_then(|t| t.as_str()),
                )
            })
            .collect();
        let twice = normalize_history_transcript(&as_items);
        assert_eq!(once, twice, "normalisation must be idempotent");
    }

    #[test]
    fn test_normalize_answers_unanswered_calls_and_drops_duplicates() {
        let calls = r#"[{"id":"call_x","type":"function","function":{"name":"sleep","arguments":"{}"}},
                        {"id":"call_y","type":"function","function":{"name":"computer.list","arguments":"{}"}}]"#;
        let history = vec![
            hist("system", "sys", None, None),
            hist("assistant", "", Some(calls), None),
            hist("tool", "only x", None, Some("call_x")),
            hist("tool", "x again", None, Some("call_x")),
            hist("user", "hi", None, None),
        ];

        let normalized = normalize_history_transcript(&history);
        assert_valid_tool_transcript(&normalized);
        // call_y synthesized, duplicate call_x reply demoted to context.
        assert!(normalized.iter().any(|m| m["tool_call_id"] == "call_y"));
        assert_eq!(
            normalized
                .iter()
                .filter(|m| m["tool_call_id"] == "call_x")
                .count(),
            1
        );
    }

    #[test]
    fn test_sanitize_synthesizes_missing_tool_response_for_multicall() {
        // Regression: one assistant message with two tool calls, but the model
        // run was interrupted after only one reply was persisted (the exact
        // production failure: "insufficient tool messages following tool_calls").
        let raw = vec![
            json!({ "role": "system", "content": "You are Neuxbane." }),
            json!({ "role": "user", "content": "go" }),
            json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    { "id": "call_a", "type": "function", "function": { "name": "sleep", "arguments": "{}" } },
                    { "id": "call_b", "type": "function", "function": { "name": "computer.list", "arguments": "{}" } }
                ]
            }),
            json!({ "role": "tool", "tool_call_id": "call_a", "content": "slept" }),
        ];

        let sanitized = sanitize_chat_messages(raw);
        assert_valid_tool_transcript(&sanitized);
        // A synthetic response for call_b must sit right after call_a's reply.
        assert_eq!(sanitized.len(), 5);
        assert_eq!(sanitized[3]["role"], "tool");
        assert_eq!(sanitized[3]["tool_call_id"], "call_a");
        assert_eq!(sanitized[4]["role"], "tool");
        assert_eq!(sanitized[4]["tool_call_id"], "call_b");
        assert_eq!(sanitized[4]["name"], "computer.list");
    }

    #[test]
    fn test_sanitize_answers_tool_calls_orphaned_by_compaction() {
        // Regression: `compact` replaced messages with [system, tool(...)] and
        // also appended a following assistant message. The trailing tool_calls
        // message used to end the transcript unanswered.
        let raw = vec![
            json!({ "role": "system", "content": "compacted" }),
            json!({ "role": "tool", "tool_call_id": "call_compacted", "content": "{\"success\":true}" }),
            json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    { "id": "call_x", "type": "function", "function": { "name": "sleep", "arguments": "{}" } }
                ]
            }),
        ];

        let sanitized = sanitize_chat_messages(raw);
        assert_valid_tool_transcript(&sanitized);
        // The orphaned tool row became system context instead of a bare `tool`.
        assert_eq!(sanitized[1]["role"], "system");
        assert_eq!(sanitized[3]["role"], "tool");
        assert_eq!(sanitized[3]["tool_call_id"], "call_x");
    }

    #[test]
    fn test_sanitize_answers_tool_calls_before_new_user_turn() {
        // Regression: a tool call left dangling when the turn was aborted, then
        // the user sent a new message.
        let raw = vec![
            json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    { "id": "call_pending", "type": "function", "function": { "name": "computer.terminal", "arguments": "{}" } }
                ]
            }),
            json!({ "role": "user", "content": "are you there?" }),
        ];

        let sanitized = sanitize_chat_messages(raw);
        assert_valid_tool_transcript(&sanitized);
        assert_eq!(sanitized[1]["role"], "tool");
        assert_eq!(sanitized[1]["tool_call_id"], "call_pending");
        assert_eq!(sanitized[2]["role"], "user");
    }

    #[test]
    fn test_call_tool_schema_exposes_optional_truncation() {
        let tools = crate::runtime::meta_tools::get_meta_tools_definitions();
        let call_tool = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["function"]["name"] == "call_tool")
            .expect("call_tool definition");

        let props = &call_tool["function"]["parameters"]["properties"];
        for key in ["head", "body", "tail"] {
            assert_eq!(props[key]["type"], "integer", "{} must be an integer", key);
        }
        // They are optional: only name + arguments are required.
        let required = call_tool["function"]["parameters"]["required"]
            .as_array()
            .unwrap();
        assert_eq!(required.len(), 2);
        for key in ["head", "body", "tail"] {
            assert!(
                !required.iter().any(|r| r == key),
                "{} must stay optional",
                key
            );
        }
    }

    #[test]
    fn test_per_call_truncation_override() {
        use crate::runtime::meta_tools::{truncate_middle, TruncationConfig};

        let text: String = (0..10_000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();

        // A per-call override changes the retained budget.
        let small = TruncationConfig {
            head: 100,
            body: 100,
            tail: 100,
        };
        let large = TruncationConfig {
            head: 1000,
            body: 1000,
            tail: 1000,
        };

        let a = truncate_middle(&text, small);
        let b = truncate_middle(&text, large);
        assert!(a.chars().count() < b.chars().count());
        assert!(a.len() < b.len());

        // Disabling one segment works (0 = omit that segment).
        let no_body = TruncationConfig {
            head: 10,
            body: 0,
            tail: 10,
        };
        let out = truncate_middle(&text, no_body);
        assert_eq!(
            out.matches("<<truncated:").count(),
            1,
            "only the head/tail gap"
        );
    }

    #[test]
    fn test_append_missing_tool_responses_skips_executed_call() {
        let assistant = json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [
                { "id": "call_done", "type": "function", "function": { "name": "readMemory", "arguments": "{}" } },
                { "id": "call_other", "type": "function", "function": { "name": "sleep", "arguments": "{}" } }
            ]
        });
        let mut messages = vec![
            json!({ "role": "system", "content": "sys" }),
            assistant.clone(),
            json!({ "role": "tool", "tool_call_id": "call_done", "content": "ok" }),
        ];

        append_missing_tool_responses(&mut messages, &assistant, Some("call_done"), "test");

        assert_valid_tool_transcript(&messages);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[3]["tool_call_id"], "call_other");
    }
}
