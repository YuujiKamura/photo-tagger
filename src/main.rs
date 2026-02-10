use base64::Engine;
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const BATCH_SIZE: usize = 10;
const RECORD_FILE: &str = "photo-tags.json";
const TAGS: &[&str] = &[
    "安全訓練", "朝礼", "社外安全パトロール", "積載量確認",
    "交通保安施設配置確認", "使用機械", "重機始業前点検",
];

fn batch_prompt(filenames: &[&str]) -> String {
    let list = filenames.join(", ");
    let tags = TAGS.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" ");
    format!(
        r#"以下の工事現場写真をそれぞれ分類せよ。Output ONLY JSON array: [{{"file":"filename","tag":"?","confidence":0}}, ...]
ファイル: {list}
tag候補(必ずこの中から選べ):
{tags}
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

fn mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("heic") => "image/heic",
        _ => "image/jpeg",
    }
}

fn call_gemini_api(images: &[PathBuf], prompt: &str) -> Result<String, String> {
    let api_key = std::env::var("GEMINI_API_KEY")
        .map_err(|_| "GEMINI_API_KEY environment variable not set".to_string())?;

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent?key={}",
        api_key
    );

    let mut parts = Vec::new();
    for img in images {
        let bytes = std::fs::read(img).map_err(|e| format!("read {}: {e}", img.display()))?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        parts.push(serde_json::json!({
            "inline_data": {
                "mime_type": mime_type(img),
                "data": b64,
            }
        }));
    }
    parts.push(serde_json::json!({"text": prompt}));

    let body = serde_json::json!({
        "contents": [{"parts": parts}],
        "generationConfig": {
            "responseMimeType": "application/json"
        }
    });

    let mut last_err = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            eprintln!("  Retry {attempt}/2 after error: {last_err}");
            std::thread::sleep(std::time::Duration::from_secs(2u64 << attempt));
        }

        let resp = match ureq::post(&url).send_json(&body) {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("API request failed: {e}");
                continue;
            }
        };

        let json: serde_json::Value = match resp.into_json() {
            Ok(v) => v,
            Err(e) => {
                last_err = format!("parse response: {e}");
                continue;
            }
        };

        match json["candidates"][0]["content"]["parts"][0]["text"].as_str() {
            Some(text) => return Ok(text.to_string()),
            None => {
                last_err = format!("unexpected response structure: {json}");
                continue;
            }
        }
    }

    Err(last_err)
}

fn classify_batch(images: &[&PathBuf]) -> Vec<(String, TagRecord)> {
    let names: Vec<&str> = images
        .iter()
        .map(|p| p.file_name().unwrap().to_str().unwrap())
        .collect();

    let prompt = batch_prompt(&names);
    let paths: Vec<PathBuf> = images.iter().map(|p| p.to_path_buf()).collect();

    let raw = match call_gemini_api(&paths, &prompt) {
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

    let mut moved = 0usize;

    for (i, batch) in batches.iter().enumerate() {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let batch_refs: Vec<&PathBuf> = batch.iter().collect();
        println!("--- Batch {}/{} ({} images) ---", i + 1, num_batches, batch.len());
        let results = classify_batch(&batch_refs);

        for (fname, rec) in &results {
            println!("  {} -> {} ({:.0}%)", fname, rec.tag, rec.confidence * 100.0);
            if !cli.dry_run {
                if let Some(full) = batch.iter().find(|p| {
                    p.file_name().unwrap().to_str().unwrap() == fname
                }) {
                    let parent = full.parent().unwrap();
                    let tag_dir = parent.join(&rec.tag);
                    let _ = std::fs::create_dir_all(&tag_dir);
                    let dest = tag_dir.join(full.file_name().unwrap());
                    if std::fs::rename(full, &dest).is_ok() {
                        moved += 1;
                    }
                }
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
    } else {
        println!("\n{moved} file(s) moved.");
    }
}

fn print_summary(records: &Records) {
    println!("\n--- Summary ({} classified) ---", records.len());
    for label in TAGS {
        let count = records.values().filter(|r| r.tag == *label).count();
        if count > 0 {
            println!("  {label}: {count}");
        }
    }
}

