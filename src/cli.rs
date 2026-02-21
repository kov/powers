use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "powers", about = "Cross-agent session query tool")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// List sessions across agents
    List(ListArgs),
    /// Search sessions for a pattern
    Search(SearchArgs),
    /// Show messages from a session
    Show(ShowArgs),
    /// Show session metadata
    Info(InfoArgs),
}

#[derive(clap::Args)]
pub struct ListArgs {
    /// Filter by agent tool
    #[arg(long)]
    pub tool: Option<ToolFilter>,

    /// Filter by project path (prefix match)
    #[arg(long)]
    pub project: Option<PathBuf>,

    /// Filter sessions after this date (YYYY-MM-DD)
    #[arg(long)]
    pub since: Option<String>,

    /// Maximum number of sessions to show
    #[arg(long, default_value = "50")]
    pub limit: usize,

    /// Output format
    #[arg(long, default_value = "table", value_enum)]
    pub format: OutputFormat,
}

#[derive(clap::Args)]
pub struct SearchArgs {
    /// Pattern to search for (regex)
    pub pattern: String,

    /// Filter by agent tool
    #[arg(long)]
    pub tool: Option<ToolFilter>,

    /// Filter by project path (prefix match)
    #[arg(long)]
    pub project: Option<PathBuf>,

    /// Filter sessions after this date (YYYY-MM-DD)
    #[arg(long)]
    pub since: Option<String>,

    /// Lines of context around each match
    #[arg(long, default_value = "2", short = 'C')]
    pub context: usize,

    /// Maximum number of matches to show
    #[arg(long, default_value = "100")]
    pub max_matches: usize,

    /// Case-sensitive search
    #[arg(long)]
    pub case_sensitive: bool,

    /// Output only session IDs (one per line, first match per session)
    #[arg(long)]
    pub session_only: bool,
}

#[derive(clap::Args)]
pub struct ShowArgs {
    /// Session ID or prefix (≥8 chars)
    pub session: String,

    /// Show last N messages
    #[arg(long)]
    pub last: Option<usize>,

    /// Show first N messages
    #[arg(long)]
    pub first: Option<usize>,

    /// Show messages from index N (inclusive)
    #[arg(long)]
    pub from: Option<usize>,

    /// Show messages to index N (inclusive)
    #[arg(long)]
    pub to: Option<usize>,

    /// Filter by role
    #[arg(long, default_value = "all", value_enum)]
    pub role: RoleFilter,

    /// Hide tool calls and tool results
    #[arg(long)]
    pub no_tool_calls: bool,

    /// Wrap output at this width (default: terminal width)
    #[arg(long)]
    pub width: Option<usize>,
}

#[derive(clap::Args)]
pub struct InfoArgs {
    /// Session ID or prefix (≥8 chars)
    pub session: String,
}

#[derive(ValueEnum, Clone, Debug, PartialEq)]
pub enum ToolFilter {
    Claude,
    Codex,
    Gemini,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum OutputFormat {
    Table,
    Tsv,
}

#[derive(ValueEnum, Clone, Debug, PartialEq)]
pub enum RoleFilter {
    User,
    Assistant,
    All,
}
