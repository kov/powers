use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use crate::config::Config;
use crate::session::{ContentPart, Message, MessageContent, Role, Session, SessionMeta, Tool};

use super::Parser;

pub struct ClaudeParser {
    pub claude_dir: PathBuf,
}

impl ClaudeParser {
    pub fn new(config: &Config) -> Self {
        ClaudeParser {
            claude_dir: config.claude_dir.clone(),
        }
    }
}

impl Parser for ClaudeParser {
    fn discover(&self) -> Result<Vec<SessionMeta>> {
        let pattern = self.claude_dir.join("projects").join("*").join("*.jsonl");
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

    // sessionId appears on every record, but not all record types carry cwd/gitBranch.
    // Scan forward until we find a record with sessionId; grab cwd/gitBranch from the
    // first record that has them (typically the system or first user record).
    let reader = BufReader::new(file);

    let mut session_id: Option<String> = None;
    let mut started_at = mtime;
    let mut cwd: Option<PathBuf> = None;
    let mut git_branch: Option<String> = None;

    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        if let Some(id) = record["sessionId"].as_str()
            && session_id.is_none()
        {
            session_id = Some(id.to_string());
            started_at = record["timestamp"]
                .as_str()
                .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                .unwrap_or(mtime);
        }
        if cwd.is_none()
            && let Some(c) = record["cwd"].as_str()
        {
            cwd = Some(PathBuf::from(c));
        }
        if git_branch.is_none()
            && let Some(b) = record["gitBranch"].as_str()
        {
            git_branch = Some(b.to_string());
        }
        // Stop once we have everything
        if session_id.is_some() && cwd.is_some() && git_branch.is_some() {
            break;
        }
    }

    let session_id = session_id.context("No sessionId found in Claude JSONL")?;

    // Count user+assistant messages with a quick scan
    let remaining = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut count = 0usize;
    for line in remaining.lines() {
        let line = line.unwrap_or_default();
        if line.contains(r#""type":"user""#) || line.contains(r#""type":"assistant""#) {
            count += 1;
        }
    }

    Ok(SessionMeta {
        id: session_id,
        tool: Tool::Claude,
        file_path: path.clone(),
        project_path: cwd,
        git_branch,
        started_at,
        last_activity: mtime,
        message_count: count,
    })
}

fn load_session(meta: &SessionMeta) -> Result<Session> {
    let file = std::fs::File::open(&meta.file_path)
        .with_context(|| format!("Cannot open {}", meta.file_path.display()))?;
    let reader = BufReader::new(file);

    let mut messages = Vec::new();
    let mut index = 0usize;

    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let record: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let record_type = match record["type"].as_str() {
            Some(t) => t,
            None => continue,
        };

        let (role, content, timestamp) = match record_type {
            "user" => {
                let msg = &record["message"];
                let content = parse_claude_user_content(&msg["content"]);
                let ts = record["timestamp"]
                    .as_str()
                    .and_then(|s| s.parse::<DateTime<Utc>>().ok());
                (Role::User, content, ts)
            }
            "assistant" => {
                let msg = &record["message"];
                let content = parse_claude_assistant_content(&msg["content"]);
                let ts = record["timestamp"]
                    .as_str()
                    .and_then(|s| s.parse::<DateTime<Utc>>().ok());
                (Role::Assistant, content, ts)
            }
            _ => continue,
        };

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

/// Parse a single already-decoded JSON record into a Message (for streaming search).
pub fn parse_message_from_record(record: &Value, record_type: &str) -> Option<Message> {
    let (role, content, timestamp) = match record_type {
        "user" => {
            let msg = &record["message"];
            let content = parse_claude_user_content(&msg["content"]);
            let ts = record["timestamp"]
                .as_str()
                .and_then(|s| s.parse::<DateTime<Utc>>().ok());
            (Role::User, content, ts)
        }
        "assistant" => {
            let msg = &record["message"];
            let content = parse_claude_assistant_content(&msg["content"]);
            let ts = record["timestamp"]
                .as_str()
                .and_then(|s| s.parse::<DateTime<Utc>>().ok());
            (Role::Assistant, content, ts)
        }
        _ => return None,
    };
    Some(Message {
        index: 0, // streaming: caller assigns final index if needed
        role,
        content,
        timestamp,
    })
}

fn parse_claude_user_content(content: &Value) -> MessageContent {
    match content {
        Value::String(s) => MessageContent::Text(s.clone()),
        Value::Array(parts) => {
            let parsed: Vec<ContentPart> =
                parts.iter().filter_map(parse_claude_content_part).collect();
            if parsed.is_empty() {
                MessageContent::Text(String::new())
            } else {
                MessageContent::Parts(parsed)
            }
        }
        _ => MessageContent::Text(String::new()),
    }
}

fn parse_claude_assistant_content(content: &Value) -> MessageContent {
    match content {
        Value::String(s) => MessageContent::Text(s.clone()),
        Value::Array(parts) => {
            let parsed: Vec<ContentPart> =
                parts.iter().filter_map(parse_claude_content_part).collect();
            if parsed.is_empty() {
                MessageContent::Text(String::new())
            } else {
                MessageContent::Parts(parsed)
            }
        }
        _ => MessageContent::Text(String::new()),
    }
}

fn parse_claude_content_part(part: &Value) -> Option<ContentPart> {
    let part_type = part["type"].as_str()?;
    match part_type {
        "text" => {
            let text = part["text"].as_str().unwrap_or("").to_string();
            Some(ContentPart::Text(text))
        }
        "thinking" => {
            let text = part["thinking"].as_str().unwrap_or("").to_string();
            Some(ContentPart::Thinking(text))
        }
        "tool_use" => {
            let name = part["name"].as_str().unwrap_or("").to_string();
            let input = serde_json::to_string(&part["input"]).unwrap_or_default();
            Some(ContentPart::ToolCall { name, input })
        }
        "tool_result" => {
            let tool_use_id = part["tool_use_id"].as_str().unwrap_or("").to_string();
            let content = match &part["content"] {
                Value::String(s) => s.clone(),
                Value::Array(parts) => parts
                    .iter()
                    .filter_map(|p| p["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                other => serde_json::to_string(other).unwrap_or_default(),
            };
            Some(ContentPart::ToolResult {
                tool_use_id,
                content,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn make_fixture() -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(
            indoc! {r#"
            {"parentUuid":null,"cwd":"/home/kov/Projects/test","sessionId":"aaaabbbb-0000-0000-0000-000000000001","version":"2.0","gitBranch":"main","type":"system","timestamp":"2026-02-21T12:00:00.000Z","uuid":"sys-1"}
            {"type":"user","message":{"role":"user","content":"hello world"},"timestamp":"2026-02-21T12:01:00.000Z","uuid":"u-1","sessionId":"aaaabbbb-0000-0000-0000-000000000001"}
            {"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello back"}]},"timestamp":"2026-02-21T12:02:00.000Z","uuid":"a-1","sessionId":"aaaabbbb-0000-0000-0000-000000000001"}
            {"type":"progress","uuid":"p-1"}
            "#}
            .as_bytes(),
        )
        .unwrap();
        f
    }

    #[test]
    fn test_load_session() {
        let fixture = make_fixture();
        let path = fixture.path().to_path_buf();

        let meta = SessionMeta {
            id: "aaaabbbb-0000-0000-0000-000000000001".to_string(),
            tool: Tool::Claude,
            file_path: path,
            project_path: Some(PathBuf::from("/home/kov/Projects/test")),
            git_branch: Some("main".to_string()),
            started_at: Utc::now(),
            last_activity: Utc::now(),
            message_count: 2,
        };

        let session = load_session(&meta).unwrap();
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].role, Role::User);
        assert_eq!(session.messages[1].role, Role::Assistant);
        assert_eq!(session.messages[0].index, 0);
        assert_eq!(session.messages[1].index, 1);
        assert_eq!(session.messages[0].content.extract_text(), "hello world");
        assert_eq!(session.messages[1].content.extract_text(), "hello back");
    }

    #[test]
    fn test_discover_session() {
        let fixture = make_fixture();
        let path = fixture.path().to_path_buf();
        let meta = discover_session(&path).unwrap();
        assert_eq!(meta.id, "aaaabbbb-0000-0000-0000-000000000001");
        assert_eq!(meta.message_count, 2);
        assert_eq!(meta.git_branch.as_deref(), Some("main"));
    }

    #[test]
    fn test_discover_session_with_file_history_snapshot_first() {
        let mut fixture = NamedTempFile::new().unwrap();
        fixture
            .write_all(
                indoc! {r#"
                {"type":"file-history-snapshot","uuid":"fh-1","timestamp":"2026-02-21T12:00:00.000Z"}
                {"type":"user","message":{"role":"user","content":"hello world"},"timestamp":"2026-02-21T12:01:00.000Z","uuid":"u-1","sessionId":"aaaabbbb-0000-0000-0000-000000000001","cwd":"/home/kov/Projects/test","gitBranch":"main"}
                {"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello back"}]},"timestamp":"2026-02-21T12:02:00.000Z","uuid":"a-1","sessionId":"aaaabbbb-0000-0000-0000-000000000001"}
                "#}
                .as_bytes(),
            )
            .unwrap();

        let path = fixture.path().to_path_buf();
        let meta = discover_session(&path).unwrap();
        assert_eq!(meta.id, "aaaabbbb-0000-0000-0000-000000000001");
        assert_eq!(meta.message_count, 2);
        assert_eq!(
            meta.project_path.as_deref(),
            Some(std::path::Path::new("/home/kov/Projects/test"))
        );
        assert_eq!(meta.git_branch.as_deref(), Some("main"));
    }

    // ── parse_claude_content_part edge cases ──────────────────────────────────

    fn make_user_record_with_content(content_json: &str) -> Value {
        serde_json::from_str(&format!(
            r#"{{"type":"user","message":{{"role":"user","content":{content_json}}},"timestamp":"2026-02-21T12:01:00.000Z","uuid":"u-x","sessionId":"s-1"}}"#
        ))
        .unwrap()
    }

    fn parts(msg: &Message) -> &[ContentPart] {
        match &msg.content {
            MessageContent::Parts(p) => p,
            _ => panic!("expected Parts variant"),
        }
    }

    #[test]
    fn test_tool_result_plain_string_content() {
        let record = make_user_record_with_content(
            r#"[{"type":"tool_result","tool_use_id":"toolu_abc","content":"simple output","is_error":false}]"#,
        );
        let msg = parse_message_from_record(&record, "user").unwrap();
        let p = parts(&msg);
        assert_eq!(p.len(), 1);
        match &p[0] {
            ContentPart::ToolResult {
                tool_use_id,
                content,
            } => {
                assert_eq!(tool_use_id, "toolu_abc");
                assert_eq!(content, "simple output");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_tool_result_persisted_content_preserved_verbatim() {
        let persisted = r#"<persisted-output>\nOutput too large (65KB). Full output saved to: /tmp/toolu_abc.txt\n\nPreview:\nhello\n</persisted-output>"#;
        let record = make_user_record_with_content(&format!(
            r#"[{{"type":"tool_result","tool_use_id":"toolu_abc","content":"{persisted}","is_error":false}}]"#
        ));
        let msg = parse_message_from_record(&record, "user").unwrap();
        match &parts(&msg)[0] {
            ContentPart::ToolResult { content, .. } => {
                assert!(content.contains("<persisted-output>"));
                assert!(content.contains("/tmp/toolu_abc.txt"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_tool_result_content_as_array_of_text_parts() {
        let record = make_user_record_with_content(
            r#"[{"type":"tool_result","tool_use_id":"toolu_arr","content":[{"type":"text","text":"line1"},{"type":"text","text":"line2"}],"is_error":false}]"#,
        );
        let msg = parse_message_from_record(&record, "user").unwrap();
        match &parts(&msg)[0] {
            ContentPart::ToolResult { content, .. } => {
                assert_eq!(content, "line1\nline2");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_tool_result_with_is_error_true_still_parses() {
        // is_error is not currently stored in ContentPart, but the record must parse.
        let record = make_user_record_with_content(
            r#"[{"type":"tool_result","tool_use_id":"toolu_err","content":"Approval required: rm -rf /","is_error":true}]"#,
        );
        let msg = parse_message_from_record(&record, "user").unwrap();
        match &parts(&msg)[0] {
            ContentPart::ToolResult {
                tool_use_id,
                content,
            } => {
                assert_eq!(tool_use_id, "toolu_err");
                assert!(content.contains("Approval required"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_tool_result_with_empty_string_content() {
        let record = make_user_record_with_content(
            r#"[{"type":"tool_result","tool_use_id":"toolu_empty","content":"","is_error":false}]"#,
        );
        let msg = parse_message_from_record(&record, "user").unwrap();
        match &parts(&msg)[0] {
            ContentPart::ToolResult { content, .. } => {
                assert_eq!(content, "");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_tool_result_with_null_content_falls_back_gracefully() {
        // content: null — should parse without panic, content becomes "null" (json fallback)
        let record = make_user_record_with_content(
            r#"[{"type":"tool_result","tool_use_id":"toolu_null","content":null,"is_error":false}]"#,
        );
        let msg = parse_message_from_record(&record, "user").unwrap();
        // Must not panic; result is implementation-defined for null
        assert_eq!(parts(&msg).len(), 1);
    }

    #[test]
    fn test_user_message_with_multiple_tool_results_some_persisted() {
        let record = make_user_record_with_content(
            r#"[
              {"type":"tool_result","tool_use_id":"toolu_a","content":"small output","is_error":false},
              {"type":"tool_result","tool_use_id":"toolu_b","content":"<persisted-output>\nOutput too large (100KB). Full output saved to: /tmp/toolu_b.txt\n</persisted-output>","is_error":false}
            ]"#,
        );
        let msg = parse_message_from_record(&record, "user").unwrap();
        let p = parts(&msg);
        assert_eq!(p.len(), 2);
        match &p[0] {
            ContentPart::ToolResult {
                tool_use_id,
                content,
            } => {
                assert_eq!(tool_use_id, "toolu_a");
                assert_eq!(content, "small output");
            }
            _ => panic!(),
        }
        match &p[1] {
            ContentPart::ToolResult {
                tool_use_id,
                content,
            } => {
                assert_eq!(tool_use_id, "toolu_b");
                assert!(content.contains("<persisted-output>"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn test_tool_result_persisted_path_with_spaces() {
        use crate::persisted::parse_persisted_output_ref;
        let content = "<persisted-output>\nOutput too large (10KB). Full output saved to: /home/my user/path with spaces/tool-results/toolu_abc.txt\n</persisted-output>";
        let p = parse_persisted_output_ref(content).unwrap();
        assert_eq!(
            p.path.to_str().unwrap(),
            "/home/my user/path with spaces/tool-results/toolu_abc.txt"
        );
    }

    #[test]
    fn test_tool_result_unknown_part_type_skipped() {
        // Unknown types should be silently ignored; known types after them still parsed.
        let record = make_user_record_with_content(
            r#"[{"type":"image","source":{"type":"base64","data":"abc"}},{"type":"tool_result","tool_use_id":"toolu_known","content":"ok","is_error":false}]"#,
        );
        let msg = parse_message_from_record(&record, "user").unwrap();
        // Only the known tool_result should survive; image is skipped
        assert_eq!(parts(&msg).len(), 1);
        match &parts(&msg)[0] {
            ContentPart::ToolResult { tool_use_id, .. } => assert_eq!(tool_use_id, "toolu_known"),
            _ => panic!(),
        }
    }
}
