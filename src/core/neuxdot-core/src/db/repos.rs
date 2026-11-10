use sqlx::{FromRow, SqlitePool};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use super::schema::*;

/// Normalizes an optional avatar URL: whitespace-only values mean "no picture".
fn normalize_avatar_url(avatar_url: Option<&str>) -> Option<&str> {
    match avatar_url {
        Some(url) if url.trim().is_empty() => None,
        other => other,
    }
}

/// Merges global MCP plugin configuration with per-agent overrides.
///
/// If both are present, agent override values take precedence.
/// Returns a clean JSON string if keys exist, or None if empty.
pub fn merge_headers_or_env(global: Option<&str>, agent_override: Option<&str>) -> Option<String> {
    match (global, agent_override) {
        (None, None) => None,
        (Some(g), None) => {
            let t = g.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }
        (None, Some(o)) => {
            let t = o.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }
        (Some(g), Some(o)) => {
            let g_trim = g.trim();
            let o_trim = o.trim();
            if o_trim.is_empty() {
                return if g_trim.is_empty() {
                    None
                } else {
                    Some(g_trim.to_string())
                };
            }
            if g_trim.is_empty() {
                return Some(o_trim.to_string());
            }

            let mut merged = crate::mcp::stdio::parse_env_vars(g_trim);
            let overrides = crate::mcp::stdio::parse_env_vars(o_trim);
            for (k, v) in overrides {
                if !v.trim().is_empty() {
                    merged.insert(k, v);
                }
            }

            if merged.is_empty() {
                None
            } else {
                serde_json::to_string(&merged).ok()
            }
        }
    }
}

#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // --- Accounts ---
    /// Creates a regular (`member`) account together with its default profile.
    pub async fn create_account(
        &self,
        username: &str,
        password_hash: &str,
    ) -> Result<Account, sqlx::Error> {
        self.create_account_with_role(username, password_hash, ROLE_MEMBER)
            .await
    }

    /// Creates an account with an explicit role together with its default profile.
    pub async fn create_account_with_role(
        &self,
        username: &str,
        password_hash: &str,
        role: &str,
    ) -> Result<Account, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO accounts (id, username, password_hash, role) VALUES (?, ?, ?, ?)")
            .bind(&id)
            .bind(username)
            .bind(password_hash)
            .bind(role)
            .execute(&self.pool)
            .await?;

        // Also create a default profile
        let _ = sqlx::query(
            "INSERT INTO profiles (account_id, display_name, avatar_color, avatar_shape) VALUES (?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(username)
        .bind("#3b82f6")
        .bind("circle")
        .execute(&self.pool)
        .await;

        self.get_account_by_id(&id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    /// Minimal, non-privileged account lookup for share pickers.
    ///
    /// Returns only `(account_id, username, display_name, avatar_url)` so any
    /// authenticated user can pick a sharee without seeing password hashes,
    /// roles or timestamps of every account.
    pub async fn search_accounts_for_sharing(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<Vec<(String, String, Option<String>, Option<String>)>, sqlx::Error> {
        let like = format!("%{}%", query.trim());
        sqlx::query_as(
            r#"
            SELECT a.id, a.username, p.display_name, p.avatar_url
            FROM accounts a
            LEFT JOIN profiles p ON p.account_id = a.id
            WHERE ? = '%%' OR a.username LIKE ? OR COALESCE(p.display_name, '') LIKE ?
            ORDER BY a.username ASC
            LIMIT ?
            "#,
        )
        .bind(&like)
        .bind(&like)
        .bind(&like)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn list_accounts(&self) -> Result<Vec<Account>, sqlx::Error> {
        sqlx::query_as::<_, Account>(
            "SELECT * FROM accounts ORDER BY CASE WHEN role = 'root' THEN 0 ELSE 1 END, username ASC",
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn count_accounts_with_role(&self, role: &str) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM accounts WHERE role = ?")
            .bind(role)
            .fetch_one(&self.pool)
            .await
    }

    /// True when the given account is the only remaining account with the `root` role.
    pub async fn is_last_root(&self, account_id: &str) -> Result<bool, sqlx::Error> {
        let Some(account) = self.get_account_by_id(account_id).await? else {
            return Ok(false);
        };
        if !account.is_root() {
            return Ok(false);
        }
        let root_count = self.count_accounts_with_role(ROLE_ROOT).await?;
        Ok(root_count <= 1)
    }

    /// Updates the mutable account fields. `None` values are left untouched.
    pub async fn update_account(
        &self,
        account_id: &str,
        username: Option<&str>,
        role: Option<&str>,
    ) -> Result<Account, sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE accounts
            SET username = coalesce(?, username),
                role = coalesce(?, role)
            WHERE id = ?
            "#,
        )
        .bind(username)
        .bind(role)
        .bind(account_id)
        .execute(&self.pool)
        .await?;

        self.get_account_by_id(account_id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn delete_account(&self, account_id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM accounts WHERE id = ?")
            .bind(account_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn update_account_password(
        &self,
        account_id: &str,
        password_hash: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE accounts SET password_hash = ? WHERE id = ?")
            .bind(password_hash)
            .bind(account_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Initializes or retrieves the root account.
    /// The raw password is deterministically derived from the hash of the root account's created date.
    /// Guarantees that the root credentials remain the exact same on every server run.
    pub async fn get_or_init_root_account(&self) -> Result<(Account, String), sqlx::Error> {
        let existing = self.get_account_by_username("root").await?;

        if let Some(acc) = existing {
            let mut acc = acc;
            if !acc.is_root() {
                // Older databases may predate the role column / backfill.
                if let Ok(updated) = self.update_account(&acc.id, None, Some(ROLE_ROOT)).await {
                    acc = updated;
                }
            }

            let created_at_str = sqlx::query_scalar::<_, String>(
                "SELECT datetime(created_at) FROM accounts WHERE id = ?",
            )
            .bind(&acc.id)
            .fetch_optional(&self.pool)
            .await?
            .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());

            let raw_password = crate::auth::derive_root_password(&created_at_str);

            if !crate::auth::verify_password(&raw_password, &acc.password_hash) {
                if let Ok(new_hash) = crate::auth::hash_password(&raw_password) {
                    let _ = self.update_account_password(&acc.id, &new_hash).await;
                }
            }

            Ok((acc, raw_password))
        } else {
            let id = Uuid::new_v4().to_string();
            let now_str = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let raw_password = crate::auth::derive_root_password(&now_str);
            let password_hash = crate::auth::hash_password(&raw_password)
                .map_err(|e| sqlx::Error::Protocol(format!("Password hashing error: {}", e)))?;

            sqlx::query(
                "INSERT INTO accounts (id, username, password_hash, role, created_at) VALUES (?, 'root', ?, 'root', ?)"
            )
            .bind(&id)
            .bind(&password_hash)
            .bind(&now_str)
            .execute(&self.pool)
            .await?;

            let _ = sqlx::query(
                "INSERT INTO profiles (account_id, display_name, avatar_color, avatar_shape) VALUES (?, 'root', '#3b82f6', 'circle')"
            )
            .bind(&id)
            .execute(&self.pool)
            .await;

            let acc = self
                .get_account_by_id(&id)
                .await?
                .ok_or(sqlx::Error::RowNotFound)?;

            Ok((acc, raw_password))
        }
    }

    pub async fn get_account_by_id(&self, id: &str) -> Result<Option<Account>, sqlx::Error> {
        sqlx::query_as::<_, Account>("SELECT * FROM accounts WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn get_account_by_username(
        &self,
        username: &str,
    ) -> Result<Option<Account>, sqlx::Error> {
        sqlx::query_as::<_, Account>("SELECT * FROM accounts WHERE username = ?")
            .bind(username)
            .fetch_optional(&self.pool)
            .await
    }

    // --- Profiles ---
    pub async fn get_profile(&self, account_id: &str) -> Result<Option<Profile>, sqlx::Error> {
        sqlx::query_as::<_, Profile>("SELECT * FROM profiles WHERE account_id = ?")
            .bind(account_id)
            .fetch_optional(&self.pool)
            .await
    }

    /// Updates profile fields. `None` values are left untouched; an empty
    /// `avatar_url` clears the custom picture.
    pub async fn update_profile(
        &self,
        account_id: &str,
        display_name: Option<&str>,
        avatar_color: Option<&str>,
        avatar_shape: Option<&str>,
        avatar_url: Option<&str>,
    ) -> Result<Profile, sqlx::Error> {
        let normalized_avatar = match avatar_url {
            Some(url) if url.trim().is_empty() => None,
            other => other,
        };

        sqlx::query(
            r#"
            INSERT INTO profiles (account_id, display_name, avatar_color, avatar_shape, avatar_url)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(account_id) DO UPDATE SET
                display_name = coalesce(excluded.display_name, profiles.display_name),
                avatar_color = coalesce(excluded.avatar_color, profiles.avatar_color),
                avatar_shape = coalesce(excluded.avatar_shape, profiles.avatar_shape),
                avatar_url = coalesce(excluded.avatar_url, profiles.avatar_url)
            "#,
        )
        .bind(account_id)
        .bind(display_name)
        .bind(avatar_color)
        .bind(avatar_shape)
        .bind(normalized_avatar)
        .execute(&self.pool)
        .await?;

        if avatar_url.is_some() && normalized_avatar.is_none() {
            // Explicit empty value => clear the custom picture.
            sqlx::query("UPDATE profiles SET avatar_url = NULL WHERE account_id = ?")
                .bind(account_id)
                .execute(&self.pool)
                .await?;
        }

        self.get_profile(account_id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    // --- Providers ---
    pub async fn list_providers(&self, account_id: &str) -> Result<Vec<Provider>, sqlx::Error> {
        sqlx::query_as::<_, Provider>(
            "SELECT * FROM providers WHERE account_id = ? ORDER BY name ASC",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn list_providers_with_models(
        &self,
        account_id: &str,
    ) -> Result<Vec<ProviderWithModels>, sqlx::Error> {
        let providers = self.list_providers(account_id).await?;
        let mut result = Vec::with_capacity(providers.len());
        for p in providers {
            let models = self.list_provider_models(&p.id).await?;
            result.push(ProviderWithModels {
                provider: p,
                models,
            });
        }
        Ok(result)
    }

    pub async fn get_provider(&self, id: &str) -> Result<Option<Provider>, sqlx::Error> {
        sqlx::query_as::<_, Provider>("SELECT * FROM providers WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn get_provider_with_models(
        &self,
        id: &str,
    ) -> Result<Option<ProviderWithModels>, sqlx::Error> {
        if let Some(p) = self.get_provider(id).await? {
            let models = self.list_provider_models(&p.id).await?;
            Ok(Some(ProviderWithModels {
                provider: p,
                models,
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn create_provider(
        &self,
        account_id: &str,
        name: &str,
        base_url: &str,
        api_key: Option<&str>,
    ) -> Result<Provider, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"
            INSERT INTO providers (id, account_id, name, base_url, api_key)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(&id)
        .bind(account_id)
        .bind(name)
        .bind(base_url)
        .bind(api_key)
        .execute(&self.pool)
        .await?;

        self.get_provider(&id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn update_provider(
        &self,
        id: &str,
        name: &str,
        base_url: &str,
        api_key: Option<&str>,
    ) -> Result<Provider, sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE providers
            SET name = ?, base_url = ?, api_key = ?
            WHERE id = ?
            "#,
        )
        .bind(name)
        .bind(base_url)
        .bind(api_key)
        .bind(id)
        .execute(&self.pool)
        .await?;

        self.get_provider(id).await?.ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn delete_provider(&self, id: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM providers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // --- Provider Models ---
    pub async fn list_provider_models(
        &self,
        provider_id: &str,
    ) -> Result<Vec<ProviderModel>, sqlx::Error> {
        sqlx::query_as::<_, ProviderModel>(
            "SELECT * FROM provider_models WHERE provider_id = ? ORDER BY model_id ASC",
        )
        .bind(provider_id)
        .fetch_all(&self.pool)
        .await
    }

    fn split_config_json(
        config_json: Option<&str>,
        params_json: Option<&str>,
        context_window: Option<i64>,
    ) -> (Option<String>, Option<i64>) {
        let mut effective_params = params_json.map(|s| s.to_string());
        let mut effective_cw = context_window;

        if let Some(raw_cfg) = config_json {
            if let Ok(serde_json::Value::Object(mut map)) =
                serde_json::from_str::<serde_json::Value>(raw_cfg)
            {
                if effective_cw.is_none() {
                    if let Some(cw_val) = map.remove("context_window") {
                        effective_cw = cw_val.as_i64();
                    }
                } else {
                    map.remove("context_window");
                }
                map.remove("name");

                if effective_params.is_none() {
                    if let Ok(serialized) = serde_json::to_string(&map) {
                        effective_params = Some(serialized);
                    }
                }
            }
        }

        (effective_params, effective_cw)
    }

    pub async fn add_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
        name: Option<&str>,
        config_json: Option<&str>,
        params_json: Option<&str>,
        context_window: Option<i64>,
    ) -> Result<ProviderModel, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let (effective_params, effective_cw) =
            Self::split_config_json(config_json, params_json, context_window);
        sqlx::query(
            r#"
            INSERT INTO provider_models (id, provider_id, model_id, name, config_json, params_json, context_window, is_active)
            VALUES (?, ?, ?, ?, ?, ?, ?, 1)
            "#,
        )
        .bind(&id)
        .bind(provider_id)
        .bind(model_id)
        .bind(name)
        .bind(config_json)
        .bind(effective_params.as_deref())
        .bind(effective_cw)
        .execute(&self.pool)
        .await?;

        sqlx::query_as::<_, ProviderModel>("SELECT * FROM provider_models WHERE id = ?")
            .bind(&id)
            .fetch_one(&self.pool)
            .await
    }

    pub async fn set_provider_models(
        &self,
        provider_id: &str,
        models: &[ModelConfigInput],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM provider_models WHERE provider_id = ?")
            .bind(provider_id)
            .execute(&mut *tx)
            .await?;

        for m in models {
            let id = Uuid::new_v4().to_string();
            let (effective_params, effective_cw) = Self::split_config_json(
                m.config_json.as_deref(),
                m.params_json.as_deref(),
                m.context_window,
            );
            sqlx::query(
                r#"
                INSERT INTO provider_models (id, provider_id, model_id, name, config_json, params_json, context_window, is_active)
                VALUES (?, ?, ?, ?, ?, ?, ?, 1)
                "#,
            )
            .bind(&id)
            .bind(provider_id)
            .bind(&m.model_id)
            .bind(m.name.as_deref())
            .bind(m.config_json.as_deref())
            .bind(effective_params.as_deref())
            .bind(effective_cw)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_provider_model(&self, id: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM provider_models WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // --- Agents ---
    pub async fn list_agents(&self, account_id: &str) -> Result<Vec<Agent>, sqlx::Error> {
        sqlx::query_as::<_, Agent>("SELECT * FROM agents WHERE account_id = ? ORDER BY name ASC")
            .bind(account_id)
            .fetch_all(&self.pool)
            .await
    }

    // --- Agent sharing ---

    /// Resolves what `account_id` may do with an agent.
    pub async fn resolve_agent_access(
        &self,
        agent: &Agent,
        account_id: &str,
    ) -> Result<AgentAccess, sqlx::Error> {
        if agent.account_id == account_id {
            return Ok(AgentAccess::Owner);
        }
        let share: Option<(bool, bool)> = sqlx::query_as(
            "SELECT can_edit, can_manage_shares FROM agent_shares WHERE agent_id = ? AND account_id = ?",
        )
        .bind(&agent.id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(match share {
            Some((can_edit, can_manage_shares)) => AgentAccess::Shared {
                can_edit,
                can_manage_shares,
            },
            None => AgentAccess::None,
        })
    }

    /// Every agent the account can see: its own, plus those shared with it.
    /// Each row is (agent, is_owner, can_edit, can_manage_shares).
    pub async fn list_agents_for_account(
        &self,
        account_id: &str,
    ) -> Result<Vec<(Agent, bool, bool, bool)>, sqlx::Error> {
        let rows = sqlx::query_as::<_, Agent>(
            r#"
            SELECT a.* FROM agents a
            WHERE a.account_id = ?
               OR EXISTS (SELECT 1 FROM agent_shares s WHERE s.agent_id = a.id AND s.account_id = ?)
            ORDER BY a.name ASC
            "#,
        )
        .bind(account_id)
        .bind(account_id)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for agent in rows {
            let access = self.resolve_agent_access(&agent, account_id).await?;
            out.push((
                agent,
                access.is_owner(),
                access.can_edit(),
                access.can_manage_shares(),
            ));
        }
        Ok(out)
    }

    /// Shares an agent with `account_id`. Idempotent per (agent, account).
    pub async fn upsert_agent_share(
        &self,
        agent_id: &str,
        account_id: &str,
        can_edit: bool,
        can_manage_shares: bool,
    ) -> Result<AgentShare, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"
            INSERT INTO agent_shares (id, agent_id, account_id, can_edit, can_manage_shares)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(agent_id, account_id)
            DO UPDATE SET can_edit = excluded.can_edit,
                          can_manage_shares = excluded.can_manage_shares
            "#,
        )
        .bind(&id)
        .bind(agent_id)
        .bind(account_id)
        .bind(can_edit)
        .bind(can_manage_shares)
        .execute(&self.pool)
        .await?;

        sqlx::query_as::<_, AgentShare>(
            "SELECT * FROM agent_shares WHERE agent_id = ? AND account_id = ?",
        )
        .bind(agent_id)
        .bind(account_id)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn update_agent_share(
        &self,
        agent_id: &str,
        account_id: &str,
        can_edit: Option<bool>,
        can_manage_shares: Option<bool>,
    ) -> Result<Option<AgentShare>, sqlx::Error> {
        let Some(current) = self.get_agent_share(agent_id, account_id).await? else {
            return Ok(None);
        };
        let final_edit = can_edit.unwrap_or(current.can_edit);
        let final_manage = can_manage_shares.unwrap_or(current.can_manage_shares);

        sqlx::query(
            "UPDATE agent_shares SET can_edit = ?, can_manage_shares = ? WHERE agent_id = ? AND account_id = ?",
        )
        .bind(final_edit)
        .bind(final_manage)
        .bind(agent_id)
        .bind(account_id)
        .execute(&self.pool)
        .await?;

        self.get_agent_share(agent_id, account_id).await
    }

    pub async fn get_agent_share(
        &self,
        agent_id: &str,
        account_id: &str,
    ) -> Result<Option<AgentShare>, sqlx::Error> {
        sqlx::query_as::<_, AgentShare>(
            "SELECT * FROM agent_shares WHERE agent_id = ? AND account_id = ?",
        )
        .bind(agent_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await
    }

    /// Removes a share. Returns false when there was nothing to remove.
    pub async fn delete_agent_share(
        &self,
        agent_id: &str,
        account_id: &str,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM agent_shares WHERE agent_id = ? AND account_id = ?")
            .bind(agent_id)
            .bind(account_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Shares with the granted account, joined with profile info for display.
    pub async fn list_agent_shares(
        &self,
        agent_id: &str,
    ) -> Result<Vec<serde_json::Value>, sqlx::Error> {
        let rows: Vec<(
            String,
            String,
            Option<String>,
            Option<String>,
            bool,
            bool,
            Option<chrono::DateTime<chrono::Utc>>,
        )> = sqlx::query_as(
            r#"
                SELECT s.account_id, a.username, p.display_name, p.avatar_url,
                       s.can_edit, s.can_manage_shares, s.created_at
                FROM agent_shares s
                JOIN accounts a ON a.id = s.account_id
                LEFT JOIN profiles p ON p.account_id = s.account_id
                WHERE s.agent_id = ?
                ORDER BY a.username ASC
                "#,
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    account_id,
                    username,
                    display_name,
                    avatar_url,
                    can_edit,
                    can_manage_shares,
                    created_at,
                )| {
                    serde_json::json!({
                        "account_id": account_id,
                        "username": username,
                        "display_name": display_name,
                        "avatar_url": avatar_url,
                        "can_edit": can_edit,
                        "can_manage_shares": can_manage_shares,
                        "created_at": created_at,
                    })
                },
            )
            .collect())
    }

    pub async fn get_agent(&self, id: &str) -> Result<Option<Agent>, sqlx::Error> {
        sqlx::query_as::<_, Agent>("SELECT * FROM agents WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn create_agent(
        &self,
        account_id: &str,
        name: &str,
        avatar_color: Option<&str>,
        avatar_shape: Option<&str>,
        avatar_url: Option<&str>,
        strategy: &str,
        system_prompt: Option<&str>,
        allow_output_text: Option<bool>,
        output_text_error_message: Option<&str>,
        max_running_history_chars: Option<i64>,
        response_hooks_json: Option<&str>,
        instruction_preset_id: Option<&str>,
    ) -> Result<Agent, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"
            INSERT INTO agents (id, account_id, name, avatar_color, avatar_shape, avatar_url, strategy, system_prompt, is_active, allow_output_text, output_text_error_message, max_running_history_chars, response_hooks_json, instruction_preset_id)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&id)
        .bind(account_id)
        .bind(name)
        .bind(avatar_color.unwrap_or("#6366f1"))
        .bind(avatar_shape.unwrap_or("circle"))
        .bind(normalize_avatar_url(avatar_url))
        .bind(strategy)
        .bind(system_prompt)
        .bind(allow_output_text.unwrap_or(false))
        .bind(output_text_error_message.unwrap_or("Output Text is not allowed, consider use correct tool or sleep."))
        .bind(max_running_history_chars.unwrap_or(300_000))
        .bind(response_hooks_json.unwrap_or("[]"))
        .bind(instruction_preset_id)
        .execute(&self.pool)
        .await?;

        self.get_agent(&id).await?.ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn patch_agent(
        &self,
        id: &str,
        name: Option<&str>,
        avatar_color: Option<&str>,
        avatar_shape: Option<&str>,
        avatar_url: Option<&str>,
        strategy: Option<&str>,
        system_prompt: Option<&str>,
        is_active: Option<bool>,
        allow_output_text: Option<bool>,
        output_text_error_message: Option<&str>,
        max_running_history_chars: Option<i64>,
        response_hooks_json: Option<&str>,
        instruction_preset_id: Option<&str>,
    ) -> Result<Agent, sqlx::Error> {
        let current = self.get_agent(id).await?.ok_or(sqlx::Error::RowNotFound)?;

        let final_name = name.unwrap_or(&current.name);
        let final_avatar_color = avatar_color.or(current.avatar_color.as_deref());
        let final_avatar_shape = avatar_shape.or(current.avatar_shape.as_deref());
        // `None` leaves the picture untouched; an explicitly empty value clears it to NULL.
        let final_avatar_url = match avatar_url {
            Some(url) if url.trim().is_empty() => None,
            Some(url) => Some(url),
            None => current.avatar_url.as_deref(),
        };
        let final_strategy = strategy.unwrap_or(&current.strategy);
        let final_system_prompt = system_prompt.or(current.system_prompt.as_deref());
        let final_is_active = is_active.unwrap_or(current.is_active);
        let final_allow_output_text = allow_output_text.unwrap_or(current.allow_output_text);
        let final_output_text_error_message =
            output_text_error_message.or(current.output_text_error_message.as_deref());
        let final_max_history =
            max_running_history_chars.unwrap_or(current.max_running_history_chars);
        let final_hooks_json = response_hooks_json
            .or(current.response_hooks_json.as_deref())
            .unwrap_or("[]");
        // `None` leaves the selection untouched; an explicitly empty value
        // detaches the agent from its preset (back to Custom).
        let final_preset_id = match instruction_preset_id {
            Some(p) if p.trim().is_empty() => None,
            Some(p) => Some(p),
            None => current.instruction_preset_id.as_deref(),
        };

        sqlx::query(
            r#"
            UPDATE agents
            SET name = ?, avatar_color = ?, avatar_shape = ?, avatar_url = ?, strategy = ?, system_prompt = ?, is_active = ?, allow_output_text = ?, output_text_error_message = ?, max_running_history_chars = ?, response_hooks_json = ?, instruction_preset_id = ?
            WHERE id = ?
            "#,
        )
        .bind(final_name)
        .bind(final_avatar_color)
        .bind(final_avatar_shape)
        .bind(final_avatar_url)
        .bind(final_strategy)
        .bind(final_system_prompt)
        .bind(final_is_active)
        .bind(final_allow_output_text)
        .bind(final_output_text_error_message)
        .bind(final_max_history)
        .bind(final_hooks_json)
        .bind(final_preset_id)
        .bind(id)
        .execute(&self.pool)
        .await?;

        self.get_agent(id).await?.ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn delete_agent(&self, id: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM agents WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // --- Agent Provider Model Bindings ---
    pub async fn get_agent_models(
        &self,
        agent_id: &str,
    ) -> Result<Vec<ActiveModelTarget>, sqlx::Error> {
        sqlx::query_as::<_, ActiveModelTarget>(
            r#"
            SELECT 
                pm.id as id,
                pm.provider_id as provider_id,
                p.name as provider_name,
                p.base_url as base_url,
                p.api_key as api_key,
                pm.model_id as model_id,
                pm.name as model_name,
                pm.config_json as config_json,
                pm.params_json as params_json,
                pm.context_window as context_window
            FROM provider_models pm
            JOIN providers p ON p.id = pm.provider_id
            JOIN agent_providers ap ON ap.provider_model_id = pm.id
            WHERE ap.agent_id = ? AND pm.is_active = 1
            ORDER BY ap.priority ASC
            "#,
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_agent_provider_model_ids(
        &self,
        agent_id: &str,
    ) -> Result<Vec<String>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String,)>(
            "SELECT provider_model_id FROM agent_providers WHERE agent_id = ? ORDER BY priority ASC",
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    pub async fn set_agent_models(
        &self,
        agent_id: &str,
        provider_model_ids: &[String],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM agent_providers WHERE agent_id = ?")
            .bind(agent_id)
            .execute(&mut *tx)
            .await?;

        for (idx, pmid) in provider_model_ids.iter().enumerate() {
            sqlx::query(
                "INSERT INTO agent_providers (agent_id, provider_model_id, priority) VALUES (?, ?, ?)",
            )
            .bind(agent_id)
            .bind(pmid)
            .bind(idx as i32)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    // --- MCP Servers ---
    pub async fn list_mcp_servers(&self, account_id: &str) -> Result<Vec<McpServer>, sqlx::Error> {
        sqlx::query_as::<_, McpServer>(
            "SELECT * FROM mcp_servers WHERE account_id = ? ORDER BY name ASC",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_mcp_server(&self, id: &str) -> Result<Option<McpServer>, sqlx::Error> {
        sqlx::query_as::<_, McpServer>("SELECT * FROM mcp_servers WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn create_mcp_server(
        &self,
        account_id: &str,
        name: &str,
        alias: Option<&str>,
        transport: &str,
        url_or_command: &str,
        headers_or_env: Option<&str>,
    ) -> Result<McpServer, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO mcp_servers (id, account_id, name, alias, transport, url_or_command, headers_or_env) VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&id)
        .bind(account_id)
        .bind(name)
        .bind(alias)
        .bind(transport)
        .bind(url_or_command)
        .bind(headers_or_env)
        .execute(&self.pool)
        .await?;

        self.get_mcp_server(&id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn update_mcp_server(
        &self,
        id: &str,
        name: &str,
        alias: Option<&str>,
        transport: &str,
        url_or_command: &str,
        headers_or_env: Option<&str>,
    ) -> Result<McpServer, sqlx::Error> {
        sqlx::query(
            "UPDATE mcp_servers SET name = ?, alias = ?, transport = ?, url_or_command = ?, headers_or_env = ? WHERE id = ?",
        )
        .bind(name)
        .bind(alias)
        .bind(transport)
        .bind(url_or_command)
        .bind(headers_or_env)
        .bind(id)
        .execute(&self.pool)
        .await?;

        self.get_mcp_server(id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn delete_mcp_server(&self, id: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM mcp_servers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        let _ = sqlx::query("DELETE FROM extension_kv WHERE extension_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await;
        Ok(res.rows_affected() > 0)
    }

    // --- Agent MCP Bindings ---
    /// Agent ids that have this plugin bound. Used to re-reconcile the push
    /// subscription hub after a plugin's config or consent changes.
    pub async fn list_agent_ids_for_mcp(&self, mcp_id: &str) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT agent_id FROM agent_mcp WHERE mcp_id = ?")
                .bind(mcp_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    pub async fn get_agent_mcp_servers(
        &self,
        agent_id: &str,
    ) -> Result<Vec<McpServer>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT m.*, am.config_override AS agent_config_override FROM mcp_servers m
            JOIN agent_mcp am ON am.mcp_id = m.id
            WHERE am.agent_id = ?
            ORDER BY m.name ASC
            "#,
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await?;

        let mut servers = Vec::with_capacity(rows.len());
        for row in rows {
            use sqlx::Row;
            let mut server = McpServer::from_row(&row)?;
            let agent_config_override: Option<String> =
                row.try_get("agent_config_override").ok().flatten();
            server.headers_or_env = merge_headers_or_env(
                server.headers_or_env.as_deref(),
                agent_config_override.as_deref(),
            );
            servers.push(server);
        }

        Ok(servers)
    }

    pub async fn get_agent_mcp_configs(
        &self,
        agent_id: &str,
    ) -> Result<HashMap<String, String>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT mcp_id, config_override FROM agent_mcp WHERE agent_id = ? AND config_override IS NOT NULL",
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await?;

        let mut map = HashMap::new();
        for row in rows {
            use sqlx::Row;
            if let (Ok(mcp_id), Ok(Some(cfg))) = (
                row.try_get::<String, _>("mcp_id"),
                row.try_get::<Option<String>, _>("config_override"),
            ) {
                if !cfg.trim().is_empty() {
                    map.insert(mcp_id, cfg);
                }
            }
        }
        Ok(map)
    }

    pub async fn get_agent_mcp_config(
        &self,
        agent_id: &str,
        mcp_id: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row =
            sqlx::query("SELECT config_override FROM agent_mcp WHERE agent_id = ? AND mcp_id = ?")
                .bind(agent_id)
                .bind(mcp_id)
                .fetch_optional(&self.pool)
                .await?;

        Ok(row.and_then(|r| {
            use sqlx::Row;
            r.try_get::<Option<String>, _>("config_override")
                .ok()
                .flatten()
        }))
    }

    pub async fn set_agent_mcp_config(
        &self,
        agent_id: &str,
        mcp_id: &str,
        config_override: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let clean_override = config_override.map(|s| s.trim()).filter(|s| !s.is_empty());

        // Ensure binding exists
        let _ = sqlx::query("INSERT OR IGNORE INTO agent_mcp (agent_id, mcp_id) VALUES (?, ?)")
            .bind(agent_id)
            .bind(mcp_id)
            .execute(&self.pool)
            .await?;

        let res = sqlx::query(
            "UPDATE agent_mcp SET config_override = ? WHERE agent_id = ? AND mcp_id = ?",
        )
        .bind(clean_override)
        .bind(agent_id)
        .bind(mcp_id)
        .execute(&self.pool)
        .await?;

        Ok(res.rows_affected() > 0)
    }

    pub async fn set_agent_mcp_servers(
        &self,
        agent_id: &str,
        mcp_ids: &[String],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        if mcp_ids.is_empty() {
            sqlx::query("DELETE FROM agent_mcp WHERE agent_id = ?")
                .bind(agent_id)
                .execute(&mut *tx)
                .await?;
        } else {
            // Delete removed bindings while preserving existing ones and their config_override
            let placeholders = mcp_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let delete_query = format!(
                "DELETE FROM agent_mcp WHERE agent_id = ? AND mcp_id NOT IN ({})",
                placeholders
            );
            let mut q = sqlx::query(&delete_query).bind(agent_id);
            for mid in mcp_ids {
                q = q.bind(mid);
            }
            q.execute(&mut *tx).await?;

            for mid in mcp_ids {
                sqlx::query("INSERT OR IGNORE INTO agent_mcp (agent_id, mcp_id) VALUES (?, ?)")
                    .bind(agent_id)
                    .bind(mid)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        tx.commit().await?;
        Ok(())
    }

    // --- MCP Pushed Notifications / Information ---
    pub async fn save_pushed_message(
        &self,
        agent_id: &str,
        mcp_id: Option<&str>,
        method: &str,
        params_json: Option<&str>,
    ) -> Result<AgentPushedMessage, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO agent_pushed_messages (id, agent_id, mcp_id, method, params_json) VALUES (?, ?, ?, ?, ?)"
        )
        .bind(&id)
        .bind(agent_id)
        .bind(mcp_id)
        .bind(method)
        .bind(params_json)
        .execute(&self.pool)
        .await?;

        sqlx::query_as::<_, AgentPushedMessage>("SELECT * FROM agent_pushed_messages WHERE id = ?")
            .bind(&id)
            .fetch_one(&self.pool)
            .await
    }

    pub async fn list_pushed_messages(
        &self,
        agent_id: &str,
        limit: i64,
    ) -> Result<Vec<AgentPushedMessage>, sqlx::Error> {
        sqlx::query_as::<_, AgentPushedMessage>(
            "SELECT * FROM agent_pushed_messages WHERE agent_id = ? ORDER BY created_at DESC LIMIT ?",
        )
        .bind(agent_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn list_unconsumed_pushed_messages(
        &self,
        agent_id: &str,
    ) -> Result<Vec<AgentPushedMessage>, sqlx::Error> {
        sqlx::query_as::<_, AgentPushedMessage>(
            "SELECT * FROM agent_pushed_messages WHERE agent_id = ? AND is_consumed = 0 ORDER BY created_at ASC",
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn mark_pushed_messages_consumed(
        &self,
        agent_id: &str,
        ids: &[String],
    ) -> Result<(), sqlx::Error> {
        for id in ids {
            sqlx::query(
                "UPDATE agent_pushed_messages SET is_consumed = 1 WHERE id = ? AND agent_id = ?",
            )
            .bind(id)
            .bind(agent_id)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    // --- Granular Agent Tool Permissions ---
    pub async fn get_agent_tool_permissions(
        &self,
        agent_id: &str,
    ) -> Result<Vec<AgentToolPermission>, sqlx::Error> {
        sqlx::query_as::<_, AgentToolPermission>(
            "SELECT * FROM agent_tool_permissions WHERE agent_id = ?",
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn set_agent_tool_permission(
        &self,
        agent_id: &str,
        mcp_id: &str,
        tool_name: &str,
        is_enabled: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO agent_tool_permissions (agent_id, mcp_id, tool_name, is_enabled)
            VALUES (?, ?, ?, ?)
            ON CONFLICT (agent_id, mcp_id, tool_name)
            DO UPDATE SET is_enabled = excluded.is_enabled
            "#,
        )
        .bind(agent_id)
        .bind(mcp_id)
        .bind(tool_name)
        .bind(is_enabled)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // --- Agent Subscriptions ---
    pub async fn get_agent_subscriptions(
        &self,
        agent_id: &str,
    ) -> Result<Vec<String>, sqlx::Error> {
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT uri FROM agent_subscriptions WHERE agent_id = ? ORDER BY uri ASC",
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn set_agent_subscriptions(
        &self,
        agent_id: &str,
        uris: &[String],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM agent_subscriptions WHERE agent_id = ?")
            .bind(agent_id)
            .execute(&mut *tx)
            .await?;
        for uri in uris {
            sqlx::query("INSERT INTO agent_subscriptions (agent_id, uri) VALUES (?, ?)")
                .bind(agent_id)
                .bind(uri)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn update_plugin_permissions(
        &self,
        id: &str,
        permissions_json: &str,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("UPDATE mcp_servers SET permissions_json = ? WHERE id = ?")
            .bind(permissions_json)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn set_plugin_active(&self, id: &str, is_active: bool) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("UPDATE mcp_servers SET is_active = ? WHERE id = ?")
            .bind(is_active)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // --- Agent Memory ---
    // -------------------------------------------------------------------------
    // Keyed memory graph (Obsidian-style)
    // -------------------------------------------------------------------------

    /// Every node for an agent, ordered by key.
    pub async fn list_memory_nodes(&self, agent_id: &str) -> Result<Vec<MemoryNode>, sqlx::Error> {
        sqlx::query_as::<_, MemoryNode>(
            "SELECT * FROM agent_memory_nodes WHERE agent_id = ? ORDER BY key ASC",
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_memory_node(
        &self,
        agent_id: &str,
        key: &str,
    ) -> Result<Option<MemoryNode>, sqlx::Error> {
        sqlx::query_as::<_, MemoryNode>(
            "SELECT * FROM agent_memory_nodes WHERE agent_id = ? AND key = ?",
        )
        .bind(agent_id)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn count_memory_nodes(&self, agent_id: &str) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_memory_nodes WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_one(&self.pool)
            .await
    }

    /// Everything needed to render the graph in one round trip.
    pub async fn memory_nodes_with_links(
        &self,
        agent_id: &str,
    ) -> Result<Vec<MemoryNodeView>, sqlx::Error> {
        let nodes = self.list_memory_nodes(agent_id).await?;
        let links = sqlx::query_as::<_, MemoryLink>(
            "SELECT agent_id, src, dst FROM agent_memory_links WHERE agent_id = ?",
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(nodes
            .into_iter()
            .map(|node| {
                let outbound: Vec<String> = links
                    .iter()
                    .filter(|l| l.src == node.key)
                    .map(|l| l.dst.clone())
                    .collect();
                let backlinks: Vec<String> = links
                    .iter()
                    .filter(|l| l.dst == node.key)
                    .map(|l| l.src.clone())
                    .collect();
                MemoryNodeView {
                    key: node.key,
                    r#type: node.r#type,
                    description: node.description,
                    value: node.value,
                    hash: node.hash,
                    updated_at: node.updated_at,
                    outbound,
                    backlinks,
                }
            })
            .collect())
    }

    /// Create or replace a node and re-derive its outgoing links atomically.
    ///
    /// The link rows for `src` are deleted and re-inserted in the same
    /// transaction as the value, so the graph can never disagree with the text
    /// that produced it.
    pub async fn upsert_memory_node(
        &self,
        agent_id: &str,
        key: &str,
        value: &str,
        node_type: &str,
        description: &str,
        hash: &str,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO agent_memory_nodes
                (agent_id, key, value, type, description, hash, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
            ON CONFLICT(agent_id, key) DO UPDATE SET
                value = excluded.value,
                type = excluded.type,
                description = excluded.description,
                hash = excluded.hash,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(agent_id)
        .bind(key)
        .bind(value)
        .bind(node_type)
        .bind(description)
        .bind(hash)
        .execute(&mut *tx)
        .await?;

        sqlx::query("DELETE FROM agent_memory_links WHERE agent_id = ? AND src = ?")
            .bind(agent_id)
            .bind(key)
            .execute(&mut *tx)
            .await?;

        for target in extract_wikilinks(value) {
            // A self-link is meaningless and would create a self-loop in the UI.
            if target == key {
                continue;
            }
            sqlx::query(
                "INSERT OR IGNORE INTO agent_memory_links (agent_id, src, dst) VALUES (?, ?, ?)",
            )
            .bind(agent_id)
            .bind(key)
            .bind(&target)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await
    }

    /// Delete a node and every edge touching it, in one transaction.
    ///
    /// Links *to* the deleted key are removed too: a dangling edge would make
    /// the graph show a node that no longer exists.
    pub async fn delete_memory_node(&self, agent_id: &str, key: &str) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        let result = sqlx::query("DELETE FROM agent_memory_nodes WHERE agent_id = ? AND key = ?")
            .bind(agent_id)
            .bind(key)
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM agent_memory_links WHERE agent_id = ? AND (src = ? OR dst = ?)")
            .bind(agent_id)
            .bind(key)
            .bind(key)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    /// Case-insensitive substring search over keys, descriptions and values.
    ///
    /// Ranked: key matches first (the node is *about* the query), then
    /// description, then value only.
    pub async fn search_memory_nodes(
        &self,
        agent_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryNode>, sqlx::Error> {
        let lowered = query.trim().to_lowercase();
        if lowered.is_empty() {
            return Ok(Vec::new());
        }
        let all = self.list_memory_nodes(agent_id).await?;
        let mut scored: Vec<(u8, MemoryNode)> = all
            .into_iter()
            .filter_map(|node| {
                let key_hit = node.key.to_lowercase().contains(&lowered);
                let desc_hit = node.description.to_lowercase().contains(&lowered);
                let value_hit = node.value.to_lowercase().contains(&lowered);
                if key_hit {
                    Some((0, node))
                } else if desc_hit {
                    Some((1, node))
                } else if value_hit {
                    Some((2, node))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.key.cmp(&b.1.key)));
        Ok(scored
            .into_iter()
            .take(limit.clamp(1, 100))
            .map(|(_, node)| node)
            .collect())
    }

    pub async fn get_agent_memory(&self, agent_id: &str) -> Result<AgentMemory, sqlx::Error> {
        if let Some(mem) =
            sqlx::query_as::<_, AgentMemory>("SELECT * FROM agent_memories WHERE agent_id = ?")
                .bind(agent_id)
                .fetch_optional(&self.pool)
                .await?
        {
            Ok(mem)
        } else {
            let empty_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
            let _ = sqlx::query(
                "INSERT INTO agent_memories (agent_id, content, hash) VALUES (?, '', ?) ON CONFLICT(agent_id) DO NOTHING",
            )
            .bind(agent_id)
            .bind(empty_hash)
            .execute(&self.pool)
            .await;

            sqlx::query_as::<_, AgentMemory>("SELECT * FROM agent_memories WHERE agent_id = ?")
                .bind(agent_id)
                .fetch_one(&self.pool)
                .await
        }
    }

    pub async fn set_agent_memory(
        &self,
        agent_id: &str,
        content: &str,
        hash: &str,
    ) -> Result<AgentMemory, sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO agent_memories (agent_id, content, hash, updated_at)
            VALUES (?, ?, ?, CURRENT_TIMESTAMP)
            ON CONFLICT(agent_id) DO UPDATE SET
                content = excluded.content,
                hash = excluded.hash,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(agent_id)
        .bind(content)
        .bind(hash)
        .execute(&self.pool)
        .await?;

        self.get_agent_memory(agent_id).await
    }

    // --- Agent Instruction Presets (owner-scoped) ---
    //
    // Every query filters by `account_id`; a preset belonging to another
    // account is indistinguishable from a missing one.

    pub async fn list_instruction_presets(
        &self,
        account_id: &str,
    ) -> Result<Vec<AgentInstructionPreset>, sqlx::Error> {
        sqlx::query_as::<_, AgentInstructionPreset>(
            "SELECT * FROM agent_instruction_presets WHERE account_id = ? ORDER BY created_at ASC, rowid ASC",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_instruction_preset(
        &self,
        account_id: &str,
        id: &str,
    ) -> Result<Option<AgentInstructionPreset>, sqlx::Error> {
        sqlx::query_as::<_, AgentInstructionPreset>(
            "SELECT * FROM agent_instruction_presets WHERE id = ? AND account_id = ?",
        )
        .bind(id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn create_instruction_preset(
        &self,
        account_id: &str,
        name: &str,
        description: &str,
        content: &str,
    ) -> Result<AgentInstructionPreset, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"
            INSERT INTO agent_instruction_presets (id, account_id, name, description, content)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(&id)
        .bind(account_id)
        .bind(name)
        .bind(description)
        .bind(content)
        .execute(&self.pool)
        .await?;

        self.get_instruction_preset(account_id, &id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    /// Updates the fields that are `Some`. Returns `None` when the preset does
    /// not exist *for this account*.
    pub async fn update_instruction_preset(
        &self,
        account_id: &str,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
        content: Option<&str>,
    ) -> Result<Option<AgentInstructionPreset>, sqlx::Error> {
        let Some(current) = self.get_instruction_preset(account_id, id).await? else {
            return Ok(None);
        };

        let final_name = name.unwrap_or(&current.name);
        let final_description = description.unwrap_or(&current.description);
        let final_content = content.unwrap_or(&current.content);

        sqlx::query(
            r#"
            UPDATE agent_instruction_presets
            SET name = ?, description = ?, content = ?, updated_at = CURRENT_TIMESTAMP
            WHERE id = ? AND account_id = ?
            "#,
        )
        .bind(final_name)
        .bind(final_description)
        .bind(final_content)
        .bind(id)
        .bind(account_id)
        .execute(&self.pool)
        .await?;

        self.get_instruction_preset(account_id, id).await
    }

    /// Deletes a preset owned by `account_id`. Returns false when not found.
    /// Agents that referenced it are detached (`instruction_preset_id` -> NULL)
    /// and keep their current `system_prompt` text, so nothing changes for them.
    pub async fn delete_instruction_preset(
        &self,
        account_id: &str,
        id: &str,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        let affected =
            sqlx::query("DELETE FROM agent_instruction_presets WHERE id = ? AND account_id = ?")
                .bind(id)
                .bind(account_id)
                .execute(&mut *tx)
                .await?
                .rows_affected();

        if affected > 0 {
            let _ = sqlx::query(
                "UPDATE agents SET instruction_preset_id = NULL WHERE instruction_preset_id = ? AND account_id = ?",
            )
            .bind(id)
            .bind(account_id)
            .execute(&mut *tx)
            .await;
        }

        tx.commit().await?;
        Ok(affected > 0)
    }

    // --- Agent Running History ---
    pub async fn get_agent_history(
        &self,
        agent_id: &str,
    ) -> Result<Vec<AgentHistoryItem>, sqlx::Error> {
        sqlx::query_as::<_, AgentHistoryItem>(
            // created_at has one-second granularity, so many rows of a single
            // turn share it; `rowid` (insertion order) is the real tie-breaker.
            // Without it a tool-call group can come back shuffled, which turns
            // a valid transcript into orphaned tool messages.
            "SELECT * FROM agent_history WHERE agent_id = ? ORDER BY created_at ASC, rowid ASC",
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn append_agent_history(
        &self,
        agent_id: &str,
        role: &str,
        content: &str,
        tool_calls_json: Option<&str>,
        tool_call_id: Option<&str>,
        source_template: Option<&str>,
    ) -> Result<AgentHistoryItem, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"
            INSERT INTO agent_history (id, agent_id, role, content, tool_calls_json, tool_call_id, source_template)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&id)
        .bind(agent_id)
        .bind(role)
        .bind(content)
        .bind(tool_calls_json)
        .bind(tool_call_id)
        .bind(source_template)
        .execute(&self.pool)
        .await?;

        sqlx::query_as::<_, AgentHistoryItem>("SELECT * FROM agent_history WHERE id = ?")
            .bind(&id)
            .fetch_one(&self.pool)
            .await
    }

    /// Parses `tool_calls_json` of an assistant history item and returns the
    /// set of tool call ids that still need a `tool` response. Returns `None`
    /// when the item is not an assistant message or carries no tool calls.
    fn pending_tool_call_ids(item: &AgentHistoryItem) -> Option<HashSet<String>> {
        if item.role != "assistant" {
            return None;
        }
        let raw = item.tool_calls_json.as_deref()?;
        let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
        let calls = parsed.as_array()?;
        if calls.is_empty() {
            return None;
        }
        let ids: HashSet<String> = calls
            .iter()
            .filter_map(|c| c.get("id").and_then(|i| i.as_str()).map(|s| s.to_string()))
            .collect();
        if ids.is_empty() {
            None
        } else {
            Some(ids)
        }
    }

    /// Compacts the *working context*, reinstalling a freshly rendered
    /// system instruction.
    ///
    /// Semantics:
    ///   * `system_instruction` is the **new** prompt (already interpolated by
    ///     the caller from the agent's current `system_prompt`, memory and tool
    ///     list). Compaction is the designated context-rebuild point, so this is
    ///     where a changed instruction preset / memory actually takes effect;
    ///   * the previous working rows are dropped, then the new instruction is
    ///     written as row 0 — exactly once, at the very start;
    ///   * the `compact` tool call is answered **immediately after its
    ///     assistant message**, so the provider never sees a dangling call;
    ///   * the compaction summary is appended afterwards as a `system` row;
    ///   * the full sequence remains in `agent_transcript` (append-only).
    ///
    /// Row order written by this call:
    ///   `[system instruction] [assistant(compact...)] [tool replies...] [summary]`
    pub async fn compact_agent_history(
        &self,
        agent_id: &str,
        system_instruction: &str,
        summary: &str,
        assistant_msg: Option<&serde_json::Value>,
        tool_replies: &[(String, String)],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        // 1. Drop only the *working context* rows; the transcript is untouched.
        sqlx::query("DELETE FROM agent_history WHERE agent_id = ?")
            .bind(agent_id)
            .execute(&mut *tx)
            .await?;

        // 2. Install the freshly rendered instruction as the new row 0.
        {
            sqlx::query(
                "INSERT INTO agent_history (id, agent_id, role, content) VALUES (?, ?, 'system', ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(agent_id)
            .bind(system_instruction)
            .execute(&mut *tx)
            .await?;
        }

        // 3. The compact tool-call group, BEFORE the summary, so the reply is
        //    adjacent to its assistant message.
        if let Some(assistant) = assistant_msg {
            let tool_calls_json = assistant.get("tool_calls").map(|tc| tc.to_string());
            let content = assistant
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("");
            sqlx::query(
                "INSERT INTO agent_history (id, agent_id, role, content, tool_calls_json) VALUES (?, ?, 'assistant', ?, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(agent_id)
            .bind(content)
            .bind(tool_calls_json)
            .execute(&mut *tx)
            .await?;

            for (call_id, content) in tool_replies {
                sqlx::query(
                    "INSERT INTO agent_history (id, agent_id, role, content, tool_call_id) VALUES (?, ?, 'tool', ?, ?)",
                )
                .bind(Uuid::new_v4().to_string())
                .bind(agent_id)
                .bind(content)
                .bind(call_id)
                .execute(&mut *tx)
                .await?;
            }
        }

        // 4. Append the summary as its own system row (never merged into the
        //    base prompt, so the original instructions appear exactly once).
        let summary_row = format!("### [Compaction Summary]\n{}", summary);
        sqlx::query(
            "INSERT INTO agent_history (id, agent_id, role, content) VALUES (?, ?, 'system', ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(agent_id)
        .bind(&summary_row)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Rewrites the whole running history for an agent in a single
    /// transaction, in the exact order given (used to normalise/repair
    /// tool-call groups). Insertion order is preserved so `rowid` ordering in
    /// `get_agent_history` stays deterministic.
    pub async fn replace_agent_history(
        &self,
        agent_id: &str,
        messages: &[serde_json::Value],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM agent_history WHERE agent_id = ?")
            .bind(agent_id)
            .execute(&mut *tx)
            .await?;

        for msg in messages {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("system");
            let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
            let tool_calls_json = msg.get("tool_calls").map(|tc| tc.to_string());
            let tool_call_id = msg.get("tool_call_id").and_then(|t| t.as_str());

            sqlx::query(
                r#"
                INSERT INTO agent_history (id, agent_id, role, content, tool_calls_json, tool_call_id)
                VALUES (?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(Uuid::new_v4().to_string())
            .bind(agent_id)
            .bind(role)
            .bind(content)
            .bind(tool_calls_json)
            .bind(tool_call_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Replace the text of the conversation's first `system` row (the base
    /// instruction) in place, leaving every other row untouched.
    ///
    /// Used when the agent's `system_prompt` changes (e.g. a new Agent
    /// Instructions preset) on an existing conversation: the instruction row is
    /// refreshed rather than the whole transcript being rebuilt, so history and
    /// tool-call ordering are preserved.
    ///
    /// Returns `true` when a row was updated.
    pub async fn replace_base_system_prompt(
        &self,
        agent_id: &str,
        content: &str,
        source_template: &str,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            r#"
            UPDATE agent_history
            SET content = ?, source_template = ?
            WHERE id = (
                SELECT id FROM agent_history
                WHERE agent_id = ? AND role = 'system'
                ORDER BY created_at ASC, rowid ASC
                LIMIT 1
            )
            "#,
        )
        .bind(content)
        .bind(source_template)
        .bind(agent_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn clear_agent_history(&self, agent_id: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM agent_history WHERE agent_id = ?")
            .bind(agent_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn prune_agent_history_to_limit(
        &self,
        agent_id: &str,
        max_chars: usize,
    ) -> Result<usize, sqlx::Error> {
        let history = self.get_agent_history(agent_id).await?;
        if history.is_empty() {
            return Ok(0);
        }

        let mut total_chars: usize = history
            .iter()
            .map(|h| {
                h.content.chars().count()
                    + h.tool_calls_json
                        .as_ref()
                        .map(|s| s.chars().count())
                        .unwrap_or(0)
            })
            .sum();

        if total_chars <= max_chars {
            return Ok(0);
        }

        // We preserve index 0 if it is the base 'system' prompt
        let start_idx = if history.first().map(|h| h.role.as_str()) == Some("system") {
            1
        } else {
            0
        };

        let mut deleted_count = 0;
        let mut ids_to_delete = Vec::new();

        // Prune in whole *atomic groups* so that the result is always a valid
        // chat transcript for OpenAI/Gemini:
        //   * an `assistant` message carrying `tool_calls` and every `tool`
        //     message answering those ids form one indivisible group;
        //   * a standalone `tool` message (its assistant was already gone) is
        //     an orphan and is removed independently.
        // Deleting only the assistant row (or only part of a group) leaves
        // orphaned tool messages behind, which the provider rejects.
        let mut idx = start_idx;
        while idx < history.len() && total_chars > max_chars {
            let item = &history[idx];
            let item_chars = item.content.chars().count()
                + item
                    .tool_calls_json
                    .as_ref()
                    .map(|s| s.chars().count())
                    .unwrap_or(0);

            // Collect the ids of an assistant tool-call group (assistant row +
            // every following tool row answering one of its tool_calls).
            let mut group_ids: Vec<&str> = Vec::new();
            let mut group_chars = item_chars;

            if item.role == "assistant" {
                if let Some(mut pending) = Self::pending_tool_call_ids(item) {
                    group_ids.push(item.id.as_str());
                    let mut j = idx + 1;
                    while j < history.len() && !pending.is_empty() {
                        let follower = &history[j];
                        if follower.role != "tool" {
                            break;
                        }
                        if let Some(fid) = follower.tool_call_id.as_deref() {
                            if pending.remove(fid) {
                                group_ids.push(follower.id.as_str());
                                group_chars += follower.content.chars().count()
                                    + follower
                                        .tool_calls_json
                                        .as_ref()
                                        .map(|s| s.chars().count())
                                        .unwrap_or(0);
                            }
                        }
                        j += 1;
                    }
                }
            }

            if group_ids.is_empty() {
                group_ids.push(item.id.as_str());
            }

            let group_len = group_ids.len();
            for id in group_ids {
                ids_to_delete.push(id.to_string());
                deleted_count += 1;
            }
            total_chars = total_chars.saturating_sub(group_chars);
            idx += group_len;
        }

        for id in ids_to_delete {
            let _ = sqlx::query("DELETE FROM agent_history WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
                .await?;
        }

        Ok(deleted_count)
    }

    // --- Append-only Agent Transcript (distillation source of truth) ---

    /// Appends one immutable row to the agent's lifetime transcript.
    /// `seq` is allocated inside the transaction so concurrent appenders
    /// cannot collide or reorder.
    pub async fn append_transcript(
        &self,
        agent_id: &str,
        role: &str,
        content: &str,
        tool_calls_json: Option<&str>,
        tool_call_id: Option<&str>,
        origin: &str,
        turn: i64,
        metadata_json: Option<&str>,
    ) -> Result<AgentTranscriptItem, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        let next_seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM agent_transcript WHERE agent_id = ?",
        )
        .bind(agent_id)
        .fetch_one(&mut *tx)
        .await?;

        let id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"
            INSERT INTO agent_transcript
                (id, agent_id, seq, role, content, tool_calls_json, tool_call_id, origin, turn, metadata_json)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&id)
        .bind(agent_id)
        .bind(next_seq)
        .bind(role)
        .bind(content)
        .bind(tool_calls_json)
        .bind(tool_call_id)
        .bind(origin)
        .bind(turn)
        .bind(metadata_json)
        .execute(&mut *tx)
        .await?;

        let item =
            sqlx::query_as::<_, AgentTranscriptItem>("SELECT * FROM agent_transcript WHERE id = ?")
                .bind(&id)
                .fetch_one(&mut *tx)
                .await?;

        tx.commit().await?;
        Ok(item)
    }

    /// Full ordered transcript, oldest first. Never mutated by pruning.
    pub async fn get_agent_transcript(
        &self,
        agent_id: &str,
    ) -> Result<Vec<AgentTranscriptItem>, sqlx::Error> {
        sqlx::query_as::<_, AgentTranscriptItem>(
            "SELECT * FROM agent_transcript WHERE agent_id = ? ORDER BY seq ASC",
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Export helper: the transcript as OpenAI-style chat messages, suitable
    /// for distillation / fine-tuning datasets. System prompts appear exactly
    /// once each, in their original position.
    pub async fn export_agent_transcript(
        &self,
        agent_id: &str,
    ) -> Result<Vec<serde_json::Value>, sqlx::Error> {
        let rows = self.get_agent_transcript(agent_id).await?;
        Ok(rows
            .iter()
            .map(|r| {
                let mut obj = serde_json::json!({
                    "role": r.role,
                    "content": r.content,
                });
                if let Some(tc) = r.tool_calls_json.as_deref() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(tc) {
                        obj["tool_calls"] = v;
                    }
                }
                if let Some(tid) = r.tool_call_id.as_deref() {
                    obj["tool_call_id"] = serde_json::Value::String(tid.to_string());
                }
                obj
            })
            .collect())
    }

    /// Case-insensitive regex search over an agent's append-only transcript.
    /// Returns lightweight hits (no full content) ordered by time.
    pub async fn search_transcript(
        &self,
        agent_id: &str,
        pattern: &str,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, sqlx::Error> {
        let rows = self.get_agent_transcript(agent_id).await?;
        // A pattern that does not compile is a caller error, not "no matches".
        // Returning an empty hit list here would make a broken search
        // indistinguishable from a search that genuinely found nothing, which
        // is precisely the silent degradation this codebase forbids.
        let re = regex::RegexBuilder::new(pattern)
            .case_insensitive(true)
            .size_limit(1 << 20)
            .build()
            .map_err(|e| sqlx::Error::Protocol(format!("invalid regular expression: {e}")))?;

        let limit = limit.clamp(1, 100);
        let mut hits = Vec::new();
        for row in rows {
            // Match against the visible text plus tool call payloads so that
            // searching for a tool name or an argument finds it too.
            let haystack = match row.tool_calls_json.as_deref() {
                Some(tc) => format!("{}\n{}", row.content, tc),
                None => row.content.clone(),
            };
            let Some(m) = re.find(&haystack) else {
                continue;
            };

            let char_offset = haystack[..m.start()].chars().count();
            let preview: String = row.content.chars().skip(char_offset).take(200).collect();
            let preview = if row.tool_calls_json.is_some() && preview.is_empty() {
                haystack.chars().skip(char_offset).take(200).collect()
            } else {
                preview
            };

            let time = row.created_at.map(|t| t.timestamp()).unwrap_or_default();

            hits.push(serde_json::json!({
                "id": row.id,
                "time": time,
                "role": row.role,
                "origin": row.origin,
                "turn": row.turn,
                "preview": preview,
                "offset": char_offset,
                "length": row.content.chars().count(),
            }));

            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    }

    /// Returns `(total_chars, slice)` for a transcript row by id, where
    /// `slice` is `content[from..to]` (character offsets, clamped).
    pub async fn inspect_transcript(
        &self,
        agent_id: &str,
        id: &str,
        from: usize,
        to: usize,
    ) -> Result<Option<(usize, String)>, sqlx::Error> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT content FROM agent_transcript WHERE agent_id = ? AND id = ?")
                .bind(agent_id)
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;

        Ok(row.map(|(content,)| {
            let total = content.chars().count();
            let from = from.min(total);
            let to = to.max(from).min(total);
            let slice: String = content.chars().skip(from).take(to - from).collect();
            (total, slice)
        }))
    }

    /// Current turn counter for an agent (max turn recorded in transcript).
    pub async fn get_agent_turn_counter(&self, agent_id: &str) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COALESCE(MAX(turn), 0) FROM agent_transcript WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_one(&self.pool)
            .await
    }

    /// Atomically folds unconsumed pushed messages into running history.
    ///
    /// The history insert, the transcript record and the `is_consumed` flags
    /// are committed in a single transaction, so a crash can never leave a
    /// push both "consumed" and absent from the context (or vice versa).
    /// Returns `None` when there was nothing to inject.
    pub async fn consume_pushed_into_history(
        &self,
        agent_id: &str,
        format_fn: impl Fn(&AgentPushedMessage) -> String,
        turn: i64,
    ) -> Result<Option<AgentHistoryItem>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        let pending = sqlx::query_as::<_, AgentPushedMessage>(
            "SELECT * FROM agent_pushed_messages WHERE agent_id = ? AND is_consumed = 0 ORDER BY created_at ASC, rowid ASC",
        )
        .bind(agent_id)
        .fetch_all(&mut *tx)
        .await?;

        if pending.is_empty() {
            tx.commit().await?;
            return Ok(None);
        }

        let mut context = String::from("### [Push MCP Updates / Subscriptions]:\n");
        for msg in &pending {
            context.push_str(&format_fn(msg));
        }

        // 1. Working context row
        let item_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO agent_history (id, agent_id, role, content) VALUES (?, ?, 'system', ?)",
        )
        .bind(&item_id)
        .bind(agent_id)
        .bind(&context)
        .execute(&mut *tx)
        .await?;

        // 2. Immutable transcript row (distillation keeps the raw batch)
        let next_seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM agent_transcript WHERE agent_id = ?",
        )
        .bind(agent_id)
        .fetch_one(&mut *tx)
        .await?;

        let event_ids: Vec<String> = pending.iter().map(|m| m.id.clone()).collect();
        let metadata = serde_json::json!({ "pushed_event_ids": event_ids }).to_string();

        sqlx::query(
            r#"
            INSERT INTO agent_transcript
                (id, agent_id, seq, role, content, origin, turn, metadata_json)
            VALUES (?, ?, ?, 'system', ?, 'push', ?, ?)
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(agent_id)
        .bind(next_seq)
        .bind(&context)
        .bind(turn)
        .bind(&metadata)
        .execute(&mut *tx)
        .await?;

        // 3. Mark consumed — same transaction
        for msg in &pending {
            sqlx::query(
                "UPDATE agent_pushed_messages SET is_consumed = 1 WHERE id = ? AND agent_id = ?",
            )
            .bind(&msg.id)
            .bind(agent_id)
            .execute(&mut *tx)
            .await?;
        }

        let item =
            sqlx::query_as::<_, AgentHistoryItem>("SELECT * FROM agent_history WHERE id = ?")
                .bind(&item_id)
                .fetch_one(&mut *tx)
                .await?;

        tx.commit().await?;
        Ok(Some(item))
    }

    // --- Isolated Host-Managed Storage ---
    pub async fn list_extension_kv(
        &self,
        extension_id: &str,
    ) -> Result<Vec<ExtensionKvItem>, sqlx::Error> {
        sqlx::query_as::<_, ExtensionKvItem>(
            "SELECT extension_id, key, value, updated_at FROM extension_kv WHERE extension_id = ? ORDER BY key ASC"
        )
        .bind(extension_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_extension_kv(
        &self,
        extension_id: &str,
        key: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM extension_kv WHERE extension_id = ? AND key = ?")
                .bind(extension_id)
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|r| r.0))
    }

    pub async fn set_extension_kv(
        &self,
        extension_id: &str,
        key: &str,
        value: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO extension_kv (extension_id, key, value, updated_at)
            VALUES (?, ?, ?, CURRENT_TIMESTAMP)
            ON CONFLICT(extension_id, key) DO UPDATE SET
                value = excluded.value,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(extension_id)
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_extension_kv(
        &self,
        extension_id: &str,
        key: &str,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM extension_kv WHERE extension_id = ? AND key = ?")
            .bind(extension_id)
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_headers_or_env() {
        assert_eq!(merge_headers_or_env(None, None), None);
        assert_eq!(merge_headers_or_env(Some(""), None), None);
        assert_eq!(merge_headers_or_env(None, Some("")), None);

        // Global only
        let g = r#"{"TOKEN": "global_tok", "HOST": "https://api.example.com"}"#;
        let merged_g = merge_headers_or_env(Some(g), None).unwrap();
        let parsed: HashMap<String, String> = serde_json::from_str(&merged_g).unwrap();
        assert_eq!(parsed.get("TOKEN").unwrap(), "global_tok");
        assert_eq!(parsed.get("HOST").unwrap(), "https://api.example.com");

        // Agent override only
        let o = r#"{"TOKEN": "agent_tok"}"#;
        let merged_o = merge_headers_or_env(None, Some(o)).unwrap();
        let parsed_o: HashMap<String, String> = serde_json::from_str(&merged_o).unwrap();
        assert_eq!(parsed_o.get("TOKEN").unwrap(), "agent_tok");

        // Merge: agent override takes precedence over global key, retains non-overridden global keys
        let merged_both = merge_headers_or_env(Some(g), Some(o)).unwrap();
        let parsed_both: HashMap<String, String> = serde_json::from_str(&merged_both).unwrap();
        assert_eq!(parsed_both.get("TOKEN").unwrap(), "agent_tok");
        assert_eq!(parsed_both.get("HOST").unwrap(), "https://api.example.com");

        // Line-based format input
        let g_lines = "API_KEY=key_g\nDEBUG=0";
        let o_lines = "API_KEY=key_agent";
        let merged_lines = merge_headers_or_env(Some(g_lines), Some(o_lines)).unwrap();
        let parsed_lines: HashMap<String, String> = serde_json::from_str(&merged_lines).unwrap();
        assert_eq!(parsed_lines.get("API_KEY").unwrap(), "key_agent");
        assert_eq!(parsed_lines.get("DEBUG").unwrap(), "0");
    }
}

// -----------------------------------------------------------------------------
// Wikilink extraction
// -----------------------------------------------------------------------------

/// Longest key accepted, in characters. Keys are canonical noun phrases, not
/// sentences; a longer string is almost certainly a mistaken link.
pub const MEMORY_KEY_MAX_CHARS: usize = 100;

/// Longest node value accepted, in characters.
pub const MEMORY_VALUE_MAX_CHARS: usize = 10_000;

/// Extract unique `[[targets]]` from `text`.
///
/// Rules, each of which exists because the naive version produced garbage:
///
/// * `[[Target|alias]]` yields `Target` (the pipe alias is display-only).
/// * Links inside fenced code blocks (\`\`\`) and inline code spans are ignored:
///   documentation about wikilinks is not a wikilink.
/// * Surrounding whitespace is trimmed, runs of internal whitespace collapse to
///   one space, and the result is lowercased — so `[[Alice]]`, `[[ alice ]]` and
///   `[[Alice]]` all point at the same node.
/// * Empty targets and targets containing a newline, `[`, `]` or `|` are
///   rejected rather than stored as broken keys.
/// * Order of first appearance is preserved and duplicates are removed.
pub fn extract_wikilinks(text: &str) -> Vec<String> {
    let stripped = strip_code(text);
    let mut out: Vec<String> = Vec::new();
    let mut rest = stripped.as_str();

    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else {
            break;
        };
        let inner = &after[..end];
        rest = &after[end + 2..];

        let target = inner.split('|').next().unwrap_or("");
        if let Some(key) = normalize_memory_key(target) {
            if !out.contains(&key) {
                out.push(key);
            }
        }
    }
    out
}

/// Normalize a candidate node key, or `None` when it cannot be a key.
pub fn normalize_memory_key(raw: &str) -> Option<String> {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MEMORY_KEY_MAX_CHARS {
        return None;
    }
    if trimmed.contains(['[', ']', '|', '\n', '\r', '\t']) {
        return None;
    }
    Some(trimmed.to_lowercase())
}

/// Remove fenced code blocks and inline code spans so their contents are not
/// scanned for links.
fn strip_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    strip_inline_code(&out)
}

/// Drop `` `...` `` spans, leaving the rest untouched.
fn strip_inline_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_code = false;
    for ch in text.chars() {
        if ch == '`' {
            in_code = !in_code;
            continue;
        }
        if !in_code {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod wikilink_tests {
    use super::*;

    #[test]
    fn extracts_simple_links_in_order_without_duplicates() {
        assert_eq!(
            extract_wikilinks("Uses [[Postgres]] and [[Redis]], also [[Postgres]]."),
            vec!["postgres".to_string(), "redis".to_string()]
        );
    }

    #[test]
    fn pipe_alias_links_to_the_target_not_the_alias() {
        assert_eq!(
            extract_wikilinks("See [[auth service|the auth thing]]."),
            vec!["auth service".to_string()]
        );
    }

    #[test]
    fn ignores_code_blocks_and_inline_code() {
        let text = "Real [[Link]].\n```\n[[NotALink]]\n```\nInline `[[AlsoNot]]` done.";
        assert_eq!(extract_wikilinks(text), vec!["link".to_string()]);
    }

    #[test]
    fn normalizes_whitespace_and_case() {
        assert_eq!(
            extract_wikilinks("[[  Alice   Smith  ]] [[ALICE SMITH]]"),
            vec!["alice smith".to_string()]
        );
    }

    #[test]
    fn rejects_junk_targets() {
        assert!(extract_wikilinks("[[]]").is_empty());
        assert!(extract_wikilinks("[[   ]]").is_empty());
        assert!(extract_wikilinks("[[a[[b]]").is_empty());
        // Unterminated link is not a link.
        assert!(extract_wikilinks("[[unclosed").is_empty());
        // Too long to be a key.
        let long = "x".repeat(MEMORY_KEY_MAX_CHARS + 1);
        assert!(extract_wikilinks(&format!("[[{long}]]")).is_empty());
    }

    #[test]
    fn normalize_memory_key_trims_and_lowercases() {
        assert_eq!(
            normalize_memory_key("  Postgres   Primary  ").as_deref(),
            Some("postgres primary")
        );
        assert_eq!(normalize_memory_key("   "), None);
        assert_eq!(normalize_memory_key("has|pipe"), None);
    }
}
