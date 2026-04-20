# WASM Binary Size Optimization — Falcon-OCR

Date: 2026-04-14

## Summary

Stripped DWARF debug data and applied `wasm-opt -Oz` to the Falcon-OCR WASM binary.
The pipeline: `wasm-strip` (WABT) to remove debug + linking sections, then `wasm-opt -Oz` (Binaryen 129) with reference-types enabled.

## Size Comparison

| Variant | Raw | Gzipped |
|---------|-----|---------|
| Original (with debug + 7359 exports) | 44 MB | 12.3 MB |
| Reduced exports (3099) | 19.6 MB | 5.9 MB |
| wasm-strip only | 14.7 MB | 3.8 MB |
| wasm-strip + wasm-opt -Oz | **8.5 MB** | **2.0 MB** |

## Runtime-only (molt_runtime.wasm)

| Variant | Raw | Gzipped |
|---------|-----|---------|
| Original | 43.9 MB | 12.3 MB |
| Stripped debug | 8.3 MB | 2.6 MB |

## Tool Versions

- wasm-opt: Binaryen version 129
- wasm-strip: WABT (homebrew)
- Features enabled: reference-types, bulk-memory, mutable-globals, sign-ext, nontrapping-float-to-int

## Reproduction

```bash
# 1. Strip custom/debug sections
wasm-strip input.wasm -o stripped.wasm

# 2. Optimize
wasm-opt -Oz --enable-reference-types --enable-bulk-memory \
  --enable-mutable-globals --enable-sign-ext \
  --enable-nontrapping-float-to-int stripped.wasm -o final.wasm
```

## Deployed

Uploaded to R2: `falcon-ocr-weights/models/falcon-ocr/falcon-ocr.wasm` (8.5 MB, ~2.0 MB gzipped)
