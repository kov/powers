use anyhow::Result;
use chrono::{NaiveDate, Timelike};
use std::fs::{File, metadata};
use std::io::{BufRead, BufReader, IsTerminal, Seek, SeekFrom, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::cli::LogArgs;
use crate::collab::{CursorState, StreamMessage, project_hash, resolve_project, stream_inode, stream_path};
use crate::config::Config;
use crate::output;

#[derive(Debug, Clone)]
struct LogRecord {
    msg: StreamMessage,
    byte_end: u64,
}

#[derive(Debug, Clone)]
struct AgentCursor {
    agent: String,
    cursor: CursorState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadStatus {
    Read,
    Unread,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
struct StreamMeta {
    inode: Option<u64>,
    len: u64,
}

pub fn run(args: &LogArgs) -> Result<()> {
    let config = Config::load();
    let project = resolve_project(&args.project)?;
    let hash = project_hash(&project);
    let stream = stream_path(&config.powers_dir, &hash);
    let state_root = config.powers_dir.join("state");

    let (mut all_records, mut offset) = read_records_from(&stream, 0)?;
    let tty = std::io::stdout().is_terminal();
    let mut prev_drawn_lines = 0usize;
    let mut cursors = load_agent_cursors(&state_root, &hash)?;
    let mut stream_meta = read_stream_meta(&stream);

    if args.follow && tty {
        eprintln!("(following collaboration log - Ctrl-C to stop)");
    }

    if tty && args.follow {
        prev_drawn_lines = redraw_tty(args, &all_records, &cursors, stream_meta, prev_drawn_lines)?;
    } else {
        print_snapshot(args, &all_records, &cursors, stream_meta, tty)?;
    }
    let mut prev_cursor_sig: Option<String> = Some(cursors_signature(&cursors));

    if !args.follow {
        return Ok(());
    }

    let min_sleep = args.poll_ms.unwrap_or(250).clamp(50, 2000);
    let mut sleep_ms = min_sleep;

    loop {
        let mut truncated = false;
        let mut appended: Vec<LogRecord> = Vec::new();

        let len_now = read_file_len(&stream).unwrap_or(0);
        if len_now < offset {
            let (records, new_offset) = read_records_from(&stream, 0)?;
            all_records = records;
            offset = new_offset;
            truncated = true;
        } else {
            let (new_records, new_offset) = read_records_from(&stream, offset)?;
            if !new_records.is_empty() {
                appended = new_records.clone();
                all_records.extend(new_records);
            }
            offset = new_offset;
        }

        cursors = load_agent_cursors(&state_root, &hash)?;
        stream_meta = read_stream_meta(&stream);

        let new_sig = cursors_signature(&cursors);
        let cursor_changed = cursors_changed(prev_cursor_sig.as_deref(), &new_sig);
        let has_new_records = !appended.is_empty();
        let changed = truncated || has_new_records || cursor_changed;

        if changed {
            if tty {
                prev_drawn_lines =
                    redraw_tty(args, &all_records, &cursors, stream_meta, prev_drawn_lines)?;
            } else if truncated {
                print_snapshot(args, &all_records, &cursors, stream_meta, tty)?;
            } else if has_new_records {
                let filtered = appended
                    .iter()
                    .filter(|r| apply_filters(&r.msg, args))
                    .collect::<Vec<_>>();
                print_records(args, &filtered, &cursors, stream_meta, tty, false)?;
            }
            sleep_ms = min_sleep;
            prev_cursor_sig = Some(new_sig);
        } else {
            sleep_ms = (sleep_ms + min_sleep).min(2000);
        }

        thread::sleep(Duration::from_millis(sleep_ms));
    }
}

fn read_records_from(path: &Path, start_offset: u64) -> Result<(Vec<LogRecord>, u64)> {
    let Ok(file) = File::open(path) else {
        return Ok((Vec::new(), start_offset));
    };

    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(start_offset))?;

    let mut line = String::new();
    let mut offset = start_offset;
    let mut out = Vec::new();

    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            break;
        }

        let byte_end = offset + bytes as u64;
        offset = byte_end;

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(msg) = serde_json::from_str::<StreamMessage>(trimmed) else {
            continue;
        };

        out.push(LogRecord { msg, byte_end });
    }

    Ok((out, offset))
}

fn read_stream_meta(path: &Path) -> StreamMeta {
    if let Ok(meta) = metadata(path) {
        StreamMeta {
            inode: stream_inode(&meta),
            len: meta.len(),
        }
    } else {
        StreamMeta {
            inode: None,
            len: 0,
        }
    }
}

fn load_agent_cursors(state_root: &Path, hash: &str) -> Result<Vec<AgentCursor>> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(state_root) else {
        return Ok(out);
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let agent = entry.file_name().to_string_lossy().to_string();
        let state_file = path.join(format!("{hash}.json"));
        let cursor = if state_file.exists() {
            let data = std::fs::read_to_string(&state_file)?;
            serde_json::from_str::<CursorState>(&data).unwrap_or_default()
        } else {
            CursorState::default()
        };

        out.push(AgentCursor { agent, cursor });
    }

    out.sort_by(|a, b| a.agent.cmp(&b.agent));
    Ok(out)
}

fn filtered_records<'a>(args: &LogArgs, records: &'a [LogRecord]) -> Vec<&'a LogRecord> {
    records
        .iter()
        .filter(|r| apply_filters(&r.msg, args))
        .collect::<Vec<_>>()
}

fn print_snapshot(
    args: &LogArgs,
    records: &[LogRecord],
    cursors: &[AgentCursor],
    stream_meta: StreamMeta,
    tty: bool,
) -> Result<()> {
    let filtered = filtered_records(args, records);
    let visible = count_visible_rows(args, usize::MAX);
    let start = filtered.len().saturating_sub(visible);
    print_records(args, &filtered[start..], cursors, stream_meta, tty, true)
}

fn redraw_tty(
    args: &LogArgs,
    records: &[LogRecord],
    cursors: &[AgentCursor],
    stream_meta: StreamMeta,
    prev_lines: usize,
) -> Result<usize> {
    let width = args.width.unwrap_or_else(output::terminal_width);
    let height = output::terminal_height();
    let max_visible = count_visible_rows(args, height.saturating_sub(4).max(1));

    let filtered = filtered_records(args, records);
    let start = filtered.len().saturating_sub(max_visible);

    let mut lines = render_header(width);
    for record in &filtered[start..] {
        lines.extend(render_record(
            args,
            record,
            cursors,
            stream_meta,
            width,
            true,
        ));
    }

    let mut stdout = std::io::stdout().lock();
    if prev_lines > 0 {
        write!(stdout, "\x1b[{}A\x1b[J", prev_lines)?;
    }
    for line in &lines {
        writeln!(stdout, "{}", line)?;
    }
    stdout.flush()?;
    Ok(lines.len())
}

fn print_records(
    args: &LogArgs,
    records: &[&LogRecord],
    cursors: &[AgentCursor],
    stream_meta: StreamMeta,
    tty: bool,
    include_header: bool,
) -> Result<()> {
    let width = args.width.unwrap_or_else(output::terminal_width);
    let mut stdout = std::io::stdout().lock();

    if include_header {
        for line in render_header(width) {
            writeln!(stdout, "{}", line)?;
        }
    }

    for record in records {
        for line in render_record(args, record, cursors, stream_meta, width, tty) {
            writeln!(stdout, "{}", line)?;
        }
    }

    stdout.flush()?;
    Ok(())
}

fn render_header(width: usize) -> Vec<String> {
    let header = format!(
        "{:<8}  {:<8}  {:<8}  {:<16}  {:<18}  {}",
        "TIME", "FROM", "TO", "KIND", "[STATUS]", "PREVIEW"
    );
    vec![header.clone(), "-".repeat(width.min(header.len().max(80)))]
}

fn render_record(
    args: &LogArgs,
    record: &LogRecord,
    cursors: &[AgentCursor],
    stream_meta: StreamMeta,
    width: usize,
    tty: bool,
) -> Vec<String> {
    let statuses = format_statuses(record, cursors, stream_meta, args.no_color || !tty);

    let time = format!(
        "{:02}:{:02}:{:02}",
        record.msg.ts.hour(),
        record.msg.ts.minute(),
        record.msg.ts.second()
    );
    let from = output::truncate(&record.msg.from, 8);
    let to = output::truncate(record.msg.to.as_deref().unwrap_or("*"), 8);
    let kind = output::truncate(&record.msg.kind, 16);

    let color = !args.no_color && tty;
    let time_col = paint(&format!("{:<8}", time), "90", color);
    let from_col = paint(&format!("{:<8}", from), "36", color);
    let to_col = paint(&format!("{:<8}", to), "33", color);
    let kind_col = paint(&format!("{:<16}", kind), "35", color);

    let statuses_plain = format_statuses(record, cursors, stream_meta, true);
    let status_visual = statuses_plain.chars().count();

    let row = if args.expand {
        format!(
            "{}  {}  {}  {}  {}",
            time_col, from_col, to_col, kind_col, statuses
        )
    } else {
        let fixed = 8 + 2 + 8 + 2 + 8 + 2 + 16 + 2 + status_visual + 2;
        let preview_width = width.saturating_sub(fixed).max(8);
        let preview_line = record.msg.body.lines().next().unwrap_or("");
        let preview = output::truncate(preview_line, preview_width);
        format!(
            "{}  {}  {}  {}  {}  {}",
            time_col, from_col, to_col, kind_col, statuses, preview
        )
    };

    if !args.expand {
        return vec![row];
    }

    let mut out = vec![row];
    let body_width = width.saturating_sub(4).max(8);
    for line in record.msg.body.lines() {
        let mut remaining = line;
        if remaining.is_empty() {
            out.push("  │ ".to_string());
            continue;
        }
        loop {
            let bp = output::find_break(remaining, body_width);
            out.push(format!("  │ {}", &remaining[..bp]));
            remaining = remaining[bp..].trim_start_matches(' ');
            if remaining.is_empty() {
                break;
            }
        }
    }
    out.push(String::new());
    out
}

fn format_statuses(
    record: &LogRecord,
    cursors: &[AgentCursor],
    stream_meta: StreamMeta,
    no_color: bool,
) -> String {
    if cursors.is_empty() {
        return "[-]".to_string();
    }

    let mut parts = Vec::with_capacity(cursors.len());
    for c in cursors {
        let status = is_read_by(
            record.byte_end,
            &c.cursor,
            stream_meta.inode,
            stream_meta.len,
        );
        let token = match status {
            ReadStatus::Read => format!("{}{}", paint("✓", "32", !no_color), c.agent),
            ReadStatus::Unread => format!("{}{}", paint("●", "31", !no_color), c.agent),
            ReadStatus::Unknown => format!("{}{}", paint("?", "2", !no_color), c.agent),
        };
        parts.push(token);
    }

    format!("[{}]", parts.join(" "))
}

fn paint(s: &str, color_code: &str, enabled: bool) -> String {
    if enabled {
        format!("\x1b[{}m{}\x1b[0m", color_code, s)
    } else {
        s.to_string()
    }
}

fn apply_filters(msg: &StreamMessage, args: &LogArgs) -> bool {
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

    if let Some(to) = &args.to {
        if to == "*" {
            if msg.to.is_some() {
                return false;
            }
        } else if msg.to.as_deref() != Some(to.as_str()) {
            return false;
        }
    }

    if let Some(since) = &args.since
        && let Ok(since_date) = NaiveDate::parse_from_str(since, "%Y-%m-%d")
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

fn is_read_by(
    msg_byte_end: u64,
    cursor: &CursorState,
    stream_inode_now: Option<u64>,
    stream_len_now: u64,
) -> ReadStatus {
    if cursor.offset > stream_len_now {
        return ReadStatus::Unknown;
    }

    if stream_inode_now.is_some()
        && cursor.stream_inode.is_some()
        && cursor.stream_inode != stream_inode_now
    {
        return ReadStatus::Unknown;
    }

    if cursor.offset >= msg_byte_end {
        ReadStatus::Read
    } else {
        ReadStatus::Unread
    }
}

fn cursors_signature(cursors: &[AgentCursor]) -> String {
    let mut out = String::new();
    for c in cursors {
        out.push_str(&c.agent);
        out.push(':');
        out.push_str(&c.cursor.offset.to_string());
        out.push(':');
        out.push_str(&c.cursor.stream_size_seen.to_string());
        out.push(':');
        if let Some(inode) = c.cursor.stream_inode {
            out.push_str(&inode.to_string());
        }
        out.push('|');
    }
    out
}

fn cursors_changed(prev_sig: Option<&str>, current_sig: &str) -> bool {
    match prev_sig {
        Some(prev) => prev != current_sig,
        None => true,
    }
}

fn count_visible_rows(args: &LogArgs, max_rows: usize) -> usize {
    args.last.unwrap_or(50).min(max_rows)
}

fn read_file_len(path: &Path) -> Option<u64> {
    metadata(path).ok().map(|m| m.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn msg(from: &str, to: Option<&str>, kind: &str, body: &str) -> StreamMessage {
        StreamMessage {
            id: "m1".to_string(),
            ts: Utc.with_ymd_and_hms(2026, 2, 22, 12, 0, 0).unwrap(),
            from: from.to_string(),
            to: to.map(|s| s.to_string()),
            kind: kind.to_string(),
            body: body.to_string(),
            project_hash: "h".to_string(),
            project_path: None,
            session_id: None,
        }
    }

    fn args() -> LogArgs {
        LogArgs {
            project: None,
            last: Some(50),
            follow: false,
            expand: false,
            since: None,
            kind: None,
            sender: None,
            to: None,
            width: Some(120),
            poll_ms: None,
            no_color: true,
        }
    }

    #[test]
    fn is_read_by_handles_read_unread_and_stale_inode() {
        let read = CursorState {
            offset: 200,
            last_msg_id: None,
            stream_inode: Some(7),
            stream_size_seen: 200,
        };
        assert_eq!(is_read_by(120, &read, Some(7), 200), ReadStatus::Read);

        let unread = CursorState {
            offset: 80,
            last_msg_id: None,
            stream_inode: Some(7),
            stream_size_seen: 80,
        };
        assert_eq!(is_read_by(120, &unread, Some(7), 200), ReadStatus::Unread);

        let stale = CursorState {
            offset: 120,
            last_msg_id: None,
            stream_inode: Some(8),
            stream_size_seen: 120,
        };
        assert_eq!(is_read_by(120, &stale, Some(7), 200), ReadStatus::Unknown);
    }

    #[test]
    fn apply_filters_matches_each_filter_type() {
        let mut a = args();
        let m = msg("claude", Some("codex"), "handoff", "body");

        a.sender = Some("claude".to_string());
        assert!(apply_filters(&m, &a));
        a.sender = Some("gemini".to_string());
        assert!(!apply_filters(&m, &a));

        let mut a = args();
        a.kind = Some("handoff".to_string());
        assert!(apply_filters(&m, &a));
        a.kind = Some("note".to_string());
        assert!(!apply_filters(&m, &a));

        let mut a = args();
        a.to = Some("codex".to_string());
        assert!(apply_filters(&m, &a));
        a.to = Some("*".to_string());
        assert!(!apply_filters(&m, &a));

        let mut a = args();
        a.since = Some("2026-02-22".to_string());
        assert!(apply_filters(&m, &a));
        a.since = Some("2026-02-23".to_string());
        assert!(!apply_filters(&m, &a));
    }

    #[test]
    fn render_record_compact_vs_expand() {
        let mut a = args();
        let rec = LogRecord {
            msg: msg("claude", Some("codex"), "note", "line one\nline two"),
            byte_end: 20,
        };

        let compact = render_record(
            &a,
            &rec,
            &[],
            StreamMeta {
                inode: Some(7),
                len: 20,
            },
            100,
            false,
        );
        assert_eq!(compact.len(), 1);
        assert!(compact[0].contains("line one"));

        a.expand = true;
        let expanded = render_record(
            &a,
            &rec,
            &[],
            StreamMeta {
                inode: Some(7),
                len: 20,
            },
            100,
            false,
        );
        assert!(expanded.len() >= 3);
        assert!(expanded.iter().any(|l| l.contains("│ line two")));
    }

    #[test]
    fn cursors_changed_detection_works() {
        let a = vec![AgentCursor {
            agent: "codex".to_string(),
            cursor: CursorState {
                offset: 10,
                ..CursorState::default()
            },
        }];
        let b = vec![AgentCursor {
            agent: "codex".to_string(),
            cursor: CursorState {
                offset: 11,
                ..CursorState::default()
            },
        }];

        let sig_a = cursors_signature(&a);
        let sig_b = cursors_signature(&b);

        assert!(!cursors_changed(Some(sig_a.as_str()), &sig_a));
        assert!(cursors_changed(Some(sig_a.as_str()), &sig_b));
    }
}
