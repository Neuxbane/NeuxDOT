use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, SET_COOKIE, COOKIE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::client::{McpError, McpNotification, McpTool, McpTransport};

/// Captures extracted from the response after tool execution
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CustomToolCaptures {
    /// If Some, captures all response `Set-Cookie` headers into this session variable (e.g. "COOKIES")
    #[serde(default)]
    pub cookies: Option<String>,
    /// Map of JSON-path or header to session variable name (e.g. "token": "TOKEN")
    #[serde(default)]
    pub json_fields: HashMap<String, String>,
}

/// Definition of an individual HTTP tool inside a Custom Plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomToolDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub cookie_expression: Option<String>,
    #[serde(default)]
    pub captures: CustomToolCaptures,
    #[serde(default)]
    pub input_schema: Option<Value>,
}

/// The declarative manifest for a `transport = "custom_tools"` plugin
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CustomPluginManifest {
    #[serde(default)]
    pub tools: Vec<CustomToolDef>,
}

/// Thread-safe in-memory session vault for custom plugins (storing $COOKIES, $TOKEN, etc.)
#[derive(Debug, Clone, Default)]
pub struct PluginSessionVault {
    pub variables: Arc<RwLock<HashMap<String, String>>>,
}

impl PluginSessionVault {
    pub fn new() -> Self {
        Self {
            variables: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn get(&self, key: &str) -> Option<String> {
        let read = self.variables.read().await;
        read.get(key).cloned()
    }

    pub async fn set(&self, key: String, value: String) {
        let mut write = self.variables.write().await;
        write.insert(key, value);
    }

    pub async fn get_all(&self) -> HashMap<String, String> {
        self.variables.read().await.clone()
    }
}

/// In-process safe MCP transport that evaluates declarative cURL/HTTP tools
pub struct CustomToolsTransport {
    pub plugin_id: String,
    pub manifest: CustomPluginManifest,
    pub session: PluginSessionVault,
    pub http_client: reqwest::Client,
    pub notif_tx: tokio::sync::mpsc::UnboundedSender<McpNotification>,
}

impl CustomToolsTransport {
    pub fn new(
        plugin_id: &str,
        manifest_json: &str,
        notif_tx: tokio::sync::mpsc::UnboundedSender<McpNotification>,
    ) -> Result<Self, McpError> {
        let manifest: CustomPluginManifest = serde_json::from_str(manifest_json)
            .map_err(|e| McpError::Protocol(format!("Invalid custom_tools manifest JSON: {}", e)))?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| McpError::Protocol(format!("Failed to build HTTP client: {}", e)))?;

        Ok(Self {
            plugin_id: plugin_id.to_string(),
            manifest,
            session: PluginSessionVault::new(),
            http_client: client,
            notif_tx,
        })
    }

    /// Resolve variables in a template string:
    /// - `{{arg}}` or `{arg}` is resolved from `arguments`
    /// - `$VAR` or `${VAR}` is resolved from `session`
    pub async fn interpolate(
        template: &str,
        arguments: &Value,
        session: &PluginSessionVault,
    ) -> String {
        let mut result = template.to_string();

        // 1. Replace session variables: $VAR or ${VAR}
        let session_vars = session.get_all().await;
        for (var_name, val) in &session_vars {
            let braced = format!("${{{}}}", var_name);
            let unbraced = format!("${}", var_name);
            result = result.replace(&braced, val);
            result = result.replace(&unbraced, val);
        }

        // 2. Replace LLM arguments: {{arg}} or {arg}
        if let Some(obj) = arguments.as_object() {
            for (key, val) in obj {
                let val_str = match val {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    other => other.to_string(),
                };
                let double_brace = format!("{{{{{}}}}}", key);
                let single_brace = format!("{{{}}}", key);
                result = result.replace(&double_brace, &val_str);
                result = result.replace(&single_brace, &val_str);
            }
        }

        result
    }

    /// Execute a tool definition and update the plugin session with captured cookies or variables
    pub async fn execute_tool_full(
        &self,
        tool: &CustomToolDef,
        arguments: &Value,
    ) -> Result<(u16, String), McpError> {
        let method_str = tool.method.to_uppercase();
        let method = reqwest::Method::from_bytes(method_str.as_bytes())
            .map_err(|e| McpError::Protocol(format!("Invalid HTTP method '{}': {}", method_str, e)))?;

        // Interpolate URL
        let url_str = Self::interpolate(&tool.url, arguments, &self.session).await;
        let url = reqwest::Url::parse(&url_str)
            .map_err(|e| McpError::Protocol(format!("Invalid URL '{}': {}", url_str, e)))?;

        // Guard against SSRF to private addresses if required (allowing standard http/https)
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(McpError::Protocol("Only http and https schemes are permitted".into()));
        }

        let mut req_builder = self.http_client.request(method, url);

        // Interpolate and append headers
        let mut header_map = HeaderMap::new();
        for (h_name, h_val) in &tool.headers {
            let interpolated_val = Self::interpolate(h_val, arguments, &self.session).await;
            if let (Ok(name), Ok(val)) = (
                HeaderName::from_bytes(h_name.as_bytes()),
                HeaderValue::from_str(&interpolated_val),
            ) {
                header_map.insert(name, val);
            }
        }

        // Cookie handling: explicit cookie_expression or fallback to $COOKIES in session
        let mut cookie_to_send = String::new();
        if let Some(cookie_expr) = &tool.cookie_expression {
            cookie_to_send = Self::interpolate(cookie_expr, arguments, &self.session).await;
        } else if let Some(stored_cookies) = self.session.get("COOKIES").await {
            cookie_to_send = stored_cookies;
        }

        if !cookie_to_send.is_empty() {
            if let Ok(cookie_val) = HeaderValue::from_str(&cookie_to_send) {
                header_map.insert(COOKIE, cookie_val);
            }
        }

        req_builder = req_builder.headers(header_map);

        // Interpolate body if present
        if let Some(body_template) = &tool.body {
            let interpolated_body = Self::interpolate(body_template, arguments, &self.session).await;
            req_builder = req_builder.body(interpolated_body);
        }

        debug!("CustomToolsTransport executing {} to {}", tool.method, url_str);

        let response = req_builder
            .send()
            .await
            .map_err(|e| McpError::Http(e))?;

        let status = response.status();

        // 1. Capture cookies if configured
        if let Some(cookie_var_name) = &tool.captures.cookies {
            let mut cookie_pairs = Vec::new();
            for cookie_header in response.headers().get_all(SET_COOKIE) {
                if let Ok(cookie_str) = cookie_header.to_str() {
                    // Extract name=value (before any semicolon)
                    if let Some(pair) = cookie_str.split(';').next() {
                        let trimmed = pair.trim();
                        if !trimmed.is_empty() {
                            cookie_pairs.push(trimmed.to_string());
                        }
                    }
                }
            }
            if !cookie_pairs.is_empty() {
                let combined_cookies = cookie_pairs.join("; ");
                self.session.set(cookie_var_name.clone(), combined_cookies.clone()).await;
                debug!("Captured {} = {}", cookie_var_name, combined_cookies);
            }
        }

        let resp_text = response
            .text()
            .await
            .unwrap_or_else(|e| format!("[Failed to read response body: {}]", e));

        // 2. Capture JSON fields if configured
        if !tool.captures.json_fields.is_empty() {
            if let Ok(json_val) = serde_json::from_str::<Value>(&resp_text) {
                for (field_path, var_name) in &tool.captures.json_fields {
                    let mut current = &json_val;
                    for part in field_path.split('.') {
                        if let Some(next) = current.get(part) {
                            current = next;
                        }
                    }
                    let extracted = match current {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        Value::Null => String::new(),
                        other => other.to_string(),
                    };
                    if !extracted.is_empty() {
                        self.session.set(var_name.clone(), extracted.clone()).await;
                        debug!("Captured {} = {}", var_name, extracted);
                    }
                }
            }
        }

        if !status.is_success() {
            warn!("HTTP custom tool '{}' returned status {}", tool.name, status);
        }

        Ok((status.as_u16(), resp_text))
    }

    pub async fn execute_tool(
        &self,
        tool: &CustomToolDef,
        arguments: &Value,
    ) -> Result<String, McpError> {
        let (_, body) = self.execute_tool_full(tool, arguments).await?;
        Ok(body)
    }

    /// Helper for testing a tool definition standalone with a given session state
    pub async fn execute_standalone(
        tool: &CustomToolDef,
        arguments: &Value,
        session_vars: &HashMap<String, String>,
    ) -> Result<(u16, String, HashMap<String, String>), String> {
        let session = PluginSessionVault::new();
        for (k, v) in session_vars {
            session.set(k.clone(), v.clone()).await;
        }
        let (notif_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let transport = CustomToolsTransport::new(
            "test_standalone",
            &json!({ "tools": [tool.clone()] }).to_string(),
            notif_tx,
        )
        .map_err(|e| e.to_string())?;

        // Swap in session with provided vars
        let custom_transport = CustomToolsTransport {
            plugin_id: transport.plugin_id,
            manifest: transport.manifest,
            session,
            http_client: transport.http_client,
            notif_tx: transport.notif_tx,
        };

        let (status, resp_text) = custom_transport
            .execute_tool_full(tool, arguments)
            .await
            .map_err(|e| e.to_string())?;

        let updated_vars = custom_transport.session.get_all().await;
        Ok((status, resp_text, updated_vars))
    }
}

#[async_trait]
impl McpTransport for CustomToolsTransport {
    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
        match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": { "listChanged": false }
                },
                "serverInfo": {
                    "name": format!("custom-tools:{}", self.plugin_id),
                    "version": "1.0.0"
                }
            })),

            "notifications/initialized" => Ok(Value::Null),

            "tools/list" => {
                let mcp_tools: Vec<McpTool> = self
                    .manifest
                    .tools
                    .iter()
                    .map(|t| McpTool {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        input_schema: t.input_schema.clone().or_else(|| {
                            Some(json!({
                                "type": "object",
                                "properties": {},
                                "required": []
                            }))
                        }),
                        meta: None,
                    })
                    .collect();

                Ok(json!({ "tools": mcp_tools }))
            }

            "tools/call" => {
                let p = params.ok_or_else(|| McpError::Protocol("Missing params for tools/call".into()))?;
                let tool_name = p
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| McpError::Protocol("Missing tool name".into()))?;

                let arguments = p.get("arguments").cloned().unwrap_or(json!({}));

                let tool_def = self
                    .manifest
                    .tools
                    .iter()
                    .find(|t| t.name == tool_name)
                    .ok_or_else(|| McpError::ToolNotFound(tool_name.to_string()))?;

                let result_text = self.execute_tool(tool_def, &arguments).await?;

                Ok(json!({
                    "content": [
                        {
                            "type": "text",
                            "text": result_text
                        }
                    ],
                    "isError": false
                }))
            }

            "resources/list" => Ok(json!({ "resources": [] })),

            "resources/read" => Err(McpError::Protocol("Resources not supported on custom tools".into())),

            _ => Err(McpError::Protocol(format!("Method not supported: {}", method))),
        }
    }

    async fn close(&self) -> Result<(), McpError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_interpolation_with_args_and_session() {
        let session = PluginSessionVault::new();
        session.set("COOKIES".into(), "session_token=xyz123".into()).await;
        session.set("BASE_URL".into(), "https://api.example.com".into()).await;

        let template = "${BASE_URL}/users/{{user_id}}?role={{role}}&c=$COOKIES";
        let args = json!({
            "user_id": "42",
            "role": "admin"
        });

        let interpolated = CustomToolsTransport::interpolate(template, &args, &session).await;
        assert_eq!(
            interpolated,
            "https://api.example.com/users/42?role=admin&c=session_token=xyz123"
        );
    }
}
