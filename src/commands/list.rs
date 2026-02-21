use anyhow::Result;
use chrono::NaiveDate;
use std::path::PathBuf;

use crate::cli::{ListArgs, OutputFormat, ToolFilter};
use crate::config::Config;
use crate::output;
use crate::parsers::{Parser, claude::ClaudeParser, codex::CodexParser, gemini::GeminiParser};
use crate::session::SessionMeta;

pub fn run(args: &ListArgs) -> Result<()> {
    let config = Config::load();

    let mut sessions: Vec<SessionMeta> = Vec::new();

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

    if include_claude {
        let parser = ClaudeParser::new(&config);
        match parser.discover() {
            Ok(mut s) => sessions.append(&mut s),
            Err(e) => output::print_warn(&format!("Claude discovery failed: {e}")),
        }
    }

    if include_codex {
        let parser = CodexParser::new(&config);
        match parser.discover() {
            Ok(mut s) => sessions.append(&mut s),
            Err(e) => output::print_warn(&format!("Codex discovery failed: {e}")),
        }
    }

    if include_gemini {
        let parser = GeminiParser::new(&config);
        match parser.discover() {
            Ok(mut s) => sessions.append(&mut s),
            Err(e) => output::print_warn(&format!("Gemini discovery failed: {e}")),
        }
    }

    // Apply filters
    let since = args.since.as_deref().and_then(parse_date_filter);
    if let Some(project) = &args.project {
        sessions.retain(|s| project_matches(&s.project_path, project));
    }
    if let Some(since_dt) = since {
        sessions.retain(|s| s.last_activity.date_naive() >= since_dt);
    }

    // Sort by last_activity descending
    sessions.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    sessions.truncate(args.limit);

    match args.format {
        OutputFormat::Table => {
            output::print_list_header();
            for s in &sessions {
                output::print_list_row(s);
            }
        }
        OutputFormat::Tsv => {
            for s in &sessions {
                output::print_list_row_tsv(s);
            }
        }
    }

    Ok(())
}

fn parse_date_filter(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

fn project_matches(project_path: &Option<PathBuf>, filter: &PathBuf) -> bool {
    match project_path {
        None => false,
        Some(p) => p.starts_with(filter),
    }
}
