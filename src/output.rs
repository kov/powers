use chrono::{DateTime, Utc};

use crate::collab::StreamMessage;
use crate::persisted::{extract_embedded_preview, parse_persisted_output_ref, read_preview};
use crate::session::{ContentPart, Message, MessageContent, SessionMeta};

#[derive(Debug, Clone, Copy)]
pub struct MessageRenderOptions {
    pub expand_persisted: bool,
    pub max_bytes: usize,
    pub expand_tool_calls: bool,
}

impl Default for MessageRenderOptions {
    fn default() -> Self {
        Self {
            expand_persisted: false,
            max_bytes: 8192,
            expand_tool_calls: false,
        }
    }
}

/// Get the current terminal width, with fallback to 100.
pub fn terminal_width() -> usize {
    terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(100)
}

/// Get the current terminal height, with fallback to 30.
pub fn terminal_height() -> usize {
    terminal_size::terminal_size()
        .map(|(_, h)| h.0 as usize)
        .unwrap_or(30)
}

/// Truncate a string to fit within max_len, appending "…" if needed.
pub(crate) fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        // Find a char boundary
        let mut end = max_len.saturating_sub(1);
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// Format a DateTime as YYYY-MM-DD
pub fn format_date(dt: &DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d").to_string()
}

/// Format a DateTime as YYYY-MM-DDTHH:MM:SSZ
pub fn format_datetime(dt: &DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Print the `list` table header.
pub fn print_list_header() {
    println!(
        "{:<36}  {:<8}  {:<28}  {:<10}  {:>5}  TITLE",
        "SESSION-ID", "TOOL", "PROJECT", "DATE", "MSGS"
    );
    println!("{}", "-".repeat(100));
}

/// The best one-line label for a session: its ai-title, falling back to the last
/// recorded prompt, then "-".
fn session_label(meta: &SessionMeta) -> String {
    meta.title
        .as_deref()
        .or(meta.last_prompt.as_deref())
        .map(|s| s.replace('\n', " "))
        .unwrap_or_else(|| "-".to_string())
}

/// Print a single session row in table format.
pub fn print_list_row(meta: &SessionMeta) {
    let project = meta
        .project_path
        .as_deref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "-".to_string());

    println!(
        "{:<36}  {:<8}  {:<28}  {:<10}  {:>5}  {}",
        truncate(&meta.id, 36),
        truncate(&meta.tool.to_string(), 8),
        truncate(&project, 28),
        format_date(&meta.last_activity),
        meta.message_count,
        truncate(&session_label(meta), 60),
    );
}

/// Print a single session row in TSV format.
pub fn print_list_row_tsv(meta: &SessionMeta) {
    let project = meta
        .project_path
        .as_deref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    println!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        meta.id,
        meta.tool,
        project,
        format_date(&meta.last_activity),
        meta.message_count,
        meta.title.as_deref().unwrap_or("").replace('\n', " "),
        meta.last_prompt.as_deref().unwrap_or("").replace('\n', " "),
    );
}

/// Print a search match header.
pub fn print_search_header(meta: &SessionMeta) {
    let project = meta
        .project_path
        .as_deref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "-".to_string());

    println!(
        "=== {} {}  {}  {} ===",
        meta.tool,
        &meta.id[..meta.id.len().min(8)],
        project,
        format_date(&meta.last_activity),
    );
}

/// Print a single search result message.
pub fn print_search_match(msg: &Message) {
    let ts = msg
        .timestamp
        .as_ref()
        .map(format_datetime)
        .unwrap_or_default();
    println!("[msg {} / {} / {}]", msg.index, msg.role, ts);
    let text = msg.content.extract_text();
    for line in text.lines().take(10) {
        println!("  {}", line);
    }
}

/// Print the `show` command header for a session slice.
pub fn print_show_header(meta: &SessionMeta, from: usize, to: usize, total: usize) {
    let project = meta
        .project_path
        .as_deref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "-".to_string());

    println!(
        "=== {} {} | {} | {} | msgs {}-{} of {} ===",
        meta.tool,
        meta.id,
        project,
        format_date(&meta.last_activity),
        from,
        to,
        total,
    );
}

/// Print a single message in `show` output.
pub fn print_message(msg: &Message, width: usize, opts: MessageRenderOptions) {
    let ts = msg
        .timestamp
        .as_ref()
        .map(format_datetime)
        .unwrap_or_default();
    println!("[{}] {} {}", msg.index, msg.role, ts);
    print_message_content(&msg.content, width, opts);
    println!();
}

fn print_message_content(content: &MessageContent, width: usize, opts: MessageRenderOptions) {
    match content {
        MessageContent::Text(s) => {
            print_wrapped(s, width, "  ");
        }
        MessageContent::Parts(parts) => {
            for part in parts {
                match part {
                    ContentPart::Text(s) => {
                        print_wrapped(s, width, "  ");
                    }
                    ContentPart::Thinking(s) => {
                        println!("  [thinking]");
                        print_wrapped(s, width, "    ");
                    }
                    ContentPart::ToolCall { name, input } => {
                        if opts.expand_tool_calls {
                            render_tool_call(name, input, width);
                        } else {
                            println!("  [tool_call: {}]", name);
                            let preview = input.lines().next().unwrap_or("").trim();
                            println!("    {}", truncate(preview, width.saturating_sub(4)));
                        }
                    }
                    ContentPart::ToolResult {
                        tool_use_id,
                        content,
                    } => {
                        println!("  [tool_result: {}]", tool_use_id);
                        if let Some(persisted) = parse_persisted_output_ref(content) {
                            println!("    persisted_path: {}", persisted.path.display());
                            if let Some(size) = persisted.declared_size {
                                println!("    persisted_size: {}", size);
                            }
                            if opts.expand_persisted {
                                match read_preview(&persisted.path, opts.max_bytes) {
                                    Ok(preview) => {
                                        println!(
                                            "    resolved_bytes: {} of {}",
                                            preview.bytes_read, preview.total_size
                                        );
                                        for line in preview.text.lines() {
                                            println!("    {}", line);
                                        }
                                        if preview.truncated {
                                            println!(
                                                "    [truncated at {} bytes; use --max-bytes to increase]",
                                                opts.max_bytes
                                            );
                                        }
                                    }
                                    Err(err) => {
                                        println!("    [error reading persisted output: {err}]");
                                        if let Some(preview) = extract_embedded_preview(content) {
                                            println!("    [embedded preview]");
                                            for line in preview.lines() {
                                                println!("    {}", line);
                                            }
                                        }
                                    }
                                }
                            } else {
                                println!(
                                    "    [persisted output not expanded; rerun with --expand-persisted --max-bytes {}]",
                                    opts.max_bytes
                                );
                                println!(
                                    "    [or query directly: powers tool-result {} --max-bytes {}]",
                                    tool_use_id, opts.max_bytes
                                );
                            }
                        } else {
                            let preview = content.lines().next().unwrap_or("").trim();
                            println!("    {}", truncate(preview, width.saturating_sub(4)));
                        }
                    }
                }
            }
        }
    }
}

fn render_tool_call(name: &str, input: &str, width: usize) {
    let parsed: Option<serde_json::Value> = serde_json::from_str(input).ok();
    match (name, parsed.as_ref()) {
        ("Edit", Some(v)) => render_edit_call(v, width),
        ("MultiEdit", Some(v)) => render_multi_edit_call(v, width),
        ("Write", Some(v)) => render_write_call(v, width),
        ("Read" | "NotebookRead", Some(v)) => render_read_call(name, v),
        ("Bash", Some(v)) => render_bash_call(v, width),
        _ => render_generic_call(name, input, width),
    }
}

fn render_generic_call(name: &str, input: &str, width: usize) {
    println!("  [tool_call: {}]", name);
    let pretty = serde_json::from_str::<serde_json::Value>(input)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| input.to_string());
    print_wrapped(&pretty, width, "    ");
}

fn render_edit_call(v: &serde_json::Value, width: usize) {
    let file_path = v["file_path"].as_str().unwrap_or("-");
    let replace_all = v["replace_all"].as_bool().unwrap_or(false);
    let suffix = if replace_all { " (replace_all)" } else { "" };
    println!("  [tool_call: Edit{}] {}", suffix, file_path);
    render_diff(
        v["old_string"].as_str().unwrap_or(""),
        v["new_string"].as_str().unwrap_or(""),
        width,
    );
}

fn render_multi_edit_call(v: &serde_json::Value, width: usize) {
    let file_path = v["file_path"].as_str().unwrap_or("-");
    let empty = Vec::new();
    let edits = v["edits"].as_array().unwrap_or(&empty);
    let plural = if edits.len() == 1 { "" } else { "s" };
    println!(
        "  [tool_call: MultiEdit] {} ({} edit{})",
        file_path,
        edits.len(),
        plural
    );
    for (i, edit) in edits.iter().enumerate() {
        let replace_all = edit["replace_all"].as_bool().unwrap_or(false);
        let suffix = if replace_all { " (replace_all)" } else { "" };
        println!("    --- edit {}/{}{} ---", i + 1, edits.len(), suffix);
        render_diff(
            edit["old_string"].as_str().unwrap_or(""),
            edit["new_string"].as_str().unwrap_or(""),
            width,
        );
    }
}

fn render_write_call(v: &serde_json::Value, width: usize) {
    let file_path = v["file_path"].as_str().unwrap_or("-");
    println!("  [tool_call: Write] {}", file_path);
    let content = v["content"].as_str().unwrap_or("");
    if !content.is_empty() {
        print_wrapped(content, width, "    + ");
    }
}

fn render_read_call(name: &str, v: &serde_json::Value) {
    let file_path = v["file_path"].as_str().unwrap_or("-");
    let mut line = format!("  [tool_call: {}] {}", name, file_path);
    if let Some(offset) = v["offset"].as_u64() {
        line.push_str(&format!(" offset={}", offset));
    }
    if let Some(limit) = v["limit"].as_u64() {
        line.push_str(&format!(" limit={}", limit));
    }
    println!("{}", line);
}

fn render_bash_call(v: &serde_json::Value, width: usize) {
    if let Some(desc) = v["description"].as_str() {
        println!("  [tool_call: Bash] {}", desc);
    } else {
        println!("  [tool_call: Bash]");
    }
    let command = v["command"].as_str().unwrap_or("");
    if !command.is_empty() {
        print_wrapped(command, width, "    $ ");
    }
}

fn render_diff(old: &str, new: &str, width: usize) {
    if !old.is_empty() {
        print_wrapped(old, width, "    - ");
    }
    if !new.is_empty() {
        print_wrapped(new, width, "    + ");
    }
}

pub fn print_tool_result_header(meta: &SessionMeta, msg_index: usize, tool_use_id: &str) {
    println!(
        "=== tool_result {} | {} {} | msg {} ===",
        tool_use_id, meta.tool, meta.id, msg_index
    );
}

pub fn print_line(line: &str) {
    println!("{line}");
}

pub fn print_blank_line() {
    println!();
}

/// Print `info` command output for a session.
pub fn print_info(meta: &SessionMeta) {
    println!("Session:      {}", meta.id);
    if let Some(title) = &meta.title {
        println!("Title:        {}", title.replace('\n', " "));
    }
    println!("Tool:         {}", meta.tool);
    println!(
        "Project:      {}",
        meta.project_path
            .as_deref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "-".to_string())
    );
    println!(
        "Git branch:   {}",
        meta.git_branch.as_deref().unwrap_or("-")
    );
    println!("Started:      {}", format_datetime(&meta.started_at));
    println!("Last active:  {}", format_datetime(&meta.last_activity));
    println!("Messages:     {}", meta.message_count);
    println!("File:         {}", meta.file_path.display());
}

pub fn print_post_result(msg: &StreamMessage) {
    match &msg.to {
        Some(to) => println!("posted {} from {} to {}", msg.id, msg.from, to),
        None => println!("posted {} from {} to *", msg.id, msg.from),
    }
}

pub fn print_inbox_message(msg: &StreamMessage) {
    print_inbox_message_prefixed(msg, None);
}

pub fn print_inbox_message_prefixed(msg: &StreamMessage, source: Option<&str>) {
    let ts = format_datetime(&msg.ts);
    let to = msg.to.as_deref().unwrap_or("*");
    if let Some(label) = source {
        print!("[{label}] ")
    }
    if let Some(session) = msg.session_id.as_deref() {
        println!(
            "[{}] {} -> {} kind={} session={}",
            ts, msg.from, to, msg.kind, session,
        );
    } else {
        println!("[{}] {} -> {} kind={}", ts, msg.from, to, msg.kind);
    }
    for line in msg.body.lines() {
        match source {
            Some(label) => println!("[{label}]   {}", line),
            None => println!("  {}", line),
        }
    }
    match source {
        Some(label) => println!("[{label}]"),
        None => println!(),
    }
}

pub fn print_inbox_message_json(msg: &StreamMessage) {
    if let Ok(json) = serde_json::to_string(msg) {
        println!("{json}");
    }
}

/// Print an error message to stderr.
pub fn print_error(msg: &str) {
    eprintln!("error: {}", msg);
}

/// Print a warning message to stderr.
pub fn print_warn(msg: &str) {
    eprintln!("warning: {}", msg);
}

/// Word-wrap text with a given prefix, respecting the terminal width.
fn print_wrapped(text: &str, width: usize, prefix: &str) {
    let content_width = width.saturating_sub(prefix.len());
    if content_width == 0 {
        print!("{}{}", prefix, text);
        return;
    }

    for line in text.lines() {
        if line.is_empty() {
            println!();
            continue;
        }
        // Simple character-based wrapping (good enough for CLI output)
        let mut remaining = line;
        while !remaining.is_empty() {
            if remaining.len() <= content_width {
                println!("{}{}", prefix, remaining);
                break;
            }
            // Try to break at a word boundary
            let break_at = find_break(remaining, content_width);
            println!("{}{}", prefix, &remaining[..break_at]);
            remaining = remaining[break_at..].trim_start();
        }
    }
}

pub(crate) fn find_break(s: &str, max: usize) -> usize {
    // Find last space before max chars
    if s.len() <= max {
        return s.len();
    }
    // Walk back from max to find a char boundary and space
    let mut pos = max;
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    // Try to find a space to break at
    if let Some(sp) = s[..pos].rfind(' ')
        && sp > 0
    {
        return sp;
    }
    pos
}
