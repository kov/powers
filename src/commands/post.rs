use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::fs::OpenOptions;
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

use crate::cli::PostArgs;
use crate::collab::{
    StreamMessage, acquire_lock, ensure_parent_dir, lock_path_for_stream, project_hash, stream_path,
};
use crate::config::Config;
use crate::output;

pub fn run(args: &PostArgs) -> Result<()> {
    let config = Config::load();

    let project = resolve_project(&args.project)?;
    let project_hash = project_hash(&project);
    let stream = stream_path(&config.powers_dir, &project_hash);
    ensure_parent_dir(&stream)?;
    let lock_path = lock_path_for_stream(&stream);
    ensure_parent_dir(&lock_path)?;
    let _lock = acquire_lock(&lock_path)?;

    let body = read_message_body(args)?;
    let msg = StreamMessage {
        id: message_id(),
        ts: Utc::now(),
        from: args.from.trim().to_string(),
        to: args.to.as_ref().map(|s| s.trim().to_string()),
        kind: args.kind.trim().to_string(),
        body,
        project_hash,
        project_path: Some(project.to_string_lossy().to_string()),
        session_id: args.session.clone(),
    };

    let mut file = OpenOptions::new().append(true).create(true).open(&stream)?;
    let line = serde_json::to_string(&msg)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()?;

    output::print_post_result(&msg);
    Ok(())
}

fn resolve_project(project: &Option<PathBuf>) -> Result<PathBuf> {
    match project {
        Some(path) => Ok(path.clone()),
        None => std::env::current_dir().context("cannot determine current working directory"),
    }
}

fn read_message_body(args: &PostArgs) -> Result<String> {
    if let Some(message) = &args.message {
        let trimmed = message.trim();
        if trimmed.is_empty() {
            bail!("--message cannot be empty");
        }
        return Ok(trimmed.to_string());
    }

    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        bail!("provide --message or pipe message body on stdin");
    }

    let mut buffer = String::new();
    stdin.read_to_string(&mut buffer)?;
    let trimmed = buffer.trim();
    if trimmed.is_empty() {
        bail!("stdin message body is empty");
    }

    Ok(trimmed.to_string())
}

fn message_id() -> String {
    let now = Utc::now();
    let nanos = now
        .timestamp_nanos_opt()
        .unwrap_or(now.timestamp_micros() * 1000);
    format!("msg-{}-{}", nanos, std::process::id())
}
