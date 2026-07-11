# Kernel B Determinism Margins

## Scope and evidence status

Kernel B is `collab/pact/pact_witness_kernel/witness_forward.py`: Fourier
features, dense matrix multiplies, `sin`/`cos`, nonlinear activation, and a
float32 output head whose contract observable is exact uint8 `argmax`.

The real learned weights are private and are not present in this repository.
This report therefore uses the deterministic synthetic fixture from
`make_weights_fixture.py`, which has the exact model shapes and documented op
graph but cannot discharge the final real-weight margin obligation. Evidence:
`docs/agent/evidence/determinism_perf/kernel_b_margins.json`.

## Measured synthetic ranges

- Fixture output shape: `96 x 128 x 5` (12,288 pixels).
- Fourier feature range: `[-1, 1]`.
- FiLM range: `[-0.508923, 0.377344]`.
- Dense preactivation range across layers: `[-15.8582, 16.0151]`.
- HOSC output range: `[-0.999329, 0.999329]`.
- Final logit range: `[-13.2459, 11.2411]`.

The active synthetic fixture uses HOSC (`tanh(4*sin(u))`), so it exercises
matrix multiplication, `sin`, `cos`, `tanh`, float32 cast, and `argmax`. It does
not exercise the alternate WIRE activation's `exp(-(s0*u)^2)`. No claim about
WIRE `exp` parity follows from this fixture.

## Argmax margins

| Metric | Result |
|---|---:|
| Minimum top1-top2 margin | `7.367134e-5` |
| 0.1 percentile | `0.00194698` |
| 1 percentile | `0.0267802` |
| Median | `1.41435` |
| Smallest margin in top-logit ULPs | `618` |
| Exact ties | `0` |

Directed worst-case stress lowers the winner and raises the runner-up by the
same number of float32 ULP steps. Argmax changes were zero at 1, 2, 4, 8, and
16 ULPs on all 12,288 pixels. The pre-authorized fallback does not trigger on
this synthetic fixture.

## WASI libm evidence

The existing parity-lab probe evaluates 100,000 deterministic inputs in
`[-50, 0)`. The native Clang/UCRT checksum is `51dea767dad4dbd9`; the sealed
wasm32-wasip1/WASI checksum is `0e0626e743526c99`. Therefore `exp` substitution
is BIT-UNSAFE over the documented broad range even though Kernel A's 16
Gaussian-weight inputs happen to match exactly.

For Kernel B, the current HOSC fixture does not call `exp`. A WIRE profile must
measure its real preactivation-derived exponent range and run the WASI/native
comparison on exactly those values before acceptance.

## Operation verdict

| Operation | Classification | Required action |
|---|---|---|
| Independent feature/pixel evaluation | BIT-SAFE | Parallelize across rows/pixels while preserving each output sequence. |
| Lane-wise `sin`/`cos`/`tanh` calls | LIBM-UNSAFE | Keep the sealed implementation or prove exact argmax stability on real weights. |
| Dense matmul/sgemm | REDUCTION-UNSAFE | Accumulation order will differ across BLAS/SIMD implementations; require real-weight margin evidence. |
| Float64-to-float32 output cast | BIT-SAFE only if retained | Do not move or change the cast boundary. |
| `argmax` | BIT-SAFE given identical logits | With drifting logits, use the pre-authorized margin gate. |
| WIRE `exp` | LIBM-UNSAFE, unexercised | Range-specific WASI/native probe required. |

## Verdict and pact-owned call

Kernel B must not begin from an exact-logit assumption. The synthetic fixture
has substantial argmax headroom and does not trigger the fallback through 16
directed ULPs, but the real learned weights are the acceptance authority.

The pact/operator-owned decision is whether the already-authorized
argmax-margin fallback admits a candidate whose logits differ while labels
remain stable with adequate measured margin. Recommendation: retain exact
uint8 argmax as the observable, require a published minimum-margin threshold
derived from the real weights, and fail closed when any pixel falls below it.
Do not weaken the contract based on the synthetic result.

Reproduction:

```powershell
python tools/kernel_b_determinism_margins.py --output kernel_b_margins.json
```
