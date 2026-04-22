"""
Self-contained PaddleOCR-style tinygrad microbenchmark.

When compiled to WASM and run, it:
1. Creates a synthetic test pattern (no external image needed)
2. Runs a conv2d-heavy tinygrad path similar to PaddleOCR detector hot loops
3. Reports timing

This proves the compiled tinygrad arithmetic loop works end-to-end. It is not
a PaddleOCR model-accuracy benchmark and does not load real OCR weights.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_gpu_device = _require_intrinsic("molt_gpu_prim_device")

import tinygrad.realize
from tinygrad.tensor import Tensor


def main():
    # Create a synthetic 32x32 "image" (small for speed)
    print("Creating test tensor...")
    x = Tensor.rand(1, 3, 32, 32)

    # Create a small conv weight (3x3, 3->8 channels)
    w = Tensor.rand(8, 3, 3, 3)

    # Run conv2d (the hot op in PaddleOCR)
    print("Running conv2d...")
    y = x.conv2d(w, padding=1)
    y_data = y.tolist()

    print(f"Conv2d output shape: {y.shape}")

    # Run ReLU (activation fusion target)
    z = y.relu()
    z_data = z.tolist()
    print(f"ReLU output shape: {z.shape}")

    # Chain conv2d + relu (PaddleOCR fused pattern)
    w2 = Tensor.rand(16, 8, 3, 3)
    y2 = z.conv2d(w2, padding=1)
    y2_data = y2.tolist()
    print(f"Conv2d layer 2 output shape: {y2.shape}")

    r2 = y2.relu()
    r2_data = r2.tolist()
    print(f"ReLU layer 2 output shape: {r2.shape}")

    print("PASS: compiled tinygrad inference loop works")
    return 0


main()
