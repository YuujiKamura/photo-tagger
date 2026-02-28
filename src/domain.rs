use anyhow::{Context, Result};
use cli_ai_analyzer::{analyze, AnalyzeOptions, UsageMode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupCore {
    pub role: String,
    pub machine_type: String,
    pub machine_id: String,
    #[serde(default)]
    pub has_board: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detected_text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct GroupItem {
    pub file: String,
    #[serde(flatten)]
    pub core: GroupCore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupRecord {
    #[serde(flatten)]
    pub core: GroupCore,
    pub group: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<i64>,
}

pub type GroupRecords = HashMap<String, GroupRecord>;

pub fn extract_json_array(s: &str) -> Option<serde_json::Value> {
    let start = s.find('[')?;
    let end = s.rfind(']')? + 1;
    let candidate = &s[start..end];
    let val: serde_json::Value = serde_json::from_str(candidate).ok()?;
    if val.is_array() { Some(val) } else { None }
}

pub fn classify_group_batch(images: &[PathBuf], vocabulary: Option<&[String]>, usage_mode: UsageMode) -> Result<Vec<(String, GroupItem)>> {
    let names: Vec<String> = images
        .iter()
        .enumerate()
        .map(|(idx, p)| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("unknown_{}", idx))
        })
        .collect();
    let names: Vec<&str> = names.iter().map(|s| s.as_str()).collect();

    let prompt = crate::prompt::group_prompt(&names, vocabulary);
    let options = AnalyzeOptions::default().json().with_usage_mode(usage_mode);

    let raw = analyze(&prompt, images, options).context("AI analyze failed")?;

    let json_val = extract_json_array(&raw)
        .with_context(|| format!("No JSON array in: {raw}"))?;

    let items: Vec<GroupItem> =
        serde_json::from_value(json_val).context("Failed to parse group JSON")?;

    Ok(items
        .into_iter()
        .map(|g| {
            let file = g.file.clone();
            (file, g)
        })
        .collect())
}
