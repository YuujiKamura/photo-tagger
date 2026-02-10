use base64::Engine;
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BATCH_SIZE: usize = 3;
const RECORD_FILE: &str = "photo-tags.json";

fn batch_prompt(filenames: &[&str]) -> String {
    let list = filenames.join(", ");
    format!(
        r#"以下の工事現場写真をそれぞれ分類せよ。Output ONLY JSON array: [{{"file":"filename","tag":"?","confidence":0}}, ...]
ファイル: {list}
tag候補(必ずこの中から選べ):
"安全訓練" "朝礼" "社外安全パトロール" "積載量確認" "交通保安施設配置確認" "使用機械" "重機始業前点検"
confidence: 0.0~1.0"#
    )
}

#[derive(Parser)]
#[command(name = "photo-tagger", version, about = "Classify construction photos")]
struct Cli {
    path: PathBuf,
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
struct BatchItem {
    file: String,
    tag: String,
    confidence: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct TagRecord {
    tag: String,
    confidence: f64,
}

type Records = HashMap<String, TagRecord>;

fn load_records(base: &Path) -> Records {
    let path = base.join(RECORD_FILE);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_records(base: &Path, records: &Records) {
    let path = base.join(RECORD_FILE);
    if let Ok(json) = serde_json::to_string_pretty(records) {
        let _ = std::fs::write(&path, json);
    }
}

fn is_image(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("jpg" | "jpeg" | "png" | "heic")
    )
}

fn collect_images(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.extend(collect_images(&p));
        } else if is_image(&p) {
            out.push(p);
        }
    }
    out.sort();
    out
}

fn extract_json_array(s: &str) -> Option<&str> {
    let start = s.find('[')?;
    let end = s.rfind(']')? + 1;
    Some(&s[start..end])
}

/// Call Gemini CLI via PowerShell, avoiding pipe deadlock by:
/// - Piping stdout only (reading in a thread)
/// - Redirecting stderr to NUL in the PS script
fn call_gemini_cli(images: &[PathBuf], prompt: &str) -> Result<String, String> {
    let tmp = std::env::temp_dir().join(format!(".photo-tagger-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);

    // Copy images to temp with neutral names
    let mut file_refs = Vec::new();
    for (i, img) in images.iter().enumerate() {
        let ext = img.extension().and_then(|e| e.to_str()).unwrap_or("jpg");
        let neutral = format!("image_{}.{}", i, ext);
        let dest = tmp.join(&neutral);
        if let Err(e) = std::fs::copy(img, &dest) {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(format!("copy failed: {e}"));
        }
        file_refs.push(format!("@{}", neutral));
    }

    let refs_str = file_refs.join(" ");
    let json_suffix = " Respond with ONLY the JSON array.";

    // Write prompt to file
    let prompt_file = tmp.join("prompt.txt");
    std::fs::write(&prompt_file, prompt).map_err(|e| format!("write prompt: {e}"))?;

    // Build PowerShell script - redirect all output to files (no pipes = no deadlock)
    let gemini_cmd = r"C:\Users\yuuji\AppData\Roaming\npm\gemini.cmd";
    let ps_script = format!(
        r#"$OutputEncoding = [Console]::OutputEncoding = [Text.Encoding]::UTF8
$prompt = Get-Content -Raw -Encoding UTF8 'prompt.txt'
("{refs} " + $prompt + "{suffix}") | & '{gemini}' -m gemini-3-flash-preview --yolo -o text 2>$null > output.txt
"#,
        refs = refs_str,
        suffix = json_suffix,
        gemini = gemini_cmd,
    );

    let script_file = tmp.join("run.ps1");
    std::fs::write(&script_file, &ps_script).map_err(|e| format!("write script: {e}"))?;

    // Run PowerShell: no pipes at all - output goes to file
    let status = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script_file)
        .current_dir(&tmp)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("spawn failed: {e}"))?;

    let output_file = tmp.join("output.txt");
    let stdout = std::fs::read_to_string(&output_file).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&tmp);

    if stdout.trim().is_empty() {
        Err(format!("empty output (exit: {})", status))
    } else {
        Ok(stdout)
    }
}

fn classify_batch(images: &[&PathBuf]) -> Vec<(String, TagRecord)> {
    let names: Vec<&str> = images
        .iter()
        .map(|p| p.file_name().unwrap().to_str().unwrap())
        .collect();

    let prompt = batch_prompt(&names);
    let paths: Vec<PathBuf> = images.iter().map(|p| p.to_path_buf()).collect();

    let raw = match call_gemini_cli(&paths, &prompt) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  Batch error: {e}");
            return Vec::new();
        }
    };

    let json_str = match extract_json_array(&raw) {
        Some(s) => s,
        None => {
            eprintln!("  No JSON array in: {raw}");
            return Vec::new();
        }
    };

    let items: Vec<BatchItem> = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("  Parse error: {e}");
            return Vec::new();
        }
    };

    items
        .into_iter()
        .map(|b| (b.file, TagRecord { tag: b.tag, confidence: b.confidence }))
        .collect()
}

fn main() {
    let cli = Cli::parse();
    let mut records = load_records(&cli.path);

    let images = collect_images(&cli.path);
    if images.is_empty() {
        println!("No images found in {}", cli.path.display());
        return;
    }

    let pending: Vec<_> = images
        .iter()
        .filter(|img| {
            let name = img.file_name().unwrap().to_string_lossy();
            !records.contains_key(name.as_ref())
        })
        .cloned()
        .collect();

    let skip = images.len() - pending.len();
    if skip > 0 {
        println!("Skipping {skip} already classified.");
    }
    if pending.is_empty() {
        println!("All {} images classified.", images.len());
        print_summary(&records);
        return;
    }

    let batches: Vec<Vec<PathBuf>> = pending
        .chunks(BATCH_SIZE)
        .map(|c| c.to_vec())
        .collect();
    let num_batches = batches.len();
    println!(
        "{} image(s) in {} batch(es) ({}枚/batch)\n",
        pending.len(),
        num_batches,
        BATCH_SIZE
    );

    let mut new_results: Vec<(PathBuf, TagRecord)> = Vec::new();

    for (i, batch) in batches.iter().enumerate() {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        let batch_refs: Vec<&PathBuf> = batch.iter().collect();
        println!("--- Batch {}/{} ({} images) ---", i + 1, num_batches, batch.len());
        let results = classify_batch(&batch_refs);

        for (fname, rec) in &results {
            println!("  {} -> {} ({:.0}%)", fname, rec.tag, rec.confidence * 100.0);
            if let Some(full) = batch.iter().find(|p| {
                p.file_name().unwrap().to_str().unwrap() == fname
            }) {
                new_results.push((full.clone(), rec.clone()));
            }
            records.insert(fname.clone(), rec.clone());
        }
        save_records(&cli.path, &records);

        let classified: usize = results.len();
        let failed = batch.len() - classified;
        if failed > 0 {
            println!("  {failed} unmatched - re-run to retry.");
        }
    }

    print_summary(&records);

    if cli.dry_run {
        println!("\n(dry-run: no files moved)");
        return;
    }

    let mut moved = 0;
    for (img, rec) in &new_results {
        let parent = img.parent().unwrap();
        let tag_dir = parent.join(&rec.tag);
        let _ = std::fs::create_dir_all(&tag_dir);
        let dest = tag_dir.join(img.file_name().unwrap());
        if std::fs::rename(img, &dest).is_ok() {
            moved += 1;
        }
    }
    println!("\n{moved} file(s) moved.");
}

fn print_summary(records: &Records) {
    println!("\n--- Summary ({} classified) ---", records.len());
    for label in &[
        "安全訓練", "朝礼", "社外安全パトロール", "積載量確認",
        "交通保安施設配置確認", "使用機械", "重機始業前点検",
    ] {
        let count = records.values().filter(|r| r.tag == *label).count();
        if count > 0 {
            println!("  {label}: {count}");
        }
    }
}

impl Clone for TagRecord {
    fn clone(&self) -> Self {
        Self { tag: self.tag.clone(), confidence: self.confidence }
    }
}
