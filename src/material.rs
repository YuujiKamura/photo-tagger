use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialRecord {
    pub file: String,
    pub scene_type: String,
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
    scene_type: Option<String>,
    objects: Option<Vec<String>>,
    board_text: Option<String>,
    other_text: Option<String>,
    notes: Option<String>,
    error: Option<String>,
}

impl MaterialRecord {
    pub fn new(file: &str) -> Self {
        Self {
            file: file.to_string(),
            scene_type: String::new(),
            objects: Vec::new(),
            board_text: String::new(),
            other_text: String::new(),
            notes: String::new(),
            error: None,
        }
    }
}

pub fn material_prompt(file: &str) -> String {
    format!(
        r#"次の画像について、写っている物体と文字情報だけを抽出せよ。推測や分類は不要。Output ONLY JSON object: {{"file":"{file}","scene_type":"overview|board_with_measure|measure_closeup","objects":["..."],"board_text":"","other_text":"","notes":""}}
対象ファイル: {file}
scene_type: 写真タイプは3択のみ (overview / board_with_measure / measure_closeup)
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
        scene_type: partial.scene_type.unwrap_or_default(),
        objects: partial.objects.unwrap_or_default(),
        board_text: partial.board_text.unwrap_or_default(),
        other_text: partial.other_text.unwrap_or_default(),
        notes: partial.notes.unwrap_or_default(),
        error: partial.error,
    })
}

pub fn append_jsonl(path: &Path, rec: &MaterialRecord) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("Failed to open {}", path.display()))?;
    let line = serde_json::to_string(rec).context("Failed to serialize material record")?;
    writeln!(file, "{line}").context("Failed to write JSONL line")?;
    Ok(())
}

pub fn read_jsonl(path: &Path) -> Result<Vec<MaterialRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let mut out = Vec::new();
    for line in data.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let rec: MaterialRecord =
            serde_json::from_str(line).context("Failed to parse JSONL line")?;
        out.push(rec);
    }
    Ok(out)
}

pub fn materialize_outputs(jsonl: &Path, out_dir: &Path) -> Result<()> {
    let records = read_jsonl(jsonl)?;

    let json_path = out_dir.join("analysis.json");
    let json = serde_json::to_string_pretty(&records)
        .context("Failed to serialize analysis.json")?;
    std::fs::write(&json_path, json)
        .with_context(|| format!("Failed to write {}", json_path.display()))?;

    let csv_path = out_dir.join("analysis.csv");
    let mut wtr = csv::Writer::from_path(&csv_path)
        .with_context(|| format!("Failed to create {}", csv_path.display()))?;

    for rec in records {
        let row = MaterialCsvRow {
            file: rec.file,
            scene_type: rec.scene_type,
            objects: rec.objects.join("; "),
            board_text: rec.board_text,
            other_text: rec.other_text,
            notes: rec.notes,
            error: rec.error.unwrap_or_default(),
        };
        wtr.serialize(row).context("Failed to write CSV row")?;
    }
    wtr.flush().context("Failed to flush CSV")?;
    Ok(())
}

fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')? + 1;
    Some(&s[start..end])
}

#[derive(Serialize)]
struct MaterialCsvRow {
    file: String,
    scene_type: String,
    objects: String,
    board_text: String,
    other_text: String,
    notes: String,
    error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_record_normalizes_missing_fields() {
        let input = r#"{"file":"a.jpg","objects":["roller"]}"#;
        let rec = parse_material_json(input).unwrap();
        assert_eq!(rec.file, "a.jpg");
        assert_eq!(rec.scene_type, "");
        assert_eq!(rec.objects, vec!["roller".to_string()]);
        assert_eq!(rec.board_text, "");
        assert_eq!(rec.other_text, "");
        assert_eq!(rec.notes, "");
    }

    #[test]
    fn materialize_outputs_from_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("analysis.jsonl");

        let rec1 = MaterialRecord::new("a.jpg");
        let rec2 = MaterialRecord {
            file: "b.jpg".into(),
            objects: vec!["roller".into()],
            ..MaterialRecord::new("b.jpg")
        };
        append_jsonl(&jsonl, &rec1).unwrap();
        append_jsonl(&jsonl, &rec2).unwrap();

        materialize_outputs(&jsonl, dir.path()).unwrap();

        assert!(dir.path().join("analysis.json").exists());
        assert!(dir.path().join("analysis.csv").exists());
    }
}
