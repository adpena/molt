# pact_witness_kernel — runnable parity contract for molt (WASM-CPU + WebGPU/WGSL)

The **exact** numpy/scipy pact wants molt to compile — TWO kernels (forward + field-solve),
deterministic fixture/reference generators, a parity oracle, and a proof the extract is
bit-identical to pact's production tac source. See `../006_precise_contract_full_witness_pipeline.md`
for the vision + determinism gates + compile-target (WASM-CPU + WebGPU/WGSL) guidance.

Pipeline:  `witness_forward.levelset_argmax` (INR → lstar) → `field_solve(lstar)` (→ viz fields).

## Files
| file | what |
|---|---|
| **`witness_forward.py`** | **Kernel B** — the witness INR. `levelset_argmax(params,cfg,coords,pair,H,W)->lstar`. Pure numpy (matmul/sin/cos/tanh/exp/argmax). Real extract of tac `lever_b_levelset_generator`. |
| **`field_solve.py`** | **Kernel A** — Morse-Smale field-solve. `field_solve(lstar[H,W] u8)->dict` (11 arrays). Pure numpy + scipy.ndimage. Real extract of tac `render_witness_morse_smale_viz`. |
| `make_weights_fixture.py` | deterministic SYNTHETIC decoder weights (real shapes, seeded) → `witness_weights_sample.npz`. |
| `make_fixture.py` | deterministic synthetic road-scene `lstar` (pure geometry) → `lstar_sample.npz` (384×512 u8). |
| `verify_against_tac.py` | **NO-FAKE fidelity proof:** imports tac, asserts the extract == canonical, bit-for-bit (ALL-MATCH). |
| `check_parity.py` | the oracle: `check_parity.py candidate.npz [reference_outputs.npz]` → per-field gates, exit 0 = PASS. |

Generated `.npz` outputs are ignored, not committed. Recreate them locally with
the commands below under numpy 1.26.4 / scipy 1.17.1; the scripts and parity
gates are the tracked authority.

## Reproduce + verify (CPython)
```bash
python make_weights_fixture.py                       # -> witness_weights_sample.npz
python witness_forward.py witness_weights_sample.npz  # -> witness_forward_reference.npz (Kernel B)
python make_fixture.py                                # -> lstar_sample.npz
python field_solve.py lstar_sample.npz                # -> reference_outputs.npz (Kernel A)
python check_parity.py reference_outputs.npz          # sanity: all PASS
PYTHONPATH=<pact>/src python verify_against_tac.py    # fidelity: ALL-MATCH (extract == tac)
```

## Reproduce the reference (CPython)
```bash
python make_fixture.py          # -> lstar_sample.npz
python field_solve.py lstar_sample.npz   # -> reference_outputs.npz  (+ self-check argmax==lstar)
python check_parity.py reference_outputs.npz   # sanity: all PASS
```

## The molt ask (precise)
1. Compile `field_solve.py`'s `field_solve(lstar)` to WASM via the documented `molt-embed` path
   (`MoltCompiler::compile_to_wasm` or the microgpt build recipe).
2. Run it in WASM on `lstar_sample.npz` (load the `lstar` array → `field_solve` → save all 11 keys
   to `candidate_outputs.npz`).
3. `python check_parity.py candidate_outputs.npz` → **PASS** is the done-criterion.

The Molt-side browser/WASM attempt is no longer a raw local command. Route it
through the proof queue:

```powershell
uv run --active --project . --python 3.12 python tools/proof_queue.py status
uv run --active --project . --python 3.12 python tools/proof_queue.py pact-witness-acceptance
```

For the smallest queued oracle check, run:

```powershell
uv run --active --project . --python 3.12 python tools/proof_queue.py pact-witness-oracle
```

The oracle lane regenerates ignored `.npz` outputs in a temporary directory and
proves the tracked `check_parity.py` gates without creating a second acceptance
path.

`field_solve` is deterministic (no RNG/time/I/O) and bit-identical across CPython re-runs. The two
cross-implementation-fragile spots (sort tie-order, eigh sign) are **already canonicalized inside the
kernel**, so WASM does not need to match LAPACK's sign or numpy's tie convention — only the ops in
the gate table below. See `../006_precise_contract.md` for the full determinism-gate breakdown.

## Output keys (all numpy arrays; H=384 W=512)
`sdf_argmax`(H,W u8, ==lstar) · `sdf_margin_m12`(H,W f32) · `sdf_gap13`(H,W f32) ·
`boundary`(H,W u8) · `m_smooth`(H,W f32) · `crit_max_rc`(≤40,2 i32) · `crit_min_rc`(≤120,2 i32) ·
`crit_saddle_rc`(k,2 i32) · `crit_saddle_eigvec`(k,4 f32) · `curvature`(H,W f32) · `dist`(H,W f32)

Note: `lstar_sample.npz` is a *synthetic road-scene-structured* partition (deterministic geometry),
sufficient for **numerical** WASM parity. A real witness-φ argmax bundle follows once pact's shared-
machine GPU-run constraint clears; it changes the input values, not the kernel or the gates.
