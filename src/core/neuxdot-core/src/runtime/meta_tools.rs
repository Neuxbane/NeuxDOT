use handlebars::Handlebars;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub const MAX_MEMORY_CHARS: usize = 5000;

pub fn calculate_memory_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// Parses strings like "1m", "30s", "2h", "1d", "1h30m" into std::time::Duration
pub fn parse_duration_string(s: &str) -> Result<Duration, String> {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return Err("Duration string cannot be empty".to_string());
    }

    let mut total_secs: u64 = 0;
    let mut current_num: u64 = 0;
    let mut has_unit = false;

    let chars = s.chars();
    for c in chars {
        if c.is_ascii_digit() {
            current_num = current_num
                .saturating_mul(10)
                .saturating_add(c as u64 - '0' as u64);
        } else {
            has_unit = true;
            match c {
                's' => {
                    total_secs = total_secs.saturating_add(current_num);
                    current_num = 0;
                }
                'm' => {
                    total_secs = total_secs.saturating_add(current_num.saturating_mul(60));
                    current_num = 0;
                }
                'h' => {
                    total_secs = total_secs.saturating_add(current_num.saturating_mul(3600));
                    current_num = 0;
                }
                'd' => {
                    total_secs = total_secs.saturating_add(current_num.saturating_mul(86400));
                    current_num = 0;
                }
                _ => {
                    return Err(format!(
                        "Unrecognized unit '{}' in duration '{}'. Expected d, h, m, or s.",
                        c, s
                    ));
                }
            }
        }
    }

    if !has_unit && current_num > 0 {
        total_secs = current_num;
    }

    if total_secs == 0 {
        return Err(format!(
            "Invalid duration '{}'. Must specify non-zero time (e.g. '1m', '30s', '2h', '1d').",
            s
        ));
    }

    Ok(Duration::from_secs(total_secs))
}

/// Returns the strictly immutable meta-tools definition sent to the LLM.
pub fn get_meta_tools_definitions() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "search_tools",
                "description": "Search for available external MCP tools by keyword or capability",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search keyword or phrase to match tool names and descriptions"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of tools to return (default 10)"
                        }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_tool_schema",
                "description": "Fetch the complete input schema and parameter specification for a specific tool",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "The exact name of the tool retrieved from search_tools"
                        }
                    },
                    "required": ["name"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "call_tool",
                "description": "Execute an external MCP tool with specified arguments. Large results are truncated to a head/body/tail view in your context; the full output is always kept in history and can be read back with history_inspect. Optionally override how many characters of the head, body and tail are kept for this call.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "The exact name of the tool to execute"
                        },
                        "arguments": {
                            "type": "object",
                            "description": "JSON arguments matching the tool's input schema"
                        },
                        "head": {
                            "type": "integer",
                            "description": "Characters to keep from the start of the result (default 1000, 0 disables the head segment)."
                        },
                        "body": {
                            "type": "integer",
                            "description": "Characters to keep from the middle of the result (default 1000, 0 disables the body segment)."
                        },
                        "tail": {
                            "type": "integer",
                            "description": "Characters to keep from the end of the result (default 1000, 0 disables the tail segment)."
                        }
                    },
                    "required": ["name", "arguments"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_servers",
                "description": "List all connected MCP servers and their operational status",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "instructionsWrite",
                "description": "Replace the whole PERSISTENT INSTRUCTIONS block (max 5,000 chars). This is a second, editable instruction set re-injected into the system prompt every turn — NOT the keyed memory graph (use memoryNodeWrite for that). Requires currentHash. Returns the new hash.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "The complete memory text to persist (maximum 5000 characters)"
                        },
                        "currentHash": {
                            "type": "string",
                            "description": "The current hash of the memory. If memory is empty, obtain it via instructionsRead()"
                        }
                    },
                    "required": ["content", "currentHash"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "instructionsEdit",
                "description": "Search-and-replace inside the PERSISTENT INSTRUCTIONS block (not the keyed memory graph). Requires currentHash. Returns the new hash.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "search": {
                            "type": "string",
                            "description": "The exact string segment to find in existing memory"
                        },
                        "replace": {
                            "type": "string",
                            "description": "The replacement string segment"
                        },
                        "currentHash": {
                            "type": "string",
                            "description": "The current hash of the memory"
                        }
                    },
                    "required": ["search", "replace", "currentHash"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "instructionsRead",
                "description": "Read the full PERSISTENT INSTRUCTIONS block (not the keyed memory graph) and its current hash.",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "memoryNodeWrite",
                "description": "Create or replace ONE KEYED MEMORY NODE in the wikilink graph (not the instructions block). Nodes are the primary long-term memory: each `key` is a canonical lowercase noun phrase and other nodes link to it with [[key]] inside their value. Replacing an existing node requires its `expectedHash`; omitting it when the key already exists is rejected. Returns the new hash.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "key": {
                            "type": "string",
                            "description": "Node identity, a canonical noun phrase (e.g. 'postgres primary'), max 100 chars. Referenced by other nodes as [[key]]."
                        },
                        "value": {
                            "type": "string",
                            "description": "The node body, max 10,000 chars. Use [[other key]] to link related nodes."
                        },
                        "type": {
                            "type": "string",
                            "enum": ["entity", "decision", "preference", "event", "file", "concept", "skill"],
                            "description": "Advisory kind, used for grouping in the graph view. Default 'concept'."
                        },
                        "description": {
                            "type": "string",
                            "description": "Optional recall-oriented one-liner: the question this node answers."
                        },
                        "expectedHash": {
                            "type": "string",
                            "description": "Hash of the node being replaced. Required when the key already exists; omit for a new node."
                        }
                    },
                    "required": ["key", "value"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "memoryNodeRead",
                "description": "Read one keyed memory NODE by key, including outgoing [[links]] and backlinks (not the instructions block).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "key": { "type": "string", "description": "Exact node key" }
                    },
                    "required": ["key"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "memoryNodeSearch",
                "description": "Search keyed memory NODES. Key matches rank first, then descriptions, then values. Use this to find a key before reading or editing it.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Case-insensitive substring" },
                        "limit": { "type": "integer", "description": "1-100, default 10" }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "memoryNodeList",
                "description": "List the keyed memory graph: every node key with its type, description and degree. Cheap overview before reading specific nodes.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "type": {
                            "type": "string",
                            "description": "Only nodes of this kind, e.g. 'skill' or 'decision'"
                        }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "memoryNodeDelete",
                "description": "Delete a keyed memory NODE and every link touching it. Use it to retract a fact that is no longer true.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "key": { "type": "string", "description": "Exact node key" },
                        "expectedHash": {
                            "type": "string",
                            "description": "Hash of the node being deleted. Required, so a stale read cannot delete newer content."
                        }
                    },
                    "required": ["key", "expectedHash"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "skillWrite",
                "description": "Create or replace a reusable skill. A skill is a memory node with type 'skill': its value holds the actual instructions/procedure, and it is recalled the same way ([[links]], search, read). Load a skill with memoryRead before following it.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Skill name, e.g. 'deploy staging'. Stored lowercased as the node key."
                        },
                        "instructions": {
                            "type": "string",
                            "description": "The procedure itself: numbered steps, commands, caveats, max 10,000 chars."
                        },
                        "description": {
                            "type": "string",
                            "description": "When to use this skill, phrased as the situation it handles. This is what memorySearch matches against."
                        },
                        "expectedHash": {
                            "type": "string",
                            "description": "Hash of the skill being replaced; omit for a new skill."
                        }
                    },
                    "required": ["name", "instructions", "description"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "skillList",
                "description": "List available skills (memory nodes of type 'skill') with their descriptions, so you can decide which to load.",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "compact",
                "description": "Compact running conversation history so it retains only the system prompt, current memory, and current hash. Write the next concrete action into `continuation`: it is injected as a user message immediately after compaction, so the rebuilt session starts working on it right away instead of waiting for an external event.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "continuation": {
                            "type": "string",
                            "description": "Handoff instruction from you to the rebuilt session: what you were doing, what is already done, and the exact next action. It becomes the first user message after compaction and the turn keeps running, so be specific and actionable. If omitted, the compacted session waits for the next event."
                        }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "sleep",
                "description": "Pause agent execution for a specified duration (e.g. '1m', '30s', '2h', '1d'). Blocks the current turn until the duration elapses, then continues. By default the sleep is INTERRUPTIBLE: a message arriving meanwhile cuts it short so you can respond. Pass interruptible=false when you must not be disturbed (e.g. waiting on a rate limit).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "duration": {
                            "type": "string",
                            "description": "Duration to sleep, formatted with d/h/m/s unit (e.g. '1m', '30s', '2h', '1d')"
                        },
                        "time": {
                            "type": "string",
                            "description": "Alternative parameter for duration string (e.g. '1m')"
                        },
                        "interruptible": {
                            "type": "boolean",
                            "description": "Default true. When true, an arriving message wakes you early. Set false to sleep the full duration regardless."
                        }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "history_search",
                "description": "Search your full lifetime history (including text omitted from the running context by truncation or compaction) with a regular expression. Returns matching entries ordered by time, with an `offset` you can pass to history_inspect to read the omitted text. Use this to recover content you can no longer see.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "regex": {
                            "type": "string",
                            "description": "Regular expression (case-insensitive) matched against historical messages, tool calls and tool results."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of matches to return (default 20, max 100)."
                        }
                    },
                    "required": ["regex"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "history_inspect",
                "description": "Read an exact character range from one historical entry identified by history_search or by a truncation marker. Returns the requested text plus the total length so you can page through it.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The `id` of the history entry (from history_search, a truncation marker or a tool result)."
                        },
                        "from": {
                            "type": "integer",
                            "description": "Start character offset (inclusive). Use the offset reported by history_search or the truncation marker."
                        },
                        "to": {
                            "type": "integer",
                            "description": "End character offset (exclusive). Defaults to from + 2000."
                        }
                    },
                    "required": ["id", "from"]
                }
            }
        }
    ])
}

/// Default number of characters kept for each truncation segment.
pub const DEFAULT_TRUNCATION_CHARS: usize = 1000;

/// Marker prefix embedded in a truncated payload. The number following it is
/// the absolute character offset in the *original* content, so the agent can
/// page the missing region with `history_inspect(id, from, to)`.
pub const TRUNCATION_MARKER_PREFIX: &str = "<<truncated:";
pub const TRUNCATION_MARKER_SUFFIX: &str = ">>";

/// Per-agent truncation configuration for large tool results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TruncationConfig {
    pub head: usize,
    pub body: usize,
    pub tail: usize,
}

impl Default for TruncationConfig {
    fn default() -> Self {
        Self {
            head: DEFAULT_TRUNCATION_CHARS,
            body: DEFAULT_TRUNCATION_CHARS,
            tail: DEFAULT_TRUNCATION_CHARS,
        }
    }
}

impl TruncationConfig {
    /// A config with every segment at zero disables truncation entirely.
    pub fn is_disabled(&self) -> bool {
        self.head == 0 && self.body == 0 && self.tail == 0
    }

    /// Total number of characters the kept segments may occupy.
    pub fn budget(&self) -> usize {
        self.head + self.body + self.tail
    }
}

/// True when `text` already contains a truncation marker produced by
/// [`truncate_middle`]; such text must never be truncated a second time.
pub fn is_already_truncated(text: &str) -> bool {
    text.contains(TRUNCATION_MARKER_PREFIX)
}

/// Truncates `text` keeping `head` characters from the start, `tail`
/// characters from the end, and `body` characters centred in the middle.
///
/// The elided regions are replaced by markers of the form
/// `<<truncated:FROM-TO>>`, where `FROM`/`TO` are absolute character offsets in
/// the original text (so `history_inspect(id, FROM, TO)` recovers exactly the
/// missing region). This is UTF-8 safe because it operates on `char` indices,
/// never on byte offsets.
///
/// Truncation is skipped when the text already fits, when the config is
/// disabled, or when the text was already truncated (idempotent).
pub fn truncate_middle(text: &str, cfg: TruncationConfig) -> String {
    if cfg.is_disabled() || is_already_truncated(text) {
        return text.to_string();
    }

    let chars: Vec<char> = text.chars().collect();
    let total = chars.len();
    if total <= cfg.budget() {
        return text.to_string();
    }

    let head = cfg.head.min(total);
    let tail = cfg.tail.min(total.saturating_sub(head));

    // The body window is centred in whatever remains between head and tail.
    let middle_start = head;
    let middle_end = total - tail;
    let middle_len = middle_end.saturating_sub(middle_start);

    let body_len = cfg.body.min(middle_len);
    let body_start = middle_start + (middle_len - body_len) / 2;
    let body_end = body_start + body_len;

    let take = |from: usize, to: usize| -> String { chars[from..to].iter().collect() };
    let marker = |from: usize, to: usize| -> String {
        format!(
            "{}{}-{}{}",
            TRUNCATION_MARKER_PREFIX, from, to, TRUNCATION_MARKER_SUFFIX
        )
    };

    let mut out = String::with_capacity(cfg.budget() + 64);

    // head [0, head)
    out.push_str(&take(0, head));
    if body_len == 0 {
        // No body window: the elided region is a single contiguous gap between
        // head and tail, so one marker describes it exactly.
        if middle_end > head {
            out.push_str(&marker(head, middle_end));
        }
    } else {
        if body_start > head {
            out.push_str(&marker(head, body_start));
        }
        out.push_str(&take(body_start, body_end));
        if middle_end > body_end {
            out.push_str(&marker(body_end, middle_end));
        }
    }
    // tail
    if tail > 0 {
        out.push_str(&take(total - tail, total));
    }

    out
}

/// Builds the per-response context header, e.g.
/// `2026-11-02T07:49:33.124Z ctx:30%`.
///
/// `current` / `max` are history character counts; the percentage is clamped to
/// `0..=100` and integer-floored so it never reads `101%`ish noise.
pub fn context_header(current: usize, max: usize) -> String {
    let pct = if max == 0 {
        0
    } else {
        ((current.saturating_mul(100)) / max).min(100)
    };
    format!("{} ctx:{}%", utc_now_millis(), pct)
}

/// Current UTC time formatted as `2026-11-02T07:49:33.124Z`.
pub fn utc_now_millis() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// Prepends the context header to `body` on its own line.
pub fn with_context_header(body: &str, current: usize, max: usize) -> String {
    format!("{}\n{}", context_header(current, max), body)
}

/// System prompts support Handlebars syntax with structured presets:
/// - `{{agent.name}}`: The agent's name
/// - `{{agent.id}}`: The agent's unique ID
/// - `{{agent.strategy}}`: The agent's routing strategy
/// - `{{agent.memory}}` / `{{agent.memory.content}}`: Persistent memory content
/// - `{{agent.memory.hash}}`: SHA-256 hash of current memory
/// - `{{tools.preset}}` / `{{tools-preset}}`: Comma-separated key names of allowed external tools
/// - `{{tools.bulleted}}`: Bulleted list of allowed tool names
/// - `{{time}}`: Current local time string (e.g. "2:30 PM")
/// - `{{context_remaining}}`: Remaining characters before max_running_history_chars
pub fn interpolate_prompt(
    template: &str,
    agent_id: &str,
    agent_name: &str,
    agent_strategy: &str,
    memory_content: &str,
    memory_hash: &str,
    allowed_tools: &[String],
    context_remaining: usize,
) -> String {
    let mut hbs = Handlebars::new();
    hbs.set_strict_mode(false);

    // Normalize {{agent.memory}} / {{{agent.memory}}} to content property
    let normalized = template
        .replace("{{{agent.memory}}}", "{{{agent.memory.content}}}")
        .replace("{{agent.memory}}", "{{agent.memory.content}}");

    let tools_preset_str = if allowed_tools.is_empty() {
        "(none)".to_string()
    } else {
        allowed_tools.join(", ")
    };

    let tools_bulleted = if allowed_tools.is_empty() {
        "(none)".to_string()
    } else {
        allowed_tools
            .iter()
            .map(|t| format!("- {}", t))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let time_str = chrono::Local::now()
        .format("%l:%M %p")
        .to_string()
        .trim()
        .to_string();

    let data = json!({
        "agent": {
            "id": agent_id,
            "name": agent_name,
            "strategy": agent_strategy,
            "memory": {
                "content": memory_content,
                "hash": memory_hash,
            }
        },
        "tools": {
            "preset": tools_preset_str,
            "bulleted": tools_bulleted,
            "list": allowed_tools,
        },
        "context_remaining": context_remaining,
        "time": time_str,
        // Convenience aliases
        "name": agent_name,
        "tools-preset": tools_preset_str,
        "tools_preset": tools_preset_str,
        "tools-bulleted": tools_bulleted,
        "tools_bulleted": tools_bulleted,
    });

    hbs.render_template(&normalized, &data).unwrap_or_else(|_| {
        normalized
            .replace("{{agent.name}}", agent_name)
            .replace("{{agent.id}}", agent_id)
            .replace("{{agent.memory.content}}", memory_content)
            .replace("{{agent.memory.hash}}", memory_hash)
            .replace("{{name}}", agent_name)
            .replace("{{tools.preset}}", &tools_preset_str)
            .replace("{{tools-preset}}", &tools_preset_str)
            .replace("{{tools_preset}}", &tools_preset_str)
            .replace("{{time}}", &time_str)
            .replace("{{context_remaining}}", &context_remaining.to_string())
    })
}

/// Evaluates configured response hooks and substitutes template variables
pub fn render_response_hooks(
    hooks: &[crate::db::schema::ResponseHook],
    current_history_chars: usize,
    max_history_chars: usize,
) -> Vec<String> {
    let remaining = max_history_chars.saturating_sub(current_history_chars);
    let time_str = chrono::Local::now()
        .format("%l:%M %p")
        .to_string()
        .trim()
        .to_string();

    let mut hbs = Handlebars::new();
    hbs.set_strict_mode(false);

    let data = json!({
        "time": time_str,
        "context_remaining": remaining,
        "context_remaining_formatted": format!("{}", remaining),
        "max_context": max_history_chars,
        "current_context": current_history_chars,
    });

    hooks
        .iter()
        .filter(|h| h.enabled && !h.message.trim().is_empty())
        .map(|h| {
            hbs.render_template(&h.message, &data).unwrap_or_else(|_| {
                h.message
                    .replace("{{time}}", &time_str)
                    .replace("{{context_remaining}}", &remaining.to_string())
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_meta_tools_schema_count() {
        let tools = get_meta_tools_definitions();
        let arr = tools.as_array().expect("array");
        assert_eq!(arr.len(), 18);
        assert_eq!(arr[14]["function"]["name"], "compact");
        assert_eq!(arr[15]["function"]["name"], "sleep");
        assert_eq!(arr[16]["function"]["name"], "history_search");
        assert_eq!(arr[17]["function"]["name"], "history_inspect");
    }

    #[test]
    fn test_truncate_middle_keeps_head_body_tail() {
        let cfg = TruncationConfig {
            head: 10,
            body: 10,
            tail: 10,
        };
        let text: String = (0..1000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let out = truncate_middle(&text, cfg);

        // Short text is untouched.
        assert_eq!(truncate_middle("short", cfg), "short");

        // Head and tail survive verbatim.
        let head: String = text.chars().take(10).collect();
        let tail: String = text.chars().skip(990).collect();
        assert!(out.starts_with(&head));
        assert!(out.ends_with(&tail));

        // Markers report absolute offsets that reconstruct the original.
        assert!(out.contains(TRUNCATION_MARKER_PREFIX));
        let rebuilt = rebuild_from_markers(&text, &out);
        assert!(!rebuilt.is_empty());
        // The visible (non-marker) characters are all preserved in order.
        let stripped: String = out
            .chars()
            .collect::<String>()
            .split(TRUNCATION_MARKER_PREFIX)
            .map(|seg| match seg.find(TRUNCATION_MARKER_SUFFIX) {
                Some(idx) => seg[idx + TRUNCATION_MARKER_SUFFIX.len()..].to_string(),
                None => seg.to_string(),
            })
            .collect();
        let mut it = text.chars();
        for c in stripped.chars() {
            assert!(
                it.any(|orig| orig == c),
                "truncated output contains a char out of order"
            );
        }
        // Output is bounded (segments + two markers).
        assert!(out.chars().count() <= cfg.budget() + 64);
    }

    #[test]
    fn test_truncate_default_marker_shape() {
        let cfg = TruncationConfig::default();
        assert_eq!(cfg.budget(), 3000);
        let text = "z".repeat(10_000);
        let out = truncate_middle(&text, cfg);
        // head + body + tail + two markers
        assert_eq!(out.matches(TRUNCATION_MARKER_PREFIX).count(), 2);
        assert_eq!(
            out.replace(TRUNCATION_MARKER_PREFIX, "")
                .chars()
                .filter(|c| *c == 'z')
                .count(),
            3000
        );
        println!("{}", out.chars().skip(985).take(60).collect::<String>());
    }

    #[test]
    fn test_truncate_middle_is_idempotent() {
        let cfg = TruncationConfig {
            head: 5,
            body: 5,
            tail: 5,
        };
        let text = "x".repeat(500);
        let once = truncate_middle(&text, cfg);
        let twice = truncate_middle(&once, cfg);
        assert_eq!(once, twice, "truncating twice must not truncate again");
    }

    #[test]
    fn test_truncate_middle_utf8_safe() {
        let cfg = TruncationConfig {
            head: 3,
            body: 3,
            tail: 3,
        };
        // Multibyte characters must never be split.
        let text = "日本語テスト🎉".repeat(100);
        let out = truncate_middle(&text, cfg);
        assert!(out.chars().count() < text.chars().count());
        assert!(out.contains(TRUNCATION_MARKER_PREFIX));
    }

    #[test]
    fn test_truncate_middle_disabled() {
        let cfg = TruncationConfig {
            head: 0,
            body: 0,
            tail: 0,
        };
        let text = "y".repeat(500);
        assert_eq!(truncate_middle(&text, cfg), text);
    }

    /// Extracts the region the markers say was elided, verifying the offsets
    /// are consistent (from < to, within bounds).
    fn rebuild_from_markers(original: &str, out: &str) -> String {
        let chars: Vec<char> = original.chars().collect();
        let mut recovered = String::new();
        let mut rest = out;
        while let Some(start) = rest.find(TRUNCATION_MARKER_PREFIX) {
            let after = &rest[start + TRUNCATION_MARKER_PREFIX.len()..];
            let end = after
                .find(TRUNCATION_MARKER_SUFFIX)
                .expect("closing marker");
            let range = &after[..end];
            let (from, to) = range.split_once('-').expect("from-to");
            let from: usize = from.parse().unwrap();
            let to: usize = to.parse().unwrap();
            assert!(from < to, "marker range must be non-empty");
            assert!(to <= chars.len(), "marker out of bounds");
            recovered.extend(chars[from..to].iter());
            rest = &after[end + TRUNCATION_MARKER_SUFFIX.len()..];
        }
        recovered
    }

    #[test]
    fn test_context_header_format() {
        let header = context_header(30_000, 300_000);
        // e.g. "2026-11-02T07:49:33.124Z ctx:10%"
        let (ts, ctx) = header.split_once(" ctx:").expect("space separated ctx");
        assert!(ts.ends_with('Z'), "UTC-Z suffix: {}", ts);
        assert_eq!(ts.len(), "2026-11-02T07:49:33.124Z".len());
        assert!(ts.contains('T') && ts.contains('.'));
        assert_eq!(ctx, "10%");

        // Rounding, clamping and zero-capacity safety.
        // Compare the suffix only: each call stamps its own timestamp, so
        // comparing whole strings races the clock (seen as ...867Z vs ...868Z).
        assert!(context_header(0, 100).ends_with("Z ctx:0%"));
        assert!(context_header(1, 3).ends_with("ctx:33%"));
        assert!(context_header(200, 100).ends_with("ctx:100%"));
        assert!(context_header(5, 0).ends_with("ctx:0%"));

        let body = with_context_header("payload", 1, 2);
        assert!(body.ends_with("\npayload"));
        assert!(body.starts_with("20"));
    }

    #[test]
    fn test_duration_parsing() {
        assert_eq!(
            parse_duration_string("30s").unwrap(),
            Duration::from_secs(30)
        );
        assert_eq!(
            parse_duration_string("1m").unwrap(),
            Duration::from_secs(60)
        );
        assert_eq!(
            parse_duration_string("2h").unwrap(),
            Duration::from_secs(7200)
        );
        assert_eq!(
            parse_duration_string("1d").unwrap(),
            Duration::from_secs(86400)
        );
        assert_eq!(
            parse_duration_string("1h30m").unwrap(),
            Duration::from_secs(5400)
        );
        assert!(parse_duration_string("xyz").is_err());
    }

    #[test]
    fn test_memory_hash_calculation() {
        let h1 = calculate_memory_hash("");
        assert_eq!(
            h1,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_interpolate_prompt_handlebars() {
        let template = "You are {{agent.name}} (ID: {{agent.id}}). Memory: {{agent.memory}} (Hash: {{agent.memory.hash}}). Tools: {{tools.preset}}.";
        let tools = vec!["chat_send".to_string(), "search_repo".to_string()];
        let rendered = interpolate_prompt(
            template,
            "agent-123",
            "Nexura",
            "fallback",
            "Initial user preferences stored",
            "abc123hash",
            &tools,
            295000,
        );
        assert_eq!(
            rendered,
            "You are Nexura (ID: agent-123). Memory: Initial user preferences stored (Hash: abc123hash). Tools: chat_send, search_repo."
        );
    }

    #[test]
    fn test_interpolate_prompt_convenience_aliases() {
        let template = "You are {{name}}. Allowed: {{tools-preset}}.";
        let tools = vec!["chat_send".to_string()];
        let rendered = interpolate_prompt(
            template,
            "agent-123",
            "Alice",
            "fallback",
            "",
            "hash0",
            &tools,
            300000,
        );
        assert_eq!(rendered, "You are Alice. Allowed: chat_send.");
    }

    #[test]
    fn test_interpolate_prompt_empty_tools() {
        let template = "Agent {{agent.name}} tools: {{tools.preset}}";
        let rendered = interpolate_prompt(
            template,
            "agent-123",
            "Bob",
            "fallback",
            "",
            "hash0",
            &[],
            300000,
        );
        assert_eq!(rendered, "Agent Bob tools: (none)");
    }

    #[test]
    fn test_render_response_hooks() {
        let hooks = vec![
            crate::db::schema::ResponseHook {
                id: "hook-1".to_string(),
                message: "Time: {{time}} | Context remaining: {{context_remaining}} chars"
                    .to_string(),
                source: "user".to_string(),
                enabled: true,
            },
            crate::db::schema::ResponseHook {
                id: "hook-2".to_string(),
                message: "Disabled hook".to_string(),
                source: "user".to_string(),
                enabled: false,
            },
        ];

        let rendered = render_response_hooks(&hooks, 5000, 300000);
        assert_eq!(rendered.len(), 1);
        assert!(rendered[0].contains("Context remaining: 295000 chars"));
    }
}
