# enjoice OCR Migration Guide: molt GPU Stack

This document describes how to update enjoice's Falcon-OCR integration
to use the new molt-compiled WASM module deployed on Cloudflare Workers.

## Overview

The current enjoice OCR architecture has three backends (defined in
`site/src/lib/ocr/index.ts`):

1. **Falcon-OCR** (browser-side, WebGPU) -- best accuracy, currently
   loading from a manifest-driven Molt driver module
2. **PaddleOCR** (browser-side, WASM) -- production-ready fallback
3. **Server-side OCR** (via `/api/ocr`) -- last resort

The migration replaces the browser-side Falcon-OCR path with the new
molt-compiled WASM module.  PaddleOCR and server-side OCR are
**unchanged**.

## What Changes

### 1. `site/src/lib/ocr/falcon-wrapper.ts`

**Current state:** Loads a browser module via dynamic `import()` from a
configured URL, initializes a `FalconDriverSession`, and calls
`session.ocrTokens()`.

**New state:** Load the molt-compiled `falcon-ocr.wasm` directly.  The
WASM module exports `init()` and `ocr_tokens()` matching the Python API.

Changes required:

- Replace `loadFalconDriverModule()` with a direct `WebAssembly.instantiateStreaming()`
  call pointing to the deployed WASM URL.
- Remove `FalconDriverModule` and `FalconDriverSession` interfaces.
- Replace `session.ocrTokens()` with a direct call to the WASM export:
  `wasmInstance.exports.ocr_tokens(width, height, rgb, promptIds, maxNewTokens)`.
- Keep the existing `imageToRgbPatchAligned()` logic for image
  preprocessing (or move it to a local utility since it's no longer
  part of the driver module).
- Keep the tokenizer loading and decode logic (`falcon-tokenizer.ts`)
  -- the WASM module returns token IDs, not text.

### 2. `site/src/lib/ocr/falcon-config.ts`

**Current state:** Three configurable URLs: browser module, manifest,
tokenizer.

**New state:** Two URLs: WASM module and tokenizer.

Changes required:

- Remove `browserModuleUrl` / `FALCON_OCR_BROWSER_MODULE_URL` -- replaced
  by a direct WASM URL.
- Remove `manifestUrl` / `FALCON_OCR_MANIFEST_URL` -- no longer needed.
- Add `wasmUrl` / `FALCON_OCR_WASM_URL` pointing to the deployed WASM binary
  (default: `https://falcon-ocr.freeinvoicemaker.workers.dev/falcon-ocr.wasm`
  or served from R2 via the Worker).
- Keep `tokenizerUrl` / `FALCON_OCR_TOKENIZER_URL`.

### 3. `site/src/lib/capabilities.ts`

**No changes required.**  The existing `webgpu` capability check is
sufficient.  The new WASM module uses WebGPU when available and falls
back to CPU WASM execution.

### 4. `site/src/lib/ocr/index.ts`

**Minimal changes.**  The `OcrResult` type, the multi-backend fallback
chain, and the PaddleOCR/server paths are all unchanged.

The only change is that `loadFalconOcrWasm()` now loads the new WASM
module instead of the old manifest-driven driver.  This is encapsulated
in `falcon-wrapper.ts`, so `index.ts` should not need changes beyond
possibly updating error messages.

### 5. `site/src/lib/ocr/falcon-tokenizer.ts`

**No changes required.**  Token encoding/decoding is independent of the
inference module.

## What Does NOT Change

- PaddleOCR fallback path (unchanged)
- Server-side OCR fallback path (unchanged)
- `OcrResult` type definition (unchanged)
- `OcrBlock` type definition (unchanged)
- Multi-backend auto-selection logic (unchanged)
- Telemetry tracking calls (unchanged)
- Privacy model: images never leave the user's device for the
  browser-side paths (unchanged)

## Performance Expectations

| Metric | Target | Notes |
|--------|--------|-------|
| WASM load | < 200ms | Cached after first load via Service Worker |
| Weight fetch | < 500ms | Served from R2 with edge caching |
| Cold start (first inference) | < 2s | WASM load + weight fetch + init |
| Warm inference (TTFT) | < 500ms | Model already initialized |
| Token throughput | 50-100 tok/s | WebGPU path; CPU WASM is slower |
| WASM binary size (gzip) | < 2 MB | Target for acceptable load time |

## Deployment Checklist

1. **Upload artifacts to R2:**
   ```bash
   # Compile WASM module
   molt build src/molt/stdlib/tinygrad/wasm_driver.py --target wasm

   # Upload to R2 (via wrangler or dashboard)
   wrangler r2 object put falcon-ocr-weights/models/falcon-ocr/falcon-ocr.wasm --file falcon-ocr.wasm
   wrangler r2 object put falcon-ocr-weights/models/falcon-ocr/weights.safetensors --file weights.safetensors
   wrangler r2 object put falcon-ocr-weights/models/falcon-ocr/config.json --file config.json
   ```

2. **Deploy the Cloudflare Worker:**
   ```bash
   cd deploy/cloudflare
   wrangler secret put X402_WALLET_ADDRESS
   wrangler secret put X402_VERIFICATION_URL
   wrangler deploy
   ```

3. **Verify the Worker:**
   ```bash
   curl https://falcon-ocr.freeinvoicemaker.workers.dev/health
   # Expected: {"status":"loading","model":"falcon-ocr","version":"0.1.0","device":"wasm"}
   ```

4. **Update enjoice config:**
   - Set `FALCON_OCR_WASM_URL` to the R2 public URL or Worker URL
   - Set `FALCON_OCR_TOKENIZER_URL` to the tokenizer JSON URL
   - Remove old `FALCON_OCR_BROWSER_MODULE_URL` and `FALCON_OCR_MANIFEST_URL`

5. **Test in staging:**
   - Verify Falcon-OCR loads and runs in Chrome (WebGPU)
   - Verify PaddleOCR fallback works in Firefox (no WebGPU)
   - Verify server-side fallback works when both client-side engines fail
   - Run the existing OCR test suite

6. **Deploy to production:**
   - Feature-flag the new WASM path behind `FALCON_OCR_V2=true`
   - Canary deploy to 5% of traffic
   - Monitor error rates, latency, and accuracy via telemetry
   - Roll out to 100% after 48h stability

## Template-from-Scan Integration

### Overview

The `POST /template/extract` endpoint on the falcon-ocr Worker scans an
invoice image and generates a reusable `TemplateDefinition` that preserves
the visual layout, branding, and section structure of the original document
without including any actual data (line items, amounts, names).

This enables a "Create Template from This Invoice" workflow in enjoice's
scan UI.

### REST API

```bash
curl -X POST https://falcon-ocr.adpena.workers.dev/template/extract \
    -H "Content-Type: application/json" \
    -H "X-Payment-402: <payment_proof_base64>" \
    -d '{
      "image": "<base64_encoded_invoice>",
      "document_type": "invoice",
      "preserve_logo": true,
      "detect_colors": true
    }'
```

Response:
```json
{
  "template": {
    "id": "uuid",
    "name": "Extracted Template",
    "type": "invoice",
    "layout": { "sections": [...], "page": {...} },
    "styles": { "colors": {...}, "fonts": {...}, ... }
  },
  "confidence": 0.85,
  "detected_sections": ["header", "parties", "metadata", "line_items", "totals", "footer"],
  "time_ms": 1200
}
```

Pricing: $0.005 per request (x402 protocol, USDC on Base).

### MCP Tool

The `ocr_extract_template` MCP tool wraps this endpoint for AI agent
use. See `deploy/mcp/template_from_scan_tool.json` for the full schema.

### enjoice ScanButton Integration

The scan flow in enjoice can offer "Create Template from This Invoice"
after a successful scan:

```typescript
// In the ScanButton component's onScanComplete handler:
async function handleCreateTemplate(imageBase64: string) {
  const response = await fetch(
    "https://falcon-ocr.adpena.workers.dev/template/extract",
    {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Payment-402": paymentProof,
      },
      body: JSON.stringify({
        image: imageBase64,
        document_type: "invoice",
      }),
    },
  );
  const { template, confidence } = await response.json();

  // Load into the template editor
  if (confidence > 0.3) {
    navigateToTemplateEditor(template);
  }
}
```

### Template Editor Reception

The extracted template is a standard `TemplateDefinition` object that
the inline editor (`useInlineTemplateEditor`) accepts directly:

```typescript
// In the InlineEditor, hydrate from extracted template:
const extractedTemplate: TemplateDefinition = result.template;
dispatch({ type: "HYDRATE", next: extractedTemplate });
```

The editor's undo/redo, autosave, and all style controls work on the
extracted template identically to manually-created templates.

## ScanButton "Create Template" Integration

After a successful OCR scan, the ScanButton can offer to create a
reusable template from the scanned invoice.  The `MoltOcrBackend`
provides an `extractTemplate()` method that calls the Worker's
`/template/extract` endpoint.

### Backend Usage

```typescript
import { MoltOcrBackend } from "./ocr-backend-molt";

const backend = new MoltOcrBackend({
  wasmUrl: "...",
  weightsUrl: "...",
  tokenizerUrl: "...",
  configUrl: "...",
  workerUrl: "https://falcon-ocr.adpena.workers.dev",
});

// After OCR completes, extract template from the same image:
const { template, confidence, detected_sections } =
  await backend.extractTemplate(imageBase64, {
    documentType: "invoice",
    preserveLogo: true,
    detectColors: true,
  });
```

### ScanButton Component Wiring

In `ScanButton.tsx`, add template creation after successful OCR:

```typescript
import { useState } from "react";

// In the ScanButton component:
const [showTemplateOption, setShowTemplateOption] = useState(false);
const [templateLoading, setTemplateLoading] = useState(false);

// After successful OCR in onScanComplete:
setShowTemplateOption(true);

// Render the template creation button:
{showTemplateOption && (
  <button
    disabled={templateLoading}
    onClick={async () => {
      setTemplateLoading(true);
      try {
        const { template, confidence } =
          await moltBackend.extractTemplate(imageBase64);
        if (confidence > 0.3) {
          window.location.href = `/templates/editor?from_template=${encodeURIComponent(
            JSON.stringify(template),
          )}`;
        } else {
          // Low confidence: offer manual template creation instead
          window.location.href = "/templates/new";
        }
      } catch (err) {
        console.error("Template extraction failed:", err);
      } finally {
        setTemplateLoading(false);
      }
    }}
  >
    Create Template from This Invoice
  </button>
)}
```

### Batch OCR for Multi-Page Invoices

For multi-page documents, use the `/ocr/batch` endpoint:

```typescript
const response = await fetch(
  "https://falcon-ocr.adpena.workers.dev/ocr/batch",
  {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      images: pageImages, // Array of base64 strings, max 10
    }),
  },
);
const { results, total_time_ms } = await response.json();
// results: Array<{ text, tokens, time_ms }>
```

## Batch Import

The `/ocr/batch` endpoint enables bulk invoice processing from enjoice's
ImportUpload component.  Each uploaded file is OCR'd in parallel, and each
successful result creates a draft invoice in the user's account.

### TypeScript Integration

Wire the batch endpoint into the `ImportUpload` component:

```typescript
// site/src/lib/import/BatchImportUploader.tsx

import { useState, useCallback } from "react";

interface BatchImportProgress {
  total: number;
  completed: number;
  results: Array<{
    filename: string;
    status: "pending" | "processing" | "done" | "error";
    invoice?: StructuredInvoice;
    error?: string;
  }>;
}

interface StructuredInvoice {
  vendor: string;
  invoice_number: string;
  issue_date: string;
  due_date: string;
  items: Array<{
    description: string;
    qty: number;
    rate: number;
    amount: number;
  }>;
  subtotal: number;
  tax: number;
  total: number;
  currency: string;
}

const WORKER_URL = "https://falcon-ocr.adpena.workers.dev";
const MAX_BATCH_SIZE = 10;

async function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const dataUrl = reader.result as string;
      // Strip the data URL prefix (data:image/png;base64,...)
      resolve(dataUrl.split(",")[1]);
    };
    reader.onerror = reject;
    reader.readAsDataURL(file);
  });
}

export function BatchImportUploader({
  onDraftCreated,
}: {
  onDraftCreated: (invoice: StructuredInvoice, filename: string) => void;
}) {
  const [progress, setProgress] = useState<BatchImportProgress | null>(null);

  const handleFiles = useCallback(
    async (files: FileList | File[]) => {
      const fileArray = Array.from(files).slice(0, MAX_BATCH_SIZE);

      const initial: BatchImportProgress = {
        total: fileArray.length,
        completed: 0,
        results: fileArray.map((f) => ({
          filename: f.name,
          status: "pending",
        })),
      };
      setProgress(initial);

      // Process each file through the structured OCR endpoint.
      // We use individual /ocr/structured requests (not /ocr/batch)
      // because the structured endpoint returns parsed JSON directly,
      // eliminating client-side parsing.
      for (let i = 0; i < fileArray.length; i++) {
        setProgress((prev) => {
          if (!prev) return prev;
          const results = [...prev.results];
          results[i] = { ...results[i], status: "processing" };
          return { ...prev, results };
        });

        try {
          const b64 = await fileToBase64(fileArray[i]);
          const resp = await fetch(`${WORKER_URL}/ocr/structured`, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ image: b64, format: "image/png" }),
          });

          if (!resp.ok) {
            const err = await resp.json();
            throw new Error(err.error || `HTTP ${resp.status}`);
          }

          const body = await resp.json();
          const invoice: StructuredInvoice = body.invoice;

          setProgress((prev) => {
            if (!prev) return prev;
            const results = [...prev.results];
            results[i] = {
              ...results[i],
              status: "done",
              invoice,
            };
            return {
              ...prev,
              completed: prev.completed + 1,
              results,
            };
          });

          onDraftCreated(invoice, fileArray[i].name);
        } catch (err) {
          setProgress((prev) => {
            if (!prev) return prev;
            const results = [...prev.results];
            results[i] = {
              ...results[i],
              status: "error",
              error: err instanceof Error ? err.message : String(err),
            };
            return {
              ...prev,
              completed: prev.completed + 1,
              results,
            };
          });
        }
      }
    },
    [onDraftCreated],
  );

  return (
    <div>
      <input
        type="file"
        accept="image/jpeg,image/png,image/webp"
        multiple
        onChange={(e) => e.target.files && handleFiles(e.target.files)}
      />
      {progress && (
        <div className="mt-4">
          <p className="text-sm font-medium">
            Processing {progress.completed}/{progress.total} invoices...
          </p>
          <div className="w-full bg-gray-200 rounded-full h-2 mt-2">
            <div
              className="bg-blue-600 h-2 rounded-full transition-all"
              style={{
                width: `${(progress.completed / progress.total) * 100}%`,
              }}
            />
          </div>
          <ul className="mt-3 space-y-1 text-sm">
            {progress.results.map((r, idx) => (
              <li key={idx} className="flex items-center gap-2">
                {r.status === "pending" && (
                  <span className="text-gray-400">Queued</span>
                )}
                {r.status === "processing" && (
                  <span className="text-blue-500">Processing...</span>
                )}
                {r.status === "done" && (
                  <span className="text-green-600">Done</span>
                )}
                {r.status === "error" && (
                  <span className="text-red-500">Error: {r.error}</span>
                )}
                <span className="text-gray-700">{r.filename}</span>
                {r.invoice && (
                  <span className="text-gray-500">
                    {r.invoice.vendor} - {r.invoice.currency}{" "}
                    {r.invoice.total.toFixed(2)}
                  </span>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
```

### Usage in ImportUpload Component

```typescript
// In the existing ImportUpload page component:
import { BatchImportUploader } from "./BatchImportUploader";

function ImportUploadPage() {
  const handleDraftCreated = (invoice: StructuredInvoice, filename: string) => {
    // Create a draft invoice in the local store
    createDraftInvoice({
      vendor: invoice.vendor,
      invoiceNumber: invoice.invoice_number,
      issueDate: invoice.issue_date,
      dueDate: invoice.due_date,
      items: invoice.items,
      subtotal: invoice.subtotal,
      tax: invoice.tax,
      total: invoice.total,
      currency: invoice.currency,
      sourceFile: filename,
      status: "draft",
    });
  };

  return (
    <div>
      <h2>Import Invoices</h2>
      <p>Upload invoice images to create draft invoices automatically.</p>
      <BatchImportUploader onDraftCreated={handleDraftCreated} />
    </div>
  );
}
```

### Batch Size Limits

- Maximum 10 files per upload (enforced client-side and by `/ocr/batch`)
- Each file is processed sequentially through `/ocr/structured` for
  maximum extraction quality (parallel processing via `/ocr/batch` is
  available for raw text mode when structured parsing is not needed)
- Total processing time: approximately 1-3 seconds per invoice (Workers AI)

## Rollback Plan

If the new WASM module has issues in production:

1. Remove `FALCON_OCR_WASM_URL` from enjoice config
2. The `isFalconConfigured()` check will return false
3. PaddleOCR automatically becomes the primary backend
4. No code changes required -- just a config change
