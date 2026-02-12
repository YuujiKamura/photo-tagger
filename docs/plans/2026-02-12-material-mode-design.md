# 2026-02-12 Material Mode Design (Photo Tagger)

## Goal
Create a new "material mode" that analyzes each image with Gemini to produce neutral, non-interpretive JSON describing visible objects and text (especially blackboard text). The output is used to decide destination folder names later, not to move files.

## Scope
- Add a new CLI mode that runs per-image analysis and writes three outputs: JSONL, JSON, CSV.
- Reuse existing image collection and Gemini backend from `cli-ai-analyzer`.
- Do not modify or delete images.
- Keep grouping mode as-is.

## Non-Goals
- No automatic folder moves or renames.
- No human-labeled categories or workflow-specific roles.
- No UI or web interface changes.

## Proposed CLI
- `photo-tagger <path> --material`
- `--out <dir>` output directory (default: input folder)
- `--overwrite` overwrite existing `analysis.*`
- `--skip-existing` skip files already present in `analysis.jsonl`
- `--profile` show timing (reuse existing behavior if possible)
- Optional later: `--concurrent N` (default: 1)

## Data Model
Per-image record (one JSON object):
- `file` string
- `objects` array of strings
- `board_text` string
- `other_text` string
- `notes` string
- `error` string (optional; present when analysis failed)

Output files:
- `analysis.jsonl` one record per line
- `analysis.json` array of records
- `analysis.csv` columns: `file, objects, board_text, other_text, notes, error`

## Prompt Strategy
- Ask Gemini to extract only factual content.
- Emphasize listing visible objects and transcribing any text on blackboards or signage.
- Avoid role/meaning classification.
- Request JSON only, with the specified keys.

## Processing Flow
1. Collect images from the target folder (flat, non-recursive).
2. Determine pending images based on `analysis.jsonl` when `--skip-existing` is set.
3. For each image, call `cli_ai_analyzer::analyze` with `AnalyzeOptions::default().json()`.
4. Parse JSON. If invalid or missing keys, record `error` and continue.
5. Append each record to `analysis.jsonl` as it completes.
6. At end, materialize `analysis.json` and `analysis.csv` from the JSONL records.

## Error Handling
- Failure on one image should not stop the batch.
- Store `error` field with the failure reason.
- Continue processing remaining images.
- Return non-zero only if zero images could be processed.

## Testing
- Unit test JSONL to JSON/CSV materialization.
- Unit test schema normalization (missing keys -> empty strings or empty array).
- No live Gemini calls in tests.

## Success Criteria
- Running `--material` produces the three output files.
- All images result in a record, with errors captured when analysis fails.
- The output can be used to decide folder names without manual re-analysis.
