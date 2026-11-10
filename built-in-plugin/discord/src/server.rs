//! The MCP surface: tool listing, dispatch, the per-agent settings app, and
//! the live Discord Gateway message stream.

use std::sync::Arc;
use std::time::Duration;

use reqwest::Method;
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
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

use crate::config::{AgentDiscordConfig, Config, normalize_snowflake};
use crate::discord::DiscordClient;
use crate::gateway::{self, GatewayEvent};
use crate::passthrough;
use crate::tools::{self, ToolSpec, Verb};

const DISCORD_APP_HTML: &str = include_str!("app.html");
const MESSAGES_URI: &str = "extension://discord/messages";

const MAX_MESSAGES_LIMIT: u32 = 100;
/// Cap on the in-memory replay buffer handed back by `discord_get_messages` and
/// by a read of the push resource.
const RECENT_LIMIT: usize = 100;
const GATEWAY_RETRY_SECS: u64 = 15;
const TOKEN_WAIT_SECS: u64 = 10;
const NO_TOKEN: &str = "No Discord bot token configured. Save one in Discord Settings, pass \
'bot_token', or set DISCORD_BOT_TOKEN.";

pub struct DiscordMcp {
    cfg: Arc<Config>,
    client: DiscordClient,
    state: Arc<RuntimeState>,
}

struct RuntimeState {
    /// Host-supplied identity for the process. When present it is authoritative:
    /// a model-supplied `agent_id` must never redirect config reads or writes at
    /// another agent.
    spawn_agent_id: Option<String>,
    /// The agent this process serves. The executor stamps `agent_id` onto every
    /// tool call; the host also exports `NEUXDOT_AGENT_ID` at spawn time so the
    /// stream can resolve the per-agent token before the first tool call.
    active_agent_id: Mutex<Option<String>>,
    peer: Mutex<Option<Peer<RoleServer>>>,
    stream: Mutex<Option<JoinHandle<()>>>,
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
            stream: Mutex::new(None),
            recent: Mutex::new(Vec::new()),
        }
    }

    async fn push_recent(&self, message: Value) {
        let mut recent = self.recent.lock().await;
        recent.push(message);
        if recent.len() > RECENT_LIMIT {
            let excess = recent.len() - RECENT_LIMIT;
            recent.drain(0..excess);
        }
    }

    async fn is_streaming(&self) -> bool {
        self.stream
            .lock()
            .await
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
    }

    async fn start_stream(self: &Arc<Self>, cfg: Arc<Config>, client: DiscordClient) {
        let mut guard = self.stream.lock().await;
        if guard.as_ref().is_some_and(|handle| !handle.is_finished()) {
            return;
        }
        let state = Arc::clone(self);
        *guard = Some(tokio::spawn(async move {
            run_stream(state, cfg, client).await;
        }));
    }

    async fn stop_stream(&self) {
        if let Some(handle) = self.stream.lock().await.take() {
            handle.abort();
        }
    }
}

impl DiscordMcp {
    pub fn new(cfg: Arc<Config>, client: DiscordClient) -> Self {
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
            "description": "Optional Discord bot token for a stateless call. Defaults to the agent's configured token."
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
                "discord_get_config".to_string(),
                "Retrieve the Discord configuration for an agent".to_string(),
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
                "discord_set_config".to_string(),
                "Save or reset the per-agent Discord configuration".to_string(),
                Self::schema(json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "The unique ID of the agent" },
                        "bot_token": { "type": "string", "description": "Discord bot token from the Developer Portal" },
                        "default_channel_id": { "type": "string", "description": "Default numeric channel id for sends" },
                        "reset": { "type": "boolean", "description": "If true, remove the agent override and inherit global settings" }
                    },
                    "required": ["agent_id"]
                })),
            )
            .annotate(ToolAnnotations::default().destructive(false))),
            Self::app_only(Tool::new(
                "discord_test_connection".to_string(),
                "Check a bot token against GET /users/@me and return the bot identity".to_string(),
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

    /// One tool per table row. The schema, the verb and the annotations all come
    /// from the same `ToolSpec`, so the published surface cannot disagree with
    /// what the dispatcher actually calls.
    fn spec_tool(spec: &ToolSpec) -> Tool {
        Tool::new(
            spec.name.to_string(),
            spec.description.to_string(),
            Self::schema(tools::schema(spec)),
        )
        .annotate(
            ToolAnnotations::default()
                .read_only(spec.verb.is_read_only())
                .destructive(spec.destructive)
                .open_world(true),
        )
    }

    /// The three calls that are not a single REST request.
    fn special_tools() -> Vec<Tool> {
        vec![
            Tool::new(
                "discord_get_messages".to_string(),
                "Read recent messages from a channel. While a live subscription is active this \
                 answers from the Gateway stream instead of paging the REST endpoint."
                    .to_string(),
                Self::schema(json!({
                    "type": "object",
                    "properties": {
                        "channel_id": { "type": "string", "description": "Numeric channel id" },
                        "limit": { "type": "integer", "description": "1-100 (default 50)" },
                        "before": { "type": "string", "description": "Only messages before this message id" },
                        "bot_token": Self::token_schema()
                    },
                    "required": ["channel_id"]
                })),
            )
            .annotate(ToolAnnotations::default().read_only(true).open_world(true)),
            Tool::new(
                "discord_send_dm".to_string(),
                "Send a direct message to a user, opening the DM channel first if needed".to_string(),
                Self::schema(json!({
                    "type": "object",
                    "properties": {
                        "recipient_id": { "type": "string", "description": "User id to message" },
                        "text": { "type": "string", "description": "Message text (1-2000 characters)" },
                        "bot_token": Self::token_schema()
                    },
                    "required": ["recipient_id", "text"]
                })),
            )
            .annotate(ToolAnnotations::default().read_only(false).open_world(true)),
            Tool::new(
                "discord_api".to_string(),
                "Call any other Discord REST endpoint directly. Use this for endpoints the named \
                 tools do not cover (auto-moderation rules, soundboard, stage instances). \
                 'path' is relative to https://discord.com/api/v10 and must start with a \
                 resource such as guilds, channels, users, webhooks, invites, applications, \
                 interactions, stickers, voice or stage-instances."
                    .to_string(),
                Self::schema(json!({
                    "type": "object",
                    "properties": {
                        "method": {
                            "type": "string",
                            "enum": ["GET", "POST", "PATCH", "PUT", "DELETE"],
                            "description": "HTTP method"
                        },
                        "path": {
                            "type": "string",
                            "description": "Resource path, e.g. guilds/123/auto-moderation/rules"
                        },
                        "body": {
                            "type": "object",
                            "description": "JSON request body. Required for POST, PATCH and PUT."
                        },
                        "reason": {
                            "type": "string",
                            "description": "Audit-log reason recorded with the action (up to 512 characters)."
                        },
                        "bot_token": Self::token_schema()
                    },
                    "required": ["method", "path"]
                })),
            )
            .annotate(ToolAnnotations::default().read_only(false).open_world(true)),
        ]
    }

    fn discord_tools() -> Vec<Tool> {
        let mut tools: Vec<Tool> = tools::TOOLS.iter().map(Self::spec_tool).collect();
        tools.extend(Self::special_tools());
        tools
    }

    fn instructions(&self) -> String {
        String::from(
            "Discord REST access, plus a live Gateway message stream. Tools cover the whole \
             server surface: guild settings and audit log, channels and permission overwrites, \
             messages (send, edit, delete, bulk delete, reactions, pins, crosspost), members \
             (kick, ban, unban, bulk ban, prune, timeout, nicknames, roles), roles, threads, \
             invites, webhooks, emoji, scheduled events and auto-moderation rules. \
             'discord_api' reaches any other endpoint. A call may pass its own 'bot_token'; \
             otherwise the agent's Discord Settings override is used, then the server-wide \
             DISCORD_BOT_TOKEN. The bot's own Discord role decides what is allowed: a 403 \
             means the bot lacks that permission in that server, so grant it there rather \
             than retrying. Pass 'reason' on moderation calls to record it in the audit log. \
             Subscribe to extension://discord/messages to receive incoming messages as push \
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
            "discord_get_config" => {
                let Some(agent_id) = requested_agent_id else {
                    return Ok(tool_error("Missing required 'agent_id' parameter"));
                };

                let agent_cfg = AgentDiscordConfig::load(agent_id);
                let resp = if let Some(cfg) = agent_cfg {
                    json!({
                        "has_override": true,
                        "bot_token": cfg.bot_token.unwrap_or_default(),
                        "default_channel_id": cfg.default_channel_id,
                        "api_base": self.cfg.api_base,
                    })
                } else {
                    json!({
                        "has_override": false,
                        "bot_token": if self.cfg.bot_token.is_some() { "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}" } else { "" },
                        "default_channel_id": self.cfg.default_channel_id,
                        "api_base": self.cfg.api_base,
                    })
                };
                Ok(tool_success(resp))
            }
            "discord_set_config" => {
                let Some(agent_id) = requested_agent_id else {
                    return Ok(tool_error("Missing required 'agent_id' parameter"));
                };

                if args.get("reset").and_then(Value::as_bool).unwrap_or(false) {
                    if let Err(e) = AgentDiscordConfig::delete(agent_id) {
                        return Ok(tool_error(format!("Failed to remove configuration: {e}")));
                    }
                    return Ok(tool_success(json!({ "ok": true, "reset": true })));
                }

                let bot_token = opt_str(args, "bot_token").or_else(|| opt_str(args, "token"));
                let default_channel_id =
                    match args.get("default_channel_id").and_then(Value::as_str) {
                        Some(raw) if !raw.trim().is_empty() => match normalize_snowflake(raw) {
                            Ok(id) => Some(id),
                            Err(e) => return Ok(tool_error(e.to_string())),
                        },
                        _ => None,
                    };
                if bot_token.is_none() && default_channel_id.is_none() {
                    let _ = AgentDiscordConfig::delete(agent_id);
                    return Ok(tool_success(json!({ "ok": true, "reset": true })));
                }

                let cfg = AgentDiscordConfig {
                    bot_token,
                    default_channel_id,
                };
                if let Err(e) = AgentDiscordConfig::save(agent_id, &cfg) {
                    return Ok(tool_error(format!("Failed to persist configuration: {e}")));
                }
                Ok(tool_success(json!({ "ok": true, "saved": true })))
            }
            "discord_test_connection" => {
                let token = opt_str(args, "token")
                    .or_else(|| opt_str(args, "bot_token"))
                    .or_else(|| {
                        requested_agent_id
                            .and_then(AgentDiscordConfig::load)
                            .and_then(|cfg| cfg.bot_token)
                    })
                    .or_else(|| self.cfg.bot_token.clone());

                let Some(token) = token else {
                    return Ok(tool_success(json!({
                        "ok": false,
                        "error": "Bot token is required."
                    })));
                };

                match self.client.get(&token, "users/@me").await {
                    Ok(bot) => Ok(tool_success(json!({ "ok": true, "bot": bot }))),
                    Err(e) => Ok(tool_success(
                        json!({ "ok": false, "error": e.user_message() }),
                    )),
                }
            }
            _ => Ok(tool_error(format!("Unknown config tool: {name}"))),
        }
    }

    async fn get_messages(&self, token: &str, args: &Map<String, Value>) -> CallToolResponse {
        let Some(raw_channel_id) = opt_str(args, "channel_id") else {
            return tool_error("'channel_id' is required");
        };
        let channel_id = match normalize_snowflake(&raw_channel_id) {
            Ok(id) => id,
            Err(e) => return tool_error(e.to_string()),
        };

        // The live stream already sees every message; a REST read of a channel
        // the bot cannot view would fail, so the streamed cache is both cheaper
        // and more useful once a subscription is active.
        if self.state.is_streaming().await {
            let recent = self.state.recent.lock().await.clone();
            let filtered: Vec<Value> = recent
                .iter()
                .filter(|message| {
                    message.get("channel_id").and_then(Value::as_str) == Some(channel_id.as_str())
                })
                .cloned()
                .collect();
            return tool_success(json!({
                "ok": true,
                "subscribed": true,
                "messages": filtered,
                "note": "A live subscription is active. These are the most recent messages streamed by the Gateway for this channel."
            }));
        }

        let limit = args
            .get("limit")
            .and_then(Value::as_i64)
            .map(|n| n.clamp(1, MAX_MESSAGES_LIMIT as i64))
            .unwrap_or(50);
        let mut path = format!("channels/{channel_id}/messages?limit={limit}");
        if let Some(raw) = opt_str(args, "before") {
            match normalize_snowflake(&raw) {
                Ok(before) => path.push_str(&format!("&before={before}")),
                Err(e) => return tool_error(e.to_string()),
            }
        }

        self.call_api(token, path, Value::Null).await
    }

    async fn call_api(&self, token: &str, path: String, body: Value) -> CallToolResponse {
        let result = if body.is_null() {
            self.client.get(token, &path).await
        } else {
            self.client.post(token, &path, &body).await
        };
        match result {
            Ok(value) => tool_success(value),
            Err(e) => tool_error(e.user_message()),
        }
    }

    /// Dispatch one table row: fill the default channel when the path needs one,
    /// validate the arguments, then issue exactly the call the table describes.
    async fn call_spec(
        &self,
        token: Option<&str>,
        spec: &'static ToolSpec,
        args: &mut Map<String, Value>,
        agent_cfg: Option<&AgentDiscordConfig>,
    ) -> CallToolResponse {
        // A message tool may omit `channel_id`; the per-agent default applies.
        if spec.path.first() == Some(&"channels")
            && !args.contains_key("channel_id")
            && let Some(channel) = resolve_channel_id(args, agent_cfg, &self.cfg)
        {
            args.insert("channel_id".to_string(), json!(channel));
        }

        let reason = args.remove("reason").and_then(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|reason| !reason.is_empty())
                .map(String::from)
        });
        if let Some(reason) = reason.as_deref()
            && reason.chars().count() > tools::MAX_AUDIT_REASON_CHARS
        {
            return tool_error(format!(
                "'reason' exceeds Discord's {}-character audit-log limit",
                tools::MAX_AUDIT_REASON_CHARS
            ));
        }

        let path = match tools::build_path(spec, args) {
            Ok(path) => path,
            Err(e) => return tool_error(e),
        };
        let body = match tools::build_body(spec, args) {
            Ok(body) => body,
            Err(e) => return tool_error(e),
        };
        let body_ref = (!body.is_null()).then_some(&body);
        let method = method_for(spec.verb);

        // Executing a webhook is public: it must not present the bot token, and
        // it must work when no token is configured at all.
        let result = if spec.name == "discord_execute_webhook" {
            self.client.call_anonymous(method, &path, body_ref).await
        } else if let Some(token) = token {
            self.client
                .call(token, method, &path, body_ref, reason.as_deref())
                .await
        } else {
            return tool_error(NO_TOKEN);
        };

        match result {
            Ok(value) => tool_success(value),
            Err(e) => tool_error(e.user_message()),
        }
    }

    /// Open (or reuse) a DM channel, then post the message into it.
    async fn send_dm(&self, token: &str, args: &Map<String, Value>) -> CallToolResponse {
        let Some(raw_recipient) = opt_str(args, "recipient_id") else {
            return tool_error("'recipient_id' is required");
        };
        let recipient_id = match normalize_snowflake(&raw_recipient) {
            Ok(id) => id,
            Err(e) => return tool_error(e.to_string()),
        };

        let channel = match self
            .client
            .post(
                token,
                "users/@me/channels",
                &json!({ "recipient_id": recipient_id }),
            )
            .await
        {
            Ok(channel) => channel,
            Err(e) => return tool_error(e.user_message()),
        };
        let Some(channel_id) = channel.get("id").and_then(Value::as_str) else {
            return tool_error("Discord did not return a DM channel id");
        };

        let Some(spec) = tools::find("discord_send_message") else {
            return tool_error("internal error: the send_message spec is missing");
        };
        let mut message_args = args.clone();
        message_args.insert("channel_id".to_string(), json!(channel_id));
        let path = match tools::build_path(spec, &message_args) {
            Ok(path) => path,
            Err(e) => return tool_error(e),
        };
        let body = match tools::build_body(spec, &message_args) {
            Ok(body) => body,
            Err(e) => return tool_error(e),
        };

        match self
            .client
            .call(token, Method::POST, &path, Some(&body), None)
            .await
        {
            Ok(message) => tool_success(json!({ "channel_id": channel_id, "message": message })),
            Err(e) => tool_error(e.user_message()),
        }
    }

    /// `discord_api`: one guarded passthrough for the endpoints no named tool
    /// covers. See `passthrough.rs` for what shape is accepted.
    async fn call_passthrough(&self, token: &str, args: &Map<String, Value>) -> CallToolResponse {
        let request = match passthrough::build(args) {
            Ok(request) => request,
            Err(e) => return tool_error(e),
        };
        let reason = args
            .get("reason")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .filter(|reason| reason.chars().count() <= tools::MAX_AUDIT_REASON_CHARS);

        match self
            .client
            .call(
                token,
                method_for(request.verb),
                &request.path,
                request.body.as_ref(),
                reason,
            )
            .await
        {
            Ok(value) => tool_success(value),
            Err(e) => tool_error(e.user_message()),
        }
    }
}

impl ServerHandler for DiscordMcp {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_resources_subscribe()
                .build(),
        )
        .with_server_info(Implementation::new(
            "discord-mcp",
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
            "ui://discord-config".to_string(),
            "Discord Settings".to_string(),
        );
        config.mime_type = Some("text/html;profile=mcp-app".to_string());
        config.description =
            Some("Discord per-agent configuration and bot authentication".to_string());

        let mut messages = Resource::new(MESSAGES_URI.to_string(), "Discord Messages".to_string());
        messages.mime_type = Some("application/json".to_string());
        messages.description = Some(
            "Incoming Discord messages streamed from the Gateway as push notifications while subscribed"
                .to_string(),
        );

        Ok(ListResourcesResult::with_all_items(vec![config, messages]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        if request.uri == "ui://discord-config" {
            let mut content = ResourceContents::text(DISCORD_APP_HTML, "ui://discord-config");
            if let ResourceContents::TextResourceContents {
                ref mut mime_type, ..
            } = content
            {
                *mime_type = Some("text/html;profile=mcp-app".into());
            }
            return Ok(ReadResourceResult::new(vec![content]).into());
        }

        if request.uri == MESSAGES_URI {
            let recent = self.state.recent.lock().await.clone();
            let text = serde_json::to_string_pretty(&recent).unwrap_or_else(|_| "[]".to_string());
            let mut content = ResourceContents::text(text, MESSAGES_URI);
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
        let mut tools = Self::discord_tools();
        tools.extend(Self::config_tools());
        Ok(ListToolsResult::with_all_items(tools))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        Self::discord_tools()
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
            "discord_get_config" | "discord_set_config" | "discord_test_connection"
        ) {
            return self
                .handle_config_tool(&name, &args, authoritative_agent_id.as_deref())
                .await;
        }

        let agent_cfg = authoritative_agent_id
            .as_deref()
            .or_else(|| args.get("agent_id").and_then(Value::as_str))
            .and_then(AgentDiscordConfig::load);

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
                .call_spec(token.as_deref(), spec, &mut args, agent_cfg.as_ref())
                .await);
        }

        let Some(token) = token else {
            return Ok(tool_error(NO_TOKEN));
        };

        match name.as_str() {
            "discord_get_messages" => Ok(self.get_messages(&token, &args).await),
            "discord_send_dm" => Ok(self.send_dm(&token, &args).await),
            "discord_api" => Ok(self.call_passthrough(&token, &args).await),
            other => Ok(tool_error(format!("unknown tool {other:?}"))),
        }
    }

    #[allow(deprecated)]
    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        if request.uri != MESSAGES_URI {
            return Err(McpError::resource_not_found(request.uri, None));
        }

        *self.state.peer.lock().await = Some(context.peer.clone());
        self.state
            .start_stream(Arc::clone(&self.cfg), self.client.clone())
            .await;
        tracing::info!("subscribed to {MESSAGES_URI}");
        Ok(())
    }

    #[allow(deprecated)]
    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        if request.uri != MESSAGES_URI {
            return Ok(());
        }
        self.state.stop_stream().await;
        tracing::info!("unsubscribed from {MESSAGES_URI}");
        Ok(())
    }
}

/// Hold a Gateway connection and forward each incoming message to the peer.
///
/// `run_gateway` reconnects internally for recoverable failures and returns
/// only on a fatal close or a closed channel. Either way the token may have
/// been fixed in the meantime, so wait and start a new connection.
async fn run_stream(state: Arc<RuntimeState>, cfg: Arc<Config>, client: DiscordClient) {
    loop {
        let agent_id = state.active_agent_id.lock().await.clone();
        let agent_cfg = agent_id.as_deref().and_then(AgentDiscordConfig::load);
        let Some(token) = client.resolve_token(None, agent_cfg.as_ref()) else {
            tracing::warn!("discord message stream waiting for a bot token");
            tokio::time::sleep(Duration::from_secs(TOKEN_WAIT_SECS)).await;
            continue;
        };

        let (tx, mut rx) = mpsc::unbounded_channel();
        let gateway_task = tokio::spawn(gateway::run_gateway(cfg.gateway_url.clone(), token, tx));

        while let Some(event) = rx.recv().await {
            if !handle_event(&state, event, agent_id.as_deref()).await {
                gateway_task.abort();
                return;
            }
        }

        gateway_task.abort();
        tracing::warn!("discord gateway stream ended; reconnecting in {GATEWAY_RETRY_SECS}s");
        tokio::time::sleep(Duration::from_secs(GATEWAY_RETRY_SECS)).await;
    }
}

/// Returns `false` when the peer is gone and the stream should stop.
async fn handle_event(state: &RuntimeState, event: GatewayEvent, agent_id: Option<&str>) -> bool {
    if event.event != "MESSAGE_CREATE" {
        return true;
    }

    // A bot-authored message is either another bot or this plugin's own reply;
    // pushing it would wake the agent on its own output.
    if is_bot_authored(&event.data) {
        return true;
    }

    let summary = summarize_message(&event.data, agent_id);

    state.push_recent(summary.clone()).await;

    let peer = state.peer.lock().await.clone();
    let Some(peer) = peer else {
        return true;
    };
    let notification = ServerNotification::CustomNotification(CustomNotification::new(
        "notifications/resources/updated",
        Some(summary),
    ));
    if let Err(e) = peer.send_notification(notification).await {
        tracing::warn!("discord message stream stopped: {e}");
        return false;
    }
    true
}

/// Reshape a Gateway `MESSAGE_CREATE` payload into the host's push summary.
///
/// The host wakes an agent from a payload that carries `from` plus a non-empty
/// `text` (or `caption`), so the sender and the trimmed content are top level.
fn summarize_message(message: &Value, agent_id: Option<&str>) -> Value {
    let mut out = Map::new();
    out.insert("uri".to_string(), json!(MESSAGES_URI));
    out.insert("type".to_string(), json!("message"));
    out.insert("event".to_string(), json!("MESSAGE_CREATE"));
    if let Some(agent_id) = agent_id {
        out.insert("agent_id".to_string(), json!(agent_id));
    }

    let field = |key: &str| message.get(key).cloned();
    for key in ["id", "channel_id", "guild_id", "timestamp", "content"] {
        if let Some(value) = field(key) {
            out.insert(key.to_string(), value);
        }
    }
    // Rename the two fields the host actually reads.
    if let Some(id) = message.get("id") {
        out.insert("message_id".to_string(), id.clone());
    }
    if let Some(content) = message.get("content").and_then(Value::as_str) {
        out.insert("text".to_string(), json!(content.trim()));
    }
    if let Some(author) = message.get("author") {
        out.insert(
            "from".to_string(),
            json!({
                "id": author.get("id"),
                "username": author.get("username"),
                "global_name": author.get("global_name"),
                "bot": author.get("bot").and_then(Value::as_bool).unwrap_or(false),
            }),
        );
    }
    if let Some(attachments) = message.get("attachments").and_then(Value::as_array)
        && !attachments.is_empty()
    {
        // The host wakes an agent only from a non-empty `text` or `caption`, so
        // an attachment-only message needs a caption or it lands silently.
        if out
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .is_empty()
        {
            let names: Vec<&str> = attachments
                .iter()
                .filter_map(|a| a.get("filename").and_then(Value::as_str))
                .collect();
            let caption = if names.is_empty() {
                "sent an attachment".to_string()
            } else {
                format!("sent an attachment: {}", names.join(", "))
            };
            out.insert("caption".to_string(), json!(caption));
        }
        out.insert(
            "attachments".to_string(),
            Value::Array(
                attachments
                    .iter()
                    .map(|a| {
                        json!({
                            "id": a.get("id"),
                            "filename": a.get("filename"),
                            "content_type": a.get("content_type"),
                            "url": a.get("url"),
                        })
                    })
                    .collect(),
            ),
        );
    }

    Value::Object(out)
}

/// Discord echoes every message the bot posts back over the Gateway, so an
/// unfiltered stream would make the agent answer itself forever.
fn is_bot_authored(message: &Value) -> bool {
    message
        .get("author")
        .and_then(|author| author.get("bot"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn method_for(verb: Verb) -> Method {
    match verb {
        Verb::Get => Method::GET,
        Verb::Post => Method::POST,
        Verb::Patch => Method::PATCH,
        Verb::Put => Method::PUT,
        Verb::Delete => Method::DELETE,
    }
}

fn resolve_channel_id(
    args: &Map<String, Value>,
    agent: Option<&AgentDiscordConfig>,
    cfg: &Config,
) -> Option<String> {
    opt_str(args, "channel_id")
        .or_else(|| agent.and_then(|c| c.default_channel_id.clone()))
        .or_else(|| cfg.default_channel_id.clone())
        .and_then(|raw| normalize_snowflake(&raw).ok())
}

fn opt_str(args: &Map<String, Value>, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
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
            default_channel_id: None,
            api_base: "https://discord.com/api/v10".to_string(),
            gateway_url: "wss://gateway.discord.gg/?v=10&encoding=json".to_string(),
            timeout: Duration::from_secs(60),
        }
    }

    fn message_create(content: &str, bot: bool) -> Value {
        json!({
            "id": "1122334455667788990",
            "channel_id": "998877665544332211",
            "guild_id": "556677889900112233",
            "content": content,
            "timestamp": "2024-01-01T00:00:00.000000+00:00",
            "author": {
                "id": "123456789012345678",
                "username": "alice",
                "global_name": "Alice",
                "bot": bot
            },
            "attachments": []
        })
    }

    #[test]
    fn summarizes_a_guild_message() {
        let summary = summarize_message(&message_create("hello", false), Some("agent-1"));
        assert_eq!(summary["uri"], MESSAGES_URI);
        assert_eq!(summary["type"], "message");
        assert_eq!(summary["event"], "MESSAGE_CREATE");
        assert_eq!(summary["agent_id"], "agent-1");
        assert_eq!(summary["message_id"], "1122334455667788990");
        assert_eq!(summary["channel_id"], "998877665544332211");
        assert_eq!(summary["guild_id"], "556677889900112233");
        assert_eq!(summary["text"], "hello");
        assert_eq!(summary["from"]["id"], "123456789012345678");
        assert_eq!(summary["from"]["username"], "alice");
        assert_eq!(summary["from"]["bot"], false);
    }

    #[test]
    fn trims_whitespace_and_flags_bot_authors() {
        let summary = summarize_message(&message_create("  hi  ", true), None);
        assert_eq!(summary["text"], "hi");
        assert_eq!(summary["from"]["bot"], true);
        assert!(summary.get("agent_id").is_none());
    }

    #[test]
    fn keeps_attachment_metadata() {
        let mut message = message_create("", false);
        message["attachments"] = json!([
            { "id": "1", "filename": "cat.png", "content_type": "image/png", "url": "https://cdn.example/cat.png" }
        ]);
        let summary = summarize_message(&message, None);
        assert_eq!(summary["attachments"][0]["filename"], "cat.png");
    }

    #[test]
    fn attachment_only_messages_get_a_caption() {
        // The host needs text or caption to wake the agent, and an image post
        // has neither content nor a caption of its own.
        let mut message = message_create("   ", false);
        message["attachments"] = json!([{ "id": "1", "filename": "cat.png" }]);
        let summary = summarize_message(&message, None);
        assert_eq!(summary["caption"], "sent an attachment: cat.png");

        // A real caption already in `content` wins.
        let summary = summarize_message(&message_create("look", false), None);
        assert!(summary.get("caption").is_none());
    }

    #[test]
    fn resolves_channel_precedence_and_rejects_non_snowflakes() {
        let args = Map::new();
        let agent = AgentDiscordConfig {
            default_channel_id: Some("111".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_channel_id(&args, Some(&agent), &config()).as_deref(),
            Some("111")
        );

        let mut args = Map::new();
        args.insert("channel_id".into(), json!("222"));
        assert_eq!(
            resolve_channel_id(&args, Some(&agent), &config()).as_deref(),
            Some("222")
        );

        let mut args = Map::new();
        args.insert("channel_id".into(), json!("not-a-channel"));
        assert_eq!(resolve_channel_id(&args, Some(&agent), &config()), None);
    }

    /// The model only ever sees the list from `tools/list`, so every table row
    /// must actually be published, with a schema and with the annotations the
    /// row declares. A row that exists but is not listed is unreachable.
    #[test]
    fn every_table_row_is_published_with_matching_annotations() {
        let tools = DiscordMcp::discord_tools();
        let by_name: std::collections::HashMap<&str, &Tool> = tools
            .iter()
            .map(|tool| (tool.name.as_ref(), tool))
            .collect();

        assert_eq!(
            tools.len(),
            tools::TOOLS.len() + 3,
            "70 table rows plus get_messages, send_dm and the passthrough"
        );

        for spec in tools::TOOLS {
            let tool = by_name
                .get(spec.name)
                .unwrap_or_else(|| panic!("{} is not published", spec.name));
            assert_eq!(tool.description.as_deref(), Some(spec.description));
            let annotations = tool.annotations.as_ref().expect("annotations");
            assert_eq!(
                annotations.read_only_hint,
                Some(spec.verb.is_read_only()),
                "{}: read_only_hint must match the verb",
                spec.name
            );
            assert_eq!(
                annotations.destructive_hint,
                Some(spec.destructive),
                "{}: destructive_hint must match the row",
                spec.name
            );
            let schema = tool.input_schema.as_ref();
            assert_eq!(schema["type"], "object", "{}", spec.name);
        }

        for name in ["discord_get_messages", "discord_send_dm", "discord_api"] {
            assert!(by_name.contains_key(name), "{name} must be published");
        }

        // Names are unique: a duplicate would shadow the earlier tool.
        assert_eq!(by_name.len(), tools.len());
    }

    #[test]
    fn settings_tools_are_hidden_from_the_model() {
        // The host hides `_meta.ui.visibility: ["app"]` tools, so the settings
        // surface must carry that marker and no data tool may.
        for tool in DiscordMcp::config_tools() {
            let meta = tool.meta.as_ref().expect("settings tools carry meta");
            assert_eq!(meta.0["ui"]["visibility"][0], "app", "{}", tool.name);
        }
        for tool in DiscordMcp::discord_tools() {
            assert!(tool.meta.is_none(), "{} must stay visible", tool.name);
        }
    }

    #[test]
    fn drops_bot_authored_messages() {
        assert!(is_bot_authored(&message_create("sent by me", true)));
        assert!(!is_bot_authored(&message_create("sent by alice", false)));
        // A payload with no author block is not treated as a bot.
        assert!(!is_bot_authored(&json!({ "id": "1", "channel_id": "2" })));
    }
}
