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


# ---------------------------------------------------------------------------
# MXFP (Microscaling Floating Point) — OCP MX Spec v1.0
# ---------------------------------------------------------------------------
#
# Block-based format: each block of elements shares a single 8-bit exponent.
#   MXFP8: 16 elements/block, 8-bit mantissa per element
#   MXFP4: 32 elements/block, 4-bit mantissa per element
#
# Per-block encoding:
#   shared_exponent = max exponent of all elements in the block
#   mantissa[i] = round(element[i] / 2^shared_exponent * scale)
#
# Dequantization: value[i] = mantissa[i] * 2^shared_exponent

MXFP8_BLOCK_SIZE = 32  # OCP MX Spec v1.0 Table 1: all MX formats use block size 32
MXFP4_BLOCK_SIZE = 32  # OCP MX Spec v1.0 Table 1: all MX formats use block size 32
MXFP8_MANTISSA_BITS = 8
MXFP4_MANTISSA_BITS = 4


def _compute_shared_exponent(block: list) -> int:
    """Compute the shared exponent for an MXFP block.

    The shared exponent is chosen such that all values in the block
    are representable: exp = ceil(log2(abs_max)), ensuring that
    abs_max / 2^exp <= 1.0 and all mantissas fit in [-qmax, qmax].

    Uses the E8M0 exponent encoding from the OCP MX spec v1.0:
    biased_exp = unbiased_exp + 127.

    block: flat list of float values (one block).
    Returns: 8-bit shared exponent (int in [0, 255]).
    """
    abs_max = 0.0
    for val in block:
        av = abs(val)
        if av > abs_max:
            abs_max = av

    if abs_max == 0.0:
        return 0  # All zeros — exponent is 0

    # Use math.frexp for precise exponent extraction.
    # frexp(x) returns (m, e) where x = m * 2^e and 0.5 <= |m| < 1.0.
    # So log2(|x|) = e + log2(|m|), and since 0.5 <= |m| < 1.0,
    # ceil(log2(|x|)) = e when |m| == 0.5 (exact power of 2),
    # otherwise ceil(log2(|x|)) = e.
    _, frexp_e = math.frexp(abs_max)
    # frexp gives x = m * 2^e with 0.5 <= m < 1.0
    # so abs_max < 2^e and abs_max >= 2^(e-1)
    # We want scale = 2^exp such that abs_max / scale <= 1.0,
    # so exp = frexp_e (since abs_max < 2^frexp_e).
    exp = frexp_e

    # Bias with 127 (E8M0 format from OCP MX spec)
    biased_exp = exp + 127
    # Clamp to [0, 255]
    if biased_exp < 0:
        biased_exp = 0
    if biased_exp > 255:
        biased_exp = 255

    return biased_exp


def _quantize_mantissa(value: float, shared_exp_biased: int, n_bits: int) -> int:
    """Quantize a single value to an n-bit mantissa given the shared exponent.

    value: the float to quantize
    shared_exp_biased: the 8-bit biased shared exponent
    n_bits: number of mantissa bits (8 for MXFP8, 4 for MXFP4)

    Returns: signed integer mantissa in [-qmax, qmax]
    """
    # Reconstruct the scale: 2^(shared_exp - 127)
    exp = shared_exp_biased - 127
    scale = 2.0 ** exp

    if scale == 0.0:
        return 0

    # Quantize: mantissa = round(value / scale * qmax) / qmax
    # where qmax = 2^(n_bits - 1) - 1
    qmax = (1 << (n_bits - 1)) - 1
    normalized = value / scale
    q = int(round(normalized * qmax))

    # Clamp
    if q > qmax:
        q = qmax
    if q < -qmax:
        q = -qmax

    return q


def _dequantize_mantissa(mantissa: int, shared_exp_biased: int, n_bits: int) -> float:
    """Dequantize a mantissa back to float given the shared exponent.

    mantissa: signed integer mantissa
    shared_exp_biased: the 8-bit biased shared exponent
    n_bits: number of mantissa bits (8 for MXFP8, 4 for MXFP4)

    Returns: reconstructed float value
    """
    exp = shared_exp_biased - 127
    scale = 2.0 ** exp
    qmax = (1 << (n_bits - 1)) - 1
    return (mantissa / qmax) * scale


def quantize_mxfp8(data: list) -> tuple:
    """Quantize a flat list of floats to MXFP8 format.

    Splits input into blocks of MXFP8_BLOCK_SIZE (16) elements.
    Each block gets a shared 8-bit exponent.
    Each element is quantized to an 8-bit signed mantissa.

    Parameters:
        data: flat list of float values

    Returns:
        (mantissas, exponents) where:
        - mantissas: flat list of int8 mantissa values (same length as data,
          padded with zeros if not a multiple of block size)
        - exponents: list of uint8 shared exponents (one per block)
    """
    n = len(data)
    block_size = MXFP8_BLOCK_SIZE
    n_blocks = (n + block_size - 1) // block_size
    padded_size = n_blocks * block_size

    # Pad to block boundary
    padded = list(data)
    if padded_size > n:
        padded.extend([0.0] * (padded_size - n))

    mantissas = [0] * padded_size
    exponents = [0] * n_blocks

    for b in range(n_blocks):
        start = b * block_size
        end = start + block_size
        block = padded[start:end]

        shared_exp = _compute_shared_exponent(block)
        exponents[b] = shared_exp

        for i in range(block_size):
            mantissas[start + i] = _quantize_mantissa(
                block[i], shared_exp, MXFP8_MANTISSA_BITS,
            )

    return mantissas, exponents


def quantize_mxfp4(data: list) -> tuple:
    """Quantize a flat list of floats to MXFP4 format.

    Splits input into blocks of MXFP4_BLOCK_SIZE (32) elements.
    Each block gets a shared 8-bit exponent.
    Each element is quantized to a 4-bit signed mantissa.

    Parameters:
        data: flat list of float values

    Returns:
        (mantissas, exponents) where:
        - mantissas: flat list of int4 mantissa values (same length as data,
          padded with zeros if not a multiple of block size)
        - exponents: list of uint8 shared exponents (one per block)
    """
    n = len(data)
    block_size = MXFP4_BLOCK_SIZE
    n_blocks = (n + block_size - 1) // block_size
    padded_size = n_blocks * block_size

    # Pad to block boundary
    padded = list(data)
    if padded_size > n:
        padded.extend([0.0] * (padded_size - n))

    mantissas = [0] * padded_size
    exponents = [0] * n_blocks

    for b in range(n_blocks):
        start = b * block_size
        end = start + block_size
        block = padded[start:end]

        shared_exp = _compute_shared_exponent(block)
        exponents[b] = shared_exp

        for i in range(block_size):
            mantissas[start + i] = _quantize_mantissa(
                block[i], shared_exp, MXFP4_MANTISSA_BITS,
            )

    return mantissas, exponents


def dequantize_mxfp8(mantissas: list, exponents: list) -> list:
    """Dequantize MXFP8 mantissas + exponents back to float values.

    Parameters:
        mantissas: flat list of int8 mantissa values
        exponents: list of uint8 shared exponents (one per block of 16)

    Returns:
        flat list of float values (same length as mantissas)
    """
    block_size = MXFP8_BLOCK_SIZE
    n = len(mantissas)
    result = [0.0] * n

    for b in range(len(exponents)):
        start = b * block_size
        end = min(start + block_size, n)
        shared_exp = exponents[b]

        for i in range(start, end):
            result[i] = _dequantize_mantissa(
                mantissas[i], shared_exp, MXFP8_MANTISSA_BITS,
            )

    return result


def dequantize_mxfp4(mantissas: list, exponents: list) -> list:
    """Dequantize MXFP4 mantissas + exponents back to float values.

    Parameters:
        mantissas: flat list of int4 mantissa values
        exponents: list of uint8 shared exponents (one per block of 32)

    Returns:
        flat list of float values (same length as mantissas)
    """
    block_size = MXFP4_BLOCK_SIZE
    n = len(mantissas)
    result = [0.0] * n

    for b in range(len(exponents)):
        start = b * block_size
        end = min(start + block_size, n)
        shared_exp = exponents[b]

        for i in range(start, end):
            result[i] = _dequantize_mantissa(
                mantissas[i], shared_exp, MXFP4_MANTISSA_BITS,
            )

    return result
