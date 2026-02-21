mod cli;
mod commands;
mod config;
mod output;
mod parsers;
mod session;

use clap::Parser;
use cli::{Cli, Commands};

fn main() {
    let cli = Cli::parse();

    let result = match &cli.command {
        Commands::List(args) => commands::list::run(args),
        Commands::Search(args) => commands::search::run(args),
        Commands::Show(args) => commands::show::run(args),
        Commands::Info(args) => commands::info::run(args),
    };

    if let Err(e) = result {
        output::print_error(&format!("{:#}", e));
        std::process::exit(1);
    }
}
