use anyhow::{Context, Result};
use cli_ai_analyzer::{analyze, AnalyzeOptions};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct GroupItem {
    pub file: String,
    pub role: String,
    pub machine_type: String,
    pub machine_id: String,
    #[serde(default)]
    pub has_board: bool,
    #[serde(default)]
    pub detected_text: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupRecord {
    pub role: String,
    pub machine_type: String,
    pub machine_id: String,
    pub group: u32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_board: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detected_text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

fn is_false(v: &bool) -> bool {
    !v
}

pub type GroupRecords = HashMap<String, GroupRecord>;

pub fn group_prompt(filenames: &[&str], vocabulary: Option<&[String]>) -> String {
    let list = filenames.join(", ");
    let mut prompt = format!(
        r#"工事写真を分類・グループ分けせよ。同一対象の写真をグループにまとめろ。Output ONLY JSON array: [{{"file":"filename","role":"?","machine_type":"?","machine_id":"?","has_board":false,"detected_text":"","description":""}}, ...]
ファイル: {list}
role: 写真の役割（例: "機械全景", "特定自主検査証票", "排ガス対策型・低騒音型機械証票", "ナンバープレート", "始業前点検", "点検状況", "安全活動", "作業状況" など）
machine_type: 機械・対象の種類（例: タイヤローラー, マカダムローラー, アスファルトフィニッシャー, バックホウ）。機械でなければ活動名（例: 安全パトロール, 朝礼）
machine_id: 型式番号や識別情報。銘板・証票・黒板から読み取れ。同一対象の写真は同じ値にせよ。不明なら空文字。
has_board: 黒板が写っていればtrue
detected_text: 黒板・銘板・証票・出来形管理用紙に書かれたテキストを記録。出来形管理用紙の場合は以下のカンマ区切り形式で記録せよ: 「出来形管理用紙 No.X, 切削高(設計) V1=数値 V2=数値 V3=数値 V4=数値 V5=数値, 切削高(実施) V1=数値 V2=数値 V3=数値 V4=数値 V5=数値, 左幅員 設計X.XX 実測X.XX, 右幅員 設計X.XX 実測X.XX」
description: 写真の内容を1文で記述"#
    );
    if let Some(vocab) = vocabulary {
        if !vocab.is_empty() {
            prompt.push_str(&format!(
                "\n工事現場で使われる用語リスト（該当するものがあればこの用語を使え。なければ自由に記述せよ）:\n{}",
                vocab.join(", ")
            ));
        }
    }
    prompt
}

pub fn extract_json_array(s: &str) -> Option<&str> {
    let start = s.find('[')?;
    let end = s.rfind(']')? + 1;
    Some(&s[start..end])
}

pub fn classify_group_batch(images: &[PathBuf], vocabulary: Option<&[String]>) -> Result<Vec<(String, GroupItem)>> {
    let names: Vec<&str> = images
        .iter()
        .map(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
        })
        .collect();

    let prompt = group_prompt(&names, vocabulary);
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
