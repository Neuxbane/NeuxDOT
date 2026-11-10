# telegram-mcp

A Telegram [Model Context Protocol](https://modelcontextprotocol.io) server in
Rust. It exposes the Bot API as MCP tools, ships a per-agent settings app, and
streams incoming updates as MCP push notifications.

## Design

Tools are generated from one table in `src/tools.rs`, so the published schema and
the method that is actually called cannot drift apart. 118 tools reach the model:
116 table rows plus `telegram_get_updates` and `telegram_api`. Three further
settings tools are published with `_meta.ui.visibility: ["app"]`, which the host
hides from the model, so the settings surface is reachable only from
`ui://telegram-config`.

| Area | Tools |
| --- | --- |
| Bot profile and commands | `telegram_get_me`, `telegram_get_my_name`, `telegram_set_my_name`, `telegram_get_my_description`, `telegram_set_my_description`, `telegram_get_my_short_description`, `telegram_set_my_short_description`, `telegram_get_my_default_administrator_rights`, `telegram_set_my_default_administrator_rights`, `telegram_get_my_commands`, `telegram_set_my_commands`, `telegram_delete_my_commands`, `telegram_get_chat_menu_button`, `telegram_set_chat_menu_button`, `telegram_log_out`, `telegram_close` |
| Update delivery | `telegram_set_webhook`, `telegram_delete_webhook`, `telegram_get_webhook_info`, `telegram_get_updates` |
| Sending | `telegram_send_message`, `telegram_forward_message`, `telegram_copy_message`, `telegram_send_photo`, `telegram_send_audio`, `telegram_send_document`, `telegram_send_video`, `telegram_send_animation`, `telegram_send_voice`, `telegram_send_video_note`, `telegram_send_media_group`, `telegram_send_location`, `telegram_send_venue`, `telegram_send_contact`, `telegram_send_poll`, `telegram_send_dice`, `telegram_send_sticker`, `telegram_send_chat_action`, `telegram_set_message_reaction` |
| Editing and deleting | `telegram_edit_message_text`, `telegram_edit_message_caption`, `telegram_edit_message_media`, `telegram_edit_message_reply_markup`, `telegram_edit_message_live_location`, `telegram_stop_message_live_location`, `telegram_delete_message`, `telegram_delete_messages` |
| Chat info | `telegram_get_chat`, `telegram_get_chat_administrators`, `telegram_get_chat_member_count`, `telegram_get_chat_member`, `telegram_get_user_profile_photos`, `telegram_get_user_chat_boosts`, `telegram_get_file` |
| Chat settings | `telegram_set_chat_title`, `telegram_set_chat_description`, `telegram_set_chat_photo`, `telegram_delete_chat_photo`, `telegram_set_chat_permissions`, `telegram_leave_chat`, `telegram_set_chat_sticker_set`, `telegram_delete_chat_sticker_set` |
| Moderation | `telegram_ban_chat_member`, `telegram_unban_chat_member`, `telegram_restrict_chat_member`, `telegram_promote_chat_member`, `telegram_set_chat_administrator_custom_title`, `telegram_ban_chat_sender_chat`, `telegram_unban_chat_sender_chat`, `telegram_approve_chat_join_request`, `telegram_decline_chat_join_request` |
| Pins and invites | `telegram_pin_chat_message`, `telegram_unpin_chat_message`, `telegram_unpin_all_chat_messages`, `telegram_export_chat_invite_link`, `telegram_create_chat_invite_link`, `telegram_edit_chat_invite_link`, `telegram_revoke_chat_invite_link` |
| Forum topics | `telegram_create_forum_topic`, `telegram_edit_forum_topic`, `telegram_close_forum_topic`, `telegram_reopen_forum_topic`, `telegram_delete_forum_topic`, `telegram_edit_general_forum_topic`, `telegram_close_general_forum_topic`, `telegram_reopen_general_forum_topic`, `telegram_hide_general_forum_topic`, `telegram_unhide_general_forum_topic`, `telegram_get_forum_topic_icon_stickers`, `telegram_unpin_all_forum_topic_messages`, `telegram_unpin_all_general_forum_topic_messages` |
| Stickers and custom emoji | `telegram_get_sticker_set`, `telegram_get_custom_emoji_stickers`, `telegram_create_new_sticker_set`, `telegram_add_sticker_to_set`, `telegram_set_sticker_position_in_set`, `telegram_delete_sticker_from_set`, `telegram_set_sticker_emoji_list`, `telegram_set_sticker_keywords`, `telegram_set_sticker_mask_position`, `telegram_set_sticker_set_title`, `telegram_set_sticker_set_thumbnail`, `telegram_delete_sticker_set` |
| Callbacks, inline, web apps | `telegram_answer_callback_query`, `telegram_answer_inline_query`, `telegram_answer_web_app_query`, `telegram_save_prepared_inline_message` |
| Payments, stars, games | `telegram_send_invoice`, `telegram_create_invoice_link`, `telegram_answer_shipping_query`, `telegram_answer_pre_checkout_query`, `telegram_refund_star_payment`, `telegram_get_star_transactions`, `telegram_send_game`, `telegram_set_game_score`, `telegram_get_game_high_scores` |
| Escape hatch | `telegram_api` — `method` + `params` for any method the table does not name |

`telegram_api` is a shape guard, not a policy gate. It rejects a method name that
is not a plain camelCase word (the name becomes part of the request URL) and
`getUpdates`, which the live poller owns — a second consumer makes Telegram
answer 409 and drop messages.

| Resource | Purpose |
| --- | --- |
| `ui://telegram-config` | The settings MCP-App rendered in the agent's Apps sidebar |
| `extension://telegram/updates` | Push resource: long-polls `getUpdates` while subscribed |

Per-agent overrides are JSON files at `data/telegram/config_<safe-agent-id>.json`
and are keyed by the `agent_id` the host stamps onto tool calls. The host also
exports `NEUXDOT_AGENT_ID` when it spawns the plugin, so the update stream can
resolve the right token before the first tool call.

## Configuration

Environment variables are preferred over CLI flags because process arguments
are visible through `ps`.

| Variable | Meaning |
| --- | --- |
| `TELEGRAM_BOT_TOKEN` | Bot token from `@BotFather`. Optional if every agent configures its own. |
| `TELEGRAM_CHAT_ID` | Default chat id/username when a tool omits `chat_id`. |
| `TELEGRAM_PARSE_MODE` | `HTML` or `MarkdownV2`; empty/plain means plain text. |
| `TELEGRAM_API_BASE` | Bot API root. Defaults to `https://api.telegram.org`; point at a self-hosted Bot API server if needed. |
| `TELEGRAM_REQUEST_TIMEOUT_MS` | HTTP timeout. Defaults to 60000. |
| `TELEGRAM_MCP_LOG` | stderr log filter. Defaults to `warn`. |

CLI equivalents: `--token`, `--chat-id`, `--api-base`.

## Permissions

There is no plugin-side allow-list. What the bot may do is decided entirely by
its own rights in each chat: promote it to administrator in a group or channel to
let it moderate, demote it to take that away. A call the bot is not allowed to
make comes back as a Telegram error naming the missing right.

## Testing

```bash
cargo test        # 22 tests: table/schema invariants, parameter building,
                  # limit validation, passthrough guard, config parsing
```

`tools::tests::every_schema_matches_its_parameters` and
`declared_parameters_reach_the_request` walk the whole table: the first fails if a
published parameter is not validated, the second if a declared parameter never
reaches the request body.

## Update stream

Subscribe the agent to `extension://telegram/updates` (the plugin panel's
**Push MCP Subscriptions** section). The plugin starts a long-poll loop with
`timeout=25` and emits one `notifications/resources/updated` notification per
incoming message:

```json
{
  "uri": "extension://telegram/updates",
  "update_id": 123,
  "type": "message",
  "message_id": 7,
  "date": 1700000000,
  "chat": { "id": 123, "type": "private" },
  "from": { "id": 123, "username": "alice" },
  "text": "hello"
}
```

The host stores each notification and pushes it into the agent's context, so the
agent sees new Telegram messages without calling `telegram_get_updates`.

Only one consumer may call `getUpdates` for a bot at a time. If a webhook or
another poller is active, Telegram answers `409`; delete the webhook or stop the
other consumer. While a subscription is active, `telegram_get_updates` returns
the poller's recent cache instead of racing it.

## Build

The plugin is a separate Cargo crate (own lockfile), like the other built-ins:

```bash
cargo build --release --manifest-path built-in-plugin/telegram/Cargo.toml
# ./compile.sh --plugins also copies it to release/plugins/telegram
```

Install `release/plugins/telegram` as a **binary** plugin in the NeuxDot plugins
page, then connect it to an agent and open **Telegram Settings** in the Apps
sidebar.
