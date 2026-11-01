use sqlx::SqlitePool;
use uuid::Uuid;

use super::schema::*;

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
    pub async fn create_account(
        &self,
        username: &str,
        password_hash: &str,
    ) -> Result<Account, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO accounts (id, username, password_hash) VALUES (?, ?, ?)"
        )
        .bind(&id)
        .bind(username)
        .bind(password_hash)
        .execute(&self.pool)
        .await?;

        // Also create a default profile
        let _ = sqlx::query(
            "INSERT INTO profiles (account_id, display_name, avatar_color, avatar_shape) VALUES (?, ?, ?, ?)"
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
                "INSERT INTO accounts (id, username, password_hash, created_at) VALUES (?, 'root', ?, ?)"
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

    pub async fn update_profile(
        &self,
        account_id: &str,
        display_name: Option<&str>,
        avatar_color: Option<&str>,
        avatar_shape: Option<&str>,
    ) -> Result<Profile, sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO profiles (account_id, display_name, avatar_color, avatar_shape)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(account_id) DO UPDATE SET
                display_name = coalesce(excluded.display_name, profiles.display_name),
                avatar_color = coalesce(excluded.avatar_color, profiles.avatar_color),
                avatar_shape = coalesce(excluded.avatar_shape, profiles.avatar_shape)
            "#,
        )
        .bind(account_id)
        .bind(display_name)
        .bind(avatar_color)
        .bind(avatar_shape)
        .execute(&self.pool)
        .await?;

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
        strategy: &str,
        system_prompt: Option<&str>,
        allow_output_text: Option<bool>,
        output_text_error_message: Option<&str>,
        max_running_history_chars: Option<i64>,
        response_hooks_json: Option<&str>,
    ) -> Result<Agent, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"
            INSERT INTO agents (id, account_id, name, avatar_color, avatar_shape, strategy, system_prompt, is_active, allow_output_text, output_text_error_message, max_running_history_chars, response_hooks_json)
            VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?)
            "#,
        )
        .bind(&id)
        .bind(account_id)
        .bind(name)
        .bind(avatar_color.unwrap_or("#6366f1"))
        .bind(avatar_shape.unwrap_or("circle"))
        .bind(strategy)
        .bind(system_prompt)
        .bind(allow_output_text.unwrap_or(false))
        .bind(output_text_error_message.unwrap_or("Output Text is not allowed, consider use correct tool or sleep."))
        .bind(max_running_history_chars.unwrap_or(300_000))
        .bind(response_hooks_json.unwrap_or("[]"))
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
        strategy: Option<&str>,
        system_prompt: Option<&str>,
        is_active: Option<bool>,
        allow_output_text: Option<bool>,
        output_text_error_message: Option<&str>,
        max_running_history_chars: Option<i64>,
        response_hooks_json: Option<&str>,
    ) -> Result<Agent, sqlx::Error> {
        let current = self.get_agent(id).await?.ok_or(sqlx::Error::RowNotFound)?;

        let final_name = name.unwrap_or(&current.name);
        let final_avatar_color = avatar_color.or(current.avatar_color.as_deref());
        let final_avatar_shape = avatar_shape.or(current.avatar_shape.as_deref());
        let final_strategy = strategy.unwrap_or(&current.strategy);
        let final_system_prompt = system_prompt.or(current.system_prompt.as_deref());
        let final_is_active = is_active.unwrap_or(current.is_active);
        let final_allow_output_text = allow_output_text.unwrap_or(current.allow_output_text);
        let final_output_text_error_message =
            output_text_error_message.or(current.output_text_error_message.as_deref());
        let final_max_history = max_running_history_chars.unwrap_or(current.max_running_history_chars);
        let final_hooks_json = response_hooks_json.or(current.response_hooks_json.as_deref()).unwrap_or("[]");

        sqlx::query(
            r#"
            UPDATE agents
            SET name = ?, avatar_color = ?, avatar_shape = ?, strategy = ?, system_prompt = ?, is_active = ?, allow_output_text = ?, output_text_error_message = ?, max_running_history_chars = ?, response_hooks_json = ?
            WHERE id = ?
            "#,
        )
        .bind(final_name)
        .bind(final_avatar_color)
        .bind(final_avatar_shape)
        .bind(final_strategy)
        .bind(final_system_prompt)
        .bind(final_is_active)
        .bind(final_allow_output_text)
        .bind(final_output_text_error_message)
        .bind(final_max_history)
        .bind(final_hooks_json)
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
        transport: &str,
        url_or_command: &str,
    ) -> Result<McpServer, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO mcp_servers (id, account_id, name, transport, url_or_command) VALUES (?, ?, ?, ?, ?)"
        )
        .bind(&id)
        .bind(account_id)
        .bind(name)
        .bind(transport)
        .bind(url_or_command)
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
        transport: &str,
        url_or_command: &str,
    ) -> Result<McpServer, sqlx::Error> {
        sqlx::query(
            "UPDATE mcp_servers SET name = ?, transport = ?, url_or_command = ? WHERE id = ?",
        )
        .bind(name)
        .bind(transport)
        .bind(url_or_command)
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
                    let transport = "wasm_stdio";
                    let url_or_command = format!("wasm://{}", ext.id);

                    sqlx::query(
                        r#"
                        INSERT INTO mcp_servers (id, account_id, name, transport, url_or_command)
                        VALUES (?, ?, ?, ?, ?)
                        ON CONFLICT (id) DO UPDATE SET
                            name = excluded.name,
                            transport = excluded.transport,
                            url_or_command = excluded.url_or_command
                        "#,
                    )
                    .bind(&mcp_id)
                    .bind(account_id)
                    .bind(&server_name)
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

    // --- Agent Running History ---
    pub async fn get_agent_history(
        &self,
        agent_id: &str,
    ) -> Result<Vec<AgentHistoryItem>, sqlx::Error> {
        sqlx::query_as::<_, AgentHistoryItem>(
            "SELECT * FROM agent_history WHERE agent_id = ? ORDER BY created_at ASC",
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

    pub async fn compact_agent_history(
        &self,
        agent_id: &str,
        compacted_system_prompt: &str,
        tool_call_id: Option<&str>,
        tool_content: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        // 1. Delete all previous history
        sqlx::query("DELETE FROM agent_history WHERE agent_id = ?")
            .bind(agent_id)
            .execute(&mut *tx)
            .await?;

        // 2. Insert new compacted system prompt
        let sys_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO agent_history (id, agent_id, role, content) VALUES (?, ?, 'system', ?)",
        )
        .bind(&sys_id)
        .bind(agent_id)
        .bind(compacted_system_prompt)
        .execute(&mut *tx)
        .await?;

        // 3. Optional tool response message acknowledgment
        if let (Some(t_id), Some(t_content)) = (tool_call_id, tool_content) {
            let tool_msg_id = Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO agent_history (id, agent_id, role, content, tool_call_id) VALUES (?, ?, 'tool', ?, ?)",
            )
            .bind(&tool_msg_id)
            .bind(agent_id)
            .bind(t_content)
            .bind(t_id)
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

        for item in history.iter().skip(start_idx) {
            if total_chars <= max_chars {
                break;
            }
            let item_chars = item.content.chars().count()
                + item
                    .tool_calls_json
                    .as_ref()
                    .map(|s| s.chars().count())
                    .unwrap_or(0);
            total_chars = total_chars.saturating_sub(item_chars);
            ids_to_delete.push(item.id.clone());
            deleted_count += 1;
        }

        for id in ids_to_delete {
            let _ = sqlx::query("DELETE FROM agent_history WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
                .await?;
        }

        Ok(deleted_count)
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
