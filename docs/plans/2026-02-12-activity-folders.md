# Activity Foldering Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `--activity-folders` mode to classify photos by activity using `analysis.csv`, with time-gap fallback when blackboard text is missing.

**Architecture:** Read `analysis.csv`, parse photo timestamps from filenames, classify by OCR keywords when present, and for missing OCR, inherit the previous activity unless the time gap from the previous photo exceeds `--gap-min` (default 10), in which case start a new time block. Create folders and move files unless `--dry-run`.

**Tech Stack:** Rust, clap, csv, std::fs.

---

### Task 1: Add Activity Classification Helpers

**Files:**
- Modify: `src/material.rs`

**Step 1: Write the failing test**

Add tests for:
- `classify_activity(text)` mapping keywords to activity names
- `infer_activity_with_gap(prev, curr, gap_min)` behavior (inherits vs new block)

```rust
#[test]
fn classify_activity_keywords() {
    assert_eq!(classify_activity("交通保安施設 設置状況"), Some("交通保安施設_設置状況"));
    assert_eq!(classify_activity("積載量 確認"), Some("積載量_確認"));
}

#[test]
fn infer_activity_with_gap_inherits() {
    let prev = ActivityFrame { activity: "積載量_確認".into(), ts: 1000 };
    let curr = ActivityFrame { activity: "".into(), ts: 1000 + 9 * 60 };
    assert_eq!(infer_activity_with_gap(Some(&prev), &curr, 10), "積載量_確認");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test classify_activity_keywords infer_activity_with_gap_inherits`
Expected: FAIL (missing functions/struct).

**Step 3: Write minimal implementation**

- Add:
  - `fn classify_activity(text: &str) -> Option<&'static str>`
  - `struct ActivityFrame { activity: String, ts: i64 }`
  - `fn infer_activity_with_gap(prev: Option<&ActivityFrame>, curr: &ActivityFrame, gap_min: i64) -> String`

**Step 4: Run test to verify it passes**

Run: `cargo test classify_activity_keywords infer_activity_with_gap_inherits`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/material.rs
git commit -m "feat: add activity classification helpers"
```

---

### Task 2: Implement Activity Foldering Mode

**Files:**
- Modify: `src/main.rs`

**Step 1: Write the failing test**

Add a unit test for timestamp parsing from filename `YYYYMMDD_HHMMSS` and gap calculation.

```rust
#[test]
fn parse_timestamp_from_filename() {
    let ts = parse_photo_timestamp("20260211_235409.jpg").unwrap();
    assert!(ts > 0);
}
```

**Step 2: Implement CLI flags and flow**

Add clap flags:
- `--activity-folders`
- `--gap-min <minutes>` default 10

Flow:
1) Read `analysis.csv` from target folder.
2) For each row, create ActivityFrame with timestamp from filename.
3) If `board_text` has activity keyword -> use that activity.
4) If not, inherit previous activity if gap < `gap-min`; else set `未分類` or `時間ブロック_YYYYMMDD_HHMM`.
5) Create folders and move files unless `--dry-run`.

**Step 3: Run tests**

Run: `cargo test`
Expected: PASS.

**Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: add activity folders mode"
```

---

### Task 3: Update CSV Schema Handling (Optional)

**Files:**
- Modify: `src/material.rs`

If needed, ensure CSV read does not break with new fields. Add robustness test for missing columns.

**Step 1: Run tests**

Run: `cargo test`
Expected: PASS.

**Step 2: Commit**

```bash
git add src/material.rs
git commit -m "test: harden activity csv parsing"
```

---

## Notes
- Activity keywords mapping should include: 交通保安施設_設置状況, トラックスケール_計量状況, 積載量_確認, 処分状況_社内検査, 出荷指示確認.
- Time gap is based on difference from the immediately previous photo.
- `--dry-run` should print intended moves without moving files.
