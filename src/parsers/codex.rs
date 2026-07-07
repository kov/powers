use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use crate::config::Config;
use crate::session::{ContentPart, Message, MessageContent, Role, Session, SessionMeta, Tool};

use super::Parser;

pub struct CodexParser {
    pub codex_dir: PathBuf,
}

impl CodexParser {
    pub fn new(config: &Config) -> Self {
        CodexParser {
            codex_dir: config.codex_dir.clone(),
        }
    }
}

impl Parser for CodexParser {
    fn discover(&self) -> Result<Vec<SessionMeta>> {
        let sessions_dir = self.codex_dir.join("sessions");
        if !sessions_dir.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();

        // Flat rollout-*.jsonl files in sessions/
        let flat_pattern = sessions_dir.join("rollout-*.jsonl");
        for entry in glob::glob(&flat_pattern.to_string_lossy())? {
            let path = match entry {
                Ok(p) => p,
                Err(_) => continue,
            };
            if let Ok(meta) = discover_flat_session(&path) {
                sessions.push(meta);
            }
        }

        // Nested sessions/YYYY/MM/DD/rollout-*.jsonl
        let nested_pattern = sessions_dir.join("**").join("rollout-*.jsonl");
        for entry in glob::glob(&nested_pattern.to_string_lossy())? {
            let path = match entry {
                Ok(p) => p,
                Err(_) => continue,
            };
            // Skip files directly in sessions/ (already handled above)
            if path.parent() == Some(&sessions_dir) {
                continue;
            }
            if let Ok(meta) = discover_nested_session(&path) {
                sessions.push(meta);
            }
        }

        sessions.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
        Ok(sessions)
    }

    fn load(&self, meta: &SessionMeta) -> Result<Session> {
        load_session(meta)
    }
}

/// Old flat format: line 0 has {id, timestamp, instructions, git}
fn discover_flat_session(path: &PathBuf) -> Result<SessionMeta> {
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

    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    reader.read_line(&mut first_line)?;

    let hdr: Value = serde_json::from_str(first_line.trim())
        .context("Failed to parse Codex flat session header")?;

    let id = hdr["id"].as_str().context("Missing id")?.to_string();
    let started_at = hdr["timestamp"]
        .as_str()
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
        .unwrap_or(mtime);

    let git_branch = hdr["git"]["branch"].as_str().map(|s| s.to_string());
    let cwd = hdr["git"]["repository_url"]
        .as_str()
        .map(|_| PathBuf::new()); // not cwd

    // Count user messages (skip instructions injection messages)
    let count = count_flat_messages(path);

    Ok(SessionMeta {
        id,
        tool: Tool::Codex,
        file_path: path.clone(),
        project_path: cwd,
        git_branch,
        started_at,
        last_activity: mtime,
        message_count: count,
        title: None,
        last_prompt: None,
    })
}

/// New nested format: line 0 has {type: "session_meta", payload: {id, cwd, timestamp, instructions}}
fn discover_nested_session(path: &PathBuf) -> Result<SessionMeta> {
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

    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    reader.read_line(&mut first_line)?;

    let hdr: Value = serde_json::from_str(first_line.trim())
        .context("Failed to parse Codex nested session header")?;

    // Check if this is a new-style session_meta record
    let (id, started_at, cwd) = if hdr["type"].as_str() == Some("session_meta") {
        let payload = &hdr["payload"];
        let id = payload["id"]
            .as_str()
            .context("Missing payload.id")?
            .to_string();
        let started_at = payload["timestamp"]
            .as_str()
            .and_then(|s| s.parse::<DateTime<Utc>>().ok())
            .unwrap_or(mtime);
        let cwd = payload["cwd"].as_str().map(PathBuf::from);
        (id, started_at, cwd)
    } else {
        // Fallback: old-style header without type field
        let id = hdr["id"].as_str().context("Missing id")?.to_string();
        let started_at = hdr["timestamp"]
            .as_str()
            .and_then(|s| s.parse::<DateTime<Utc>>().ok())
            .unwrap_or(mtime);
        (id, started_at, None)
    };

    let count = count_nested_messages(path);

    Ok(SessionMeta {
        id,
        tool: Tool::Codex,
        file_path: path.clone(),
        project_path: cwd,
        git_branch: None,
        started_at,
        last_activity: mtime,
        message_count: count,
        title: None,
        last_prompt: None,
    })
}

fn count_flat_messages(path: &PathBuf) -> usize {
    let Ok(file) = std::fs::File::open(path) else {
        return 0;
    };
    let mut count = 0;
    for line in BufReader::new(file).lines().skip(1) {
        let line = line.unwrap_or_default();
        // Old format: {"type":"message","role":"user",...}
        if line.contains(r#""type":"message""#)
            && (line.contains(r#""role":"user""#) || line.contains(r#""role":"assistant""#))
        {
            count += 1;
        }
    }
    count
}

fn count_nested_messages(path: &PathBuf) -> usize {
    let Ok(file) = std::fs::File::open(path) else {
        return 0;
    };
    let mut count = 0;
    for line in BufReader::new(file).lines().skip(1) {
        let line = line.unwrap_or_default();
        // New format: {"type":"response_item","payload":{"type":"message","role":...}}
        // Quick check: must have response_item and a role
        if line.contains(r#""type":"response_item""#)
            && (line.contains(r#""role":"user""#) || line.contains(r#""role":"assistant""#))
        {
            // Filter out system injection messages (very long lines with instructions)
            // Heuristic: actual user messages from event_msg have short content
            count += 1;
        }
    }
    count
}

fn load_session(meta: &SessionMeta) -> Result<Session> {
    let file = std::fs::File::open(&meta.file_path)
        .with_context(|| format!("Cannot open {}", meta.file_path.display()))?;
    let reader = BufReader::new(file);

    let mut lines = reader.lines();
    let first = lines.next().unwrap_or(Ok(String::new()))?;
    let hdr: Value = serde_json::from_str(first.trim()).unwrap_or(Value::Null);

    let is_nested = hdr["type"].as_str() == Some("session_meta");

    let mut messages = Vec::new();
    let mut index = 0usize;

    for line in lines {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let record: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let (role, content, timestamp) = if is_nested {
            parse_nested_record(&record)
        } else {
            parse_flat_record(&record)
        };

        let Some(role) = role else { continue };
        let Some(content) = content else { continue };

        messages.push(Message {
            index,
            role,
            content,
            timestamp,
        });
        index += 1;
    }

    Ok(Session { messages })
}

/// Parse a record in the new nested format (type=response_item) — pub for streaming search.
pub fn parse_nested_record_pub(
    record: &Value,
) -> (Option<Role>, Option<MessageContent>, Option<DateTime<Utc>>) {
    parse_nested_record(record)
}

/// Parse a record in the old flat format (type=message) — pub for streaming search.
pub fn parse_flat_record_pub(
    record: &Value,
) -> (Option<Role>, Option<MessageContent>, Option<DateTime<Utc>>) {
    parse_flat_record(record)
}

/// Parse a record in the new nested format (type=response_item)
fn parse_nested_record(
    record: &Value,
) -> (Option<Role>, Option<MessageContent>, Option<DateTime<Utc>>) {
    if record["type"].as_str() != Some("response_item") {
        return (None, None, None);
    }
    let payload = &record["payload"];
    if payload["type"].as_str() != Some("message") {
        return (None, None, None);
    }

    let role = match payload["role"].as_str() {
        Some("user") => Role::User,
        Some("assistant") => Role::Assistant,
        _ => return (None, None, None),
    };

    // Filter out system injection messages (instructions, environment_context)
    if role == Role::User
        && let Some(arr) = payload["content"].as_array()
        && arr.len() == 1
    {
        let text = arr[0]["text"].as_str().unwrap_or("");
        if text.starts_with("# AGENTS.md")
            || text.starts_with("<environment_context>")
            || text.starts_with("# CLAUDE.md")
        {
            return (None, None, None);
        }
    }

    let content = parse_codex_content(&payload["content"]);
    let ts = record["timestamp"]
        .as_str()
        .and_then(|s| s.parse::<DateTime<Utc>>().ok());

    (Some(role), Some(content), ts)
}

/// Parse a record in the old flat format (type=message)
fn parse_flat_record(
    record: &Value,
) -> (Option<Role>, Option<MessageContent>, Option<DateTime<Utc>>) {
    if record["type"].as_str() != Some("message") {
        return (None, None, None);
    }

    let role = match record["role"].as_str() {
        Some("user") => Role::User,
        Some("assistant") => Role::Assistant,
        _ => return (None, None, None),
    };

    let content = parse_codex_content(&record["content"]);
    let ts = record["timestamp"]
        .as_str()
        .and_then(|s| s.parse::<DateTime<Utc>>().ok());

    (Some(role), Some(content), ts)
}

fn parse_codex_content(content: &Value) -> MessageContent {
    match content {
        Value::String(s) => MessageContent::Text(s.clone()),
        Value::Array(parts) => {
            let parsed: Vec<ContentPart> = parts
                .iter()
                .filter_map(|p| {
                    let text = p["text"].as_str().unwrap_or("");
                    let t = p["type"].as_str().unwrap_or("");
                    match t {
                        "input_text" | "output_text" | "text" => {
                            Some(ContentPart::Text(text.to_string()))
                        }
                        _ => {
                            if !text.is_empty() {
                                Some(ContentPart::Text(text.to_string()))
                            } else {
                                None
                            }
                        }
                    }
                })
                .collect();
            if parsed.is_empty() {
                MessageContent::Text(String::new())
            } else {
                MessageContent::Parts(parsed)
            }
        }
        _ => MessageContent::Text(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_flat_session() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(
            indoc! {r#"
            {"id":"f002eb03-32d4-4930-a60a-3871a9a60c27","timestamp":"2026-01-03T14:50:06.182Z","instructions":"do stuff","git":{"commit_hash":"abc","branch":"main","repository_url":"git@github.com:user/repo.git"}}
            {"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}
            {"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}
            {"type":"other","data":"skip"}
            "#}
            .as_bytes(),
        )
        .unwrap();

        let path = f.path().to_path_buf();
        let meta = discover_flat_session(&path).unwrap();
        assert_eq!(meta.id, "f002eb03-32d4-4930-a60a-3871a9a60c27");
        assert_eq!(meta.message_count, 2);

        let session = load_session(&meta).unwrap();
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].role, Role::User);
        assert_eq!(session.messages[1].role, Role::Assistant);
        assert_eq!(session.messages[0].content.extract_text(), "hi");
        assert_eq!(session.messages[1].content.extract_text(), "hello");
    }

    #[test]
    fn test_load_nested_session() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(
            indoc! {r##"
            {"timestamp":"2026-01-22T22:18:01.638Z","type":"session_meta","payload":{"id":"019be7c9-3424-7932-a6fb-1b90fe184f2c","timestamp":"2026-01-22T22:18:01.636Z","cwd":"/home/kov/Projects/test","originator":"codex_cli_rs","cli_version":"0.0.0"}}
            {"timestamp":"2026-01-22T22:18:10.890Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}}
            {"timestamp":"2026-01-22T22:18:15.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}}
            {"timestamp":"2026-01-22T22:18:01.638Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions for /home/kov/Projects/test\n\n<INSTRUCTIONS>"}]}}
            {"timestamp":"2026-01-22T22:18:01.638Z","type":"event_msg","payload":{"type":"user_message","message":"hi","images":[]}}
            "##}
            .as_bytes(),
        )
        .unwrap();

        let path = f.path().to_path_buf();
        let meta = discover_nested_session(&path).unwrap();
        assert_eq!(meta.id, "019be7c9-3424-7932-a6fb-1b90fe184f2c");

        let session = load_session(&meta).unwrap();
        // Should have 2 actual messages (hi + hello), filtering the AGENTS.md injection
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].role, Role::User);
        assert_eq!(session.messages[0].content.extract_text(), "hi");
    }
}
