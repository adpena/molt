"""
tinygrad.realize — realize() pipeline.

schedule() -> fuse() -> render() -> execute()

This module provides the CPU reference path. When molt-gpu FFI is available,
the pipeline dispatches to Rust for GPU execution.
"""

from __future__ import annotations
from _intrinsics import require_intrinsic as _require_intrinsic

_gpu_device = _require_intrinsic("molt_gpu_prim_device")

from tinygrad.lazy import LazyBuffer


def realize(buf: LazyBuffer) -> list:
    """Materialize a LazyBuffer.

    For the CPU reference path, this directly executes the LazyOp DAG.
    For GPU paths, this would go through:
      1. schedule() — linearize the DAG into an execution schedule
      2. fuse() — merge compatible ops into fused kernels
      3. render() — generate shader source from fused kernels
      4. execute() — compile and dispatch shaders on the target device
    """
    return buf.realize()
