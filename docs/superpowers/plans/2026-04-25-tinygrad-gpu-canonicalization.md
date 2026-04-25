# Tinygrad GPU Canonicalization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Canonicalize Molt's GPU-facing ML surface so tinygrad is the public source of truth and `molt.gpu` is only substrate.

**Architecture:** Add tests that encode upstream tinygrad contracts, then align `src/molt/stdlib/tinygrad/tensor.py`, `src/molt/stdlib/tinygrad/nn/__init__.py`, and `src/molt/gpu/nn.py` to those contracts. Unsupported semantics raise explicitly; no host Python, PyTorch, NumPy, or hidden fallback path is introduced.

**Tech Stack:** Python 3.12, pytest, Molt tinygrad stdlib, Molt GPU substrate.

---

### Task 1: Contract Tests

**Files:**
- Modify: `tests/test_tinygrad_import_shim.py`
- Modify: `tests/test_gpu_api.py`

- [ ] Add tests for `Tensor.conv2d` instance-call signature with `groups`, tuple `stride`, tuple `dilation`, and tuple `padding`.
- [ ] Add tests for `Tensor.conv_transpose2d` using upstream tinygrad sample semantics.
- [ ] Add tests for `nn.Conv2d`, `nn.ConvTranspose2d`, and `nn.GroupNorm` constructor shape, delegation, and affine behavior.
- [ ] Run focused tests and confirm the new tests fail for missing or drifted behavior.

### Task 2: Tensor Substrate

**Files:**
- Modify: `src/molt/stdlib/tinygrad/tensor.py`

- [ ] Replace the static 2D-only convolution implementation with an instance method matching upstream tinygrad call order.
- [ ] Implement grouped, strided, dilated, padded N-dimensional convolution through deterministic primitive-compatible loops.
- [ ] Implement `conv_transpose2d` through deterministic scatter accumulation for N-dimensional spatial weights.
- [ ] Preserve existing `Tensor.conv2d(x, ...)` class-call compatibility through normal Python instance-method binding.
- [ ] Run focused tensor tests and fix root causes only.

### Task 3: Tinygrad NN Surface

**Files:**
- Modify: `src/molt/stdlib/tinygrad/nn/__init__.py`

- [ ] Add `Conv2d`, `ConvTranspose2d`, and `GroupNorm` with upstream tinygrad signatures.
- [ ] Delegate `Conv2d.__call__` and `ConvTranspose2d.__call__` exactly through Tensor methods.
- [ ] Implement `GroupNorm` through `reshape(...).layernorm(eps=...).reshape(...)` plus channel affine.
- [ ] Update `__all__` only for behavior implemented in this task.
- [ ] Run focused tinygrad import shim tests.

### Task 4: Remove `molt.gpu` Drift

**Files:**
- Modify: `src/molt/gpu/nn.py`
- Modify: `tests/test_gpu_api.py`

- [ ] Align `molt.gpu.nn.Conv2d` constructor and call path with tinygrad where it is exposed.
- [ ] Remove narrowed legacy assumptions from tests.
- [ ] Do not add aliases or compatibility branches for the old narrowed constructor.
- [ ] Run focused GPU API tests.

### Task 5: Verification And Staging

**Files:**
- Stage all modified files immediately after writes.

- [ ] Run focused pytest commands for tinygrad import shim and GPU API.
- [ ] Run native Molt smoke for the supported tinygrad conv path.
- [ ] Report any unsupported tinygrad surface that still intentionally raises.
- [ ] Confirm `git status --short` separates this work from pre-existing partner edits.
