//! Serde structs for `--format json` output across list / search / show.
//!
//! These types are the machine-readable contract; keep field names stable.

use serde::Serialize;

fn is_false(b: &bool) -> bool {
    !b
}

// ── list ──────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ListJson {
    pub sessions: Vec<SessionJson>,
}

#[derive(Serialize)]
pub struct SessionJson {
    pub session_id: String,
    pub tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    pub last_activity: String,
    pub message_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_prompt: Option<String>,
}

// ── search ────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct SearchJson {
    pub meta: SearchMetaJson,
    pub results: Vec<SessionHitsJson>,
}

#[derive(Serialize)]
pub struct SearchMetaJson {
    pub query: String,
    pub regex: bool,
    pub kinds: Vec<String>,
    pub total_matches: usize,
    pub sessions_matched: usize,
}

#[derive(Serialize)]
pub struct SessionHitsJson {
    pub session_id: String,
    pub tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub last_activity: String,
    pub hits: Vec<HitJson>,
}

#[derive(Serialize)]
pub struct HitJson {
    pub index: usize,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    pub matched_in: String,
    /// The initiating user prompt this hit falls under (most recent user prose
    /// at or before it), truncated. Helps judge relevance without a follow-up read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub is_error: bool,
    pub snippet: String,
    pub match_count: usize,
    pub full_length: usize,
}

// ── show ──────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ShowJson {
    pub session_id: String,
    pub tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    pub message_count: usize,
    pub from: usize,
    pub to: usize,
    pub total: usize,
    pub messages: Vec<TurnJson>,
}

#[derive(Serialize)]
pub struct TurnJson {
    pub index: usize,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    pub blocks: Vec<BlockJson>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockJson {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolUse {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
        input: String,
        summary: String,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(skip_serializing_if = "is_false")]
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        persisted_path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        persisted_size: Option<String>,
        content: String,
    },
}
