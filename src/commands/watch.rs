use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::thread;
use std::time::Duration;

use anyhow::{Result, bail};
use serde_json::Value;

use crate::cli::{InboxArgs, InboxFormat, RoleFilter, WatchArgs};
use crate::collab::{project_hash, state_path, stream_path};
use crate::commands::inbox::{
    StreamReset, file_inode, file_len, load_cursor, read_stream, reset_reason, resolve_project,
    save_cursor,
};
use crate::commands::info::{discover_all, resolve_session_meta};
use crate::config::Config;
use crate::output;
use crate::parsers::claude::parse_message_from_record;
use crate::parsers::codex::{parse_flat_record_pub, parse_nested_record_pub};
use crate::session::{Message, Role, SessionMeta, Tool};

pub fn run(args: &WatchArgs) -> Result<()> {
    let config = Config::load();
    let all_sessions = discover_all(&config)?;
    let meta = resolve_session_meta(&args.session, &all_sessions)?;

    if meta.tool == Tool::Gemini {
        bail!("watch is not supported for Gemini sessions");
    }

    match &args.inbox_for {
        None => run_session_only(meta, args),
        Some(inbox_for) => run_watch_mode(meta, args, inbox_for, &config),
    }
}

fn run_session_only(meta: &SessionMeta, args: &WatchArgs) -> Result<()> {
    let width = args.width.unwrap_or_else(output::terminal_width);
    let poll_ms = args.poll_ms.unwrap_or(750).max(50);

    let mut offset: u64 = if args.from_start {
        0
    } else {
        file_len(&meta.file_path).unwrap_or(0)
    };
    let mut sleep_ms = poll_ms;
    output::print_show_header(meta, 0, 0, meta.message_count);
    eprintln!("(following — Ctrl-C to stop)");

    loop {
        let current_len = file_len(&meta.file_path).unwrap_or(0);
        if current_len < offset {
            offset = 0;
        }
        if current_len <= offset {
            sleep_ms = (sleep_ms + 250).min(2000);
            thread::sleep(Duration::from_millis(sleep_ms));
            continue;
        }

        let file = File::open(&meta.file_path)?;
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(offset))?;

        let mut had_output = false;
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = reader.read_line(&mut line)?;
            if bytes == 0 {
                break;
            }
            offset += bytes as u64;

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let Ok(record) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };

            let Some(msg) = parse_session_message(&meta.tool, &record) else {
                continue;
            };
            if !passes_role_filter(&msg, &args.role) {
                continue;
            }

            let content = if args.no_tool_calls {
                msg.content.without_tool_calls()
            } else {
                msg.content.clone()
            };
            if content.is_empty() {
                continue;
            }

            let display = Message { content, ..msg };
            output::print_message(&display, width, output::MessageRenderOptions::default());
            had_output = true;
        }

        if had_output {
            sleep_ms = (poll_ms / 3).max(250);
        } else {
            sleep_ms = (sleep_ms + 250).min(2000);
        }
        thread::sleep(Duration::from_millis(sleep_ms));
    }
}

fn run_watch_mode(
    meta: &SessionMeta,
    args: &WatchArgs,
    inbox_for: &str,
    config: &Config,
) -> Result<()> {
    let poll_ms = args.poll_ms.unwrap_or(750).max(50);
    let mut session_offset = file_len(&meta.file_path).unwrap_or(0);
    let mut session_index: usize = 0;
    let mut sleep_ms = poll_ms;
    eprintln!(
        "(watching session {} + inbox for {} — Ctrl-C to stop)",
        &meta.id[..meta.id.len().min(12)],
        inbox_for
    );

    let inbox_args = InboxArgs {
        identity: inbox_for.to_string(),
        project: args.project.clone(),
        unread: true,
        mark_read: args.mark_read,
        follow: false,
        sender: None,
        kind: None,
        session: None,
        since: None,
        last: None,
        format: InboxFormat::Table,
        wait: false,
        timeout: 0,
    };

    let project = resolve_project(&inbox_args)?;
    let project_hash = project_hash(&project);
    let stream = stream_path(&config.powers_dir, &project_hash);
    let state_file = state_path(&config.powers_dir, &inbox_args.identity, &project_hash);
    let mut cursor = load_cursor(&state_file)?;

    loop {
        let mut had_new_data = false;

        if poll_session(
            &meta.tool,
            &meta.file_path,
            &mut session_offset,
            &mut session_index,
            args,
        )? {
            had_new_data = true;
        }

        let mut after_last = false;
        let reset = reset_reason(&stream, &cursor);
        if !matches!(reset, StreamReset::None) {
            cursor.offset = 0;
            cursor.stream_size_seen = 0;
            cursor.stream_inode = file_inode(&stream);
            after_last = matches!(reset, StreamReset::Truncated) && cursor.last_msg_id.is_some();
        }

        let start_offset = cursor.offset;
        let (messages, new_offset, last_msg_id, inbox_had_new_data) = read_stream(
            &stream,
            start_offset,
            &inbox_args,
            &cursor.last_msg_id,
            after_last,
        )?;

        for msg in &messages {
            output::print_inbox_message_prefixed(msg, Some("inbox"));
        }

        cursor.offset = new_offset;
        cursor.stream_size_seen = new_offset;
        cursor.stream_inode = file_inode(&stream);
        if last_msg_id.is_some() {
            cursor.last_msg_id = last_msg_id;
        }
        if args.mark_read {
            save_cursor(&state_file, &cursor)?;
        }

        if inbox_had_new_data {
            had_new_data = true;
        }

        if had_new_data {
            sleep_ms = (poll_ms / 3).max(250);
        } else {
            // Keep a low-CPU idle cadence while preserving responsiveness after activity.
            sleep_ms = (sleep_ms + 250).min(2000);
        }
        thread::sleep(Duration::from_millis(sleep_ms));
    }
}

fn poll_session(
    tool: &Tool,
    path: &std::path::Path,
    offset: &mut u64,
    display_index: &mut usize,
    args: &WatchArgs,
) -> Result<bool> {
    let current_len = file_len(path).unwrap_or(0);
    if current_len < *offset {
        *offset = 0;
    }
    if current_len <= *offset {
        return Ok(false);
    }

    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(*offset))?;

    let mut line = String::new();
    let mut had_new_data = false;
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            break;
        }
        had_new_data = true;
        *offset += bytes as u64;

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };

        let Some(msg) = parse_session_message(tool, &record) else {
            continue;
        };
        if !passes_role_filter(&msg, &args.role) {
            continue;
        }

        let content = if args.no_tool_calls {
            msg.content.without_tool_calls()
        } else {
            msg.content.clone()
        };
        if content.is_empty() {
            continue;
        }

        let display = Message { content, ..msg };
        print_session_message_prefixed(display_index, &display);
        *display_index += 1;
    }

    Ok(had_new_data)
}

fn parse_session_message(tool: &Tool, record: &Value) -> Option<Message> {
    match tool {
        Tool::Claude => {
            let t = record["type"].as_str().unwrap_or("");
            parse_message_from_record(record, t)
        }
        Tool::Codex => {
            let (role, content, ts) = if record["type"].as_str() == Some("response_item") {
                parse_nested_record_pub(record)
            } else {
                parse_flat_record_pub(record)
            };
            match (role, content) {
                (Some(role), Some(content)) => Some(Message {
                    index: 0,
                    role,
                    content,
                    timestamp: ts,
                }),
                _ => None,
            }
        }
        Tool::Copilot => {
            use crate::parsers::copilot::parse_copilot_event_pub;
            parse_copilot_event_pub(record)
        }
        Tool::Gemini => None,
    }
}

fn passes_role_filter(msg: &Message, role: &RoleFilter) -> bool {
    match role {
        RoleFilter::User => msg.role == Role::User,
        RoleFilter::Assistant => msg.role == Role::Assistant,
        RoleFilter::All => true,
    }
}

fn print_session_message_prefixed(display_index: &usize, msg: &Message) {
    let ts = msg
        .timestamp
        .as_ref()
        .map(output::format_datetime)
        .unwrap_or_default();
    println!("[session] [{}] {} {}", display_index, msg.role, ts);
    for line in msg.content.extract_text().lines() {
        println!("[session]   {}", line);
    }
    println!("[session]");
}
