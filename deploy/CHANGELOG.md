# Falcon-OCR Deploy Changelog

## 2026-04-14

### Browser test page hardening
- Added `window.__falconOCR` exposure to standalone `test.html` for headless
  automation parity with the embedded worker test page
- Added `imageBitmap.close()` after drawing to canvas in both standalone and
  embedded test pages to prevent GPU memory leaks on repeated OCR runs
- Verified module import chain: `test.html -> falcon-ocr-loader.js ->
  compute-engine.js -> webgpu-matmul.js / webgl2-engine.js` all use correct
  relative imports that resolve when served from `/browser/` on R2

### enjoice integration hardening
- Added structured error classification in `ocr-backend-molt.ts` for CORS,
  network, and OOM failures during WASM init with diagnostic messages
- Added try/catch around WASM inference in the browser path with progress
  reporting on failure
- Added auto-fill warning propagation: WASM-based OCR results now set
  `autoFilled`, `autoFillWarning`, and `autoFillDismissable` so enjoice
  ScanButton can display the verification banner
- Confidence estimate corrected from "INT4" to "INT8" in code comment

### Documentation
- Created `deploy/PRODUCTION_STATUS.md` documenting full architecture,
  component status, browser WASM inference path, and Workers AI usage scope
- Created `deploy/CHANGELOG.md` (this file)

### Workers AI clarification
- Confirmed Workers AI OCR references in `ai-fallback.js` and `worker.js`
  are already correctly scoped: Workers AI is explicitly documented as NOT
  used for OCR text extraction (hallucinates), only for NL fill and template
  extraction. No stale references to remove.
