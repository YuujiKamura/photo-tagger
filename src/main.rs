use anyhow::{Context, Result};
use clap::Parser;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;
use std::time::{Duration, Instant};
use std::thread;

use photo_tagger::{
    ApiKeyGuard, CarrierConfig, GroupRecord, GroupRecords, VoucherType, classify_group_batch,
    collect_voucher_inputs, convert_voucher_file, extract_voucher_from_path, is_json, is_xlsx,
    write_voucher_csv, write_voucher_json, write_voucher_xlsx,
};
use photo_tagger::fs_ops;

const BATCH_SIZE: usize = 10;
const MAX_CONCURRENT: usize = 3;
const GROUP_GAP_SECS: i64 = 5 * 60;

#[derive(Parser)]
#[command(name = "photo-tagger", version, about = "Classify and group construction photos")]
struct Cli {
    path: PathBuf,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    profile: bool,
    #[arg(long, value_enum)]
    voucher_type: Option<VoucherType>,
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long)]
    convert: Option<PathBuf>,
    #[arg(long)]
    page_from: Option<u32>,
    #[arg(long)]
    page_to: Option<u32>,
    #[arg(long)]
    progress_file: Option<PathBuf>,
    /// API key従量課金モード（GEMINI_API_KEY環境変数が必要）
    #[arg(long)]
    pay_per_use: bool,
}

fn fmt_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

fn assign_groups(records: &mut GroupRecords) {
    let mut by_id: HashMap<String, Vec<String>> = HashMap::new();
    for (fname, rec) in records.iter() {
        by_id.entry(rec.core.machine_id.clone()).or_default().push(fname.clone());
    }

    let mut segment_heads: Vec<(i64, String, u32)> = Vec::new();
    let mut fname_to_tmp_group: HashMap<String, u32> = HashMap::new();
    let mut next_tmp_group = 1u32;

    for (machine_id, mut files) in by_id {
        files.sort_by(|a, b| {
            let ra = &records[a];
            let rb = &records[b];
            ra.captured_at
                .unwrap_or(i64::MAX)
                .cmp(&rb.captured_at.unwrap_or(i64::MAX))
                .then(a.cmp(b))
        });
        if files.is_empty() {
            continue;
        }

        let mut current_group = next_tmp_group;
        next_tmp_group += 1;
        let first_ts = records[&files[0]].captured_at.unwrap_or(i64::MAX);
        segment_heads.push((first_ts, machine_id.clone(), current_group));
        fname_to_tmp_group.insert(files[0].clone(), current_group);

        for pair in files.windows(2) {
            let prev = &records[&pair[0]];
            let curr = &records[&pair[1]];
            let prev_ts = prev.captured_at.unwrap_or(i64::MAX);
            let curr_ts = curr.captured_at.unwrap_or(i64::MAX);
            let gap = if prev_ts == i64::MAX || curr_ts == i64::MAX {
                0
            } else {
                (curr_ts - prev_ts).abs()
            };
            let prev_attach = has_attachment_hint(prev);
            let curr_attach = has_attachment_hint(curr);

            if gap > GROUP_GAP_SECS || prev_attach != curr_attach {
                current_group = next_tmp_group;
                next_tmp_group += 1;
                segment_heads.push((curr_ts, machine_id.clone(), current_group));
            }
            fname_to_tmp_group.insert(pair[1].clone(), current_group);
        }
    }

    segment_heads.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut compact_map: HashMap<u32, u32> = HashMap::new();
    for (idx, (_, _, tmp)) in segment_heads.iter().enumerate() {
        compact_map.insert(*tmp, (idx + 1) as u32);
    }

    for (fname, rec) in records.iter_mut() {
        if let Some(tmp) = fname_to_tmp_group.get(fname) {
            rec.group = *compact_map.get(tmp).unwrap_or(tmp);
        } else {
            rec.group = 0;
        }
    }
}

fn has_attachment_hint(rec: &GroupRecord) -> bool {
    rec.core.machine_id.contains("取付")
        || rec.core.detected_text.contains("取付")
}

fn extract_no(text: &str) -> Option<String> {
    for marker in ["No.", "No ", "NO.", "NO "] {
        if let Some(pos) = text.find(marker) {
            let rest = &text[pos + marker.len()..];
            let digits: String = rest
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                return Some(format!("No.{}", digits));
            }
        }
    }
    None
}

fn normalize_machine_id(rec: &mut GroupRecord) {
    let merged = format!("{} {}", rec.core.detected_text, rec.core.description);
    if merged.contains("取付") {
        if let Some(no) = extract_no(&merged).or_else(|| extract_no(&rec.core.machine_id)) {
            rec.core.machine_id = format!("取付道路 {}", no);
        }
    }
}

fn collect_capture_times(images: &[PathBuf]) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    for p in images {
        let fname = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if fname.is_empty() {
            continue;
        }
        let ts = std::fs::metadata(p)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        if let Some(v) = ts {
            out.insert(fname, v);
        }
    }
    out
}

fn apply_capture_times(records: &mut GroupRecords, capture_times: &HashMap<String, i64>) {
    for (fname, rec) in records.iter_mut() {
        normalize_machine_id(rec);
        if rec.captured_at.is_none() {
            if let Some(ts) = capture_times.get(fname) {
                rec.captured_at = Some(*ts);
            }
        }
    }
    propagate_attachment_by_time(records);
}

fn propagate_attachment_by_time(records: &mut GroupRecords) {
    let mut by_no: HashMap<String, Vec<String>> = HashMap::new();
    for (fname, rec) in records.iter() {
        if let Some(no) = extract_no(&rec.core.machine_id)
            .or_else(|| extract_no(&rec.core.detected_text))
            .or_else(|| extract_no(&rec.core.description))
        {
            by_no.entry(no).or_default().push(fname.clone());
        }
    }

    for (no, mut files) in by_no {
        files.sort_by(|a, b| {
            let ra = &records[a];
            let rb = &records[b];
            ra.captured_at
                .unwrap_or(i64::MAX)
                .cmp(&rb.captured_at.unwrap_or(i64::MAX))
                .then(a.cmp(b))
        });
        if files.is_empty() {
            continue;
        }

        let mut chunk: Vec<String> = vec![files[0].clone()];
        for pair in files.windows(2) {
            let prev = &records[&pair[0]];
            let curr = &records[&pair[1]];
            let prev_ts = prev.captured_at.unwrap_or(i64::MAX);
            let curr_ts = curr.captured_at.unwrap_or(i64::MAX);
            let gap = if prev_ts == i64::MAX || curr_ts == i64::MAX {
                0
            } else {
                (curr_ts - prev_ts).abs()
            };
            if gap > GROUP_GAP_SECS {
                apply_attach_to_chunk(records, &chunk, &no);
                chunk.clear();
            }
            chunk.push(pair[1].clone());
        }
        if !chunk.is_empty() {
            apply_attach_to_chunk(records, &chunk, &no);
        }
    }
}

fn apply_attach_to_chunk(records: &mut GroupRecords, chunk: &[String], no: &str) {
    let has_attach = chunk
        .iter()
        .any(|fname| records.get(fname).map(has_attachment_hint).unwrap_or(false));
    if !has_attach {
        return;
    }
    for fname in chunk {
        if let Some(rec) = records.get_mut(fname) {
            rec.core.machine_id = format!("取付道路 {}", no);
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
        let machine_type = &members[0].1.core.machine_type;
        let machine_id = &members[0].1.core.machine_id;
        println!("  Group {g}: {machine_type} ({machine_id})");
        for (fname, rec) in members {
            println!("    - {fname}: {}", rec.core.role);
        }
    }
}

fn main() -> Result<()> {
    let total_start = Instant::now();
    let cli = Cli::parse();
    let carrier = if cli.pay_per_use {
        CarrierConfig { billing: photo_ai_common::carrier::BillingMode::PayPerUse, ..Default::default() }
    } else {
        CarrierConfig::default()
    };

    // PayPerUseモード: APIキーを対話入力→暗号化保持（_guardのlifetimeでenv var管理）
    let _api_key_guard = if cli.pay_per_use {
        Some(ApiKeyGuard::prompt()?)
    } else {
        None
    };

    if let Some(input) = cli.convert.as_deref() {
        let out = cli
            .output
            .as_deref()
            .context("--convert requires --output")?;
        convert_voucher_file(input, out)?;
        println!("Converted: {} -> {}", input.display(), out.display());
        return Ok(());
    }

    if let Some(voucher_type) = cli.voucher_type {
        return run_voucher_mode(
            &cli.path,
            voucher_type,
            cli.output.as_deref(),
            cli.dry_run,
            cli.page_from,
            cli.page_to,
            cli.progress_file.as_deref(),
        );
    }

    let mut records = fs_ops::load_group_records(&cli.path);

    let t = Instant::now();
    let images = fs_ops::collect_images_flat(&cli.path);
    let capture_times = collect_capture_times(&images);
    let collect_dur = t.elapsed();

    if images.is_empty() {
        println!("No images found in {}", cli.path.display());
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
        apply_capture_times(&mut records, &capture_times);
        assign_groups(&mut records);
        if !cli.dry_run {
            fs_ops::save_group_records(&cli.path, &records)?;
        }
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
                    let results = match classify_group_batch(&batch, None, carrier) {
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

            for (fname, item) in results {
                println!(
                    "  [B{batch_num}] {} -> {} / {} ({})",
                    fname, item.core.role, item.core.machine_type, item.core.machine_id
                );
                records.insert(
                    fname,
                    GroupRecord {
                        core: item.core,
                        group: 0,
                        captured_at: None,
                    },
                );
            }

            if cli.profile {
                eprintln!("  [B{batch_num}] {}", fmt_duration(elapsed));
            }
        }
    }
    let classify_dur = classify_start.elapsed();

    apply_capture_times(&mut records, &capture_times);
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

fn run_voucher_mode(
    path: &std::path::Path,
    voucher_type: VoucherType,
    output: Option<&std::path::Path>,
    dry_run: bool,
    page_from: Option<u32>,
    page_to: Option<u32>,
    progress_file: Option<&std::path::Path>,
) -> Result<()> {
    let inputs = collect_voucher_inputs(path);
    if inputs.is_empty() {
        println!("No supported voucher files found: {}", path.display());
        return Ok(());
    }

    let mut results = Vec::new();
    let out_dir = if path.is_file() {
        path.parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf()
    } else {
        path.to_path_buf()
    };
    let cache_dir = out_dir.join(".voucher_cache");
    std::fs::create_dir_all(&cache_dir)?;
    let progress_path = progress_file
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| cache_dir.join(progress_name(voucher_type, page_from, page_to)));
    std::fs::write(&progress_path, "")?;
    println!("progress log: {}", progress_path.display());

    for input in &inputs {
        let checkpoint = cache_dir.join(checkpoint_name(input, voucher_type, page_from, page_to));
        println!(
            "start: {} (checkpoint: {})",
            input.display(),
            checkpoint.display()
        );
        let extracted = extract_voucher_from_path(
            input,
            voucher_type,
            page_from,
            page_to,
            Some(&checkpoint),
            Some(progress_path.as_path()),
        )?;
        for one in extracted {
            if let Some(page) = one.source_page {
                println!("{} [p{}] -> extracted", one.source_file, page);
            } else {
                println!("{} -> extracted", one.source_file);
            }
            results.push(one);
        }
    }

    if dry_run {
        let json = serde_json::to_string_pretty(&results)?;
        println!("\n[DRY-RUN] voucher extraction result:\n{json}");
        return Ok(());
    }

    let default_out = if path.is_file() {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("voucher");
        let range_suffix = match (page_from, page_to) {
            (Some(f), Some(t)) => format!("_p{:03}-{:03}", f, t),
            (Some(f), None) => format!("_p{:03}-end", f),
            (None, Some(t)) => format!("_p001-{:03}", t),
            (None, None) => String::new(),
        };
        let name = format!("{}_{}{}.xlsx", stem, voucher_type.as_str(), range_suffix);
        path.parent()
            .unwrap_or(std::path::Path::new("."))
            .join(name)
    } else {
        let range_suffix = match (page_from, page_to) {
            (Some(f), Some(t)) => format!("_p{:03}-{:03}", f, t),
            (Some(f), None) => format!("_p{:03}-end", f),
            (None, Some(t)) => format!("_p001-{:03}", t),
            (None, None) => String::new(),
        };
        let name = format!("voucher-extract_{}{}.xlsx", voucher_type.as_str(), range_suffix);
        path.join(name)
    };
    let out_path = output.map(|p| p.to_path_buf()).unwrap_or(default_out);
    if is_json(&out_path) {
        write_voucher_json(&out_path, &results)?;
    } else if is_xlsx(&out_path) {
        write_voucher_xlsx(&out_path, &results)?;
    } else {
        write_voucher_csv(&out_path, &results)?;
    }
    println!("Saved: {}", out_path.display());

    // 中間JSONを常に残す（書式変更時に再解析せず convert だけで済ませるため）
    let json_path = if is_json(&out_path) {
        out_path.clone()
    } else {
        out_path.with_extension("json")
    };
    if !is_json(&out_path) {
        write_voucher_json(&json_path, &results)?;
        println!("Saved intermediate JSON: {}", json_path.display());
    }
    Ok(())
}

fn progress_name(
    voucher_type: VoucherType,
    page_from: Option<u32>,
    page_to: Option<u32>,
) -> String {
    let range = match (page_from, page_to) {
        (Some(f), Some(t)) => format!("p{:03}-{:03}", f, t),
        (Some(f), None) => format!("p{:03}-end", f),
        (None, Some(t)) => format!("p001-{:03}", t),
        (None, None) => "pall".to_string(),
    };
    format!("progress_{}_{}.log", voucher_type.as_str(), range)
}

fn checkpoint_name(
    input: &std::path::Path,
    voucher_type: VoucherType,
    page_from: Option<u32>,
    page_to: Option<u32>,
) -> String {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("voucher");
    let safe_stem: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let range = match (page_from, page_to) {
        (Some(f), Some(t)) => format!("p{:03}-{:03}", f, t),
        (Some(f), None) => format!("p{:03}-end", f),
        (None, Some(t)) => format!("p001-{:03}", t),
        (None, None) => "pall".to_string(),
    };
    format!("{}_{}_{}.json", safe_stem, voucher_type.as_str(), range)
}
