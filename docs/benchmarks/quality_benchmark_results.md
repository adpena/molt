# Falcon-OCR Quality Benchmark Results

Date: 2026-04-14

## Test Setup

- 5 synthetic invoices generated with Pillow (800x600 PNG, default font)
- Known ground truth: vendor name, invoice number, line items, total
- Endpoint: https://falcon-ocr.adpena.workers.dev/ocr
- Headers: `Origin: https://freeinvoicemaker.app`, `Content-Type: application/json`

## Test Cases

| # | Name | Vendor | Invoice # | Total | Items |
|---|------|--------|-----------|-------|-------|
| 1 | Simple | Acme Corp | INV-2026-001 | $4,200.00 | 1 |
| 2 | Multi-line | TechFlow Inc | TF-8891 | $12,750.00 | 3 |
| 3 | International | Berlin GmbH | DE-2026-042 | EUR 8.500,00 | 1 |
| 4 | Complex | Global Solutions Ltd | GS-10054 | $25,680.50 | 3 |
| 5 | Minimal | Jane Doe | (none) | $500.00 | 1 |

## Results

All 5 requests returned HTTP 403 (Cloudflare error code 1010 - Access Denied).

The Worker is behind Cloudflare Bot Management which blocks non-browser programmatic access.
This is expected behavior for production: the bot protection prevents abuse while allowing
real browser requests from freeinvoicemaker.app.

### Implications

- **CLI/script testing** requires either:
  1. A service token bypass (CF Access Service Auth header)
  2. A `/ocr/test` endpoint exempted from bot rules
  3. Browser-based testing via Playwright or similar

- **Browser-based OCR** (the primary use case) is unaffected -- real browser requests
  with proper Origin pass through Cloudflare's challenge seamlessly.

## Recommended Next Steps

1. Add a CF Access service token for CI/automated testing
2. Run browser-based quality validation via Playwright (see Task: "End-to-end browser WASM accuracy validation" in PRODUCTION_CHECKLIST.md)
3. Once endpoint is accessible, re-run with character-level accuracy metrics (Levenshtein distance)

## Previous Results Reference

See `docs/benchmarks/invoice_accuracy.md` and `docs/benchmarks/ocr_quality_comparison.md`
for prior accuracy measurements taken before bot protection was enabled.
