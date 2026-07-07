use anyhow::Result;

use crate::cli::{RoleFilter, ShowArgs, TextFormat};
use crate::config::Config;
use crate::output;
use crate::output::MessageRenderOptions;
use crate::parsers::{
    Parser, claude::ClaudeParser, codex::CodexParser, copilot::CopilotParser, gemini::GeminiParser,
};
use crate::persisted::parse_persisted_output_ref;
use crate::response::{BlockJson, ShowJson, TurnJson};
use crate::session::{ContentPart, Message, MessageContent, Role, Session};
use crate::util;

use super::info::{discover_all, resolve_session_meta};

pub fn run(args: &ShowArgs) -> Result<()> {
    let config = Config::load();
    let all_sessions = discover_all(&config)?;
    let meta = resolve_session_meta(&args.session, &all_sessions)?;

    // Load the full session
    let session = load_session(meta, &config)?;

    let width = args.width.unwrap_or_else(output::terminal_width);
    let render_opts = MessageRenderOptions {
        expand_persisted: args.expand_persisted,
        max_bytes: args.max_bytes,
        expand_tool_calls: args.expand_tool_calls,
    };

    // Apply role filter
    let messages: Vec<_> = session
        .messages
        .iter()
        .filter(|m| match args.role {
            RoleFilter::User => m.role == Role::User,
            RoleFilter::Assistant => m.role == Role::Assistant,
            RoleFilter::All => true,
        })
        .collect();

    // Apply --no-tool-calls: filter messages and strip tool call parts
    let messages: Vec<_> = if args.no_tool_calls {
        messages
            .into_iter()
            .filter_map(|m| {
                let content = m.content.without_tool_calls();
                if content.is_empty() {
                    None
                } else {
                    Some((m, content))
                }
            })
            .collect()
    } else {
        messages
            .into_iter()
            .map(|m| (m, m.content.clone()))
            .collect()
    };

    // Determine slice
    let total = messages.len();
    let (start, end) = compute_slice(args, total);
    // Guard against an inverted range (e.g. --from 5 --to 2), which would panic
    // the slice below; an empty window is the sensible result.
    let end = end.max(start);

    if args.format == TextFormat::Json {
        let doc = ShowJson {
            session_id: meta.id.clone(),
            tool: meta.tool.to_string(),
            project: meta
                .project_path
                .as_deref()
                .map(|p| p.to_string_lossy().into_owned()),
            git_branch: meta.git_branch.clone(),
            message_count: meta.message_count,
            from: start,
            to: end.saturating_sub(1),
            total,
            messages: messages[start..end]
                .iter()
                .map(|(msg, content)| turn_json(msg, content))
                .collect(),
        };
        return output::print_json(&doc);
    }

    output::print_show_header(meta, start, end.saturating_sub(1), total);
    println!();

    for (msg, content) in &messages[start..end] {
        let display_msg = crate::session::Message {
            index: msg.index,
            role: msg.role.clone(),
            content: content.clone(),
            timestamp: msg.timestamp,
        };
        output::print_message(&display_msg, width, render_opts);
    }

    Ok(())
}

/// Convert a message into its JSON turn representation, tagging each block and
/// surfacing persisted-output metadata for tool results.
fn turn_json(msg: &Message, content: &MessageContent) -> TurnJson {
    let blocks = match content {
        MessageContent::Text(s) => vec![BlockJson::Text { text: s.clone() }],
        MessageContent::Parts(parts) => parts.iter().map(block_json).collect(),
    };
    TurnJson {
        index: msg.index,
        role: msg.role.to_string(),
        timestamp: msg.timestamp.as_ref().map(output::format_datetime),
        blocks,
    }
}

fn block_json(part: &ContentPart) -> BlockJson {
    match part {
        ContentPart::Text(t) => BlockJson::Text { text: t.clone() },
        ContentPart::Thinking(t) => BlockJson::Thinking { text: t.clone() },
        ContentPart::ToolCall { id, name, input } => BlockJson::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
            summary: util::summarize_tool_input(name, input),
        },
        ContentPart::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let persisted = parse_persisted_output_ref(content);
            BlockJson::ToolResult {
                tool_use_id: tool_use_id.clone(),
                is_error: *is_error,
                persisted_path: persisted.as_ref().map(|p| p.path.display().to_string()),
                persisted_size: persisted.as_ref().and_then(|p| p.declared_size.clone()),
                content: content.clone(),
            }
        }
    }
}

/// Returns (start_inclusive, end_exclusive) indices into the filtered message list.
fn compute_slice(args: &ShowArgs, total: usize) -> (usize, usize) {
    if let (Some(from), Some(to)) = (args.from, args.to) {
        let start = from.min(total);
        let end = (to + 1).min(total);
        return (start, end);
    }

    if let Some(from) = args.from {
        return (from.min(total), total);
    }

    if let Some(to) = args.to {
        return (0, (to + 1).min(total));
    }

    if let Some(last) = args.last {
        let start = total.saturating_sub(last);
        return (start, total);
    }

    if let Some(first) = args.first {
        return (0, first.min(total));
    }

    // Default: show all
    (0, total)
}

fn load_session(meta: &crate::session::SessionMeta, config: &Config) -> Result<Session> {
    use crate::session::Tool;
    match meta.tool {
        Tool::Claude => ClaudeParser::new(config).load(meta),
        Tool::Codex => CodexParser::new(config).load(meta),
        Tool::Gemini => GeminiParser::new(config).load(meta),
        Tool::Copilot => CopilotParser::new(config).load(meta),
    }
}
