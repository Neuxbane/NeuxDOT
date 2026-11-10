//! The MCP surface: tool listing, dispatch and result formatting.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ServerCapabilities, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::{json, Map, Value};

use crate::config::{normalize_urls, AgentGitLabConfig};
use crate::exec::Executor;
use crate::registry::{Registry, ToolEntry};

const GITLAB_APP_HTML: &str = include_str!("app.html");

pub struct GitLabMcp {
    registry: Arc<Registry>,
    exec: Arc<Executor>,
}

impl GitLabMcp {
    pub fn new(registry: Arc<Registry>, exec: Arc<Executor>) -> Self {
        Self { registry, exec }
    }

    fn to_mcp_tool(entry: &ToolEntry) -> Tool {
        let mut schema = entry.meta.input_schema.clone();
        if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            if !props.contains_key("gitlab_token") {
                props.insert(
                    "gitlab_token".to_string(),
                    json!({
                        "type": "string",
                        "description": "Optional GitLab Personal Access Token or Bearer token for stateless per-request authentication"
                    }),
                );
            }
        }

        Tool::new(
            entry.meta.name.clone(),
            entry.meta.description.clone(),
            Arc::new(schema),
        )
        .annotate(
            ToolAnnotations::default()
                .read_only(entry.endpoint.read_only)
                .destructive(entry.endpoint.destructive)
                .idempotent(matches!(
                    entry.endpoint.method.as_deref(),
                    Some("GET") | Some("PUT") | Some("DELETE")
                ))
                .open_world(true),
        )
    }

    fn config_tools() -> Vec<Tool> {
        let make_schema = |val: Value| -> Arc<Map<String, Value>> {
            match val {
                Value::Object(m) => Arc::new(m),
                _ => unreachable!(),
            }
        };

        vec![
            Tool::new(
                "gitlab_get_config".to_string(),
                "Retrieve GitLab configuration for an agent".to_string(),
                make_schema(json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "The unique ID of the agent" }
                    },
                    "required": ["agent_id"]
                })),
            )
            .annotate(ToolAnnotations::default().read_only(true)),

            Tool::new(
                "gitlab_set_config".to_string(),
                "Save or reset per-agent GitLab configuration".to_string(),
                make_schema(json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "The unique ID of the agent" },
                        "domain": { "type": "string", "description": "GitLab domain instance URL" },
                        "token": { "type": "string", "description": "Personal Access Token" },
                        "default_project_id": { "type": "string", "description": "Default project ID or path" },
                        "read_only": { "type": "boolean", "description": "Enforce read-only tool execution" },
                        "reset": { "type": "boolean", "description": "If true, revert to global settings" }
                    },
                    "required": ["agent_id"]
                })),
            )
            .annotate(ToolAnnotations::default().destructive(false)),

            Tool::new(
                "gitlab_test_connection".to_string(),
                "Test connectivity and token validity against a GitLab instance".to_string(),
                make_schema(json!({
                    "type": "object",
                    "properties": {
                        "domain": { "type": "string", "description": "GitLab instance URL (e.g. https://gitlab.com)" },
                        "token": { "type": "string", "description": "GitLab Personal Access Token" }
                    },
                    "required": ["domain", "token"]
                })),
            )
            .annotate(ToolAnnotations::default().read_only(true)),
        ]
    }

    fn instructions(&self) -> String {
        let visible = self.registry.visible().len();
        let inactive: Vec<&str> = {
            let active = self.registry.active_categories();
            self.registry
                .toolsets()
                .iter()
                .filter(|t| !active.contains(&t.id))
                .map(|t| t.id.as_str())
                .collect()
        };
        let mut s = format!(
            "GitLab API access. {visible} tools are active. Project arguments accept either a \
             numeric id or a URL-encoded path such as group/subgroup/project. \
             Authentication is stateless: tools accept an optional 'gitlab_token' or 'token' parameter \
             for per-request credentials."
        );
        if self.registry.read_only() {
            s.push_str(" The server runs in read-only mode, so no tool can change GitLab state.");
        }
        if !inactive.is_empty() {
            s.push_str(&format!(
                " Further categories are available on request via discover_tools: {}.",
                inactive.join(", ")
            ));
        }
        s
    }

    async fn handle_config_tool(&self, name: &str, args: &Map<String, Value>) -> Result<CallToolResponse, McpError> {
        match name {
            "gitlab_get_config" => {
                let agent_id = args.get("agent_id").and_then(Value::as_str).unwrap_or("");
                let agent_cfg = if agent_id.is_empty() {
                    None
                } else {
                    AgentGitLabConfig::load(agent_id)
                };

                let resp = if let Some(cfg) = agent_cfg {
                    json!({
                        "has_override": true,
                        "domain": cfg.api_url.as_deref().unwrap_or(&self.exec.api_url_root()),
                        "token": cfg.token.as_deref().unwrap_or(""),
                        "default_project_id": cfg.default_project_id,
                        "read_only": cfg.read_only.unwrap_or(self.exec.read_only),
                    })
                } else {
                    json!({
                        "has_override": false,
                        "domain": self.exec.api_url_root(),
                        "token": if self.exec.has_global_token() { "••••••••" } else { "" },
                        "default_project_id": null,
                        "read_only": self.exec.read_only,
                    })
                };

                Ok(CallToolResult::success(vec![ContentBlock::text(resp.to_string())]).into())
            }
            "gitlab_set_config" => {
                let agent_id = args.get("agent_id").and_then(Value::as_str).unwrap_or("");
                if agent_id.is_empty() {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
                        "Missing required 'agent_id' parameter",
                    )])
                    .into());
                }

                let reset = args.get("reset").and_then(Value::as_bool).unwrap_or(false);
                if reset {
                    let _ = AgentGitLabConfig::delete(agent_id);
                    return Ok(CallToolResult::success(vec![ContentBlock::text(
                        json!({ "ok": true, "reset": true }).to_string(),
                    )])
                    .into());
                }

                let domain = args.get("domain").and_then(Value::as_str).unwrap_or("https://gitlab.com");
                let (api_url, _) = normalize_urls(domain).unwrap_or_else(|_| (
                    "https://gitlab.com/api/v4".to_string(),
                    "https://gitlab.com/api/graphql".to_string(),
                ));
                let token = args.get("token").and_then(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                let default_project_id = args.get("default_project_id").and_then(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                let read_only = args.get("read_only").and_then(Value::as_bool);

                let cfg = AgentGitLabConfig {
                    api_url: Some(api_url),
                    token,
                    default_project_id,
                    read_only,
                };

                if let Err(e) = AgentGitLabConfig::save(agent_id, &cfg) {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                        "Failed to persist configuration: {e}"
                    ))])
                    .into());
                }

                Ok(CallToolResult::success(vec![ContentBlock::text(
                    json!({ "ok": true, "saved": true }).to_string(),
                )])
                .into())
            }
            "gitlab_test_connection" => {
                let domain = args.get("domain").and_then(Value::as_str).unwrap_or("https://gitlab.com");
                let token = args.get("token").and_then(Value::as_str).unwrap_or("").trim();

                if token.is_empty() {
                    return Ok(CallToolResult::success(vec![ContentBlock::text(
                        json!({ "ok": false, "error": "Token cannot be empty." }).to_string(),
                    )])
                    .into());
                }

                let (api_url, _) = normalize_urls(domain).unwrap_or_else(|_| (
                    "https://gitlab.com/api/v4".to_string(),
                    "https://gitlab.com/api/graphql".to_string(),
                ));
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(10))
                    .build()
                    .unwrap_or_default();

                let user_url = format!("{}/user", api_url.trim_end_matches('/'));
                let user_res = client
                    .get(&user_url)
                    .header("PRIVATE-TOKEN", token)
                    .send()
                    .await;

                match user_res {
                    Ok(resp) => {
                        let status = resp.status();
                        if status == reqwest::StatusCode::UNAUTHORIZED {
                            let body = json!({
                                "ok": false,
                                "error": "Invalid or expired GitLab Personal Access Token (HTTP 401 Unauthorized)."
                            });
                            return Ok(CallToolResult::success(vec![ContentBlock::text(body.to_string())]).into());
                        }

                        if !status.is_success() {
                            let text = resp.text().await.unwrap_or_default();
                            let body = json!({
                                "ok": false,
                                "error": format!("GitLab API error (HTTP {status}): {text}")
                            });
                            return Ok(CallToolResult::success(vec![ContentBlock::text(body.to_string())]).into());
                        }

                        let user_data: Value = resp.json().await.unwrap_or_default();

                        // Query version
                        let version_url = format!("{}/version", api_url.trim_end_matches('/'));
                        let version = if let Ok(vres) = client.get(&version_url).header("PRIVATE-TOKEN", token).send().await {
                            if vres.status().is_success() {
                                let vdata: Value = vres.json().await.unwrap_or_default();
                                vdata.get("version").and_then(Value::as_str).map(|s| s.to_string())
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        let body = json!({
                            "ok": true,
                            "user": {
                                "id": user_data.get("id"),
                                "name": user_data.get("name"),
                                "username": user_data.get("username"),
                                "avatar_url": user_data.get("avatar_url"),
                                "email": user_data.get("email"),
                            },
                            "version": version
                        });

                        Ok(CallToolResult::success(vec![ContentBlock::text(body.to_string())]).into())
                    }
                    Err(e) => {
                        let body = json!({
                            "ok": false,
                            "error": format!("Network connection failed: {e}")
                        });
                        Ok(CallToolResult::success(vec![ContentBlock::text(body.to_string())]).into())
                    }
                }
            }
            _ => Ok(CallToolResult::error(vec![ContentBlock::text(format!("Unknown config tool: {name}"))]).into()),
        }
    }
}

impl ServerHandler for GitLabMcp {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("gitlab-mcp", env!("CARGO_PKG_VERSION")))
        .with_instructions(self.instructions())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let mut r = Resource::new(
            "ui://gitlab-config".to_string(),
            "GitLab Settings".to_string(),
        );
        r.mime_type = Some("text/html;profile=mcp-app".to_string());
        r.description = Some("GitLab Per-Agent Configuration & Authentication".to_string());
        Ok(ListResourcesResult::with_all_items(vec![r]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        if request.uri == "ui://gitlab-config" {
            let mut content = ResourceContents::text(GITLAB_APP_HTML, "ui://gitlab-config");
            if let ResourceContents::TextResourceContents { ref mut mime_type, .. } = content {
                *mime_type = Some("text/html;profile=mcp-app".into());
            }
            Ok(ReadResourceResult::new(vec![content]).into())
        } else {
            Err(McpError::resource_not_found(request.uri, None))
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools: Vec<Tool> = self.registry.visible().iter().map(|e| Self::to_mcp_tool(e)).collect();
        tools.extend(Self::config_tools());
        Ok(ListToolsResult::with_all_items(tools))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        for t in Self::config_tools() {
            if t.name == name {
                return Some(t);
            }
        }
        self.registry.get(name).map(|e| Self::to_mcp_tool(&e))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name.to_string();

        if name == "gitlab_get_config" || name == "gitlab_set_config" || name == "gitlab_test_connection" {
            let args: Map<String, Value> = request.arguments.unwrap_or_default();
            return self.handle_config_tool(&name, &args).await;
        }

        let Some(entry) = self.registry.get(&name) else {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "unknown tool {name:?}"
            ))])
            .into());
        };

        if !self.registry.is_callable(&entry) {
            let reason = if self.registry.read_only() && !entry.endpoint.read_only {
                format!("{name} changes GitLab state and the server is in read-only mode")
            } else {
                format!(
                    "{name} belongs to the inactive category {:?}; call discover_tools with that \
                     category to enable it",
                    entry.endpoint.category
                )
            };
            return Ok(CallToolResult::error(vec![ContentBlock::text(reason)]).into());
        }

        let args: Map<String, Value> = request.arguments.unwrap_or_default();
        let listing_may_change = entry.meta.name == "discover_tools";

        match self.exec.call(&entry, args).await {
            Ok(value) => {
                let text = match &value {
                    Value::String(s) => s.clone(),
                    other => serde_json::to_string(other)
                        .unwrap_or_else(|e| format!("could not serialise the result: {e}")),
                };
                if listing_may_change {
                    tracing::info!("tool list changed after discover_tools");
                }
                Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into())
            }
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e.to_string())]).into()),
        }
    }
}
