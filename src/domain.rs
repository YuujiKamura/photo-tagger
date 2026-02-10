use anyhow::{Context, Result};
use cli_ai_analyzer::{analyze, AnalyzeOptions};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub const TAGS: &[&str] = &[
    "安全訓練",
    "朝礼",
    "社外安全パトロール",
    "積載量確認",
    "交通保安施設配置確認",
    "使用機械",
    "重機始業前点検",
];

#[derive(Debug, Deserialize)]
pub struct BatchItem {
    pub file: String,
    pub tag: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagRecord {
    pub tag: String,
    pub confidence: f64,
}

pub type Records = HashMap<String, TagRecord>;

pub fn batch_prompt(filenames: &[&str]) -> String {
    let list = filenames.join(", ");
    let tags = TAGS
        .iter()
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        r#"以下の工事現場写真をそれぞれ分類せよ。Output ONLY JSON array: [{{"file":"filename","tag":"?","confidence":0}}, ...]
ファイル: {list}
tag候補(必ずこの中から選べ):
{tags}
confidence: 0.0~1.0"#
    )
}

pub fn extract_json_array(s: &str) -> Option<&str> {
    let start = s.find('[')?;
    let end = s.rfind(']')? + 1;
    Some(&s[start..end])
}

pub fn classify_batch(images: &[PathBuf]) -> Result<Vec<(String, TagRecord)>> {
    let names: Vec<&str> = images
        .iter()
        .map(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
        })
        .collect();

    let prompt = batch_prompt(&names);
    let options = AnalyzeOptions::default().json();

    let raw = analyze(&prompt, images, options).context("AI analyze failed")?;

    let json_str = extract_json_array(&raw)
        .with_context(|| format!("No JSON array in: {raw}"))?;

    let items: Vec<BatchItem> =
        serde_json::from_str(json_str).context("Failed to parse batch JSON")?;

    Ok(items
        .into_iter()
        .map(|b| {
            (
                b.file,
                TagRecord {
                    tag: b.tag,
                    confidence: b.confidence,
                },
            )
        })
        .collect())
}
