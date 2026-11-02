use sqlx::SqlitePool;
use std::collections::HashSet;
use uuid::Uuid;

use super::schema::*;

/// Normalizes an optional avatar URL: whitespace-only values mean "no picture".
fn normalize_avatar_url(avatar_url: Option<&str>) -> Option<&str> {
    match avatar_url {
        Some(url) if url.trim().is_empty() => None,
        other => other,
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
        sqlx::query(
            "INSERT INTO accounts (id, username, password_hash, role) VALUES (?, ?, ?, ?)",
        )
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
                "SELECT datetime(created_at) FROM accounts WHERE id = ?"
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

            let acc = self.get_account_by_id(&id)
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

        self.get_provider(id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn delete_provider(&self, id: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM providers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // --- Provider Models ---
    pub async fn list_provider_models(&self, provider_id: &str) -> Result<Vec<ProviderModel>, sqlx::Error> {
        sqlx::query_as::<_, ProviderModel>(
            "SELECT * FROM provider_models WHERE provider_id = ? ORDER BY model_id ASC",
        )
        .bind(provider_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn add_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
        name: Option<&str>,
        config_json: Option<&str>,
    ) -> Result<ProviderModel, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"
            INSERT INTO provider_models (id, provider_id, model_id, name, config_json, is_active)
            VALUES (?, ?, ?, ?, ?, 1)
            "#,
        )
        .bind(&id)
        .bind(provider_id)
        .bind(model_id)
        .bind(name)
        .bind(config_json)
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
            sqlx::query(
                r#"
                INSERT INTO provider_models (id, provider_id, model_id, name, config_json, is_active)
                VALUES (?, ?, ?, ?, ?, 1)
                "#,
            )
            .bind(&id)
            .bind(provider_id)
            .bind(&m.model_id)
            .bind(m.name.as_deref())
            .bind(m.config_json.as_deref())
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
        let final_max_history = max_running_history_chars.unwrap_or(current.max_running_history_chars);
        let final_hooks_json = response_hooks_json.or(current.response_hooks_json.as_deref()).unwrap_or("[]");
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
    pub async fn get_agent_models(&self, agent_id: &str) -> Result<Vec<ActiveModelTarget>, sqlx::Error> {
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
                pm.config_json as config_json
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

    pub async fn get_agent_provider_model_ids(&self, agent_id: &str) -> Result<Vec<String>, sqlx::Error> {
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
        let _ = self.sync_wasm_mcp_servers(account_id).await;
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
        Ok(res.rows_affected() > 0)
    }

    // --- Agent MCP Bindings ---
    pub async fn get_agent_mcp_servers(
        &self,
        agent_id: &str,
    ) -> Result<Vec<McpServer>, sqlx::Error> {
        sqlx::query_as::<_, McpServer>(
            r#"
            SELECT m.* FROM mcp_servers m
            JOIN agent_mcp am ON am.mcp_id = m.id
            WHERE am.agent_id = ?
            ORDER BY m.name ASC
            "#,
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn set_agent_mcp_servers(
        &self,
        agent_id: &str,
        mcp_ids: &[String],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM agent_mcp WHERE agent_id = ?")
            .bind(agent_id)
            .execute(&mut *tx)
            .await?;

        for mid in mcp_ids {
            sqlx::query("INSERT INTO agent_mcp (agent_id, mcp_id) VALUES (?, ?)")
                .bind(agent_id)
                .bind(mid)
                .execute(&mut *tx)
                .await?;
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

        sqlx::query_as::<_, AgentPushedMessage>(
            "SELECT * FROM agent_pushed_messages WHERE id = ?",
        )
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
            sqlx::query("UPDATE agent_pushed_messages SET is_consumed = 1 WHERE id = ? AND agent_id = ?")
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

    // --- Synchronize WASM MCP Servers ---
    pub async fn sync_wasm_mcp_servers(&self, account_id: &str) -> Result<(), sqlx::Error> {
        let active_exts = self.list_active_extensions(account_id).await?;
        let mut active_wasm_mcp_ids = std::collections::HashSet::new();

        for ext in active_exts {
            if let Ok(manifest_val) = serde_json::from_str::<serde_json::Value>(&ext.manifest_json) {
                if let Some(mcp) = manifest_val.get("mcp") {
                    let mcp_id = format!("wasm:{}", ext.id);
                    active_wasm_mcp_ids.insert(mcp_id.clone());
                    let server_name = mcp.get("server_name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("[WASM] {}", ext.name));
                    let server_alias = mcp.get("alias")
                        .or_else(|| mcp.get("server_alias"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| ext.name.clone());
                    let transport = "wasm_stdio";
                    let url_or_command = format!("wasm://{}", ext.id);

                    sqlx::query(
                        r#"
                        INSERT INTO mcp_servers (id, account_id, name, alias, transport, url_or_command)
                        VALUES (?, ?, ?, ?, ?, ?)
                        ON CONFLICT (id) DO UPDATE SET
                            name = excluded.name,
                            alias = excluded.alias,
                            transport = excluded.transport,
                            url_or_command = excluded.url_or_command
                        "#,
                    )
                    .bind(&mcp_id)
                    .bind(account_id)
                    .bind(&server_name)
                    .bind(&server_alias)
                    .bind(transport)
                    .bind(&url_or_command)
                    .execute(&self.pool)
                    .await?;
                }
            }
        }

        // Clean up deactivated WASM MCP servers
        let existing = sqlx::query_as::<_, McpServer>("SELECT * FROM mcp_servers WHERE account_id = ?")
            .bind(account_id)
            .fetch_all(&self.pool)
            .await?;
        for s in existing {
            if s.transport == "wasm_stdio" && !active_wasm_mcp_ids.contains(&s.id) {
                let _ = self.delete_mcp_server(&s.id).await;
            }
        }

        Ok(())
    }

    // --- Agent Memory ---
    pub async fn get_agent_memory(&self, agent_id: &str) -> Result<AgentMemory, sqlx::Error> {
        if let Some(mem) = sqlx::query_as::<_, AgentMemory>(
            "SELECT * FROM agent_memories WHERE agent_id = ?",
        )
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

            sqlx::query_as::<_, AgentMemory>(
                "SELECT * FROM agent_memories WHERE agent_id = ?",
            )
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

        let affected = sqlx::query(
            "DELETE FROM agent_instruction_presets WHERE id = ? AND account_id = ?",
        )
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
    ) -> Result<AgentHistoryItem, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"
            INSERT INTO agent_history (id, agent_id, role, content, tool_calls_json, tool_call_id)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&id)
        .bind(agent_id)
        .bind(role)
        .bind(content)
        .bind(tool_calls_json)
        .bind(tool_call_id)
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

        let item = sqlx::query_as::<_, AgentTranscriptItem>(
            "SELECT * FROM agent_transcript WHERE id = ?",
        )
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
        let re = match regex::RegexBuilder::new(pattern)
            .case_insensitive(true)
            .size_limit(1 << 20)
            .build()
        {
            Ok(re) => re,
            Err(_) => return Ok(Vec::new()),
        };

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

            let time = row
                .created_at
                .map(|t| t.timestamp())
                .unwrap_or_default();

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
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT content FROM agent_transcript WHERE agent_id = ? AND id = ?",
        )
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
        sqlx::query_scalar(
            "SELECT COALESCE(MAX(turn), 0) FROM agent_transcript WHERE agent_id = ?",
        )
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

    // --- Extensions & Isolated Host-Managed Storage ---
    pub async fn list_extensions(
        &self,
        account_id: &str,
    ) -> Result<Vec<ExtensionSummary>, sqlx::Error> {
        sqlx::query_as::<_, ExtensionSummary>(
            "SELECT id, account_id, name, version, description, wasm_hash, manifest_json, is_active, storage_bytes, created_at FROM extensions WHERE account_id = ? ORDER BY name ASC"
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn list_active_extensions(
        &self,
        account_id: &str,
    ) -> Result<Vec<Extension>, sqlx::Error> {
        sqlx::query_as::<_, Extension>(
            "SELECT * FROM extensions WHERE account_id = ? AND is_active = 1 ORDER BY name ASC"
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_extension(&self, id: &str) -> Result<Option<Extension>, sqlx::Error> {
        sqlx::query_as::<_, Extension>("SELECT * FROM extensions WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn get_extension_by_hash(
        &self,
        wasm_hash: &str,
    ) -> Result<Option<Extension>, sqlx::Error> {
        sqlx::query_as::<_, Extension>("SELECT * FROM extensions WHERE wasm_hash = ?")
            .bind(wasm_hash)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn upsert_extension(&self, ext: &Extension) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO extensions (id, account_id, name, version, description, wasm_hash, wasm_binary, manifest_json, is_active, storage_bytes)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                version = excluded.version,
                description = excluded.description,
                wasm_hash = excluded.wasm_hash,
                wasm_binary = excluded.wasm_binary,
                manifest_json = excluded.manifest_json,
                is_active = excluded.is_active,
                storage_bytes = excluded.storage_bytes
            "#
        )
        .bind(&ext.id)
        .bind(&ext.account_id)
        .bind(&ext.name)
        .bind(&ext.version)
        .bind(&ext.description)
        .bind(&ext.wasm_hash)
        .bind(&ext.wasm_binary)
        .bind(&ext.manifest_json)
        .bind(ext.is_active)
        .bind(ext.storage_bytes)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn set_extension_active(
        &self,
        id: &str,
        is_active: bool,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("UPDATE extensions SET is_active = ? WHERE id = ?")
            .bind(is_active)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn update_extension_storage(
        &self,
        id: &str,
        storage_bytes: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE extensions SET storage_bytes = ? WHERE id = ?")
            .bind(storage_bytes)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_extension(&self, id: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM extensions WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

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
        let row: Option<(String,)> = sqlx::query_as("SELECT value FROM extension_kv WHERE extension_id = ? AND key = ?")
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
            "#
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
