# discord-mcp

A Discord [Model Context Protocol](https://modelcontextprotocol.io) server in
Rust. It exposes the Discord REST API as MCP tools, ships a per-agent settings
app, and streams incoming messages as MCP push notifications over the Discord
Gateway WebSocket.

## Design

Tools are generated from one table in `src/tools.rs`, so the published schema and
the call that is actually made cannot drift apart. 73 tools reach the model: 70
table rows plus the three that are not a single REST call
(`discord_get_messages`, `discord_send_dm`, `discord_api`). Three further
settings tools are published with `_meta.ui.visibility: ["app"]`, which the host
hides from the model (see `src/mcp/client.rs::is_app_only`), so the settings
surface is reachable only from `ui://discord-config`.

| Area | Tools |
| --- | --- |
| Bot, servers | `discord_get_me`, `discord_list_guilds`, `discord_get_guild`, `discord_modify_guild`, `discord_get_guild_preview`, `discord_get_guild_vanity_url`, `discord_get_audit_log` |
| Channels | `discord_list_guild_channels`, `discord_get_channel`, `discord_create_guild_channel`, `discord_modify_channel`, `discord_delete_channel`, `discord_set_channel_permissions`, `discord_delete_channel_permission`, `discord_list_channel_invites`, `discord_create_channel_invite`, `discord_create_dm` |
| Messages | `discord_send_message`, `discord_send_embed`, `discord_send_dm`, `discord_edit_message`, `discord_delete_message`, `discord_bulk_delete_messages`, `discord_add_reaction`, `discord_remove_own_reaction`, `discord_remove_user_reaction`, `discord_list_reactions`, `discord_pin_message`, `discord_unpin_message`, `discord_list_pinned_messages`, `discord_crosspost_message`, `discord_get_messages` |
| Members & moderation | `discord_list_guild_members`, `discord_get_guild_member`, `discord_search_guild_members`, `discord_modify_guild_member`, `discord_add_member_role`, `discord_remove_member_role`, `discord_kick_member`, `discord_list_bans`, `discord_ban_member`, `discord_unban_member`, `discord_bulk_ban`, `discord_get_prune_count`, `discord_begin_prune` |
| Roles | `discord_list_roles`, `discord_create_role`, `discord_modify_role`, `discord_delete_role` |
| Threads | `discord_list_active_threads`, `discord_list_archived_threads`, `discord_create_thread`, `discord_join_thread`, `discord_leave_thread`, `discord_add_thread_member`, `discord_remove_thread_member`, `discord_list_thread_members` |
| Invites | `discord_list_guild_invites`, `discord_get_invite`, `discord_delete_invite` |
| Webhooks | `discord_list_channel_webhooks`, `discord_list_guild_webhooks`, `discord_create_webhook`, `discord_delete_webhook`, `discord_execute_webhook` |
| Users, emoji, events | `discord_get_user`, `discord_list_guild_emojis`, `discord_list_stickers`, `discord_list_scheduled_events`, `discord_create_scheduled_event`, `discord_delete_scheduled_event`, `discord_list_automod_rules` |
| Escape hatch | `discord_api` — `method` + `path` + `body` against any resource root |
| Settings app | `discord_get_config`, `discord_set_config`, `discord_test_connection` (app-only) |

Moderating calls accept a `reason`, sent as `X-Audit-Log-Reason` so Discord
records it on the audit-log entry.

`discord_api` is a shape guard, not a policy gate: it rejects full URLs, `..`,
whitespace, unknown top-level resources and the credential endpoints (`auth`,
`oauth2`, `login`). Everything else is left to the bot's Discord permissions.

| Resource | Purpose |
| --- | --- |
| `ui://discord-config` | The settings MCP-App rendered in the agent's Apps sidebar |
| `extension://discord/messages` | Push resource: a Gateway v10 WebSocket while subscribed |

Per-agent overrides are JSON files at `data/discord/config_<safe-agent-id>.json`
and are keyed by the `agent_id` the host stamps onto tool calls. The host also
exports `NEUXDOT_AGENT_ID` when it spawns the plugin, so the message stream can
resolve the right token before the first tool call.

## Configuration

Environment variables are preferred over CLI flags because process arguments
are visible through `ps`.

| Variable | Meaning |
| --- | --- |
| `DISCORD_BOT_TOKEN` | Bot token from the Developer Portal. `DISCORD_TOKEN` is accepted as an alias. Optional if every agent configures its own. |
| `DISCORD_CHANNEL_ID` | Default channel id when a tool omits `channel_id`. |
| `DISCORD_API_BASE` | REST root. Defaults to `https://discord.com/api/v10`. |
| `DISCORD_GATEWAY_URL` | Gateway WebSocket URL. Defaults to `wss://gateway.discord.gg/?v=10&encoding=json`. |
| `DISCORD_REQUEST_TIMEOUT_MS` | HTTP timeout. Defaults to 60000. |
| `DISCORD_MCP_LOG` | stderr log filter. Defaults to `warn`. |

CLI equivalents: `--token`, `--channel-id`, `--api-base`, `--gateway-url`.

## Testing

```bash
cargo test        # 33 tests: table/schema invariants, path and body builders,
                  # passthrough guard, config parsing, token precedence
```

The strongest test is `tools::tests::every_schema_matches_its_route`: it walks
the whole table and fails if any path placeholder, query parameter or body field
is echoed to the model without a declared, described property — the exact way a
generated tool surface rots.

## Permissions

There is no plugin-side allow-list. What the bot may do is decided entirely by
its own Discord role, its server permissions and the channel overwrites — the
same place a human moderator's authority comes from. Revoke a capability in
Discord and the tool that needs it starts answering `403`.

## Gateway stream

Subscribe the agent to `extension://discord/messages` (the plugin panel's
**Push MCP Subscriptions** section). The plugin opens a Gateway v10 WebSocket and
identifies with these intents:

| Intent | Value |
| --- | --- |
| `GUILDS` | 1 |
| `GUILD_MESSAGES` | 512 |
| `DIRECT_MESSAGES` | 4096 |
| `MESSAGE_CONTENT` | 32768 |
| **Total** | **37377** |

The **Message Content** intent is privileged and must be enabled in the
Developer Portal, or `content` arrives empty. The bot must also be a member of
the guild and able to view the channel.

The plugin emits one `notifications/resources/updated` notification per incoming
message:

```json
{
  "uri": "extension://discord/messages",
  "type": "message",
  "event": "MESSAGE_CREATE",
  "message_id": "1122334455667788990",
  "channel_id": "998877665544332211",
  "guild_id": "556677889900112233",
  "from": { "id": "123456789012345678", "username": "alice" },
  "text": "hello",
  "timestamp": "2024-01-01T00:00:00.000000+00:00"
}
```

The host stores the notification and wakes the agent from its `from` and `text`
fields, so the agent sees new Discord messages without calling
`discord_get_messages`. An attachment-only message has no `text`, so the plugin
synthesises a `caption` from the filenames ("sent an attachment: cat.png") and
the host wakes the agent from that instead. Both DM and guild messages arrive.
Bot-authored messages, including the plugin's own replies, are dropped to
prevent an echo loop.

The connection is kept alive with heartbeats and ACKs, and resumes with `RESUME`
after a drop. Fatal close codes stop the stream instead of reconnecting forever:
`4004` authentication failed, `4013` invalid intents, `4014` disallowed intent.

## Intents and permissions

`MESSAGE_CONTENT` is a privileged intent — enable it in the Developer Portal
before installing the plugin, otherwise message text is empty. The bot needs
**View Channel** and **Read Message History** to receive messages, and
**Send Messages** to reply. Invite it with the `bot` + `applications.commands`
scopes.

## Build

The plugin is a separate Cargo crate (own lockfile), like the other built-ins:

```bash
cargo build --release --manifest-path built-in-plugin/discord/Cargo.toml
# ./compile.sh --plugins also copies it to release/plugins/discord
```

Install `release/plugins/discord` as a **binary** plugin in the NeuxDot plugins
page, then connect it to an agent, open **Discord Settings** in the Apps sidebar
and subscribe to `extension://discord/messages`.
