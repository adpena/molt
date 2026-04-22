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

import time

from _intrinsics import require_intrinsic as _require_intrinsic

_gpu_device = _require_intrinsic("molt_gpu_prim_device")

import tinygrad.realize
from tinygrad.tensor import Tensor


def main():
    device_code = _gpu_device()
    print(f"GPU primitive device code: {device_code}")

    # Create a synthetic 32x32 "image" (small for speed)
    print("Creating test tensor...")
    x = Tensor.rand(1, 3, 32, 32)

    # Create a small conv weight (3x3, 3->8 channels)
    w = Tensor.rand(8, 3, 3, 3)

    # Run conv2d (the hot op in PaddleOCR)
    print("Running conv2d...")
    start = time.time()
    y = x.conv2d(w, padding=1)
    tinygrad.realize.realize(y.lazydata)
    elapsed = (time.time() - start) * 1000

    print(f"Conv2d output shape: {y.shape}")
    print(f"Time: {elapsed:.1f} ms")

    # Run ReLU (activation fusion target)
    z = y.relu()
    tinygrad.realize.realize(z.lazydata)
    print(f"ReLU output shape: {z.shape}")

    # Run a simple matmul (for comparison)
    a = Tensor.rand(8, 8)
    b = Tensor.rand(8, 8)
    c = a.dot(b)
    tinygrad.realize.realize(c.lazydata)
    print(f"Matmul output shape: {c.shape}")

    print("PASS: compiled tinygrad inference loop works")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
