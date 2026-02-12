use anyhow::Result;
use cli_ai_analyzer::AnalyzeOptions;
use clap::Parser;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use std::thread;

use photo_tagger::{
    GroupRecord,
    GroupRecords,
    MaterialRecord,
    append_jsonl,
    classify_group_batch,
    material_prompt,
    materialize_outputs,
    parse_material_json,
    read_jsonl,
};
use photo_tagger::fs_ops;

const BATCH_SIZE: usize = 10;
const MAX_CONCURRENT: usize = 3;

#[derive(Parser)]
#[command(name = "photo-tagger", version, about = "Classify and group construction photos")]
struct Cli {
    path: PathBuf,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    material: bool,
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long)]
    overwrite: bool,
    #[arg(long)]
    skip_existing: bool,
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

    let mut records = fs_ops::load_group_records(&cli.path);

    let t = Instant::now();
    let images = fs_ops::collect_images_flat(&cli.path);
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
        println!("Skipping {skip} already analyzed.");
    }
    if pending.is_empty() {
        println!("All {} images analyzed.", images.len());
        materialize_outputs(&jsonl_path, &out_dir)?;
        return Ok(());
    }

    println!(
        "{} image(s) to analyze (material mode)",
        pending.len()
    );

    let partial_json = r#"{"file":null,"scene_type":null,"objects":null,"board_text":null,"other_text":null,"notes":null}"#;

    let classify_start = Instant::now();
    for img in pending {
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

        append_jsonl(&jsonl_path, &record)?;
        println!("  {fname}");
    }
    let classify_dur = classify_start.elapsed();

    materialize_outputs(&jsonl_path, &out_dir)?;

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
