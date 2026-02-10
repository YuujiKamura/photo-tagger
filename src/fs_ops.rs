use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::domain::Records;

const RECORD_FILE: &str = "photo-tags.json";

pub fn is_image(p: &Path) -> bool {
    matches!(
        p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "heic")
    )
}

pub fn load_records(base: &Path) -> Records {
    let path = base.join(RECORD_FILE);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_records(base: &Path, records: &Records) -> Result<()> {
    let path = base.join(RECORD_FILE);
    let json = serde_json::to_string_pretty(records).context("Failed to serialize records")?;
    std::fs::write(&path, json).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// Collect subdirectory names directly under dir (non-recursive, sorted)
pub fn collect_subdirs(dir: &Path) -> Vec<String> {
    let mut dirs = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return dirs };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                dirs.push(name.to_string());
            }
        }
    }
    dirs.sort();
    dirs
}

/// Collect image files directly under dir only (NOT recursive)
pub fn collect_images_flat(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() && is_image(&p) {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// Move a file into a tag subdirectory under its parent
pub fn move_to_tag_dir(file: &Path, tag: &str) -> Result<()> {
    let parent = file.parent().context("no parent directory")?;
    let tag_dir = parent.join(tag);
    std::fs::create_dir_all(&tag_dir)?;
    let name = file.file_name().context("no filename")?;
    let dest = tag_dir.join(name);
    std::fs::rename(file, &dest).with_context(|| format!("Failed to move {} to {}", file.display(), dest.display()))?;
    Ok(())
}
