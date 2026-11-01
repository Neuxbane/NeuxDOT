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
            current_num = current_num.saturating_mul(10).saturating_add(c as u64 - '0' as u64);
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
                "description": "Execute an external MCP tool with specified arguments",
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
                "name": "setMemory",
                "description": "Set the complete agent memory block (max 5,000 characters). Requires correct currentHash for optimistic locking. Returns updated hash.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "The complete memory text to persist (maximum 5000 characters)"
                        },
                        "currentHash": {
                            "type": "string",
                            "description": "The current hash of the memory. If memory is empty, obtain it via readMemory()"
                        }
                    },
                    "required": ["content", "currentHash"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "editMemory",
                "description": "Perform a search-and-replace edit on the agent memory. Requires correct currentHash. Returns updated hash.",
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
                "name": "readMemory",
                "description": "Read the full, untruncated agent memory block and its current hash.",
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
                "description": "Compact running conversation history so it retains only the system prompt, current memory, and current hash.",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "sleep",
                "description": "Pause agent execution for a specified duration (e.g. '1m', '30s', '2h', '1d'). Requires duration or time parameter. If interruptible is true (default), incoming events can wake the agent early.",
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
                            "description": "Whether incoming notifications can interrupt the sleep early (default: true)"
                        }
                    }
                }
            }
        }
    ])
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

    let time_str = chrono::Local::now().format("%l:%M %p").to_string().trim().to_string();

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
    let time_str = chrono::Local::now().format("%l:%M %p").to_string().trim().to_string();

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
        assert_eq!(arr.len(), 9);
        assert_eq!(arr[8]["function"]["name"], "sleep");
    }

    #[test]
    fn test_duration_parsing() {
        assert_eq!(parse_duration_string("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration_string("1m").unwrap(), Duration::from_secs(60));
        assert_eq!(parse_duration_string("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration_string("1d").unwrap(), Duration::from_secs(86400));
        assert_eq!(parse_duration_string("1h30m").unwrap(), Duration::from_secs(5400));
        assert!(parse_duration_string("xyz").is_err());
    }

    #[test]
    fn test_memory_hash_calculation() {
        let h1 = calculate_memory_hash("");
        assert_eq!(h1, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
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
                message: "Time: {{time}} | Context remaining: {{context_remaining}} chars".to_string(),
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
