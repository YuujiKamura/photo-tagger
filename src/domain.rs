use anyhow::{Context, Result};
use photo_ai_common::agentapi;
use photo_ai_common::carrier::CarrierConfig;
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
    // Try each '[' position from the end, since the actual response
    // is typically after the prompt echo / TUI output.
    let bytes = s.as_bytes();
    let last_bracket = s.rfind(']')?;
    for i in (0..=last_bracket).rev() {
        if bytes[i] == b'[' {
            let candidate = &s[i..=last_bracket];
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(candidate) {
                if val.is_array() {
                    return Some(val);
                }
            }
        }
    }
    None
}

pub fn classify_group_batch(images: &[PathBuf], vocabulary: Option<&[String]>, carrier: CarrierConfig) -> Result<Vec<(String, GroupItem)>> {
    eprintln!("[photo-tagger] classify:start images={}", images.len());
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
    eprintln!("[photo-tagger] classify:built_names names={}", names.join(", "));

    let prompt = crate::prompt::group_prompt(&names, vocabulary);
    eprintln!("[photo-tagger] classify:prompt_ready chars={}", prompt.len());
    eprintln!("[photo-tagger] classify:before_analyze (AgentAPI)");

    let raw = agentapi::analyze(&prompt, images, carrier)?;
    eprintln!("[photo-tagger] classify:after_analyze chars={}", raw.len());

    let json_val = extract_json_array(&raw)
        .with_context(|| format!("No JSON array in: {raw}"))?;
    eprintln!("[photo-tagger] classify:json_extracted");

    let items: Vec<GroupItem> =
        serde_json::from_value(json_val).context("Failed to parse group JSON")?;
    eprintln!("[photo-tagger] classify:json_parsed count={}", items.len());

    Ok(items
        .into_iter()
        .map(|g| {
            let file = g.file.clone();
            (file, g)
        })
        .collect())
}
