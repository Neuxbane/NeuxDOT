use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Role assigned to an account that holds full administrative privileges.
pub const ROLE_ROOT: &str = "root";
/// Default role for regular accounts.
pub const ROLE_MEMBER: &str = "member";

/// Returns true when `role` is a recognised account role.
pub fn is_valid_role(role: &str) -> bool {
    role == ROLE_ROOT || role == ROLE_MEMBER
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Account {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    #[sqlx(default)]
    pub role: String,
    pub created_at: Option<DateTime<Utc>>,
}

impl Account {
    pub fn is_root(&self) -> bool {
        self.role == ROLE_ROOT
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Profile {
    pub account_id: String,
    pub display_name: Option<String>,
    pub avatar_color: Option<String>,
    pub avatar_shape: Option<String>,
    #[sqlx(default)]
    pub avatar_url: Option<String>,
}

/// A reusable system instruction ("Agent Instructions" preset).
/// Strictly owner-scoped by `account_id`.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentInstructionPreset {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Provider {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ProviderModel {
    pub id: String,
    pub provider_id: String,
    pub model_id: String,
    pub name: Option<String>,
    pub config_json: Option<String>,
    #[serde(default)]
    #[sqlx(default)]
    pub params_json: Option<String>,
    #[serde(default)]
    #[sqlx(default)]
    pub context_window: Option<i64>,
    #[sqlx(default)]
    pub is_active: bool,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfigInput {
    pub model_id: String,
    pub name: Option<String>,
    pub config_json: Option<String>,
    #[serde(default)]
    pub params_json: Option<String>,
    #[serde(default)]
    pub context_window: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderWithModels {
    #[serde(flatten)]
    pub provider: Provider,
    pub models: Vec<ProviderModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ActiveModelTarget {
    pub id: String, // provider_model_id
    pub provider_id: String,
    pub provider_name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub model_id: String,
    pub model_name: Option<String>,
    pub config_json: Option<String>,
    #[serde(default)]
    #[sqlx(default)]
    pub params_json: Option<String>,
    #[serde(default)]
    #[sqlx(default)]
    pub context_window: Option<i64>,
}

fn default_hook_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn default_hook_source() -> String {
    "user".to_string()
}
fn default_hook_enabled() -> bool {
    true
}
fn default_max_running_history_chars() -> i64 {
    300_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseHook {
    #[serde(default = "default_hook_id")]
    pub id: String,
    pub message: String,
    #[serde(default = "default_hook_source")]
    pub source: String,
    #[serde(default = "default_hook_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Agent {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub avatar_color: Option<String>,
    pub avatar_shape: Option<String>,
    #[sqlx(default)]
    pub avatar_url: Option<String>,
    pub strategy: String,
    pub system_prompt: Option<String>,
    pub is_active: bool,
    #[sqlx(default)]
    pub allow_output_text: bool,
    #[sqlx(default)]
    pub output_text_error_message: Option<String>,
    #[serde(default = "default_max_running_history_chars")]
    #[sqlx(default)]
    pub max_running_history_chars: i64,
    #[sqlx(default)]
    pub response_hooks_json: Option<String>,
    /// Selected Agent Instructions preset; `None` = Custom (free-text prompt).
    #[sqlx(default)]
    pub instruction_preset_id: Option<String>,
}

impl Agent {
    pub fn max_history_chars(&self) -> usize {
        if self.max_running_history_chars > 0 {
            self.max_running_history_chars as usize
        } else {
            300_000
        }
    }

    pub fn get_response_hooks(&self) -> Vec<ResponseHook> {
        let json_str = self.response_hooks_json.as_deref().unwrap_or("[]");
        if let Ok(hooks) = serde_json::from_str::<Vec<ResponseHook>>(json_str) {
            return hooks;
        }
        if let Ok(strings) = serde_json::from_str::<Vec<String>>(json_str) {
            return strings
                .into_iter()
                .map(|msg| ResponseHook {
                    id: uuid::Uuid::new_v4().to_string(),
                    message: msg,
                    source: "user".to_string(),
                    enabled: true,
                })
                .collect();
        }
        Vec::new()
    }
}

/// One account's access to an agent it does not own.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentShare {
    pub id: String,
    pub agent_id: String,
    pub account_id: String,
    #[sqlx(default)]
    pub can_edit: bool,
    #[sqlx(default)]
    pub can_manage_shares: bool,
    pub created_at: Option<DateTime<Utc>>,
}

/// Resolved rights of an account over a given agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentAccess {
    Owner,
    Shared {
        can_edit: bool,
        can_manage_shares: bool,
    },
    None,
}

impl AgentAccess {
    pub fn can_view(self) -> bool {
        !matches!(self, AgentAccess::None)
    }

    pub fn can_edit(self) -> bool {
        match self {
            AgentAccess::Owner => true,
            AgentAccess::Shared { can_edit, .. } => can_edit,
            AgentAccess::None => false,
        }
    }

    pub fn can_manage_shares(self) -> bool {
        match self {
            AgentAccess::Owner => true,
            AgentAccess::Shared {
                can_manage_shares, ..
            } => can_manage_shares,
            AgentAccess::None => false,
        }
    }

    pub fn is_owner(self) -> bool {
        matches!(self, AgentAccess::Owner)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentProvider {
    pub agent_id: String,
    pub provider_model_id: String,
    pub priority: i32,
}

fn default_plugin_version() -> String {
    "1.0.0".to_string()
}
fn default_true() -> bool {
    true
}

/// Marker stored in `ui.resources` to mean "every ui:// resource".
/// normalised. Matches any `ui://` URI.
pub const UI_RESOURCE_WILDCARD: &str = "*";

/// Marker in `network.allow` meaning "every destination".
pub const NETWORK_WILDCARD: &str = "*";

/// Canonical capability strings (see `docs/HOST_BRIDGE.md` §1). Only granted
/// capabilities appear in a host bootstrap payload.
pub const CAP_UI_CONTEXT_REFRESH: &str = "ui.context.refresh";
pub const CAP_STORAGE_GET: &str = "storage.get";
pub const CAP_STORAGE_SET: &str = "storage.set";
pub const CAP_STORAGE_DELETE: &str = "storage.delete";
pub const CAP_STORAGE_KEYS: &str = "storage.keys";
pub const CAP_AGENT_TRIGGER: &str = "agent.trigger";
pub const CAP_IDENTITY_AGENT_NAME: &str = "identity.agent.name";
pub const CAP_IDENTITY_AGENT_PICTURE: &str = "identity.agent.profile_picture";
pub const CAP_IDENTITY_USER_NAME: &str = "identity.user.name";
pub const CAP_IDENTITY_USER_PICTURE: &str = "identity.user.profile_picture";

/// Resolve live display data for account ids an app already references.
/// Granted whenever identity read is available, since it exposes the same
/// non-sensitive fields (name/avatar) for authors of messages the app shows.
pub const CAP_IDENTITY_RESOLVE: &str = "identity.resolve";

/// How a sandboxed MCP-App UI may use the `ui://` resources of a plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiPermissions {
    #[serde(default)]
    pub enabled: bool,
    /// Allow-list of `ui://` URIs. `["*"]` (or an empty list) means "any".
    #[serde(default)]
    pub resources: Vec<String>,
}

impl Default for UiPermissions {
    fn default() -> Self {
        Self {
            enabled: true,
            resources: vec![UI_RESOURCE_WILDCARD.to_string()],
        }
    }
}

/// Isolated per-plugin key/value storage grants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoragePermissions {
    #[serde(default)]
    pub get: bool,
    #[serde(default)]
    pub set: bool,
    #[serde(default)]
    pub delete: bool,
    /// Key **prefix** allow-list; `None` means "every key".
    #[serde(default)]
    pub keys: Option<Vec<String>>,
}

impl Default for StoragePermissions {
    fn default() -> Self {
        Self {
            get: true,
            set: true,
            delete: true,
            keys: None,
        }
    }
}

/// Permission to drive an agent from inside the hosted UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentPermissions {
    #[serde(default)]
    pub trigger: bool,
    #[serde(default)]
    pub tools: bool,
}

impl Default for AgentPermissions {
    fn default() -> Self {
        Self {
            trigger: true,
            tools: true,
        }
    }
}

/// Consent to expose the signed-in account and/or the bound agent's identity to
/// the hosted UI. Every grant defaults to **false** (privacy by default).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct IdentityPermissions {
    #[serde(default, rename = "agent.name")]
    pub agent_name: bool,
    #[serde(default, rename = "agent.profile_picture")]
    pub agent_profile_picture: bool,
    #[serde(default, rename = "user.name")]
    pub user_name: bool,
    #[serde(default, rename = "user.profile_picture")]
    pub user_profile_picture: bool,
}

impl IdentityPermissions {
    /// All four identity grants.
    pub fn all_granted() -> Self {
        Self {
            agent_name: true,
            agent_profile_picture: true,
            user_name: true,
            user_profile_picture: true,
        }
    }
}

/// Resolved grants for a plugin. See `docs/HOST_BRIDGE.md` §2 for the
/// canonical JSON shape — this struct serialises to exactly that.
///
/// The flat boolean permission blob of earlier releases is **not** supported:
/// only the canonical grouped shape deserialises. Missing sections default to
/// *not* granted.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PluginPermissions {
    pub ui: UiPermissions,
    pub storage: StoragePermissions,
    pub agent: AgentPermissions,
    pub identity: IdentityPermissions,
    /// Process spawning (stdio plugins). Out of scope for the consent editor.
    #[serde(default = "default_true")]
    pub process: bool,
    /// Outbound network grants. Empty means "no network at all", which is the
    /// default for a sandboxed plugin.
    #[serde(default)]
    pub network: NetworkPermissions,
    /// Run this plugin **on the host**, without the bubblewrap sandbox.
    ///
    /// Deny-by-default and never inferred from the transport. A plugin that
    /// can reach the host filesystem, the host network stack and the host
    /// credentials has to declare it in its manifest, and the consent UI shows
    /// it with an amber warning. The operator may also grant or revoke it
    /// explicitly in the consent editor; the stored decision wins.
    #[serde(default)]
    pub elevated: bool,
    /// Server control grants. Deny-by-default; see [`ControlPermissions`].
    #[serde(default)]
    pub control: ControlPermissions,
}

/// Grants to drive the NeuxDOT server's own control API.
///
/// A plugin holding these can read and mutate the host's agents, plugins,
/// providers, shares and (when the credential's account is `root`) accounts.
/// Every flag defaults to **false**: this is the most privileged surface a
/// plugin can reach, so it is never inferred from a transport, from `process`,
/// or from network access. The manifest must ask for each resource separately.
///
/// The credential's own role is still the outer gate — a `member` credential
/// cannot act as `root` no matter what these flags say. The flags only control
/// which *tools* the plugin is allowed to call.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ControlPermissions {
    /// List/read agents, including shared ones the credential can view.
    #[serde(default)]
    pub agents_read: bool,
    /// Create, edit, delete agents and rebind their models/plugins.
    #[serde(default)]
    pub agents_write: bool,
    /// Grant, change and revoke agent shares.
    #[serde(default)]
    pub shares: bool,
    /// List/read plugins.
    #[serde(default)]
    pub plugins_read: bool,
    /// Install, edit, enable/disable and delete plugins.
    #[serde(default)]
    pub plugins_write: bool,
    /// List/read providers and their models.
    #[serde(default)]
    pub providers_read: bool,
    /// Create, edit and delete providers.
    #[serde(default)]
    pub providers_write: bool,
    /// Account administration. Only reachable with a `root` credential.
    #[serde(default)]
    pub accounts: bool,
}

impl ControlPermissions {
    /// True when at least one control grant is present.
    pub fn any(&self) -> bool {
        self.agents_read
            || self.agents_write
            || self.shares
            || self.plugins_read
            || self.plugins_write
            || self.providers_read
            || self.providers_write
            || self.accounts
    }

    /// Human-readable list of the granted resources, for the consent UI.
    pub fn granted(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.agents_read {
            out.push("agents_read");
        }
        if self.agents_write {
            out.push("agents_write");
        }
        if self.shares {
            out.push("shares");
        }
        if self.plugins_read {
            out.push("plugins_read");
        }
        if self.plugins_write {
            out.push("plugins_write");
        }
        if self.providers_read {
            out.push("providers_read");
        }
        if self.providers_write {
            out.push("providers_write");
        }
        if self.accounts {
            out.push("accounts");
        }
        out
    }
}

/// Outbound network grants for a plugin.
///
/// The canonical wire format is grouped (`{"allow": [...]}`), but the loader is
/// liberal: a legacy flat `true`/`false` still deserialises, because stored
/// `permissions_json` rows predate the allow-list.
///
/// * `allow: []` — no network at all. This is the default.
/// * `allow: ["*"]` — unrestricted.
/// * otherwise — a list of hostnames, `*.suffix` wildcards, IP literals or
///   CIDR blocks.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NetworkPermissions {
    pub allow: Vec<String>,
}

impl Default for NetworkPermissions {
    fn default() -> Self {
        Self { allow: Vec::new() }
    }
}

/// Accept `{"allow": [...]}`, a bare array, or the legacy boolean.
///
/// Hand-written rather than derived because the legacy wire format put a
/// boolean where the struct now sits, and `#[serde(untagged)]` on the struct
/// itself would also swallow malformed grouped objects silently.
impl<'de> Deserialize<'de> for NetworkPermissions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Grouped { allow: Vec<String> },
            List(Vec<String>),
            Bool(bool),
            Text(String),
        }

        let raw = Raw::deserialize(deserializer)?;
        let allow = match raw {
            Raw::Bool(true) => vec![NETWORK_WILDCARD.to_string()],
            Raw::Bool(false) => Vec::new(),
            Raw::Grouped { allow } | Raw::List(allow) => allow
                .into_iter()
                .map(|entry| entry.trim().to_string())
                .filter(|entry| !entry.is_empty())
                .collect(),
            Raw::Text(text) => {
                let trimmed = text.trim().to_string();
                if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
                    Vec::new()
                } else {
                    vec![trimmed]
                }
            }
        };
        Ok(NetworkPermissions { allow })
    }
}

impl NetworkPermissions {
    /// Every granted destination marker.
    pub fn allow_list(&self) -> &[String] {
        &self.allow
    }

    /// No destination is reachable.
    pub fn is_denied(&self) -> bool {
        self.allow.is_empty()
    }

    /// `["*"]` — every destination is reachable.
    pub fn is_unrestricted(&self) -> bool {
        self.allow.iter().any(|entry| entry == NETWORK_WILDCARD)
    }

    /// True when `host` is reachable: exact match, a `*.suffix` wildcard, an IP
    /// literal, or membership of a CIDR block.
    ///
    /// Hostnames are compared case-insensitively. A leading `.` or `*.` in an
    /// entry means "this domain and every subdomain".
    pub fn allows_host(&self, host: &str) -> bool {
        if self.is_denied() {
            return false;
        }
        if self.is_unrestricted() {
            return true;
        }
        let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty() {
            return false;
        }
        self.allow.iter().any(|entry| {
            let entry = entry.trim().trim_end_matches('.').to_ascii_lowercase();
            if entry.is_empty() {
                return false;
            }
            if let Some(suffix) = entry.strip_prefix("*.") {
                return host == suffix || host.ends_with(&format!(".{suffix}"));
            }
            if let Some(suffix) = entry.strip_prefix('.') {
                return host == suffix || host.ends_with(&format!(".{suffix}"));
            }
            if entry.contains('/') {
                return cidr_contains(&entry, &host);
            }
            entry == host
        })
    }
}

impl Default for PluginPermissions {
    fn default() -> Self {
        Self {
            ui: UiPermissions::default(),
            storage: StoragePermissions::default(),
            agent: AgentPermissions::default(),
            // Privacy by default: identity grants must be asked for explicitly.
            identity: IdentityPermissions::default(),
            process: true,
            // No network and no elevated host access unless asked for.
            network: NetworkPermissions::default(),
            elevated: false,
            control: ControlPermissions::default(),
        }
    }
}

/// Nothing granted at all — used as the base for manifest-derived grants.
impl PluginPermissions {
    /// A permission object where every capability is denied.
    pub fn deny_all() -> Self {
        Self {
            ui: UiPermissions {
                enabled: false,
                resources: Vec::new(),
            },
            storage: StoragePermissions {
                get: false,
                set: false,
                delete: false,
                keys: Some(Vec::new()),
            },
            agent: AgentPermissions {
                trigger: false,
                tools: false,
            },
            identity: IdentityPermissions::default(),
            process: false,
            network: NetworkPermissions::default(),
            elevated: false,
            control: ControlPermissions::default(),
        }
    }
}

impl PluginPermissions {
    /// Returns `true` iff `capability` (a contract capability string) is granted.
    ///
    /// For `storage.*` this only checks the verb grant; the key-prefix
    /// allow-list is applied separately via [`PluginPermissions::storage_allows_key`]
    /// because it depends on the concrete key being touched.
    pub fn allows(&self, capability: &str) -> bool {
        match capability {
            CAP_UI_CONTEXT_REFRESH => self.ui.enabled,
            CAP_STORAGE_GET | CAP_STORAGE_KEYS => self.storage.get,
            CAP_STORAGE_SET => self.storage.set,
            CAP_STORAGE_DELETE => self.storage.delete,
            CAP_AGENT_TRIGGER => self.agent.trigger,
            CAP_IDENTITY_AGENT_NAME => self.identity.agent_name,
            CAP_IDENTITY_AGENT_PICTURE => self.identity.agent_profile_picture,
            CAP_IDENTITY_USER_NAME => self.identity.user_name,
            CAP_IDENTITY_USER_PICTURE => self.identity.user_profile_picture,
            CAP_IDENTITY_RESOLVE => self.identity.user_name || self.identity.user_profile_picture,
            _ => false,
        }
    }

    /// Every granted capability, in contract order.
    ///
    /// `storage.keys` is only emitted when `storage.get` is granted (listing
    /// keys without read access would be meaningless).
    pub fn capabilities(&self) -> Vec<String> {
        let mut out = Vec::new();
        for cap in [
            CAP_UI_CONTEXT_REFRESH,
            CAP_STORAGE_GET,
            CAP_STORAGE_SET,
            CAP_STORAGE_DELETE,
            CAP_STORAGE_KEYS,
            CAP_AGENT_TRIGGER,
            CAP_IDENTITY_AGENT_NAME,
            CAP_IDENTITY_AGENT_PICTURE,
            CAP_IDENTITY_USER_NAME,
            CAP_IDENTITY_USER_PICTURE,
            CAP_IDENTITY_RESOLVE,
        ] {
            if self.allows(cap) {
                out.push(cap.to_string());
            }
        }
        out
    }

    /// True when the UI may render `uri`. Honours the `"*"` wildcard marker
    /// and treats an empty allow-list as "any".
    pub fn allows_ui_resource(&self, uri: &str) -> bool {
        if !self.ui.enabled {
            return false;
        }
        if !uri.starts_with("ui://") {
            return false;
        }
        if self.ui.resources.is_empty() {
            return true;
        }
        self.ui
            .resources
            .iter()
            .any(|r| r == UI_RESOURCE_WILDCARD || r == uri)
    }

    /// True when `key` falls inside the `storage.keys` prefix allow-list.
    /// `None` (or an empty list) means "all keys".
    pub fn storage_allows_key(&self, key: &str) -> bool {
        match &self.storage.keys {
            None => true,
            Some(prefixes) if prefixes.is_empty() => true,
            Some(prefixes) => prefixes.iter().any(|p| {
                !p.is_empty() && (key.starts_with(p.as_str()) || p.as_str() == UI_RESOURCE_WILDCARD)
            }),
        }
    }

    /// True when this plugin must run inside the isolation sandbox.
    ///
    /// Everything process-spawning is sandboxed unless the manifest explicitly
    /// requested elevated host access. `process` alone does not escape it.
    pub fn requires_sandbox(&self) -> bool {
        self.process && !self.elevated
    }

    /// True when `host` may be reached from inside the sandbox.
    pub fn network_allows_host(&self, host: &str) -> bool {
        self.network.allows_host(host)
    }

    /// The effective capability name for a storage verb (`storage.get`,
    /// `storage.set`, `storage.delete`, `storage.list` -> `storage.keys`).
    pub fn storage_capability(verb: &str) -> Option<&'static str> {
        match verb {
            "get" => Some(CAP_STORAGE_GET),
            "set" => Some(CAP_STORAGE_SET),
            "delete" => Some(CAP_STORAGE_DELETE),
            "list" | "keys" => Some(CAP_STORAGE_KEYS),
            _ => None,
        }
    }

    /// Checks a storage operation end-to-end: verb grant **and** key prefix.
    /// Returns `Err(reason)` when the operation must be refused.
    pub fn check_storage(&self, verb: &str, key: &str) -> Result<(), String> {
        let cap = Self::storage_capability(verb)
            .ok_or_else(|| format!("unknown storage verb '{}'", verb))?;
        if !self.allows(cap) {
            return Err(format!("capability '{}' is not granted", cap));
        }
        if !self.storage_allows_key(key) {
            return Err(format!(
                "key '{}' is outside the allowed storage prefixes",
                key
            ));
        }
        Ok(())
    }
}

/// True when `host` (an IP literal) falls inside `cidr` (`10.0.0.0/8`).
///
/// Only IPv4 and IPv6 literals are matched; a hostname never matches a CIDR.
fn cidr_contains(cidr: &str, host: &str) -> bool {
    let Ok(host_ip) = host.parse::<std::net::IpAddr>() else {
        return false;
    };
    let Ok(network) = cidr.parse::<std::net::IpAddr>() else {
        // `10.0.0.0/8` splits into an address and a prefix length.
        let Some((addr, prefix)) = cidr.split_once('/') else {
            return false;
        };
        let (Ok(addr), Ok(prefix)) = (addr.parse::<std::net::IpAddr>(), prefix.parse::<u8>())
        else {
            return false;
        };
        return ip_in_network(host_ip, addr, prefix);
    };
    // A bare address matches only itself.
    host_ip == network
}

fn ip_in_network(host: std::net::IpAddr, network: std::net::IpAddr, prefix: u8) -> bool {
    match (host, network) {
        (std::net::IpAddr::V4(host), std::net::IpAddr::V4(net)) => {
            if prefix > 32 {
                return false;
            }
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            (u32::from(host) & mask) == (u32::from(net) & mask)
        }
        (std::net::IpAddr::V6(host), std::net::IpAddr::V6(net)) => {
            if prefix > 128 {
                return false;
            }
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix)
            };
            (u128::from(host) & mask) == (u128::from(net) & mask)
        }
        _ => false,
    }
}

/// Intermediate representation of the canonical grouped shape. Every field is
/// optional so a partial object (e.g. only `identity`) is accepted; missing
/// sections mean "not granted".
#[derive(Debug, Clone, Deserialize)]
struct GroupedPluginPermissions {
    #[serde(default)]
    ui: Option<UiPermissions>,
    #[serde(default)]
    storage: Option<StoragePermissions>,
    #[serde(default)]
    agent: Option<AgentPermissions>,
    #[serde(default)]
    identity: Option<IdentityPermissions>,
    #[serde(default)]
    process: Option<bool>,
    #[serde(default)]
    network: Option<NetworkPermissions>,
    #[serde(default)]
    elevated: Option<bool>,
    #[serde(default)]
    control: Option<ControlPermissions>,
}

impl From<GroupedPluginPermissions> for PluginPermissions {
    fn from(g: GroupedPluginPermissions) -> Self {
        let mut perms = PluginPermissions::deny_all();
        if let Some(ui) = g.ui {
            perms.ui = ui;
        }
        if let Some(storage) = g.storage {
            perms.storage = storage;
        }
        if let Some(agent) = g.agent {
            perms.agent = agent;
        }
        if let Some(identity) = g.identity {
            perms.identity = identity;
        }
        perms.process = g.process.unwrap_or(false);
        perms.network = g.network.unwrap_or_default();
        perms.elevated = g.elevated.unwrap_or(false);
        perms.control = g.control.unwrap_or_default();
        perms
    }
}

impl<'de> Deserialize<'de> for PluginPermissions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let grouped = GroupedPluginPermissions::deserialize(deserializer)?;
        Ok(PluginPermissions::from(grouped))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct McpServer {
    pub id: String,
    pub account_id: String,
    pub name: String,
    #[sqlx(default)]
    pub alias: Option<String>,
    pub transport: String, // "wasm" ("wasm_stdio"), "stdio", or "http"
    pub url_or_command: String,
    #[sqlx(default)]
    pub headers_or_env: Option<String>,
    #[serde(default = "default_plugin_version")]
    #[sqlx(default)]
    pub version: String,
    #[sqlx(default)]
    pub description: Option<String>,
    #[sqlx(default)]
    pub wasm_hash: Option<String>,
    #[serde(skip_serializing)]
    #[sqlx(default)]
    pub wasm_binary: Option<Vec<u8>>,
    #[sqlx(default)]
    pub manifest_json: Option<String>,
    #[sqlx(default)]
    pub permissions_json: Option<String>,
    #[serde(default = "default_true")]
    #[sqlx(default)]
    pub is_active: bool,
    #[serde(default)]
    #[sqlx(default)]
    pub storage_bytes: i64,
    #[sqlx(default)]
    pub created_at: Option<DateTime<Utc>>,
}

pub type Plugin = McpServer;

pub fn slugify_alias(name: &str) -> String {
    let cleaned = name.replace("[WASM]", "").replace("[wasm]", "");
    let mut s = String::with_capacity(cleaned.len());
    let mut prev_was_underscore = false;

    for c in cleaned.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
            prev_was_underscore = false;
        } else if !prev_was_underscore && !s.is_empty() {
            s.push('_');
            prev_was_underscore = true;
        }
    }

    let trimmed = s.trim_matches('_');
    if trimmed.is_empty() {
        "mcp".to_string()
    } else {
        trimmed.to_string()
    }
}

impl McpServer {
    pub fn get_alias(&self) -> String {
        if let Some(a) = &self.alias {
            let trimmed = a.trim();
            if !trimmed.is_empty() {
                return slugify_alias(trimmed);
            }
        }
        slugify_alias(&self.name)
    }

    /// Resolves the effective grants for this plugin.
    ///
    /// Resolution order:
    /// 1. An explicit `permissions` block in the manifest — parsed into the
    ///    canonical grouped shape.
    /// 2. Otherwise, behaviour derived from the manifest (`ui://` resources /
    ///    `ui_hooks`, `network.hosts`) or from the transport.
    ///
    /// **Privacy by default:** identity grants are only ever given when the
    /// manifest explicitly asks for them (`identity.user.name: true`, …).
    /// Neither an omitted `data_access` block nor a transport default grants
    /// access to the account profile any more.
    pub fn get_permissions(&self) -> PluginPermissions {
        // The account's stored consent decision is authoritative. It is written
        // by the /mcp consent editor (`permissions_json`) and must win over
        // whatever the plugin's own manifest asked for — otherwise a plugin
        // could grant itself access nobody approved.
        if let Some(stored) = self
            .permissions_json
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty() && *v != "{}")
        {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(stored) {
                // Only a grouped override is a real consent decision. A row
                // saved in the removed flat format (`{ui:true, agent_control:true}`)
                // has no grouped section: treating it as a decision would deny
                // everything and hide the plugin's apps, so ignore it and fall
                // back to what the plugin declared.
                // A grouped section (or an explicit `elevated` flag) marks a
                // real consent decision. `control` and `network` are included
                // because a stored decision may consist *only* of those (e.g. a
                // plugin granted nothing but a control scope); omitting them
                // would silently fall back to the manifest and ignore the
                // operator's decision.
                let is_grouped = ["ui", "storage", "agent", "identity", "control", "network"]
                    .iter()
                    .any(|k| val.get(*k).map(|v| v.is_object()).unwrap_or(false))
                    || val.get("elevated").and_then(|v| v.as_bool()).is_some();
                if is_grouped {
                    // `elevated` is the one grant an operator may deliberately
                    // add as well as revoke. An old stored consent written
                    // before that was possible has no `elevated` key, so keep
                    // whatever the manifest declared instead of silently
                    // revoking it.
                    let stored_elevated = val.get("elevated").and_then(|v| v.as_bool());
                    if let Ok(mut perms) = serde_json::from_value::<PluginPermissions>(val) {
                        if stored_elevated.is_none() {
                            perms.elevated = self.declared_permissions().elevated;
                        }
                        if self.transport == "stdio" || self.transport == "binary" {
                            perms.process = true;
                        }
                        return perms;
                    }
                }
            }
        }
        self.declared_permissions()
    }

    /// Grants derived from the plugin's own manifest (`ui://` resources, the
    /// permission block, transport defaults). Account consent overrides live in
    /// [`McpServer::get_permissions`].
    pub fn declared_permissions(&self) -> PluginPermissions {
        if let Some(manifest_str) = &self.manifest_json {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(manifest_str) {
                if let Some(p) = val.get("permissions") {
                    return Self::permissions_from_manifest_block(p);
                }
                let has_ui = Self::manifest_has_ui_resources(&val);
                // A legacy manifest declares reachable hosts as
                // `network.hosts`; those map straight onto the allow-list.
                let network_hosts: Vec<String> = val
                    .get("network")
                    .and_then(|n| n.get("hosts"))
                    .and_then(|h| h.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|h| h.as_str())
                            .map(|h| h.trim().to_string())
                            .filter(|h| !h.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                let has_tools = val
                    .get("mcp")
                    .and_then(|m| m.get("tools"))
                    .and_then(|t| t.as_array())
                    .map(|a| !a.is_empty())
                    .unwrap_or(false);
                return PluginPermissions {
                    ui: UiPermissions {
                        enabled: has_ui,
                        resources: if has_ui {
                            Self::manifest_ui_resources(&val)
                        } else {
                            Vec::new()
                        },
                    },
                    storage: StoragePermissions::default(),
                    agent: AgentPermissions {
                        trigger: false,
                        tools: has_tools,
                    },
                    identity: IdentityPermissions::default(),
                    process: self.transport == "stdio" || self.transport == "binary",
                    network: NetworkPermissions {
                        allow: network_hosts,
                    },
                    // Never inferred: a plugin must ask for host access, and
                    // control grants are only ever explicit.
                    elevated: false,
                    control: ControlPermissions::default(),
                };
            }
        }

        if self.transport == "stdio" || self.transport == "binary" {
            return PluginPermissions {
                process: true,
                storage: StoragePermissions {
                    get: false,
                    set: false,
                    delete: false,
                    keys: Some(vec![]),
                },
                identity: IdentityPermissions::default(),
                ..PluginPermissions::default()
            };
        }

        if self.transport == "custom_tools" || self.transport == "curl" {
            return PluginPermissions {
                ui: UiPermissions {
                    enabled: false,
                    resources: Vec::new(),
                },
                storage: StoragePermissions::default(),
                agent: AgentPermissions::default(),
                identity: IdentityPermissions::default(),
                process: false,
                network: NetworkPermissions {
                    allow: vec![NETWORK_WILDCARD.to_string()],
                },
                elevated: false,
                control: ControlPermissions::default(),
            };
        }

        // Plain HTTP MCP server: network only, no UI, no storage, no identity.
        PluginPermissions {
            storage: StoragePermissions {
                get: false,
                set: false,
                delete: false,
                keys: Some(vec![]),
            },
            identity: IdentityPermissions::default(),
            process: false,
            network: NetworkPermissions {
                allow: vec![NETWORK_WILDCARD.to_string()],
            },
            elevated: false,
            ..PluginPermissions::default()
        }
    }

    /// Parses a manifest `permissions` block into the canonical shape.
    /// Accepts the canonical grouped shape only.
    fn permissions_from_manifest_block(p: &serde_json::Value) -> PluginPermissions {
        // The manifest block is exactly the PluginPermissions wire format, so
        // reuse its (backward compatible) Deserialize impl.
        if let Ok(perms) = serde_json::from_value::<PluginPermissions>(p.clone()) {
            return perms;
        }
        // Unparseable block: deny everything rather than guess.
        PluginPermissions::deny_all()
    }

    /// `true` when the manifest declares at least one `ui://` resource or a
    /// non-empty `ui_hooks` array.
    fn manifest_has_ui_resources(val: &serde_json::Value) -> bool {
        let by_resource = val
            .get("mcp")
            .and_then(|m| m.get("resources"))
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter().any(|item| {
                    item.get("uri")
                        .and_then(|u| u.as_str())
                        .map(|u| u.starts_with("ui://"))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        let by_hooks = val
            .get("ui_hooks")
            .and_then(|h| h.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        by_resource || by_hooks
    }

    /// The `ui://` URIs the manifest exposes. Falls back to the wildcard when
    /// the UI is only implied by `ui_hooks`.
    fn manifest_ui_resources(val: &serde_json::Value) -> Vec<String> {
        let uris: Vec<String> = val
            .get("mcp")
            .and_then(|m| m.get("resources"))
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.get("uri").and_then(|u| u.as_str()))
                    .filter(|u| u.starts_with("ui://"))
                    .map(|u| u.to_string())
                    .collect()
            })
            .unwrap_or_default();
        if uris.is_empty() {
            vec![UI_RESOURCE_WILDCARD.to_string()]
        } else {
            uris
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentMcp {
    pub agent_id: String,
    pub mcp_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentPushedMessage {
    pub id: String,
    pub agent_id: String,
    pub mcp_id: Option<String>,
    pub method: String,
    pub params_json: Option<String>,
    #[sqlx(default)]
    pub is_consumed: bool,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentToolPermission {
    pub agent_id: String,
    pub mcp_id: String,
    pub tool_name: String,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentSubscription {
    pub agent_id: String,
    pub uri: String,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentMemory {
    pub agent_id: String,
    pub content: String,
    pub hash: String,
    pub updated_at: Option<DateTime<Utc>>,
}

/// One node of the Obsidian-style memory graph.
///
/// `key` is the node's identity and is what `[[wikilinks]]` point at. Unlike the
/// legacy single-blob memory ([`AgentMemory`]) each node is written and versioned
/// independently, so an agent can update one fact without rewriting everything,
/// and can retract a stale fact by deleting its node.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct MemoryNode {
    pub agent_id: String,
    pub key: String,
    pub value: String,
    /// `entity | decision | preference | event | file | concept | skill`.
    /// Advisory: it drives grouping and the graph rendering, not access.
    #[serde(default)]
    pub r#type: String,
    /// A recall-oriented description, phrased as the question this node answers.
    #[serde(default)]
    pub description: String,
    pub hash: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// A directed `src -> dst` edge extracted from a `[[dst]]` reference in `src`.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct MemoryLink {
    pub agent_id: String,
    pub src: String,
    pub dst: String,
}

/// A node plus its graph neighbourhood, for rendering and for `memory_get`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryNodeView {
    pub key: String,
    pub r#type: String,
    pub description: String,
    pub value: String,
    pub hash: String,
    pub updated_at: Option<DateTime<Utc>>,
    /// Nodes this one points at (`[[link]]` inside its value).
    pub outbound: Vec<String>,
    /// Nodes pointing at this one.
    pub backlinks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentHistoryItem {
    pub id: String,
    pub agent_id: String,
    pub role: String, // "system", "user", "assistant", "tool"
    pub content: String,
    pub tool_calls_json: Option<String>,
    pub tool_call_id: Option<String>,
    /// For the base `system` row: the raw template that produced `content`.
    #[sqlx(default)]
    pub source_template: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

/// One immutable row of an agent's full lifetime transcript.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentTranscriptItem {
    pub id: String,
    pub agent_id: String,
    pub seq: i64,
    pub role: String,
    pub content: String,
    pub tool_calls_json: Option<String>,
    pub tool_call_id: Option<String>,
    pub origin: String,
    pub turn: i64,
    pub metadata_json: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ExtensionKvItem {
    pub extension_id: String,
    pub key: String,
    pub value: String,
    pub updated_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify_alias() {
        assert_eq!(slugify_alias("Telegram"), "telegram");
        assert_eq!(slugify_alias("Discord Bot"), "discord_bot");
        assert_eq!(
            slugify_alias("[WASM] Agent Chat Console"),
            "agent_chat_console"
        );
        assert_eq!(slugify_alias("Filesystem-MCP 1.0"), "filesystem_mcp_1_0");
        assert_eq!(slugify_alias(""), "mcp");
    }

    #[test]
    fn test_mcp_server_get_alias() {
        let server = McpServer {
            id: "1".into(),
            account_id: "acc".into(),
            name: "Telegram Bot".into(),
            alias: Some("tg".into()),
            transport: "http".into(),
            url_or_command: "http://localhost:3000".into(),
            headers_or_env: None,
            version: "1.0.0".into(),
            description: None,
            wasm_hash: None,
            wasm_binary: None,
            manifest_json: None,
            permissions_json: None,
            is_active: true,
            storage_bytes: 0,
            created_at: None,
        };
        assert_eq!(server.get_alias(), "tg");

        let server_no_alias = McpServer {
            id: "2".into(),
            account_id: "acc".into(),
            name: "Discord Bot".into(),
            alias: None,
            transport: "http".into(),
            url_or_command: "http://localhost:3000".into(),
            headers_or_env: None,
            version: "1.0.0".into(),
            description: None,
            wasm_hash: None,
            wasm_binary: None,
            manifest_json: None,
            permissions_json: None,
            is_active: true,
            storage_bytes: 0,
            created_at: None,
        };
        assert_eq!(server_no_alias.get_alias(), "discord_bot");
    }

    // --- PluginPermissions ---

    fn manifest_server(manifest: serde_json::Value) -> McpServer {
        McpServer {
            id: "test".into(),
            account_id: "acc".into(),
            name: "Test".into(),
            alias: None,
            transport: "binary".into(),
            url_or_command: "./test".into(),
            headers_or_env: None,
            version: "1.0.0".into(),
            description: None,
            wasm_hash: None,
            wasm_binary: None,
            manifest_json: Some(manifest.to_string()),
            permissions_json: None,
            is_active: true,
            storage_bytes: 0,
            created_at: None,
        }
    }

    #[test]
    fn flat_permission_blob_is_rejected() {
        // The historic flat boolean blob is no longer a supported input shape;
        // only the grouped contract shape deserialises.
        let flat = serde_json::json!({
            "ui": true,
            "storage": true,
            "process": false,
            "network": false,
            "agent_control": true,
            "identity": true
        });
        assert!(
            serde_json::from_value::<PluginPermissions>(flat).is_err(),
            "flat permission blobs must be rejected"
        );
    }

    #[test]
    fn unknown_sections_are_denied() {
        // A grouped object with fields we do not know about parses, but any
        // section that is absent stays denied.
        let perms: PluginPermissions =
            serde_json::from_value(serde_json::json!({"ui": {"enabled": true}})).expect("parses");
        assert!(perms.ui.enabled);
        assert!(!perms.storage.get);
        assert!(!perms.agent.trigger);
        assert_eq!(perms.identity, IdentityPermissions::default());
    }

    #[test]
    fn grouped_permissions_round_trip() {
        let json = serde_json::json!({
            "ui": { "enabled": true, "resources": ["ui://chat-console"] },
            "storage": { "get": true, "set": true, "delete": false, "keys": ["chat."] },
            "agent": { "trigger": true, "tools": false },
            "identity": {
                "agent.name": false,
                "agent.profile_picture": true,
                "user.name": true,
                "user.profile_picture": false
            },
            "process": false,
            "network": false
        });
        let perms: PluginPermissions =
            serde_json::from_value(json.clone()).expect("grouped parses");
        assert!(perms.ui.enabled);
        assert_eq!(perms.ui.resources, vec!["ui://chat-console".to_string()]);
        assert!(perms.storage.get);
        assert!(!perms.storage.delete);
        assert_eq!(
            perms.storage.keys.as_deref(),
            Some(&["chat.".to_string()][..])
        );
        assert!(perms.agent.trigger);
        assert!(!perms.agent.tools);
        assert!(!perms.identity.agent_name);
        assert!(perms.identity.agent_profile_picture);
        assert!(perms.identity.user_name);
        assert!(!perms.identity.user_profile_picture);

        // Serialises back to the same canonical shape (dotted identity keys).
        let out = serde_json::to_value(&perms).expect("serialises");
        assert_eq!(
            out["identity"]["agent.profile_picture"],
            serde_json::json!(true)
        );
        assert_eq!(out["identity"]["agent.name"], serde_json::json!(false));
        assert_eq!(out["storage"]["keys"], serde_json::json!(["chat."]));
        assert_eq!(
            out["ui"]["resources"],
            serde_json::json!(["ui://chat-console"])
        );
    }

    #[test]
    fn network_allow_list_matches_hosts_wildcards_and_cidrs() {
        let denied = NetworkPermissions::default();
        assert!(denied.is_denied());
        assert!(!denied.allows_host("api.example.com"));
        assert!(!denied.allows_host("1.1.1.1"));

        let unrestricted = NetworkPermissions {
            allow: vec![NETWORK_WILDCARD.to_string()],
        };
        assert!(unrestricted.is_unrestricted());
        assert!(unrestricted.allows_host("anything.example"));

        let listed = NetworkPermissions {
            allow: vec![
                "api.example.com".into(),
                "*.trusted.test".into(),
                "10.0.0.0/8".into(),
                "1.1.1.1".into(),
            ],
        };
        assert!(listed.allows_host("api.example.com"));
        assert!(listed.allows_host("API.Example.COM"));
        assert!(!listed.allows_host("api.example.com.evil.test"));
        assert!(listed.allows_host("trusted.test"));
        assert!(listed.allows_host("deep.sub.trusted.test"));
        assert!(!listed.allows_host("nottrusted.test"));
        assert!(listed.allows_host("10.1.2.3"));
        assert!(!listed.allows_host("11.1.2.3"));
        assert!(listed.allows_host("1.1.1.1"));
        assert!(!listed.allows_host("1.1.1.2"));
        // A hostname never matches a CIDR entry.
        assert!(!listed.allows_host("example.com"));
    }

    #[test]
    fn legacy_boolean_network_flag_still_deserialises() {
        let enabled: PluginPermissions =
            serde_json::from_value(serde_json::json!({ "process": true, "network": true }))
                .expect("parses");
        assert!(enabled.network.is_unrestricted());

        let disabled: PluginPermissions =
            serde_json::from_value(serde_json::json!({ "process": true, "network": false }))
                .expect("parses");
        assert!(disabled.network.is_denied());

        // The canonical grouped shape is unchanged.
        let grouped: PluginPermissions = serde_json::from_value(serde_json::json!({
            "process": true,
            "network": { "allow": ["api.example.com"] }
        }))
        .expect("parses");
        assert_eq!(grouped.network.allow, vec!["api.example.com".to_string()]);
        assert!(!grouped.elevated);
    }

    #[test]
    fn elevated_is_never_inferred_from_the_transport() {
        let binary = manifest_server(serde_json::json!({ "id": "x" }));
        let mut server = binary;
        server.transport = "binary".to_string();
        let perms = server.get_permissions();
        assert!(perms.process);
        assert!(!perms.elevated);
        assert!(perms.requires_sandbox());

        // Only an explicit opt-in escapes the sandbox.
        let elevated = PluginPermissions {
            process: true,
            elevated: true,
            ..PluginPermissions::deny_all()
        };
        assert!(!elevated.requires_sandbox());
    }

    #[test]
    fn partial_grouped_permissions_deny_by_default() {
        let perms: PluginPermissions =
            serde_json::from_value(serde_json::json!({"identity": {"user.name": true}}))
                .expect("parses");
        assert!(!perms.ui.enabled);
        assert!(!perms.storage.get);
        assert!(!perms.agent.trigger);
        assert!(perms.identity.user_name);
        assert!(!perms.identity.agent_name);
        // Requesting any identity read also permits resolving display data for
        // message authors, so the app can render sender names.
        assert_eq!(
            perms.capabilities(),
            vec![
                "identity.user.name".to_string(),
                "identity.resolve".to_string()
            ]
        );
    }

    #[test]
    fn allows_and_capabilities_per_namespace() {
        let perms = PluginPermissions {
            ui: UiPermissions {
                enabled: true,
                resources: vec!["ui://a".into()],
            },
            storage: StoragePermissions {
                get: true,
                set: false,
                delete: false,
                keys: Some(vec!["k.".into()]),
            },
            agent: AgentPermissions {
                trigger: true,
                tools: false,
            },
            identity: IdentityPermissions {
                agent_name: true,
                ..Default::default()
            },
            process: true,
            network: NetworkPermissions::default(),
            elevated: false,
            control: ControlPermissions::default(),
        };

        assert!(perms.allows("ui.context.refresh"));
        assert!(perms.allows("storage.get"));
        assert!(perms.allows("storage.keys"));
        assert!(!perms.allows("storage.set"));
        assert!(!perms.allows("storage.delete"));
        assert!(perms.allows("agent.trigger"));
        assert!(!perms.allows("agent.tools"));
        assert!(perms.allows("identity.agent.name"));
        assert!(!perms.allows("identity.agent.profile_picture"));
        assert!(!perms.allows("identity.user.name"));
        assert!(!perms.allows("identity.user.profile_picture"));
        assert!(!perms.allows("totally.unknown"));
        assert!(!perms.allows(""));

        assert_eq!(
            perms.capabilities(),
            vec![
                "ui.context.refresh",
                "storage.get",
                "storage.keys",
                "agent.trigger",
                "identity.agent.name",
            ]
        );

        // process stays a flat boolean; network is an allow-list.
        assert!(perms.process);
        assert!(perms.network.is_denied());
        assert!(!perms.elevated);
    }

    #[test]
    fn allows_ui_resource_honours_wildcard_and_allow_list() {
        let wildcard = PluginPermissions {
            ui: UiPermissions {
                enabled: true,
                resources: vec![UI_RESOURCE_WILDCARD.to_string()],
            },
            ..PluginPermissions::deny_all()
        };
        assert!(wildcard.allows_ui_resource("ui://anything"));
        assert!(!wildcard.allows_ui_resource("http://example.com"));

        let listed = PluginPermissions {
            ui: UiPermissions {
                enabled: true,
                resources: vec!["ui://a".into()],
            },
            ..PluginPermissions::deny_all()
        };
        assert!(listed.allows_ui_resource("ui://a"));
        assert!(!listed.allows_ui_resource("ui://b"));

        let disabled = PluginPermissions {
            ui: UiPermissions {
                enabled: false,
                resources: vec![UI_RESOURCE_WILDCARD.to_string()],
            },
            ..PluginPermissions::deny_all()
        };
        assert!(!disabled.allows_ui_resource("ui://a"));
    }

    #[test]
    fn storage_key_prefix_rejection() {
        let perms = PluginPermissions {
            storage: StoragePermissions {
                get: true,
                set: true,
                delete: false,
                keys: Some(vec!["chat.".into(), "draft.".into()]),
            },
            ..PluginPermissions::deny_all()
        };

        assert!(perms.check_storage("get", "chat.draft").is_ok());
        assert!(perms.check_storage("set", "draft.1").is_ok());
        assert!(perms.check_storage("get", "secret.key").is_err());
        assert!(perms.check_storage("set", "chatx").is_err());
        // Verb not granted.
        assert!(perms.check_storage("delete", "chat.draft").is_err());
        // Unknown verb.
        assert!(perms.check_storage("nuke", "chat.draft").is_err());

        // `keys: None` means every key.
        let all = PluginPermissions {
            storage: StoragePermissions {
                get: true,
                set: true,
                delete: true,
                keys: None,
            },
            ..PluginPermissions::deny_all()
        };
        assert!(all.storage_allows_key("anything"));
        assert!(all.check_storage("delete", "anything").is_ok());

        // `keys: None` means every key, and an explicit `[]` must not lock the
        // plugin out of its own isolated storage either.
        let empty = PluginPermissions {
            storage: StoragePermissions {
                get: true,
                set: false,
                delete: false,
                keys: Some(vec![]),
            },
            ..PluginPermissions::deny_all()
        };
        assert!(empty.storage_allows_key("anything"));
    }

    #[test]
    fn get_permissions_reads_grouped_manifest_block() {
        let server = manifest_server(serde_json::json!({
            "id": "x",
            "permissions": {
                "ui": { "enabled": true, "resources": ["ui://chat-console"] },
                "storage": { "get": true, "set": true, "delete": true, "keys": ["chat."] },
                "agent": { "trigger": true, "tools": true },
                "identity": { "agent.name": true, "user.name": true },
                "process": false,
                "network": false
            }
        }));
        let perms = server.get_permissions();
        assert!(perms.allows_ui_resource("ui://chat-console"));
        assert!(!perms.allows_ui_resource("ui://other"));
        assert!(perms.allows("storage.get"));
        assert!(perms.allows("agent.trigger"));
        assert_eq!(
            perms.capabilities(),
            vec![
                "ui.context.refresh",
                "storage.get",
                "storage.set",
                "storage.delete",
                "storage.keys",
                "agent.trigger",
                "identity.agent.name",
                "identity.user.name",
                "identity.resolve"
            ]
        );
    }

    #[test]
    fn flat_style_override_is_ignored_not_denied() {
        // A `permissions_json` row written before the grouped shape must not be
        // read as "deny everything": that silently hid every plugin app. It is
        // ignored and the plugin's declared grants apply instead.
        let mut server = manifest_server(serde_json::json!({
            "permissions": {
                "ui": { "enabled": true, "resources": ["ui://chat-console"] }
            }
        }));
        server.permissions_json = Some(
            serde_json::json!({"ui": true, "storage": true, "agent_control": true}).to_string(),
        );

        let perms = server.get_permissions();
        assert!(
            perms.ui.enabled,
            "declared UI grant must survive an unusable override"
        );
        assert!(perms.allows_ui_resource("ui://chat-console"));

        // A real grouped override is still authoritative.
        server.permissions_json =
            Some(serde_json::json!({"ui": {"enabled": false, "resources": []}}).to_string());
        assert!(!server.get_permissions().ui.enabled);
    }

    #[test]
    fn stored_consent_overrides_manifest_grants() {
        // The manifest asks for everything...
        let mut server = manifest_server(serde_json::json!({
            "permissions": {
                "ui": { "enabled": true, "resources": ["ui://chat-console"] },
                "storage": { "get": true, "set": true, "delete": true, "keys": null },
                "agent": { "trigger": true, "tools": true },
                "identity": {
                    "agent.name": true, "agent.profile_picture": true,
                    "user.name": true, "user.profile_picture": true
                }
            }
        }));
        assert!(server.get_permissions().allows("identity.user.name"));

        // ...but the account approved only a narrow subset.
        server.permissions_json = Some(
            serde_json::json!({
                "ui": { "enabled": true, "resources": ["ui://chat-console"] },
                "storage": { "get": true, "set": false, "delete": false, "keys": ["chat."] },
                "agent": { "trigger": false, "tools": true },
                "identity": { "user.name": true }
            })
            .to_string(),
        );
        let perms = server.get_permissions();
        assert!(perms.allows("identity.user.name"));
        assert!(
            !perms.allows("identity.agent.name"),
            "unapproved agent name leaked"
        );
        assert!(
            !perms.allows("agent.trigger"),
            "unapproved agent trigger leaked"
        );
        assert!(!perms.storage.set);
        assert!(perms.storage.get);
        assert_eq!(
            perms.storage.keys.as_deref(),
            Some(&["chat.".to_string()][..])
        );

        // An empty/garbage store falls back to what the manifest declared.
        server.permissions_json = Some("{}".to_string());
        assert!(server.get_permissions().allows("identity.agent.name"));
        server.permissions_json = Some("not json".to_string());
        assert!(server.get_permissions().allows("identity.agent.name"));
    }

    #[test]
    fn stored_consent_can_grant_and_revoke_host_execution() {
        let mut server = manifest_server(serde_json::json!({
            "permissions": { "process": true, "elevated": true }
        }));
        assert!(server.get_permissions().elevated);

        // An explicit operator revocation wins.
        server.permissions_json =
            Some(serde_json::json!({ "process": true, "elevated": false }).to_string());
        assert!(!server.get_permissions().elevated);

        // A stored decision written before `elevated` was toggleable has no
        // key for it: keep the manifest declaration.
        server.permissions_json = Some(serde_json::json!({ "process": true }).to_string());
        assert!(server.get_permissions().elevated);
    }

    #[test]
    fn get_permissions_manifest_privacy_by_default() {
        // No `permissions` block at all, but a ui:// resource and data_access.
        let server = manifest_server(serde_json::json!({
            "id": "x",
            "mcp": { "resources": [{ "uri": "ui://chat-console" }] },
            "data_access": ["history"],
            "network": { "hosts": ["api.example.com"] }
        }));
        let perms = server.get_permissions();
        assert!(perms.ui.enabled);
        assert!(perms.allows_ui_resource("ui://chat-console"));
        assert!(!perms.allows_ui_resource("ui://something-else"));
        // `network.hosts` from a legacy manifest becomes the allow-list.
        assert_eq!(perms.network.allow, vec!["api.example.com".to_string()]);
        assert!(perms.network.allows_host("api.example.com"));
        assert!(!perms.network.allows_host("evil.example.com"));
        assert!(perms
            .capabilities()
            .iter()
            .all(|c| !c.starts_with("identity.")));

        // A grouped manifest block is honoured verbatim.
        let grouped = manifest_server(serde_json::json!({
            "permissions": {
                "ui": { "enabled": true, "resources": ["ui://chat-console"] },
                "storage": { "get": true, "set": false, "delete": false, "keys": ["chat."] },
                "agent": { "trigger": false, "tools": true },
                "identity": { "user.name": true }
            }
        }));
        let gp = grouped.get_permissions();
        assert!(gp.allows("identity.user.name"));
        assert!(!gp.allows("identity.agent.name"));
        assert!(gp.storage.get && !gp.storage.set);

        // An unparseable (e.g. flat) block denies everything rather than guessing.
        let bad = manifest_server(serde_json::json!({
            "permissions": { "ui": true, "storage": true, "identity": true }
        }));
        let p = bad.get_permissions();
        assert!(!p.ui.enabled);
        assert_eq!(p.capabilities(), Vec::<String>::new());
    }

    #[test]
    fn test_binary_transport_permissions() {
        let mut server = manifest_server(serde_json::json!({}));
        server.transport = "binary".to_string();
        server.url_or_command = "./release/plugins/computer".to_string();
        assert_eq!(server.transport, "binary");
        let perms = server.get_permissions();
        assert!(
            perms.process,
            "Binary transport must have process permission enabled"
        );
    }
}
