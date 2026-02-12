# 2026-02-12 Object BBox Design

## Goal
Add bounding boxes (normalized 0..1) and area_ratio for detected objects so scene classification can be done without opening the image.

## Scope
- Extend material analysis output to include structured objects with bbox and area_ratio.
- Preserve existing blackboard OCR fields (board_text, board_lines, board_fields).
- Write objects to JSONL/JSON and add an objects_json column to CSV.

## Data Model
Each object:
- label: string
- bbox: { x, y, w, h } (0..1 normalized)
- area_ratio: number (expected ~= w*h)

MaterialRecord additions:
- objects: array of objects (replaces string list)

CSV:
- objects_json: JSON string of objects array

## Prompt Strategy
- Require JSON only.
- Ask for top N objects (e.g., 8) with bbox in 0..1 coordinates.
- Keep existing fields (board_text/lines/fields).

## Error Handling
- If objects parsing fails, store empty array and set error field.
- Do not clamp or correct bbox values; record as-is.
- Continue processing other images on error.

## Testing
- Serialize/deserialize objects JSON.
- CSV includes objects_json column.
- Empty/malformed objects handled gracefully.

## Success Criteria
- analysis.jsonl/json include objects with bbox.
- analysis.csv has objects_json column with valid JSON strings.
- No regression to existing OCR fields.
