use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use crate::config::Config;
use crate::session::{ContentPart, Message, MessageContent, Role, Session, SessionMeta, Tool};

use super::Parser;

pub struct CopilotParser {
    pub copilot_dir: PathBuf,
}

impl CopilotParser {
    pub fn new(config: &Config) -> Self {
        CopilotParser {
            copilot_dir: config.copilot_dir.clone(),
        }
    }
}

impl Parser for CopilotParser {
    fn discover(&self) -> Result<Vec<SessionMeta>> {
        let pattern = self.copilot_dir.join("*").join("workspace.yaml");
        let pattern_str = pattern.to_string_lossy();

        let mut sessions = Vec::new();

        for entry in glob::glob(&pattern_str).context("Invalid glob pattern")? {
            let path = match entry {
                Ok(p) => p,
                Err(_) => continue,
            };

            match discover_session(&path) {
                Ok(meta) => sessions.push(meta),
                Err(_) => continue,
            }
        }

        sessions.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
        Ok(sessions)
    }

    fn load(&self, meta: &SessionMeta) -> Result<Session> {
        load_session(meta)
    }
}

fn discover_session(workspace_yaml: &PathBuf) -> Result<SessionMeta> {
    let content = std::fs::read_to_string(workspace_yaml)
        .with_context(|| format!("Cannot read {}", workspace_yaml.display()))?;

    let mut id: Option<String> = None;
    let mut cwd: Option<PathBuf> = None;
    let mut created_at: Option<DateTime<Utc>> = None;
    let mut updated_at: Option<DateTime<Utc>> = None;

    for line in content.lines() {
        if let Some(val) = line.strip_prefix("id:") {
            id = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("cwd:") {
            cwd = Some(PathBuf::from(val.trim()));
        } else if let Some(val) = line.strip_prefix("created_at:") {
            created_at = DateTime::parse_from_rfc3339(val.trim())
                .ok()
                .map(|dt| dt.with_timezone(&Utc));
        } else if let Some(val) = line.strip_prefix("updated_at:") {
            updated_at = DateTime::parse_from_rfc3339(val.trim())
                .ok()
                .map(|dt| dt.with_timezone(&Utc));
        }
    }

    let id = id.context("workspace.yaml missing 'id' field")?;
    let now = Utc::now();
    let started_at = created_at.unwrap_or(now);
    let last_activity = updated_at.unwrap_or(started_at);

    // Prefer events.jsonl for git branch and accurate message count; fall back gracefully.
    let events_path = workspace_yaml
        .parent()
        .map(|p| p.join("events.jsonl"))
        .unwrap_or_default();

    let (git_branch, message_count) = if events_path.exists() {
        read_events_metadata(&events_path)
    } else {
        (None, 0)
    };

    Ok(SessionMeta {
        id,
        tool: Tool::Copilot,
        file_path: events_path,
        project_path: cwd,
        git_branch,
        started_at,
        last_activity,
        message_count,
        title: None,
        last_prompt: None,
    })
}

fn read_events_metadata(events_path: &PathBuf) -> (Option<String>, usize) {
    let Ok(file) = std::fs::File::open(events_path) else {
        return (None, 0);
    };
    let reader = BufReader::new(file);
    let mut git_branch: Option<String> = None;
    let mut count = 0usize;

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        match record["type"].as_str() {
            Some("session.start") => {
                if let Some(branch) = record["data"]["context"]["branch"].as_str() {
                    git_branch = Some(branch.to_string());
                }
            }
            Some("user.message") | Some("assistant.message") | Some("tool.execution_complete") => {
                count += 1;
            }
            _ => {}
        }
    }

    (git_branch, count)
}

fn load_session(meta: &SessionMeta) -> Result<Session> {
    if !meta.file_path.exists() {
        return Ok(Session { messages: vec![] });
    }

    let file = std::fs::File::open(&meta.file_path)
        .with_context(|| format!("Cannot open {}", meta.file_path.display()))?;
    let reader = BufReader::new(file);
    let mut messages = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if let Some(msg) = parse_copilot_event_pub(&record) {
            messages.push(msg);
        }
    }

    // Assign stable 0-based indices
    for (i, msg) in messages.iter_mut().enumerate() {
        msg.index = i;
    }

    Ok(Session { messages })
}

/// Parse a single Copilot CLI event record into a Message, if applicable.
/// Returns None for non-message event types.
pub fn parse_copilot_event_pub(record: &Value) -> Option<Message> {
    let event_type = record["type"].as_str()?;
    let data = &record["data"];
    let timestamp = record["timestamp"]
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    match event_type {
        "user.message" => {
            let text = data["content"].as_str().unwrap_or("").to_string();
            if text.is_empty() {
                return None;
            }
            Some(Message {
                index: 0,
                role: Role::User,
                content: MessageContent::Text(text),
                timestamp,
            })
        }
        "assistant.message" => {
            let mut parts: Vec<ContentPart> = Vec::new();

            let text = data["content"].as_str().unwrap_or("");
            if !text.is_empty() {
                parts.push(ContentPart::Text(text.to_string()));
            }

            if let Some(requests) = data["toolRequests"].as_array() {
                for req in requests {
                    let id = req["toolCallId"].as_str().map(|s| s.to_string());
                    let name = req["name"].as_str().unwrap_or("").to_string();
                    let input = req["arguments"].to_string();
                    if !name.is_empty() {
                        parts.push(ContentPart::ToolCall { id, name, input });
                    }
                }
            }

            if parts.is_empty() {
                return None;
            }

            Some(Message {
                index: 0,
                role: Role::Assistant,
                content: MessageContent::Parts(parts),
                timestamp,
            })
        }
        "tool.execution_complete" => {
            let tool_use_id = data["toolCallId"].as_str().unwrap_or("").to_string();
            if tool_use_id.is_empty() {
                return None;
            }
            let content = match &data["result"] {
                Value::Object(_) => data["result"]["content"].as_str().unwrap_or("").to_string(),
                Value::String(s) => s.clone(),
                _ => String::new(),
            };
            // `success` is present and true on ok results; treat missing as ok.
            let is_error = !data["success"].as_bool().unwrap_or(true);
            Some(Message {
                index: 0,
                role: Role::User,
                content: MessageContent::Parts(vec![ContentPart::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                }]),
                timestamp,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn make_events_jsonl(events: &[&str]) -> NamedTempFile {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(file.path(), events.join("\n")).unwrap();
        file
    }

    #[test]
    fn test_parse_user_message() {
        let record: Value = serde_json::from_str(
            r#"{"type":"user.message","data":{"content":"hello world"},"timestamp":"2026-02-24T21:00:00Z","id":"x"}"#,
        )
        .unwrap();
        let msg = parse_copilot_event_pub(&record).unwrap();
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.extract_text(), "hello world");
    }

    #[test]
    fn test_parse_assistant_message_with_tool_call() {
        let record: Value = serde_json::from_str(
            r#"{"type":"assistant.message","data":{"content":"sure","toolRequests":[{"name":"bash","arguments":{"command":"ls"},"toolCallId":"x","type":"function"}]},"timestamp":"2026-02-24T21:01:00Z","id":"y"}"#,
        )
        .unwrap();
        let msg = parse_copilot_event_pub(&record).unwrap();
        assert_eq!(msg.role, Role::Assistant);
        let text = msg.content.without_tool_calls();
        assert!(!text.is_empty());
        // The tool_use id (toolCallId) must be captured for tool_result attribution.
        match &msg.content {
            MessageContent::Parts(parts) => {
                let has_id = parts
                    .iter()
                    .any(|p| matches!(p, ContentPart::ToolCall { id: Some(id), .. } if id == "x"));
                assert!(has_id, "toolCallId should populate ToolCall.id");
            }
            _ => panic!("expected Parts"),
        }
    }

    #[test]
    fn test_parse_tool_execution_complete_failure_sets_is_error() {
        let record: Value = serde_json::from_str(
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"tooluse_err","success":false,"result":{"content":"boom"}},"timestamp":"2026-02-24T21:02:00Z","id":"z"}"#,
        )
        .unwrap();
        let msg = parse_copilot_event_pub(&record).unwrap();
        match &msg.content {
            MessageContent::Parts(parts) => match &parts[0] {
                ContentPart::ToolResult { is_error, .. } => {
                    assert!(is_error, "success:false must set is_error");
                }
                _ => panic!("expected ToolResult"),
            },
            _ => panic!("expected Parts"),
        }
    }

    #[test]
    fn test_parse_skips_non_message_events() {
        let record: Value = serde_json::from_str(
            r#"{"type":"session.start","data":{},"timestamp":"2026-02-24T21:00:00Z","id":"z"}"#,
        )
        .unwrap();
        assert!(parse_copilot_event_pub(&record).is_none());
    }

    #[test]
    fn test_parse_tool_execution_complete() {
        let record: Value = serde_json::from_str(
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"tooluse_abc123","success":true,"result":{"content":"file.txt\nother.txt","detailedContent":"diff output"}},"timestamp":"2026-02-24T21:02:00Z","id":"z"}"#,
        )
        .unwrap();
        let msg = parse_copilot_event_pub(&record).unwrap();
        assert_eq!(msg.role, Role::User);
        match &msg.content {
            MessageContent::Parts(parts) => {
                assert_eq!(parts.len(), 1);
                match &parts[0] {
                    ContentPart::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } => {
                        assert_eq!(tool_use_id, "tooluse_abc123");
                        assert_eq!(content, "file.txt\nother.txt");
                    }
                    _ => panic!("expected ToolResult"),
                }
            }
            _ => panic!("expected Parts"),
        }
    }

    #[test]
    fn test_parse_tool_execution_complete_missing_tool_call_id_skipped() {
        let record: Value = serde_json::from_str(
            r#"{"type":"tool.execution_complete","data":{"success":true,"result":{"content":"output"}},"timestamp":"2026-02-24T21:02:00Z","id":"z"}"#,
        )
        .unwrap();
        assert!(parse_copilot_event_pub(&record).is_none());
    }

    #[test]
    fn test_parse_tool_execution_complete_empty_result_content() {
        let record: Value = serde_json::from_str(
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"tooluse_xyz","success":true,"result":{}},"timestamp":"2026-02-24T21:02:00Z","id":"z"}"#,
        )
        .unwrap();
        let msg = parse_copilot_event_pub(&record).unwrap();
        match &msg.content {
            MessageContent::Parts(parts) => match &parts[0] {
                ContentPart::ToolResult { content, .. } => assert_eq!(content, ""),
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn test_read_events_metadata_counts_tool_executions() {
        let file = make_events_jsonl(&[
            r#"{"type":"session.start","data":{"context":{"branch":"main"}},"timestamp":"2026-02-24T21:00:00Z","id":"a"}"#,
            r#"{"type":"user.message","data":{"content":"hello"},"timestamp":"2026-02-24T21:01:00Z","id":"b"}"#,
            r#"{"type":"assistant.message","data":{"content":"hi","toolRequests":[{"name":"bash","arguments":{},"toolCallId":"t1"}]},"timestamp":"2026-02-24T21:02:00Z","id":"c"}"#,
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"t1","success":true,"result":{"content":"ok"}},"timestamp":"2026-02-24T21:03:00Z","id":"d"}"#,
        ]);
        let (branch, count) = read_events_metadata(&file.path().to_path_buf());
        assert_eq!(branch.as_deref(), Some("main"));
        assert_eq!(count, 3); // user.message + assistant.message + tool.execution_complete
    }

    #[test]
    fn test_read_events_metadata() {
        let file = make_events_jsonl(&[
            r#"{"type":"session.start","data":{"context":{"branch":"main"}},"timestamp":"2026-02-24T21:00:00Z","id":"a"}"#,
            r#"{"type":"user.message","data":{"content":"hello"},"timestamp":"2026-02-24T21:01:00Z","id":"b"}"#,
            r#"{"type":"assistant.message","data":{"content":"hi"},"timestamp":"2026-02-24T21:02:00Z","id":"c"}"#,
        ]);
        let (branch, count) = read_events_metadata(&file.path().to_path_buf());
        assert_eq!(branch.as_deref(), Some("main"));
        assert_eq!(count, 2);
    }
}
