use anyhow::Result;
use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};
use cli_ai_analyzer::AnalyzeOptions;
use clap::Parser;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use std::thread;

use photo_tagger::{
    GroupRecord,
    GroupRecords,
    ActivityFrame,
    MaterialRecord,
    append_jsonl,
    classify_activity,
    classify_group_batch,
    infer_activity_with_gap,
    extract_top_keywords,
    is_e_board_only,
    infer_scene_from_objects_with_params,
    default_measure_labels,
    material_prompt,
    materialize_outputs,
    parse_material_json,
    read_jsonl,
};
use photo_tagger::fs_ops;

const BATCH_SIZE: usize = 10;
const MAX_CONCURRENT: usize = 3;
const MATERIAL_CONCURRENT_DEFAULT: usize = 5;

#[derive(Parser)]
#[command(name = "photo-tagger", version, about = "Classify and group construction photos")]
struct Cli {
    path: PathBuf,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    material: bool,
    #[arg(long)]
    include_e_board: bool,
    #[arg(long)]
    activity_folders: bool,
    #[arg(long)]
    activity_folders_auto: bool,
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long)]
    overwrite: bool,
    #[arg(long)]
    skip_existing: bool,
    #[arg(long, default_value_t = MATERIAL_CONCURRENT_DEFAULT)]
    concurrent: usize,
    #[arg(long, default_value_t = 10)]
    gap_min: i64,
    #[arg(long)]
    profile: bool,
    #[arg(long, default_value_t = 0.15)]
    scene_board_threshold: f64,
    #[arg(long, default_value_t = 0.25)]
    scene_measure_threshold: f64,
    #[arg(long)]
    scene_measure_labels: Option<PathBuf>,
}

fn fmt_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

fn safe_println(line: &str) {
    let mut out = std::io::stdout();
    if let Err(e) = writeln!(out, "{}", line) {
        if e.kind() != std::io::ErrorKind::BrokenPipe {
            let _ = writeln!(std::io::stderr(), "stdout error: {}", e);
        }
    }
}

fn parse_photo_timestamp(name: &str) -> Option<i64> {
    let stem = name.split('.').next()?;
    let mut parts = stem.split('_');
    let date = parts.next()?;
    let time = parts.next()?;

    if date.len() != 8 || time.len() != 6 {
        return None;
    }

    let y: i32 = date[0..4].parse().ok()?;
    let m: u32 = date[4..6].parse().ok()?;
    let d: u32 = date[6..8].parse().ok()?;
    let hh: u32 = time[0..2].parse().ok()?;
    let mm: u32 = time[2..4].parse().ok()?;
    let ss: u32 = time[4..6].parse().ok()?;

    let date = NaiveDate::from_ymd_opt(y, m, d)?;
    let time = NaiveTime::from_hms_opt(hh, mm, ss)?;
    let dt = date.and_time(time);
    Some(Utc.from_utc_datetime(&dt).timestamp())
}

fn make_activity_name(keywords: &[String]) -> String {
    if keywords.is_empty() {
        return "未分類".to_string();
    }
    if keywords.len() == 1 {
        return keywords[0].clone();
    }
    format!("{}_{}", keywords[0], keywords[1])
}

fn select_focus_text(board_text: &str, other_text: &str, notes: &str) -> String {
    let bt = board_text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>();
    if let Some(last) = bt.last() {
        return last.to_string();
    }
    format!("{}\n{}\n{}", board_text, other_text, notes)
}

fn activity_name_from_fields(
    fields: &std::collections::HashMap<String, String>,
) -> Option<String> {
    let meta: std::collections::HashSet<&'static str> =
        ["工事名", "工種", "測点", "年月日", "撮影", "写真"].into_iter().collect();
    let mut keys: Vec<&String> = fields
        .keys()
        .filter(|k| !k.trim().is_empty())
        .filter(|k| !meta.contains(k.as_str()))
        .filter(|k| !k.ends_with('員'))
        .collect();
    if keys.is_empty() {
        return None;
    }
    keys.retain(|k| {
        let val = fields.get(*k).map(|v| v.as_str()).unwrap_or("");
        !val.chars().any(|c| c.is_ascii_digit())
    });
    if keys.is_empty() {
        return None;
    }
    keys.sort();

    if keys.len() >= 2 {
        return Some(format!("{}_{}", keys[0], keys[1]));
    }

    let key = keys[0];
    let val = fields.get(key).map(|v| v.trim()).unwrap_or("");
    if !val.is_empty() {
        Some(format!("{}_{}", key, val))
    } else {
        Some(key.clone())
    }
}

fn focus_from_fields_or_text(row: &ActivityCsvRow) -> String {
    if !row.board_fields.trim().is_empty() && row.board_fields.trim() != "{}" {
        if let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, String>>(
            &row.board_fields,
        ) {
            if let Some(name) = activity_name_from_fields(&map) {
                return name;
            }
        }
    }
    select_focus_text(&row.board_text, &row.other_text, &row.notes)
}

fn assign_groups(records: &mut GroupRecords) {
    let mut id_to_group: HashMap<String, u32> = HashMap::new();
    let mut next_group = 1u32;

    let mut ids: Vec<String> = records
        .values()
        .map(|r| r.machine_id.clone())
        .collect();
    ids.sort();
    ids.dedup();

    for id in ids {
        id_to_group.insert(id, next_group);
        next_group += 1;
    }

    for rec in records.values_mut() {
        if let Some(&g) = id_to_group.get(&rec.machine_id) {
            rec.group = g;
        }
    }
}

fn print_group_summary(records: &GroupRecords) {
    if records.is_empty() {
        return;
    }

    let mut groups: HashMap<u32, Vec<(&String, &GroupRecord)>> = HashMap::new();
    for (fname, rec) in records {
        groups.entry(rec.group).or_default().push((fname, rec));
    }

    let mut group_nums: Vec<u32> = groups.keys().copied().collect();
    group_nums.sort();

    println!("\n--- Summary ({} machines, {} photos) ---", group_nums.len(), records.len());
    for g in group_nums {
        let members = &groups[&g];
        let machine_type = &members[0].1.machine_type;
        let machine_id = &members[0].1.machine_id;
        println!("  Group {g}: {machine_type} ({machine_id})");
        for (fname, rec) in members {
            println!("    - {fname}: {}", rec.role);
        }
    }
}

fn main() -> Result<()> {
    let total_start = Instant::now();
    let cli = Cli::parse();

    if cli.material {
        run_material_mode(&cli)?;
        return Ok(());
    }
    if cli.activity_folders {
        run_activity_folders(&cli)?;
        return Ok(());
    }
    if cli.activity_folders_auto {
        run_activity_folders_auto(&cli)?;
        return Ok(());
    }

    let mut records = fs_ops::load_group_records(&cli.path);

    let t = Instant::now();
    let images = fs_ops::collect_images_flat(&cli.path);
    let collect_dur = t.elapsed();

    if images.is_empty() {
        safe_println(&format!("No images found in {}", cli.path.display()));
        return Ok(());
    }

    let pending: Vec<_> = images
        .iter()
        .filter(|img| {
            let name = img
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();
            !records.contains_key(name.as_ref())
        })
        .cloned()
        .collect();

    let skip = images.len() - pending.len();
    if skip > 0 {
        println!("Skipping {skip} already grouped.");
    }
    if pending.is_empty() {
        println!("All {} images grouped.", images.len());
        print_group_summary(&records);
        return Ok(());
    }

    let batches: Vec<Vec<PathBuf>> = pending.chunks(BATCH_SIZE).map(|c| c.to_vec()).collect();
    let num_batches = batches.len();
    println!(
        "{} image(s) in {} batch(es) ({}枚/batch, {}並列)\n",
        pending.len(),
        num_batches,
        BATCH_SIZE,
        MAX_CONCURRENT
    );

    let classify_start = Instant::now();

    for (chunk_idx, chunk) in batches.chunks(MAX_CONCURRENT).enumerate() {
        let handles: Vec<_> = chunk
            .iter()
            .enumerate()
            .map(|(i, batch)| {
                let batch_num = chunk_idx * MAX_CONCURRENT + i + 1;
                let batch = batch.clone();
                thread::spawn(move || {
                    eprintln!(
                        "--- Batch {batch_num}/{num_batches} ({} images) ---",
                        batch.len()
                    );
                    let start = Instant::now();
                    let results = match classify_group_batch(&batch) {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("  Batch {batch_num} error: {e}");
                            Vec::new()
                        }
                    };
                    let elapsed = start.elapsed();
                    (batch_num, results, elapsed)
                })
            })
            .collect();

        for handle in handles {
            let (batch_num, results, elapsed) = handle.join().expect("batch thread panicked");

            for (fname, item) in &results {
                println!(
                    "  [B{batch_num}] {} -> {} / {} ({})",
                    fname, item.role, item.machine_type, item.machine_id
                );
                records.insert(
                    fname.clone(),
                    GroupRecord {
                        role: item.role.clone(),
                        machine_type: item.machine_type.clone(),
                        machine_id: item.machine_id.clone(),
                        group: 0,
                        has_board: item.has_board,
                        detected_text: item.detected_text.clone(),
                        description: item.description.clone(),
                    },
                );
            }

            if cli.profile {
                eprintln!("  [B{batch_num}] {}", fmt_duration(elapsed));
            }
        }
    }
    let classify_dur = classify_start.elapsed();

    assign_groups(&mut records);

    if !cli.dry_run {
        fs_ops::save_group_records(&cli.path, &records)?;
    }

    print_group_summary(&records);

    if cli.dry_run {
        println!("\n(dry-run: no files saved)");
    }

    let total_dur = total_start.elapsed();
    if cli.profile {
        println!("\n--- Profile ---");
        println!("  {:<12} {:>8}", "collect:", fmt_duration(collect_dur));
        println!("  {:<12} {:>8}", "classify:", fmt_duration(classify_dur));
        println!("  {:<12} {:>8}", "total:", fmt_duration(total_dur));
    } else {
        println!("\nCompleted in {}.", fmt_duration(total_dur));
    }

    Ok(())
}

fn run_material_mode(cli: &Cli) -> Result<()> {
    let total_start = Instant::now();
    let t = Instant::now();
    let images = fs_ops::collect_images_flat(&cli.path);
    let collect_dur = t.elapsed();

    if images.is_empty() {
        println!("No images found in {}", cli.path.display());
        return Ok(());
    }

    let out_dir = cli.out.clone().unwrap_or_else(|| cli.path.clone());
    std::fs::create_dir_all(&out_dir)?;

    let jsonl_path = out_dir.join("analysis.jsonl");
    let json_path = out_dir.join("analysis.json");
    let csv_path = out_dir.join("analysis.csv");
    let profile_path = out_dir.join("analysis.profile.jsonl");

    if cli.overwrite {
        let _ = std::fs::remove_file(&jsonl_path);
        let _ = std::fs::remove_file(&json_path);
        let _ = std::fs::remove_file(&csv_path);
        let _ = std::fs::remove_file(&profile_path);
    } else if (jsonl_path.exists() || json_path.exists() || csv_path.exists()) && !cli.skip_existing {
        anyhow::bail!(
            "analysis.* exists in {} (use --overwrite or --skip-existing)",
            out_dir.display()
        );
    }

    let mut existing = std::collections::HashSet::new();
    if cli.skip_existing {
        let records = read_jsonl(&jsonl_path)?;
        for rec in records {
            existing.insert(rec.file);
        }
    }

    let pending: Vec<_> = images
        .iter()
        .filter(|img| {
            let name = img
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
                .to_string();
            !existing.contains(&name)
        })
        .cloned()
        .collect();

    let skip = images.len() - pending.len();
    if skip > 0 {
        safe_println(&format!("Skipping {skip} already analyzed."));
    }
    if pending.is_empty() {
        safe_println(&format!("All {} images analyzed.", images.len()));
        materialize_outputs(&jsonl_path, &out_dir)?;
        return Ok(());
    }

    safe_println(&format!(
        "{} image(s) to analyze (material mode, {} parallel)",
        pending.len(),
        cli.concurrent
    ));

    let partial_json = r#"{"file":null,"scene_type":null,"objects":null,"board_text":null,"board_lines":null,"board_fields":null,"other_text":null,"notes":null}"#;

    let include_e_board = cli.include_e_board;
    let measure_labels = load_measure_labels(
        cli.scene_measure_labels.as_deref(),
        default_measure_labels(),
    )?;
    let classify_start = Instant::now();
    let mut pending_chunks: Vec<Vec<PathBuf>> = pending
        .chunks(cli.concurrent.max(1))
        .map(|c| c.to_vec())
        .collect();

    for chunk in pending_chunks.drain(..) {
        let handles: Vec<_> = chunk
            .into_iter()
            .map(|img| {
                let fname = img
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let prompt = material_prompt(&fname);
                let mut options = AnalyzeOptions::default()
                    .json()
                    .with_partial_json(partial_json);
                if cli.profile {
                    options = options.with_profile_path(&profile_path);
                }
                let measure_labels = measure_labels.clone();
                let board_threshold = cli.scene_board_threshold;
                let measure_threshold = cli.scene_measure_threshold;

                thread::spawn(move || {
                    let record = match cli_ai_analyzer::analyze(
                        &prompt,
                        &[&img],
                        options,
                    ) {
                        Ok(raw) => match parse_material_json(&raw) {
                            Ok(mut rec) => {
                                if rec.file.is_empty() {
                                    rec.file = fname.clone();
                                }
                                rec.scene_type_inferred = infer_scene_from_objects_with_params(
                                    &rec.objects,
                                    include_e_board,
                                    board_threshold,
                                    measure_threshold,
                                    &measure_labels,
                                );
                                rec.scene_board_threshold = board_threshold;
                                rec.scene_measure_threshold = measure_threshold;
                                rec.scene_measure_labels = measure_labels.clone();
                                if !include_e_board && is_e_board_only(&rec.objects) {
                                    rec.scene_type = "overview".to_string();
                                }
                                rec
                            }
                            Err(e) => MaterialRecord {
                                error: Some(format!("parse error: {e}")),
                                ..MaterialRecord::new(&fname)
                            },
                        },
                        Err(e) => MaterialRecord {
                            error: Some(format!("analyze error: {e}")),
                            ..MaterialRecord::new(&fname)
                        },
                    };
                    (fname, record)
                })
            })
            .collect();

        for handle in handles {
            let (fname, record) = handle.join().expect("worker thread panicked");
            append_jsonl(&jsonl_path, &record)?;
            safe_println(&format!("  {fname}"));
        }
    }
    let classify_dur = classify_start.elapsed();

    materialize_outputs(&jsonl_path, &out_dir)?;

    let total_dur = total_start.elapsed();
    if cli.profile {
        safe_println("\n--- Profile ---");
        safe_println(&format!("  {:<12} {:>8}", "collect:", fmt_duration(collect_dur)));
        safe_println(&format!("  {:<12} {:>8}", "classify:", fmt_duration(classify_dur)));
        safe_println(&format!("  {:<12} {:>8}", "total:", fmt_duration(total_dur)));
    } else {
        safe_println(&format!("\nCompleted in {}.", fmt_duration(total_dur)));
    }

    Ok(())
}

fn load_measure_labels(path: Option<&std::path::Path>, defaults: Vec<String>) -> Result<Vec<String>> {
    let Some(path) = path else {
        return Ok(defaults);
    };
    let data = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        out.push(line.to_string());
    }
    if out.is_empty() {
        return Ok(defaults);
    }
    Ok(out)
}

#[derive(Debug, serde::Deserialize)]
struct ActivityCsvRow {
    file: String,
    board_text: String,
    #[serde(default)]
    board_lines: String,
    #[serde(default)]
    board_fields: String,
    #[serde(default)]
    other_text: String,
    #[serde(default)]
    notes: String,
}

fn run_activity_folders(cli: &Cli) -> Result<()> {
    let base = &cli.path;
    let csv_path = base.join("analysis.csv");
    if !csv_path.exists() {
        anyhow::bail!("analysis.csv not found in {}", base.display());
    }

    let mut rdr = csv::Reader::from_path(&csv_path)?;
    let mut rows: Vec<(i64, ActivityCsvRow)> = Vec::new();
    for result in rdr.deserialize() {
        let row: ActivityCsvRow = result?;
        if let Some(ts) = parse_photo_timestamp(&row.file) {
            rows.push((ts, row));
        }
    }
    rows.sort_by_key(|(ts, _)| *ts);

    let mut prev: Option<ActivityFrame> = None;
    let mut moves: Vec<(String, String)> = Vec::new();

    for (ts, row) in rows {
        let combined = focus_from_fields_or_text(&row);
        let activity = if let Some(act) = classify_activity(&combined) {
            act.to_string()
        } else {
            let frame = ActivityFrame { activity: String::new(), ts };
            infer_activity_with_gap(prev.as_ref(), &frame, cli.gap_min)
        };

        let frame = ActivityFrame {
            activity: activity.clone(),
            ts,
        };
        prev = Some(frame);

        let src = base.join(&row.file);
        let dst_dir = base.join(&activity);
        let dst = dst_dir.join(&row.file);
        moves.push((src.to_string_lossy().to_string(), dst.to_string_lossy().to_string()));

        if !cli.dry_run {
            std::fs::create_dir_all(&dst_dir)?;
            std::fs::rename(&src, &dst)?;
        }
    }

    if cli.dry_run {
        for (src, dst) in moves {
            safe_println(&format!("MOVE {src} -> {dst}"));
        }
    }

    Ok(())
}

fn run_activity_folders_auto(cli: &Cli) -> Result<()> {
    let base = &cli.path;
    let csv_path = base.join("analysis.csv");
    if !csv_path.exists() {
        anyhow::bail!("analysis.csv not found in {}", base.display());
    }

    let mut rdr = csv::Reader::from_path(&csv_path)?;
    let mut rows: Vec<(i64, ActivityCsvRow)> = Vec::new();
    for result in rdr.deserialize() {
        let row: ActivityCsvRow = result?;
        if let Some(ts) = parse_photo_timestamp(&row.file) {
            rows.push((ts, row));
        }
    }
    rows.sort_by_key(|(ts, _)| *ts);

    let mut prev: Option<ActivityFrame> = None;
    let mut moves: Vec<(String, String)> = Vec::new();

    for (ts, row) in rows {
        let activity = auto_activity_name_from_row(&row, prev.as_ref(), cli.gap_min, ts);
        prev = Some(ActivityFrame {
            activity: activity.clone(),
            ts,
        });

        let src = base.join(&row.file);
        let dst_dir = base.join(&activity);
        let dst = dst_dir.join(&row.file);
        moves.push((src.to_string_lossy().to_string(), dst.to_string_lossy().to_string()));

        if !cli.dry_run {
            std::fs::create_dir_all(&dst_dir)?;
            std::fs::rename(&src, &dst)?;
        }
    }

    if cli.dry_run {
        for (src, dst) in moves {
            safe_println(&format!("MOVE {src} -> {dst}"));
        }
    }

    Ok(())
}

fn auto_activity_name_from_row(
    row: &ActivityCsvRow,
    prev: Option<&ActivityFrame>,
    gap_min: i64,
    ts: i64,
) -> String {
    if !row.board_fields.trim().is_empty() && row.board_fields.trim() != "{}" {
        if let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, String>>(
            &row.board_fields,
        ) {
            if let Some(name) = activity_name_from_fields(&map) {
                return name;
            }
        }
    }

    let text_for_keywords = if !row.board_lines.trim().is_empty() {
        format!(
            "{} {} {} {}",
            row.other_text,
            row.board_lines.replace(" / ", " "),
            row.board_text,
            row.notes
        )
    } else if !row.board_text.trim().is_empty() || !row.other_text.trim().is_empty() {
        format!("{} {}", row.other_text, row.board_text)
    } else {
        row.notes.clone()
    };

    if text_for_keywords.trim().is_empty() {
        let frame = ActivityFrame { activity: String::new(), ts };
        return infer_activity_with_gap(prev, &frame, gap_min);
    }

    let keywords = extract_top_keywords(&text_for_keywords, 2);
    make_activity_name(&keywords)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_timestamp_from_filename() {
        let ts = parse_photo_timestamp("20260211_235409.jpg").unwrap();
        assert!(ts > 0);
    }

    #[test]
    fn auto_name_from_keywords() {
        let name = make_activity_name(&["交通保安施設".into(), "設置状況".into()]);
        assert_eq!(name, "交通保安施設_設置状況");
    }

    #[test]
    fn select_focus_text_last_line() {
        let bt = "工事名 市道\n交通保安施設 設置状況\n";
        let text = select_focus_text(bt, "", "");
        assert_eq!(text, "交通保安施設 設置状況");
    }

    #[test]
    fn focus_from_fields_uses_known_keys() {
        let row = ActivityCsvRow {
            file: "a.jpg".into(),
            board_text: "".into(),
            board_lines: "".into(),
            board_fields: r#"{"処分状況":"社内検査"}"#.into(),
            other_text: "".into(),
            notes: "".into(),
        };
        let text = focus_from_fields_or_text(&row);
        assert_eq!(text, "処分状況_社内検査");
    }

    #[test]
    fn activity_name_from_fields_two_keys() {
        let mut map = std::collections::HashMap::new();
        map.insert("出荷指示".to_string(), "As混合物".to_string());
        map.insert("外観検査".to_string(), "".to_string());
        let name = activity_name_from_fields(&map).unwrap();
        assert_eq!(name, "出荷指示_外観検査");
    }

    #[test]
    fn activity_name_from_fields_key_and_value() {
        let mut map = std::collections::HashMap::new();
        map.insert("処分状況".to_string(), "社内検査".to_string());
        let name = activity_name_from_fields(&map).unwrap();
        assert_eq!(name, "処分状況_社内検査");
    }

    #[test]
    fn activity_name_from_fields_skips_numeric_and_person() {
        let mut map = std::collections::HashMap::new();
        map.insert("最大積載量".to_string(), "3.750 t".to_string());
        map.insert("社内検査員".to_string(), "池田 好敬".to_string());
        let name = activity_name_from_fields(&map);
        assert!(name.is_none());
    }

    #[test]
    fn auto_activity_uses_board_lines_keywords() {
        let row = ActivityCsvRow {
            file: "a.jpg".into(),
            board_text: "".into(),
            board_lines: "As混合物出荷指示 / 外観検査".into(),
            board_fields: "".into(),
            other_text: "".into(),
            notes: "".into(),
        };
        let name = auto_activity_name_from_row(&row, None, 10, 0);
        assert_eq!(name, "出荷指示_外観検査");
    }

    #[test]
    fn auto_activity_uses_other_text_keywords() {
        let row = ActivityCsvRow {
            file: "a.jpg".into(),
            board_text: "".into(),
            board_lines: "最大積載量 3.750 t".into(),
            board_fields: "".into(),
            other_text: "処分状況 社内検査".into(),
            notes: "".into(),
        };
        let name = auto_activity_name_from_row(&row, None, 10, 0);
        assert_eq!(name, "処分状況_社内検査");
    }

    #[test]
    fn load_measure_labels_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("labels.txt");
        std::fs::write(&path, "レーザー距離計\n# comment\n\nメジャー\n").unwrap();
        let defaults = vec!["メジャー".to_string()];
        let labels = load_measure_labels(Some(&path), defaults).unwrap();
        assert_eq!(labels, vec!["レーザー距離計".to_string(), "メジャー".to_string()]);
    }
}
