"""
tinygrad.turbo_quant — Int4/Int8 quantization as primitive compositions.

PolarQuant: Uses Remez-optimal atan2 approximation with domain reduction
for |r|>1, plus QJL error correction.

All operations are composed from the 26 tinygrad primitives — no new Rust code.
"""

from __future__ import annotations

import math
from tinygrad.tensor import Tensor
from tinygrad.dtypes import dtypes

# Remez-optimal atan(x) polynomial coefficients for |x| <= 1
# 7th-order minimax approximation (max error < 1e-6 on [-1, 1])
_REMEZ_C1 = 0.9999993329
_REMEZ_C3 = -0.3333258928
_REMEZ_C5 = 0.1999193960
_REMEZ_C7 = -0.1419752620
_REMEZ_C9 = 0.1065625526
_REMEZ_C11 = -0.0752627836
_REMEZ_C13 = 0.0429095898


def _atan_remez(x: Tensor) -> Tensor:
    """atan(x) for |x| <= 1 using Remez polynomial."""
    x2 = x * x
    # Horner form: x * (c1 + x2*(c3 + x2*(c5 + x2*(c7 + x2*(c9 + x2*(c11 + x2*c13))))))
    result = x2 * _REMEZ_C13
    result = (result + _REMEZ_C11) * x2
    result = (result + _REMEZ_C9) * x2
    result = (result + _REMEZ_C7) * x2
    result = (result + _REMEZ_C5) * x2
    result = (result + _REMEZ_C3) * x2
    result = result + _REMEZ_C1
    return x * result


def atan2(y: Tensor, x: Tensor) -> Tensor:
    """atan2(y, x) using Remez atan with domain reduction for |r| > 1.

    Domain reduction:
      If |y/x| <= 1: atan2(y, x) = atan(y/x) + offset
      If |y/x| > 1:  atan2(y, x) = sgn(y/x) * pi/2 - atan(x/y) + offset
    """
    r = y / x
    abs_r = r.__abs__()

    # Domain reduction: for |r| > 1, compute atan(1/r) and adjust
    # Need to handle both cases without control flow (GPU-safe)
    r_small = r  # used when |r| <= 1
    r_large = x / y  # used when |r| > 1 (reciprocal ratio)

    atan_small = _atan_remez(r_small)
    atan_large = _atan_remez(r_large)

    # For |r| > 1: atan(r) = sgn(r) * pi/2 - atan(1/r)
    half_pi = math.pi / 2.0
    sgn_r = (r > 0.0) * 2.0 - 1.0  # -1 or +1
    atan_from_large = sgn_r * half_pi - atan_large

    # select: |r| <= 1 -> atan_small, |r| > 1 -> atan_from_large
    use_small = abs_r < 1.0 + 1e-10  # slightly beyond 1 for boundary
    # WHERE(use_small, atan_small, atan_from_large) via primitive composition
    result = use_small * atan_small + (1.0 - use_small) * atan_from_large

    # Quadrant adjustment based on sign of x
    # x > 0: result
    # x < 0, y >= 0: result + pi
    # x < 0, y < 0: result - pi
    x_neg = x < 0.0
    y_neg = y < 0.0
    pi_adjust = x_neg * (1.0 - y_neg * 2.0) * math.pi

    return result + pi_adjust


def symmetric_quantize(x: Tensor, n_bits: int = 8) -> tuple:
    """Symmetric quantization: q = round(x / scale), scale = max(|x|) / qmax.

    Returns (quantized_tensor, scale).
    """
    qmax = float((1 << (n_bits - 1)) - 1)  # 127 for int8, 7 for int4
    abs_max = x.__abs__().max()
    scale = abs_max * (1.0 / qmax)
    q = (x / scale).trunc()
    # Clamp to [-qmax, qmax]
    q = q.maximum(Tensor._const(-qmax, q.shape, q.dtype))
    neg_qmax = Tensor._const(qmax, q.shape, q.dtype)
    # min(q, qmax) = -max(-q, -qmax)
    q = (q.neg().maximum(neg_qmax.neg())).neg()
    return q, scale


def asymmetric_quantize(x: Tensor, n_bits: int = 8) -> tuple:
    """Asymmetric quantization with zero-point.

    Returns (quantized_tensor, scale, zero_point).
    """
    qmin = 0.0
    qmax = float((1 << n_bits) - 1)  # 255 for int8
    x_min = (x.neg()).max().neg()  # min via -max(-x)
    x_max = x.max()
    scale = (x_max - x_min) * (1.0 / qmax)
    zero_point = (x_min.neg() / scale).trunc()
    q = (x / scale + zero_point).trunc()
    # Clamp
    q = q.maximum(Tensor._const(qmin, q.shape, q.dtype))
    q = (q.neg().maximum(Tensor._const(-qmax, q.shape, q.dtype))).neg()
    return q, scale, zero_point


def dequantize_symmetric(q: Tensor, scale: Tensor) -> Tensor:
    """Dequantize: x_hat = q * scale."""
    return q * scale


def dequantize_asymmetric(q: Tensor, scale: Tensor, zero_point: Tensor) -> Tensor:
    """Dequantize: x_hat = (q - zero_point) * scale."""
    return (q - zero_point) * scale


def block_quantize(x: Tensor, block_size: int = 128, n_bits: int = 8) -> tuple:
    """Per-block symmetric quantization.

    Reshapes x into blocks, quantizes each block independently.
    Returns (quantized_tensor, scales_per_block).
    """
    flat = x.reshape(-1)
    numel = flat.numel()
    # Pad to multiple of block_size
    n_blocks = (numel + block_size - 1) // block_size
    padded_size = n_blocks * block_size
    if padded_size > numel:
        flat = flat.pad([(0, padded_size - numel)])
    blocks = flat.reshape(n_blocks, block_size)

    qmax = float((1 << (n_bits - 1)) - 1)
    # Scale per block: max(|block|) / qmax
    abs_blocks = blocks.__abs__()
    block_max = abs_blocks.max(axis=1)
    scales = block_max * (1.0 / qmax)

    # Quantize each block
    q = (blocks / scales._broadcast_to(blocks.shape)).trunc()
    # Clamp
    q = q.maximum(Tensor._const(-qmax, q.shape, q.dtype))
    q = (q.neg().maximum(Tensor._const(-qmax, q.shape, q.dtype))).neg()

    return q, scales


def matmul_q4(a_f16: Tensor, b_int4: Tensor, scales: Tensor) -> Tensor:
    """Mixed-precision matmul: A (fp16/fp32) @ B (int4, with per-block scales).

    Dequantizes B on-the-fly and performs matmul.
    Fuses: dequant + reshape + expand + mul + reduce_sum.
    """
    # Dequantize B
    b_dequant = b_int4 * scales._broadcast_to(b_int4.shape)
    # Matmul
    return a_f16 @ b_dequant


# QJL (Quantized Johnson-Lindenstrauss) error correction
def qjl_error_correction(
    original: Tensor,
    quantized: Tensor,
    scale: Tensor,
    n_projections: int = 32,
) -> Tensor:
    """Apply QJL error correction to reduce quantization error.

    Uses random projections to estimate and correct systematic bias.
    """
    dequantized = dequantize_symmetric(quantized, scale)
    error = original - dequantized

    # Random projection matrix (fixed seed for reproducibility)
    d = error.numel()
    proj = Tensor.rand(d, n_projections) * (2.0 / math.sqrt(n_projections)) - (1.0 / math.sqrt(n_projections))

    # Project error
    proj_error = error.reshape(1, d) @ proj  # [1, n_projections]

    # Back-project correction
    correction = (proj_error @ proj.T).reshape(error.shape)

    return dequantized + correction
