//! The MCP surface: tool listing, dispatch, the per-agent settings app, and
//! the live Telegram update stream.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, CustomNotification,
    Implementation, InitializeResult, ListResourcesResult, ListToolsResult, MetaObject,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    Resource, ResourceContents, ServerCapabilities, ServerNotification, SubscribeRequestParams,
    Tool, ToolAnnotations, UnsubscribeRequestParams,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, Peer, RoleServer, ServerHandler};
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::config::{AgentTelegramConfig, Config, DATA_DIR, normalize_parse_mode, safe_id};
use crate::passthrough;
use crate::telegram::{TelegramClient, TelegramError};
use crate::tools::{self, ToolSpec};

const TELEGRAM_APP_HTML: &str = include_str!("app.html");
const UPDATES_URI: &str = "extension://telegram/updates";

const POLL_TIMEOUT_SECS: u64 = 25;
const POLL_LIMIT: u32 = 100;

pub struct TelegramMcp {
    cfg: Arc<Config>,
    client: TelegramClient,
    state: Arc<RuntimeState>,
}

struct RuntimeState {
    /// Host-supplied identity for the process. When present it is authoritative:
    /// a model-supplied `agent_id` must never redirect config reads or writes at
    /// another agent.
    spawn_agent_id: Option<String>,
    /// The agent this process serves. The executor stamps `agent_id` onto every
    /// tool call; the host also exports `NEUXDOT_AGENT_ID` at spawn time so the
    /// poller can resolve the per-agent token before the first tool call.
    active_agent_id: Mutex<Option<String>>,
    peer: Mutex<Option<Peer<RoleServer>>>,
    poller: Mutex<Option<JoinHandle<()>>>,
    recent: Mutex<Vec<Value>>,
}

impl RuntimeState {
    fn new() -> Self {
        let spawn_agent_id = std::env::var("NEUXDOT_AGENT_ID")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        Self {
            active_agent_id: Mutex::new(spawn_agent_id.clone()),
            spawn_agent_id,
            peer: Mutex::new(None),
            poller: Mutex::new(None),
            recent: Mutex::new(Vec::new()),
        }
    }

    async fn push_recent(&self, update: Value) {
        let mut recent = self.recent.lock().await;
        recent.push(update);
        if recent.len() > POLL_LIMIT as usize {
            let excess = recent.len() - POLL_LIMIT as usize;
            recent.drain(0..excess);
        }
    }

    async fn start_poller(self: &Arc<Self>, client: TelegramClient) {
        let mut guard = self.poller.lock().await;
        if guard.as_ref().is_some_and(|handle| !handle.is_finished()) {
            return;
        }
        let state = Arc::clone(self);
        *guard = Some(tokio::spawn(async move {
            run_poller(state, client).await;
        }));
    }
}

impl TelegramMcp {
    pub fn new(cfg: Arc<Config>, client: TelegramClient) -> Self {
        Self {
            cfg,
            client,
            state: Arc::new(RuntimeState::new()),
        }
    }

    fn schema(val: Value) -> Arc<Map<String, Value>> {
        match val {
            Value::Object(map) => Arc::new(map),
            _ => unreachable!("tool schemas are always JSON objects"),
        }
    }

    fn token_schema() -> Value {
        json!({
            "type": "string",
            "description": "Optional Telegram bot token for a stateless call. Defaults to the agent's configured token."
        })
    }

    /// Settings tools carry the bot token, so they are for the MCP-App only.
    /// The executor hides `_meta.ui.visibility: ["app"]` tools from the model.
    fn app_only(tool: Tool) -> Tool {
        let mut tool = tool;
        let meta = json!({ "ui": { "visibility": ["app"] } });
        tool.meta = Some(MetaObject(meta.as_object().cloned().unwrap_or_default()));
        tool
    }

    fn config_tools() -> Vec<Tool> {
        vec![
            Self::app_only(Tool::new(
                "telegram_get_config".to_string(),
                "Retrieve the Telegram configuration for an agent".to_string(),
                Self::schema(json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "The unique ID of the agent" }
                    },
                    "required": ["agent_id"]
                })),
            )
            .annotate(ToolAnnotations::default().read_only(true))),
            Self::app_only(Tool::new(
                "telegram_set_config".to_string(),
                "Save or reset the per-agent Telegram configuration".to_string(),
                Self::schema(json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "The unique ID of the agent" },
                        "bot_token": { "type": "string", "description": "Telegram bot token from @BotFather" },
                        "default_chat_id": { "type": "string", "description": "Default chat id or @channel username for sends" },
                        "parse_mode": {
                            "type": "string",
                            "enum": ["HTML", "MarkdownV2"],
                            "description": "Formatting mode for outgoing text; omit for plain text"
                        },
                        "reset": { "type": "boolean", "description": "If true, remove the agent override and inherit global settings" }
                    },
                    "required": ["agent_id"]
                })),
            )
            .annotate(ToolAnnotations::default().destructive(false))),
            Self::app_only(Tool::new(
                "telegram_test_connection".to_string(),
                "Check a bot token with Telegram's getMe and return the bot identity".to_string(),
                Self::schema(json!({
                    "type": "object",
                    "properties": {
                        "token": { "type": "string", "description": "Bot token to test" },
                        "agent_id": { "type": "string", "description": "Optional agent whose saved token should be used" }
                    }
                })),
            )
            .annotate(ToolAnnotations::default().read_only(true))),
        ]
    }

    /// One tool per table row: schema, method and annotations all come from the
    /// same `ToolSpec`, so the published surface cannot disagree with what the
    /// dispatcher actually calls.
    fn spec_tool(spec: &ToolSpec) -> Tool {
        Tool::new(
            spec.name.to_string(),
            spec.description.to_string(),
            Self::schema(tools::schema(spec)),
        )
        .annotate(
            ToolAnnotations::default()
                .read_only(spec.read_only)
                .destructive(spec.destructive)
                .open_world(true),
        )
    }

    /// The two calls that are not a plain Bot API method invocation.
    fn special_tools() -> Vec<Tool> {
        vec![
            Tool::new(
                "telegram_get_updates".to_string(),
                "Read incoming updates. While a live subscription is active, answers from the \
                 poller's cache instead, because the poller owns the Telegram offset."
                    .to_string(),
                Self::schema(json!({
                    "type": "object",
                    "properties": {
                        "offset": { "type": "integer", "description": "First update id to return" },
                        "limit": { "type": "integer", "description": "1-100 (default 100)" },
                        "timeout": { "type": "integer", "description": "Long-poll timeout in seconds, 0-50" },
                        "allowed_updates": { "type": "array", "items": { "type": "string" }, "description": "Update types to receive" },
                        "bot_token": Self::token_schema()
                    }
                })),
            )
            .annotate(ToolAnnotations::default().read_only(true).open_world(true)),
            Tool::new(
                "telegram_api".to_string(),
                "Call any other Telegram Bot API method directly. Use this for methods the named \
                 tools do not cover. 'method' is the Bot API method name in PascalCase, e.g. \
                 sendMessage; 'params' is the raw parameter object."
                    .to_string(),
                Self::schema(json!({
                    "type": "object",
                    "properties": {
                        "method": {
                            "type": "string",
                            "description": "Bot API method name in PascalCase, e.g. sendMessage"
                        },
                        "params": {
                            "type": "object",
                            "description": "The method's parameter object, e.g. {\"chat_id\": 123, \"text\": \"hi\"}"
                        },
                        "bot_token": Self::token_schema()
                    },
                    "required": ["method"]
                })),
            )
            .annotate(ToolAnnotations::default().read_only(false).open_world(true)),
        ]
    }

    fn telegram_tools() -> Vec<Tool> {
        let mut tools: Vec<Tool> = tools::TOOLS.iter().map(Self::spec_tool).collect();
        tools.extend(Self::special_tools());
        tools
    }

    fn instructions(&self) -> String {
        String::from(
            "Telegram Bot API access, plus a live update stream. Tools cover the whole Bot API: \
             messages (send, forward, copy, edit, delete, pins, reactions, media, albums, polls, \
             dice, locations, contacts, invoices), chats (settings, permissions, invite links, \
             join requests, forum topics), members (ban, unban, restrict, promote, custom titles), \
             bot profile and commands, webhooks, stickers and custom emoji, callbacks, inline \
             mode, Mini Apps, stars and games. 'telegram_api' reaches any other method. \
             A call may pass its own 'bot_token'; otherwise the agent's Telegram Settings \
             override is used, then the server-wide TELEGRAM_BOT_TOKEN. The bot's own Telegram \
             rights decide what is allowed: a 400/403 means the bot lacks that right or is not \
             an administrator in that chat, so fix it in Telegram rather than retrying. \
             Subscribe to extension://telegram/updates to receive incoming messages as push \
             notifications instead of polling.",
        )
    }

    async fn remember_agent(&self, args: &Map<String, Value>) {
        // The host-exported id wins. Only fall back to the call argument when
        // the host did not provide one (for example an older host build).
        if self.state.spawn_agent_id.is_some() {
            return;
        }
        if let Some(agent_id) = args
            .get("agent_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            *self.state.active_agent_id.lock().await = Some(agent_id.to_string());
        }
    }

    async fn agent_id(&self) -> Option<String> {
        self.state.active_agent_id.lock().await.clone()
    }

    async fn handle_config_tool(
        &self,
        name: &str,
        args: &Map<String, Value>,
        authoritative_agent_id: Option<&str>,
    ) -> Result<CallToolResponse, McpError> {
        let requested_agent_id = authoritative_agent_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                args.get("agent_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            });

        match name {
            "telegram_get_config" => {
                let Some(agent_id) = requested_agent_id else {
                    return Ok(tool_error("Missing required 'agent_id' parameter"));
                };

                let agent_cfg = AgentTelegramConfig::load(agent_id);
                let resp = if let Some(cfg) = agent_cfg {
                    json!({
                        "has_override": true,
                        "bot_token": cfg.bot_token.unwrap_or_default(),
                        "default_chat_id": cfg.default_chat_id,
                        "parse_mode": cfg.parse_mode,
                        "api_base": self.cfg.api_base,
                    })
                } else {
                    json!({
                        "has_override": false,
                        "bot_token": if self.cfg.bot_token.is_some() { "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}" } else { "" },
                        "default_chat_id": self.cfg.default_chat_id,
                        "parse_mode": self.cfg.parse_mode,
                        "api_base": self.cfg.api_base,
                    })
                };
                Ok(tool_success(resp))
            }
            "telegram_set_config" => {
                let Some(agent_id) = requested_agent_id else {
                    return Ok(tool_error("Missing required 'agent_id' parameter"));
                };

                if args.get("reset").and_then(Value::as_bool).unwrap_or(false) {
                    if let Err(e) = AgentTelegramConfig::delete(agent_id) {
                        return Ok(tool_error(format!("Failed to remove configuration: {e}")));
                    }
                    return Ok(tool_success(json!({ "ok": true, "reset": true })));
                }

                let bot_token = opt_str(args, "bot_token").or_else(|| opt_str(args, "token"));
                let default_chat_id = opt_str(args, "default_chat_id");
                let parse_mode = match args.get("parse_mode") {
                    Some(value) => match normalize_parse_mode(value.as_str()) {
                        Ok(mode) => mode,
                        Err(e) => return Ok(tool_error(e.to_string())),
                    },
                    None => None,
                };
                if bot_token.is_none() && default_chat_id.is_none() && parse_mode.is_none() {
                    let _ = AgentTelegramConfig::delete(agent_id);
                    return Ok(tool_success(json!({ "ok": true, "reset": true })));
                }

                let cfg = AgentTelegramConfig {
                    bot_token,
                    default_chat_id,
                    parse_mode,
                };
                if let Err(e) = AgentTelegramConfig::save(agent_id, &cfg) {
                    return Ok(tool_error(format!("Failed to persist configuration: {e}")));
                }
                Ok(tool_success(json!({ "ok": true, "saved": true })))
            }
            "telegram_test_connection" => {
                let token = opt_str(args, "token")
                    .or_else(|| opt_str(args, "bot_token"))
                    .or_else(|| {
                        requested_agent_id
                            .and_then(AgentTelegramConfig::load)
                            .and_then(|cfg| cfg.bot_token)
                    })
                    .or_else(|| self.cfg.bot_token.clone());

                let Some(token) = token else {
                    return Ok(tool_success(json!({
                        "ok": false,
                        "error": "Bot token is required."
                    })));
                };

                match self.client.call(&token, "getMe", &json!({})).await {
                    Ok(bot) => Ok(tool_success(json!({ "ok": true, "bot": bot }))),
                    Err(e) => Ok(tool_success(
                        json!({ "ok": false, "error": e.user_message() }),
                    )),
                }
            }
            _ => Ok(tool_error(format!("Unknown config tool: {name}"))),
        }
    }

    async fn get_updates(&self, token: &str, args: &Map<String, Value>) -> CallToolResponse {
        // The live poller owns the getUpdates offset while a subscription is
        // active. Calling getUpdates concurrently would make Telegram answer
        // 409, so serve the streamed cache instead.
        let subscribed = self
            .state
            .poller
            .lock()
            .await
            .as_ref()
            .is_some_and(|handle| !handle.is_finished());
        if subscribed {
            let recent = self.state.recent.lock().await.clone();
            return tool_success(json!({
                "ok": true,
                "subscribed": true,
                "updates": recent,
                "note": "A live subscription is active. These are the most recent updates streamed by the poller; the poller owns the Telegram offset."
            }));
        }

        let mut params = Map::new();
        if let Some(value) = args.get("offset").and_then(Value::as_i64) {
            params.insert("offset".to_string(), json!(value));
        }
        if let Some(value) = args.get("limit").and_then(Value::as_i64) {
            params.insert(
                "limit".to_string(),
                json!(value.clamp(1, POLL_LIMIT as i64)),
            );
        }
        if let Some(value) = args.get("timeout").and_then(Value::as_i64) {
            params.insert("timeout".to_string(), json!(value.clamp(0, 50)));
        }
        if let Some(value) = args.get("allowed_updates").cloned() {
            params.insert("allowed_updates".to_string(), value);
        }

        match self
            .client
            .call(token, "getUpdates", &Value::Object(params))
            .await
        {
            Ok(result) => {
                if let Some(updates) = result.as_array()
                    && let Some(next) = updates
                        .iter()
                        .filter_map(|update| update.get("update_id").and_then(Value::as_i64))
                        .max()
                        .map(|id| id + 1)
                {
                    let agent_id = self.state.active_agent_id.lock().await.clone();
                    save_offset(agent_id.as_deref(), next);
                }
                tool_success(result)
            }
            Err(e) => tool_error(e.user_message()),
        }
    }

    async fn call_api(&self, token: &str, method: &str, params: Value) -> CallToolResponse {
        match self.client.call(token, method, &params).await {
            Ok(value) => tool_success(value),
            Err(e) => tool_error(e.user_message()),
        }
    }

    /// Dispatch one table row: fill the configured chat and formatting mode,
    /// validate the arguments, then call exactly the method the row names.
    async fn call_spec(
        &self,
        token: Option<&str>,
        spec: &'static ToolSpec,
        args: &Map<String, Value>,
        agent_cfg: Option<&AgentTelegramConfig>,
    ) -> CallToolResponse {
        let default_chat = resolve_chat_id(args, agent_cfg, &self.cfg);
        let default_parse_mode = normalize_parse_mode(
            agent_cfg
                .and_then(|cfg| cfg.parse_mode.as_deref())
                .or(self.cfg.parse_mode.as_deref()),
        )
        .ok()
        .flatten();

        let params = match tools::build_params(
            spec,
            args,
            default_chat.as_deref(),
            default_parse_mode.as_deref(),
        ) {
            Ok(params) => params,
            Err(e) => return tool_error(e),
        };

        let Some(token) = token else {
            return tool_error(
                "No Telegram bot token configured. Save one in Telegram Settings, pass 'bot_token', or set TELEGRAM_BOT_TOKEN.",
            );
        };

        match self.client.call(token, spec.method, &params).await {
            Ok(value) => tool_success(value),
            Err(e) => tool_error(e.user_message()),
        }
    }

    /// `telegram_api`: one guarded passthrough for methods no named tool covers.
    /// See `passthrough.rs` for what is rejected and why.
    async fn call_passthrough(&self, token: &str, args: &Map<String, Value>) -> CallToolResponse {
        let request = match passthrough::build(args) {
            Ok(request) => request,
            Err(e) => return tool_error(e),
        };
        self.call_api(token, &request.method, request.params).await
    }
}

impl ServerHandler for TelegramMcp {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_resources_subscribe()
                .build(),
        )
        .with_server_info(Implementation::new(
            "telegram-mcp",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(self.instructions())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let mut config = Resource::new(
            "ui://telegram-config".to_string(),
            "Telegram Settings".to_string(),
        );
        config.mime_type = Some("text/html;profile=mcp-app".to_string());
        config.description =
            Some("Telegram per-agent configuration and bot authentication".to_string());

        let mut updates = Resource::new(UPDATES_URI.to_string(), "Telegram Updates".to_string());
        updates.mime_type = Some("application/json".to_string());
        updates.description = Some(
            "Incoming Telegram messages streamed as push notifications while subscribed"
                .to_string(),
        );

        Ok(ListResourcesResult::with_all_items(vec![config, updates]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        if request.uri == "ui://telegram-config" {
            let mut content = ResourceContents::text(TELEGRAM_APP_HTML, "ui://telegram-config");
            if let ResourceContents::TextResourceContents {
                ref mut mime_type, ..
            } = content
            {
                *mime_type = Some("text/html;profile=mcp-app".into());
            }
            return Ok(ReadResourceResult::new(vec![content]).into());
        }

        if request.uri == UPDATES_URI {
            let updates = self.state.recent.lock().await.clone();
            let text = serde_json::to_string_pretty(&updates).unwrap_or_else(|_| "[]".to_string());
            let mut content = ResourceContents::text(text, UPDATES_URI);
            if let ResourceContents::TextResourceContents {
                ref mut mime_type, ..
            } = content
            {
                *mime_type = Some("application/json".into());
            }
            return Ok(ReadResourceResult::new(vec![content]).into());
        }

        Err(McpError::resource_not_found(request.uri, None))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools = Self::telegram_tools();
        tools.extend(Self::config_tools());
        Ok(ListToolsResult::with_all_items(tools))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        Self::telegram_tools()
            .into_iter()
            .chain(Self::config_tools())
            .find(|tool| tool.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name.to_string();
        let mut args: Map<String, Value> = request.arguments.unwrap_or_default();

        self.remember_agent(&args).await;
        *self.state.peer.lock().await = Some(context.peer.clone());

        let authoritative_agent_id = self.agent_id().await;

        if matches!(
            name.as_str(),
            "telegram_get_config" | "telegram_set_config" | "telegram_test_connection"
        ) {
            return self
                .handle_config_tool(&name, &args, authoritative_agent_id.as_deref())
                .await;
        }

        let agent_cfg = authoritative_agent_id
            .as_deref()
            .or_else(|| args.get("agent_id").and_then(Value::as_str))
            .and_then(AgentTelegramConfig::load);

        let token_arg = args.remove("bot_token").or_else(|| args.remove("token"));
        let explicit_token = token_arg
            .as_ref()
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let token = self
            .client
            .resolve_token(explicit_token, agent_cfg.as_ref());
        args.remove("agent_id");

        if let Some(spec) = tools::find(&name) {
            return Ok(self
                .call_spec(token.as_deref(), spec, &args, agent_cfg.as_ref())
                .await);
        }

        let Some(token) = token else {
            return Ok(tool_error(
                "No Telegram bot token configured. Save one in Telegram Settings, pass 'bot_token', or set TELEGRAM_BOT_TOKEN.",
            ));
        };

        match name.as_str() {
            "telegram_get_updates" => Ok(self.get_updates(&token, &args).await),
            "telegram_api" => Ok(self.call_passthrough(&token, &args).await),
            other => Ok(tool_error(format!("unknown tool {other:?}"))),
        }
    }

    #[allow(deprecated)]
    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        if request.uri != UPDATES_URI {
            return Err(McpError::resource_not_found(request.uri, None));
        }

        *self.state.peer.lock().await = Some(context.peer.clone());
        self.state.start_poller(self.client.clone()).await;
        tracing::info!("subscribed to {UPDATES_URI}");
        Ok(())
    }

    #[allow(deprecated)]
    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        if request.uri != UPDATES_URI {
            return Ok(());
        }
        if let Some(handle) = self.state.poller.lock().await.take() {
            handle.abort();
        }
        tracing::info!("unsubscribed from {UPDATES_URI}");
        Ok(())
    }
}

/// Long-poll Telegram until the subscription is cancelled.
async fn run_poller(state: Arc<RuntimeState>, client: TelegramClient) {
    loop {
        let agent_id = state.active_agent_id.lock().await.clone();
        let agent_cfg = agent_id.as_deref().and_then(AgentTelegramConfig::load);
        let Some(token) = client.resolve_token(None, agent_cfg.as_ref()) else {
            tracing::warn!("telegram update stream waiting for a bot token");
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        };

        let offset = load_offset(agent_id.as_deref());
        let params = json!({
            "offset": offset,
            "timeout": POLL_TIMEOUT_SECS,
            "limit": POLL_LIMIT,
            "allowed_updates": [
                "message",
                "edited_message",
                "channel_post",
                "edited_channel_post",
                "callback_query"
            ]
        });

        match client.call(&token, "getUpdates", &params).await {
            Ok(result) => {
                let updates = result.as_array().cloned().unwrap_or_default();
                let mut next_offset = offset;
                for update in &updates {
                    if let Some(update_id) = update.get("update_id").and_then(Value::as_i64) {
                        next_offset = next_offset.max(update_id + 1);
                    }

                    let summary = summarize_update(update, agent_id.as_deref());
                    state.push_recent(summary.clone()).await;

                    let peer = state.peer.lock().await.clone();
                    let Some(peer) = peer else {
                        continue;
                    };
                    let notification = ServerNotification::CustomNotification(
                        CustomNotification::new("notifications/resources/updated", Some(summary)),
                    );
                    if let Err(e) = peer.send_notification(notification).await {
                        tracing::warn!("telegram update stream stopped: {e}");
                        return;
                    }
                }
                if !updates.is_empty() {
                    save_offset(agent_id.as_deref(), next_offset);
                }
            }
            Err(TelegramError::Api { code: 409, .. }) => {
                tracing::warn!("Telegram getUpdates conflict; another poller or webhook is active");
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
            Err(e) => {
                tracing::warn!("Telegram polling failed: {}", e.user_message());
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    }
}

fn summarize_update(update: &Value, agent_id: Option<&str>) -> Value {
    let mut out = Map::new();
    out.insert("uri".to_string(), json!(UPDATES_URI));
    if let Some(agent_id) = agent_id {
        out.insert("agent_id".to_string(), json!(agent_id));
    }
    if let Some(update_id) = update.get("update_id") {
        out.insert("update_id".to_string(), update_id.clone());
    }

    let (kind, message) = if let Some(message) = update.get("message") {
        ("message", Some(message))
    } else if let Some(message) = update.get("edited_message") {
        ("edited_message", Some(message))
    } else if let Some(message) = update.get("channel_post") {
        ("channel_post", Some(message))
    } else if let Some(message) = update.get("edited_channel_post") {
        ("edited_channel_post", Some(message))
    } else {
        ("other", None)
    };
    out.insert("type".to_string(), json!(kind));

    if let Some(message) = message {
        for key in ["message_id", "date", "chat", "from", "text", "caption"] {
            if let Some(value) = message.get(key) {
                out.insert(key.to_string(), value.clone());
            }
        }
    }

    if let Some(query) = update.get("callback_query") {
        out.insert(
            "callback_query".to_string(),
            json!({
                "id": query.get("id"),
                "from": query.get("from"),
                "data": query.get("data"),
                "message_id": query.get("message").and_then(|m| m.get("message_id")),
                "chat": query.get("message").and_then(|m| m.get("chat")),
            }),
        );
    }

    Value::Object(out)
}

fn resolve_chat_id(
    args: &Map<String, Value>,
    agent: Option<&AgentTelegramConfig>,
    cfg: &Config,
) -> Option<String> {
    opt_str(args, "chat_id")
        .or_else(|| agent.and_then(|c| c.default_chat_id.clone()))
        .or_else(|| cfg.default_chat_id.clone())
}

fn opt_str(args: &Map<String, Value>, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn offset_path(agent_id: Option<&str>) -> PathBuf {
    let name = agent_id
        .map(safe_id)
        .unwrap_or_else(|| "global".to_string());
    PathBuf::from(DATA_DIR).join(format!("offset_{name}.json"))
}

fn load_offset(agent_id: Option<&str>) -> i64 {
    std::fs::read_to_string(offset_path(agent_id))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| value.get("offset").and_then(Value::as_i64))
        .unwrap_or(0)
}

fn save_offset(agent_id: Option<&str>, offset: i64) {
    let path = offset_path(agent_id);
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!("could not create Telegram state directory: {e}");
        return;
    }
    if let Err(e) = std::fs::write(&path, json!({ "offset": offset }).to_string()) {
        tracing::warn!("could not persist Telegram update offset: {e}");
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn tool_success(value: Value) -> CallToolResponse {
    CallToolResult::success(vec![ContentBlock::text(pretty(&value))]).into()
}

fn tool_error(message: impl Into<String>) -> CallToolResponse {
    CallToolResult::error(vec![ContentBlock::text(message.into())]).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            bot_token: None,
            default_chat_id: None,
            parse_mode: None,
            api_base: "https://api.telegram.org".to_string(),
            timeout: Duration::from_secs(60),
        }
    }

    #[test]
    fn summarizes_a_private_message() {
        let update = json!({
            "update_id": 42,
            "message": {
                "message_id": 7,
                "date": 1700000000,
                "chat": { "id": 123, "type": "private" },
                "from": { "id": 123, "username": "alice" },
                "text": "hello"
            }
        });
        let summary = summarize_update(&update, Some("agent-1"));
        assert_eq!(summary["uri"], UPDATES_URI);
        assert_eq!(summary["agent_id"], "agent-1");
        assert_eq!(summary["update_id"], 42);
        assert_eq!(summary["type"], "message");
        assert_eq!(summary["text"], "hello");
        assert_eq!(summary["chat"]["id"], 123);
    }

    #[test]
    fn summarizes_an_edited_channel_post() {
        let update = json!({
            "update_id": 9,
            "edited_channel_post": {
                "message_id": 1,
                "chat": { "id": -1001, "type": "channel" },
                "caption": "new caption"
            }
        });
        let summary = summarize_update(&update, None);
        assert_eq!(summary["type"], "edited_channel_post");
        assert_eq!(summary["caption"], "new caption");
        assert!(summary.get("agent_id").is_none());
    }

    /// The model only sees `tools/list`, so every table row must be published,
    /// with the schema and annotations the row declares.
    #[test]
    fn every_table_row_is_published_with_matching_annotations() {
        let tools = TelegramMcp::telegram_tools();
        let by_name: std::collections::HashMap<&str, &Tool> = tools
            .iter()
            .map(|tool| (tool.name.as_ref(), tool))
            .collect();

        assert_eq!(
            tools.len(),
            tools::TOOLS.len() + 2,
            "table rows plus get_updates and the passthrough"
        );
        assert_eq!(by_name.len(), tools.len(), "tool names must be unique");

        for spec in tools::TOOLS {
            let tool = by_name
                .get(spec.name)
                .unwrap_or_else(|| panic!("{} is not published", spec.name));
            assert_eq!(tool.description.as_deref(), Some(spec.description));
            let annotations = tool.annotations.as_ref().expect("annotations");
            assert_eq!(
                annotations.read_only_hint,
                Some(spec.read_only),
                "{}: read_only_hint must match the row",
                spec.name
            );
            assert_eq!(
                annotations.destructive_hint,
                Some(spec.destructive),
                "{}: destructive_hint must match the row",
                spec.name
            );
            assert_eq!(
                tool.input_schema.as_ref()["type"],
                "object",
                "{}",
                spec.name
            );
        }

        for name in ["telegram_get_updates", "telegram_api"] {
            assert!(by_name.contains_key(name), "{name} must be published");
        }
    }

    #[test]
    fn settings_tools_are_hidden_from_the_model() {
        for tool in TelegramMcp::config_tools() {
            let meta = tool.meta.as_ref().expect("settings tools carry meta");
            assert_eq!(meta.0["ui"]["visibility"][0], "app", "{}", tool.name);
        }
        for tool in TelegramMcp::telegram_tools() {
            assert!(tool.meta.is_none(), "{} must stay visible", tool.name);
        }
    }

    #[test]
    fn resolves_chat_precedence() {
        let args = Map::new();
        let agent = AgentTelegramConfig {
            bot_token: None,
            default_chat_id: Some("agent-chat".into()),
            parse_mode: Some("HTML".into()),
        };
        assert_eq!(
            resolve_chat_id(&args, Some(&agent), &config()).as_deref(),
            Some("agent-chat")
        );

        let mut args = Map::new();
        args.insert("chat_id".into(), json!("explicit"));
        args.insert("parse_mode".into(), json!("markdownv2"));
        assert_eq!(
            resolve_chat_id(&args, Some(&agent), &config()).as_deref(),
            Some("explicit")
        );
    }

    /// The chat default and the parse-mode default both live in the table
    /// builder now; these are the same assertions the old `resolve_parse_mode`
    /// helper carried, exercised through the path the dispatcher uses.
    #[test]
    fn parse_mode_default_is_normalised_and_overridable() {
        let send = tools::find("telegram_send_message").unwrap();
        let mut args = Map::new();
        args.insert("text".into(), json!("hi"));
        let built = tools::build_params(send, &args, Some("agent-chat"), Some("HTML")).unwrap();
        assert_eq!(built["parse_mode"], "HTML");
        assert_eq!(built["chat_id"], "agent-chat");

        let mut args = Map::new();
        args.insert("text".into(), json!("hi"));
        args.insert("parse_mode".into(), json!("markdownv2"));
        let built = tools::build_params(send, &args, Some("agent-chat"), Some("HTML")).unwrap();
        assert_eq!(built["parse_mode"], "markdownv2");

        let mut args = Map::new();
        args.insert("text".into(), json!("hi"));
        args.insert("parse_mode".into(), json!("rtf"));
        assert!(
            tools::build_params(send, &args, Some("agent-chat"), Some("HTML")).is_err(),
            "an unknown parse mode is rejected before the request"
        );
    }
}
