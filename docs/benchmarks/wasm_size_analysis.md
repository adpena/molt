# WASM Runtime Size Analysis

**Date**: 2026-04-14
**File**: `wasm/molt_runtime.wasm`
**Total size**: 43.9 MB (12.3 MB gzipped)

## Section Breakdown

| Section | Size | % of Total | Notes |
|---------|------|-----------|-------|
| code | 7.8 MB | 17.7% | 10,611 functions |
| data | 0.7 MB | 1.7% | String tables, lookup tables |
| export | 0.3 MB | 0.8% | 6,419 exports |
| custom(.debug_info) | 14.2 MB | 32.3% | DWARF debug info |
| custom(.debug_str) | 11.2 MB | 25.4% | DWARF debug strings |
| custom(.debug_line) | 6.6 MB | 15.0% | DWARF line tables |
| custom(.debug_ranges) | 2.2 MB | 5.0% | DWARF ranges |
| custom(.debug_abbrev) | 0.1 MB | 0.2% | DWARF abbreviations |
| custom(.debug_loc) | 0.04 MB | 0.1% | DWARF locations |
| custom(name) | 0.7 MB | 1.6% | Function/local names |
| custom(producers) | <1 KB | 0.0% | Toolchain metadata |
| custom(target_features) | <1 KB | 0.0% | Target feature flags |
| type | 2.6 KB | 0.0% | Type signatures |
| import | 3.5 KB | 0.0% | Import declarations |
| func | 10.8 KB | 0.0% | Function type indices |
| global | 585 B | 0.0% | Global variables |
| elem | 5.9 KB | 0.0% | Table element segments |
| Other | <1 KB | 0.0% | start, memory, table |

## Key Findings

### 1. Debug sections dominate (80% of file size)

Total debug/custom sections: **35.0 MB** (79.6% of file).

A production `wasm-strip` or `wasm-opt --strip-debug` would reduce the file to:
- **~9.3 MB** uncompressed (code + data + export + structural sections)
- **~3.0 MB** estimated gzipped (code compresses well, ~2.5x ratio)

### 2. Export count is high but already addressed

6,419 exports is significant. Commit `599affbd` already eliminated 7,359 dead table ref exports from non-split builds. The remaining exports are live runtime entry points used by compiled user code.

### 3. Function count (10,611) suggests tree-shaking opportunity

The runtime ships all builtins regardless of what the user program actually imports. A whole-program tree-shaking pass that traces from the user's import set could eliminate unused builtins.

Estimated savings from function-level DCE:
- Typical user program uses 20-40% of builtins
- Potential code section reduction: 4-6 MB (before strip)
- Potential stripped+gzipped: **1.5-2.0 MB** (from current ~3 MB stripped estimate)

### 4. Data section is compact

At 0.7 MB, the data section (string constants, type descriptors, vtables) is well-optimized. No immediate action needed.

## Recommendations

| Priority | Action | Expected Savings |
|----------|--------|-----------------|
| P0 | Strip debug sections for production builds | -35 MB (80% reduction) |
| P1 | Per-program runtime tree-shaking (link-time DCE) | -3-5 MB unstripped |
| P2 | Merge duplicate function bodies (wasm-opt --merge-similar-functions) | -0.5-1 MB |
| P3 | Compress name section or strip for production | -0.7 MB |

## Production Build Command

```bash
# Strip debug + optimize
wasm-opt -O3 --strip-debug --strip-producers \
    wasm/molt_runtime.wasm -o wasm/molt_runtime_prod.wasm

# Expected result: ~8-9 MB uncompressed, ~2.5-3 MB gzipped
```

## Comparison: falcon-ocr.wasm

The compiled Falcon-OCR model WASM (served from R2) is 13.4 MB / 4 MB gzipped.
This is the user-facing artifact and already has debug stripped.
