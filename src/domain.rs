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

#[derive(Debug, Deserialize)]
pub struct GroupItem {
    pub file: String,
    pub role: String,
    pub machine_type: String,
    pub machine_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupRecord {
    pub role: String,
    pub machine_type: String,
    pub machine_id: String,
    pub group: u32,
}

pub type GroupRecords = HashMap<String, GroupRecord>;

pub fn group_prompt(filenames: &[&str]) -> String {
    let list = filenames.join(", ");
    format!(
        r#"工事現場の使用機械写真を分類せよ。各機械につき3枚1組: 機械全景/特定自主検査証票(またはナンバープレート)/排ガス対策型・低騒音型機械証票。Output ONLY JSON array: [{{"file":"filename","role":"?","machine_type":"?","machine_id":"?"}}, ...]
ファイル: {list}
role: "機械全景" or "特定自主検査証票" or "排ガス対策型・低騒音型機械証票" or "ナンバープレート"
machine_type: 機械の種類(例: タイヤローラー, アスファルトフィニッシャー, バックホウ)
machine_id: 型式番号(例: TZ-703, HA60C-2)。証票や銘板から読み取れ。同一機械の3枚は同じmachine_idにせよ。"#
    )
}

pub fn classify_group_batch(images: &[PathBuf]) -> Result<Vec<(String, GroupItem)>> {
    let names: Vec<&str> = images
        .iter()
        .map(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
        })
        .collect();

    let prompt = group_prompt(&names);
    let options = AnalyzeOptions::default().json();

    let raw = analyze(&prompt, images, options).context("AI analyze failed")?;

    let json_str = extract_json_array(&raw)
        .with_context(|| format!("No JSON array in: {raw}"))?;

    let items: Vec<GroupItem> =
        serde_json::from_str(json_str).context("Failed to parse group JSON")?;

    Ok(items
        .into_iter()
        .map(|g| {
            let file = g.file.clone();
            (file, g)
        })
        .collect())
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
