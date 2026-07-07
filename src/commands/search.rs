use anyhow::{Context, Result};
use regex::Regex;
use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use crate::cli::{SearchArgs, ToolFilter};
use crate::config::Config;
use crate::output;
use crate::parsers::{
    Parser, claude::ClaudeParser, codex::CodexParser, copilot::CopilotParser, gemini::GeminiParser,
};
use crate::session::{Message, SessionMeta, Tool};
use crate::util;

pub fn run(args: &SearchArgs) -> Result<()> {
    // Literal substring by default; opt into regex with --regex.
    let raw = if args.regex {
        args.pattern.clone()
    } else {
        regex::escape(&args.pattern)
    };
    let pattern = if args.case_sensitive {
        Regex::new(&raw)
    } else {
        Regex::new(&format!("(?i){raw}"))
    }
    .with_context(|| format!("Invalid regex: {}", args.pattern))?;

    let config = Config::load();

    // Phase 1: discover and filter by tool/project/time using only SessionMeta
    let all_sessions = discover_filtered(&config, args)?;

    let mut total_matches = 0usize;
    // uuids that have already produced a match, so the same message reappearing in
    // a resumed/forked session file is not reported twice. Sessions are sorted
    // newest-first, so the surviving copy is the most recent one.
    let mut seen_uuids: HashSet<String> = HashSet::new();

    for meta in &all_sessions {
        if total_matches >= args.max_matches {
            break;
        }
        let matched = search_session(meta, &pattern, args, &mut total_matches, &mut seen_uuids)?;
        if args.session_only && matched {
            println!("{}", meta.id);
        }
    }

    Ok(())
}

fn discover_filtered(config: &Config, args: &SearchArgs) -> Result<Vec<SessionMeta>> {
    let mut sessions = Vec::new();

    let include_claude = args
        .tool
        .as_ref()
        .map(|t| t == &ToolFilter::Claude)
        .unwrap_or(true);
    let include_codex = args
        .tool
        .as_ref()
        .map(|t| t == &ToolFilter::Codex)
        .unwrap_or(true);
    let include_gemini = args
        .tool
        .as_ref()
        .map(|t| t == &ToolFilter::Gemini)
        .unwrap_or(true);
    let include_copilot = args
        .tool
        .as_ref()
        .map(|t| t == &ToolFilter::Copilot)
        .unwrap_or(true);

    if include_claude {
        match ClaudeParser::new(config).discover() {
            Ok(mut s) => sessions.append(&mut s),
            Err(e) => output::print_warn(&format!("Claude discovery failed: {e}")),
        }
    }
    if include_codex {
        match CodexParser::new(config).discover() {
            Ok(mut s) => sessions.append(&mut s),
            Err(e) => output::print_warn(&format!("Codex discovery failed: {e}")),
        }
    }
    if include_gemini {
        match GeminiParser::new(config).discover() {
            Ok(mut s) => sessions.append(&mut s),
            Err(e) => output::print_warn(&format!("Gemini discovery failed: {e}")),
        }
    }
    if include_copilot {
        match CopilotParser::new(config).discover() {
            Ok(mut s) => sessions.append(&mut s),
            Err(e) => output::print_warn(&format!("Copilot discovery failed: {e}")),
        }
    }

    let since = args
        .since
        .as_deref()
        .map(util::parse_time_bound)
        .transpose()?;
    let until = args
        .until
        .as_deref()
        .map(util::parse_time_bound_until)
        .transpose()?;
    sessions.retain(|s| {
        if let Some(project) = &args.project
            && !project_matches(&s.project_path, project)
        {
            return false;
        }
        if let Some(exclude) = &args.exclude_session
            && s.matches_prefix(exclude)
        {
            return false;
        }
        if let Some(since_dt) = since
            && s.last_activity < since_dt
        {
            return false;
        }
        if let Some(until_dt) = until
            && s.last_activity > until_dt
        {
            return false;
        }
        true
    });

    sessions.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    Ok(sessions)
}

fn search_session(
    meta: &SessionMeta,
    pattern: &Regex,
    args: &SearchArgs,
    total_matches: &mut usize,
    seen_uuids: &mut HashSet<String>,
) -> Result<bool> {
    match meta.tool {
        // Gemini has no per-message uuids, so cross-file dedup does not apply.
        Tool::Gemini => search_gemini(meta, pattern, args, total_matches),
        _ => search_jsonl(meta, pattern, args, total_matches, seen_uuids),
    }
}
///
/// Uses a ring buffer of `context` messages for pre-context.
/// After each match, collects up to `context` post-context messages before
/// printing the group.  Never holds more than `2*context + 1` messages in memory.
fn search_jsonl(
    meta: &SessionMeta,
    pattern: &Regex,
    args: &SearchArgs,
    total_matches: &mut usize,
    seen_uuids: &mut HashSet<String>,
) -> Result<bool> {
    let file = match std::fs::File::open(&meta.file_path) {
        Ok(f) => f,
        Err(_) => return Ok(false),
    };

    let ctx_n = args.context;
    // pre_ctx: ring buffer of the last `ctx_n` messages before the current line
    let mut pre_ctx: VecDeque<Message> = VecDeque::with_capacity(ctx_n + 1);
    // post_ctx_remaining: how many more messages to emit as post-context
    let mut post_ctx_remaining: usize = 0;
    let mut found_in_session = false;
    // Track the position of each parsed user/assistant message so indices are meaningful
    let mut msg_index: usize = 0;

    let mut lines = BufReader::new(file).lines();
    lines.next(); // skip header line (session metadata)

    for line in lines {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Parse only lines that look like user/assistant messages
        let Some((mut msg, uuid)) = parse_jsonl_message(line, &meta.tool) else {
            continue;
        };
        msg.index = msg_index;
        msg_index += 1;

        // Reminder blocks are injected context, not something the user or assistant
        // wrote — strip them so guidance/skill text does not create false matches.
        let raw = msg.content.extract_searchable_text(args.include_tool_calls);
        let text = util::strip_reminders(&raw);
        let mut is_match = pattern.is_match(&text);

        // Suppress a match already reported from a newer session file (resume/fork),
        // recording the uuid the first (newest) time it matches.
        if is_match
            && let Some(u) = uuid
            && !seen_uuids.insert(u)
        {
            is_match = false;
        }

        if post_ctx_remaining > 0 {
            if !args.session_only {
                output::print_search_match(&msg);
            }
            post_ctx_remaining -= 1;

            if is_match {
                // New match inside post-context: extend post-context window
                post_ctx_remaining = ctx_n;
                *total_matches += 1;
                if *total_matches >= args.max_matches {
                    break;
                }
            } else if post_ctx_remaining == 0 && !args.session_only {
                println!();
            }
        } else if is_match {
            if *total_matches >= args.max_matches {
                break;
            }

            if !found_in_session {
                if !args.session_only {
                    output::print_search_header(meta);
                }
                found_in_session = true;
            }

            if args.session_only {
                return Ok(true);
            }

            // Print pre-context
            for ctx_msg in &pre_ctx {
                output::print_search_match(ctx_msg);
            }
            pre_ctx.clear();

            output::print_search_match(&msg);
            *total_matches += 1;

            if ctx_n == 0 {
                println!();
            } else {
                post_ctx_remaining = ctx_n;
            }
        } else {
            // Not a match, not in post-context: add to pre-context ring buffer
            if pre_ctx.len() >= ctx_n.max(1) {
                pre_ctx.pop_front();
            }
            if ctx_n > 0 {
                pre_ctx.push_back(msg);
            }
        }
    }

    Ok(found_in_session)
}

/// Parse a single JSONL line into a `(Message, uuid)` pair, or None if the line
/// is not a relevant user/assistant message. The uuid is present only for Claude
/// (used for cross-file dedup); other tools return None.
///
/// A cheap substring pre-filter avoids JSON parsing for lines that cannot be
/// messages.
fn parse_jsonl_message(line: &str, tool: &Tool) -> Option<(Message, Option<String>)> {
    match tool {
        Tool::Claude => parse_claude_line(line),
        Tool::Codex => parse_codex_line(line).map(|m| (m, None)),
        Tool::Copilot => parse_copilot_line(line).map(|m| (m, None)),
        _ => None,
    }
}

fn parse_claude_line(line: &str) -> Option<(Message, Option<String>)> {
    // Pre-filter: must contain "type":"user" or "type":"assistant"
    let is_user = line.contains("\"type\":\"user\"");
    let is_assistant = line.contains("\"type\":\"assistant\"");
    if !is_user && !is_assistant {
        return None;
    }

    let record: serde_json::Value = serde_json::from_str(line).ok()?;
    let record_type = record["type"].as_str()?;

    // Injected meta rows (command echoes, injected context) are not real turns.
    if record["isMeta"].as_bool() == Some(true) {
        return None;
    }

    let uuid = record["uuid"].as_str().map(|s| s.to_string());
    use crate::parsers::claude::parse_message_from_record;
    parse_message_from_record(&record, record_type).map(|m| (m, uuid))
}

fn parse_codex_line(line: &str) -> Option<Message> {
    // Pre-filter: must contain a role
    let has_user = line.contains("\"role\":\"user\"");
    let has_assistant = line.contains("\"role\":\"assistant\"");
    if !has_user && !has_assistant {
        return None;
    }

    let record: serde_json::Value = serde_json::from_str(line).ok()?;

    use crate::parsers::codex::{parse_flat_record_pub, parse_nested_record_pub};

    // Try nested format first
    if record["type"].as_str() == Some("response_item") {
        let (role, content, ts) = parse_nested_record_pub(&record);
        let role = role?;
        let content = content?;
        return Some(Message {
            index: 0,
            role,
            content,
            timestamp: ts,
        });
    }

    // Try flat format
    if record["type"].as_str() == Some("message") {
        let (role, content, ts) = parse_flat_record_pub(&record);
        let role = role?;
        let content = content?;
        return Some(Message {
            index: 0,
            role,
            content,
            timestamp: ts,
        });
    }

    None
}

fn parse_copilot_line(line: &str) -> Option<Message> {
    // Pre-filter: must be a user or assistant message event
    let is_user = line.contains("\"type\":\"user.message\"");
    let is_assistant = line.contains("\"type\":\"assistant.message\"");
    if !is_user && !is_assistant {
        return None;
    }

    let record: serde_json::Value = serde_json::from_str(line).ok()?;
    use crate::parsers::copilot::parse_copilot_event_pub;
    parse_copilot_event_pub(&record)
}

/// For Gemini (full JSON, not JSONL): load all messages but still use the ring
/// buffer approach so we never hold more than 2*context+1 in an active window.
fn search_gemini(
    meta: &SessionMeta,
    pattern: &Regex,
    args: &SearchArgs,
    total_matches: &mut usize,
) -> Result<bool> {
    let config = Config::load();
    let session = GeminiParser::new(&config).load(meta)?;

    let ctx_n = args.context;
    let mut pre_ctx: VecDeque<&Message> = VecDeque::with_capacity(ctx_n + 1);
    let mut post_ctx_remaining: usize = 0;
    let mut found_in_session = false;

    for msg in &session.messages {
        let raw = msg.content.extract_searchable_text(args.include_tool_calls);
        let text = util::strip_reminders(&raw);
        let is_match = pattern.is_match(&text);

        if post_ctx_remaining > 0 {
            if !args.session_only {
                output::print_search_match(msg);
            }
            post_ctx_remaining -= 1;

            if is_match {
                post_ctx_remaining = ctx_n;
                *total_matches += 1;
                if *total_matches >= args.max_matches {
                    break;
                }
            } else if post_ctx_remaining == 0 && !args.session_only {
                println!();
            }
        } else if is_match {
            if *total_matches >= args.max_matches {
                break;
            }

            if !found_in_session {
                if !args.session_only {
                    output::print_search_header(meta);
                }
                found_in_session = true;
            }

            if args.session_only {
                return Ok(true);
            }

            for ctx_msg in &pre_ctx {
                output::print_search_match(ctx_msg);
            }
            pre_ctx.clear();

            output::print_search_match(msg);
            *total_matches += 1;

            if ctx_n == 0 {
                println!();
            } else {
                post_ctx_remaining = ctx_n;
            }
        } else {
            if pre_ctx.len() >= ctx_n.max(1) {
                pre_ctx.pop_front();
            }
            if ctx_n > 0 {
                pre_ctx.push_back(msg);
            }
        }
    }

    Ok(found_in_session)
}

fn project_matches(project_path: &Option<PathBuf>, filter: &PathBuf) -> bool {
    match project_path {
        None => false,
        Some(p) => p.starts_with(filter),
    }
}
