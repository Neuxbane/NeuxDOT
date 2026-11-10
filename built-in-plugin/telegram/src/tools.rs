//! The Telegram Bot API tool surface, as data.
//!
//! Every row is `(tool name, Bot API method, declared parameters)`. The JSON
//! schema is **generated** from the row, so a parameter that is forwarded can
//! never be missing from what the model sees, and a method name can never drift
//! from the name the dispatcher sends. `tests` below enforce both.
//!
//! Two tools are not a plain method call and live in `server.rs`:
//! `telegram_get_updates` (the live poller owns the offset) and `telegram_api`
//! (the guarded passthrough). `telegram_download_file` is absent on purpose:
//! the Bot API returns a file path, and streaming binary through a text tool is
//! worse than useless to a model.

use serde_json::{Map, Value, json};

/// Telegram's hard text limits.
pub const MAX_MESSAGE_CHARS: usize = 4096;
pub const MAX_CAPTION_CHARS: usize = 1024;

/// The JSON type of a declared parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Str,
    Int,
    Bool,
    Float,
    Obj,
    Arr,
    ArrStr,
    ArrInt,
}

impl Kind {
    fn schema(self) -> Value {
        match self {
            Kind::Str => json!({ "type": "string" }),
            Kind::Int => json!({ "type": "integer" }),
            Kind::Bool => json!({ "type": "boolean" }),
            Kind::Float => json!({ "type": "number" }),
            Kind::Obj => json!({ "type": "object" }),
            Kind::Arr => json!({ "type": "array" }),
            Kind::ArrStr => json!({ "type": "array", "items": { "type": "string" } }),
            Kind::ArrInt => json!({ "type": "array", "items": { "type": "integer" } }),
        }
    }

    fn accepts(self, value: &Value) -> bool {
        match self {
            Kind::Str => value.is_string(),
            // A snowflake-style id is sometimes typed as a number by the model;
            // Telegram accepts either in a chat id.
            Kind::Int => value.is_i64() || value.is_u64() || value.is_string(),
            Kind::Bool => value.is_boolean(),
            Kind::Float => value.is_number(),
            Kind::Obj => value.is_object(),
            Kind::Arr | Kind::ArrStr | Kind::ArrInt => value.is_array(),
        }
    }
}

pub struct Param {
    pub name: &'static str,
    pub kind: Kind,
    pub required: bool,
    pub description: &'static str,
}

pub const fn p(name: &'static str, kind: Kind, required: bool, description: &'static str) -> Param {
    Param {
        name,
        kind,
        required,
        description,
    }
}

pub struct ToolSpec {
    pub name: &'static str,
    /// The Bot API method this tool calls, e.g. `sendMessage`.
    pub method: &'static str,
    pub description: &'static str,
    pub params: &'static [Param],
    /// A `GET`-equivalent: it cannot change state.
    pub read_only: bool,
    /// Removes or destroys something; shown as a consent hint by the host.
    pub destructive: bool,
}

const fn tool(
    name: &'static str,
    method: &'static str,
    description: &'static str,
    params: &'static [Param],
    read_only: bool,
    destructive: bool,
) -> ToolSpec {
    ToolSpec {
        name,
        method,
        description,
        params,
        read_only,
        destructive,
    }
}

// Frequently repeated parameters.
const CHAT_ID: Param = p(
    "chat_id",
    Kind::Str,
    true,
    "Chat id (may be negative for groups and channels) or @channelusername. Defaults to the agent's configured chat when omitted.",
);
const MESSAGE_ID: Param = p("message_id", Kind::Int, true, "Message id inside the chat.");
const USER_ID: Param = p("user_id", Kind::Int, true, "Target user id.");
const PARSE_MODE: Param = p(
    "parse_mode",
    Kind::Str,
    false,
    "Formatting mode: HTML or MarkdownV2. Omit for plain text.",
);
const NOTIFY: Param = p(
    "disable_notification",
    Kind::Bool,
    false,
    "Deliver silently without a notification sound.",
);
const PROTECT: Param = p(
    "protect_content",
    Kind::Bool,
    false,
    "Prevent forwarding and saving of this message.",
);
const REPLY_MARKUP: Param = p(
    "reply_markup",
    Kind::Obj,
    false,
    "Inline keyboard, reply keyboard or force-reply object, as raw JSON.",
);
const THREAD_ID: Param = p(
    "message_thread_id",
    Kind::Int,
    false,
    "Forum topic id to post into (use the topic's message id).",
);

// Repeated, fully described parameter lists.
const SEND_TEXT: &[Param] = &[
    CHAT_ID,
    p(
        "text",
        Kind::Str,
        true,
        "Message text (1-4096 characters after entities parsing).",
    ),
    PARSE_MODE,
    NOTIFY,
    PROTECT,
    REPLY_MARKUP,
    THREAD_ID,
];

const EDIT_TEXT: &[Param] = &[
    p(
        "chat_id",
        Kind::Str,
        false,
        "Chat id. Required unless inline_message_id is given.",
    ),
    p(
        "message_id",
        Kind::Int,
        false,
        "Message id. Required unless inline_message_id is given.",
    ),
    p(
        "inline_message_id",
        Kind::Str,
        false,
        "Identifier of an inline message, for messages sent via the bot inline mode.",
    ),
    p("text", Kind::Str, true, "New text (1-4096 characters)."),
    PARSE_MODE,
    REPLY_MARKUP,
];

const CHAT_TARGET: &[Param] = &[CHAT_ID];

/// The full table. Ordered by area, mirroring the Bot API reference.
pub const TOOLS: &[ToolSpec] = &[
    // ---- Bot identity and profile --------------------------------------
    tool(
        "telegram_get_me",
        "getMe",
        "Return the bot's own user object",
        &[],
        true,
        false,
    ),
    tool(
        "telegram_log_out",
        "logOut",
        "Log the bot out of the local Bot API server, so it can switch to a different one",
        &[],
        false,
        true,
    ),
    tool(
        "telegram_close",
        "close",
        "Close the bot instance on the local Bot API server before moving it to another server",
        &[],
        false,
        true,
    ),
    tool(
        "telegram_get_my_name",
        "getMyName",
        "Return the bot's current display name",
        &[p(
            "language_code",
            Kind::Str,
            false,
            "Two-letter language code",
        )],
        true,
        false,
    ),
    tool(
        "telegram_set_my_name",
        "setMyName",
        "Change the bot's display name",
        &[
            p(
                "name",
                Kind::Str,
                false,
                "New name (0-64 characters), empty to remove",
            ),
            p(
                "language_code",
                Kind::Str,
                false,
                "Two-letter language code",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_get_my_description",
        "getMyDescription",
        "Return the bot's description, shown in an empty private chat",
        &[p(
            "language_code",
            Kind::Str,
            false,
            "Two-letter language code",
        )],
        true,
        false,
    ),
    tool(
        "telegram_set_my_description",
        "setMyDescription",
        "Change the bot's description",
        &[
            p(
                "description",
                Kind::Str,
                false,
                "New description (0-512 characters)",
            ),
            p(
                "language_code",
                Kind::Str,
                false,
                "Two-letter language code",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_get_my_short_description",
        "getMyShortDescription",
        "Return the bot's short description, shown on its profile and in shares",
        &[p(
            "language_code",
            Kind::Str,
            false,
            "Two-letter language code",
        )],
        true,
        false,
    ),
    tool(
        "telegram_set_my_short_description",
        "setMyShortDescription",
        "Change the bot's short description",
        &[
            p(
                "short_description",
                Kind::Str,
                false,
                "New short description (0-120 characters)",
            ),
            p(
                "language_code",
                Kind::Str,
                false,
                "Two-letter language code",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_get_my_default_administrator_rights",
        "getMyDefaultAdministratorRights",
        "Return the default administrator rights the bot requests when added to a group or channel",
        &[p(
            "for_channels",
            Kind::Bool,
            false,
            "Return the channel defaults instead of the group defaults",
        )],
        true,
        false,
    ),
    tool(
        "telegram_set_my_default_administrator_rights",
        "setMyDefaultAdministratorRights",
        "Change the default administrator rights the bot requests",
        &[
            p("rights", Kind::Obj, false, "ChatAdministratorRights object"),
            p(
                "for_channels",
                Kind::Bool,
                false,
                "Apply to channels instead of groups",
            ),
        ],
        false,
        false,
    ),
    // ---- Commands and menus --------------------------------------------
    tool(
        "telegram_get_my_commands",
        "getMyCommands",
        "List the bot's registered slash commands",
        &[
            p(
                "scope",
                Kind::Obj,
                false,
                "BotCommandScope object, e.g. {\"type\": \"all_private_chats\"}",
            ),
            p(
                "language_code",
                Kind::Str,
                false,
                "Two-letter language code",
            ),
        ],
        true,
        false,
    ),
    tool(
        "telegram_set_my_commands",
        "setMyCommands",
        "Replace the bot's slash-command list, optionally for one scope or language",
        &[
            p(
                "commands",
                Kind::Arr,
                true,
                "Array of {\"command\": \"name\", \"description\": \"...\"} objects. 1-100 entries, 32 chars max per command.",
            ),
            p("scope", Kind::Obj, false, "BotCommandScope object"),
            p(
                "language_code",
                Kind::Str,
                false,
                "Two-letter language code",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_delete_my_commands",
        "deleteMyCommands",
        "Remove the bot's slash commands, optionally for one scope or language",
        &[
            p("scope", Kind::Obj, false, "BotCommandScope object"),
            p(
                "language_code",
                Kind::Str,
                false,
                "Two-letter language code",
            ),
        ],
        false,
        true,
    ),
    tool(
        "telegram_get_chat_menu_button",
        "getChatMenuButton",
        "Return the bot's menu button in a private chat or a group's default",
        &[p(
            "chat_id",
            Kind::Int,
            false,
            "Chat id; omit for the default button",
        )],
        true,
        false,
    ),
    tool(
        "telegram_set_chat_menu_button",
        "setChatMenuButton",
        "Change the bot's menu button in a chat",
        &[
            p(
                "chat_id",
                Kind::Int,
                false,
                "Chat id; omit for the default button",
            ),
            p(
                "menu_button",
                Kind::Obj,
                false,
                "MenuButton object, e.g. {\"type\": \"web_app\", \"text\": \"Open\", \"web_app\": {\"url\": \"https://...\"}}",
            ),
        ],
        false,
        false,
    ),
    // ---- Update delivery ------------------------------------------------
    tool(
        "telegram_set_webhook",
        "setWebhook",
        "Deliver updates to an HTTPS URL instead of polling",
        &[
            p(
                "url",
                Kind::Str,
                true,
                "HTTPS URL that receives updates. An empty string removes it.",
            ),
            p(
                "secret_token",
                Kind::Str,
                false,
                "1-256 characters; echoed back in X-Telegram-Bot-Api-Secret-Token",
            ),
            p(
                "max_connections",
                Kind::Int,
                false,
                "1-100 parallel connections, default 40",
            ),
            p(
                "allowed_updates",
                Kind::ArrStr,
                false,
                "Update types to deliver, e.g. [\"message\", \"callback_query\"]",
            ),
            p(
                "drop_pending_updates",
                Kind::Bool,
                false,
                "Discard updates queued before the change",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_delete_webhook",
        "deleteWebhook",
        "Stop delivering updates to a webhook, so getUpdates can be used again",
        &[p(
            "drop_pending_updates",
            Kind::Bool,
            false,
            "Discard updates queued while the webhook was active",
        )],
        false,
        true,
    ),
    tool(
        "telegram_get_webhook_info",
        "getWebhookInfo",
        "Return the current webhook URL, pending update count and last error",
        &[],
        true,
        false,
    ),
    // ---- Sending messages ----------------------------------------------
    tool(
        "telegram_send_message",
        "sendMessage",
        "Send a text message to a chat",
        SEND_TEXT,
        false,
        false,
    ),
    tool(
        "telegram_forward_message",
        "forwardMessage",
        "Forward one message from one chat to another without a forward header option",
        &[
            p(
                "chat_id",
                Kind::Str,
                true,
                "Destination chat id or @username",
            ),
            p(
                "from_chat_id",
                Kind::Str,
                true,
                "Source chat id or @username. The bot must be able to read it.",
            ),
            MESSAGE_ID,
            THREAD_ID,
            NOTIFY,
            PROTECT,
        ],
        false,
        false,
    ),
    tool(
        "telegram_copy_message",
        "copyMessage",
        "Copy a message into another chat without a forward header, optionally with a new caption",
        &[
            p(
                "chat_id",
                Kind::Str,
                true,
                "Destination chat id or @username",
            ),
            p(
                "from_chat_id",
                Kind::Str,
                true,
                "Source chat id or @username",
            ),
            MESSAGE_ID,
            p(
                "caption",
                Kind::Str,
                false,
                "New caption instead of the original one",
            ),
            PARSE_MODE,
            THREAD_ID,
            NOTIFY,
            PROTECT,
            REPLY_MARKUP,
        ],
        false,
        false,
    ),
    tool(
        "telegram_send_photo",
        "sendPhoto",
        "Send a photo by HTTPS URL or Telegram file_id",
        &[
            CHAT_ID,
            p(
                "photo",
                Kind::Str,
                true,
                "HTTPS URL or an existing Telegram file_id",
            ),
            p("caption", Kind::Str, false, "Caption (0-1024 characters)"),
            PARSE_MODE,
            NOTIFY,
            PROTECT,
            REPLY_MARKUP,
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_send_audio",
        "sendAudio",
        "Send an audio file by HTTPS URL or file_id",
        &[
            CHAT_ID,
            p(
                "audio",
                Kind::Str,
                true,
                "HTTPS URL or file_id. Must be .MP3 or .M4A for the music player.",
            ),
            p("caption", Kind::Str, false, "Caption (0-1024 characters)"),
            p("duration", Kind::Int, false, "Duration in seconds"),
            p("performer", Kind::Str, false, "Performer name"),
            p("title", Kind::Str, false, "Track title"),
            PARSE_MODE,
            NOTIFY,
            PROTECT,
            REPLY_MARKUP,
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_send_document",
        "sendDocument",
        "Send a general file by HTTPS URL or file_id",
        &[
            CHAT_ID,
            p("document", Kind::Str, true, "HTTPS URL or file_id"),
            p("caption", Kind::Str, false, "Caption (0-1024 characters)"),
            p(
                "disable_content_type_detection",
                Kind::Bool,
                false,
                "Send as application/octet-stream",
            ),
            PARSE_MODE,
            NOTIFY,
            PROTECT,
            REPLY_MARKUP,
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_send_video",
        "sendVideo",
        "Send a video by HTTPS URL or file_id",
        &[
            CHAT_ID,
            p("video", Kind::Str, true, "HTTPS URL or file_id"),
            p("caption", Kind::Str, false, "Caption (0-1024 characters)"),
            p("duration", Kind::Int, false, "Duration in seconds"),
            p("width", Kind::Int, false, "Video width"),
            p("height", Kind::Int, false, "Video height"),
            PARSE_MODE,
            NOTIFY,
            PROTECT,
            REPLY_MARKUP,
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_send_animation",
        "sendAnimation",
        "Send a GIF or video without sound by HTTPS URL or file_id",
        &[
            CHAT_ID,
            p("animation", Kind::Str, true, "HTTPS URL or file_id"),
            p("caption", Kind::Str, false, "Caption (0-1024 characters)"),
            p("duration", Kind::Int, false, "Duration in seconds"),
            PARSE_MODE,
            NOTIFY,
            PROTECT,
            REPLY_MARKUP,
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_send_voice",
        "sendVoice",
        "Send a voice message by HTTPS URL or file_id",
        &[
            CHAT_ID,
            p(
                "voice",
                Kind::Str,
                true,
                "HTTPS URL or file_id. Must be OGG/OPUS for the waveform.",
            ),
            p("caption", Kind::Str, false, "Caption (0-1024 characters)"),
            p("duration", Kind::Int, false, "Duration in seconds"),
            PARSE_MODE,
            NOTIFY,
            PROTECT,
            REPLY_MARKUP,
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_send_video_note",
        "sendVideoNote",
        "Send a round video message by HTTPS URL or file_id",
        &[
            CHAT_ID,
            p("video_note", Kind::Str, true, "HTTPS URL or file_id"),
            p("duration", Kind::Int, false, "Duration in seconds"),
            p("length", Kind::Int, false, "Diameter of the video note"),
            NOTIFY,
            PROTECT,
            REPLY_MARKUP,
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_send_media_group",
        "sendMediaGroup",
        "Send 2-10 photos or videos as one album",
        &[
            CHAT_ID,
            p(
                "media",
                Kind::Arr,
                true,
                "Array of 2-10 InputMedia objects, e.g. {\"type\": \"photo\", \"media\": \"https://...\", \"caption\": \"...\"}",
            ),
            NOTIFY,
            PROTECT,
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_send_location",
        "sendLocation",
        "Send a point on the map",
        &[
            CHAT_ID,
            p("latitude", Kind::Float, true, "Latitude"),
            p("longitude", Kind::Float, true, "Longitude"),
            p(
                "horizontal_accuracy",
                Kind::Float,
                false,
                "Accuracy radius in metres (0-1500)",
            ),
            p(
                "live_period",
                Kind::Int,
                false,
                "Seconds the location updates for (60-86400)",
            ),
            NOTIFY,
            PROTECT,
            REPLY_MARKUP,
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_send_venue",
        "sendVenue",
        "Send a venue with a title and address",
        &[
            CHAT_ID,
            p("latitude", Kind::Float, true, "Latitude"),
            p("longitude", Kind::Float, true, "Longitude"),
            p("title", Kind::Str, true, "Venue name"),
            p("address", Kind::Str, true, "Venue address"),
            p("foursquare_id", Kind::Str, false, "Foursquare place id"),
            PARSE_MODE,
            NOTIFY,
            PROTECT,
            REPLY_MARKUP,
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_send_contact",
        "sendContact",
        "Send a phone contact",
        &[
            CHAT_ID,
            p("phone_number", Kind::Str, true, "Contact phone number"),
            p("first_name", Kind::Str, true, "Contact first name"),
            p("last_name", Kind::Str, false, "Contact last name"),
            p("vcard", Kind::Str, false, "Additional vCard data"),
            PARSE_MODE,
            NOTIFY,
            PROTECT,
            REPLY_MARKUP,
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_send_poll",
        "sendPoll",
        "Send a native poll or quiz",
        &[
            CHAT_ID,
            p(
                "question",
                Kind::Str,
                true,
                "Poll question (1-300 characters)",
            ),
            p(
                "options",
                Kind::Arr,
                true,
                "Array of 2-12 {\"text\": \"...\"} option objects",
            ),
            p(
                "is_anonymous",
                Kind::Bool,
                false,
                "Hide who voted (default true)",
            ),
            p(
                "type",
                Kind::Str,
                false,
                "\"regular\" (default) or \"quiz\"",
            ),
            p(
                "allows_multiple_answers",
                Kind::Bool,
                false,
                "Let voters pick several options",
            ),
            p(
                "correct_option_id",
                Kind::Int,
                false,
                "0-based index of the right answer (quiz only)",
            ),
            p(
                "explanation",
                Kind::Str,
                false,
                "Explanation shown for quiz answers (0-200 characters)",
            ),
            p(
                "open_period",
                Kind::Int,
                false,
                "Seconds the poll stays open (5-600)",
            ),
            PARSE_MODE,
            NOTIFY,
            PROTECT,
            REPLY_MARKUP,
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_send_dice",
        "sendDice",
        "Send an animated dice that draws a random value",
        &[
            CHAT_ID,
            p(
                "emoji",
                Kind::Str,
                false,
                "Dice style: 🎲 (default), 🎯, 🏀, ⚽, 🎰 or 🎳",
            ),
            NOTIFY,
            PROTECT,
            REPLY_MARKUP,
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_send_sticker",
        "sendSticker",
        "Send a sticker by HTTPS URL or file_id",
        &[
            CHAT_ID,
            p(
                "sticker",
                Kind::Str,
                true,
                "HTTPS URL (WEBP/WEBM/TGS) or file_id",
            ),
            NOTIFY,
            PROTECT,
            REPLY_MARKUP,
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_send_chat_action",
        "sendChatAction",
        "Show a status such as typing or uploading in the chat for about five seconds",
        &[
            CHAT_ID,
            p(
                "action",
                Kind::Str,
                true,
                "typing, upload_photo, record_video, upload_video, record_voice, upload_voice, upload_document, choose_sticker, find_location, record_video_note, upload_video_note",
            ),
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_set_message_reaction",
        "setMessageReaction",
        "React to a message, replacing the bot's existing reactions",
        &[
            CHAT_ID,
            MESSAGE_ID,
            p(
                "reaction",
                Kind::Arr,
                false,
                "Array of reaction types, e.g. [{\"type\": \"emoji\", \"emoji\": \"👍\"}]. Empty removes the reaction.",
            ),
            p(
                "is_big",
                Kind::Bool,
                false,
                "Play a large reaction animation",
            ),
        ],
        false,
        false,
    ),
    // ---- Editing and deleting -------------------------------------------
    tool(
        "telegram_edit_message_text",
        "editMessageText",
        "Replace the text of a message the bot sent",
        EDIT_TEXT,
        false,
        false,
    ),
    tool(
        "telegram_edit_message_caption",
        "editMessageCaption",
        "Replace the caption of a media message the bot sent",
        &[
            p(
                "chat_id",
                Kind::Str,
                false,
                "Chat id, required unless inline_message_id is given",
            ),
            p(
                "message_id",
                Kind::Int,
                false,
                "Message id, required unless inline_message_id is given",
            ),
            p(
                "inline_message_id",
                Kind::Str,
                false,
                "Inline message identifier",
            ),
            p(
                "caption",
                Kind::Str,
                false,
                "New caption (0-1024 characters)",
            ),
            PARSE_MODE,
            REPLY_MARKUP,
        ],
        false,
        false,
    ),
    tool(
        "telegram_edit_message_media",
        "editMessageMedia",
        "Replace a media message's content with different media",
        &[
            p(
                "chat_id",
                Kind::Str,
                false,
                "Chat id, required unless inline_message_id is given",
            ),
            p(
                "message_id",
                Kind::Int,
                false,
                "Message id, required unless inline_message_id is given",
            ),
            p(
                "inline_message_id",
                Kind::Str,
                false,
                "Inline message identifier",
            ),
            p(
                "media",
                Kind::Obj,
                true,
                "InputMedia object, e.g. {\"type\": \"photo\", \"media\": \"https://...\"}",
            ),
            REPLY_MARKUP,
        ],
        false,
        false,
    ),
    tool(
        "telegram_edit_message_reply_markup",
        "editMessageReplyMarkup",
        "Replace only the inline keyboard of a message the bot sent",
        &[
            p(
                "chat_id",
                Kind::Str,
                false,
                "Chat id, required unless inline_message_id is given",
            ),
            p(
                "message_id",
                Kind::Int,
                false,
                "Message id, required unless inline_message_id is given",
            ),
            p(
                "inline_message_id",
                Kind::Str,
                false,
                "Inline message identifier",
            ),
            REPLY_MARKUP,
        ],
        false,
        false,
    ),
    tool(
        "telegram_edit_message_live_location",
        "editMessageLiveLocation",
        "Move a live location the bot is sharing",
        &[
            p(
                "chat_id",
                Kind::Str,
                false,
                "Chat id, required unless inline_message_id is given",
            ),
            p(
                "message_id",
                Kind::Int,
                false,
                "Message id, required unless inline_message_id is given",
            ),
            p(
                "inline_message_id",
                Kind::Str,
                false,
                "Inline message identifier",
            ),
            p("latitude", Kind::Float, true, "New latitude"),
            p("longitude", Kind::Float, true, "New longitude"),
            p(
                "horizontal_accuracy",
                Kind::Float,
                false,
                "Accuracy radius in metres",
            ),
            REPLY_MARKUP,
        ],
        false,
        false,
    ),
    tool(
        "telegram_stop_message_live_location",
        "stopMessageLiveLocation",
        "Stop updating a live location the bot is sharing",
        &[
            p(
                "chat_id",
                Kind::Str,
                false,
                "Chat id, required unless inline_message_id is given",
            ),
            p(
                "message_id",
                Kind::Int,
                false,
                "Message id, required unless inline_message_id is given",
            ),
            p(
                "inline_message_id",
                Kind::Str,
                false,
                "Inline message identifier",
            ),
            REPLY_MARKUP,
        ],
        false,
        false,
    ),
    tool(
        "telegram_delete_message",
        "deleteMessage",
        "Delete one message. A bot may delete its own messages and, in groups, other recent ones as an admin.",
        &[CHAT_ID, MESSAGE_ID],
        false,
        true,
    ),
    tool(
        "telegram_delete_messages",
        "deleteMessages",
        "Delete 1-100 messages at once in a chat",
        &[
            CHAT_ID,
            p("message_ids", Kind::ArrInt, true, "1-100 message ids"),
        ],
        false,
        true,
    ),
    // ---- Chat information ------------------------------------------------
    tool(
        "telegram_get_chat",
        "getChat",
        "Return a chat's settings, pinned message, permissions and linked chat",
        CHAT_TARGET,
        true,
        false,
    ),
    tool(
        "telegram_get_chat_administrators",
        "getChatAdministrators",
        "List the administrators of a chat, with their rights",
        CHAT_TARGET,
        true,
        false,
    ),
    tool(
        "telegram_get_chat_member_count",
        "getChatMemberCount",
        "Return how many members a chat has",
        CHAT_TARGET,
        true,
        false,
    ),
    tool(
        "telegram_get_chat_member",
        "getChatMember",
        "Return one member's status and rights in a chat",
        &[CHAT_ID, USER_ID],
        true,
        false,
    ),
    tool(
        "telegram_get_user_profile_photos",
        "getUserProfilePhotos",
        "List a user's profile photos",
        &[
            USER_ID,
            p(
                "offset",
                Kind::Int,
                false,
                "Index of the first photo to return",
            ),
            p("limit", Kind::Int, false, "1-100, default 100"),
        ],
        true,
        false,
    ),
    tool(
        "telegram_get_user_chat_boosts",
        "getUserChatBoosts",
        "List the boosts a user contributed to a chat",
        &[CHAT_ID, USER_ID],
        true,
        false,
    ),
    tool(
        "telegram_get_file",
        "getFile",
        "Resolve a file_id into a file path that can be downloaded, plus its size",
        &[p(
            "file_id",
            Kind::Str,
            true,
            "file_id from a message attachment",
        )],
        true,
        false,
    ),
    // ---- Chat settings ---------------------------------------------------
    tool(
        "telegram_set_chat_title",
        "setChatTitle",
        "Rename a group or channel",
        &[
            CHAT_ID,
            p("title", Kind::Str, true, "New title (1-128 characters)"),
        ],
        false,
        false,
    ),
    tool(
        "telegram_set_chat_description",
        "setChatDescription",
        "Change a group or channel description",
        &[
            CHAT_ID,
            p(
                "description",
                Kind::Str,
                false,
                "New description (0-255 characters)",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_set_chat_photo",
        "setChatPhoto",
        "Set a group or channel photo from an HTTPS URL",
        &[
            CHAT_ID,
            p(
                "photo",
                Kind::Str,
                true,
                "HTTPS URL of a JPEG. Uploading raw bytes is not supported.",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_delete_chat_photo",
        "deleteChatPhoto",
        "Remove a group or channel photo",
        CHAT_TARGET,
        false,
        true,
    ),
    tool(
        "telegram_set_chat_permissions",
        "setChatPermissions",
        "Set the default permissions for every non-administrator member",
        &[
            CHAT_ID,
            p(
                "permissions",
                Kind::Obj,
                true,
                "ChatPermissions object, e.g. {\"can_send_messages\": false}",
            ),
            p(
                "use_independent_chat_permissions",
                Kind::Bool,
                false,
                "Treat the supplied flags as the full set rather than inheriting the defaults",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_leave_chat",
        "leaveChat",
        "Make the bot leave a group, channel or forum",
        CHAT_TARGET,
        false,
        true,
    ),
    tool(
        "telegram_set_chat_sticker_set",
        "setChatStickerSet",
        "Attach a supergroup's sticker set, replacing the default suggestion",
        &[
            CHAT_ID,
            p("sticker_set_name", Kind::Str, true, "Sticker set name"),
        ],
        false,
        false,
    ),
    tool(
        "telegram_delete_chat_sticker_set",
        "deleteChatStickerSet",
        "Remove the attached sticker set from a supergroup",
        CHAT_TARGET,
        false,
        true,
    ),
    // ---- Moderation ------------------------------------------------------
    tool(
        "telegram_ban_chat_member",
        "banChatMember",
        "Ban a user from a group or channel",
        &[
            CHAT_ID,
            USER_ID,
            p(
                "until_date",
                Kind::Int,
                false,
                "Unix time the ban expires; omit for a permanent ban",
            ),
            p(
                "revoke_messages",
                Kind::Bool,
                false,
                "Also delete their recent messages in the chat",
            ),
        ],
        false,
        true,
    ),
    tool(
        "telegram_unban_chat_member",
        "unbanChatMember",
        "Lift a ban so the user may rejoin",
        &[
            CHAT_ID,
            USER_ID,
            p(
                "only_if_banned",
                Kind::Bool,
                false,
                "Leave the user alone unless they are actually banned",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_restrict_chat_member",
        "restrictChatMember",
        "Mute, read-only or otherwise restrict one member",
        &[
            CHAT_ID,
            USER_ID,
            p(
                "permissions",
                Kind::Obj,
                true,
                "ChatPermissions object with the rights the member keeps, e.g. {\"can_send_messages\": false}",
            ),
            p(
                "until_date",
                Kind::Int,
                false,
                "Unix time the restriction expires",
            ),
            p(
                "use_independent_chat_permissions",
                Kind::Bool,
                false,
                "Treat the supplied flags as the full set",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_promote_chat_member",
        "promoteChatMember",
        "Promote a member to administrator and/or set their rights",
        &[
            CHAT_ID,
            USER_ID,
            p(
                "is_anonymous",
                Kind::Bool,
                false,
                "Hide the admin in the chat",
            ),
            p("can_manage_chat", Kind::Bool, false, "Manage the chat"),
            p(
                "can_delete_messages",
                Kind::Bool,
                false,
                "Delete any message",
            ),
            p(
                "can_manage_video_chats",
                Kind::Bool,
                false,
                "Manage video chats",
            ),
            p(
                "can_restrict_members",
                Kind::Bool,
                false,
                "Restrict, ban and unban",
            ),
            p(
                "can_promote_members",
                Kind::Bool,
                false,
                "Add administrators",
            ),
            p(
                "can_change_info",
                Kind::Bool,
                false,
                "Edit the chat title, photo and description",
            ),
            p("can_invite_users", Kind::Bool, false, "Create invite links"),
            p("can_pin_messages", Kind::Bool, false, "Pin messages"),
            p("can_post_stories", Kind::Bool, false, "Post stories"),
            p("can_edit_stories", Kind::Bool, false, "Edit stories"),
            p("can_delete_stories", Kind::Bool, false, "Delete stories"),
            p(
                "can_manage_topics",
                Kind::Bool,
                false,
                "Manage forum topics",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_set_chat_administrator_custom_title",
        "setChatAdministratorCustomTitle",
        "Set a custom title shown next to one administrator's name",
        &[
            CHAT_ID,
            USER_ID,
            p("custom_title", Kind::Str, true, "Title (0-16 characters)"),
        ],
        false,
        false,
    ),
    tool(
        "telegram_ban_chat_sender_chat",
        "banChatSenderChat",
        "Ban a channel's automated posts from a group",
        &[
            CHAT_ID,
            p(
                "sender_chat_id",
                Kind::Int,
                true,
                "Identifier of the channel to ban",
            ),
        ],
        false,
        true,
    ),
    tool(
        "telegram_unban_chat_sender_chat",
        "unbanChatSenderChat",
        "Allow a previously banned channel to post in a group again",
        &[
            CHAT_ID,
            p(
                "sender_chat_id",
                Kind::Int,
                true,
                "Identifier of the channel to unban",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_approve_chat_join_request",
        "approveChatJoinRequest",
        "Approve a pending join request from a user or channel",
        &[CHAT_ID, USER_ID],
        false,
        false,
    ),
    tool(
        "telegram_decline_chat_join_request",
        "declineChatJoinRequest",
        "Decline a pending join request",
        &[CHAT_ID, USER_ID],
        false,
        true,
    ),
    // ---- Pins ------------------------------------------------------------
    tool(
        "telegram_pin_chat_message",
        "pinChatMessage",
        "Pin a message in a chat",
        &[
            CHAT_ID,
            MESSAGE_ID,
            p("disable_notification", Kind::Bool, false, "Pin silently"),
        ],
        false,
        false,
    ),
    tool(
        "telegram_unpin_chat_message",
        "unpinChatMessage",
        "Unpin one message, or the most recent pin when no id is given",
        &[
            CHAT_ID,
            p(
                "message_id",
                Kind::Int,
                false,
                "Message to unpin; omit for the latest pin",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_unpin_all_chat_messages",
        "unpinAllChatMessages",
        "Unpin every pinned message in a chat",
        CHAT_TARGET,
        false,
        true,
    ),
    // ---- Invite links ----------------------------------------------------
    tool(
        "telegram_export_chat_invite_link",
        "exportChatInviteLink",
        "Return the chat's primary invite link, creating it if needed",
        CHAT_TARGET,
        false,
        false,
    ),
    tool(
        "telegram_create_chat_invite_link",
        "createChatInviteLink",
        "Create an additional invite link with its own expiry and use limit",
        &[
            CHAT_ID,
            p(
                "name",
                Kind::Str,
                false,
                "Label for the link (0-32 characters)",
            ),
            p(
                "expire_date",
                Kind::Int,
                false,
                "Unix time the link expires",
            ),
            p(
                "member_limit",
                Kind::Int,
                false,
                "1-99999 uses, or omit for unlimited",
            ),
            p(
                "creates_join_request",
                Kind::Bool,
                false,
                "Require approval before the invite takes effect",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_edit_chat_invite_link",
        "editChatInviteLink",
        "Change an existing additional invite link",
        &[
            CHAT_ID,
            p("invite_link", Kind::Str, true, "The invite link to edit"),
            p("name", Kind::Str, false, "New label (0-32 characters)"),
            p("expire_date", Kind::Int, false, "New expiry as Unix time"),
            p("member_limit", Kind::Int, false, "New use limit (1-99999)"),
            p(
                "creates_join_request",
                Kind::Bool,
                false,
                "Require approval before the invite takes effect",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_revoke_chat_invite_link",
        "revokeChatInviteLink",
        "Revoke an invite link. The primary link is replaced, not disabled.",
        &[
            CHAT_ID,
            p("invite_link", Kind::Str, true, "The invite link to revoke"),
        ],
        false,
        true,
    ),
    // ---- Forum topics ----------------------------------------------------
    tool(
        "telegram_get_forum_topic_icon_stickers",
        "getForumTopicIconStickers",
        "List the stickers usable as a forum topic icon",
        &[],
        true,
        false,
    ),
    tool(
        "telegram_create_forum_topic",
        "createForumTopic",
        "Create a topic in a forum supergroup",
        &[
            CHAT_ID,
            p("name", Kind::Str, true, "Topic name (1-128 characters)"),
            p(
                "icon_color",
                Kind::Int,
                false,
                "0x6FB9F0, 0xFFD67E, 0xCB86DB, 0x8EEE98, 0xFF93B2 or 0xFB6F5F",
            ),
            p(
                "icon_custom_emoji_id",
                Kind::Str,
                false,
                "Custom emoji id to use as the icon",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_edit_forum_topic",
        "editForumTopic",
        "Rename a forum topic or change its icon",
        &[
            CHAT_ID,
            THREAD_ID,
            p("name", Kind::Str, false, "New name (0-128 characters)"),
            p(
                "icon_custom_emoji_id",
                Kind::Str,
                false,
                "New icon emoji id",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_close_forum_topic",
        "closeForumTopic",
        "Close a forum topic so members cannot post",
        &[CHAT_ID, THREAD_ID],
        false,
        false,
    ),
    tool(
        "telegram_reopen_forum_topic",
        "reopenForumTopic",
        "Reopen a closed forum topic",
        &[CHAT_ID, THREAD_ID],
        false,
        false,
    ),
    tool(
        "telegram_delete_forum_topic",
        "deleteForumTopic",
        "Delete a forum topic and all of its messages",
        &[CHAT_ID, THREAD_ID],
        false,
        true,
    ),
    tool(
        "telegram_edit_general_forum_topic",
        "editGeneralForumTopic",
        "Rename the General topic of a forum",
        &[
            CHAT_ID,
            p("name", Kind::Str, true, "New name (1-128 characters)"),
        ],
        false,
        false,
    ),
    tool(
        "telegram_close_general_forum_topic",
        "closeGeneralForumTopic",
        "Close the General topic of a forum",
        CHAT_TARGET,
        false,
        false,
    ),
    tool(
        "telegram_reopen_general_forum_topic",
        "reopenGeneralForumTopic",
        "Reopen the General topic of a forum",
        CHAT_TARGET,
        false,
        false,
    ),
    tool(
        "telegram_hide_general_forum_topic",
        "hideGeneralForumTopic",
        "Hide the General topic from members",
        CHAT_TARGET,
        false,
        false,
    ),
    tool(
        "telegram_unhide_general_forum_topic",
        "unhideGeneralForumTopic",
        "Make a hidden General topic visible again",
        CHAT_TARGET,
        false,
        false,
    ),
    tool(
        "telegram_unpin_all_forum_topic_messages",
        "unpinAllForumTopicMessages",
        "Unpin everything in one forum topic",
        &[CHAT_ID, THREAD_ID],
        false,
        true,
    ),
    tool(
        "telegram_unpin_all_general_forum_topic_messages",
        "unpinAllGeneralForumTopicMessages",
        "Unpin everything in a forum's General topic",
        CHAT_TARGET,
        false,
        true,
    ),
    // ---- Stickers and custom emoji --------------------------------------
    tool(
        "telegram_get_sticker_set",
        "getStickerSet",
        "Return a sticker set with all of its stickers",
        &[p(
            "name",
            Kind::Str,
            true,
            "Sticker set name (from its /addstickers link)",
        )],
        true,
        false,
    ),
    tool(
        "telegram_get_custom_emoji_stickers",
        "getCustomEmojiStickers",
        "Resolve custom emoji ids into sticker objects",
        &[p(
            "custom_emoji_ids",
            Kind::ArrStr,
            true,
            "1-200 custom emoji ids",
        )],
        true,
        false,
    ),
    tool(
        "telegram_create_new_sticker_set",
        "createNewStickerSet",
        "Create a sticker set owned by the bot",
        &[
            USER_ID,
            p(
                "name",
                Kind::Str,
                true,
                "Short name ending in _by_<bot username>",
            ),
            p("title", Kind::Str, true, "Set title (1-64 characters)"),
            p(
                "stickers",
                Kind::Arr,
                true,
                "1-50 InputSticker objects with \"sticker\", \"format\" and \"emoji_list\"",
            ),
            p(
                "sticker_type",
                Kind::Str,
                false,
                "\"regular\" (default), \"mask\" or \"custom_emoji\"",
            ),
            p(
                "needs_repainting",
                Kind::Bool,
                false,
                "Let Telegram repaint custom emoji",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_add_sticker_to_set",
        "addStickerToSet",
        "Add one sticker to a set the bot owns",
        &[
            USER_ID,
            p("name", Kind::Str, true, "Sticker set name"),
            p("sticker", Kind::Obj, true, "InputSticker object"),
        ],
        false,
        false,
    ),
    tool(
        "telegram_set_sticker_position_in_set",
        "setStickerPositionInSet",
        "Move a sticker to another position in its set",
        &[
            p("sticker", Kind::Str, true, "File identifier of the sticker"),
            p("position", Kind::Int, true, "New 0-based position"),
        ],
        false,
        false,
    ),
    tool(
        "telegram_delete_sticker_from_set",
        "deleteStickerFromSet",
        "Remove a sticker from a set the bot owns",
        &[p(
            "sticker",
            Kind::Str,
            true,
            "File identifier of the sticker",
        )],
        false,
        true,
    ),
    tool(
        "telegram_set_sticker_emoji_list",
        "setStickerEmojiList",
        "Change the emoji a sticker is associated with",
        &[
            p("sticker", Kind::Str, true, "File identifier of the sticker"),
            p("emoji_list", Kind::ArrStr, true, "1-20 associated emoji"),
        ],
        false,
        false,
    ),
    tool(
        "telegram_set_sticker_keywords",
        "setStickerKeywords",
        "Set the search keywords of a sticker",
        &[
            p("sticker", Kind::Str, true, "File identifier of the sticker"),
            p(
                "keywords",
                Kind::ArrStr,
                false,
                "0-20 keywords, 0-64 characters each",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_set_sticker_mask_position",
        "setStickerMaskPosition",
        "Set where a mask sticker's face sits",
        &[
            p("sticker", Kind::Str, true, "File identifier of the sticker"),
            p(
                "mask_position",
                Kind::Obj,
                false,
                "MaskPosition object; omit to remove it",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_set_sticker_set_title",
        "setStickerSetTitle",
        "Rename a sticker set the bot owns",
        &[
            p("name", Kind::Str, true, "Sticker set name"),
            p("title", Kind::Str, true, "New title (1-64 characters)"),
        ],
        false,
        false,
    ),
    tool(
        "telegram_set_sticker_set_thumbnail",
        "setStickerSetThumbnail",
        "Set a sticker set's thumbnail image",
        &[
            p("name", Kind::Str, true, "Sticker set name"),
            USER_ID,
            p(
                "thumbnail",
                Kind::Str,
                false,
                "HTTPS URL or file_id. Required for mask and custom-emoji sets.",
            ),
            p(
                "format",
                Kind::Str,
                true,
                "\"static\", \"animated\" or \"video\"",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_delete_sticker_set",
        "deleteStickerSet",
        "Delete a sticker set the bot owns",
        &[p("name", Kind::Str, true, "Sticker set name")],
        false,
        true,
    ),
    // ---- Callbacks, inline mode and web apps -----------------------------
    tool(
        "telegram_answer_callback_query",
        "answerCallbackQuery",
        "Answer a callback query from an inline keyboard button, dismissing its spinner",
        &[
            p(
                "callback_query_id",
                Kind::Str,
                true,
                "Identifier from the callback query update",
            ),
            p(
                "text",
                Kind::Str,
                false,
                "Notification text (0-200 characters)",
            ),
            p(
                "show_alert",
                Kind::Bool,
                false,
                "Show an alert instead of a toast",
            ),
            p(
                "url",
                Kind::Str,
                false,
                "URL to open instead of the notification",
            ),
            p(
                "cache_time",
                Kind::Int,
                false,
                "Seconds the answer may be cached",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_answer_inline_query",
        "answerInlineQuery",
        "Return results for an inline query",
        &[
            p(
                "inline_query_id",
                Kind::Str,
                true,
                "Identifier from the inline query update",
            ),
            p("results", Kind::Arr, true, "1-50 InlineQueryResult objects"),
            p(
                "cache_time",
                Kind::Int,
                false,
                "Seconds the result may be cached",
            ),
            p("is_personal", Kind::Bool, false, "Cache per user"),
            p(
                "next_offset",
                Kind::Str,
                false,
                "Pagination cursor for the next page",
            ),
            p(
                "button",
                Kind::Obj,
                false,
                "InlineQueryResultsButton object",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_answer_web_app_query",
        "answerWebAppQuery",
        "Set the result of a Mini App query, sending it to the user's chat",
        &[
            p(
                "web_app_query_id",
                Kind::Str,
                true,
                "Identifier from the web_app_data update",
            ),
            p("result", Kind::Obj, true, "InlineQueryResult object"),
        ],
        false,
        false,
    ),
    tool(
        "telegram_save_prepared_inline_message",
        "savePreparedInlineMessage",
        "Store a prepared inline message for sharing with one user",
        &[
            USER_ID,
            p(
                "result",
                Kind::Obj,
                true,
                "InlineQueryResult object to prepare",
            ),
            p(
                "allow_user_chats",
                Kind::Bool,
                false,
                "Allow sharing in private chats",
            ),
            p(
                "allow_bot_chats",
                Kind::Bool,
                false,
                "Allow sharing in bot chats",
            ),
            p(
                "allow_group_chats",
                Kind::Bool,
                false,
                "Allow sharing in groups",
            ),
            p(
                "allow_channel_chats",
                Kind::Bool,
                false,
                "Allow sharing in channels",
            ),
        ],
        false,
        false,
    ),
    // ---- Payments and stars ---------------------------------------------
    tool(
        "telegram_send_invoice",
        "sendInvoice",
        "Send an invoice for goods or services",
        &[
            p("chat_id", Kind::Str, true, "Destination chat id"),
            p("title", Kind::Str, true, "Product name (1-32 characters)"),
            p(
                "description",
                Kind::Str,
                true,
                "Product description (1-255 characters)",
            ),
            p(
                "payload",
                Kind::Str,
                true,
                "Bot-defined invoice payload (1-128 bytes)",
            ),
            p(
                "currency",
                Kind::Str,
                true,
                "Three-letter ISO 4217 currency, or XTR for stars",
            ),
            p(
                "prices",
                Kind::Arr,
                true,
                "Array of {\"label\": \"...\", \"amount\": 100} objects",
            ),
            p(
                "provider_token",
                Kind::Str,
                false,
                "Payment provider token; empty or omitted for stars",
            ),
            p(
                "max_tip_amount",
                Kind::Int,
                false,
                "Largest tip in the smallest currency unit",
            ),
            p(
                "start_parameter",
                Kind::Str,
                false,
                "Deep-link parameter for the payment",
            ),
            THREAD_ID,
            PROTECT,
            REPLY_MARKUP,
        ],
        false,
        false,
    ),
    tool(
        "telegram_create_invoice_link",
        "createInvoiceLink",
        "Create a reusable payment link for an invoice",
        &[
            p("title", Kind::Str, true, "Product name (1-32 characters)"),
            p(
                "description",
                Kind::Str,
                true,
                "Product description (1-255 characters)",
            ),
            p("payload", Kind::Str, true, "Bot-defined invoice payload"),
            p(
                "currency",
                Kind::Str,
                true,
                "Three-letter ISO 4217 currency, or XTR",
            ),
            p(
                "prices",
                Kind::Arr,
                true,
                "Array of {\"label\": \"...\", \"amount\": 100} objects",
            ),
            p(
                "provider_token",
                Kind::Str,
                false,
                "Payment provider token; omit for stars",
            ),
            p(
                "max_tip_amount",
                Kind::Int,
                false,
                "Largest tip in the smallest currency unit",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_answer_shipping_query",
        "answerShippingQuery",
        "Answer a shipping-address request from a checkout",
        &[
            p(
                "shipping_query_id",
                Kind::Str,
                true,
                "Identifier from the shipping query update",
            ),
            p("ok", Kind::Bool, true, "True when shipping is available"),
            p(
                "shipping_options",
                Kind::Arr,
                false,
                "Array of ShippingOption objects when ok is true",
            ),
            p(
                "error_message",
                Kind::Str,
                false,
                "Reason shown to the user when ok is false",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_answer_pre_checkout_query",
        "answerPreCheckoutQuery",
        "Confirm or reject a checkout before payment is taken",
        &[
            p(
                "pre_checkout_query_id",
                Kind::Str,
                true,
                "Identifier from the pre-checkout update",
            ),
            p("ok", Kind::Bool, true, "True to accept the order"),
            p(
                "error_message",
                Kind::Str,
                false,
                "Reason shown to the user when ok is false",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_refund_star_payment",
        "refundStarPayment",
        "Refund a payment made in Telegram stars",
        &[
            USER_ID,
            p(
                "telegram_payment_charge_id",
                Kind::Str,
                true,
                "Charge id from the successful payment",
            ),
        ],
        false,
        true,
    ),
    tool(
        "telegram_get_star_transactions",
        "getStarTransactions",
        "List the bot's star transactions",
        &[
            p("offset", Kind::Int, false, "Number of transactions to skip"),
            p("limit", Kind::Int, false, "1-100, default 100"),
        ],
        true,
        false,
    ),
    // ---- Games -----------------------------------------------------------
    tool(
        "telegram_send_game",
        "sendGame",
        "Send a game attached to the bot",
        &[
            CHAT_ID,
            p(
                "game_short_name",
                Kind::Str,
                true,
                "Short name from BotFather",
            ),
            NOTIFY,
            PROTECT,
            REPLY_MARKUP,
            THREAD_ID,
        ],
        false,
        false,
    ),
    tool(
        "telegram_set_game_score",
        "setGameScore",
        "Update a player's score in a game message",
        &[
            USER_ID,
            p("score", Kind::Int, true, "New non-negative score"),
            p(
                "force",
                Kind::Bool,
                false,
                "Update even if the new score is lower",
            ),
            p(
                "disable_edit_message",
                Kind::Bool,
                false,
                "Do not rewrite the message",
            ),
            p(
                "chat_id",
                Kind::Int,
                false,
                "Chat id, required unless inline_message_id is given",
            ),
            p(
                "message_id",
                Kind::Int,
                false,
                "Message id, required unless inline_message_id is given",
            ),
            p(
                "inline_message_id",
                Kind::Str,
                false,
                "Inline message identifier",
            ),
        ],
        false,
        false,
    ),
    tool(
        "telegram_get_game_high_scores",
        "getGameHighScores",
        "Return the high-score table around one player",
        &[
            USER_ID,
            p(
                "chat_id",
                Kind::Int,
                false,
                "Chat id, required unless inline_message_id is given",
            ),
            p(
                "message_id",
                Kind::Int,
                false,
                "Message id, required unless inline_message_id is given",
            ),
            p(
                "inline_message_id",
                Kind::Str,
                false,
                "Inline message identifier",
            ),
        ],
        true,
        false,
    ),
];

/// Look up one tool by name.
pub fn find(name: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|spec| spec.name == name)
}

/// Every declared parameter, plus `bot_token`.
pub fn schema(spec: &ToolSpec) -> Value {
    let mut properties = Map::new();
    let mut required: Vec<Value> = Vec::new();
    for param in spec.params {
        let mut value = param.kind.schema();
        value["description"] = json!(param.description);
        properties.insert(param.name.to_string(), value);
        if param.required {
            required.push(json!(param.name));
        }
    }
    properties.insert(
        "bot_token".to_string(),
        json!({
            "type": "string",
            "description": "Optional Telegram bot token for a stateless call. Defaults to the agent's configured token."
        }),
    );
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

/// Validate the arguments and build the Bot API parameter object.
///
/// `default_chat` and `default_parse_mode` are the per-agent configuration, used
/// only when the call omits the field and the method declares it.
pub fn build_params(
    spec: &ToolSpec,
    args: &Map<String, Value>,
    default_chat: Option<&str>,
    default_parse_mode: Option<&str>,
) -> Result<Value, String> {
    let mut params = Map::new();

    for param in spec.params {
        match args.get(param.name) {
            Some(Value::Null) | None => {
                if param.required {
                    // Allow the configured chat to satisfy a required chat_id.
                    if param.name == "chat_id"
                        && let Some(chat) = default_chat.map(str::trim).filter(|c| !c.is_empty())
                    {
                        params.insert("chat_id".to_string(), json!(chat));
                        continue;
                    }
                    return Err(format!("'{}' is required", param.name));
                }
                // The configured formatting mode fills an absent parse_mode.
                if param.name == "parse_mode"
                    && let Some(mode) = default_parse_mode.map(str::trim).filter(|m| !m.is_empty())
                {
                    params.insert("parse_mode".to_string(), json!(mode));
                }
            }
            Some(value) => {
                if !param.kind.accepts(value) {
                    return Err(format!(
                        "'{}' must be {}",
                        param.name,
                        type_name(param.kind)
                    ));
                }
                if let Value::String(text) = value
                    && text.trim().is_empty()
                    && param.required
                {
                    return Err(format!("'{}' cannot be empty", param.name));
                }
                params.insert(param.name.to_string(), value.clone());
            }
        }
    }

    // A chat-scoped method accepts the configured default chat even when the
    // table marks `chat_id` optional (edit*, live location, inline messages).
    if !params.contains_key("chat_id")
        && spec.params.iter().any(|p| p.name == "chat_id")
        && !args.contains_key("inline_message_id")
        && let Some(chat) = default_chat.map(str::trim).filter(|c| !c.is_empty())
    {
        params.insert("chat_id".to_string(), json!(chat));
    }

    validate(spec, &params)?;
    Ok(Value::Object(params))
}

/// Per-method limits that Telegram enforces with a 400. Checked here so the
/// model gets a precise error instead of a generic API failure.
fn validate(spec: &ToolSpec, params: &Map<String, Value>) -> Result<(), String> {
    for (field, limit, label) in [
        (
            "text",
            MAX_MESSAGE_CHARS,
            "Telegram's 4096-character message limit",
        ),
        (
            "caption",
            MAX_CAPTION_CHARS,
            "Telegram's 1024-character caption limit",
        ),
    ] {
        if let Some(Value::String(text)) = params.get(field)
            && text.chars().count() > limit
        {
            return Err(format!("'{field}' exceeds {label}"));
        }
    }

    if let Some(Value::String(mode)) = params.get("parse_mode") {
        match mode.trim().to_ascii_lowercase().as_str() {
            "" => {}
            "html" | "markdownv2" => {}
            other => {
                return Err(format!(
                    "'parse_mode' must be HTML or MarkdownV2, got {other:?}"
                ));
            }
        }
    }

    if spec.method == "sendMediaGroup"
        && let Some(Value::Array(media)) = params.get("media")
        && !(2..=10).contains(&media.len())
    {
        return Err("'media' must contain 2-10 items".to_string());
    }

    if let Some(Value::String(url)) = params.get("url")
        && spec.method == "setWebhook"
        && !url.trim().is_empty()
        && !url.starts_with("https://")
    {
        return Err("'url' must start with https:// (Telegram requires TLS)".to_string());
    }

    Ok(())
}

fn type_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Str => "a string",
        Kind::Int => "an integer",
        Kind::Bool => "a boolean",
        Kind::Float => "a number",
        Kind::Obj => "a JSON object",
        Kind::Arr | Kind::ArrStr | Kind::ArrInt => "an array",
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
                spec.name.starts_with("telegram_"),
                "{} must be namespaced",
                spec.name
            );
            assert!(seen.insert(spec.name), "duplicate tool {}", spec.name);
            assert!(!spec.description.is_empty());
            // Bot API method names are camelCase words: they become part of a
            // URL path, so anything else would be a path-injection bug.
            assert!(
                spec.method
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase()),
                "{}: method {:?} must be camelCase",
                spec.name,
                spec.method
            );
            assert!(
                spec.method.chars().all(|c| c.is_ascii_alphanumeric()),
                "{}: method {:?} must be alphanumeric",
                spec.name,
                spec.method
            );
        }
    }

    /// The generated schema is the only thing the model sees, so it must agree
    /// with the builder exactly.
    #[test]
    fn every_schema_matches_its_parameters() {
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

            for param in spec.params {
                assert!(
                    properties.contains_key(param.name),
                    "{}: parameter {} is not published",
                    spec.name,
                    param.name
                );
                assert_eq!(
                    required.contains(&param.name),
                    param.required,
                    "{}: {} required flag disagrees with the schema",
                    spec.name,
                    param.name
                );
            }
            assert!(
                properties.contains_key("bot_token"),
                "{}: every tool accepts a per-call token",
                spec.name
            );
            // No duplicate parameter names: the later one would silently win.
            let mut seen = std::collections::HashSet::new();
            for param in spec.params {
                assert!(
                    seen.insert(param.name),
                    "{}: duplicate parameter {}",
                    spec.name,
                    param.name
                );
            }
        }
    }

    /// Every declared parameter reaches the API. This is the anti-drift guard:
    /// a parameter published but not forwarded would make the model's argument
    /// silently vanish.
    #[test]
    fn declared_parameters_reach_the_request() {
        for spec in TOOLS {
            let mut pairs: Vec<(&str, Value)> = Vec::new();
            for param in spec.params {
                // Representative values: a few methods validate their content
                // (a webhook URL must be TLS, a parse mode must be known), and a
                // placeholder would test the validator rather than the routing.
                let value = match param.name {
                    "url" => json!("https://example.test/hook"),
                    "parse_mode" => json!("HTML"),
                    "text" => json!("representative text"),
                    _ => match param.kind {
                        Kind::Str => json!("x"),
                        Kind::Int => json!(1),
                        Kind::Bool => json!(true),
                        Kind::Float => json!(1.5),
                        Kind::Obj => json!({}),
                        Kind::Arr => json!([
                            {"type": "photo", "media": "https://e/x.jpg"},
                            {"type": "photo", "media": "https://e/y.jpg"}
                        ]),
                        Kind::ArrStr => json!(["a"]),
                        Kind::ArrInt => json!([1]),
                    },
                };
                pairs.push((param.name, value));
            }
            let params = build_params(spec, &args(&pairs), None, None)
                .unwrap_or_else(|e| panic!("{}: {e}", spec.name));
            let object = params.as_object().unwrap();
            for param in spec.params {
                assert!(
                    object.contains_key(param.name),
                    "{}: {} was declared but not forwarded",
                    spec.name,
                    param.name
                );
            }
        }
    }

    #[test]
    fn fills_the_default_chat_and_parse_mode() {
        let send = find("telegram_send_message").unwrap();
        let built = build_params(
            send,
            &args(&[("text", json!("hi"))]),
            Some("-100123"),
            Some("HTML"),
        )
        .unwrap();
        assert_eq!(built["chat_id"], "-100123");
        assert_eq!(built["parse_mode"], "HTML");
        assert_eq!(built["text"], "hi");

        // An explicit value beats the configured default.
        let built = build_params(
            send,
            &args(&[
                ("text", json!("hi")),
                ("chat_id", json!("@chan")),
                ("parse_mode", json!("MarkdownV2")),
            ]),
            Some("-100123"),
            Some("HTML"),
        )
        .unwrap();
        assert_eq!(built["chat_id"], "@chan");
        assert_eq!(built["parse_mode"], "MarkdownV2");

        // No default, no chat: a required chat_id is an error, not a guess.
        assert!(build_params(send, &args(&[("text", json!("hi"))]), None, None).is_err());

        // An edit tool takes the default chat when it omits one.
        let edit = find("telegram_edit_message_text").unwrap();
        let built = build_params(
            edit,
            &args(&[("message_id", json!(7)), ("text", json!("new"))]),
            Some("-100123"),
            None,
        )
        .unwrap();
        assert_eq!(built["chat_id"], "-100123");

        // …unless it addresses an inline message, which has no chat.
        let built = build_params(
            edit,
            &args(&[("inline_message_id", json!("abc")), ("text", json!("new"))]),
            Some("-100123"),
            None,
        )
        .unwrap();
        assert!(!built.as_object().unwrap().contains_key("chat_id"));
    }

    #[test]
    fn rejects_bad_arguments_before_the_request() {
        let send = find("telegram_send_message").unwrap();
        // Wrong type.
        assert!(
            build_params(
                send,
                &args(&[
                    ("chat_id", json!("1")),
                    ("text", json!("x")),
                    ("parse_mode", json!(1))
                ]),
                None,
                None
            )
            .is_err()
        );
        // Empty required string.
        assert!(
            build_params(
                send,
                &args(&[("chat_id", json!("1")), ("text", json!("   "))]),
                None,
                None
            )
            .is_err()
        );
        // Text over Telegram's limit.
        let long: String = "a".repeat(MAX_MESSAGE_CHARS + 1);
        let err = build_params(
            send,
            &args(&[("chat_id", json!("1")), ("text", json!(long))]),
            None,
            None,
        )
        .unwrap_err();
        assert!(err.contains("4096"), "{err}");
        // Unknown parse mode.
        assert!(
            build_params(
                send,
                &args(&[
                    ("chat_id", json!("1")),
                    ("text", json!("x")),
                    ("parse_mode", json!("rtf"))
                ]),
                None,
                None
            )
            .is_err()
        );
        // Webhook must be TLS.
        assert!(
            build_params(
                find("telegram_set_webhook").unwrap(),
                &args(&[("url", json!("http://plain.example/hook"))]),
                None,
                None
            )
            .is_err()
        );
        // A media group needs 2-10 items.
        assert!(
            build_params(
                find("telegram_send_media_group").unwrap(),
                &args(&[
                    ("chat_id", json!("1")),
                    (
                        "media",
                        json!([{"type": "photo", "media": "https://e/x.jpg"}])
                    )
                ]),
                None,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn moderation_rows_are_flagged() {
        for name in [
            "telegram_ban_chat_member",
            "telegram_delete_messages",
            "telegram_leave_chat",
            "telegram_revoke_chat_invite_link",
            "telegram_delete_forum_topic",
            "telegram_delete_sticker_set",
            "telegram_refund_star_payment",
        ] {
            let spec = find(name).unwrap();
            assert!(!spec.read_only, "{name} mutates");
            assert!(spec.destructive, "{name} must be marked destructive");
            assert!(
                !spec.method.is_empty() && spec.method.chars().all(|c| c.is_ascii_alphanumeric()),
                "{name}: bad method"
            );
        }
        // A pure read must never be flagged destructive.
        for name in [
            "telegram_get_chat",
            "telegram_get_sticker_set",
            "telegram_get_file",
        ] {
            let spec = find(name).unwrap();
            assert!(spec.read_only, "{name} is a read");
            assert!(!spec.destructive, "{name} is a read");
        }
    }
}
