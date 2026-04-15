# Tinygrad GPU Primitives — Plan 3: Python Tensor API + Falcon-OCR Migration

**Goal:** Build the Python `Tensor` class that wraps `molt-gpu` and exposes the tinygrad-compatible API. Migrate Falcon-OCR from the legacy GPU system to the new primitive stack.

**Depends on:** Plan 2 (complete)

---

## File Map

| Path | Responsibility |
| --- | --- |
| `stdlib/tinygrad/tensor.py` | Tensor class — tinygrad-compatible Python API |
| `stdlib/tinygrad/dtypes.py` | DType Python wrappers (dtypes.float32, etc.) |
| `stdlib/tinygrad/device.py` | Device selection and management |
| `stdlib/tinygrad/lazy.py` | LazyOp DAG construction from Python |
| `stdlib/tinygrad/realize.py` | realize() — schedule, fuse, render, execute pipeline |
| `stdlib/tinygrad/nn/__init__.py` | Neural network layers (Linear, Conv2d, LayerNorm, etc.) |
| `stdlib/tinygrad/nn/optim.py` | Optimizers (SGD, Adam) — inference-only for now |
| `tests/gpu/test_tensor.py` | Tensor API conformance tests |
| `tests/gpu/test_falcon_ocr.py` | Falcon-OCR integration test |

## Tasks

### Task 1: DType Python Module
- `dtypes.float32`, `dtypes.float16`, `dtypes.int32`, etc.
- Maps 1:1 to `molt_gpu::dtype::DType`
- `dtypes.default_float = dtypes.float32`

### Task 2: Device Module
- `Device.DEFAULT = "METAL"` (macOS) or `"WEBGPU"` or `"CPU"`
- `Device.set(name)` — select active device
- FFI bridge to `molt-gpu` device creation

### Task 3: Tensor Class — Creation Methods
- `Tensor.zeros(*shape, dtype=dtypes.float32)`
- `Tensor.ones(*shape, dtype=dtypes.float32)`
- `Tensor.rand(*shape, dtype=dtypes.float32)` (PRNG-based)
- `Tensor.eye(n, dtype=dtypes.float32)`
- `Tensor.empty(*shape, dtype=dtypes.float32)`
- `Tensor.full(*shape, fill_value, dtype=dtypes.float32)`
- `Tensor(data)` — from Python list/tuple

### Task 4: Tensor Class — Unary Ops
- `exp`, `log`, `sqrt`, `sin`, `cos`, `neg`, `reciprocal`, `relu`, `sigmoid`, `tanh`, `gelu`
- Each builds a LazyOp DAG node using primitive compositions
- `exp(x)` = `EXP2(MUL(x, LOG2_E))`, etc.

### Task 5: Tensor Class — Binary Ops
- `__add__`, `__sub__`, `__mul__`, `__truediv__`, `__floordiv__`, `__mod__`
- `maximum`, `__and__`, `__or__`, `__xor__`, `__lshift__`, `__rshift__`
- Scalar broadcasting: `Tensor + 1.0` wraps scalar as constant

### Task 6: Tensor Class — Reduce Ops
- `sum(axis=None)`, `max(axis=None)`, `mean(axis=None)`
- `argmax(axis=-1)`, `softmax(axis=-1)`, `log_softmax(axis=-1)`
- Multi-axis reduces chain single-axis reduces

### Task 7: Tensor Class — Movement Ops
- `reshape`, `permute`, `expand`, `pad`, `shrink`, `flip`
- `T` (transpose), `flatten`, `unsqueeze`, `squeeze`
- `contiguous()` — force materialization
- All free via ShapeTracker (no kernel generated)

### Task 8: Tensor Class — Matrix Ops
- `dot(other)` / `matmul(other)` / `@` operator
- `cat(*tensors, dim=0)`, `stack(*tensors, dim=0)`
- Implemented as RESHAPE + EXPAND + MUL + REDUCE_SUM

### Task 9: realize() Pipeline
- `Tensor.realize()` -> FFI to schedule() + fuse() + render() + execute()
- `.numpy()` -> realize + copy_out to Python list
- `.tolist()` -> realize + copy_out

### Task 10: nn Module (Inference)
- `nn.Linear(in_features, out_features)` — matmul + bias
- `nn.Conv2d(in_channels, out_channels, kernel_size)` — via im2col + matmul
- `nn.LayerNorm(normalized_shape)` — mean + var + scale + shift
- `nn.Embedding(num_embeddings, embedding_dim)` — gather

### Task 11: Falcon-OCR Migration
- Replace all `tensor_linear`, `tensor_softmax_last_axis` calls with Tensor API
- Verify OCR accuracy matches previous implementation
- Benchmark: should be faster due to kernel fusion

### Task 12: Conformance Test Suite
- Port tinygrad's test_tensor.py test cases
- Verify all Tensor methods produce correct results
- Cross-backend: Metal vs CPU vs WebGPU

---

## What Plan 3 Delivers

1. Full tinygrad-compatible Python Tensor class
2. All creation, unary, binary, ternary, reduce, movement, matrix ops
3. Lazy evaluation with realize() pipeline
4. nn module for inference (Linear, Conv2d, LayerNorm, Embedding)
5. Falcon-OCR migrated to new stack
6. Conformance test suite
