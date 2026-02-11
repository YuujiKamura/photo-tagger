use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::domain::GroupRecords;

const GROUP_FILE: &str = "photo-groups.json";

pub fn is_image(p: &Path) -> bool {
    matches!(
        p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "heic")
    )
}

pub fn load_group_records(base: &Path) -> GroupRecords {
    let path = base.join(GROUP_FILE);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_group_records(base: &Path, records: &GroupRecords) -> Result<()> {
    let path = base.join(GROUP_FILE);
    let json =
        serde_json::to_string_pretty(records).context("Failed to serialize group records")?;
    std::fs::write(&path, json)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
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
