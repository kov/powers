use anyhow::{Context, Result, bail};
use regex::Regex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use crate::cli::{SearchArgs, ToolFilter};
use crate::config::Config;
use crate::output;
use crate::parsers::{
    Parser, claude::ClaudeParser, codex::CodexParser, copilot::CopilotParser, gemini::GeminiParser,
};
use crate::session::{ContentPart, Message, MessageContent, Role, SessionMeta, Tool};
use crate::util;

/// Which block kinds to search, and any tool-name restriction.
struct MatchSpec {
    user: bool,
    assistant: bool,
    thinking: bool,
    tool_use: bool,
    tool_result: bool,
    tool_names: Vec<String>,
}

impl MatchSpec {
    /// Build from `--in` (comma-separated kinds) plus the legacy
    /// `--include-tool-calls` flag and `--tool-name` restriction.
    fn from_args(args: &SearchArgs) -> Result<MatchSpec> {
        let mut spec = MatchSpec {
            user: false,
            assistant: false,
            thinking: false,
            tool_use: false,
            tool_result: false,
            tool_names: args.tool_names.clone(),
        };
        match args.r#in.as_deref() {
            Some(list) => {
                for tok in list.split(',').map(str::trim).filter(|t| !t.is_empty()) {
                    match tok {
                        "user" => spec.user = true,
                        "assistant" => spec.assistant = true,
                        "thinking" => spec.thinking = true,
                        "tool_use" => spec.tool_use = true,
                        "tool_result" => spec.tool_result = true,
                        "prose" => {
                            spec.user = true;
                            spec.assistant = true;
                        }
                        "all" => {
                            spec.user = true;
                            spec.assistant = true;
                            spec.thinking = true;
                            spec.tool_use = true;
                            spec.tool_result = true;
                        }
                        other => bail!(
                            "unknown --in kind '{other}' (valid: user, assistant, thinking, tool_use, tool_result, prose, all)"
                        ),
                    }
                }
            }
            None => {
                spec.user = true;
                spec.assistant = true;
                // A bare --tool-name implies searching tool blocks.
                if !spec.tool_names.is_empty() {
                    spec.tool_use = true;
                    spec.tool_result = true;
                }
            }
        }
        // Legacy flag folds in tool_use.
        if args.include_tool_calls {
            spec.tool_use = true;
        }
        Ok(spec)
    }

    /// The selected kinds as names, for JSON metadata.
    fn kind_names(&self) -> Vec<String> {
        let mut v = Vec::new();
        for (on, name) in [
            (self.user, "user"),
            (self.assistant, "assistant"),
            (self.thinking, "thinking"),
            (self.tool_use, "tool_use"),
            (self.tool_result, "tool_result"),
        ] {
            if on {
                v.push(name.to_string());
            }
        }
        v
    }

    fn tool_name_allowed(&self, name: Option<&str>) -> bool {
        if self.tool_names.is_empty() {
            return true;
        }
        match name {
            Some(n) => self.tool_names.iter().any(|t| t.eq_ignore_ascii_case(n)),
            None => false,
        }
    }
}

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

    let spec = MatchSpec::from_args(args)?;

    let config = Config::load();

    // Phase 1: discover and filter by tool/project/time using only SessionMeta
    let all_sessions = discover_filtered(&config, args)?;

    if args.format == crate::cli::TextFormat::Json {
        return run_json(&pattern, &spec, args, &all_sessions);
    }

    let mut total_matches = 0usize;
    // uuids that have already produced a match, so the same message reappearing in
    // a resumed/forked session file is not reported twice. Sessions are sorted
    // newest-first, so the surviving copy is the most recent one.
    let mut seen_uuids: HashSet<String> = HashSet::new();

    for meta in &all_sessions {
        if total_matches >= args.max_matches {
            break;
        }
        let matched = search_session(
            meta,
            &pattern,
            &spec,
            args,
            &mut total_matches,
            &mut seen_uuids,
        )?;
        if args.session_only && matched {
            println!("{}", meta.id);
        }
    }

    Ok(())
}

/// JSON output path: collect hits per session (no context messages) and emit one
/// structured document. Shares the same matching, dedup, and attribution logic.
fn run_json(
    pattern: &Regex,
    spec: &MatchSpec,
    args: &SearchArgs,
    sessions: &[SessionMeta],
) -> Result<()> {
    use crate::response::{SearchJson, SearchMetaJson, SessionHitsJson};

    let mut seen_uuids: HashSet<String> = HashSet::new();
    let mut total_matches = 0usize;
    let mut results: Vec<SessionHitsJson> = Vec::new();

    for meta in sessions {
        if total_matches >= args.max_matches {
            break;
        }
        let hits = collect_hits(
            meta,
            pattern,
            spec,
            args,
            &mut total_matches,
            &mut seen_uuids,
        )?;
        if !hits.is_empty() {
            results.push(SessionHitsJson {
                session_id: meta.id.clone(),
                tool: meta.tool.to_string(),
                project: meta
                    .project_path
                    .as_deref()
                    .map(|p| p.to_string_lossy().into_owned()),
                last_activity: output::format_datetime(&meta.last_activity),
                hits,
            });
        }
    }

    let doc = SearchJson {
        meta: SearchMetaJson {
            query: args.pattern.clone(),
            regex: args.regex,
            kinds: spec.kind_names(),
            total_matches,
            sessions_matched: results.len(),
        },
        results,
    };
    output::print_json(&doc)
}

/// Collect matching hits for one session as JSON records (no surrounding context).
fn collect_hits(
    meta: &SessionMeta,
    pattern: &Regex,
    spec: &MatchSpec,
    args: &SearchArgs,
    total_matches: &mut usize,
    seen_uuids: &mut HashSet<String>,
) -> Result<Vec<crate::response::HitJson>> {
    let mut tool_map: HashMap<String, (String, String)> = HashMap::new();
    let mut hits = Vec::new();

    if meta.tool == Tool::Gemini {
        let session = GeminiParser::new(&Config::load()).load(meta)?;
        for msg in &session.messages {
            if *total_matches >= args.max_matches {
                break;
            }
            record_tool_calls(msg, &mut tool_map);
            if let Some(hit) = match_message(msg, pattern, spec, &tool_map) {
                hits.push(hit_json(msg, &hit));
                *total_matches += 1;
            }
        }
        return Ok(hits);
    }

    let file = match std::fs::File::open(&meta.file_path) {
        Ok(f) => f,
        Err(_) => return Ok(hits),
    };
    let mut lines = BufReader::new(file).lines();
    lines.next(); // skip header line
    let mut msg_index = 0usize;
    for line in lines {
        if *total_matches >= args.max_matches {
            break;
        }
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((mut msg, uuid)) = parse_jsonl_message(line, &meta.tool) else {
            continue;
        };
        msg.index = msg_index;
        msg_index += 1;
        record_tool_calls(&msg, &mut tool_map);
        let Some(hit) = match_message(&msg, pattern, spec, &tool_map) else {
            continue;
        };
        // Cross-file dedup by uuid (record on first, newest match).
        if let Some(u) = uuid
            && !seen_uuids.insert(u)
        {
            continue;
        }
        hits.push(hit_json(&msg, &hit));
        *total_matches += 1;
    }
    Ok(hits)
}

fn hit_json(msg: &Message, hit: &Hit) -> crate::response::HitJson {
    crate::response::HitJson {
        index: msg.index,
        role: msg.role.to_string(),
        timestamp: msg.timestamp.as_ref().map(output::format_datetime),
        matched_in: hit.matched_in.to_string(),
        tool: hit.tool.clone(),
        tool_input: hit.tool_input.clone(),
        is_error: hit.is_error,
        snippet: hit.snippet.clone(),
        match_count: hit.match_count,
        full_length: hit.full_length,
    }
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

/// A match within a message: a snippet centered on the first hit, plus which
/// block it came from and (for tool_result) which tool produced it.
struct Hit {
    matched_in: &'static str,
    tool: Option<String>,
    tool_input: Option<String>,
    is_error: bool,
    snippet: String,
    match_count: usize,
    full_length: usize,
}

/// Scan one block's text; on a match, build a snippet centered on it.
fn scan(
    pattern: &Regex,
    text: &str,
    kind: &'static str,
    tool: Option<String>,
    tool_input: Option<String>,
    is_error: bool,
) -> Option<Hit> {
    let first = pattern.find(text)?;
    let match_count = pattern.find_iter(text).count();
    let snippet = util::build_snippet(text, first.start(), first.end(), 2, 240);
    Some(Hit {
        matched_in: kind,
        tool,
        tool_input,
        is_error,
        snippet,
        match_count,
        full_length: text.chars().count(),
    })
}

/// Test a message against the pattern, honoring the `--in` kinds and
/// `--tool-name` restriction. Returns the first matching block as a [`Hit`].
/// `tool_map` attributes a tool_result back to the tool_use that produced it.
fn match_message(
    msg: &Message,
    pattern: &Regex,
    spec: &MatchSpec,
    tool_map: &HashMap<String, (String, String)>,
) -> Option<Hit> {
    let text_wanted = match msg.role {
        Role::User => spec.user,
        Role::Assistant => spec.assistant,
    };
    match &msg.content {
        MessageContent::Text(s) => {
            if !text_wanted {
                return None;
            }
            scan(
                pattern,
                &util::strip_reminders(s),
                "text",
                None,
                None,
                false,
            )
        }
        MessageContent::Parts(parts) => {
            for p in parts {
                let hit = match p {
                    ContentPart::Text(t) if text_wanted => scan(
                        pattern,
                        &util::strip_reminders(t),
                        "text",
                        None,
                        None,
                        false,
                    ),
                    ContentPart::Thinking(t) if spec.thinking => {
                        scan(pattern, t, "thinking", None, None, false)
                    }
                    ContentPart::ToolCall { name, input, .. } if spec.tool_use => {
                        if !spec.tool_name_allowed(Some(name)) {
                            continue;
                        }
                        let summary = util::summarize_tool_input(name, input);
                        scan(
                            pattern,
                            input,
                            "tool_use",
                            Some(name.clone()),
                            Some(summary),
                            false,
                        )
                    }
                    ContentPart::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } if spec.tool_result => {
                        let origin = tool_map.get(tool_use_id);
                        let name = origin.map(|(n, _)| n.clone());
                        let input = origin.map(|(_, s)| s.clone());
                        if !spec.tool_name_allowed(name.as_deref()) {
                            continue;
                        }
                        scan(pattern, content, "tool_result", name, input, *is_error)
                    }
                    _ => continue,
                };
                if hit.is_some() {
                    return hit;
                }
            }
            None
        }
    }
}

/// Record a message's tool_use ids so later tool_result blocks can be attributed.
/// Claude always emits a tool_use before the tool_result that references it, so a
/// forward pass keeps the map complete. Bounded by tool_use count per session.
fn record_tool_calls(msg: &Message, tool_map: &mut HashMap<String, (String, String)>) {
    if let MessageContent::Parts(parts) = &msg.content {
        for p in parts {
            if let ContentPart::ToolCall {
                id: Some(id),
                name,
                input,
            } = p
            {
                tool_map.insert(
                    id.clone(),
                    (name.clone(), util::summarize_tool_input(name, input)),
                );
            }
        }
    }
}

/// Print a message: a match-centered snippet when it matched, else a short
/// context preview.
fn print_msg(msg: &Message, hit: Option<&Hit>) {
    match hit {
        Some(h) => output::print_search_snippet(
            msg,
            h.matched_in,
            h.tool.as_deref(),
            h.tool_input.as_deref(),
            h.is_error,
            &h.snippet,
            h.match_count,
            h.full_length,
        ),
        None => output::print_search_match(msg),
    }
}

fn search_session(
    meta: &SessionMeta,
    pattern: &Regex,
    spec: &MatchSpec,
    args: &SearchArgs,
    total_matches: &mut usize,
    seen_uuids: &mut HashSet<String>,
) -> Result<bool> {
    match meta.tool {
        // Gemini has no per-message uuids, so cross-file dedup does not apply.
        Tool::Gemini => search_gemini(meta, pattern, spec, args, total_matches),
        _ => search_jsonl(meta, pattern, spec, args, total_matches, seen_uuids),
    }
}
///
/// Uses a ring buffer of `context` messages for pre-context.
/// After each match, collects up to `context` post-context messages before
/// printing the group.  Never holds more than `2*context + 1` messages in memory.
fn search_jsonl(
    meta: &SessionMeta,
    pattern: &Regex,
    spec: &MatchSpec,
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
    // tool_use id -> (tool name, one-line input summary), for tool_result attribution.
    let mut tool_map: HashMap<String, (String, String)> = HashMap::new();

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

        // Record this message's tool_use ids before matching, so a later
        // tool_result can be attributed back to the tool that produced it.
        record_tool_calls(&msg, &mut tool_map);

        // Match per selected block kind (reminders stripped from prose inside).
        let mut hit = match_message(&msg, pattern, spec, &tool_map);

        // Suppress a match already reported from a newer session file (resume/fork),
        // recording the uuid the first (newest) time it matches.
        if hit.is_some()
            && let Some(u) = uuid
            && !seen_uuids.insert(u)
        {
            hit = None;
        }
        let is_match = hit.is_some();

        if post_ctx_remaining > 0 {
            if !args.session_only {
                print_msg(&msg, hit.as_ref());
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

            print_msg(&msg, hit.as_ref());
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
    spec: &MatchSpec,
    args: &SearchArgs,
    total_matches: &mut usize,
) -> Result<bool> {
    let config = Config::load();
    let session = GeminiParser::new(&config).load(meta)?;

    let ctx_n = args.context;
    let mut pre_ctx: VecDeque<&Message> = VecDeque::with_capacity(ctx_n + 1);
    let mut post_ctx_remaining: usize = 0;
    let mut found_in_session = false;
    let mut tool_map: HashMap<String, (String, String)> = HashMap::new();

    for msg in &session.messages {
        record_tool_calls(msg, &mut tool_map);
        let hit = match_message(msg, pattern, spec, &tool_map);
        let is_match = hit.is_some();

        if post_ctx_remaining > 0 {
            if !args.session_only {
                print_msg(msg, hit.as_ref());
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

            print_msg(msg, hit.as_ref());
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
