use anyhow::Result;

use crate::cli::{RoleFilter, ShowArgs};
use crate::config::Config;
use crate::output;
use crate::parsers::{
    Parser, claude::ClaudeParser, codex::CodexParser, copilot::CopilotParser, gemini::GeminiParser,
};
use crate::session::{Role, Session};

use super::info::{discover_all, resolve_session_meta};

pub fn run(args: &ShowArgs) -> Result<()> {
    let config = Config::load();
    let all_sessions = discover_all(&config)?;
    let meta = resolve_session_meta(&args.session, &all_sessions)?;

    // Load the full session
    let session = load_session(meta, &config)?;

    let width = args.width.unwrap_or_else(output::terminal_width);

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

    output::print_show_header(meta, start, end.saturating_sub(1), total);
    println!();

    for (msg, content) in &messages[start..end] {
        let display_msg = crate::session::Message {
            index: msg.index,
            role: msg.role.clone(),
            content: content.clone(),
            timestamp: msg.timestamp,
        };
        output::print_message(&display_msg, width);
    }

    Ok(())
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
