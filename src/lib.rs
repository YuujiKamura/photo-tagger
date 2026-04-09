pub mod api_key;
pub mod domain;
pub mod fs_ops;
pub mod prompt;
pub mod voucher;

pub use domain::{GroupCore, GroupRecord, GroupRecords, classify_group_batch};
pub use api_key::ApiKeyGuard;
pub use cli_ai_analyzer::UsageMode;
pub use prompt::group_prompt;
pub use fs_ops::{collect_images_flat, load_group_records, save_group_records};
pub use voucher::{
    AsgaraFields, AsphaltFields, GuardmanFields, VoucherExtraction, VoucherFields, VoucherType,
    collect_voucher_inputs, convert_voucher_file, extract_voucher_from_path, is_json, is_xlsx,
    write_voucher_csv, write_voucher_json, write_voucher_xlsx,
};

use std::collections::HashMap;
use std::path::Path;
use std::time::UNIX_EPOCH;
use anyhow::Result;

fn force_reclassify_enabled() -> bool {
    std::env::var("PHOTO_TAGGER_FORCE_RECLASSIFY")
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

const GROUP_GAP_SECS: i64 = 5 * 60;

/// フォルダ内の画像をグループ分けして photo-groups.json に保存
/// 既存のグループはスキップ。戻り値は全レコード。
pub fn run_grouping(folder: &Path, batch_size: usize, vocabulary: Option<&[String]>, usage_mode: UsageMode) -> Result<GroupRecords> {
    eprintln!("[photo-tagger] run_grouping:start folder={} batch_size={}", folder.display(), batch_size);
    let mut records = load_group_records(folder);
    eprintln!("[photo-tagger] step1:loaded_records count={}", records.len());
    let images = collect_images_flat(folder);
    eprintln!("[photo-tagger] step2:collected_images count={}", images.len());
    let capture_times = collect_capture_times(&images);
    eprintln!("[photo-tagger] step3:capture_times count={}", capture_times.len());
    let force_reclassify = force_reclassify_enabled();
    eprintln!("[photo-tagger] step4:force_reclassify enabled={}", force_reclassify);

    if images.is_empty() {
        eprintln!("[photo-tagger] stop:no_images");
        return Ok(records);
    }

    let pending: Vec<_> = if force_reclassify {
        images.clone()
    } else {
        images
            .iter()
            .filter(|img| {
                let name = img.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
                !records.contains_key(name.as_ref())
            })
            .cloned()
            .collect()
    };
    eprintln!("[photo-tagger] step5:pending_images count={}", pending.len());

    if !pending.is_empty() {
        for (batch_idx, batch) in pending.chunks(batch_size).enumerate() {
            eprintln!("[photo-tagger] step6:batch_start idx={} size={}", batch_idx, batch.len());
            let results = classify_group_batch(batch, vocabulary, usage_mode)?;
            eprintln!("[photo-tagger] step7:batch_done idx={} classified={}", batch_idx, results.len());
            for (fname, item) in results {
                records.insert(fname, GroupRecord {
                    core: item.core,
                    group: 0,
                    captured_at: None,
                });
            }
        }
    }

    eprintln!("[photo-tagger] step8:apply_capture_times");
    apply_capture_times(&mut records, &capture_times);
    eprintln!("[photo-tagger] step9:assign_groups");
    assign_groups(&mut records);
    eprintln!("[photo-tagger] step10:save_group_records count={}", records.len());
    save_group_records(folder, &records)?;
    eprintln!("[photo-tagger] run_grouping:done count={}", records.len());
    Ok(records)
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

fn collect_capture_times(images: &[std::path::PathBuf]) -> HashMap<String, i64> {
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
