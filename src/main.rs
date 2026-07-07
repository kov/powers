mod cli;
mod collab;
mod commands;
mod config;
mod output;
mod parsers;
mod persisted;
mod response;
mod session;
mod util;

use clap::Parser;
use cli::{Cli, Commands};

fn main() {
    let cli = Cli::parse();

    let result = match &cli.command {
        Commands::List(args) => commands::list::run(args),
        Commands::Search(args) => commands::search::run(args),
        Commands::Show(args) => commands::show::run(args),
        Commands::Info(args) => commands::info::run(args),
        Commands::Post(args) => commands::post::run(args),
        Commands::Inbox(args) => commands::inbox::run(args),
        Commands::Log(args) => commands::log::run(args),
        Commands::Watch(args) => commands::watch::run(args),
        Commands::ToolResult(args) => commands::tool_result::run(args),
        Commands::Clear(args) => commands::clear::run(args),
    };

    if let Err(e) = result {
        output::print_error(&format!("{:#}", e));
        std::process::exit(1);
    }
}
