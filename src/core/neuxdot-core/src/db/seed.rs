//! Idempotent seeding of the server's own control plugin.
//!
//! NeuxDOT exposes an MCP endpoint that lets an agent drive the server itself
//! (`POST /api/control/mcp`: create agents, bind plugins, manage providers,
//! sharing, accounts). Until now that endpoint existed but nothing installed a
//! plugin row for it, so an agent could only reach it if an operator manually
//! created an `http` plugin. That made the feature undiscoverable and created a
//! chicken-and-egg: the bot cannot add a plugin it has no tool for.
//!
//! This module installs one `Server Control` plugin per account, pinned to the
//! server's own loopback URL, and binds it to that account's agents.
//!
//! ## Why there is no global credential
//!
//! The control MCP authenticates every call with **HTTP Basic** against a real
//! account. A credential in the shared plugin row would be usable by every agent
//! that binds it. Instead the seeded row carries **no** `headers_or_env`: each
//! agent supplies its own `Authorization` header through `agent_mcp.config_override`
//! (the per-agent Auth/Config button), so authority stays per-agent and a
//! least-privilege credential can be issued for each bot.
//!
//! ## Consent
//!
//! The declared `permissions.control` scopes are granted for the resources the
//! control surface can reach, but `accounts` is **not**: account administration
//! remains root-only and must be granted explicitly. The authoritative gate is
//! still the Basic-auth credential — an agent without a configured override
//! cannot call a single control tool, no matter what the manifest declares.

use serde_json::json;
use sqlx::SqlitePool;

/// Marker written into `manifest_json` so re-seeding can refresh the loopback
/// URL after a port change without clobbering an operator's permission edits.
pub const MANAGED_BY: &str = "neuxdot-core";

/// Stable per-account plugin id. Deterministic so seeding is idempotent and a
/// reboot never creates a second control plugin.
pub fn control_plugin_id(account_id: &str) -> String {
    format!("control-mcp-{account_id}")
}

/// The plugin row's declared manifest. `auth.fields` is what makes the agent's
/// Auth/Config button appear (`McpList.svelte::hasAuthRequired`); the fields are
/// documented as a single HTTP `Authorization` header because the control
/// transport is HTTP, not stdio.
fn manifest(control_url: &str) -> serde_json::Value {
    json!({
        "name": "Server Control",
        "version": "1.0.0",
        "description": "NeuxDOT control MCP: create and manage agents, plugins, providers and shares. Requires a per-agent HTTP Basic credential.",
        "managed_by": MANAGED_BY,
        "transport_hint": "http",
        "control_url": control_url,
        "auth": {
            "type": "http-basic",
            "fields": [
                {
                    "name": "authorization",
                    "env": "Authorization",
                    "label": "Authorization header",
                    "type": "password",
                    "placeholder": "Basic base64(username:password)",
                    "required": true,
                    "description": "base64(username:password) for the account this agent should act as. Generate with: printf 'user:pass' | base64"
                }
            ]
        },
        "mcp": {
            "tools": [
                { "name": "server_whoami" },
                { "name": "server_list_agents" },
                { "name": "server_get_agent" },
                { "name": "server_create_agent" },
                { "name": "server_update_agent" },
                { "name": "server_delete_agent" },
                { "name": "server_list_agent_shares" },
                { "name": "server_share_agent" },
                { "name": "server_unshare_agent" },
                { "name": "server_list_plugins" },
                { "name": "server_create_plugin" },
                { "name": "server_update_plugin" },
                { "name": "server_delete_plugin" },
                { "name": "server_list_providers" },
                { "name": "server_create_provider" },
                { "name": "server_update_provider" },
                { "name": "server_delete_provider" }
            ]
        }
    })
}

/// Declared grants for the seeded plugin.
///
/// `accounts` stays false on purpose: account administration is root-only and
/// must be opted into explicitly. Everything else is the surface an agent needs
/// to "add another bot".
fn permissions() -> serde_json::Value {
    json!({
        "ui": { "enabled": false, "resources": [] },
        "storage": { "get": false, "set": false, "delete": false, "keys": [] },
        "agent": { "trigger": false, "tools": true },
        "identity": {
            "agent.name": false,
            "agent.profile_picture": false,
            "user.name": false,
            "user.profile_picture": false
        },
        "process": false,
        // An `http` transport plugin is refused when network is denied, so the
        // loopback endpoint must be reachable. Scoped to loopback, not "*".
        "network": { "allow": ["127.0.0.1"] },
        "elevated": false,
        "control": {
            "agents_read": true,
            "agents_write": true,
            "shares": true,
            "plugins_read": true,
            "plugins_write": true,
            "providers_read": true,
            "providers_write": true,
            "accounts": false
        }
    })
}

/// Install (or refresh) the control plugin for one account and bind it to every
/// agent that account owns.
///
/// Idempotent: an existing row is left alone except for the loopback URL, which
/// is refreshed only when the row is still one this module manages.
pub async fn ensure_control_plugin(
    pool: &SqlitePool,
    account_id: &str,
    control_url: &str,
) -> Result<(), sqlx::Error> {
    let id = control_plugin_id(account_id);
    let manifest = manifest(control_url).to_string();
    let permissions = permissions().to_string();

    let existing: Option<(String, String)> =
        sqlx::query_as("SELECT id, COALESCE(manifest_json,'') FROM mcp_servers WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await?;

    match existing {
        None => {
            sqlx::query(
                r#"
                INSERT INTO mcp_servers
                    (id, account_id, name, alias, transport, url_or_command,
                     headers_or_env, manifest_json, permissions_json, is_active)
                VALUES (?, ?, 'Server Control', 'server', 'http', ?, NULL, ?, ?, 1)
                "#,
            )
            .bind(&id)
            .bind(account_id)
            .bind(control_url)
            .bind(&manifest)
            .bind(&permissions)
            .execute(pool)
            .await?;
        }
        Some((_, existing_manifest)) => {
            // Refresh only what we own. A URL change (different PORT) would
            // otherwise leave a dead endpoint behind. Permission edits made by
            // the operator are deliberately preserved.
            let managed = serde_json::from_str::<serde_json::Value>(&existing_manifest)
                .ok()
                .and_then(|v| v.get("managed_by").and_then(|m| m.as_str()).map(str::to_string))
                .as_deref()
                == Some(MANAGED_BY);
            if managed {
                sqlx::query(
                    "UPDATE mcp_servers SET url_or_command = ?, manifest_json = ? WHERE id = ?",
                )
                .bind(control_url)
                .bind(&manifest)
                .bind(&id)
                .execute(pool)
                .await?;
            }
        }
    }

    // Bind to every agent this account owns so the tools are discoverable.
    // `INSERT OR IGNORE` keeps an existing per-agent config override intact.
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO agent_mcp (agent_id, mcp_id)
        SELECT id, ? FROM agents WHERE account_id = ?
        "#,
    )
    .bind(&id)
    .bind(account_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// Seed the control plugin for every account on the server.
///
/// Called at boot, so the feature behaves as if it had always been installed —
/// including for databases created before this module existed.
pub async fn ensure_control_plugins(
    pool: &SqlitePool,
    control_url: &str,
) -> Result<usize, sqlx::Error> {
    let accounts: Vec<(String,)> = sqlx::query_as("SELECT id FROM accounts")
        .fetch_all(pool)
        .await?;
    let mut seeded = 0;
    for (account_id,) in accounts {
        ensure_control_plugin(pool, &account_id, control_url).await?;
        seeded += 1;
    }
    Ok(seeded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;

    async fn pool() -> SqlitePool {
        init_db("sqlite::memory:").await.unwrap()
    }

    async fn account(pool: &SqlitePool, username: &str) -> String {
        sqlx::query(
            "INSERT INTO accounts (id, username, password_hash, role) VALUES (?, ?, 'x', 'member')",
        )
        .bind(format!("acc-{username}"))
        .bind(username)
        .execute(pool)
        .await
        .unwrap();
        format!("acc-{username}")
    }

    #[tokio::test]
    async fn seeds_one_control_plugin_with_no_global_credential() {
        let pool = pool().await;
        let acc = account(&pool, "a").await;
        ensure_control_plugin(&pool, &acc, "http://127.0.0.1:8000/api/control/mcp")
            .await
            .unwrap();

        let row: (String, String, Option<String>) = sqlx::query_as(
            "SELECT transport, url_or_command, headers_or_env FROM mcp_servers WHERE id = ?",
        )
        .bind(control_plugin_id(&acc))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "http");
        assert!(row.1.ends_with("/api/control/mcp"));
        assert!(
            row.2.is_none(),
            "the shared row must not carry a credential; each agent supplies its own"
        );
    }

    #[tokio::test]
    async fn seeding_twice_does_not_duplicate() {
        let pool = pool().await;
        let acc = account(&pool, "a").await;
        ensure_control_plugin(&pool, &acc, "http://127.0.0.1:8000/api/control/mcp")
            .await
            .unwrap();
        ensure_control_plugin(&pool, &acc, "http://127.0.0.1:8000/api/control/mcp")
            .await
            .unwrap();
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM mcp_servers")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn reseed_refreshes_the_url_but_keeps_operator_permissions() {
        let pool = pool().await;
        let acc = account(&pool, "a").await;
        let id = control_plugin_id(&acc);
        ensure_control_plugin(&pool, &acc, "http://127.0.0.1:8000/api/control/mcp")
            .await
            .unwrap();

        // Operator narrows the grants.
        sqlx::query("UPDATE mcp_servers SET permissions_json = ? WHERE id = ?")
            .bind(r#"{"network":{"allow":["127.0.0.1"]},"control":{"agents_read":true}}"#)
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();

        // A later boot on a different port must not resurrect permissions.
        ensure_control_plugin(&pool, &acc, "http://127.0.0.1:8123/api/control/mcp")
            .await
            .unwrap();

        let (url, perms): (String, String) =
            sqlx::query_as("SELECT url_or_command, permissions_json FROM mcp_servers WHERE id = ?")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(url.contains(":8123"), "URL should follow the new port: {url}");
        assert!(
            perms.contains("agents_read"),
            "operator permission edit must survive: {perms}"
        );
    }

    #[tokio::test]
    async fn binds_to_existing_agents_but_keeps_their_override() {
        let pool = pool().await;
        let acc = account(&pool, "a").await;
        let id = control_plugin_id(&acc);
        sqlx::query("INSERT INTO agents (id, account_id, name) VALUES ('ag1', ?, 'Nexura')")
            .bind(&acc)
            .execute(&pool)
            .await
            .unwrap();
        // First boot seeds the row and binds the agent.
        ensure_control_plugin(&pool, &acc, "http://127.0.0.1:8000/api/control/mcp")
            .await
            .unwrap();

        // The operator then stores a per-agent credential.
        sqlx::query("UPDATE agent_mcp SET config_override = ? WHERE agent_id = 'ag1' AND mcp_id = ?")
            .bind(r#"{"Authorization":"Basic abc"}"#)
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();

        // A later boot re-runs the seed.
        ensure_control_plugin(&pool, &acc, "http://127.0.0.1:8000/api/control/mcp")
            .await
            .unwrap();

        let rows: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT agent_id, config_override FROM agent_mcp WHERE mcp_id = ?")
                .bind(&id)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "ag1");
        assert_eq!(
            rows[0].1.as_deref(),
            Some(r#"{"Authorization":"Basic abc"}"#),
            "an existing per-agent credential must not be clobbered"
        );
    }
}
