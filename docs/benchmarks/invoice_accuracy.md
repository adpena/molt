# Invoice OCR Accuracy Benchmark

**Date**: 2026-04-20 19:11:52 UTC
**Endpoint**: `https://falcon-ocr.adpena.workers.dev/ocr`
**Invoices tested**: 5

## Summary

| Invoice | Fields Expected | Fields Found | Accuracy | Latency (ms) |
|---------|----------------|--------------|----------|--------------|
| Simple | 4 | 0 | 0% | 3151 |
| Multi-line | 8 | 0 | 0% | 16873 |
| International (EUR) | 5 | 0 | 0% | 4111 |
| Complex | 6 | 0 | 0% | 7035 |
| Minimal | 3 | 0 | 0% | 4175 |
| **Overall** | **26** | **0** | **0%** | — |

## Field-Level Details

### Simple (0%)

- `vendor`: MISSING
- `invoice_number`: MISSING
- `total_amount`: MISSING
- `line_item:Cloud Hosting`: MISSING

<details><summary>Raw OCR text (truncated)</summary>

```
[Invoice Image Data]
```
</details>

### Multi-line (0%)

- `vendor`: MISSING
- `invoice_number`: MISSING
- `total_amount`: MISSING
- `line_item:API Integration`: MISSING
- `line_item:Data Migration`: MISSING
- `line_item:Security Audit`: MISSING
- `line_item:Performance Tuning`: MISSING
- `line_item:Premium Support`: MISSING

<details><summary>Raw OCR text (truncated)</summary>

```
Company Name: [Unclear - Image quality prevents clear identification]
Invoice Number: 1187
Dates:
  Invoice Date: 7/19/2024
  Due Date: 8/19/2024

Line Items:
  Item: QTY: 1 X 1500.00
  Item: QTY: 1 X 1500.00
  Item: QTY: 1 X 1500.00
  Item: QTY: 1 X 1500.00
  Item: QTY: 1 X 1500.00
  Item: QTY: 1 X 1500.00

Subtotal: 9000.00
Taxes: 0.00
Total: 9000.00
```
</details>

### International (EUR) (0%)

- `vendor`: MISSING
- `invoice_number`: MISSING
- `total_amount`: MISSING
- `line_item:Consultoria IT`: MISSING
- `line_item:Licencia Software`: MISSING

<details><summary>Raw OCR text (truncated)</summary>

```
Company Name: Not available

Invoice Number: Not available

Dates: Not available

Line Items: Not available

Subtotals: Not available

Taxes: Not available

Totals: Not available

Other Text: Not available
```
</details>

### Complex (0%)

- `vendor`: MISSING
- `invoice_number`: MISSING
- `total_amount`: MISSING
- `line_item:Custom Dashboard`: MISSING
- `line_item:Load Testing`: MISSING
- `line_item:SSL Certificate`: MISSING

<details><summary>Raw OCR text (truncated)</summary>

```
Company Name: [Company Name Not Visible]

Invoice Number: [Invoice Number Not Visible]

Dates: [Dates Not Visible]

Line Items:
[Line Item Details Not Visible]

Subtotals: [Subtotal Amount Not Visible]

Taxes: [Tax Amount Not Visible]

Totals: [Total Amount Not Visible]
```
</details>

### Minimal (0%)

- `vendor`: MISSING
- `invoice_number`: MISSING
- `total_amount`: MISSING

<details><summary>Raw OCR text (truncated)</summary>

```
**INVOICE**

**Company Name:** The Grill House

**Invoice Number:** 2024-04-23-001

**Dates:**
*   Invoice Date: April 23, 2024
*   Due Date: May 23, 2024

**Line Items:**

*   Item 1: 1 x Grilled Chicken Salad - $14.99
*   Item 2: 2 x Cheeseburgers - $18.00
*   Item 3: 1 x French Fries - $5.99
*   Item 4: 2 x Soft Drinks - $6.00
*   Item 5: 1 x Chocolate Brownie - $7.99

**Subtotal:** $52.97

**Tax (8%):** $4.24

**Total:** $57.21
```
</details>
