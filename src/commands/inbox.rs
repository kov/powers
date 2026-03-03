use anyhow::Result;
use std::fs::{File, metadata};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::thread;
use std::time::{Duration, Instant};

use crate::cli::{InboxArgs, InboxFormat};
use crate::collab::{
    CursorState, StreamMessage, ensure_parent_dir, project_hash, state_path, stream_inode,
    stream_path,
};
use crate::config::Config;
use crate::output;

pub(crate) enum StreamReset {
    None,
    InodeChanged,
    Truncated,
}

pub fn run(args: &InboxArgs) -> Result<()> {
    let config = Config::load();
    let project = resolve_project(args)?;
    let project_hash = project_hash(&project);
    let stream = stream_path(&config.powers_dir, &project_hash);
    let state_file = state_path(&config.powers_dir, &args.identity, &project_hash);

    let mut cursor = load_cursor(&state_file)?;
    if args.follow && !args.unread {
        cursor.offset = file_len(&stream).unwrap_or(0);
        cursor.stream_size_seen = cursor.offset;
        cursor.stream_inode = file_inode(&stream);
    }

    if args.follow {
        follow_loop(args, &stream, &state_file, &mut cursor)?;
        return Ok(());
    }

    let reset = reset_reason(&stream, &cursor);
    let mut after_last = false;
    if !matches!(reset, StreamReset::None) {
        cursor.offset = 0;
        cursor.stream_size_seen = 0;
        cursor.stream_inode = file_inode(&stream);
        // Only dedupe on truncation of the same file; inode replacement means
        // this is a new stream and old IDs are irrelevant.
        after_last = matches!(reset, StreamReset::Truncated) && cursor.last_msg_id.is_some();
    }

    let start_offset = if args.unread { cursor.offset } else { 0 };
    let (messages, new_offset, last_msg_id, _) =
        read_stream(&stream, start_offset, args, &cursor.last_msg_id, after_last)?;

    if args.wait && messages.is_empty() {
        cursor.offset = new_offset;
        cursor.stream_size_seen = new_offset;
        cursor.stream_inode = file_inode(&stream);
        if last_msg_id.is_some() {
            cursor.last_msg_id = last_msg_id;
        }
        wait_for_message(args, &stream, &state_file, &mut cursor)?;
        return Ok(());
    }

    print_messages(&messages, args);

    if args.mark_read {
        cursor.offset = new_offset;
        cursor.stream_size_seen = new_offset;
        cursor.stream_inode = file_inode(&stream);
        if last_msg_id.is_some() {
            cursor.last_msg_id = last_msg_id;
        }
        save_cursor(&state_file, &cursor)?;
    }

    Ok(())
}

fn wait_for_message(
    args: &InboxArgs,
    stream: &std::path::Path,
    state_file: &std::path::Path,
    cursor: &mut CursorState,
) -> Result<()> {
    let deadline = if args.timeout > 0 {
        Some(Instant::now() + Duration::from_secs(args.timeout))
    } else {
        None
    };

    loop {
        thread::sleep(Duration::from_millis(500));

        if let Some(d) = deadline
            && Instant::now() >= d
        {
            return Ok(()); // timeout — exit 0, no output
        }

        let reset = reset_reason(stream, cursor);
        let mut after_last = false;
        if !matches!(reset, StreamReset::None) {
            cursor.offset = 0;
            cursor.stream_size_seen = 0;
            cursor.stream_inode = file_inode(stream);
            after_last = matches!(reset, StreamReset::Truncated) && cursor.last_msg_id.is_some();
        }

        let (messages, new_offset, last_msg_id, _) =
            read_stream(stream, cursor.offset, args, &cursor.last_msg_id, after_last)?;

        if !messages.is_empty() {
            print_messages(&messages, args);
            cursor.offset = new_offset;
            cursor.stream_size_seen = new_offset;
            cursor.stream_inode = file_inode(stream);
            if last_msg_id.is_some() {
                cursor.last_msg_id = last_msg_id;
            }
            if args.mark_read {
                save_cursor(state_file, cursor)?;
            }
            return Ok(());
        }

        cursor.offset = new_offset;
        cursor.stream_size_seen = new_offset;
        cursor.stream_inode = file_inode(stream);
        if last_msg_id.is_some() {
            cursor.last_msg_id = last_msg_id;
        }
    }
}

fn follow_loop(
    args: &InboxArgs,
    stream: &std::path::Path,
    state_file: &std::path::Path,
    cursor: &mut CursorState,
) -> Result<()> {
    let mut sleep_ms = 750u64;

    loop {
        let reset = reset_reason(stream, cursor);
        let mut after_last = false;
        if !matches!(reset, StreamReset::None) {
            cursor.offset = 0;
            cursor.stream_size_seen = 0;
            cursor.stream_inode = file_inode(stream);
            // Only dedupe on truncation of the same file; inode replacement means
            // this is a new stream and old IDs are irrelevant.
            after_last = matches!(reset, StreamReset::Truncated) && cursor.last_msg_id.is_some();
        }

        let start = cursor.offset;
        let (messages, new_offset, last_msg_id, had_new_data) =
            read_stream(stream, start, args, &cursor.last_msg_id, after_last)?;

        print_messages(&messages, args);

        cursor.offset = new_offset;
        cursor.stream_size_seen = new_offset;
        cursor.stream_inode = file_inode(stream);
        if last_msg_id.is_some() {
            cursor.last_msg_id = last_msg_id;
        }

        if args.mark_read {
            save_cursor(state_file, cursor)?;
        }

        if had_new_data {
            sleep_ms = 250;
        } else {
            sleep_ms = (sleep_ms + 250).min(2000);
        }
        thread::sleep(Duration::from_millis(sleep_ms));
    }
}

fn print_messages(messages: &[StreamMessage], args: &InboxArgs) {
    for msg in messages {
        match args.format {
            InboxFormat::Table => output::print_inbox_message(msg),
            InboxFormat::Json => output::print_inbox_message_json(msg),
        }
    }
}

pub(crate) fn resolve_project(args: &InboxArgs) -> Result<std::path::PathBuf> {
    match &args.project {
        Some(path) => Ok(path.clone()),
        None => Ok(std::env::current_dir()?),
    }
}

pub(crate) fn load_cursor(path: &std::path::Path) -> Result<CursorState> {
    if !path.exists() {
        return Ok(CursorState::default());
    }
    let data = std::fs::read_to_string(path)?;
    let state = serde_json::from_str::<CursorState>(&data).unwrap_or_default();
    Ok(state)
}

pub(crate) fn save_cursor(path: &std::path::Path, cursor: &CursorState) -> Result<()> {
    ensure_parent_dir(path)?;
    let data = serde_json::to_string_pretty(cursor)?;
    std::fs::write(path, data)?;
    Ok(())
}

pub(crate) fn reset_reason(stream: &std::path::Path, cursor: &CursorState) -> StreamReset {
    let Ok(meta) = metadata(stream) else {
        return StreamReset::None;
    };
    let inode = stream_inode(&meta);
    if inode.is_some() && cursor.stream_inode.is_some() && inode != cursor.stream_inode {
        return StreamReset::InodeChanged;
    }
    if meta.len() < cursor.offset || meta.len() < cursor.stream_size_seen {
        return StreamReset::Truncated;
    }
    StreamReset::None
}

pub(crate) fn file_len(path: &std::path::Path) -> Option<u64> {
    metadata(path).ok().map(|m| m.len())
}

pub(crate) fn file_inode(path: &std::path::Path) -> Option<u64> {
    metadata(path).ok().and_then(|m| stream_inode(&m))
}

pub(crate) fn read_stream(
    stream: &std::path::Path,
    start_offset: u64,
    args: &InboxArgs,
    last_msg_id: &Option<String>,
    mut waiting_for_last_seen: bool,
) -> Result<(Vec<StreamMessage>, u64, Option<String>, bool)> {
    let Ok(file) = File::open(stream) else {
        return Ok((Vec::new(), start_offset, last_msg_id.clone(), false));
    };

    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(start_offset))?;

    let mut messages = Vec::new();
    let mut had_new_data = false;
    let mut offset = start_offset;
    let mut newest_id = last_msg_id.clone();
    let mut line = String::new();

    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            break;
        }
        had_new_data = true;
        offset += bytes as u64;

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(msg) = serde_json::from_str::<StreamMessage>(trimmed) else {
            continue;
        };

        // Track the newest stream record seen, even if filtered out for this reader.
        newest_id = Some(msg.id.clone());

        if waiting_for_last_seen {
            if Some(msg.id.as_str()) == last_msg_id.as_deref() {
                waiting_for_last_seen = false;
            }
            continue;
        }

        if !visible_to_reader(&msg, args) {
            continue;
        }
        if !matches_filters(&msg, args) {
            continue;
        }

        messages.push(msg);
    }

    if let Some(last) = args.last
        && messages.len() > last
    {
        messages = messages.split_off(messages.len() - last);
    }

    Ok((messages, offset, newest_id, had_new_data))
}

fn visible_to_reader(msg: &StreamMessage, args: &InboxArgs) -> bool {
    match msg.to.as_deref() {
        Some(target) => target == args.identity,
        None => true,
    }
}

fn matches_filters(msg: &StreamMessage, args: &InboxArgs) -> bool {
    if let Some(sender) = &args.sender
        && msg.from != *sender
    {
        return false;
    }
    if let Some(kind) = &args.kind
        && msg.kind != *kind
    {
        return false;
    }
    if let Some(session) = &args.session {
        match msg.session_id.as_deref() {
            Some(id) if id.starts_with(session) => {}
            _ => return false,
        }
    }
    if let Some(since) = &args.since
        && let Ok(since_date) = chrono::NaiveDate::parse_from_str(since, "%Y-%m-%d")
    {
        let since_dt = since_date
            .and_hms_opt(0, 0, 0)
            .map(|dt| dt.and_utc())
            .unwrap_or_default();
        if msg.ts < since_dt {
            return false;
        }
    }
    true
}
