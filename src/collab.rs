use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamMessage {
    pub id: String,
    pub ts: DateTime<Utc>,
    pub from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    pub kind: String,
    pub body: String,
    pub project_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CursorState {
    pub offset: u64,
    pub last_msg_id: Option<String>,
    pub stream_inode: Option<u64>,
    pub stream_size_seen: u64,
}

pub fn resolve_project(project: &Option<PathBuf>) -> Result<PathBuf> {
    match project {
        Some(path) => Ok(path.clone()),
        None => std::env::current_dir().context("cannot determine current working directory"),
    }
}

pub fn project_hash(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    hex::encode(digest)
}

pub fn stream_path(powers_dir: &Path, project_hash: &str) -> PathBuf {
    powers_dir
        .join("streams")
        .join(format!("{project_hash}.jsonl"))
}

pub fn state_path(powers_dir: &Path, agent: &str, project_hash: &str) -> PathBuf {
    powers_dir
        .join("state")
        .join(agent)
        .join(format!("{project_hash}.json"))
}

pub fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

pub fn stream_inode(meta: &std::fs::Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(meta.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

pub struct FileLockGuard {
    path: PathBuf,
}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn lock_path_for_stream(stream_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.lock", stream_path.to_string_lossy()))
}

pub fn acquire_lock(lock_path: &Path) -> Result<FileLockGuard> {
    const MAX_ATTEMPTS: usize = 200;
    const WAIT_MS: u64 = 25;

    for _ in 0..MAX_ATTEMPTS {
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(lock_path)
        {
            Ok(_) => {
                return Ok(FileLockGuard {
                    path: lock_path.to_path_buf(),
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                thread::sleep(Duration::from_millis(WAIT_MS));
            }
            Err(e) => return Err(e.into()),
        }
    }

    anyhow::bail!("timed out acquiring stream lock {}", lock_path.display())
}
