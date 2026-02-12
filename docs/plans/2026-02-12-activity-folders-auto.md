# Auto Activity Naming Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `--activity-folders-auto` to derive folder names from OCR text clusters without hardcoded rules, using top 2 keywords for naming.

**Architecture:** Read `analysis.csv`, extract OCR text per photo, normalize tokens, compute frequent keywords per cluster, assign cluster names as `keyword1_keyword2`. Use time-gap fallback for photos without OCR (inherit previous cluster if gap < `--gap-min`, else `未分類`).

**Tech Stack:** Rust, clap, csv, std::collections.

---

### Task 1: Tokenization + Keyword Scoring

**Files:**
- Modify: `src/material.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn extract_top_keywords_basic() {
    let text = "交通保安施設 設置状況 交通保安施設";
    let kws = extract_top_keywords(text, 2);
    assert_eq!(kws, vec!["交通保安施設", "設置状況"]);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test extract_top_keywords_basic`
Expected: FAIL.

**Step 3: Implement minimal code**

- Add `fn extract_top_keywords(text: &str, k: usize) -> Vec<String>`
  - Split on whitespace and punctuation, remove empty
  - Count frequency, tie-break by first occurrence
  - Return top-k

**Step 4: Run test to verify it passes**

Run: `cargo test extract_top_keywords_basic`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/material.rs
git commit -m "feat: add keyword extraction"
```

---

### Task 2: Auto Activity Foldering Flow

**Files:**
- Modify: `src/main.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn auto_name_from_keywords() {
    let name = make_activity_name(&["交通保安施設".into(), "設置状況".into()]);
    assert_eq!(name, "交通保安施設_設置状況");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test auto_name_from_keywords`
Expected: FAIL.

**Step 3: Implement flow**

- Add CLI flag `--activity-folders-auto`
- Read `analysis.csv`
- Build OCR text per photo
- For each photo with OCR text:
  - Extract top 2 keywords -> activity name
- If OCR missing:
  - inherit previous activity if gap < `--gap-min`
  - else `未分類`
- Create folders and move files unless `--dry-run`

**Step 4: Run tests**

Run: `cargo test`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: add auto activity foldering"
```

---

## Notes
- Default `k=2` keywords for folder name.
- If keywords are empty, fallback to `未分類`.
