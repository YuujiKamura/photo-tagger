use anyhow::Result;
use clap::Parser;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;
use std::time::{Duration, Instant};
use std::thread;

use photo_tagger::{GroupRecord, GroupRecords, classify_group_batch};
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
        by_id.entry(rec.machine_id.clone()).or_default().push(fname.clone());
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
    rec.machine_id.contains("取付")
        || rec.detected_text.contains("取付")
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
    let merged = format!("{} {}", rec.detected_text, rec.description);
    if merged.contains("取付") {
        if let Some(no) = extract_no(&merged).or_else(|| extract_no(&rec.machine_id)) {
            rec.machine_id = format!("取付道路 {}", no);
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
        if let Some(no) = extract_no(&rec.machine_id)
            .or_else(|| extract_no(&rec.detected_text))
            .or_else(|| extract_no(&rec.description))
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
            rec.machine_id = format!("取付道路 {}", no);
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
                    let results = match classify_group_batch(&batch, None) {
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
