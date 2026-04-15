# Falcon-OCR End-to-End Parity Tests

## Overview

This test suite verifies that molt's Falcon-OCR implementation produces
identical results to the reference CPython+tinygrad path. All tests use
deterministic stub weights (no real model weights required).

## Files

| File | Purpose |
|------|---------|
| `falcon_ocr_stub_weights.py` | Deterministic stub weight generator (SafeTensors format) |
| `falcon_ocr_baseline.py` | CLI harness for generating and comparing reference/molt outputs |
| `test_falcon_ocr_parity.py` | Pytest suite for numerical parity of all primitives |
| `test_falcon_ocr_targets.py` | Pytest suite for cross-target parity (CPU, Metal, MSL, WGSL, CUDA) |

## Quick Start

### Run the parity tests

```bash
python -m pytest tests/e2e/test_falcon_ocr_parity.py -v
```

### Run the target matrix tests

```bash
python -m pytest tests/e2e/test_falcon_ocr_targets.py -v
```

### Run both

```bash
python -m pytest tests/e2e/ -v
```

### Generate and compare baselines

```bash
# Generate reference output (CPython path)
python tests/e2e/falcon_ocr_baseline.py --reference -o /tmp/falcon_ref.json

# Generate molt output
python tests/e2e/falcon_ocr_baseline.py --molt -o /tmp/falcon_molt.json

# Compare
python tests/e2e/falcon_ocr_baseline.py --compare \
    --reference-file /tmp/falcon_ref.json \
    --molt-file /tmp/falcon_molt.json
```

## What "Passing" Means

### Token parity (strict)

Both paths must produce **exactly identical** token ID sequences for the
same input. Any token divergence is a failure.

### Logit parity (numerical)

At each decoding step, the softmax probability distributions must satisfy:
- KL divergence < 1e-6
- Per-element absolute difference in logits < 1e-5

### Cross-target parity

CPU and Metal must produce identical token sequences. For shader source
targets (MSL, WGSL, CUDA), the rendered source must compile/validate
without errors.

## Stub Model

The stub model uses reduced dimensions for fast testing:

| Parameter | Real | Stub |
|-----------|------|------|
| dim | 768 | 64 |
| n_layers | 22 | 2 |
| n_heads | 16 | 4 |
| head_dim | 64 | 16 |
| n_kv_heads | 8 | 2 |
| ffn_dim | 2304 | 128 |
| vocab_size | 65536 | 256 |

Stub weights are generated with a fixed seed (42) and are byte-identical
across runs and platforms.

## Architecture

```
falcon_ocr_stub_weights.py
    |
    +-- generate_stub_weights() -> SafeTensors bytes
    +-- generate_stub_config_json() -> JSON string
    +-- generate_test_image() -> RGB bytes
    |
    v
falcon_ocr_baseline.py (CLI)          test_falcon_ocr_parity.py (pytest)
    |                                       |
    +-- --reference: CPython+tinygrad       +-- TestRMSNormParity
    +-- --molt: molt runtime                +-- TestRoPEParity
    +-- --compare: JSON diff                +-- TestAttentionParity
                                            +-- TestLogitDistributionParity
                                            +-- TestStubWeightDeterminism
                                            +-- TestForwardBlockParity
                                            +-- TestFullInferenceParity
                                            +-- TestPerformanceBaseline

                                       test_falcon_ocr_targets.py (pytest)
                                            |
                                            +-- TestCPUTarget
                                            +-- TestMetalTarget
                                            +-- TestMSLSource
                                            +-- TestWGSLSource
                                            +-- TestCUDASource
                                            +-- TestCrossTargetParity
```

## Adding New Targets

To add a new compilation target:

1. Add a new test class in `test_falcon_ocr_targets.py`
2. Implement a `_<target>_available()` detection function
3. Add a compile/validate test for rendered source
4. If the target supports execution, add a numerical parity test against CPU

## Dependencies

- Python 3.12+
- pytest
- molt runtime (for runtime tests; pure-Python math tests run without it)
- xcrun metal (macOS, for MSL compilation tests)
- naga (optional, for WGSL validation tests)
- nvcc (optional, for CUDA compilation tests)
