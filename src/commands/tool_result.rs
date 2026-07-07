use anyhow::{Result, bail};

use crate::cli::ToolResultArgs;
use crate::config::Config;
use crate::output;
use crate::parsers::{
    Parser, claude::ClaudeParser, codex::CodexParser, copilot::CopilotParser, gemini::GeminiParser,
};
use crate::persisted::{extract_embedded_preview, parse_persisted_output_ref, read_preview};
use crate::session::{ContentPart, Session, SessionMeta, Tool};

use super::info::discover_all;

pub fn run(args: &ToolResultArgs) -> Result<()> {
    let config = Config::load();
    let all_sessions = discover_all(&config)?;
    let width = output::terminal_width();

    let mut hits: Vec<(SessionMeta, usize, String)> = Vec::new();
    for meta in all_sessions {
        if !matches!(meta.tool, Tool::Claude | Tool::Copilot) {
            continue;
        }
        let session = match load_session(&meta, &config) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for msg in session.messages {
            if let crate::session::MessageContent::Parts(parts) = msg.content {
                for part in parts {
                    if let ContentPart::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } = part
                        && tool_use_id == args.tool_use_id
                    {
                        hits.push((meta.clone(), msg.index, content));
                    }
                }
            }
        }
    }

    if hits.is_empty() {
        bail!("No tool result found for tool_use_id {}", args.tool_use_id);
    }
    if hits.len() > 1 {
        output::print_warn(&format!(
            "Multiple tool results matched {} ({}); showing all.",
            args.tool_use_id,
            hits.len()
        ));
    }

    for (idx, (meta, msg_index, content)) in hits.iter().enumerate() {
        if idx > 0 {
            output::print_blank_line();
        }
        output::print_tool_result_header(meta, *msg_index, &args.tool_use_id);
        if let Some(persisted) = parse_persisted_output_ref(content) {
            output::print_line(&format!("persisted_path: {}", persisted.path.display()));
            if let Some(size) = persisted.declared_size {
                output::print_line(&format!("persisted_size: {}", size));
            }
            match read_preview(&persisted.path, args.max_bytes) {
                Ok(preview) => {
                    output::print_line(&format!(
                        "resolved_bytes: {} of {}",
                        preview.bytes_read, preview.total_size
                    ));
                    for line in preview.text.lines() {
                        output::print_line(&format!("  {}", line));
                    }
                    if preview.truncated {
                        output::print_line(&format!("[truncated at {} bytes]", args.max_bytes));
                    }
                }
                Err(err) => {
                    output::print_warn(&format!("Cannot read persisted output: {err}"));
                    if let Some(preview) = extract_embedded_preview(content) {
                        output::print_line("[embedded preview]");
                        for line in preview.lines() {
                            output::print_line(&format!("  {}", line));
                        }
                    } else {
                        let preview = content.lines().next().unwrap_or("").trim();
                        output::print_line(&output::truncate(preview, width));
                    }
                }
            }
        } else {
            let text = if content.len() > args.max_bytes {
                let mut byte_end = args.max_bytes;
                while byte_end > 0 && !content.is_char_boundary(byte_end) {
                    byte_end -= 1;
                }
                &content[..byte_end]
            } else {
                content
            };
            for line in text.lines() {
                output::print_line(&format!("  {}", line));
            }
            if content.len() > args.max_bytes {
                output::print_line(&format!("[truncated at {} bytes]", args.max_bytes));
            }
        }
    }

    Ok(())
}

fn load_session(meta: &SessionMeta, config: &Config) -> Result<Session> {
    match meta.tool {
        Tool::Claude => ClaudeParser::new(config).load(meta),
        Tool::Codex => CodexParser::new(config).load(meta),
        Tool::Gemini => GeminiParser::new(config).load(meta),
        Tool::Copilot => CopilotParser::new(config).load(meta),
    }
}
