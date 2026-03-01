use anyhow::{Context, Result};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct PersistedOutputRef {
    pub path: PathBuf,
    pub declared_size: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PreviewData {
    pub text: String,
    pub bytes_read: usize,
    pub truncated: bool,
    pub total_size: u64,
}

pub fn parse_persisted_output_ref(content: &str) -> Option<PersistedOutputRef> {
    if !content.contains("<persisted-output>") {
        return None;
    }

    let mut path: Option<PathBuf> = None;
    let mut declared_size: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(start) = trimmed.find("Output too large (") {
            let size_start = start + "Output too large (".len();
            if let Some(end_rel) = trimmed[size_start..].find(')') {
                let size = &trimmed[size_start..size_start + end_rel];
                if !size.is_empty() {
                    declared_size = Some(size.to_string());
                }
            }
        }
        if let Some(start) = trimmed.find("Full output saved to: ") {
            let rest = &trimmed[start + "Full output saved to: ".len()..];
            path = Some(PathBuf::from(rest.trim()));
        }
    }

    path.map(|path| PersistedOutputRef {
        path,
        declared_size,
    })
}

pub fn read_preview(path: &PathBuf, max_bytes: usize) -> Result<PreviewData> {
    let mut file = File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let total_size = file
        .metadata()
        .map(|m| m.len())
        .with_context(|| format!("Cannot stat {}", path.display()))?;

    let mut buf = vec![0_u8; max_bytes.saturating_add(1)];
    let read = file.read(&mut buf)?;
    let truncated = read > max_bytes;
    let used = if truncated { max_bytes } else { read };
    buf.truncate(used);

    Ok(PreviewData {
        text: String::from_utf8_lossy(&buf).into_owned(),
        bytes_read: used,
        truncated,
        total_size,
    })
}

pub fn extract_embedded_preview(content: &str) -> Option<String> {
    if !content.contains("<persisted-output>") {
        return None;
    }

    let mut in_preview = false;
    let mut lines = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if in_preview {
            if trimmed == "..." || trimmed == "</persisted-output>" {
                break;
            }
            lines.push(line);
            continue;
        }
        if trimmed.starts_with("Preview (first ") {
            in_preview = true;
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}
