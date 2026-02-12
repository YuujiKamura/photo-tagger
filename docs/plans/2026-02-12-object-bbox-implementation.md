# Object BBox Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extend material analysis to return object bounding boxes (normalized 0..1) and store them in JSONL/JSON/CSV.

**Architecture:** Replace objects string list with structured objects in MaterialRecord. Prompt Gemini to output objects with bbox and area_ratio. Write objects JSON to CSV as objects_json.

**Tech Stack:** Rust, serde, serde_json, csv.

---

### Task 1: Add Object Struct + Parsing

**Files:**
- Modify: `src/material.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn parse_objects_with_bbox() {
    let input = r#"{\"file\":\"a.jpg\",\"objects\":[{\"label\":\"看板\",\"bbox\":{\"x\":0.1,\"y\":0.2,\"w\":0.3,\"h\":0.4},\"area_ratio\":0.12}]}"#;
    let rec = parse_material_json(input).unwrap();
    assert_eq!(rec.objects.len(), 1);
    assert_eq!(rec.objects[0].label, "看板");
    assert_eq!(rec.objects[0].bbox.w, 0.3);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test parse_objects_with_bbox`
Expected: FAIL.

**Step 3: Write minimal implementation**

- Add `ObjectBBox` and `ObjectItem` structs (serde).
- Change `MaterialRecord.objects` to `Vec<ObjectItem>`.
- Update parsing to support objects array.

**Step 4: Run test to verify it passes**

Run: `cargo test parse_objects_with_bbox`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/material.rs
git commit -m "feat: add object bbox structs"
```

---

### Task 2: Update Prompt + CSV Output

**Files:**
- Modify: `src/material.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn csv_includes_objects_json() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join("analysis.jsonl");
    let rec = MaterialRecord::new("a.jpg");
    append_jsonl(&jsonl, &rec).unwrap();
    materialize_outputs(&jsonl, dir.path()).unwrap();
    let csv = std::fs::read_to_string(dir.path().join("analysis.csv")).unwrap();
    assert!(csv.contains("objects_json"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test csv_includes_objects_json`
Expected: FAIL.

**Step 3: Implement updates**

- Prompt: request objects with `label`, `bbox`, `area_ratio`, max N.
- CSV: add `objects_json` column (serialize objects array).

**Step 4: Run test to verify it passes**

Run: `cargo test csv_includes_objects_json`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/material.rs
git commit -m "feat: add objects_json to csv"
```

---

### Task 3: Verify End-to-End

**Files:**
- None

**Step 1: Run tests**

Run: `cargo test`
Expected: PASS.

**Step 2: Commit**

```bash
# no-op commit if nothing else changed
```

---

## Notes
- Keep bbox normalized 0..1.
- Do not clamp or validate in code.
- Store objects as JSON string in CSV.
