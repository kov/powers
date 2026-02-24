use anyhow::{Result, bail};

use crate::cli::InfoArgs;
use crate::config::Config;
use crate::output;
use crate::parsers::{
    Parser, claude::ClaudeParser, codex::CodexParser, copilot::CopilotParser, gemini::GeminiParser,
};
use crate::session::SessionMeta;

pub fn run(args: &InfoArgs) -> Result<()> {
    let config = Config::load();
    let all_sessions = discover_all(&config)?;
    let meta = resolve_session_meta(&args.session, &all_sessions)?;
    output::print_info(meta);

    Ok(())
}

pub(crate) fn resolve_session_meta<'a>(
    prefix: &str,
    all_sessions: &'a [SessionMeta],
) -> Result<&'a SessionMeta> {
    if prefix.len() < 8 {
        bail!("Session prefix must be at least 8 characters");
    }

    let matches: Vec<_> = all_sessions
        .iter()
        .filter(|s| s.matches_prefix(prefix))
        .collect();

    match matches.len() {
        0 => bail!("No session found matching prefix '{}'", prefix),
        1 => Ok(matches[0]),
        _ => {
            output::print_warn(&format!(
                "Ambiguous prefix '{}' matches {} sessions:",
                prefix,
                matches.len()
            ));
            for m in &matches {
                eprintln!("  {}", m.id);
            }
            bail!("Provide a longer prefix");
        }
    }
}

pub fn discover_all(config: &Config) -> Result<Vec<crate::session::SessionMeta>> {
    let mut sessions = Vec::new();

    let claude = ClaudeParser::new(config);
    match claude.discover() {
        Ok(mut s) => sessions.append(&mut s),
        Err(e) => output::print_warn(&format!("Claude discovery failed: {e}")),
    }

    let codex = CodexParser::new(config);
    match codex.discover() {
        Ok(mut s) => sessions.append(&mut s),
        Err(e) => output::print_warn(&format!("Codex discovery failed: {e}")),
    }

    let gemini = GeminiParser::new(config);
    match gemini.discover() {
        Ok(mut s) => sessions.append(&mut s),
        Err(e) => output::print_warn(&format!("Gemini discovery failed: {e}")),
    }

    let copilot = CopilotParser::new(config);
    match copilot.discover() {
        Ok(mut s) => sessions.append(&mut s),
        Err(e) => output::print_warn(&format!("Copilot discovery failed: {e}")),
    }

    Ok(sessions)
}
