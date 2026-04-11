use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use photo_ai_common::agentapi;
use photo_ai_common::carrier::CarrierConfig;
use encoding_rs::SHIFT_JIS;
use rust_xlsxwriter::{Color, Format, FormatAlign, Workbook};
use serde::{Deserialize, Serialize};
use serde::de::Deserializer;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum VoucherType {
    Guardman,
    Asphalt,
    Asgara,
}

impl VoucherType {
    pub fn as_str(self) -> &'static str {
        match self {
            VoucherType::Guardman => "guardman",
            VoucherType::Asphalt => "asphalt",
            VoucherType::Asgara => "asgara",
        }
    }
}

impl FromStr for VoucherType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "guardman" => Ok(VoucherType::Guardman),
            "asphalt" => Ok(VoucherType::Asphalt),
            "asgara" => Ok(VoucherType::Asgara),
            _ => bail!("Unknown voucher type: {s}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GuardmanFields {
    pub work_date: Option<String>,
    pub company_name: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub break_minutes: Option<u32>,
    pub worker_names: Vec<String>,
    pub worker_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AsphaltFields {
    pub work_date: Option<String>,
    pub work_time: Option<String>,
    pub vehicle_no: Option<String>,
    pub quantity_ton: Option<f64>,
    pub cumulative_ton: Option<f64>,
    pub departure_temp_c: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AsgaraFields {
    pub work_date: Option<String>,
    pub work_time: Option<String>,
    pub vehicle_no: Option<String>,
    pub quantity_ton: Option<f64>,
    pub cumulative_ton: Option<f64>,
    pub transport_company: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "voucher_type", rename_all = "lowercase")]
pub enum VoucherFields {
    Guardman(GuardmanFields),
    Asphalt(AsphaltFields),
    Asgara(AsgaraFields),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoucherExtraction {
    pub source_file: String,
    pub source_page: Option<u32>,
    pub fields: VoucherFields,
    pub confidence: Option<f32>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct VoucherCsvRow {
    source_file: String,
    source_page: Option<u32>,
    voucher_type: String,
    work_date: Option<String>,
    company_name: Option<String>,
    start_time: Option<String>,
    end_time: Option<String>,
    break_minutes: Option<u32>,
    worker_names: Option<String>,
    worker_count: Option<u32>,
    work_time: Option<String>,
    vehicle_no: Option<String>,
    quantity_ton: Option<f64>,
    cumulative_ton: Option<f64>,
    departure_temp_c: Option<f64>,
    transport_company: Option<String>,
    confidence: Option<f32>,
    evidence: Option<String>,
    warnings: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GuardmanAiJson {
    pub work_date: Option<String>,
    pub company_name: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    #[serde(default, deserialize_with = "de_opt_u32")]
    pub break_minutes: Option<u32>,
    #[serde(default)]
    pub worker_names: Vec<String>,
    #[serde(default, deserialize_with = "de_opt_u32")]
    pub worker_count: Option<u32>,
    #[serde(default, deserialize_with = "de_opt_f32")]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AsphaltAiJson {
    pub work_date: Option<String>,
    pub work_time: Option<String>,
    pub vehicle_no: Option<String>,
    #[serde(default, deserialize_with = "de_opt_f64")]
    pub quantity_ton: Option<f64>,
    #[serde(default, deserialize_with = "de_opt_f64")]
    pub cumulative_ton: Option<f64>,
    #[serde(default, deserialize_with = "de_opt_f64")]
    pub departure_temp_c: Option<f64>,
    #[serde(default, deserialize_with = "de_opt_f32")]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AsgaraAiJson {
    pub work_date: Option<String>,
    pub work_time: Option<String>,
    pub vehicle_no: Option<String>,
    #[serde(default, deserialize_with = "de_opt_f64")]
    pub quantity_ton: Option<f64>,
    #[serde(default, deserialize_with = "de_opt_f64")]
    pub cumulative_ton: Option<f64>,
    pub transport_company: Option<String>,
    #[serde(default, deserialize_with = "de_opt_f32")]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

pub fn collect_voucher_inputs(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_path_buf()];
    }
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(path) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() && is_supported_doc(&p) {
            out.push(p);
        }
    }
    out.sort();
    out
}

pub fn extract_voucher_from_path(
    path: &Path,
    voucher_type: VoucherType,
    page_from: Option<u32>,
    page_to: Option<u32>,
    checkpoint_path: Option<&Path>,
    progress_file: Option<&Path>,
) -> Result<Vec<VoucherExtraction>> {
    if !path.exists() {
        bail!("Input not found: {}", path.display());
    }
    let source_file = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
    let mut rows = Vec::new();
    let mut completed_by_page: BTreeMap<u32, VoucherExtraction> = BTreeMap::new();
    if let Some(cp) = checkpoint_path {
        if cp.exists() {
            if let Ok(existing) = read_voucher_json(cp) {
                for r in existing {
                    if r.source_file == source_file {
                        if let Some(p) = r.source_page {
                            completed_by_page.insert(p, r);
                        }
                    }
                }
            }
        }
    }

    if is_pdf(path) {
        let pages = pdf_page_count(path).unwrap_or(1);
        let start = page_from.unwrap_or(1).max(1).min(pages);
        let end = page_to.unwrap_or(pages).max(1).min(pages);
        if start > end {
            bail!("Invalid page range: {}..{}", start, end);
        }
        let total = end - start + 1;
        let mut done = 0u32;
        for page in start..=end {
            if let Some(existing) = completed_by_page.get(&page) {
                rows.push(existing.clone());
                done += 1;
                report_progress(
                    progress_file,
                    &format!("[resume] {} page {}/{} (p{})", source_file, done, total, page),
                );
                continue;
            }
            report_progress(
                progress_file,
                &format!("[analyze] {} page {}/{} (p{})", source_file, done + 1, total, page),
            );
            let image_path = render_pdf_page(path, page)?;
            match extract_voucher_from_image(&image_path, voucher_type, &source_file, Some(page)) {
                Ok(v) => {
                    rows.push(v.clone());
                    completed_by_page.insert(page, v);
                    done += 1;
                    report_progress(
                        progress_file,
                        &format!("[extract] {} page {}/{} (p{})", source_file, done, total, page),
                    );
                    if let Some(cp) = checkpoint_path {
                        let mut cp_rows: Vec<VoucherExtraction> =
                            completed_by_page.values().cloned().collect();
                        cp_rows.sort_by_key(|r| r.source_page.unwrap_or(0));
                        let _ = write_voucher_json(cp, &cp_rows);
                        report_progress(
                            progress_file,
                            &format!("[checkpoint] {} saved p{}", source_file, page),
                        );
                    }
                }
                Err(e) => report_progress(
                    progress_file,
                    &format!("skip page {} ({}): {}", page, source_file, e),
                ),
            }
            let _ = std::fs::remove_file(image_path);
        }
    } else {
        rows.push(extract_voucher_from_image(
            path,
            voucher_type,
            &source_file,
            None,
        )?);
    }

    if rows.is_empty() {
        bail!("No pages extracted for {}", source_file);
    }
    rows.sort_by_key(|r| r.source_page.unwrap_or(0));
    Ok(rows)
}

fn report_progress(progress_file: Option<&Path>, message: &str) {
    println!("{message}");
    let _ = std::io::stdout().flush();
    if let Some(p) = progress_file {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{message}");
            let _ = f.flush();
        }
    }
}

pub fn write_voucher_json(path: &Path, rows: &[VoucherExtraction]) -> Result<()> {
    let json = serde_json::to_string_pretty(rows)?;
    std::fs::write(path, json).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

pub fn read_voucher_json(path: &Path) -> Result<Vec<VoucherExtraction>> {
    let s = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let rows: Vec<VoucherExtraction> = serde_json::from_str(&s)
        .with_context(|| format!("Failed to parse JSON: {}", path.display()))?;
    Ok(rows)
}

pub fn write_voucher_csv(path: &Path, rows: &[VoucherExtraction]) -> Result<()> {
    let mut lines: Vec<String> = Vec::new();
    lines.push(
        [
            "source_file",
            "source_page",
            "voucher_type",
            "work_date",
            "company_name",
            "start_time",
            "end_time",
            "break_minutes",
            "worker_names",
            "worker_count",
            "work_time",
            "vehicle_no",
            "quantity_ton",
            "cumulative_ton",
            "departure_temp_c",
            "transport_company",
            "confidence",
            "evidence",
            "warnings",
        ]
        .join(","),
    );
    for r in rows {
        let row = to_csv_row(r);
        let fields = [
            row.source_file,
            row.source_page.map(|v| v.to_string()).unwrap_or_default(),
            row.voucher_type,
            row.work_date.unwrap_or_default(),
            row.company_name.unwrap_or_default(),
            row.start_time.unwrap_or_default(),
            row.end_time.unwrap_or_default(),
            row.break_minutes.map(|v| v.to_string()).unwrap_or_default(),
            row.worker_names.unwrap_or_default(),
            row.worker_count.map(|v| v.to_string()).unwrap_or_default(),
            row.work_time.unwrap_or_default(),
            row.vehicle_no.unwrap_or_default(),
            row.quantity_ton.map(|v| v.to_string()).unwrap_or_default(),
            row.cumulative_ton.map(|v| v.to_string()).unwrap_or_default(),
            row.departure_temp_c.map(|v| v.to_string()).unwrap_or_default(),
            row.transport_company.unwrap_or_default(),
            row.confidence.map(|v| v.to_string()).unwrap_or_default(),
            row.evidence.unwrap_or_default(),
            row.warnings.unwrap_or_default(),
        ];
        let escaped: Vec<String> = fields.iter().map(|f| csv_escape(f)).collect();
        lines.push(escaped.join(","));
    }
    let body = lines.join("\r\n");
    let (bytes, _, _) = SHIFT_JIS.encode(&body);
    std::fs::write(path, bytes.as_ref())
        .with_context(|| format!("Failed to write CSV: {}", path.display()))?;
    Ok(())
}

pub fn read_voucher_csv(path: &Path) -> Result<Vec<VoucherExtraction>> {
    let raw = std::fs::read(path).with_context(|| format!("Failed to read CSV: {}", path.display()))?;
    let (decoded, _, _) = SHIFT_JIS.decode(&raw);
    let mut rdr = csv::Reader::from_reader(decoded.as_bytes());
    let mut out = Vec::new();
    for row in rdr.deserialize::<VoucherCsvRow>() {
        let row = row.with_context(|| format!("Invalid CSV row: {}", path.display()))?;
        out.push(from_csv_row(row)?);
    }
    Ok(out)
}

pub fn convert_voucher_file(input: &Path, output: &Path) -> Result<()> {
    let in_fmt = detect_format(input).context("Unsupported input format. Use .json or .csv")?;
    let out_fmt = detect_format(output).context("Unsupported output format. Use .json, .csv, or .xlsx")?;
    if in_fmt == out_fmt {
        bail!("Input and output formats are the same");
    }

    let rows = if in_fmt == "json" {
        read_voucher_json(input)?
    } else if in_fmt == "csv" {
        read_voucher_csv(input)?
    } else {
        bail!("xlsx input conversion is not supported yet");
    };
    if out_fmt == "json" {
        write_voucher_json(output, &rows)
    } else if out_fmt == "csv" {
        write_voucher_csv(output, &rows)
    } else {
        write_voucher_xlsx(output, &rows)
    }
}

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

pub fn is_json(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
}

pub fn is_xlsx(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("xlsx"))
        .unwrap_or(false)
}

pub fn is_csv(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("csv"))
        .unwrap_or(false)
}

fn is_supported_doc(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("pdf" | "jpg" | "jpeg" | "png" | "heic")
    )
}

fn render_pdf_page(pdf_path: &Path, page: u32) -> Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let tmp_base = std::env::temp_dir().join(format!("denpyo_p{}_{}", page, stamp));
    let status = Command::new("pdftoppm")
        .arg("-f")
        .arg(page.to_string())
        .arg("-l")
        .arg(page.to_string())
        .arg("-singlefile")
        .arg("-png")
        .arg(pdf_path)
        .arg(&tmp_base)
        .status()
        .context("Failed to launch pdftoppm (required for PDF input)")?;
    if !status.success() {
        bail!("pdftoppm failed for {}", pdf_path.display());
    }
    let png = tmp_base.with_extension("png");
    if !png.exists() {
        bail!("First-page image was not created: {}", png.display());
    }
    Ok(png)
}

fn pdf_page_count(pdf_path: &Path) -> Option<u32> {
    let out = Command::new("pdfinfo").arg(pdf_path).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    for line in s.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Pages:") {
            if let Ok(n) = rest.trim().parse::<u32>() {
                return Some(n);
            }
        }
    }
    None
}

fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')? + 1;
    Some(&s[start..end])
}

fn voucher_prompt(voucher_type: VoucherType, filename: &str) -> String {
    match voucher_type {
        VoucherType::Guardman => format!(
            r#"画像はガードマン伝票です。1ページ目のみを見て、次のJSONオブジェクトのみを返せ。推測禁止。不明はnull。
必ずこのキーだけを出力:
{{
  "work_date": "YYYY-MM-DD or null",
  "company_name": "string or null",
  "start_time": "HH:MM or null",
  "end_time": "HH:MM or null",
  "break_minutes": "number or null",
  "worker_names": ["string", ...],
  "worker_count": "number or null",
  "confidence": "0.0-1.0",
  "evidence": ["短い根拠文字列", ...],
  "warnings": ["不確実な点", ...]
}}
対象ファイル: {filename}"#
        ),
        VoucherType::Asphalt => format!(
            r#"画像は合材伝票です。1ページ目のみを見て、次のJSONオブジェクトのみを返せ。推測禁止。不明はnull。
必ずこのキーだけを出力:
{{
  "work_date": "YYYY-MM-DD or null",
  "work_time": "HH:MM or null",
  "vehicle_no": "string or null",
  "quantity_ton": "number or null",
  "cumulative_ton": "number or null",
  "departure_temp_c": "number or null",
  "confidence": "0.0-1.0",
  "evidence": ["短い根拠文字列", ...],
  "warnings": ["不確実な点", ...]
}}
対象ファイル: {filename}"#
        ),
        VoucherType::Asgara => format!(
            r#"画像はアスガラ伝票です。1ページ目のみを見て、次のJSONオブジェクトのみを返せ。推測禁止。不明はnull。
必ずこのキーだけを出力:
{{
  "work_date": "YYYY-MM-DD or null",
  "work_time": "HH:MM or null",
  "vehicle_no": "string or null",
  "quantity_ton": "number or null",
  "cumulative_ton": "number or null",
  "transport_company": "string or null",
  "confidence": "0.0-1.0",
  "evidence": ["短い根拠文字列", ...],
  "warnings": ["不確実な点", ...]
}}
対象ファイル: {filename}"#
        ),
    }
}

fn extract_voucher_from_image(
    image_path: &Path,
    voucher_type: VoucherType,
    source_file: &str,
    source_page: Option<u32>,
) -> Result<VoucherExtraction> {
    let prompt = voucher_prompt(voucher_type, source_file);
    let raw = agentapi::analyze(&prompt, &[image_path.to_path_buf()], CarrierConfig::default()).context("AI analyze failed")?;
    let json_str = extract_json_object(&raw)
        .with_context(|| format!("No JSON object found in AI response: {raw}"))?;

    let out = match voucher_type {
        VoucherType::Guardman => {
            let r: GuardmanAiJson = serde_json::from_str(json_str).context("Failed to parse guardman JSON")?;
            VoucherExtraction {
                source_file: source_file.to_string(),
                source_page,
                fields: VoucherFields::Guardman(GuardmanFields {
                    work_date: r.work_date,
                    company_name: r.company_name,
                    start_time: r.start_time,
                    end_time: r.end_time,
                    break_minutes: r.break_minutes,
                    worker_names: r.worker_names,
                    worker_count: r.worker_count,
                }),
                confidence: r.confidence,
                evidence: r.evidence,
                warnings: r.warnings,
            }
        }
        VoucherType::Asphalt => {
            let r: AsphaltAiJson = serde_json::from_str(json_str).context("Failed to parse asphalt JSON")?;
            VoucherExtraction {
                source_file: source_file.to_string(),
                source_page,
                fields: VoucherFields::Asphalt(AsphaltFields {
                    work_date: r.work_date,
                    work_time: r.work_time,
                    vehicle_no: r.vehicle_no,
                    quantity_ton: r.quantity_ton,
                    cumulative_ton: r.cumulative_ton,
                    departure_temp_c: r.departure_temp_c,
                }),
                confidence: r.confidence,
                evidence: r.evidence,
                warnings: r.warnings,
            }
        }
        VoucherType::Asgara => {
            let r: AsgaraAiJson = serde_json::from_str(json_str).context("Failed to parse asgara JSON")?;
            VoucherExtraction {
                source_file: source_file.to_string(),
                source_page,
                fields: VoucherFields::Asgara(AsgaraFields {
                    work_date: r.work_date,
                    work_time: r.work_time,
                    vehicle_no: r.vehicle_no,
                    quantity_ton: r.quantity_ton,
                    cumulative_ton: r.cumulative_ton,
                    transport_company: r.transport_company,
                }),
                confidence: r.confidence,
                evidence: r.evidence,
                warnings: r.warnings,
            }
        }
    };
    Ok(out)
}

fn to_csv_row(v: &VoucherExtraction) -> VoucherCsvRow {
    match &v.fields {
        VoucherFields::Guardman(f) => VoucherCsvRow {
            source_file: v.source_file.clone(),
            source_page: v.source_page,
            voucher_type: VoucherType::Guardman.as_str().to_string(),
            work_date: f.work_date.clone(),
            company_name: f.company_name.clone(),
            start_time: f.start_time.clone(),
            end_time: f.end_time.clone(),
            break_minutes: f.break_minutes,
            worker_names: if f.worker_names.is_empty() { None } else { Some(f.worker_names.join(" | ")) },
            worker_count: f.worker_count,
            work_time: None,
            vehicle_no: None,
            quantity_ton: None,
            cumulative_ton: None,
            departure_temp_c: None,
            transport_company: None,
            confidence: v.confidence,
            evidence: if v.evidence.is_empty() { None } else { Some(v.evidence.join(" | ")) },
            warnings: if v.warnings.is_empty() { None } else { Some(v.warnings.join(" | ")) },
        },
        VoucherFields::Asphalt(f) => VoucherCsvRow {
            source_file: v.source_file.clone(),
            source_page: v.source_page,
            voucher_type: VoucherType::Asphalt.as_str().to_string(),
            work_date: f.work_date.clone(),
            company_name: None,
            start_time: None,
            end_time: None,
            break_minutes: None,
            worker_names: None,
            worker_count: None,
            work_time: f.work_time.clone(),
            vehicle_no: f.vehicle_no.clone(),
            quantity_ton: f.quantity_ton,
            cumulative_ton: f.cumulative_ton,
            departure_temp_c: f.departure_temp_c,
            transport_company: None,
            confidence: v.confidence,
            evidence: if v.evidence.is_empty() { None } else { Some(v.evidence.join(" | ")) },
            warnings: if v.warnings.is_empty() { None } else { Some(v.warnings.join(" | ")) },
        },
        VoucherFields::Asgara(f) => VoucherCsvRow {
            source_file: v.source_file.clone(),
            source_page: v.source_page,
            voucher_type: VoucherType::Asgara.as_str().to_string(),
            work_date: f.work_date.clone(),
            company_name: None,
            start_time: None,
            end_time: None,
            break_minutes: None,
            worker_names: None,
            worker_count: None,
            work_time: f.work_time.clone(),
            vehicle_no: f.vehicle_no.clone(),
            quantity_ton: f.quantity_ton,
            cumulative_ton: f.cumulative_ton,
            departure_temp_c: None,
            transport_company: f.transport_company.clone(),
            confidence: v.confidence,
            evidence: if v.evidence.is_empty() { None } else { Some(v.evidence.join(" | ")) },
            warnings: if v.warnings.is_empty() { None } else { Some(v.warnings.join(" | ")) },
        },
    }
}

fn from_csv_row(row: VoucherCsvRow) -> Result<VoucherExtraction> {
    let vtype = <VoucherType as FromStr>::from_str(&row.voucher_type)?;
    let evidence = split_multi(row.evidence);
    let warnings = split_multi(row.warnings);

    let fields = match vtype {
        VoucherType::Guardman => VoucherFields::Guardman(GuardmanFields {
            work_date: row.work_date,
            company_name: row.company_name,
            start_time: row.start_time,
            end_time: row.end_time,
            break_minutes: row.break_minutes,
            worker_names: split_multi(row.worker_names),
            worker_count: row.worker_count,
        }),
        VoucherType::Asphalt => VoucherFields::Asphalt(AsphaltFields {
            work_date: row.work_date,
            work_time: row.work_time,
            vehicle_no: row.vehicle_no,
            quantity_ton: row.quantity_ton,
            cumulative_ton: row.cumulative_ton,
            departure_temp_c: row.departure_temp_c,
        }),
        VoucherType::Asgara => VoucherFields::Asgara(AsgaraFields {
            work_date: row.work_date,
            work_time: row.work_time,
            vehicle_no: row.vehicle_no,
            quantity_ton: row.quantity_ton,
            cumulative_ton: row.cumulative_ton,
            transport_company: row.transport_company,
        }),
    };

    Ok(VoucherExtraction {
        source_file: row.source_file,
        source_page: row.source_page,
        fields,
        confidence: row.confidence,
        evidence,
        warnings,
    })
}

fn split_multi(s: Option<String>) -> Vec<String> {
    match s {
        Some(v) => v
            .split('|')
            .map(|x| x.trim())
            .filter(|x| !x.is_empty())
            .map(|x| x.to_string())
            .collect(),
        None => Vec::new(),
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn write_voucher_xlsx(path: &Path, rows: &[VoucherExtraction]) -> Result<()> {
    let headers = [
        "source_file",
        "source_page",
        "voucher_type",
        "work_date",
        "company_name",
        "start_time",
        "end_time",
        "break_minutes",
        "worker_names",
        "worker_count",
        "work_time",
        "vehicle_no",
        "quantity_ton",
        "cumulative_ton",
        "departure_temp_c",
        "transport_company",
        "confidence",
        "evidence",
        "warnings",
    ];

    let csv_rows: Vec<VoucherCsvRow> = rows.iter().map(to_csv_row).collect();
    let mut max_widths: Vec<usize> = headers.iter().map(|h| display_width(h)).collect();
    let mut values_rows: Vec<Vec<String>> = Vec::new();
    for row in &csv_rows {
        let values = csv_row_values(row);
        for (i, v) in values.iter().enumerate() {
            max_widths[i] = max_widths[i].max(display_width(v));
        }
        values_rows.push(values);
    }

    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();
    let header_format = Format::new()
        .set_bold()
        .set_align(FormatAlign::Center)
        .set_align(FormatAlign::VerticalCenter)
        .set_background_color(Color::RGB(0xD9E1F2));
    let wrap_format = Format::new().set_text_wrap();
    let compact_format = Format::new().set_align(FormatAlign::Center);

    for (col, header) in headers.iter().enumerate() {
        worksheet.write_string_with_format(0, col as u16, *header, &header_format)?;
    }
    for (r, values) in values_rows.iter().enumerate() {
        let row = (r + 1) as u32;
        for (c, v) in values.iter().enumerate() {
            match headers[c] {
                "source_page" | "worker_count" | "quantity_ton" | "cumulative_ton"
                | "departure_temp_c" | "confidence" => {
                    worksheet.write_string_with_format(row, c as u16, v, &compact_format)?;
                }
                "evidence" | "warnings" => {
                    worksheet.write_string_with_format(row, c as u16, v, &wrap_format)?;
                }
                _ => {
                    worksheet.write_string(row, c as u16, v)?;
                }
            }
        }
    }

    // Keep the header visible and allow filtering in large extracts.
    worksheet.set_freeze_panes(1, 0)?;
    if !values_rows.is_empty() {
        worksheet.autofilter(0, 0, values_rows.len() as u32, (headers.len() - 1) as u16)?;
    }

    for (c, (header, w)) in headers.iter().zip(max_widths.iter()).enumerate() {
        let mut width = (*w as f64 + 2.0).min(80.0).max(8.0);
        width = match *header {
            "source_file" => width.min(22.0),
            "source_page" => width.min(8.0),
            "voucher_type" => width.min(10.0),
            "work_date" => width.min(12.0),
            "company_name" => width.min(20.0),
            "start_time" | "end_time" | "work_time" => width.min(10.0),
            "break_minutes" | "worker_count" => width.min(10.0),
            "worker_names" => width.min(26.0),
            "vehicle_no" => width.min(12.0),
            "quantity_ton" | "cumulative_ton" | "departure_temp_c" => width.min(12.0),
            "transport_company" => width.min(18.0),
            "confidence" => width.min(10.0),
            "evidence" => 42.0,
            "warnings" => 32.0,
            _ => width,
        };
        worksheet.set_column_width(c as u16, width)?;
    }

    if std::env::var("PHOTO_TAGGER_SHOW_LONG_TEXT")
        .map(|v| v.trim().eq_ignore_ascii_case("1") || v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        // Keep long-text columns visible when explicitly requested.
    } else {
        // Default: hide verbose trace columns to improve readability.
        if let Some(evidence_col) = headers.iter().position(|h| *h == "evidence") {
            worksheet.set_column_hidden(evidence_col as u16)?;
        }
        if let Some(warnings_col) = headers.iter().position(|h| *h == "warnings") {
            worksheet.set_column_hidden(warnings_col as u16)?;
        }
    }

    workbook
        .save(path)
        .with_context(|| format!("Failed to write xlsx: {}", path.display()))?;
    Ok(())
}

fn csv_row_values(row: &VoucherCsvRow) -> Vec<String> {
    vec![
        row.source_file.clone(),
        row.source_page.map(|v| v.to_string()).unwrap_or_default(),
        row.voucher_type.clone(),
        row.work_date.clone().unwrap_or_default(),
        row.company_name.clone().unwrap_or_default(),
        row.start_time.clone().unwrap_or_default(),
        row.end_time.clone().unwrap_or_default(),
        row.break_minutes.map(|v| v.to_string()).unwrap_or_default(),
        row.worker_names.clone().unwrap_or_default(),
        row.worker_count.map(|v| v.to_string()).unwrap_or_default(),
        row.work_time.clone().unwrap_or_default(),
        row.vehicle_no.clone().unwrap_or_default(),
        row.quantity_ton.map(|v| v.to_string()).unwrap_or_default(),
        row.cumulative_ton.map(|v| v.to_string()).unwrap_or_default(),
        row.departure_temp_c.map(|v| v.to_string()).unwrap_or_default(),
        row.transport_company.clone().unwrap_or_default(),
        row.confidence.map(|v| v.to_string()).unwrap_or_default(),
        row.evidence.clone().unwrap_or_default(),
        row.warnings.clone().unwrap_or_default(),
    ]
}

fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn detect_format(path: &Path) -> Option<&'static str> {
    if is_json(path) {
        Some("json")
    } else if is_csv(path) {
        Some("csv")
    } else if is_xlsx(path) {
        Some("xlsx")
    } else {
        None
    }
}

fn de_opt_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(deserializer).unwrap_or(serde_json::Value::Null);
    let out = match v {
        serde_json::Value::Null => None,
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    };
    Ok(out)
}

fn de_opt_f32<'de, D>(deserializer: D) -> Result<Option<f32>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(deserializer).unwrap_or(serde_json::Value::Null);
    let out = match v {
        serde_json::Value::Null => None,
        serde_json::Value::Number(n) => n.as_f64().map(|x| x as f32),
        serde_json::Value::String(s) => s.trim().parse::<f32>().ok(),
        _ => None,
    };
    Ok(out)
}

fn de_opt_u32<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(deserializer).unwrap_or(serde_json::Value::Null);
    let out = match v {
        serde_json::Value::Null => None,
        serde_json::Value::Number(n) => n.as_u64().map(|x| x as u32),
        serde_json::Value::String(s) => s.trim().parse::<u32>().ok(),
        _ => None,
    };
    Ok(out)
}
