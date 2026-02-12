use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use std::fs::OpenOptions;
use std::io::Write;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialRecord {
    pub file: String,
    pub scene_type: String,
    pub scene_type_inferred: String,
    pub objects: Vec<ObjectItem>,
    #[serde(default)]
    pub scene_board_threshold: f64,
    #[serde(default)]
    pub scene_measure_threshold: f64,
    #[serde(default)]
    pub scene_measure_labels: Vec<String>,
    #[serde(default)]
    pub scene_measure_matched_labels: Vec<String>,
    pub board_text: String,
    pub board_lines: Vec<String>,
    pub board_fields: HashMap<String, String>,
    pub other_text: String,
    pub notes: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MaterialRecordPartial {
    file: Option<String>,
    scene_type: Option<String>,
    scene_type_inferred: Option<String>,
    objects: Option<Vec<ObjectValue>>,
    scene_board_threshold: Option<f64>,
    scene_measure_threshold: Option<f64>,
    scene_measure_labels: Option<Vec<String>>,
    scene_measure_matched_labels: Option<Vec<String>>,
    board_text: Option<String>,
    board_lines: Option<Vec<String>>,
    board_fields: Option<HashMap<String, String>>,
    other_text: Option<String>,
    notes: Option<String>,
    error: Option<String>,
}

impl MaterialRecord {
    pub fn new(file: &str) -> Self {
        Self {
            file: file.to_string(),
            scene_type: String::new(),
            scene_type_inferred: String::new(),
            objects: Vec::new(),
            scene_board_threshold: 0.0,
            scene_measure_threshold: 0.0,
            scene_measure_labels: Vec::new(),
            scene_measure_matched_labels: Vec::new(),
            board_text: String::new(),
            board_lines: Vec::new(),
            board_fields: HashMap::new(),
            other_text: String::new(),
            notes: String::new(),
            error: None,
        }
    }
}

pub fn material_prompt(file: &str) -> String {
    format!(
        r#"次の画像について、写っている物体と文字情報だけを抽出せよ。推測や分類は不要。Output ONLY JSON object: {{"file":"{file}","scene_type":"overview|board_with_measure|measure_closeup","objects":[{{"label":"","bbox":{{"x":0,"y":0,"w":0,"h":0}},"area_ratio":0}}],"board_text":"","other_text":"","notes":""}}
対象ファイル: {file}
scene_type: 写真タイプは3択のみ (overview / board_with_measure / measure_closeup)
objects: 写っている主要物体を最大8件。各要素は {{label, bbox, area_ratio}}。bboxは0..1の正規化座標。
board_text: 黒板があればその文字をそのまま
board_lines: 黒板の各行を配列で返す
board_fields: 黒板のラベル(例: 工事名, 工種, 測点, 処分状況)をキーにした辞書
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
        scene_type_inferred: partial.scene_type_inferred.unwrap_or_default(),
        objects: normalize_objects(partial.objects.unwrap_or_default()),
        scene_board_threshold: partial.scene_board_threshold.unwrap_or_default(),
        scene_measure_threshold: partial.scene_measure_threshold.unwrap_or_default(),
        scene_measure_labels: partial.scene_measure_labels.unwrap_or_default(),
        scene_measure_matched_labels: partial.scene_measure_matched_labels.unwrap_or_default(),
        board_text: partial.board_text.unwrap_or_default(),
        board_lines: partial.board_lines.unwrap_or_default(),
        board_fields: partial.board_fields.unwrap_or_default(),
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
            scene_type_inferred: rec.scene_type_inferred,
            objects: rec.objects.iter().map(|o| o.label.clone()).collect::<Vec<_>>().join("; "),
            objects_json: serde_json::to_string(&rec.objects).unwrap_or_default(),
            scene_board_threshold: rec.scene_board_threshold,
            scene_measure_threshold: rec.scene_measure_threshold,
            scene_measure_labels: rec.scene_measure_labels.join("; "),
            scene_measure_matched_labels: rec.scene_measure_matched_labels.join("; "),
            board_text: rec.board_text,
            board_lines: rec.board_lines.join(" / "),
            board_fields: serde_json::to_string(&rec.board_fields).unwrap_or_default(),
            other_text: rec.other_text,
            notes: rec.notes,
            error: rec.error.unwrap_or_default(),
        };
        wtr.serialize(row).context("Failed to write CSV row")?;
    }
    wtr.flush().context("Failed to flush CSV")?;
    Ok(())
}

pub struct ActivityFrame {
    pub activity: String,
    pub ts: i64,
}

pub fn classify_activity(text: &str) -> Option<&'static str> {
    if text.contains("交通保安施設") && text.contains("設置状況") {
        return Some("交通保安施設_設置状況");
    }
    if text.contains("トラックスケール") && text.contains("計量状況") {
        return Some("トラックスケール_計量状況");
    }
    if text.contains("積載量") && text.contains("確認") {
        return Some("積載量_確認");
    }
    if text.contains("処分状況") && text.contains("社内検査") {
        return Some("処分状況_社内検査");
    }
    if text.contains("出荷指示確認") {
        return Some("出荷指示確認");
    }
    None
}

pub fn infer_activity_with_gap(
    prev: Option<&ActivityFrame>,
    curr: &ActivityFrame,
    gap_min: i64,
) -> String {
    if let Some(prev) = prev {
        if curr.ts - prev.ts < gap_min * 60 {
            return prev.activity.clone();
        }
    }
    "未分類".to_string()
}

pub fn extract_top_keywords(text: &str, k: usize) -> Vec<String> {
    let stopwords: std::collections::HashSet<&'static str> = [
        "工事名",
        "市道",
        "舗装補修工事",
        "工種",
        "測点",
        "年月日",
        "撮影",
        "写真",
    ]
    .into_iter()
    .collect();
    let allowlist: Vec<&'static str> = vec![
        "交通保安施設",
        "設置状況",
        "トラックスケール",
        "計量状況",
        "積載量",
        "確認",
        "処分状況",
        "社内検査",
        "出荷指示",
        "出荷指示確認",
        "外観検査",
    ];
    let allowset: std::collections::HashSet<&'static str> =
        allowlist.iter().copied().collect();
    let mut counts: std::collections::HashMap<String, (usize, usize)> = std::collections::HashMap::new();
    let mut order: usize = 0;

    for token in text
        .split(|c: char| c.is_whitespace() || c == ',' || c == '、' || c == '。')
        .filter(|t| !t.is_empty())
    {
        if stopwords.contains(token) {
            continue;
        }
        if token.chars().any(|c| c.is_ascii_digit() || c.is_ascii_punctuation()) {
            continue;
        }
        if allowset.contains(token) {
            let entry = counts.entry(token.to_string()).or_insert((0, order));
            entry.0 += 1;
            order += 1;
            continue;
        }

        // If token is a combined phrase, add any allowlist terms it contains.
        for term in allowlist.iter() {
            if token.contains(term) {
                let entry = counts.entry((*term).to_string()).or_insert((0, order));
                entry.0 += 1;
                order += 1;
            }
        }
    }

    let mut items: Vec<(String, usize, usize, i32)> = counts
        .into_iter()
        .map(|(t, (count, first))| {
            let mut bonus = 0;
            if t.contains("状況") {
                bonus += 3;
            } else if t.contains("検査") || t.contains("指示") {
                bonus += 2;
            } else if t.contains("確認") {
                bonus += 1;
            }
            (t, count, first, bonus)
        })
        .collect();

    items.sort_by(|a, b| {
        b.3.cmp(&a.3)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.cmp(&b.2))
    });

    items.into_iter().take(k).map(|(t, _, _, _)| t).collect()
}

pub fn is_e_board_only(objects: &[ObjectItem]) -> bool {
    let has_e = objects.iter().any(|o| o.label.contains("電子小黒板") || o.label.contains("電子黒板"));
    if !has_e {
        return false;
    }
    let has_physical = objects.iter().any(|o| {
        (o.label.contains("黒板") && !o.label.contains("電子")) ||
            o.label.contains("ホワイトボード") ||
            o.label.contains("工事用黒板") ||
            o.label.contains("手書きボード")
    });
    !has_physical
}

fn is_board_label(label: &str, include_e_board: bool) -> bool {
    if label.contains("黒板")
        || label.contains("ホワイトボード")
        || label.contains("工事用黒板")
        || label.contains("手書きボード")
    {
        if !include_e_board && (label.contains("電子小黒板") || label.contains("電子黒板")) {
            return false;
        }
        return true;
    }
    false
}

fn is_measure_label(label: &str, measure_labels: &[String]) -> bool {
    let label_norm = normalize_for_match(label);
    if label_norm.is_empty() {
        return false;
    }
    for k in measure_labels {
        let k_norm = normalize_for_match(k);
        if k_norm.is_empty() {
            continue;
        }
        if label_norm.contains(&k_norm) {
            return true;
        }
    }
    false
}

pub fn default_measure_labels() -> Vec<String> {
    [
        "メジャー",
        "巻尺",
        "定規",
        "スケール",
        "ノギス",
        "クラックスケール",
        "厚さ",
        "ゲージ",
        "温度計",
        "計測",
        "測定",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

pub fn infer_scene_from_objects(objects: &[ObjectItem], include_e_board: bool) -> String {
    let labels = default_measure_labels();
    infer_scene_from_objects_with_params(objects, include_e_board, 0.15, 0.25, &labels)
}

pub fn infer_scene_from_objects_with_params(
    objects: &[ObjectItem],
    include_e_board: bool,
    board_threshold: f64,
    measure_threshold: f64,
    measure_labels: &[String],
) -> String {
    let mut max_board = 0.0;
    let mut max_measure = 0.0;

    for obj in objects {
        if is_board_label(&obj.label, include_e_board) {
            if obj.area_ratio > max_board {
                max_board = obj.area_ratio;
            }
        }
        if is_measure_label(&obj.label, measure_labels) {
            if obj.area_ratio > max_measure {
                max_measure = obj.area_ratio;
            }
        }
    }

    // Heuristic thresholds: closeup if measurement object dominates; otherwise board+measure.
    if max_measure >= measure_threshold {
        return "measure_closeup".to_string();
    }
    if max_board >= board_threshold || (max_board > 0.0 && max_measure > 0.0) {
        return "board_with_measure".to_string();
    }
    "overview".to_string()
}

fn normalize_for_match(s: &str) -> String {
    let nfkc = s.nfkc().collect::<String>().to_lowercase();
    nfkc.chars().filter(|c| !is_noise_char(*c)).collect()
}

fn is_noise_char(c: char) -> bool {
    if c.is_whitespace() || c.is_ascii_punctuation() {
        return true;
    }
    matches!(
        c,
        '‐' | '‑' | '‒' | '–' | '—' | '―' | '－' | 'ｰ' | 'ー' | '_' | '・' | '･' | '/' | '／'
            | '\\' | '｜' | '|' | '.' | '．' | '。' | '、' | '，' | '､' | ':' | '：' | ';' | '；'
            | '(' | ')' | '（' | '）' | '[' | ']' | '【' | '】' | '「' | '」' | '『' | '』'
            | '〈' | '〉' | '《' | '》' | '〜' | '~'
    )
}

pub fn match_measure_labels(objects: &[ObjectItem], measure_labels: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for label in measure_labels {
        let label_norm = normalize_for_match(label);
        if label_norm.is_empty() {
            continue;
        }
        let mut matched = false;
        for obj in objects {
            let obj_norm = normalize_for_match(&obj.label);
            if obj_norm.contains(&label_norm) {
                matched = true;
                break;
            }
        }
        if matched {
            out.push(label.clone());
        }
    }
    out
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
    scene_type_inferred: String,
    objects: String,
    objects_json: String,
    scene_board_threshold: f64,
    scene_measure_threshold: f64,
    scene_measure_labels: String,
    scene_measure_matched_labels: String,
    board_text: String,
    board_lines: String,
    board_fields: String,
    other_text: String,
    notes: String,
    error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ObjectBBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ObjectItem {
    pub label: String,
    #[serde(default)]
    pub bbox: ObjectBBox,
    #[serde(default)]
    pub area_ratio: f64,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ObjectValue {
    Obj(ObjectItem),
    Str(String),
}

fn normalize_objects(values: Vec<ObjectValue>) -> Vec<ObjectItem> {
    values
        .into_iter()
        .map(|v| match v {
            ObjectValue::Obj(o) => o,
            ObjectValue::Str(s) => ObjectItem {
                label: s,
                ..Default::default()
            },
        })
        .collect()
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
        assert_eq!(rec.objects.len(), 1);
        assert_eq!(rec.objects[0].label, "roller");
        assert_eq!(rec.board_text, "");
        assert!(rec.board_lines.is_empty());
        assert!(rec.board_fields.is_empty());
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
            objects: vec![ObjectItem {
                label: "roller".into(),
                ..Default::default()
            }],
            ..MaterialRecord::new("b.jpg")
        };
        append_jsonl(&jsonl, &rec1).unwrap();
        append_jsonl(&jsonl, &rec2).unwrap();

        materialize_outputs(&jsonl, dir.path()).unwrap();

        assert!(dir.path().join("analysis.json").exists());
        assert!(dir.path().join("analysis.csv").exists());
    }

    #[test]
    fn csv_includes_objects_json() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("analysis.jsonl");
        let rec = MaterialRecord::new("a.jpg");
        append_jsonl(&jsonl, &rec).unwrap();
        materialize_outputs(&jsonl, dir.path()).unwrap();
        let csv = std::fs::read_to_string(dir.path().join("analysis.csv")).unwrap();
        assert!(csv.contains("objects_json"));
    }

    #[test]
    fn classify_activity_keywords() {
        assert_eq!(
            classify_activity("交通保安施設 設置状況"),
            Some("交通保安施設_設置状況")
        );
        assert_eq!(classify_activity("積載量 確認"), Some("積載量_確認"));
        assert_eq!(classify_activity("unknown text"), None);
    }

    #[test]
    fn infer_activity_with_gap_inherits() {
        let prev = ActivityFrame {
            activity: "積載量_確認".to_string(),
            ts: 1000,
        };
        let curr = ActivityFrame {
            activity: String::new(),
            ts: 1000 + 9 * 60,
        };
        assert_eq!(infer_activity_with_gap(Some(&prev), &curr, 10), "積載量_確認");
    }

    #[test]
    fn extract_top_keywords_basic() {
        let text = "交通保安施設 設置状況 交通保安施設";
        let kws = extract_top_keywords(text, 2);
        assert_eq!(kws, vec!["設置状況", "交通保安施設"]);
    }

    #[test]
    fn extract_top_keywords_skips_stopwords() {
        let text = "工事名 市道 交通保安施設 設置状況";
        let kws = extract_top_keywords(text, 2);
        assert_eq!(kws, vec!["設置状況", "交通保安施設"]);
    }

    #[test]
    fn extract_top_keywords_combined_terms() {
        let text = "トラックスケール計量状況";
        let kws = extract_top_keywords(text, 2);
        assert_eq!(kws, vec!["計量状況", "トラックスケール"]);
    }

    #[test]
    fn extract_top_keywords_prefers_status_terms() {
        let text = "積載量 確認 処分状況 社内検査";
        let kws = extract_top_keywords(text, 2);
        assert_eq!(kws, vec!["処分状況", "社内検査"]);
    }

    #[test]
    fn is_e_board_only_detects_e_board() {
        let objs = vec![
            ObjectItem { label: "電子小黒板".to_string(), ..Default::default() },
            ObjectItem { label: "道路".to_string(), ..Default::default() },
        ];
        assert!(is_e_board_only(&objs));
        let objs2 = vec![
            ObjectItem { label: "黒板".to_string(), ..Default::default() },
            ObjectItem { label: "電子小黒板".to_string(), ..Default::default() },
        ];
        assert!(!is_e_board_only(&objs2));
    }

    #[test]
    fn parse_objects_with_bbox() {
        let input = r#"{"file":"a.jpg","objects":[{"label":"看板","bbox":{"x":0.1,"y":0.2,"w":0.3,"h":0.4},"area_ratio":0.12}]}"#;
        let rec = parse_material_json(input).unwrap();
        assert_eq!(rec.objects.len(), 1);
        assert_eq!(rec.objects[0].label, "看板");
        assert_eq!(rec.objects[0].bbox.w, 0.3);
    }

    #[test]
    fn infer_scene_board_with_measure() {
        let objects = vec![
            ObjectItem { label: "工事用黒板".to_string(), area_ratio: 0.18, ..Default::default() },
            ObjectItem { label: "メジャー".to_string(), area_ratio: 0.05, ..Default::default() },
        ];
        let scene = infer_scene_from_objects(&objects, false);
        assert_eq!(scene, "board_with_measure");
    }

    #[test]
    fn infer_scene_measure_closeup() {
        let objects = vec![
            ObjectItem { label: "メジャー".to_string(), area_ratio: 0.32, ..Default::default() },
            ObjectItem { label: "舗装面".to_string(), area_ratio: 0.40, ..Default::default() },
        ];
        let scene = infer_scene_from_objects(&objects, false);
        assert_eq!(scene, "measure_closeup");
    }

    #[test]
    fn infer_scene_overview_ignores_e_board_by_default() {
        let objects = vec![
            ObjectItem { label: "電子小黒板".to_string(), area_ratio: 0.22, ..Default::default() },
            ObjectItem { label: "道路".to_string(), area_ratio: 0.50, ..Default::default() },
        ];
        let scene = infer_scene_from_objects(&objects, false);
        assert_eq!(scene, "overview");
        let scene_include = infer_scene_from_objects(&objects, true);
        assert_eq!(scene_include, "board_with_measure");
    }

    #[test]
    fn infer_scene_with_custom_thresholds() {
        let objects = vec![
            ObjectItem { label: "メジャー".to_string(), area_ratio: 0.20, ..Default::default() },
        ];
        let labels = vec!["メジャー".to_string()];
        let scene = infer_scene_from_objects_with_params(&objects, false, 0.10, 0.15, &labels);
        assert_eq!(scene, "measure_closeup");
    }

    #[test]
    fn infer_scene_with_custom_measure_labels() {
        let objects = vec![
            ObjectItem { label: "レーザー距離計".to_string(), area_ratio: 0.30, ..Default::default() },
        ];
        let labels = vec!["レーザー距離計".to_string()];
        let scene = infer_scene_from_objects_with_params(&objects, false, 0.15, 0.25, &labels);
        assert_eq!(scene, "measure_closeup");
    }

    #[test]
    fn infer_scene_with_normalized_labels() {
        let objects = vec![
            ObjectItem { label: "ﾒｼﾞｬｰ-12".to_string(), area_ratio: 0.30, ..Default::default() },
        ];
        let labels = vec!["メジャー".to_string()];
        let scene = infer_scene_from_objects_with_params(&objects, false, 0.15, 0.25, &labels);
        assert_eq!(scene, "measure_closeup");
    }

    #[test]
    fn csv_includes_scene_params() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("analysis.jsonl");
        let mut rec = MaterialRecord::new("a.jpg");
        rec.scene_board_threshold = 0.12;
        rec.scene_measure_threshold = 0.24;
        rec.scene_measure_labels = vec!["メジャー".to_string()];
        append_jsonl(&jsonl, &rec).unwrap();
        materialize_outputs(&jsonl, dir.path()).unwrap();
        let csv = std::fs::read_to_string(dir.path().join("analysis.csv")).unwrap();
        assert!(csv.contains("scene_board_threshold"));
        assert!(csv.contains("scene_measure_threshold"));
        assert!(csv.contains("scene_measure_labels"));
        assert!(csv.contains("scene_measure_matched_labels"));
    }

    #[test]
    fn measure_label_matching_reports_labels() {
        let objects = vec![
            ObjectItem { label: "ﾒｼﾞｬｰ-12".to_string(), area_ratio: 0.10, ..Default::default() },
            ObjectItem { label: "道路".to_string(), area_ratio: 0.50, ..Default::default() },
        ];
        let labels = vec!["メジャー".to_string(), "レーザー距離計".to_string()];
        let matched = match_measure_labels(&objects, &labels);
        assert_eq!(matched, vec!["メジャー".to_string()]);
    }
}
