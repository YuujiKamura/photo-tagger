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

pub fn collect_images(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.extend(collect_images(&p));
        } else if is_image(&p) {
            out.push(p);
        }
    }
    out.sort();
    out
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
