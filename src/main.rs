mod domain;
mod fs_ops;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use std::thread;

use domain::Records;

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

fn main() -> Result<()> {
    let total_start = Instant::now();
    let cli = Cli::parse();
    let profile = cli.profile;

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
