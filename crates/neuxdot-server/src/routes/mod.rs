pub mod accounts;
pub mod agents;
pub mod auth;
pub mod extensions;
pub mod mcp;
pub mod instruction_presets;
pub mod profile;
pub mod providers;
pub mod ws;

use axum::{
    routing::{get, patch, post, put},
    Router,
};

use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        // Auth (login only - root user initialized via server CLI)
        .route("/api/auth/login", post(auth::login_handler))
        .route("/api/auth/me", get(auth::me_handler))
        // Accounts (root-only administration)
        .route(
            "/api/accounts",
            get(accounts::list_accounts_handler).post(accounts::create_account_handler),
        )
        .route(
            "/api/accounts/:id",
            axum::routing::patch(accounts::update_account_handler)
                .delete(accounts::delete_account_handler),
        )
        .route(
            "/api/accounts/:id/password",
            post(accounts::change_password_handler),
        )
        // Agents
        .route("/api/agents", get(agents::list_agents_handler).post(agents::create_agent_handler))
        .route(
            "/api/agents/:id",
            get(agents::get_agent_handler)
                .patch(agents::patch_agent_handler)
                .delete(agents::delete_agent_handler),
        )
        .route("/api/agents/:id/run", post(agents::run_agent_handler))
        .route(
            "/api/agents/:id/pushed",
            get(agents::list_agent_pushed_messages_handler),
        )
        .route(
            "/api/agents/:id/memory",
            get(agents::get_agent_memory_handler),
        )
        .route(
            "/api/agents/:id/history",
            get(agents::get_agent_history_handler).delete(agents::clear_agent_history_handler),
        )
        .route(
            "/api/agents/:id/tools",
            get(agents::get_agent_tools_handler).put(agents::update_agent_tool_permission_handler),
        )
        .route(
            "/api/agents/:id/subscriptions",
            get(agents::get_agent_subscriptions_handler).put(agents::update_agent_subscriptions_handler),
        )
        // Providers
        .route(
            "/api/providers",
            get(providers::list_providers_handler).post(providers::create_provider_handler),
        )
        .route(
            "/api/providers/fetch-models",
            post(providers::fetch_models_handler),
        )
        .route(
            "/api/providers/:id",
            put(providers::update_provider_handler).delete(providers::delete_provider_handler),
        )
        // MCP Servers
        .route("/api/mcp", get(mcp::list_mcp_handler).post(mcp::create_mcp_handler))
        .route(
            "/api/mcp/:id",
            put(mcp::update_mcp_handler).delete(mcp::delete_mcp_handler),
        )
        .route("/api/mcp/:id/push", post(mcp::push_mcp_information_handler))
        // Profile
        .route(
            "/api/profile",
            get(profile::get_profile_handler).put(profile::update_profile_handler),
        )
        // Agent Instructions presets (owner-scoped)
        .route(
            "/api/instruction-presets",
            get(instruction_presets::list_instruction_presets_handler)
                .post(instruction_presets::create_instruction_preset_handler),
        )
        .route(
            "/api/instruction-presets/:id",
            patch(instruction_presets::update_instruction_preset_handler)
                .delete(instruction_presets::delete_instruction_preset_handler),
        )
        // Extensions & Isolated Host-Managed Storage
        .route(
            "/api/extensions",
            get(extensions::list_extensions_handler),
        )
        .route(
            "/api/extensions/upload",
            post(extensions::upload_extension_handler),
        )
        .route(
            "/api/extensions/inspect",
            post(extensions::inspect_extension_handler),
        )
        .route(
            "/api/extensions/url",
            post(extensions::install_url_handler),
        )
        .route(
            "/api/extensions/:id",
            axum::routing::patch(extensions::patch_extension_handler)
                .delete(extensions::delete_extension_handler),
        )
        .route(
            "/api/extensions/:id/kv",
            get(extensions::list_extension_kv_handler),
        )
        .route(
            "/api/extensions/:id/kv/:key",
            axum::routing::delete(extensions::delete_extension_kv_key_handler),
        )
        .route(
            "/api/extensions/:id/storage",
            get(extensions::get_extension_storage_handler),
        )
        .route(
            "/api/extensions/:id/action",
            post(extensions::execute_extension_action_handler),
        )
        // WebSocket
        .route("/ws/events", get(ws::ws_events_handler))
        .with_state(state)
}
