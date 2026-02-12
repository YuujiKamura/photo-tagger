pub mod domain;
pub mod fs_ops;
pub mod material;

pub use domain::{GroupRecord, GroupRecords, classify_group_batch, group_prompt};
pub use fs_ops::{collect_images_flat, load_group_records, save_group_records};
pub use material::{
    ActivityFrame,
    MaterialRecord,
    material_prompt,
    parse_material_json,
    append_jsonl,
    read_jsonl,
    materialize_outputs,
    classify_activity,
    infer_activity_with_gap,
    extract_top_keywords,
    is_e_board_only,
    infer_scene_from_objects,
    infer_scene_from_objects_with_params,
    infer_scene_from_objects_with_params_and_rules,
    default_measure_labels,
    default_normalize_rules,
    NormalizeRules,
    MatchMode,
    match_measure_labels,
    MatchResult,
};

use std::collections::HashMap;
use std::path::Path;
use anyhow::Result;

/// フォルダ内の画像をグループ分けして photo-groups.json に保存
/// 既存のグループはスキップ。戻り値は全レコード。
pub fn run_grouping(folder: &Path, batch_size: usize) -> Result<GroupRecords> {
    let mut records = load_group_records(folder);
    let images = collect_images_flat(folder);

    if images.is_empty() {
        return Ok(records);
    }

    let pending: Vec<_> = images
        .iter()
        .filter(|img| {
            let name = img.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
            !records.contains_key(name.as_ref())
        })
        .cloned()
        .collect();

    if pending.is_empty() {
        return Ok(records);
    }

    for batch in pending.chunks(batch_size) {
        let results = classify_group_batch(batch)?;
        for (fname, item) in results {
            records.insert(fname, GroupRecord {
                role: item.role,
                machine_type: item.machine_type,
                machine_id: item.machine_id,
                group: 0,
                has_board: item.has_board,
                detected_text: item.detected_text,
                description: item.description,
            });
        }
    }

    assign_groups(&mut records);
    save_group_records(folder, &records)?;
    Ok(records)
}

fn assign_groups(records: &mut GroupRecords) {
    let mut id_to_group: HashMap<String, u32> = HashMap::new();
    let mut next_group = 1u32;

    let mut ids: Vec<String> = records.values().map(|r| r.machine_id.clone()).collect();
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
