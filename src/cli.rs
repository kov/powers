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
    /// Post a collaboration message to the project stream
    Post(PostArgs),
    /// Read collaboration messages from the project stream
    Inbox(InboxArgs),
    /// Inspect collaboration stream with per-agent read status
    Log(LogArgs),
    /// Watch a live agent session, optionally with collaboration inbox updates
    Watch(WatchArgs),
    /// Show one tool result by tool use ID
    ToolResult(ToolResultArgs),
    /// Clear the collaboration stream for a project
    Clear(ClearArgs),
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

    /// Also search inside tool-call inputs (Edit diffs, Write content, Bash commands, ...).
    /// Off by default — tool-call JSON is noisy and contains escaped whitespace.
    #[arg(long)]
    pub include_tool_calls: bool,
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

    /// Expand Claude persisted tool outputs inline when available
    #[arg(long)]
    pub expand_persisted: bool,

    /// Maximum bytes to print for expanded tool output
    #[arg(long, default_value = "8192")]
    pub max_bytes: usize,

    /// Pretty-print tool call inputs multi-line (Edit diffs, full Bash commands, etc.)
    /// instead of the default single-line truncated JSON preview
    #[arg(long)]
    pub expand_tool_calls: bool,
}

#[derive(clap::Args)]
pub struct InfoArgs {
    /// Session ID or prefix (≥8 chars)
    pub session: String,
}

#[derive(clap::Args)]
pub struct PostArgs {
    /// Sender identity (for example: claude, codex, gemini)
    #[arg(long)]
    pub identity: String,

    /// Optional recipient identity. If absent, message is broadcast.
    #[arg(long)]
    pub to: Option<String>,

    /// Message kind (for example: note, status, review, handoff)
    #[arg(long, default_value = "note")]
    pub kind: String,

    /// Project path (defaults to current working directory)
    #[arg(long)]
    pub project: Option<PathBuf>,

    /// Related session ID or prefix
    #[arg(long)]
    pub session: Option<String>,

    /// Message body. If omitted, stdin is used when piped.
    #[arg(long)]
    pub message: Option<String>,
}

#[derive(clap::Args)]
pub struct InboxArgs {
    /// Reader identity (for example: claude, codex, gemini)
    #[arg(long)]
    pub identity: String,

    /// Project path (defaults to current working directory)
    #[arg(long)]
    pub project: Option<PathBuf>,

    /// Show only messages not yet consumed by this reader for this project
    #[arg(long)]
    pub unread: bool,

    /// Advance read cursor after printing messages
    #[arg(long)]
    pub mark_read: bool,

    /// Keep tailing the stream for new messages
    #[arg(long)]
    pub follow: bool,

    /// Filter by sender identity
    #[arg(long)]
    pub sender: Option<String>,

    /// Filter by kind
    #[arg(long)]
    pub kind: Option<String>,

    /// Filter by related session ID or prefix
    #[arg(long)]
    pub session: Option<String>,

    /// Only show messages at or after this date (YYYY-MM-DD)
    #[arg(long)]
    pub since: Option<String>,

    /// Show only the last N visible messages
    #[arg(long)]
    pub last: Option<usize>,

    /// Output format
    #[arg(long, default_value = "table", value_enum)]
    pub format: InboxFormat,

    /// Block until a message arrives (exit 0 with output) or timeout (exit 0, no output)
    #[arg(long)]
    pub wait: bool,

    /// Internal timeout in seconds for --wait (0 = no timeout, rely on Bash tool timeout)
    #[arg(long, default_value = "0")]
    pub timeout: u64,
}

#[derive(clap::Args)]
pub struct WatchArgs {
    /// Session ID or prefix (≥8 chars)
    pub session: String,

    /// Optional reader identity for inbox stream (for example: codex, claude, gemini)
    #[arg(long)]
    pub inbox_for: Option<String>,

    /// Project path for inbox stream (defaults to current working directory)
    #[arg(long)]
    pub project: Option<PathBuf>,

    /// Hide tool calls and tool results from session stream
    #[arg(long)]
    pub no_tool_calls: bool,

    /// Filter session messages by role
    #[arg(long, default_value = "all", value_enum)]
    pub role: RoleFilter,

    /// Poll interval baseline in milliseconds (default: 750)
    #[arg(long)]
    pub poll_ms: Option<u64>,

    /// Start from the beginning of the session rather than the current end
    #[arg(long)]
    pub from_start: bool,

    /// Mark inbox messages as read while watching
    #[arg(long)]
    pub mark_read: bool,

    /// Wrap output at this width (default: terminal width)
    #[arg(long)]
    pub width: Option<usize>,

    /// Pretty-print tool call inputs multi-line (Edit diffs, full Bash commands, etc.)
    /// instead of the default single-line truncated JSON preview
    #[arg(long)]
    pub expand_tool_calls: bool,
}

#[derive(clap::Args)]
pub struct LogArgs {
    /// Project path (defaults to current working directory)
    #[arg(long)]
    pub project: Option<PathBuf>,

    /// Show only the last N messages
    #[arg(short = 'n', long = "last", default_value = "50")]
    pub last: Option<usize>,

    /// Keep tailing the stream and refresh in-place when stdout is a TTY
    #[arg(short = 'f', long)]
    pub follow: bool,

    /// Expand full message body lines
    #[arg(long)]
    pub expand: bool,

    /// Only show messages at or after this date (YYYY-MM-DD)
    #[arg(long)]
    pub since: Option<String>,

    /// Filter by kind
    #[arg(long)]
    pub kind: Option<String>,

    /// Filter by sender identity
    #[arg(long)]
    pub sender: Option<String>,

    /// Filter by recipient identity. Use '*' for broadcast messages only.
    #[arg(long)]
    pub to: Option<String>,

    /// Wrap output at this width (default: terminal width)
    #[arg(long)]
    pub width: Option<usize>,

    /// Poll interval floor in milliseconds for --follow mode
    #[arg(long)]
    pub poll_ms: Option<u64>,

    /// Disable ANSI colors
    #[arg(long)]
    pub no_color: bool,
}

#[derive(clap::Args)]
pub struct ToolResultArgs {
    /// Tool use ID (for example: toolu_01FqFtbmmtXPTmjTrMfAuirY)
    pub tool_use_id: String,

    /// Maximum bytes to print from resolved output body
    #[arg(long, default_value = "65536")]
    pub max_bytes: usize,
}

#[derive(clap::Args)]
pub struct ClearArgs {
    /// Project path (defaults to current working directory)
    #[arg(long)]
    pub project: Option<PathBuf>,

    /// Skip confirmation prompt
    #[arg(long, short = 'y')]
    pub yes: bool,

    /// Delete only the stream file; leave per-agent cursor state intact
    #[arg(long)]
    pub keep_cursors: bool,
}

#[derive(ValueEnum, Clone, Debug, PartialEq)]
pub enum ToolFilter {
    Claude,
    Codex,
    Gemini,
    Copilot,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum OutputFormat {
    Table,
    Tsv,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum InboxFormat {
    Table,
    Json,
}

#[derive(ValueEnum, Clone, Debug, PartialEq)]
pub enum RoleFilter {
    User,
    Assistant,
    All,
}
