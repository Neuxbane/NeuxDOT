//! The Discord REST tool surface, as data.
//!
//! Each tool is one [`ToolSpec`] row: the HTTP verb, the path template, the
//! arguments it forwards and the arguments it publishes. The JSON schema is
//! **generated** from that row, so a documented argument and a forwarded
//! argument cannot drift apart — the invariants are checked in `tests` below.
//!
//! Three tools do not fit a single REST call and live in `server.rs` instead:
//! `discord_get_messages` (the live Gateway cache), `discord_send_dm` (open a
//! DM then send) and `discord_api` (the guarded passthrough).

use serde_json::{Map, Value, json};

use crate::discord::percent_encode;

/// Discord's hard limits, checked before the call so the model gets a precise
/// error instead of a generic 400 from the API.
pub const MAX_MESSAGE_CHARS: usize = 2000;
pub const MAX_EMBED_DESCRIPTION_CHARS: usize = 4096;
pub const MAX_EMBED_TITLE_CHARS: usize = 256;
pub const MAX_AUDIT_REASON_CHARS: usize = 512;

/// The HTTP verb of a call. Mirrors `reqwest::Method` without depending on it,
/// so this table stays testable without a runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Get,
    Post,
    Patch,
    Put,
    Delete,
}

impl Verb {
    pub fn as_str(self) -> &'static str {
        match self {
            Verb::Get => "GET",
            Verb::Post => "POST",
            Verb::Patch => "PATCH",
            Verb::Put => "PUT",
            Verb::Delete => "DELETE",
        }
    }

    /// A GET is the only verb that cannot change Discord state.
    pub fn is_read_only(self) -> bool {
        matches!(self, Verb::Get)
    }
}

/// The JSON type of one published argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Str,
    Int,
    Bool,
    Obj,
    Arr,
    ArrStr,
}

impl Kind {
    fn schema(self) -> Value {
        match self {
            Kind::Str => json!({ "type": "string" }),
            Kind::Int => json!({ "type": "integer" }),
            Kind::Bool => json!({ "type": "boolean" }),
            Kind::Obj => json!({ "type": "object" }),
            Kind::Arr => json!({ "type": "array" }),
            Kind::ArrStr => json!({ "type": "array", "items": { "type": "string" } }),
        }
    }
}

/// One published argument.
pub struct Prop {
    pub name: &'static str,
    pub kind: Kind,
    pub required: bool,
    pub description: &'static str,
}

pub const fn p(name: &'static str, kind: Kind, required: bool, description: &'static str) -> Prop {
    Prop {
        name,
        kind,
        required,
        description,
    }
}

/// How the arguments become a request body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// No body (every GET and most DELETEs).
    None,
    /// Copy these argument names verbatim into the body.
    Fields(&'static [&'static str]),
    /// `{"content": text}`.
    Content,
    /// A single embed built from `title`, `description`, `url` and `color`.
    Embed,
    /// `{"messages": [...]}` from `message_ids`.
    BulkDelete,
    /// A channel permission overwrite: `{"type", "allow", "deny"}`.
    Overwrite,
    /// A webhook message body.
    WebhookMessage,
}

pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub verb: Verb,
    pub path: &'static [&'static str],
    /// Arguments forwarded as query parameters, in order.
    pub query: &'static [&'static str],
    pub body: Body,
    pub destructive: bool,
    pub props: &'static [Prop],
}

// The row builder is a positional constructor used 70 times in one const table;
// bundling its arguments into a wrapper struct would only move the same eight
// fields one line up and make the table harder to scan.
#[allow(clippy::too_many_arguments)]
const fn tool(
    name: &'static str,
    description: &'static str,
    verb: Verb,
    path: &'static [&'static str],
    query: &'static [&'static str],
    body: Body,
    destructive: bool,
    props: &'static [Prop],
) -> ToolSpec {
    ToolSpec {
        name,
        description,
        verb,
        path,
        query,
        body,
        destructive,
        props,
    }
}

// Frequently repeated property descriptions, so the table stays readable.
const GUILD_ID: Prop = p(
    "guild_id",
    Kind::Str,
    true,
    "Server (guild) id. Discord calls servers guilds.",
);
const CHANNEL_ID: Prop = p("channel_id", Kind::Str, true, "Channel id.");
const MESSAGE_ID: Prop = p("message_id", Kind::Str, true, "Message id.");
const USER_ID: Prop = p("user_id", Kind::Str, true, "User id (snowflake).");
const ROLE_ID: Prop = p("role_id", Kind::Str, true, "Role id.");

const GUILD_ID_OPT: Prop = p(
    "guild_id",
    Kind::Str,
    true,
    "Server (guild) id. Discord calls servers guilds.",
);

/// The full table. Ordered by area so the tool list reads like the docs.
pub const TOOLS: &[ToolSpec] = &[
    // ---- Bot and server discovery -------------------------------------
    tool(
        "discord_get_me",
        "Return the bot identity the configured token belongs to",
        Verb::Get,
        &["users", "@me"],
        &[],
        Body::None,
        false,
        &[],
    ),
    tool(
        "discord_list_guilds",
        "List the servers (guilds) this bot is a member of, with the bot's permissions in each",
        Verb::Get,
        &["users", "@me", "guilds"],
        &[],
        Body::None,
        false,
        &[],
    ),
    tool(
        "discord_get_guild",
        "Return one server's settings (name, icon, owner, member counts, roles, channels)",
        Verb::Get,
        &["guilds", "{guild_id}"],
        &["with_counts"],
        Body::None,
        false,
        &[
            GUILD_ID_OPT,
            p(
                "with_counts",
                Kind::Bool,
                false,
                "Include approximate member and presence counts",
            ),
        ],
    ),
    tool(
        "discord_modify_guild",
        "Update server settings: name, description, verification level, notification defaults, AFK and system channels",
        Verb::Patch,
        &["guilds", "{guild_id}"],
        &[],
        Body::Fields(&[
            "name",
            "description",
            "verification_level",
            "default_message_notifications",
            "explicit_content_filter",
            "afk_channel_id",
            "afk_timeout",
            "system_channel_id",
            "preferred_locale",
        ]),
        false,
        &[
            GUILD_ID,
            p(
                "name",
                Kind::Str,
                false,
                "New server name (2-100 characters)",
            ),
            p("description", Kind::Str, false, "Server description"),
            p(
                "verification_level",
                Kind::Int,
                false,
                "0 none, 1 low, 2 medium, 3 high, 4 highest",
            ),
            p(
                "default_message_notifications",
                Kind::Int,
                false,
                "0 all messages, 1 mentions only",
            ),
            p(
                "explicit_content_filter",
                Kind::Int,
                false,
                "0 disabled, 1 members without roles, 2 all members",
            ),
            p(
                "afk_channel_id",
                Kind::Str,
                false,
                "Voice channel id used for AFK",
            ),
            p("afk_timeout", Kind::Int, false, "AFK timeout in seconds"),
            p(
                "system_channel_id",
                Kind::Str,
                false,
                "Channel id for system messages",
            ),
            p("preferred_locale", Kind::Str, false, "Locale, e.g. en-US"),
        ],
    ),
    tool(
        "discord_get_guild_preview",
        "Return the public preview of a server, including the online member count",
        Verb::Get,
        &["guilds", "{guild_id}", "preview"],
        &[],
        Body::None,
        false,
        &[GUILD_ID],
    ),
    tool(
        "discord_get_guild_vanity_url",
        "Return the vanity invite URL of a server",
        Verb::Get,
        &["guilds", "{guild_id}", "vanity-url"],
        &[],
        Body::None,
        false,
        &[GUILD_ID],
    ),
    tool(
        "discord_get_audit_log",
        "Read a server's audit log, newest first",
        Verb::Get,
        &["guilds", "{guild_id}", "audit-logs"],
        &["user_id", "action_type", "before", "limit"],
        Body::None,
        false,
        &[
            GUILD_ID,
            p(
                "user_id",
                Kind::Str,
                false,
                "Only entries by this moderator",
            ),
            p(
                "action_type",
                Kind::Int,
                false,
                "Only this action type, e.g. 22 for MEMBER_BAN_ADD",
            ),
            p(
                "before",
                Kind::Str,
                false,
                "Only entries before this entry id",
            ),
            p("limit", Kind::Int, false, "1-100, default 50"),
        ],
    ),
    // ---- Channels ------------------------------------------------------
    tool(
        "discord_list_guild_channels",
        "List every channel in a server, including categories, with their ids and positions",
        Verb::Get,
        &["guilds", "{guild_id}", "channels"],
        &[],
        Body::None,
        false,
        &[GUILD_ID],
    ),
    tool(
        "discord_get_channel",
        "Return one channel's settings",
        Verb::Get,
        &["channels", "{channel_id}"],
        &[],
        Body::None,
        false,
        &[CHANNEL_ID],
    ),
    tool(
        "discord_create_guild_channel",
        "Create a text, voice, category, announcement or forum channel in a server",
        Verb::Post,
        &["guilds", "{guild_id}", "channels"],
        &[],
        Body::Fields(&[
            "name",
            "type",
            "topic",
            "parent_id",
            "position",
            "nsfw",
            "rate_limit_per_user",
            "bitrate",
            "user_limit",
        ]),
        false,
        &[
            GUILD_ID,
            p("name", Kind::Str, true, "Channel name (1-100 characters)"),
            p(
                "type",
                Kind::Int,
                false,
                "0 text, 2 voice, 4 category, 5 announcement, 15 forum, 13 stage",
            ),
            p("topic", Kind::Str, false, "Channel topic"),
            p(
                "parent_id",
                Kind::Str,
                false,
                "Category channel id to nest under",
            ),
            p("position", Kind::Int, false, "Sort position"),
            p("nsfw", Kind::Bool, false, "Mark the channel age-restricted"),
            p(
                "rate_limit_per_user",
                Kind::Int,
                false,
                "Slowmode in seconds (0-21600)",
            ),
            p(
                "bitrate",
                Kind::Int,
                false,
                "Voice bitrate in bits per second",
            ),
            p(
                "user_limit",
                Kind::Int,
                false,
                "Voice user limit (0 = unlimited)",
            ),
        ],
    ),
    tool(
        "discord_modify_channel",
        "Update a channel: name, topic, position, slowmode, parent, voice settings or NSFW flag",
        Verb::Patch,
        &["channels", "{channel_id}"],
        &[],
        Body::Fields(&[
            "name",
            "topic",
            "position",
            "nsfw",
            "rate_limit_per_user",
            "parent_id",
            "bitrate",
            "user_limit",
            "archived",
            "auto_archive_duration",
        ]),
        false,
        &[
            CHANNEL_ID,
            p("name", Kind::Str, false, "New channel name"),
            p("topic", Kind::Str, false, "New topic"),
            p("position", Kind::Int, false, "New sort position"),
            p("nsfw", Kind::Bool, false, "Age-restrict the channel"),
            p(
                "rate_limit_per_user",
                Kind::Int,
                false,
                "Slowmode in seconds",
            ),
            p("parent_id", Kind::Str, false, "Category channel id"),
            p("bitrate", Kind::Int, false, "Voice bitrate"),
            p("user_limit", Kind::Int, false, "Voice user limit"),
            p(
                "archived",
                Kind::Bool,
                false,
                "Archive or unarchive a thread",
            ),
            p(
                "auto_archive_duration",
                Kind::Int,
                false,
                "Thread auto-archive minutes: 60, 1440, 4320 or 10080",
            ),
        ],
    ),
    tool(
        "discord_delete_channel",
        "Permanently delete a channel or thread and all of its messages",
        Verb::Delete,
        &["channels", "{channel_id}"],
        &[],
        Body::None,
        true,
        &[CHANNEL_ID],
    ),
    tool(
        "discord_set_channel_permissions",
        "Create or replace a permission overwrite for a role or member on one channel",
        Verb::Put,
        &["channels", "{channel_id}", "permissions", "{overwrite_id}"],
        &[],
        Body::Overwrite,
        false,
        &[
            CHANNEL_ID,
            p(
                "overwrite_id",
                Kind::Str,
                true,
                "Role id or member (user) id the overwrite applies to",
            ),
            p(
                "type",
                Kind::Int,
                true,
                "0 for a role overwrite, 1 for a member overwrite",
            ),
            p(
                "allow",
                Kind::Str,
                false,
                "Decimal permission bitfield to allow, e.g. \"1024\"",
            ),
            p(
                "deny",
                Kind::Str,
                false,
                "Decimal permission bitfield to deny, e.g. \"2048\"",
            ),
        ],
    ),
    tool(
        "discord_delete_channel_permission",
        "Remove a permission overwrite from a channel",
        Verb::Delete,
        &["channels", "{channel_id}", "permissions", "{overwrite_id}"],
        &[],
        Body::None,
        true,
        &[
            CHANNEL_ID,
            p("overwrite_id", Kind::Str, true, "Role id or member id"),
        ],
    ),
    tool(
        "discord_list_channel_invites",
        "List the invites created for one channel",
        Verb::Get,
        &["channels", "{channel_id}", "invites"],
        &[],
        Body::None,
        false,
        &[CHANNEL_ID],
    ),
    tool(
        "discord_create_channel_invite",
        "Create an invite for a channel: expiry, use limit, temporary membership",
        Verb::Post,
        &["channels", "{channel_id}", "invites"],
        &[],
        Body::Fields(&[
            "max_age",
            "max_uses",
            "temporary",
            "unique",
            "target_type",
            "target_user_id",
        ]),
        false,
        &[
            CHANNEL_ID,
            p("max_age", Kind::Int, false, "Seconds until expiry, 0 never"),
            p("max_uses", Kind::Int, false, "Maximum uses, 0 unlimited"),
            p(
                "temporary",
                Kind::Bool,
                false,
                "Kick members who never got a role",
            ),
            p(
                "unique",
                Kind::Bool,
                false,
                "Always create a new invite code",
            ),
            p(
                "target_type",
                Kind::Int,
                false,
                "1 stream, 2 embedded application",
            ),
            p(
                "target_user_id",
                Kind::Str,
                false,
                "Target user id for the invite",
            ),
        ],
    ),
    tool(
        "discord_create_dm",
        "Open (or reuse) the direct-message channel with a user and return its channel id",
        Verb::Post,
        &["users", "@me", "channels"],
        &[],
        Body::Fields(&["recipient_id"]),
        false,
        &[p(
            "recipient_id",
            Kind::Str,
            true,
            "User id to open a DM with",
        )],
    ),
    // ---- Messages ------------------------------------------------------
    tool(
        "discord_send_message",
        "Send a text message to a channel",
        Verb::Post,
        &["channels", "{channel_id}", "messages"],
        &[],
        Body::Content,
        false,
        &[
            p(
                "channel_id",
                Kind::Str,
                true,
                "Channel id. Defaults to the agent's configured channel when omitted.",
            ),
            p("text", Kind::Str, true, "Message text (1-2000 characters)"),
            p(
                "reply_to_message_id",
                Kind::Str,
                false,
                "Reply to this message id",
            ),
        ],
    ),
    tool(
        "discord_send_embed",
        "Send a rich embed to a channel",
        Verb::Post,
        &["channels", "{channel_id}", "messages"],
        &[],
        Body::Embed,
        false,
        &[
            p(
                "channel_id",
                Kind::Str,
                true,
                "Channel id. Defaults to the agent's configured channel when omitted.",
            ),
            p(
                "title",
                Kind::Str,
                false,
                "Embed title (up to 256 characters)",
            ),
            p(
                "description",
                Kind::Str,
                true,
                "Embed body (up to 4096 characters)",
            ),
            p("url", Kind::Str, false, "Link applied to the embed title"),
            p(
                "color",
                Kind::Int,
                false,
                "Decimal RGB colour, e.g. 5793266 for blurple",
            ),
        ],
    ),
    tool(
        "discord_edit_message",
        "Replace the text of a message the bot sent",
        Verb::Patch,
        &["channels", "{channel_id}", "messages", "{message_id}"],
        &[],
        Body::Content,
        false,
        &[
            CHANNEL_ID,
            MESSAGE_ID,
            p(
                "text",
                Kind::Str,
                true,
                "New message text (up to 2000 characters)",
            ),
        ],
    ),
    tool(
        "discord_delete_message",
        "Delete one message from a channel",
        Verb::Delete,
        &["channels", "{channel_id}", "messages", "{message_id}"],
        &[],
        Body::None,
        true,
        &[CHANNEL_ID, MESSAGE_ID],
    ),
    tool(
        "discord_bulk_delete_messages",
        "Delete 2-100 messages from a channel in one call",
        Verb::Post,
        &["channels", "{channel_id}", "messages", "bulk-delete"],
        &[],
        Body::BulkDelete,
        true,
        &[
            CHANNEL_ID,
            p(
                "message_ids",
                Kind::ArrStr,
                true,
                "2-100 message ids. Messages older than 14 days cannot be bulk deleted.",
            ),
        ],
    ),
    tool(
        "discord_add_reaction",
        "Add an emoji reaction to a message as the bot",
        Verb::Put,
        &[
            "channels",
            "{channel_id}",
            "messages",
            "{message_id}",
            "reactions",
            "{emoji}",
            "@me",
        ],
        &[],
        Body::None,
        false,
        &[
            CHANNEL_ID,
            MESSAGE_ID,
            p(
                "emoji",
                Kind::Str,
                true,
                "Unicode emoji, or name:id for a custom emoji",
            ),
        ],
    ),
    tool(
        "discord_remove_own_reaction",
        "Remove one of the bot's own reactions from a message",
        Verb::Delete,
        &[
            "channels",
            "{channel_id}",
            "messages",
            "{message_id}",
            "reactions",
            "{emoji}",
            "@me",
        ],
        &[],
        Body::None,
        false,
        &[
            CHANNEL_ID,
            MESSAGE_ID,
            p(
                "emoji",
                Kind::Str,
                true,
                "Unicode emoji, or name:id for a custom emoji",
            ),
        ],
    ),
    tool(
        "discord_remove_user_reaction",
        "Remove a specific user's reaction from a message",
        Verb::Delete,
        &[
            "channels",
            "{channel_id}",
            "messages",
            "{message_id}",
            "reactions",
            "{emoji}",
            "{user_id}",
        ],
        &[],
        Body::None,
        true,
        &[
            CHANNEL_ID,
            MESSAGE_ID,
            p(
                "emoji",
                Kind::Str,
                true,
                "Unicode emoji, or name:id for a custom emoji",
            ),
            USER_ID,
        ],
    ),
    tool(
        "discord_list_reactions",
        "List the users who reacted with one emoji on a message",
        Verb::Get,
        &[
            "channels",
            "{channel_id}",
            "messages",
            "{message_id}",
            "reactions",
            "{emoji}",
        ],
        &["limit", "after"],
        Body::None,
        false,
        &[
            CHANNEL_ID,
            MESSAGE_ID,
            p(
                "emoji",
                Kind::Str,
                true,
                "Unicode emoji, or name:id for a custom emoji",
            ),
            p("limit", Kind::Int, false, "1-100, default 25"),
            p("after", Kind::Str, false, "Only users after this user id"),
        ],
    ),
    tool(
        "discord_pin_message",
        "Pin a message in a channel (50 pins per channel)",
        Verb::Put,
        &["channels", "{channel_id}", "pins", "{message_id}"],
        &[],
        Body::None,
        false,
        &[CHANNEL_ID, MESSAGE_ID],
    ),
    tool(
        "discord_unpin_message",
        "Unpin a message from a channel",
        Verb::Delete,
        &["channels", "{channel_id}", "pins", "{message_id}"],
        &[],
        Body::None,
        false,
        &[CHANNEL_ID, MESSAGE_ID],
    ),
    tool(
        "discord_list_pinned_messages",
        "List the pinned messages in a channel",
        Verb::Get,
        &["channels", "{channel_id}", "pins"],
        &[],
        Body::None,
        false,
        &[CHANNEL_ID],
    ),
    tool(
        "discord_crosspost_message",
        "Publish an announcement-channel message to every server that follows it",
        Verb::Post,
        &[
            "channels",
            "{channel_id}",
            "messages",
            "{message_id}",
            "crosspost",
        ],
        &[],
        Body::None,
        false,
        &[CHANNEL_ID, MESSAGE_ID],
    ),
    // ---- Members and moderation ----------------------------------------
    tool(
        "discord_list_guild_members",
        "List the members of a server, with their roles and join dates",
        Verb::Get,
        &["guilds", "{guild_id}", "members"],
        &["limit", "after"],
        Body::None,
        false,
        &[
            GUILD_ID,
            p("limit", Kind::Int, false, "1-1000, default 100"),
            p("after", Kind::Str, false, "Only members after this user id"),
        ],
    ),
    tool(
        "discord_get_guild_member",
        "Return one member: nickname, roles, join date and timeout state",
        Verb::Get,
        &["guilds", "{guild_id}", "members", "{user_id}"],
        &[],
        Body::None,
        false,
        &[GUILD_ID, USER_ID],
    ),
    tool(
        "discord_search_guild_members",
        "Search a server's members by username or nickname prefix",
        Verb::Get,
        &["guilds", "{guild_id}", "members", "search"],
        &["query", "limit"],
        Body::None,
        false,
        &[
            GUILD_ID,
            p("query", Kind::Str, true, "Name or nickname prefix to match"),
            p("limit", Kind::Int, false, "1-1000, default 25"),
        ],
    ),
    tool(
        "discord_modify_guild_member",
        "Edit one member: nickname, roles, server mute/deafen, or a timeout until a timestamp",
        Verb::Patch,
        &["guilds", "{guild_id}", "members", "{user_id}"],
        &[],
        Body::Fields(&[
            "nick",
            "roles",
            "mute",
            "deaf",
            "communication_disabled_until",
            "channel_id",
        ]),
        false,
        &[
            GUILD_ID,
            USER_ID,
            p(
                "nick",
                Kind::Str,
                false,
                "New nickname, empty string to clear",
            ),
            p(
                "roles",
                Kind::ArrStr,
                false,
                "Complete list of role ids the member should have",
            ),
            p("mute", Kind::Bool, false, "Server-mute the member"),
            p("deaf", Kind::Bool, false, "Server-deafen the member"),
            p(
                "communication_disabled_until",
                Kind::Str,
                false,
                "ISO8601 timestamp for a timeout, or null to lift it",
            ),
            p(
                "channel_id",
                Kind::Str,
                false,
                "Move the member to this voice channel",
            ),
        ],
    ),
    tool(
        "discord_add_member_role",
        "Give one role to one member",
        Verb::Put,
        &[
            "guilds",
            "{guild_id}",
            "members",
            "{user_id}",
            "roles",
            "{role_id}",
        ],
        &[],
        Body::None,
        false,
        &[GUILD_ID, USER_ID, ROLE_ID],
    ),
    tool(
        "discord_remove_member_role",
        "Take one role away from one member",
        Verb::Delete,
        &[
            "guilds",
            "{guild_id}",
            "members",
            "{user_id}",
            "roles",
            "{role_id}",
        ],
        &[],
        Body::None,
        true,
        &[GUILD_ID, USER_ID, ROLE_ID],
    ),
    tool(
        "discord_kick_member",
        "Kick a member from a server. They can rejoin with a new invite.",
        Verb::Delete,
        &["guilds", "{guild_id}", "members", "{user_id}"],
        &[],
        Body::None,
        true,
        &[GUILD_ID, USER_ID],
    ),
    tool(
        "discord_list_bans",
        "List the users banned from a server, with the reason recorded for each",
        Verb::Get,
        &["guilds", "{guild_id}", "bans"],
        &["limit", "before", "after"],
        Body::None,
        false,
        &[
            GUILD_ID,
            p("limit", Kind::Int, false, "1-1000, default 100"),
            p("before", Kind::Str, false, "Only bans before this user id"),
            p("after", Kind::Str, false, "Only bans after this user id"),
        ],
    ),
    tool(
        "discord_ban_member",
        "Ban a user from a server, optionally deleting their recent messages",
        Verb::Put,
        &["guilds", "{guild_id}", "bans", "{user_id}"],
        &[],
        Body::Fields(&["delete_message_seconds"]),
        true,
        &[
            GUILD_ID,
            USER_ID,
            p(
                "delete_message_seconds",
                Kind::Int,
                false,
                "Delete this many seconds of the user's recent messages (0-604800)",
            ),
        ],
    ),
    tool(
        "discord_unban_member",
        "Lift a ban so the user may rejoin with an invite",
        Verb::Delete,
        &["guilds", "{guild_id}", "bans", "{user_id}"],
        &[],
        Body::None,
        true,
        &[GUILD_ID, USER_ID],
    ),
    tool(
        "discord_bulk_ban",
        "Ban up to 200 users in one call",
        Verb::Post,
        &["guilds", "{guild_id}", "bulk-ban"],
        &[],
        Body::Fields(&["user_ids", "delete_message_seconds"]),
        true,
        &[
            GUILD_ID,
            p("user_ids", Kind::ArrStr, true, "1-200 user ids"),
            p(
                "delete_message_seconds",
                Kind::Int,
                false,
                "Delete this many seconds of their recent messages",
            ),
        ],
    ),
    tool(
        "discord_get_prune_count",
        "Count the members who would be pruned for inactivity",
        Verb::Get,
        &["guilds", "{guild_id}", "prune"],
        &["days"],
        Body::None,
        false,
        &[
            GUILD_ID,
            p(
                "days",
                Kind::Int,
                false,
                "Inactivity window in days (1-30, default 7)",
            ),
        ],
    ),
    tool(
        "discord_begin_prune",
        "Kick every member inactive for the given number of days. Irreversible.",
        Verb::Post,
        &["guilds", "{guild_id}", "prune"],
        &[],
        Body::Fields(&["days", "compute_prune_count", "include_roles"]),
        true,
        &[
            GUILD_ID,
            p("days", Kind::Int, true, "Inactivity window in days (1-30)"),
            p(
                "compute_prune_count",
                Kind::Bool,
                false,
                "Return the pruned count",
            ),
            p(
                "include_roles",
                Kind::ArrStr,
                false,
                "Also prune members holding these role ids",
            ),
        ],
    ),
    // ---- Roles ---------------------------------------------------------
    tool(
        "discord_list_roles",
        "List a server's roles, with permissions, colour and position",
        Verb::Get,
        &["guilds", "{guild_id}", "roles"],
        &[],
        Body::None,
        false,
        &[GUILD_ID],
    ),
    tool(
        "discord_create_role",
        "Create a role with a name, colour, permission bitfield and hoist/mention flags",
        Verb::Post,
        &["guilds", "{guild_id}", "roles"],
        &[],
        Body::Fields(&[
            "name",
            "permissions",
            "color",
            "hoist",
            "mentionable",
            "unicode_emoji",
        ]),
        false,
        &[
            GUILD_ID,
            p("name", Kind::Str, false, "Role name"),
            p(
                "permissions",
                Kind::Str,
                false,
                "Decimal permission bitfield, e.g. \"1024\" for VIEW_CHANNEL",
            ),
            p("color", Kind::Int, false, "Decimal RGB colour"),
            p(
                "hoist",
                Kind::Bool,
                false,
                "Show members with this role separately",
            ),
            p(
                "mentionable",
                Kind::Bool,
                false,
                "Allow anyone to mention the role",
            ),
            p("unicode_emoji", Kind::Str, false, "Role icon emoji"),
        ],
    ),
    tool(
        "discord_modify_role",
        "Update a role's name, colour, permissions, hoist/mentionable flags or position",
        Verb::Patch,
        &["guilds", "{guild_id}", "roles", "{role_id}"],
        &[],
        Body::Fields(&[
            "name",
            "permissions",
            "color",
            "hoist",
            "mentionable",
            "position",
            "unicode_emoji",
        ]),
        false,
        &[
            GUILD_ID,
            ROLE_ID,
            p("name", Kind::Str, false, "New role name"),
            p(
                "permissions",
                Kind::Str,
                false,
                "Decimal permission bitfield",
            ),
            p("color", Kind::Int, false, "Decimal RGB colour"),
            p(
                "hoist",
                Kind::Bool,
                false,
                "Display role members separately",
            ),
            p(
                "mentionable",
                Kind::Bool,
                false,
                "Allow anyone to mention the role",
            ),
            p(
                "position",
                Kind::Int,
                false,
                "Role position in the hierarchy",
            ),
            p("unicode_emoji", Kind::Str, false, "Role icon emoji"),
        ],
    ),
    tool(
        "discord_delete_role",
        "Delete a role. Members lose it and its permissions.",
        Verb::Delete,
        &["guilds", "{guild_id}", "roles", "{role_id}"],
        &[],
        Body::None,
        true,
        &[GUILD_ID, ROLE_ID],
    ),
    // ---- Threads -------------------------------------------------------
    tool(
        "discord_list_active_threads",
        "List the active threads in a server",
        Verb::Get,
        &["guilds", "{guild_id}", "threads", "active"],
        &[],
        Body::None,
        false,
        &[GUILD_ID],
    ),
    tool(
        "discord_list_archived_threads",
        "List the archived public threads of a channel",
        Verb::Get,
        &["channels", "{channel_id}", "threads", "archived", "public"],
        &["before", "limit"],
        Body::None,
        false,
        &[
            CHANNEL_ID,
            p(
                "before",
                Kind::Str,
                false,
                "Only threads archived before this timestamp",
            ),
            p("limit", Kind::Int, false, "1-100, default 50"),
        ],
    ),
    tool(
        "discord_create_thread",
        "Start a thread in a channel without a starting message",
        Verb::Post,
        &["channels", "{channel_id}", "threads"],
        &[],
        Body::Fields(&[
            "name",
            "type",
            "auto_archive_duration",
            "invitable",
            "rate_limit_per_user",
        ]),
        false,
        &[
            CHANNEL_ID,
            p("name", Kind::Str, true, "Thread name (1-100 characters)"),
            p(
                "type",
                Kind::Int,
                false,
                "11 public thread, 12 private thread",
            ),
            p(
                "auto_archive_duration",
                Kind::Int,
                false,
                "Minutes of inactivity before archiving: 60, 1440, 4320 or 10080",
            ),
            p(
                "invitable",
                Kind::Bool,
                false,
                "Let members add others (private threads)",
            ),
            p(
                "rate_limit_per_user",
                Kind::Int,
                false,
                "Slowmode in seconds",
            ),
        ],
    ),
    tool(
        "discord_join_thread",
        "Add the bot to a thread",
        Verb::Put,
        &["channels", "{channel_id}", "thread-members", "@me"],
        &[],
        Body::None,
        false,
        &[CHANNEL_ID],
    ),
    tool(
        "discord_leave_thread",
        "Remove the bot from a thread",
        Verb::Delete,
        &["channels", "{channel_id}", "thread-members", "@me"],
        &[],
        Body::None,
        false,
        &[CHANNEL_ID],
    ),
    tool(
        "discord_add_thread_member",
        "Add a member to a thread",
        Verb::Put,
        &["channels", "{channel_id}", "thread-members", "{user_id}"],
        &[],
        Body::None,
        false,
        &[CHANNEL_ID, USER_ID],
    ),
    tool(
        "discord_remove_thread_member",
        "Remove a member from a thread",
        Verb::Delete,
        &["channels", "{channel_id}", "thread-members", "{user_id}"],
        &[],
        Body::None,
        true,
        &[CHANNEL_ID, USER_ID],
    ),
    tool(
        "discord_list_thread_members",
        "List the members of a thread",
        Verb::Get,
        &["channels", "{channel_id}", "thread-members"],
        &[],
        Body::None,
        false,
        &[CHANNEL_ID],
    ),
    // ---- Invites -------------------------------------------------------
    tool(
        "discord_list_guild_invites",
        "List the active invites of a server",
        Verb::Get,
        &["guilds", "{guild_id}", "invites"],
        &[],
        Body::None,
        false,
        &[GUILD_ID],
    ),
    tool(
        "discord_get_invite",
        "Resolve an invite code into the channel, server and inviter it points at",
        Verb::Get,
        &["invites", "{invite_code}"],
        &["with_counts", "with_expiration"],
        Body::None,
        false,
        &[
            p(
                "invite_code",
                Kind::Str,
                true,
                "Invite code, the part after discord.gg/",
            ),
            p(
                "with_counts",
                Kind::Bool,
                false,
                "Include approximate member counts",
            ),
            p(
                "with_expiration",
                Kind::Bool,
                false,
                "Include the expiry timestamp",
            ),
        ],
    ),
    tool(
        "discord_delete_invite",
        "Revoke an invite code",
        Verb::Delete,
        &["invites", "{invite_code}"],
        &[],
        Body::None,
        true,
        &[p("invite_code", Kind::Str, true, "Invite code to revoke")],
    ),
    // ---- Webhooks ------------------------------------------------------
    tool(
        "discord_list_channel_webhooks",
        "List the webhooks attached to a channel",
        Verb::Get,
        &["channels", "{channel_id}", "webhooks"],
        &[],
        Body::None,
        false,
        &[CHANNEL_ID],
    ),
    tool(
        "discord_list_guild_webhooks",
        "List a server's webhooks",
        Verb::Get,
        &["guilds", "{guild_id}", "webhooks"],
        &[],
        Body::None,
        false,
        &[GUILD_ID],
    ),
    tool(
        "discord_create_webhook",
        "Create a webhook on a channel and return its URL and token",
        Verb::Post,
        &["channels", "{channel_id}", "webhooks"],
        &[],
        Body::Fields(&["name", "avatar_url"]),
        false,
        &[
            CHANNEL_ID,
            p("name", Kind::Str, true, "Webhook name (1-80 characters)"),
            p("avatar_url", Kind::Str, false, "Webhook avatar image URL"),
        ],
    ),
    tool(
        "discord_delete_webhook",
        "Delete a webhook",
        Verb::Delete,
        &["webhooks", "{webhook_id}"],
        &[],
        Body::None,
        true,
        &[p("webhook_id", Kind::Str, true, "Webhook id")],
    ),
    tool(
        "discord_execute_webhook",
        "Post a message through a webhook, with an optional custom username and avatar",
        Verb::Post,
        &["webhooks", "{webhook_id}", "{webhook_token}"],
        &["wait"],
        Body::WebhookMessage,
        false,
        &[
            p("webhook_id", Kind::Str, true, "Webhook id"),
            p(
                "webhook_token",
                Kind::Str,
                true,
                "Webhook token from the webhook URL",
            ),
            p(
                "content",
                Kind::Str,
                false,
                "Message text (up to 2000 characters)",
            ),
            p(
                "username",
                Kind::Str,
                false,
                "Override the displayed username",
            ),
            p(
                "avatar_url",
                Kind::Str,
                false,
                "Override the displayed avatar",
            ),
            p("tts", Kind::Bool, false, "Send as a text-to-speech message"),
            p("embeds", Kind::Arr, false, "Raw embed objects to attach"),
            p(
                "thread_name",
                Kind::Str,
                false,
                "Create a forum post with this title (forum webhooks only)",
            ),
            p(
                "wait",
                Kind::Bool,
                false,
                "Return the created message instead of 204",
            ),
        ],
    ),
    // ---- Users, emoji, events, automod ---------------------------------
    tool(
        "discord_get_user",
        "Return a user's public profile by id",
        Verb::Get,
        &["users", "{user_id}"],
        &[],
        Body::None,
        false,
        &[p("user_id", Kind::Str, true, "User id (snowflake)")],
    ),
    tool(
        "discord_list_guild_emojis",
        "List a server's custom emoji",
        Verb::Get,
        &["guilds", "{guild_id}", "emojis"],
        &[],
        Body::None,
        false,
        &[GUILD_ID],
    ),
    tool(
        "discord_list_stickers",
        "List a server's custom stickers",
        Verb::Get,
        &["guilds", "{guild_id}", "stickers"],
        &[],
        Body::None,
        false,
        &[GUILD_ID],
    ),
    tool(
        "discord_list_scheduled_events",
        "List a server's scheduled events",
        Verb::Get,
        &["guilds", "{guild_id}", "scheduled-events"],
        &["with_user_count"],
        Body::None,
        false,
        &[
            GUILD_ID,
            p(
                "with_user_count",
                Kind::Bool,
                false,
                "Include subscriber counts",
            ),
        ],
    ),
    tool(
        "discord_create_scheduled_event",
        "Schedule an event in a server, either at a location or in a stage/voice channel",
        Verb::Post,
        &["guilds", "{guild_id}", "scheduled-events"],
        &[],
        Body::Fields(&[
            "name",
            "description",
            "scheduled_start_time",
            "scheduled_end_time",
            "privacy_level",
            "entity_type",
            "entity_metadata",
            "channel_id",
        ]),
        false,
        &[
            GUILD_ID,
            p("name", Kind::Str, true, "Event name"),
            p("description", Kind::Str, false, "Event description"),
            p(
                "scheduled_start_time",
                Kind::Str,
                true,
                "ISO8601 start timestamp, e.g. 2026-01-31T18:00:00Z",
            ),
            p(
                "scheduled_end_time",
                Kind::Str,
                false,
                "ISO8601 end timestamp (required for external events)",
            ),
            p("privacy_level", Kind::Int, false, "2 guild only"),
            p(
                "entity_type",
                Kind::Int,
                false,
                "1 stage instance, 2 voice, 3 external",
            ),
            p(
                "entity_metadata",
                Kind::Obj,
                false,
                "{\"location\": \"...\"} for external events",
            ),
            p("channel_id", Kind::Str, false, "Stage or voice channel id"),
        ],
    ),
    tool(
        "discord_delete_scheduled_event",
        "Cancel and delete a scheduled event",
        Verb::Delete,
        &["guilds", "{guild_id}", "scheduled-events", "{event_id}"],
        &[],
        Body::None,
        true,
        &[
            GUILD_ID,
            p("event_id", Kind::Str, true, "Scheduled event id"),
        ],
    ),
    tool(
        "discord_list_automod_rules",
        "List a server's auto-moderation rules",
        Verb::Get,
        &["guilds", "{guild_id}", "auto-moderation", "rules"],
        &[],
        Body::None,
        false,
        &[GUILD_ID],
    ),
];

/// Look up one tool by name.
pub fn find(name: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|spec| spec.name == name)
}

/// Every argument the schema publishes, including the two injected ones.
pub fn schema(spec: &ToolSpec) -> Value {
    let mut properties = Map::new();
    let mut required: Vec<Value> = Vec::new();

    for placeholder in path_placeholders(spec.path) {
        // A placeholder is always required: without it there is no URL.
        if let Some(prop) = spec.props.iter().find(|p| p.name == placeholder) {
            let mut value = prop.kind.schema();
            value["description"] = json!(prop.description);
            properties.insert(placeholder.to_string(), value);
        } else {
            properties.insert(placeholder.to_string(), json!({ "type": "string" }));
        }
        required.push(json!(placeholder));
    }

    for prop in spec.props {
        if properties.contains_key(prop.name) {
            continue;
        }
        let mut value = prop.kind.schema();
        value["description"] = json!(prop.description);
        properties.insert(prop.name.to_string(), value);
    }
    for prop in spec.props.iter().filter(|p| p.required) {
        if !required.iter().any(|r| r == prop.name) {
            required.push(json!(prop.name));
        }
    }

    properties.insert(
        "bot_token".to_string(),
        json!({
            "type": "string",
            "description": "Optional Discord bot token for a stateless call. Defaults to the agent's configured token."
        }),
    );
    if !spec.verb.is_read_only() {
        properties.insert(
            "reason".to_string(),
            json!({
                "type": "string",
                "description": "Audit-log reason recorded with the action (up to 512 characters)."
            }),
        );
    }

    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

/// The `{name}` segments of a path template, in order.
pub fn path_placeholders(path: &'static [&'static str]) -> Vec<&'static str> {
    path.iter()
        .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
        .collect()
}

/// Path placeholders whose value must be a decimal snowflake.
///
/// `emoji` is deliberately absent: a custom emoji is `name:id`, and a unicode
/// emoji is not an id at all. Everything else in a path *is* an id, and a typo
/// must be reported before a request is issued rather than as a Discord 404.
const ID_PLACEHOLDERS: &[&str] = &[
    "channel_id",
    "event_id",
    "guild_id",
    "message_id",
    "overwrite_id",
    "role_id",
    "user_id",
    "webhook_id",
];

/// Substitute the path placeholders and append the query string.
///
/// Every substituted segment is percent-encoded: custom emoji are `name:id`,
/// which would otherwise be read as a path separator.
pub fn build_path(spec: &ToolSpec, args: &Map<String, Value>) -> Result<String, String> {
    let mut segments: Vec<String> = Vec::with_capacity(spec.path.len());
    for segment in spec.path {
        match segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            Some(key) => {
                let raw = args
                    .get(key)
                    .and_then(scalar)
                    .ok_or_else(|| format!("'{key}' is required"))?;
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Err(format!("'{key}' cannot be empty"));
                }
                if ID_PLACEHOLDERS.contains(&key) && !trimmed.chars().all(|c| c.is_ascii_digit()) {
                    return Err(format!(
                        "'{key}' must be a decimal Discord snowflake, got {trimmed:?}"
                    ));
                }
                segments.push(percent_encode(trimmed, 128));
            }
            None => segments.push((*segment).to_string()),
        }
    }

    let mut path = segments.join("/");
    let query: Vec<String> = spec
        .query
        .iter()
        .filter_map(|key| {
            let value = args.get(*key)?;
            if value.is_null() {
                return None;
            }
            let rendered = match value {
                Value::String(s) => s.trim().to_string(),
                other => other.to_string(),
            };
            if rendered.is_empty() {
                return None;
            }
            Some(format!("{}={}", key, percent_encode(&rendered, 512)))
        })
        .collect();
    if !query.is_empty() {
        path.push('?');
        path.push_str(&query.join("&"));
    }
    Ok(path)
}

/// Build the JSON body for one call.
pub fn build_body(spec: &ToolSpec, args: &Map<String, Value>) -> Result<Value, String> {
    match spec.body {
        Body::None => Ok(Value::Null),
        Body::Fields(fields) => {
            let mut body = Map::new();
            for field in fields {
                if let Some(value) = args.get(*field)
                    && !value.is_null()
                {
                    body.insert((*field).to_string(), value.clone());
                }
            }
            Ok(Value::Object(body))
        }
        Body::Content => {
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if text.is_empty() {
                return Err("'text' is required and cannot be empty".to_string());
            }
            if text.chars().count() > MAX_MESSAGE_CHARS {
                return Err(format!(
                    "'text' exceeds Discord's {MAX_MESSAGE_CHARS}-character limit"
                ));
            }
            Ok(json!({ "content": text }))
        }
        Body::Embed => {
            let description = args
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if description.is_empty() {
                return Err("'description' is required and cannot be empty".to_string());
            }
            if description.chars().count() > MAX_EMBED_DESCRIPTION_CHARS {
                return Err(format!(
                    "'description' exceeds Discord's {MAX_EMBED_DESCRIPTION_CHARS}-character limit"
                ));
            }
            if let Some(title) = args.get("title").and_then(Value::as_str)
                && title.chars().count() > MAX_EMBED_TITLE_CHARS
            {
                return Err(format!(
                    "'title' exceeds Discord's {MAX_EMBED_TITLE_CHARS}-character limit"
                ));
            }
            if let Some(url) = args.get("url").and_then(Value::as_str)
                && !url.starts_with("http://")
                && !url.starts_with("https://")
            {
                return Err("'url' must start with http:// or https://".to_string());
            }
            if let Some(color) = args.get("color").and_then(Value::as_i64)
                && !(0..=0x00ff_ffff).contains(&color)
            {
                return Err("'color' must be an RGB value between 0 and 16777215".to_string());
            }
            let mut embed = Map::new();
            embed.insert("description".to_string(), json!(description));
            if let Some(title) = args.get("title").and_then(Value::as_str) {
                embed.insert("title".to_string(), json!(title));
            }
            if let Some(url) = args.get("url").and_then(Value::as_str) {
                embed.insert("url".to_string(), json!(url));
            }
            if let Some(color) = args.get("color").and_then(Value::as_i64) {
                embed.insert("color".to_string(), json!(color));
            }
            Ok(json!({ "embeds": [embed] }))
        }
        Body::BulkDelete => {
            let ids = args
                .get("message_ids")
                .and_then(Value::as_array)
                .ok_or_else(|| "'message_ids' is required".to_string())?;
            let messages: Vec<Value> = ids
                .iter()
                .filter_map(scalar)
                .map(|id| json!(percent_encode(id.trim(), 32)))
                .collect();
            if messages.len() < 2 {
                return Err("'message_ids' must contain at least 2 message ids".to_string());
            }
            if messages.len() > 100 {
                return Err("'message_ids' accepts at most 100 message ids".to_string());
            }
            Ok(json!({ "messages": messages }))
        }
        Body::Overwrite => {
            let overwrite_type = args
                .get("type")
                .and_then(Value::as_i64)
                .ok_or_else(|| "'type' is required (0 for role, 1 for member)".to_string())?;
            if overwrite_type != 0 && overwrite_type != 1 {
                return Err("'type' must be 0 (role) or 1 (member)".to_string());
            }
            let mut body = Map::new();
            body.insert("type".to_string(), json!(overwrite_type));
            for field in ["allow", "deny"] {
                let value = args
                    .get(field)
                    .and_then(Value::as_str)
                    .unwrap_or("0")
                    .trim()
                    .to_string();
                body.insert(field.to_string(), json!(value));
            }
            Ok(Value::Object(body))
        }
        Body::WebhookMessage => {
            let mut body = Map::new();
            for field in [
                "content",
                "username",
                "avatar_url",
                "tts",
                "embeds",
                "thread_name",
            ] {
                if let Some(value) = args.get(field)
                    && !value.is_null()
                {
                    body.insert(field.to_string(), value.clone());
                }
            }
            if body.is_empty() {
                return Err("provide 'content' or 'embeds'".to_string());
            }
            Ok(Value::Object(body))
        }
    }
}

/// A scalar argument rendered as a string: snowflakes are often typed by the
/// model as numbers, and Discord accepts either in a path segment.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn tool_names_are_unique_and_prefixed() {
        let mut seen = std::collections::HashSet::new();
        for spec in TOOLS {
            assert!(
                spec.name.starts_with("discord_"),
                "{} must be namespaced",
                spec.name
            );
            assert!(seen.insert(spec.name), "duplicate tool {}", spec.name);
            assert!(!spec.description.is_empty());
        }
    }

    /// The generated schema is the only thing the model sees, so it must agree
    /// with the routing table exactly. This is the guard against drift.
    #[test]
    fn every_schema_matches_its_route() {
        for spec in TOOLS {
            let schema = schema(spec);
            assert_eq!(schema["type"], "object", "{}", spec.name);
            let properties = schema["properties"].as_object().unwrap();
            let required: Vec<&str> = schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();

            // Every path placeholder is published and required.
            for placeholder in path_placeholders(spec.path) {
                assert!(
                    properties.contains_key(placeholder),
                    "{}: path placeholder {placeholder} is not published",
                    spec.name
                );
                assert!(
                    required.contains(&placeholder),
                    "{}: path placeholder {placeholder} is not required",
                    spec.name
                );
            }

            // Everything the body copies, and every query parameter, is published
            // and has a description of its own.
            let forwarded: Vec<&str> = match spec.body {
                Body::Fields(fields) => fields.to_vec(),
                Body::Content => vec!["text"],
                Body::Embed => vec!["title", "description", "url", "color"],
                Body::BulkDelete => vec!["message_ids"],
                Body::Overwrite => vec!["type", "allow", "deny"],
                Body::WebhookMessage => vec![
                    "content",
                    "username",
                    "avatar_url",
                    "tts",
                    "embeds",
                    "thread_name",
                ],
                Body::None => Vec::new(),
            };
            for name in forwarded.iter().chain(spec.query.iter()) {
                assert!(
                    properties.contains_key(*name),
                    "{}: forwards {name} but does not publish it",
                    spec.name
                );
                assert!(
                    spec.props.iter().any(|p| p.name == *name),
                    "{}: forwards {name} without a declared property (no description)",
                    spec.name
                );
            }

            // A required property must exist.
            for name in &required {
                assert!(
                    properties.contains_key(*name),
                    "{}: required {name} is not a property",
                    spec.name
                );
            }

            assert!(
                properties.contains_key("bot_token"),
                "{}: every tool accepts a per-call token",
                spec.name
            );
            assert_eq!(
                properties.contains_key("reason"),
                !spec.verb.is_read_only(),
                "{}: only mutating tools accept an audit reason",
                spec.name
            );
        }
    }

    /// A tool that changes state must not claim to be read-only, and a GET must
    /// never be marked destructive: the host may use these flags for consent.
    #[test]
    fn destructive_flags_agree_with_the_verb() {
        for spec in TOOLS {
            if spec.verb.is_read_only() {
                assert!(
                    !spec.destructive,
                    "{} is a GET and cannot be destructive",
                    spec.name
                );
            }
            assert!(!spec.path.is_empty(), "{} needs a path", spec.name);
        }
    }

    #[test]
    fn builds_paths_and_query_strings() {
        let ban = find("discord_ban_member").unwrap();
        assert_eq!(
            build_path(
                ban,
                &args(&[("guild_id", json!("1")), ("user_id", json!(2))])
            )
            .unwrap(),
            "guilds/1/bans/2"
        );

        let reactions = find("discord_list_reactions").unwrap();
        assert_eq!(
            build_path(
                reactions,
                &args(&[
                    ("channel_id", json!("10")),
                    ("message_id", json!("20")),
                    ("emoji", json!("wave:123")),
                    ("limit", json!(50)),
                    ("after", json!("")),
                ])
            )
            .unwrap(),
            "channels/10/messages/20/reactions/wave%3A123?limit=50"
        );

        let members = find("discord_list_guild_members").unwrap();
        assert_eq!(
            build_path(
                members,
                &args(&[("guild_id", json!("1")), ("limit", json!(1000))])
            )
            .unwrap(),
            "guilds/1/members?limit=1000"
        );

        assert!(build_path(ban, &args(&[("guild_id", json!("1"))])).is_err());
        assert!(
            build_path(
                ban,
                &args(&[("guild_id", json!("  ")), ("user_id", json!("2"))])
            )
            .is_err()
        );
    }

    #[test]
    fn builds_bodies_for_each_shape() {
        let content = find("discord_edit_message").unwrap();
        assert_eq!(
            build_body(content, &args(&[("text", json!("hi"))])).unwrap(),
            json!({ "content": "hi" })
        );
        assert!(build_body(content, &args(&[("text", json!("  "))])).is_err());

        let bulk = find("discord_bulk_delete_messages").unwrap();
        assert_eq!(
            build_body(bulk, &args(&[("message_ids", json!(["1", "2"]))])).unwrap(),
            json!({ "messages": ["1", "2"] })
        );
        assert!(build_body(bulk, &args(&[("message_ids", json!(["1"]))])).is_err());

        let overwrite = find("discord_set_channel_permissions").unwrap();
        assert_eq!(
            build_body(
                overwrite,
                &args(&[("type", json!(0)), ("allow", json!("1024"))])
            )
            .unwrap(),
            json!({ "type": 0, "allow": "1024", "deny": "0" })
        );
        assert!(build_body(overwrite, &args(&[("type", json!(7))])).is_err());

        let kick = find("discord_kick_member").unwrap();
        assert_eq!(
            build_body(kick, &args(&[])).unwrap(),
            Value::Null,
            "a DELETE carries no body"
        );

        let embed = find("discord_send_embed").unwrap();
        assert_eq!(
            build_body(
                embed,
                &args(&[
                    ("description", json!("body")),
                    ("title", json!("t")),
                    ("color", json!(42)),
                ])
            )
            .unwrap(),
            json!({ "embeds": [{ "description": "body", "title": "t", "color": 42 }] })
        );

        let webhook = find("discord_execute_webhook").unwrap();
        assert_eq!(
            build_body(webhook, &args(&[("content", json!("hello"))])).unwrap(),
            json!({ "content": "hello" })
        );
        assert!(build_body(webhook, &args(&[])).is_err());
    }

    #[test]
    fn moderation_tools_forward_their_audit_reason() {
        for name in [
            "discord_ban_member",
            "discord_kick_member",
            "discord_begin_prune",
            "discord_delete_role",
        ] {
            let spec = find(name).unwrap();
            assert!(!spec.verb.is_read_only(), "{name} mutates");
            assert!(
                schema(spec)["properties"]
                    .as_object()
                    .unwrap()
                    .contains_key("reason"),
                "{name} must accept an audit reason"
            );
        }
    }
}
