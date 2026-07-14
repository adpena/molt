# collab/pact - Pact <-> Molt channel / START HERE

This directory is the additive correspondence and executable handoff index for
the Pact browser witness collaboration. Do not copy the moving execution
frontier into this file.

## Authority order

1. [`docs/agent/PACT_CONTRACT_LEDGER.md`](../../docs/agent/PACT_CONTRACT_LEDGER.md)
   owns every obligation, status, owner, dependency, and report-to-row mapping.
2. [`parity/check_parity.py`](parity/check_parity.py) plus a kernel's gate
   manifest is the single acceptance authority. The original inline
   `pact_witness_kernel/check_parity.py` is retained only as a frozen equivalence
   oracle for tests.
3. `tools/proof_queue.py` and its logs own live proof status. Heavy WASM/browser
   work runs only through the named queue lanes documented in
   [`docs/agent/PROOF_QUEUE.md`](../../docs/agent/PROOF_QUEUE.md).
4. [`STATUS.md`](STATUS.md) is a dated dogfooding narrative. It is useful
   history, not a live queue or obligation authority.
5. Implementation and reproducible evidence outrank every narrative document.

## Current P0 acceptance contract

P0 remains Kernel A (`field_solve`) from every report through 012. Molt must
compile and execute the real package-native NumPy/SciPy kernel through the WASM
lane, write all 11 outputs to `candidate_outputs.npz`, and pass:

```powershell
python collab/pact/parity/check_parity.py `
  candidate_outputs.npz `
  collab/pact/pact_witness_kernel/reference_outputs.npz `
  collab/pact/pact_witness_kernel/field_solve_gates.json
```

The engine refuses missing/extra arrays, dtype or shape drift, NaN/Inf-mask
drift, unknown gates, and an `atol` above `1e-3`. Integer/label, bitwise,
exact-set, and tolerance policy is declared per output. No report, package seal,
build/link success, shader-model result, or forward-only smoke is acceptance.

Check live queue state before interpreting or launching proof:

```powershell
uv run --active --project . --python 3.12 python tools/proof_queue.py status
uv run --active --project . --python 3.12 python tools/proof_queue.py pact-witness-acceptance --detach
```

No Molt-produced `candidate_outputs.npz` parity PASS is currently recorded in
the ledger. Consult its `KA` row and the proof queue for the exact current
prerequisite; do not revive a dated blocker from `STATUS.md` or an older memo.

## Correspondence index

The numbered series is additive. Both 011 documents are intentional: one is the
Molt progress/interface proposal; the other is Pact's later WebGPU/WebNN demo
and concrete W6 ask. Report 012 is Molt's reply and delivered harness contract.

| memo | direction and durable contribution |
|---|---|
| [`001`](001_witness_forward_to_wasm_use_case.md) | Pact -> Molt: browser witness use case and original blockers |
| [`002`](002_numpy_scipy_wasm_coverage.md) | Pact -> Molt: NumPy/SciPy WASM support-matrix ask |
| [`003`](003_browser_single_function_embed_api.md) | Pact <-> Molt: minimal browser embed contract and recovery evidence |
| [`004`](004_molt_progress_ack_and_refined_asks.md) | Pact -> Molt: `molt-embed` acknowledgement and refined asks |
| [`005`](005_max_in_browser_witness_acceptance_kernel.md) | Pact -> Molt: concrete Kernel A/B acceptance target |
| [`006`](006_precise_contract_full_witness_pipeline.md) | Pact -> Molt: exact kernels, determinism gates, and three-phase vision |
| [`007`](007_molt_response_numpy_scipy_c_api_greenup_and_witness_kernel_plan.md) | Molt -> Pact: C-API scan greenup, ownership boundary, and Kernel-A plan |
| [`008`](008_addendum_v2_witness_decoder_20260629.md) | Pact -> Molt: decode chain, rule-118, runtime axes, and native sister backend |
| [`009`](009_theta_prime_capstone_sync_forward_kernels_and_vision_20260701.md) | Pact -> Molt: seven-kernel map and P3/P4 horizons |
| [`010`](010_pact_update_and_work_package_20260709.md) | Pact -> Molt: ranked W1-W5 work package and current measured vehicle context |
| [`011 Molt reply`](011_molt_reply_progress_sync_and_harness_proposal_20260710.md) | Molt -> Pact: honest P0 sync, support matrix, and harness proposal |
| [`011 Pact demo`](011_webgpu_webnn_witness_demo_20260711.md) | Pact -> Molt: Kernel-B WGSL/WebNN prototype, 0/73,728 shader-model mismatch, and W6 ask; explicitly non-authority |
| [`012`](012_molt_reply_parity_harness_interface_and_kernel_b_intake_20260711.md) | Molt -> Pact: delivered fail-loud W6 engine, Kernel-B intake, and Gaussian rounding constraint |

The ledger's correspondence-to-obligation matrix proves every ask and delivery
above maps to a stable ID. New correspondence must update existing IDs for
restatements and create a new ID only for a genuinely new obligation.

## Work sequencing

- Kernel A remains P0. Package and toolchain custody are prerequisites, not
  substitutes for its real WASM parity verdict.
- `W6-HARNESS`, `MATRIX`, and the Molt reply are delivered.
- Pact's Kernel-B WGSL/WebNN work is reusable evidence for `KB`, `W2-FLOW`,
  `W3-ONNX`, and `KERN7`, but it captured no Molt-WASM candidate or browser GPU
  execution and promotes none of those obligations to done.
- WASM-CPU is the deterministic authority. WebGPU is a separately labelled,
  deterministic-per-device showcase/speed lane. CPU and GPU cells never inherit
  proof from one another.
- Third-party behavior comes only from upstream package source/build systems and
  checksummed source-recompiled artifacts. No host fallback or Molt-owned
  NumPy/SciPy semantic clone can satisfy an obligation.

## About Pact

Pact is the lab's entry for the comma.ai video-compression challenge: the
shortest compliant `archive.zip` whose decoded witness lands in the same frozen
evaluator cells (SegNet argmax plus PoseNet) as the source clip. The capstone is
a non-RGB task-space witness: a coordinate INR amortizing the SegNet argmax
partition as signed-distance fields. Canonical source pointers are in report
006 and [`pact_witness_kernel/`](pact_witness_kernel/).
