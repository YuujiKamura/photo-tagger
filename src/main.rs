mod domain;
mod fs_ops;

use anyhow::Result;
use clap::Parser;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use std::thread;

use domain::{GroupRecord, GroupRecords, Records};

const BATCH_SIZE: usize = 10;
const MAX_CONCURRENT: usize = 3;

#[derive(Parser)]
#[command(name = "photo-tagger", version, about = "Classify construction photos")]
struct Cli {
    path: PathBuf,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    profile: bool,
    #[arg(long)]
    group: bool,
}

fn fmt_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

fn print_summary(records: &Records, categories: &[String]) {
    println!("\n--- Summary ({} classified) ---", records.len());
    for label in categories {
        let count = records.values().filter(|r| r.tag == *label).count();
        if count > 0 {
            println!("  {label}: {count}");
        }
    }
}

fn run_group_mode(cli: &Cli) -> Result<()> {
    let total_start = Instant::now();
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
                    let results = match domain::classify_group_batch(&batch) {
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
                        group: 0, // assigned later
                    },
                );
            }

            if cli.profile {
                eprintln!("  [B{batch_num}] {}", fmt_duration(elapsed));
            }
        }
    }
    let classify_dur = classify_start.elapsed();

    // Assign group numbers by machine_id
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

fn assign_groups(records: &mut GroupRecords) {
    let mut id_to_group: HashMap<String, u32> = HashMap::new();
    let mut next_group = 1u32;

    // Collect unique machine_ids in stable order
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

    // Group by group number
    let mut groups: HashMap<u32, Vec<(&String, &GroupRecord)>> = HashMap::new();
    for (fname, rec) in records {
        groups.entry(rec.group).or_default().push((fname, rec));
    }

    let mut group_nums: Vec<u32> = groups.keys().copied().collect();
    group_nums.sort();

    println!("\n--- Group Summary ({} machines, {} photos) ---", group_nums.len(), records.len());
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
    let profile = cli.profile;

    if cli.group {
        return run_group_mode(&cli);
    }

    let records = Mutex::new(fs_ops::load_records(&cli.path));

    // -- discover categories from subdirectories --
    let categories = fs_ops::collect_subdirs(&cli.path);
    if categories.is_empty() {
        eprintln!("Error: no subdirectories found in {}. Create category directories first.", cli.path.display());
        return Ok(());
    }
    println!("Categories: {}", categories.join(", "));

    // -- collect phase --
    let t = Instant::now();
    let images = fs_ops::collect_images_flat(&cli.path);
    let collect_dur = t.elapsed();

    if images.is_empty() {
        println!("No images found in {}", cli.path.display());
        return Ok(());
    }

    // -- filter phase --
    let t = Instant::now();
    let pending: Vec<_> = {
        let recs = records.lock().expect("mutex poisoned");
        images
            .iter()
            .filter(|img| {
                let name = img
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default();
                !recs.contains_key(name.as_ref())
            })
            .cloned()
            .collect()
    };
    let filter_dur = t.elapsed();

    let skip = images.len() - pending.len();
    if skip > 0 {
        println!("Skipping {skip} already classified.");
    }
    if pending.is_empty() {
        println!("All {} images classified.", images.len());
        print_summary(&records.lock().expect("mutex poisoned"), &categories);
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

    let moved = Mutex::new(0usize);
    let base_path = cli.path.clone();
    let dry_run = cli.dry_run;

    // -- classify phase --
    let classify_start = Instant::now();
    let mut batch_durations: Vec<(usize, Duration)> = Vec::new();
    let mut move_dur = Duration::ZERO;

    for (chunk_idx, chunk) in batches.chunks(MAX_CONCURRENT).enumerate() {
        let handles: Vec<_> = chunk
            .iter()
            .enumerate()
            .map(|(i, batch)| {
                let batch_num = chunk_idx * MAX_CONCURRENT + i + 1;
                let batch = batch.clone();
                let cats = categories.clone();
                thread::spawn(move || {
                    eprintln!(
                        "--- Batch {batch_num}/{num_batches} ({} images) ---",
                        batch.len()
                    );
                    let start = Instant::now();
                    let results = match domain::classify_batch(&batch, &cats) {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("  Batch {batch_num} error: {e}");
                            Vec::new()
                        }
                    };
                    let elapsed = start.elapsed();
                    (batch_num, batch, results, elapsed)
                })
            })
            .collect();

        for handle in handles {
            let (batch_num, batch, results, elapsed) =
                handle.join().expect("batch thread panicked");
            batch_durations.push((batch_num, elapsed));

            for (fname, rec) in &results {
                println!(
                    "  [B{batch_num}] {} -> {} ({:.0}%)",
                    fname,
                    rec.tag,
                    rec.confidence * 100.0
                );

                // -- move phase (per file) --
                if !dry_run {
                    let move_t = Instant::now();
                    if let Some(full) = batch.iter().find(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .map_or(false, |n| n == fname)
                    }) {
                        if let Err(e) = fs_ops::move_to_tag_dir(full, &rec.tag) {
                            eprintln!("  Warning: {e}");
                        } else {
                            *moved.lock().expect("mutex poisoned") += 1;
                        }
                    }
                    move_dur += move_t.elapsed();
                }
                records
                    .lock()
                    .expect("mutex poisoned")
                    .insert(fname.clone(), rec.clone());
            }

            if let Err(e) =
                fs_ops::save_records(&base_path, &records.lock().expect("mutex poisoned"))
            {
                eprintln!("  Warning: failed to save records: {e}");
            }

            let classified = results.len();
            let failed = batch.len() - classified;
            if failed > 0 {
                println!("  [B{batch_num}] {failed} unmatched - re-run to retry.");
            }
        }
    }
    let classify_dur = classify_start.elapsed();

    print_summary(&records.lock().expect("mutex poisoned"), &categories);

    if dry_run {
        println!("\n(dry-run: no files moved)");
    } else {
        println!("\n{} file(s) moved.", moved.lock().expect("mutex poisoned"));
    }

    // -- Timing output --
    let total_dur = total_start.elapsed();

    if profile {
        let batch_detail = batch_durations
            .iter()
            .map(|(num, dur)| format!("B{num}: {}", fmt_duration(*dur)))
            .collect::<Vec<_>>()
            .join(", ");

        println!("\n--- Profile ---");
        println!("  {:<12} {:>8}", "collect:", fmt_duration(collect_dur));
        println!("  {:<12} {:>8}", "filter:", fmt_duration(filter_dur));
        if batch_detail.is_empty() {
            println!("  {:<12} {:>8}", "classify:", fmt_duration(classify_dur));
        } else {
            println!(
                "  {:<12} {:>8} ({})",
                "classify:",
                fmt_duration(classify_dur),
                batch_detail
            );
        }
        if !dry_run {
            println!("  {:<12} {:>8}", "move:", fmt_duration(move_dur));
        }
        println!("  {:<12} {:>8}", "total:", fmt_duration(total_dur));
    } else {
        println!("\nCompleted in {}.", fmt_duration(total_dur));
    }

    Ok(())
}
