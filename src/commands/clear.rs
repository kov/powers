use anyhow::{Context, Result};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use crate::cli::ClearArgs;
use crate::collab::{
    acquire_lock, ensure_parent_dir, lock_path_for_stream, project_hash, resolve_project,
    stream_path,
};
use crate::config::Config;

pub fn run(args: &ClearArgs) -> Result<()> {
    let config = Config::load();

    let project = resolve_project(&args.project)?;
    let hash = project_hash(&project);
    let stream = stream_path(&config.powers_dir, &hash);

    if !stream.exists() {
        println!("No collaboration stream for {}.", project.display());
        return Ok(());
    }

    let cursor_files = if args.keep_cursors {
        vec![]
    } else {
        find_cursor_files(&config.powers_dir, &hash)
    };

    if !args.yes {
        println!("Will delete:");
        println!("  stream: {}", stream.display());
        for p in &cursor_files {
            println!("  cursor: {}", p.display());
        }
        print!("Proceed? [y/N] ");
        std::io::stdout().flush()?;

        if std::io::stdin().is_terminal() {
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            if !input.trim().eq_ignore_ascii_case("y") {
                println!("Aborted.");
                return Ok(());
            }
        } else {
            // Non-interactive without --yes: abort safely
            println!("Aborted (non-interactive; use --yes to confirm).");
            return Ok(());
        }
    }

    let lock_path = lock_path_for_stream(&stream);
    ensure_parent_dir(&lock_path)?;
    let _lock = acquire_lock(&lock_path)?;

    std::fs::remove_file(&stream).or_else(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(e).with_context(|| format!("failed to delete {}", stream.display()))
        }
    })?;

    let mut removed_cursors = 0usize;
    for path in &cursor_files {
        if let Err(e) = std::fs::remove_file(path) {
            eprintln!("warning: could not remove {}: {e}", path.display());
        } else {
            removed_cursors += 1;
        }
    }

    drop(_lock);
    let _ = std::fs::remove_file(&lock_path);

    println!(
        "Cleared stream for {} ({} cursor file{} removed).",
        project.display(),
        removed_cursors,
        if removed_cursors == 1 { "" } else { "s" },
    );
    Ok(())
}

fn find_cursor_files(powers_dir: &std::path::Path, hash: &str) -> Vec<PathBuf> {
    let state_dir = powers_dir.join("state");
    let filename = format!("{hash}.json");
    let mut found = vec![];
    let Ok(agents) = std::fs::read_dir(&state_dir) else {
        return found;
    };
    for entry in agents.flatten() {
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let candidate = entry.path().join(&filename);
            if candidate.exists() {
                found.push(candidate);
            }
        }
    }
    found
}
