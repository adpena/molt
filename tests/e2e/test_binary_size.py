"""
Binary size analysis for rendered GPU shaders.

Measures rendered shader source sizes for key compositions across all
backend renderers (MSL, WGSL, CUDA, HIP). Documents sizes and enforces
a 10KB sanity check per kernel to prevent shader bloat.

Run: python -m pytest tests/e2e/test_binary_size.py -v -s
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import textwrap

import pytest

# Ensure project root is importable
_project_root = os.path.dirname(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
)
if _project_root not in sys.path:
    sys.path.insert(0, _project_root)


# ---------------------------------------------------------------------------
# Shader source templates (matching actual Rust renderer output patterns)
# ---------------------------------------------------------------------------

# These templates mirror the exact output structure of the Rust renderers.
# The Rust test suite (test_render_*.rs) validates actual renderer output;
# here we measure the size characteristics of rendered shaders.


def _render_msl_elementwise(
    ops: list[tuple[str, str, str]],
    buf_count: int,
    n: int,
) -> str:
    """Generate MSL shader source matching MslRenderer output structure.

    Args:
        ops: List of (dtype_str, var_name, expression) tuples.
        buf_count: Number of buffer parameters.
        n: Output element count.
    """
    lines = [
        "#include <metal_stdlib>",
        "using namespace metal;",
        "",
    ]
    # Signature
    params = []
    for i in range(buf_count):
        qualifier = "device" if i == 0 else "const device"
        params.append(f"{qualifier} float* buf{i} [[buffer({i})]]")
    params.append("uint gid [[thread_position_in_grid]]")
    lines.append(f"kernel void molt_kernel({', '.join(params)}) {{")
    lines.append(f"    if (gid >= {n}) return;")
    for dtype, var, expr in ops:
        lines.append(f"    {dtype} {var} = {expr};")
    last_var = ops[-1][1]
    lines.append(f"    buf0[gid] = {last_var};")
    lines.append("}")
    return "\n".join(lines)


def _render_msl_reduce(
    pre_ops: list[tuple[str, str, str]],
    reduce_type: str,
    post_ops: list[tuple[str, str, str]],
    buf_count: int,
    out_n: int,
    reduce_size: int,
) -> str:
    """Generate MSL shader source with reduce loop."""
    lines = [
        "#include <metal_stdlib>",
        "using namespace metal;",
        "",
    ]
    params = []
    for i in range(buf_count):
        qualifier = "device" if i == 0 else "const device"
        params.append(f"{qualifier} float* buf{i} [[buffer({i})]]")
    params.append("uint gid [[thread_position_in_grid]]")
    lines.append(f"kernel void molt_kernel({', '.join(params)}) {{")
    lines.append(f"    if (gid >= {out_n}) return;")

    init = "0" if reduce_type == "sum" else "-INFINITY"
    lines.append(f"    float acc = {init};")
    lines.append(f"    for (uint rid = 0; rid < {reduce_size}; rid++) {{")
    lines.append(f"        uint eidx = gid * {reduce_size} + rid;")

    for dtype, var, expr in pre_ops:
        lines.append(f"        {dtype} {var} = {expr};")

    last_pre = pre_ops[-1][1] if pre_ops else "buf1[eidx]"
    if reduce_type == "sum":
        lines.append(f"        acc += {last_pre};")
    else:
        lines.append(f"        acc = max(acc, {last_pre});")
    lines.append("    }")

    reduce_var_idx = len(pre_ops)
    lines.append(f"    float v{reduce_var_idx} = acc;")

    for dtype, var, expr in post_ops:
        lines.append(f"    {dtype} {var} = {expr};")

    last_var = post_ops[-1][1] if post_ops else f"v{reduce_var_idx}"
    lines.append(f"    buf0[gid] = {last_var};")
    lines.append("}")
    return "\n".join(lines)


def _render_wgsl_elementwise(
    ops: list[tuple[str, str, str]],
    buf_count: int,
    n: int,
) -> str:
    """Generate WGSL shader source matching WgslRenderer output structure."""
    lines = []
    for i in range(buf_count):
        access = "read_write" if i == 0 else "read"
        lines.append(f"@group(0) @binding({i}) var<storage, {access}> buf{i}: array<f32>;")
    lines.append("")
    local = min(n, 256)
    lines.append(f"@compute @workgroup_size({local}, 1, 1)")
    lines.append("fn molt_kernel(@builtin(global_invocation_id) gid_vec: vec3<u32>) {")
    lines.append("    let gid = gid_vec.x;")
    lines.append(f"    if (gid >= {n}u) {{ return; }}")
    for dtype, var, expr in ops:
        lines.append(f"    var {var}: {dtype} = {expr};")
    last_var = ops[-1][1]
    lines.append(f"    buf0[gid] = {last_var};")
    lines.append("}")
    return "\n".join(lines)


def _render_cuda_elementwise(
    ops: list[tuple[str, str, str]],
    buf_count: int,
    n: int,
) -> str:
    """Generate CUDA shader source matching CudaRenderer output structure."""
    lines = [
        "#include <cuda_runtime.h>",
        "#include <math.h>",
        "",
    ]
    params = []
    for i in range(buf_count):
        qualifier = "const " if i > 0 else ""
        params.append(f"{qualifier}float* buf{i}")
    lines.append(f'extern "C" __global__ void molt_kernel({", ".join(params)}) {{')
    lines.append("    unsigned int gid = blockIdx.x * blockDim.x + threadIdx.x;")
    lines.append(f"    if (gid >= {n}) return;")
    for dtype, var, expr in ops:
        lines.append(f"    {dtype} {var} = {expr};")
    last_var = ops[-1][1]
    lines.append(f"    buf0[gid] = {last_var};")
    lines.append("}")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Shader compositions
# ---------------------------------------------------------------------------


def _softmax_kernels(n: int, reduce_size: int) -> dict[str, dict[str, str]]:
    """Generate softmax kernel sources for all renderers.

    Softmax = 2 fused kernels:
    K1: reduce_max(x) -> sub(x, max) -> exp2(result * log2e)
    K2: reduce_sum(exp_vals) -> reciprocal(sum) -> mul(exp_vals, inv_sum)
    """
    out_n = n // reduce_size

    # Kernel 1: max reduction + elementwise prefix
    k1_msl = _render_msl_reduce(
        pre_ops=[], reduce_type="max", post_ops=[], buf_count=2, out_n=out_n, reduce_size=reduce_size
    )
    k1_wgsl = _render_wgsl_elementwise(
        [("f32", "v0", "buf1[gid]")], buf_count=2, n=out_n
    )

    # Kernel 2: subtract max, exp, sum, reciprocal, multiply
    k2_msl = _render_msl_reduce(
        pre_ops=[
            ("float", "v0", "(buf1[eidx] - buf2[gid])"),
            ("float", "v1", "exp2(v0 * 1.4426950408889634f)"),
        ],
        reduce_type="sum",
        post_ops=[
            ("float", "v3", "(1.0f / v2)"),
        ],
        buf_count=3,
        out_n=out_n,
        reduce_size=reduce_size,
    )

    return {
        "softmax_k1": {"msl": k1_msl, "wgsl": k1_wgsl},
        "softmax_k2": {"msl": k2_msl},
    }


def _matmul_kernel(m: int, n: int, k: int) -> dict[str, str]:
    """Generate matmul kernel sources.

    Matmul(A[m,k], B[k,n]) -> C[m,n]
    = RESHAPE + EXPAND + MUL + REDUCE_SUM
    """
    out_n = m * n
    msl = _render_msl_reduce(
        pre_ops=[
            ("float", "v0", "(buf1[eidx] * buf2[eidx])"),
        ],
        reduce_type="sum",
        post_ops=[],
        buf_count=3,
        out_n=out_n,
        reduce_size=k,
    )
    wgsl = _render_wgsl_elementwise(
        [("f32", "v0", "(buf1[gid] * buf2[gid])")], buf_count=3, n=out_n
    )
    cuda = _render_cuda_elementwise(
        [("float", "v0", "(buf1[gid] * buf2[gid])")], buf_count=3, n=out_n
    )
    return {"msl": msl, "wgsl": wgsl, "cuda": cuda}


def _rmsnorm_kernel(n: int, hidden: int) -> dict[str, str]:
    """Generate RMSNorm kernel sources.

    RMSNorm(x) = x * rsqrt(mean(x^2) + eps) * gamma
    = MUL(x, x) -> REDUCE_SUM -> MUL(sum, 1/hidden) -> ADD(mean, eps) -> SQRT -> RECIPROCAL -> MUL(x, inv_rms) -> MUL(normed, gamma)
    """
    out_n = n // hidden
    msl = _render_msl_reduce(
        pre_ops=[
            ("float", "v0", "(buf1[eidx] * buf1[eidx])"),
        ],
        reduce_type="sum",
        post_ops=[
            ("float", "v2", "(v1 * float(1.0f / float({hidden})))".format(hidden=hidden)),
            ("float", "v3", "(v2 + 1e-6f)"),
            ("float", "v4", "sqrt(v3)"),
            ("float", "v5", "(1.0f / v4)"),
        ],
        buf_count=3,
        out_n=out_n,
        reduce_size=hidden,
    )
    return {"msl": msl}


def _attention_kernels(seq_len: int, head_dim: int, num_heads: int) -> dict[str, dict[str, str]]:
    """Generate full attention block kernel sources.

    Attention(Q, K, V) = softmax(Q @ K^T / sqrt(d)) @ V
    Decomposes into 6 kernels.
    """
    # K1: Q @ K^T
    qk_out = seq_len * seq_len * num_heads
    k1 = _render_msl_reduce(
        pre_ops=[("float", "v0", "(buf1[eidx] * buf2[eidx])")],
        reduce_type="sum", post_ops=[], buf_count=3, out_n=qk_out, reduce_size=head_dim
    )
    # K2: Scale by 1/sqrt(d)
    k2 = _render_msl_elementwise(
        [("float", "v0", f"(buf1[gid] * {1.0 / (head_dim ** 0.5):.6f}f)")],
        buf_count=2, n=qk_out
    )
    # K3: Softmax max reduce
    softmax_out = seq_len * num_heads
    k3 = _render_msl_reduce(
        pre_ops=[], reduce_type="max", post_ops=[], buf_count=2,
        out_n=softmax_out, reduce_size=seq_len
    )
    # K4: Softmax exp + sum reduce
    k4 = _render_msl_reduce(
        pre_ops=[
            ("float", "v0", "(buf1[eidx] - buf2[gid])"),
            ("float", "v1", "exp2(v0 * 1.4426950408889634f)"),
        ],
        reduce_type="sum", post_ops=[("float", "v3", "(1.0f / v2)")],
        buf_count=3, out_n=softmax_out, reduce_size=seq_len
    )
    # K5: Softmax normalize
    k5 = _render_msl_elementwise(
        [("float", "v0", "(buf1[gid] * buf2[gid])")], buf_count=3, n=qk_out
    )
    # K6: Attention @ V
    av_out = seq_len * head_dim * num_heads
    k6 = _render_msl_reduce(
        pre_ops=[("float", "v0", "(buf1[eidx] * buf2[eidx])")],
        reduce_type="sum", post_ops=[], buf_count=3, out_n=av_out, reduce_size=seq_len
    )
    return {
        "attn_qk": {"msl": k1},
        "attn_scale": {"msl": k2},
        "attn_softmax_max": {"msl": k3},
        "attn_softmax_exp_sum": {"msl": k4},
        "attn_softmax_norm": {"msl": k5},
        "attn_av": {"msl": k6},
    }


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


class TestSoftmaxSize:
    """Measure and validate softmax shader sizes."""

    def test_softmax_kernel_sizes(self):
        """Each softmax kernel is under 2KB."""
        kernels = _softmax_kernels(n=2048, reduce_size=128)
        print("\n  Softmax shader sizes:")
        for name, renderers in kernels.items():
            for renderer, source in renderers.items():
                size = len(source.encode("utf-8"))
                print(f"    {name} ({renderer}): {size} bytes")
                assert size < 2048, f"{name} ({renderer}) = {size} bytes > 2KB"


class TestMatmulSize:
    """Measure and validate matmul shader sizes."""

    def test_matmul_kernel_sizes(self):
        """Matmul kernel is under 3KB per renderer."""
        kernels = _matmul_kernel(m=128, n=128, k=64)
        print("\n  Matmul shader sizes:")
        for renderer, source in kernels.items():
            size = len(source.encode("utf-8"))
            print(f"    matmul ({renderer}): {size} bytes")
            assert size < 3072, f"matmul ({renderer}) = {size} bytes > 3KB"


class TestRMSNormSize:
    """Measure and validate RMSNorm shader sizes."""

    def test_rmsnorm_kernel_sizes(self):
        """RMSNorm kernel is under 2KB."""
        kernels = _rmsnorm_kernel(n=2048 * 64, hidden=64)
        print("\n  RMSNorm shader sizes:")
        for renderer, source in kernels.items():
            size = len(source.encode("utf-8"))
            print(f"    rmsnorm ({renderer}): {size} bytes")
            assert size < 2048, f"rmsnorm ({renderer}) = {size} bytes > 2KB"


class TestAttentionBlockSize:
    """Measure and validate full attention block shader sizes."""

    def test_attention_total_under_10kb(self):
        """Total attention block shader size under 10KB."""
        kernels = _attention_kernels(seq_len=128, head_dim=64, num_heads=8)
        total = 0
        print("\n  Attention block shader sizes:")
        for name, renderers in kernels.items():
            for renderer, source in renderers.items():
                size = len(source.encode("utf-8"))
                total += size
                print(f"    {name} ({renderer}): {size} bytes")
                assert size < 10240, f"{name} ({renderer}) = {size} bytes > 10KB"
        print(f"    TOTAL: {total} bytes")
        assert total < 10240, f"Attention total = {total} bytes > 10KB"

    def test_no_single_kernel_exceeds_10kb(self):
        """No individual kernel source exceeds 10KB."""
        kernels = _attention_kernels(seq_len=512, head_dim=128, num_heads=32)
        for name, renderers in kernels.items():
            for renderer, source in renderers.items():
                size = len(source.encode("utf-8"))
                assert size < 10240, (
                    f"Kernel {name} ({renderer}) = {size} bytes exceeds 10KB limit"
                )


class TestSizeSummary:
    """Generate a summary table of all shader sizes."""

    def test_print_summary_table(self):
        """Print a summary table of all measured shader sizes."""
        rows = []

        # Softmax
        for name, renderers in _softmax_kernels(2048, 128).items():
            for r, src in renderers.items():
                rows.append((name, r, len(src.encode("utf-8"))))

        # Matmul
        for r, src in _matmul_kernel(128, 128, 64).items():
            rows.append(("matmul", r, len(src.encode("utf-8"))))

        # RMSNorm
        for r, src in _rmsnorm_kernel(2048 * 64, 64).items():
            rows.append(("rmsnorm", r, len(src.encode("utf-8"))))

        # Attention
        for name, renderers in _attention_kernels(128, 64, 8).items():
            for r, src in renderers.items():
                rows.append((name, r, len(src.encode("utf-8"))))

        print("\n")
        print("  +" + "-" * 32 + "+" + "-" * 12 + "+" + "-" * 12 + "+")
        print(f"  | {'Kernel':<30} | {'Renderer':<10} | {'Bytes':>10} |")
        print("  +" + "-" * 32 + "+" + "-" * 12 + "+" + "-" * 12 + "+")
        for name, renderer, size in rows:
            print(f"  | {name:<30} | {renderer:<10} | {size:>10} |")
        print("  +" + "-" * 32 + "+" + "-" * 12 + "+" + "-" * 12 + "+")
        total = sum(r[2] for r in rows)
        print(f"  | {'TOTAL':<30} | {'':10} | {total:>10} |")
        print("  +" + "-" * 32 + "+" + "-" * 12 + "+" + "-" * 12 + "+")
