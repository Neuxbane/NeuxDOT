CREATE TABLE IF NOT EXISTS accounts (
    id TEXT PRIMARY KEY,
    username TEXT UNIQUE NOT NULL,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'member',
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS profiles (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    display_name TEXT,
    avatar_color TEXT,
    avatar_shape TEXT,
    avatar_url TEXT
);

CREATE TABLE IF NOT EXISTS providers (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    base_url TEXT NOT NULL,
    api_key TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS provider_models (
    id TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    name TEXT,
    config_json TEXT,
    params_json TEXT,
    context_window INTEGER,
    is_active BOOLEAN DEFAULT 1,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Shares an agent with another account. Ownership is recorded by
-- agents.account_id and never changes. A sharee can use the bot and may
-- optionally edit it or re-share it, but can never delete it or remove the
-- owner.
CREATE TABLE IF NOT EXISTS agent_shares (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    can_edit BOOLEAN NOT NULL DEFAULT 0,
    can_manage_shares BOOLEAN NOT NULL DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (agent_id, account_id)
);

CREATE INDEX IF NOT EXISTS idx_agent_shares_account ON agent_shares(account_id);

-- Per-account reusable system instructions ("Agent Instructions" presets).
-- Strictly owner-scoped: every query filters by account_id.
CREATE TABLE IF NOT EXISTS agent_instruction_presets (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_instruction_presets_account
    ON agent_instruction_presets(account_id, created_at);

CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    avatar_color TEXT,
    avatar_shape TEXT,
    avatar_url TEXT,
    strategy TEXT NOT NULL DEFAULT 'fallback',
    system_prompt TEXT,
    is_active BOOLEAN DEFAULT 0,
    allow_output_text BOOLEAN DEFAULT 0,
    output_text_error_message TEXT DEFAULT 'Output Text is not allowed, consider use correct tool or sleep.',
    max_running_history_chars INTEGER DEFAULT 300000,
    response_hooks_json TEXT DEFAULT '[]',
    -- Selected Agent Instructions preset (NULL = Custom / free-text prompt)
    instruction_preset_id TEXT REFERENCES agent_instruction_presets(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS agent_providers (
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    provider_model_id TEXT NOT NULL REFERENCES provider_models(id) ON DELETE CASCADE,
    priority INTEGER DEFAULT 0,
    PRIMARY KEY (agent_id, provider_model_id)
);

CREATE TABLE IF NOT EXISTS mcp_servers (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    alias TEXT,
    transport TEXT NOT NULL,
    url_or_command TEXT NOT NULL,
    headers_or_env TEXT,
    version TEXT NOT NULL DEFAULT '1.0.0',
    description TEXT DEFAULT '',
    wasm_hash TEXT,
    wasm_binary BLOB,
    manifest_json TEXT DEFAULT '{}',
    permissions_json TEXT DEFAULT '{}',
    is_active BOOLEAN DEFAULT 1,
    storage_bytes INTEGER DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS agent_mcp (
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    mcp_id TEXT NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
    config_override TEXT DEFAULT NULL,
    PRIMARY KEY (agent_id, mcp_id)
);

CREATE TABLE IF NOT EXISTS agent_pushed_messages (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    mcp_id TEXT REFERENCES mcp_servers(id) ON DELETE SET NULL,
    method TEXT NOT NULL,
    params_json TEXT,
    is_consumed BOOLEAN NOT NULL DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS agent_tool_permissions (
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    mcp_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    is_enabled BOOLEAN NOT NULL DEFAULT 1,
    PRIMARY KEY (agent_id, mcp_id, tool_name)
);

CREATE TABLE IF NOT EXISTS agent_subscriptions (
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    uri TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (agent_id, uri)
);

CREATE TABLE IF NOT EXISTS agent_memories (
    agent_id TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    content TEXT NOT NULL DEFAULT '',
    hash TEXT NOT NULL DEFAULT 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Obsidian-style keyed memory: one row per node, linked with [[wikilinks]].
-- The legacy `agent_memories` blob is kept for compatibility. Nodes are the
-- primary store and are rendered into the prompt as a manifest.
CREATE TABLE IF NOT EXISTS agent_memory_nodes (
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value TEXT NOT NULL DEFAULT '',
    type TEXT NOT NULL DEFAULT 'concept',
    description TEXT NOT NULL DEFAULT '',
    hash TEXT NOT NULL DEFAULT '',
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (agent_id, key)
);

-- Directed graph edges extracted from [[Key]] references inside node values.
CREATE TABLE IF NOT EXISTS agent_memory_links (
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    src TEXT NOT NULL,
    dst TEXT NOT NULL,
    PRIMARY KEY (agent_id, src, dst)
);

CREATE INDEX IF NOT EXISTS idx_memory_links_dst
    ON agent_memory_links (agent_id, dst);

CREATE TABLE IF NOT EXISTS agent_history (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '',
    tool_calls_json TEXT,
    tool_call_id TEXT,
    -- For the base `system` row: the raw instruction template that produced
    -- `content`. Lets the runtime tell "the user changed the system prompt /
    -- instruction preset" apart from "memory or tools changed", and refresh
    -- only the latter case without rebuilding the transcript.
    source_template TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Append-only, immutable record of everything an agent ever did or saw.
-- Never pruned and never compacted: this is the source of truth for long-horizon
-- training / distillation exports. `agent_history` is only the *working context*.
CREATE TABLE IF NOT EXISTS agent_transcript (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '',
    tool_calls_json TEXT,
    tool_call_id TEXT,
    -- provenance: "turn" | "push" | "compaction" | "tool" | "repair"
    origin TEXT NOT NULL DEFAULT 'turn',
    -- turn number the row belongs to (monotonic per agent), for distillation
    turn INTEGER NOT NULL DEFAULT 0,
    metadata_json TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_agent_transcript_agent_seq
    ON agent_transcript(agent_id, seq);

CREATE TABLE IF NOT EXISTS extensions (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    version TEXT NOT NULL DEFAULT '1.0.0',
    description TEXT DEFAULT '',
    wasm_hash TEXT NOT NULL,
    wasm_binary BLOB NOT NULL,
    manifest_json TEXT NOT NULL,
    is_active BOOLEAN DEFAULT 1,
    storage_bytes INTEGER DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS extension_kv (
    extension_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (extension_id, key)
);
