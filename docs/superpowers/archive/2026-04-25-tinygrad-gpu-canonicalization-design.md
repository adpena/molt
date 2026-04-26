# Tinygrad GPU Canonicalization Design

## Goal

Make upstream tinygrad the canonical public ML contract for Molt tensor and neural-network behavior, while keeping `molt.gpu` as the optimized implementation substrate.

## Contract

The public compatibility surface is `src/molt/stdlib/tinygrad/**`. Any API exposed under the `tinygrad` package must match the targeted upstream tinygrad signature, call shape, and numerical behavior for supported inputs. If Molt does not yet implement a required tinygrad semantic, it must raise a clear unsupported error at the boundary instead of silently taking a host dependency or using a reduced substitute.

`src/molt/gpu/**` remains available as low-level substrate for buffers, intrinsics, optimized kernels, and device-specific implementations. It must not define alternate semantics for tinygrad-owned behavior. Shared behavior moves toward tinygrad-compatible helpers and direct delegation.

## Canonical Sources

- Official tinygrad documentation: `https://docs.tinygrad.org/`
- Upstream tinygrad source: `https://github.com/tinygrad/tinygrad`
- Molt runtime primitive contract: `runtime/molt-gpu/src/ops.rs`

The target upstream tinygrad reference for this slice is master as observed on 2026-04-25. Future upgrades should update this document or add a narrower compatibility note with the exact upstream revision used for verification.

## First Implementation Slice

The first slice fixes the ML boundary needed by Pact/Openpilot renderer work without widening unsupported surface:

- `Tensor.conv2d` becomes an instance method compatible with upstream call order:
  `x.conv2d(weight, bias=None, groups=1, stride=1, dilation=1, padding=0, dtype=None)`.
- `Tensor.conv_transpose2d` is added with upstream call order:
  `x.conv_transpose2d(weight, bias=None, groups=1, stride=1, dilation=1, padding=0, output_padding=0)`.
- `nn.Conv2d`, `nn.ConvTranspose2d`, and `nn.GroupNorm` mirror upstream constructor signatures and delegate through Tensor methods.
- Legacy `molt.gpu.nn.Conv2d` drift is removed by aligning its signature and call path with tinygrad.

## Verification

Coverage must include:

- Signature and delegation tests against upstream tinygrad call shapes.
- Numerical tests for grouped convolution, dilation, tuple padding, transposed convolution, and GroupNorm affine/no-affine paths.
- Native Molt compile/run smoke for the tinygrad contract paths that are supported in this slice.

No compatibility claim is valid without fresh command output from the focused tests.
