# OCR Quality Comparison: Falcon-OCR vs PaddleOCR

## Methodology

5 synthetic invoice images generated with Pillow, each with known ground truth.
Character accuracy = 1 - (Levenshtein distance / ground truth length).
Field extraction = substring match (case-insensitive) for vendor, invoice number, total.

## Test Cases

| # | Name | Description |
|---|------|-------------|
| 1 | clean_text | Standard 24pt text: "INVOICE #2026-042 | Acme Corp | Total: $4,200.00" |
| 2 | small_font | Same text at 8pt equivalent (11px) |
| 3 | rotated_5deg | Same text rotated 5 degrees |
| 4 | low_contrast | Light gray (200,200,200) on white |
| 5 | dense_table | 5 columns x 10 rows of line items |

## Results

Run `python3 tests/e2e/test_ocr_quality_comparison.py` to generate live results.

| Test | Engine | Char Accuracy | Vendor | Invoice # | Total | Latency (ms) | Available |
|------|--------|--------------|--------|-----------|-------|--------------|-----------|
| (run test to populate) | | | | | | | |

## Setup Notes

- Falcon-OCR: Live endpoint at https://falcon-ocr.adpena.workers.dev/ocr (Gemma 3 12B via Workers AI)
  - Requires `Origin: https://freeinvoicemaker.app` header
  - POST JSON: `{"image": "<base64_png>"}`
- PaddleOCR: `pip3 install paddleocr` (requires paddlepaddle). If not installed, results show N/A.
- Images generated at runtime with Pillow; no external image assets needed.
- To run: `python3 tests/e2e/test_ocr_quality_comparison.py`
