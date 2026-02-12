# Material Mode Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a new CLI "material mode" that analyzes each image with Gemini and writes `analysis.jsonl`, `analysis.json`, and `analysis.csv` for folder-name decision making.

**Architecture:** Implement a dedicated material-analysis module that builds prompts, parses per-image JSON results, and materializes JSON/CSV outputs. Wire this into the CLI with a new `--material` flag and output options, without changing existing grouping behavior.

**Tech Stack:** Rust, clap, serde/serde_json, csv crate, cli-ai-analyzer.

---

### Task 1: Define Material Record + Prompt

**Files:**
- Create: `src/material.rs`
- Modify: `src/lib.rs`

**Step 1: Write the failing test**

Add a test in `src/material.rs` that validates normalization of a partial JSON object into a full `MaterialRecord` (missing keys should become empty strings/arrays).

```rust
#[test]
fn material_record_normalizes_missing_fields() {
    let input = r#"{\"file\":\"a.jpg\",\"objects\":[\"roller\"]}"#;
    let rec = parse_material_json(input).unwrap();
    assert_eq!(rec.file, "a.jpg");
    assert_eq!(rec.objects, vec!["roller".to_string()]);
    assert_eq!(rec.board_text, "");
    assert_eq!(rec.other_text, "");
    assert_eq!(rec.notes, "");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test material_record_normalizes_missing_fields`
Expected: FAIL (missing `parse_material_json` and struct).

**Step 3: Write minimal implementation**

In `src/material.rs`:
- Define `MaterialRecord` (serde derive) with fields:
  - `file: String`
  - `objects: Vec<String>`
  - `board_text: String`
  - `other_text: String`
  - `notes: String`
  - `error: Option<String>`
- Implement `material_prompt(file: &str) -> String` that requests:
  - visible objects list
  - blackboard text
  - other visible text
  - JSON object only
- Implement `parse_material_json(raw: &str) -> Result<MaterialRecord>`:
  - Extract JSON object (first `{` to last `}`)
  - Deserialize into a helper struct with `Option<>` fields
  - Normalize missing fields to empty values

Expose needed items in `src/lib.rs` for use in main.

**Step 4: Run test to verify it passes**

Run: `cargo test material_record_normalizes_missing_fields`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/material.rs src/lib.rs
git commit -m "feat: add material record and prompt"
```

---

### Task 2: JSONL/JSON/CSV Materialization

**Files:**
- Modify: `src/material.rs`
- Modify: `Cargo.toml`

**Step 1: Write the failing test**

Add a test that writes two `MaterialRecord`s to JSONL, then materializes `analysis.json` and `analysis.csv` in a temp dir and validates the outputs exist and contain expected values.

```rust
#[test]
fn materialize_outputs_from_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join("analysis.jsonl");

    let rec1 = MaterialRecord::new("a.jpg");
    let rec2 = MaterialRecord { file: "b.jpg".into(), objects: vec!["roller".into()], ..MaterialRecord::new("b.jpg") };
    append_jsonl(&jsonl, &rec1).unwrap();
    append_jsonl(&jsonl, &rec2).unwrap();

    materialize_outputs(&jsonl, dir.path()).unwrap();

    assert!(dir.path().join("analysis.json").exists());
    assert!(dir.path().join("analysis.csv").exists());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test materialize_outputs_from_jsonl`
Expected: FAIL (missing functions and csv crate).

**Step 3: Write minimal implementation**

- Add dependency: `csv = "1.3"` and `tempfile = "3"` (dev-dependency).
- In `src/material.rs` implement:
  - `append_jsonl(path: &Path, rec: &MaterialRecord)`
  - `read_jsonl(path: &Path) -> Vec<MaterialRecord>`
  - `materialize_outputs(jsonl: &Path, out_dir: &Path)`
    - write `analysis.json` (pretty JSON array)
    - write `analysis.csv` using `csv::Writer`

**Step 4: Run test to verify it passes**

Run: `cargo test materialize_outputs_from_jsonl`
Expected: PASS.

**Step 5: Commit**

```bash
git add Cargo.toml src/material.rs
git commit -m "feat: add material output writers"
```

---

### Task 3: Add Material Mode CLI Flow

**Files:**
- Modify: `src/main.rs`
- Modify: `src/fs_ops.rs` (optional helper)

**Step 1: Write the failing test**

Add a unit test for a small helper (if created) that filters pending images against an existing JSONL list. If no helper is created, skip this step and proceed to implementation-only change.

**Step 2: Implement CLI flags and flow**

- Add clap flags:
  - `--material` (bool)
  - `--out <dir>` (PathBuf)
  - `--overwrite` (bool)
  - `--skip-existing` (bool)
- Ensure `--material` conflicts with existing grouping behavior if necessary.
- Flow when `--material` is set:
  1. Collect images (flat)
  2. Determine output dir (default: input path)
  3. Handle overwrite policy for `analysis.*`
  4. If `--skip-existing`, load existing JSONL and skip those files
  5. For each pending image:
     - Call `analyze(prompt, &[image], AnalyzeOptions::default().json())`
     - Parse with `parse_material_json`
     - On error, create `MaterialRecord` with `error`
     - Append to JSONL immediately
  6. After all, call `materialize_outputs`

**Step 3: Run tests**

Run: `cargo test`
Expected: PASS.

**Step 4: Commit**

```bash
git add src/main.rs src/fs_ops.rs
/git commit -m "feat: add material mode CLI"
```

---

### Task 4: Validation and Docs

**Files:**
- Modify: `README.md` (if present in repo) or skip

**Step 1: Add CLI usage notes**

Document the new flags and sample usage, including output files produced.

**Step 2: Run full test suite**

Run: `cargo test`
Expected: PASS.

**Step 3: Commit**

```bash
git add README.md
/git commit -m "docs: document material mode"
```

---

## Notes
- Keep material mode neutral: no role/machine classification in the prompt.
- Ensure errors don’t stop the batch; write `error` per file.
- Do not move or rename any images.
