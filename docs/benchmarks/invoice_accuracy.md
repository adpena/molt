# Invoice OCR Accuracy Benchmark

**Date**: 2026-04-14
**Endpoint**: `https://falcon-ocr.adpena.workers.dev/ocr`
**Invoices tested**: 5
**Evaluator**: Fuzzy matching (Levenshtein ratio > 0.7), numeric tolerance 1%, date normalization

## Summary

| Invoice | Fields Expected | Fields Found | Accuracy | Latency (ms) |
|---------|----------------|--------------|----------|--------------|
| Simple | 4 | 0 | 0% | 32394 |
| Multi-line | 8 | 0 | 0% | 6585 |
| International (EUR) | 5 | 0 | 0% | 12365 |
| Complex | 6 | 0 | 0% | 7398 |
| Minimal | 3 | 0 | 0% | 10419 |
| **Overall** | **26** | **0** | **0%** | -- |

## Root Cause

The Workers AI backend (gemma-3-12b-it) is not performing OCR on the submitted
images. Instead it returns hallucinated/template content (e.g., `[No company
name visible]`, `[Invoice Number]`) or completely fabricated invoice data
unrelated to the input image. The `auto_filled: true` flag in some responses
confirms the model is generating content rather than extracting it.

**Action items:**
1. Switch to a vision model that supports actual image OCR (e.g., `@cf/meta/llama-3.2-11b-vision-instruct`)
2. Alternatively, use PaddleOCR as the primary extraction engine with AI-assisted field classification
3. Re-run this benchmark after the backend model change

## Evaluator Improvements (2026-04-14)

The evaluator was upgraded from exact string matching to:
- **Fuzzy matching**: Levenshtein distance ratio >= 0.7 for text fields
- **Numeric tolerance**: Extract amounts and compare within 1% tolerance
- **Date normalization**: Parse multiple date formats to ISO for comparison
- **Prose extraction**: Handle model responses like "The vendor is Acme Corp"
- **Substring containment**: Check normalized substrings before Levenshtein fallback

## Field-Level Details

### Simple (0%)

- `vendor`: MISSING (expected: "Northwind Traders")
- `invoice_number`: MISSING (expected: "INV-20241")
- `total_amount`: MISSING (expected: "2,400.00")
- `line_item:Cloud Hosting`: MISSING

### Multi-line (0%)

- `vendor`: MISSING (expected: "Quantum Dynamics")
- `invoice_number`: MISSING (expected: "QD-88712")
- `total_amount`: MISSING (expected: "4,170.74")
- `line_item:API Integration`: MISSING
- `line_item:Data Migration`: MISSING
- `line_item:Security Audit`: MISSING
- `line_item:Performance Tuning`: MISSING
- `line_item:Premium Support`: MISSING

### International (EUR) (0%)

- `vendor`: MISSING (expected: "Solaris GmbH")
- `invoice_number`: MISSING (expected: "SOL-44291")
- `total_amount`: MISSING (expected: "15.700")
- `line_item:Consultoria IT`: MISSING
- `line_item:Licencia Software`: MISSING

### Complex (0%)

- `vendor`: MISSING (expected: "IronForge Industries")
- `invoice_number`: MISSING (expected: "IFI-2026-0073")
- `total_amount`: MISSING (expected: "11,441.67")
- `line_item:Custom Dashboard`: MISSING
- `line_item:Load Testing`: MISSING
- `line_item:SSL Certificate`: MISSING

### Minimal (0%)

- `vendor`: MISSING (expected: "Pinnacle Dynamics")
- `invoice_number`: MISSING (expected: "PD-0042")
- `total_amount`: MISSING (expected: "750.00")
