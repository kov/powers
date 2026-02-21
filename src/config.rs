use std::path::PathBuf;

pub struct Config {
    pub claude_dir: PathBuf,
    pub codex_dir: PathBuf,
    pub gemini_dir: PathBuf,
}

impl Config {
    pub fn load() -> Self {
        let home = dirs::home_dir().expect("Cannot determine home directory");

        let claude_dir = std::env::var("POWERS_CLAUDE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".claude"));

        let codex_dir = std::env::var("POWERS_CODEX_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".codex"));

        let gemini_dir = std::env::var("POWERS_GEMINI_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".gemini"));

        Config {
            claude_dir,
            codex_dir,
            gemini_dir,
        }
    }
}
