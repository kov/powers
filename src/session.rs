use chrono::{DateTime, Utc};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum Tool {
    Claude,
    Codex,
    Gemini,
    Copilot,
}

impl std::fmt::Display for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tool::Claude => write!(f, "claude"),
            Tool::Codex => write!(f, "codex"),
            Tool::Gemini => write!(f, "gemini"),
            Tool::Copilot => write!(f, "copilot"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    User,
    Assistant,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub id: String,
    pub tool: Tool,
    pub file_path: PathBuf,
    pub project_path: Option<PathBuf>,
    pub git_branch: Option<String>,
    pub started_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub message_count: usize,
    /// Agent-generated session title (Claude `ai-title`); None for tools that don't emit one.
    pub title: Option<String>,
    /// Most recent user prompt recorded for the session (Claude `last-prompt`).
    pub last_prompt: Option<String>,
}

impl SessionMeta {
    pub fn matches_prefix(&self, prefix: &str) -> bool {
        self.id.starts_with(prefix)
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub index: usize,
    pub role: Role,
    pub content: MessageContent,
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl MessageContent {
    pub fn extract_text(&self) -> String {
        self.extract_searchable_text(false)
    }

    /// Like `extract_text`, but optionally includes the raw JSON of tool-call inputs
    /// so searches can match text the assistant wrote inside Edit/Write/Bash args.
    /// Tool results are still excluded (they contain noisy log output).
    pub fn extract_searchable_text(&self, include_tool_calls: bool) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Parts(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text(t) => Some(t.as_str()),
                    ContentPart::Thinking(t) => Some(t.as_str()),
                    ContentPart::ToolCall { input, .. } if include_tool_calls => {
                        Some(input.as_str())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    pub fn without_tool_calls(&self) -> MessageContent {
        match self {
            MessageContent::Text(s) => MessageContent::Text(s.clone()),
            MessageContent::Parts(parts) => {
                let filtered: Vec<ContentPart> = parts
                    .iter()
                    .filter(|p| {
                        !matches!(
                            p,
                            ContentPart::ToolCall { .. } | ContentPart::ToolResult { .. }
                        )
                    })
                    .cloned()
                    .collect();
                if filtered.is_empty() {
                    MessageContent::Text(String::new())
                } else {
                    MessageContent::Parts(filtered)
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            MessageContent::Text(s) => s.trim().is_empty(),
            MessageContent::Parts(parts) => parts.is_empty(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ContentPart {
    Text(String),
    Thinking(String),
    ToolCall {
        /// Provider tool-use id (Claude `toolu_…`), used to attribute a later
        /// tool_result back to this call. None for tools that don't expose one.
        id: Option<String>,
        name: String,
        input: String,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_prefix() {
        let meta = SessionMeta {
            id: "5241ab6c-5c33-4285-826c-5acfd8cc0090".to_string(),
            tool: Tool::Claude,
            file_path: PathBuf::from("/tmp/test.jsonl"),
            project_path: None,
            git_branch: None,
            started_at: Utc::now(),
            last_activity: Utc::now(),
            message_count: 0,
            title: None,
            last_prompt: None,
        };
        assert!(meta.matches_prefix("5241ab6c"));
        assert!(meta.matches_prefix("5241ab6c-5c33"));
        assert!(!meta.matches_prefix("deadbeef"));
    }

    #[test]
    fn test_extract_text() {
        let content = MessageContent::Parts(vec![
            ContentPart::Text("hello".to_string()),
            ContentPart::ToolCall {
                id: None,
                name: "bash".to_string(),
                input: "ls".to_string(),
            },
            ContentPart::Text("world".to_string()),
        ]);
        assert_eq!(content.extract_text(), "hello\nworld");
    }

    #[test]
    fn test_extract_searchable_text_includes_tool_calls() {
        let content = MessageContent::Parts(vec![
            ContentPart::Text("hello".to_string()),
            ContentPart::ToolCall {
                id: None,
                name: "Edit".to_string(),
                input: r#"{"new_string":"fn foo() {}"}"#.to_string(),
            },
            ContentPart::ToolResult {
                tool_use_id: "x".to_string(),
                content: "noisy log line".to_string(),
                is_error: false,
            },
        ]);
        assert_eq!(content.extract_searchable_text(false), "hello");
        let with_tools = content.extract_searchable_text(true);
        assert!(with_tools.contains("hello"));
        assert!(with_tools.contains("fn foo"));
        assert!(!with_tools.contains("noisy log line"));
    }

    #[test]
    fn test_without_tool_calls() {
        let content = MessageContent::Parts(vec![
            ContentPart::ToolCall {
                id: None,
                name: "bash".to_string(),
                input: "ls".to_string(),
            },
            ContentPart::ToolResult {
                tool_use_id: "x".to_string(),
                content: "file.txt".to_string(),
                is_error: false,
            },
        ]);
        let filtered = content.without_tool_calls();
        assert!(filtered.is_empty());
    }
}
