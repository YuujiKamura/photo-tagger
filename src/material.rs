use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialRecord {
    pub file: String,
    pub objects: Vec<String>,
    pub board_text: String,
    pub other_text: String,
    pub notes: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MaterialRecordPartial {
    file: Option<String>,
    objects: Option<Vec<String>>,
    board_text: Option<String>,
    other_text: Option<String>,
    notes: Option<String>,
    error: Option<String>,
}

pub fn material_prompt(file: &str) -> String {
    format!(
        r#"次の画像について、写っている物体と文字情報だけを抽出せよ。推測や分類は不要。Output ONLY JSON object: {{"file":"{file}","objects":["..."],"board_text":"","other_text":"","notes":""}}
対象ファイル: {file}
objects: 写っている物体の短いリスト（例: ローラー, アスファルト, 作業員, 看板）
board_text: 黒板があればその文字をそのまま
other_text: 黒板以外の文字（標識、銘板、番号など）
notes: 事実ベースの補足（任意）"#
    )
}

pub fn parse_material_json(raw: &str) -> Result<MaterialRecord> {
    let json_str = extract_json_object(raw).context("No JSON object in response")?;
    let partial: MaterialRecordPartial =
        serde_json::from_str(json_str).context("Failed to parse material JSON")?;

    Ok(MaterialRecord {
        file: partial.file.unwrap_or_default(),
        objects: partial.objects.unwrap_or_default(),
        board_text: partial.board_text.unwrap_or_default(),
        other_text: partial.other_text.unwrap_or_default(),
        notes: partial.notes.unwrap_or_default(),
        error: partial.error,
    })
}

fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')? + 1;
    Some(&s[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_record_normalizes_missing_fields() {
        let input = r#"{"file":"a.jpg","objects":["roller"]}"#;
        let rec = parse_material_json(input).unwrap();
        assert_eq!(rec.file, "a.jpg");
        assert_eq!(rec.objects, vec!["roller".to_string()]);
        assert_eq!(rec.board_text, "");
        assert_eq!(rec.other_text, "");
        assert_eq!(rec.notes, "");
    }
}
