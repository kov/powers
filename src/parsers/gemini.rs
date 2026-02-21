use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::session::{ContentPart, Message, MessageContent, Role, Session, SessionMeta, Tool};

use super::Parser;

pub struct GeminiParser {
    pub gemini_dir: PathBuf,
}

impl GeminiParser {
    pub fn new(config: &Config) -> Self {
        GeminiParser {
            gemini_dir: config.gemini_dir.clone(),
        }
    }
}

impl Parser for GeminiParser {
    fn discover(&self) -> Result<Vec<SessionMeta>> {
        let tmp_dir = self.gemini_dir.join("tmp");
        if !tmp_dir.exists() {
            return Ok(Vec::new());
        }

        let pattern = tmp_dir.join("*").join("chats").join("*.json");
        let mut sessions = Vec::new();

        for entry in glob::glob(&pattern.to_string_lossy())? {
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

fn discover_session(path: &PathBuf) -> Result<SessionMeta> {
    let file =
        std::fs::File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let mtime = file
        .metadata()
        .map(|m| {
            m.modified()
                .map(DateTime::from)
                .unwrap_or_else(|_| Utc::now())
        })
        .unwrap_or_else(|_| Utc::now());

    // Fast path: read just the top-level keys (sessionId, projectHash, startTime, lastUpdated)
    // by reading the file and parsing only what we need
    let content =
        std::fs::read_to_string(path).with_context(|| format!("Cannot read {}", path.display()))?;

    let data: Value = serde_json::from_str(&content).context("Failed to parse Gemini JSON")?;

    let session_id = data["sessionId"]
        .as_str()
        .context("Missing sessionId")?
        .to_string();

    let started_at = data["startTime"]
        .as_str()
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
        .unwrap_or(mtime);

    // Project path: try to resolve from the parent directory name (project hash)
    let project_path = resolve_project_path(path);

    // Count user + gemini messages
    let message_count = data["messages"]
        .as_array()
        .map(|msgs| {
            msgs.iter()
                .filter(|m| matches!(m["type"].as_str(), Some("user") | Some("gemini")))
                .count()
        })
        .unwrap_or(0);

    Ok(SessionMeta {
        id: session_id,
        tool: Tool::Gemini,
        file_path: path.clone(),
        project_path,
        git_branch: None,
        started_at,
        last_activity: mtime,
        message_count,
    })
}

/// Try to resolve a project path from the hash directory name.
/// The Gemini CLI hashes the project path using SHA-256.
/// We enumerate candidate paths and compute their hashes to find a match.
fn resolve_project_path(session_path: &Path) -> Option<PathBuf> {
    // Path structure: ~/.gemini/tmp/<hash_or_name>/chats/session-*.json
    let hash_dir = session_path.parent()?.parent()?;
    let dir_name = hash_dir.file_name()?.to_str()?;

    // If it doesn't look like a SHA-256 hex hash, it's a named dir — return the name
    if dir_name.len() != 64 || !dir_name.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(PathBuf::from(dir_name));
    }

    // Try to find the project by hashing candidate paths
    let hash_bytes = hex::decode(dir_name).ok()?;

    // Candidates: home dir and common project dirs
    let home = dirs::home_dir()?;
    let mut candidates = vec![home.clone()];

    // Check common project locations
    for subdir in &["Projects", "Code", "Work", "src", "repos"] {
        let d = home.join(subdir);
        if d.exists() {
            // Add immediate subdirectories
            if let Ok(entries) = std::fs::read_dir(&d) {
                for entry in entries.flatten() {
                    candidates.push(entry.path());
                }
            }
        }
    }

    // Also check current working directory and its ancestors
    if let Ok(cwd) = std::env::current_dir() {
        let mut p = cwd.as_path();
        loop {
            candidates.push(p.to_path_buf());
            match p.parent() {
                Some(parent) if parent != p => p = parent,
                _ => break,
            }
        }
    }

    // Hash each candidate and compare
    for candidate in &candidates {
        if let Some(computed) = hash_path(candidate)
            && computed == hash_bytes
        {
            return Some(candidate.clone());
        }
    }

    None
}

fn hash_path(path: &Path) -> Option<Vec<u8>> {
    use sha2::{Digest, Sha256};
    let path_str = path.to_str()?;
    let mut hasher = Sha256::new();
    hasher.update(path_str.as_bytes());
    Some(hasher.finalize().to_vec())
}

fn load_session(meta: &SessionMeta) -> Result<Session> {
    let content = std::fs::read_to_string(&meta.file_path)
        .with_context(|| format!("Cannot read {}", meta.file_path.display()))?;

    let data: Value = serde_json::from_str(&content).context("Failed to parse Gemini JSON")?;

    let messages_arr = data["messages"]
        .as_array()
        .context("Missing messages array")?;

    let mut messages = Vec::new();
    let mut index = 0usize;

    for msg in messages_arr {
        let msg_type = msg["type"].as_str().unwrap_or("");
        let role = match msg_type {
            "user" => Role::User,
            "gemini" => Role::Assistant,
            _ => continue, // skip info, error, etc.
        };

        let content = parse_gemini_content(&msg["content"]);
        let timestamp = msg["timestamp"]
            .as_str()
            .and_then(|s| s.parse::<DateTime<Utc>>().ok());

        // Check for tool calls on gemini messages
        let has_tool_calls = msg["toolCalls"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false);

        let final_content = if has_tool_calls {
            // Combine text content with tool call parts
            let mut parts = Vec::new();

            // Add any text content
            match &content {
                MessageContent::Text(s) if !s.is_empty() => {
                    parts.push(ContentPart::Text(s.clone()));
                }
                MessageContent::Parts(p) => parts.extend(p.clone()),
                _ => {}
            }

            // Add tool calls
            if let Some(tool_calls) = msg["toolCalls"].as_array() {
                for tc in tool_calls {
                    let name = tc["name"].as_str().unwrap_or("").to_string();
                    let input = serde_json::to_string(&tc["args"]).unwrap_or_default();
                    parts.push(ContentPart::ToolCall { name, input });
                }
            }

            if parts.is_empty() {
                MessageContent::Text(String::new())
            } else {
                MessageContent::Parts(parts)
            }
        } else {
            content
        };

        messages.push(Message {
            index,
            role,
            content: final_content,
            timestamp,
        });
        index += 1;
    }

    Ok(Session { messages })
}

fn parse_gemini_content(content: &Value) -> MessageContent {
    match content {
        Value::String(s) => MessageContent::Text(s.clone()),
        Value::Array(parts) => {
            let text = parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("");
            MessageContent::Text(text)
        }
        _ => MessageContent::Text(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn make_gemini_fixture() -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        let json = serde_json::json!({
            "sessionId": "f804b513-e9a5-40fc-8db5-1662ca2c3c5b",
            "projectHash": "abc123",
            "startTime": "2026-02-21T16:35:03.158Z",
            "lastUpdated": "2026-02-21T16:36:00.000Z",
            "messages": [
                {
                    "id": "msg-1",
                    "timestamp": "2026-02-21T16:35:10.000Z",
                    "type": "user",
                    "content": "hello gemini"
                },
                {
                    "id": "msg-2",
                    "timestamp": "2026-02-21T16:35:20.000Z",
                    "type": "gemini",
                    "content": "hello back"
                },
                {
                    "id": "msg-info",
                    "timestamp": "2026-02-21T16:35:30.000Z",
                    "type": "info",
                    "content": "context compressed"
                }
            ]
        });
        writeln!(f, "{}", json).unwrap();
        f
    }

    #[test]
    fn test_discover_gemini_session() {
        let fixture = make_gemini_fixture();
        let path = fixture.path().to_path_buf();
        let meta = discover_session(&path).unwrap();
        assert_eq!(meta.id, "f804b513-e9a5-40fc-8db5-1662ca2c3c5b");
        assert_eq!(meta.message_count, 2); // user + gemini, not info
    }

    #[test]
    fn test_load_gemini_session() {
        let fixture = make_gemini_fixture();
        let path = fixture.path().to_path_buf();
        let meta = discover_session(&path).unwrap();
        let session = load_session(&meta).unwrap();
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].role, Role::User);
        assert_eq!(session.messages[1].role, Role::Assistant);
        assert_eq!(session.messages[0].content.extract_text(), "hello gemini");
        assert_eq!(session.messages[1].content.extract_text(), "hello back");
    }
}
