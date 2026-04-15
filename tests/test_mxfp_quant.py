"""Tests for MXFP (Microscaling Floating Point) quantization.

Verifies quantize/dequantize roundtrip accuracy for MXFP8 and MXFP4
formats per OCP Microscaling Formats Specification v1.0.
"""

import math
import sys
import os
import importlib.util
import types

# Load turbo_quant.py directly, bypassing molt's import system.
# The MXFP functions use only plain Python (no Tensor/LazyOp), so
# we can extract them without the runtime being active.
_tq_path = os.path.join(
    os.path.dirname(__file__), "..", "src", "molt", "stdlib", "tinygrad", "turbo_quant.py"
)
_tq_path = os.path.abspath(_tq_path)

# Read the source, extract only the MXFP section (which has no imports)
with open(_tq_path) as f:
    _full_source = f.read()

# Find the MXFP section — it starts after the QJL function
_mxfp_marker = "# MXFP (Microscaling Floating Point)"
_mxfp_start = _full_source.index(_mxfp_marker)
_mxfp_source = _full_source[_mxfp_start:]

# Execute the MXFP section in a clean namespace
_ns = {"__builtins__": __builtins__, "math": math}
exec(compile(_mxfp_source, _tq_path, "exec"), _ns)

MXFP8_BLOCK_SIZE = _ns["MXFP8_BLOCK_SIZE"]
MXFP4_BLOCK_SIZE = _ns["MXFP4_BLOCK_SIZE"]
MXFP8_MANTISSA_BITS = _ns["MXFP8_MANTISSA_BITS"]
MXFP4_MANTISSA_BITS = _ns["MXFP4_MANTISSA_BITS"]
_compute_shared_exponent = _ns["_compute_shared_exponent"]
_quantize_mantissa = _ns["_quantize_mantissa"]
_dequantize_mantissa = _ns["_dequantize_mantissa"]
quantize_mxfp8 = _ns["quantize_mxfp8"]
quantize_mxfp4 = _ns["quantize_mxfp4"]
dequantize_mxfp8 = _ns["dequantize_mxfp8"]
dequantize_mxfp4 = _ns["dequantize_mxfp4"]


def _max_relative_error(original, reconstructed):
    """Compute max relative error between two lists, ignoring zeros."""
    max_err = 0.0
    for o, r in zip(original, reconstructed):
        if abs(o) < 1e-30:
            continue
        err = abs(o - r) / abs(o)
        if err > max_err:
            max_err = err
    return max_err


# --- Block size constants ---

def test_block_sizes():
    assert MXFP8_BLOCK_SIZE == 16
    assert MXFP4_BLOCK_SIZE == 32


def test_mantissa_bits():
    assert MXFP8_MANTISSA_BITS == 8
    assert MXFP4_MANTISSA_BITS == 4


# --- Shared exponent computation ---

def test_shared_exponent_zeros():
    block = [0.0] * 16
    assert _compute_shared_exponent(block) == 0


def test_shared_exponent_ones():
    block = [1.0] * 16
    exp = _compute_shared_exponent(block)
    # frexp(1.0) = (0.5, 1), so exp=1, biased = 128
    # Scale = 2^1 = 2.0, and 1.0/2.0 = 0.5 which is representable.
    assert exp == 128


def test_shared_exponent_power_of_two():
    block = [0.0] * 15 + [8.0]
    exp = _compute_shared_exponent(block)
    # frexp(8.0) = (0.5, 4), so exp=4, biased = 131
    assert exp == 131


def test_shared_exponent_small_values():
    block = [0.001] * 16
    exp = _compute_shared_exponent(block)
    # 0.001 ~ 2^(-10), biased ~117
    assert 115 <= exp <= 120


def test_shared_exponent_negative_values():
    block = [-4.0, 2.0] + [0.0] * 14
    exp = _compute_shared_exponent(block)
    # frexp(4.0) = (0.5, 3), so exp=3, biased = 130
    assert exp == 130


# --- MXFP8 quantize/dequantize roundtrip ---

def test_mxfp8_roundtrip_zeros():
    data = [0.0] * 16
    mantissas, exponents = quantize_mxfp8(data)
    recon = dequantize_mxfp8(mantissas, exponents)
    assert len(mantissas) == 16
    assert len(exponents) == 1
    for v in recon:
        assert v == 0.0


def test_mxfp8_roundtrip_ones():
    data = [1.0] * 16
    mantissas, exponents = quantize_mxfp8(data)
    recon = dequantize_mxfp8(mantissas, exponents)
    for v in recon:
        assert abs(v - 1.0) < 0.02  # MXFP8 ~1% relative error


def test_mxfp8_roundtrip_accuracy():
    """MXFP8 should achieve < 2% relative error for uniform-magnitude data.

    When all values in a block have similar magnitudes, the shared exponent
    is well-suited and quantization error is minimal. This is the expected
    use case (e.g., neural network weight blocks).
    """
    # Uniform magnitude: all values between 0.8 and 1.2.
    data = [0.8 + 0.025 * i for i in range(16)]
    mantissas, exponents = quantize_mxfp8(data)
    recon = dequantize_mxfp8(mantissas, exponents)
    max_err = _max_relative_error(data, recon)
    assert max_err < 0.02, f"MXFP8 relative error {max_err} exceeds 2%"


def test_mxfp8_roundtrip_negative():
    data = [-2.5, -1.0, 0.5, 3.0] + [0.0] * 12
    mantissas, exponents = quantize_mxfp8(data)
    recon = dequantize_mxfp8(mantissas, exponents)
    max_err = _max_relative_error(data[:4], recon[:4])
    assert max_err < 0.02


def test_mxfp8_padding():
    """Non-multiple-of-16 input should be zero-padded."""
    data = [1.0] * 10
    mantissas, exponents = quantize_mxfp8(data)
    # Padded to 16 elements
    assert len(mantissas) == 16
    assert len(exponents) == 1
    recon = dequantize_mxfp8(mantissas, exponents)
    for i in range(10):
        assert abs(recon[i] - 1.0) < 0.02
    # Padded positions should be ~0
    for i in range(10, 16):
        assert abs(recon[i]) < 0.02


def test_mxfp8_multi_block():
    """Multiple blocks should each get their own exponent."""
    data = [1.0] * 16 + [100.0] * 16
    mantissas, exponents = quantize_mxfp8(data)
    assert len(mantissas) == 32
    assert len(exponents) == 2
    # Exponents should differ (different magnitudes per block)
    assert exponents[0] != exponents[1]
    recon = dequantize_mxfp8(mantissas, exponents)
    for i in range(16):
        assert abs(recon[i] - 1.0) < 0.02
    for i in range(16, 32):
        assert abs(recon[i] - 100.0) < 2.0  # 2% of 100


def test_mxfp8_large_values():
    data = [1e6] * 16
    mantissas, exponents = quantize_mxfp8(data)
    recon = dequantize_mxfp8(mantissas, exponents)
    max_err = _max_relative_error(data, recon)
    assert max_err < 0.02


# --- MXFP4 quantize/dequantize roundtrip ---

def test_mxfp4_roundtrip_zeros():
    data = [0.0] * 32
    mantissas, exponents = quantize_mxfp4(data)
    recon = dequantize_mxfp4(mantissas, exponents)
    assert len(mantissas) == 32
    assert len(exponents) == 1
    for v in recon:
        assert v == 0.0


def test_mxfp4_roundtrip_ones():
    data = [1.0] * 32
    mantissas, exponents = quantize_mxfp4(data)
    recon = dequantize_mxfp4(mantissas, exponents)
    for v in recon:
        assert abs(v - 1.0) < 0.2  # MXFP4 ~15% relative error


def test_mxfp4_roundtrip_accuracy():
    """MXFP4 has lower precision — expect < 20% relative error for uniform data.

    With only 4-bit mantissas (7 quantization levels), error is inherently
    higher than MXFP8. Uniform-magnitude blocks minimize the dynamic range
    penalty from shared exponents.
    """
    # Uniform magnitude: all values between 0.5 and 1.5.
    data = [0.5 + (1.0 / 32) * i for i in range(32)]
    mantissas, exponents = quantize_mxfp4(data)
    recon = dequantize_mxfp4(mantissas, exponents)
    max_err = _max_relative_error(data, recon)
    assert max_err < 0.20, f"MXFP4 relative error {max_err} exceeds 20%"


def test_mxfp4_padding():
    data = [2.0] * 20
    mantissas, exponents = quantize_mxfp4(data)
    assert len(mantissas) == 32  # padded to 32
    assert len(exponents) == 1


def test_mxfp4_multi_block():
    data = [0.5] * 32 + [50.0] * 32
    mantissas, exponents = quantize_mxfp4(data)
    assert len(exponents) == 2
    assert exponents[0] != exponents[1]


def test_mxfp4_mantissa_range():
    """MXFP4 mantissas should be in [-7, 7]."""
    data = [100.0, -100.0, 50.0, -50.0] + [0.0] * 28
    mantissas, exponents = quantize_mxfp4(data)
    qmax = (1 << (MXFP4_MANTISSA_BITS - 1)) - 1  # 7
    for m in mantissas:
        assert -qmax <= m <= qmax, f"mantissa {m} out of range"


# --- Edge cases ---

def test_mxfp8_single_element():
    data = [42.0]
    mantissas, exponents = quantize_mxfp8(data)
    recon = dequantize_mxfp8(mantissas, exponents)
    assert abs(recon[0] - 42.0) < 1.0  # ~2% of 42


def test_mxfp4_single_element():
    data = [42.0]
    mantissas, exponents = quantize_mxfp4(data)
    recon = dequantize_mxfp4(mantissas, exponents)
    assert abs(recon[0] - 42.0) < 10.0  # MXFP4 lower precision


def test_mxfp8_mixed_magnitudes():
    """Block with mixed magnitudes: small values lose precision."""
    data = [0.001, 100.0] + [1.0] * 14
    mantissas, exponents = quantize_mxfp8(data)
    recon = dequantize_mxfp8(mantissas, exponents)
    # The large value (100.0) should be well-represented.
    assert abs(recon[1] - 100.0) < 2.0
    # The small value (0.001) will have high relative error due to shared exponent.
    # This is expected behavior for MXFP — it's a known tradeoff.


def test_quantize_mantissa_clamp():
    """Mantissa should be clamped to [-qmax, qmax]."""
    # With shared_exp for 1.0 (biased 127), quantizing 1000.0 should clamp
    q = _quantize_mantissa(1000.0, 127, 8)
    qmax = (1 << 7) - 1  # 127
    assert -qmax <= q <= qmax


# --- Symmetry tests ---

def test_mxfp8_symmetry():
    """Quantizing -x should give the negative of quantizing x."""
    data_pos = [2.0] * 16
    data_neg = [-2.0] * 16
    m_pos, e_pos = quantize_mxfp8(data_pos)
    m_neg, e_neg = quantize_mxfp8(data_neg)
    assert e_pos == e_neg  # Same exponents
    for mp, mn in zip(m_pos, m_neg):
        assert mp == -mn  # Symmetric mantissas


if __name__ == "__main__":
    import pytest
    pytest.main([__file__, "-v"])
