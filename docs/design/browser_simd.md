# Browser SIMD source, ABI and numerical contract

Status: implemented; target execution qualification is recorded in the V1 findings ledger.
Owner: browser deployment and numerical kernels.

`deploy/browser/simd-ops-rs` is the sole source of `deploy/browser/simd-ops.wasm`.
The browser and benchmarks consume that artifact's tiled matmul and fused softmax
exports. The duplicate Zig implementation and its build/loader lane are retired.
The separate Cloudflare WAT module has its own consumer and remains separate.

## Build and publication

Run `uv run --python 3.12 python tools/build_browser_simd.py` from the repository.
The builder invokes the standalone crate from its directory, with the pinned Rust
compiler and installed `wasm32-unknown-unknown` target. Its `.cargo/config.toml`
enables SIMD and exports `__heap_base`. There are no Cargo dependencies or Molt
runtime builds in this lane.
The crate declares its own `[workspace]` boundary. The repository's exclusion
alone does not stop [Cargo's ancestor workspace discovery](https://doc.rust-lang.org/cargo/reference/workspaces.html#the-members-and-exclude-fields)
when a landing checkout is nested inside another workspace. Lock validation
and changed-package clippy admit every independent root before Cargo executes;
ordinary members and excluded path-dependency helpers keep their existing owners.

Cargo's complete successful JSON transcript selects the one `simd_ops` cdylib
artifact by package, manifest and target identity. The builder reads Cargo's
reported filename, including a redirected `CARGO_TARGET_DIR`, through the shared
bounded stable-file reader. It verifies the consumed browser function signatures,
owned memory and heap boundary before atomic publication. Missing, ambiguous,
malformed, failed or ABI-incompatible results preserve the previous artifact.
`--check` builds through the same boundary and compares without publishing.
No guessed target-directory copy or optional optimizer changes the product.
The complete published module has a 6 KiB regression budget, checked after building.
Inherited compiler flags that remove required exports cause publication to fail.

`browser_simd` in `tools/proof_plan.toml` owns CI compilation, clippy, source/product
synchronization, independent arithmetic oracles and the actual browser API consumer.
It uses Python, Rust, Cargo and Node without the compiler/runtime/LLVM build matrix.
The local `browser-simd` rule runs the same owning tests; changed-crate clippy
selects its standalone command. One test invocation builds once and runs the Node
harness once. Tooling and validation execute on the development side only.

## Runtime ABI and cost

The module imports nothing and exports its own wasm32 memory. `__heap_base` is an
immutable positive, 16-byte-aligned i32 address at or below initial memory size.
The JS arena begins there, preserving Rust's stack/data; the first operation may
grow an initially empty heap. Each operation plans all aligned buffer offsets
without a mutable cursor and grows memory at most once, only when its complete
live buffers exceed existing capacity. Checked arithmetic rejects dimensions
and byte extents that would wrap wasm32. Inputs must have the exact float32 shape
required by the unsafe kernels.

The browser probes SIMD with `WebAssembly.validate`, fetches one artifact and
instantiates once. A forced backend exposes all missing exports in one diagnostic;
automatic backend selection owns the scalar fallback. Each nonempty call copies
inputs into linear memory and copies the result out, so returned arrays survive
arena reuse and memory growth. Multirow softmax/RMSNorm copy each input and result
once and upload the shared weight vector once. Empty rows return before dispatch,
independent of row count. Space is O(live input and output bytes), retained to the
largest call; there is no per-call imported memory, secondary binary or retry.

## Numerical policy

These float32 inference kernels have an explicit operation order. They do not
supply the Python standard library's scalar math contract.

- Basic arithmetic follows the [WebAssembly numerical semantics](https://webassembly.github.io/spec/core/exec/numerics.html).
  `max_f32` and `reduce_max_f32` propagate a canonical quiet NaN and select +0 for
  mixed signed zeros. Empty maximum is -Infinity.
- Reduction accumulates four lanes in input order, then combines
  `(lane0 + lane2) + (lane1 + lane3)` before scalar tails. Cancellation goldens
  distinguish it from a left fold. Empty sum is +0. No reassociation or relaxed
  SIMD instructions are enabled.
- `exp2` uses a positive degree-four interpolant on [0, 1]. Its Chebyshev-Lobatto
  nodes are 0, (2-sqrt(2))/4, 1/2, (2+sqrt(2))/4 and 1. Coefficients are obtained
  by interpolating 2^x at those nodes with 80-digit arithmetic, rounding C1..C3
  to f32 and taking C4 as the exact compile-time endpoint residual. The owning
  Rust constants are the sole coefficient authority; there is no runtime table.
  Positive argument/coefficient Horner evaluation is monotone in f32. Reverse
  Horner evaluation at 1 sums exact dyadic residuals to 2, so integer boundaries
  cannot jump downward. Exact endpoint and adjacent-float tests enforce this
  invariant across scalar and vector bodies. The relative-error limit remains
  1e-5 plus one minimum f32 subnormal of absolute rounding error. NaN, infinities,
  overflow, underflow and powers have explicit policies and exact goldens.
  The common `abs(x) < 126` path has one range check; exceptional handling is
  shared out of line to bound code growth.
- Softmax uses SIMD maximum, one fused exponent/store/sum pass, and SIMD output
  normalization. It accumulates positive exponents in f64 SIMD lanes, bounding
  summation error on long rows without another allocation. Storing exponents
  temporarily in the result buffer trades one extra result read/write for
  eliminating repeated scalar exponent evaluation and online-rescaling error.
  Gradually increasing rows exposed 0.35% error in the previous algorithm; the
  independent stable-softmax reference enforces relative tolerance 2e-5 plus
  one minimum subnormal. Nonfinite rows follow that reference: a +Infinity
  maximum, all -Infinity or any NaN yields all NaNs.
- Tiled matmul retains its 4x4 kernel, checked against independently indexed
  scalar f32 multiply/accumulate in ascending K order. RMSNorm uses a mathematical
  reference at relative tolerance 2e-6 plus one minimum subnormal. RoPE operates
  on complete adjacent pairs.

The degree-four interpolation remainder is bounded by
`(ln(2)^5 / 120) * |product(x-node)| * 2^(1-x)`. On [0, 1/4], the nodal product
is at most 0.002929688; on [1/4, 1] it is below 0.00363 and the exponential factor
is at most 2^(3/4). The real relative error is therefore below 8.15e-6. Coefficient
rounding, eight Horner operations and fractional subtraction add conservative
allowances below 0.06e-6, 0.954e-6 and 0.042e-6 respectively; the total normal-range
bound remains below 1e-5. Subnormal output rounding has the separate absolute term.
The numerical oracle independently checks the domain against `Math.pow`, with
exact boundaries and monotonicity controls.

[Arm's optimized exp2f](https://github.com/ARM-software/optimized-routines/blob/master/math/exp2f.c)
uses range reduction, a table and polynomial approximation; this smaller,
positive-interval polynomial chooses the inference accuracy contract and avoids
SIMD table gathers. [Online softmax](https://arxiv.org/abs/1805.02867) is useful
when avoiding materialization dominates. Here the output buffer already exists;
measuring the stable vector passes justified replacing the online scalar loop.

The independent oracles include special values, signed zeros, vector bodies,
tails, cancellation, degenerate dimensions, repeated execution, heap growth and
persistent result copies. Full correctly rounded `exp2` would require a different
contract and measured cost; [LLVM libc's implementation](https://github.com/llvm/llvm-project/blob/main/libc/src/math/generic/exp2f.cpp)
is a useful reference for range reduction and exceptional-value engineering.
The earlier unmeasured minimax/error claims are removed.

See [the benchmark procedure and scoped measurements](../benchmarks/simd_benchmark.md).
Those microbenchmarks do not qualify end-to-end OCR, all browsers or the V1 release.
