use anyhow::{Context, Result};
use cli_ai_analyzer::{analyze, AnalyzeOptions};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

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

pub fn batch_prompt(filenames: &[&str], categories: &[String]) -> String {
    let list = filenames.join(", ");
    let cats = categories.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(" ");
    format!(
        r#"以下の工事現場写真の黒板に書かれたテキストを読み取り、最も近いカテゴリに分類せよ。Output ONLY JSON array: [{{"file":"filename","tag":"?","confidence":0}}, ...]
ファイル: {list}
カテゴリ候補(必ずこの中から選べ):
{cats}
黒板のテキストとカテゴリ名を照合し、最も一致するものを選べ。
confidence: 0.0~1.0"#
    )
}

pub fn extract_json_array(s: &str) -> Option<&str> {
    let start = s.find('[')?;
    let end = s.rfind(']')? + 1;
    Some(&s[start..end])
}

pub fn classify_batch(images: &[PathBuf], categories: &[String]) -> Result<Vec<(String, TagRecord)>> {
    let names: Vec<&str> = images
        .iter()
        .map(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
        })
        .collect();

    let prompt = batch_prompt(&names, categories);
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
