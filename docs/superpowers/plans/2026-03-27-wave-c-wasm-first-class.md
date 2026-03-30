# Wave C: WASM First-Class Residual Plan

> Audited on 2026-03-30. Much of the originally described Wave C work already exists: partition-aware cache identity, explicit link fingerprinting, WASM importlib coverage, artifact validation, and Cloudflare verification tooling. This plan now tracks only the remaining parity and proof work.

## Audit outcome

- Already landed:
  - `_build_cache_variant(..., partition_mode=...)` in `src/molt/cli.py`;
  - `_link_fingerprint()` coverage for explicit stdlib artifact changes;
  - `tests/test_wasm_importlib_machinery.py`;
  - `tools/cloudflare_demo_verify.py` and `tools/cloudflare_demo_deploy_verify.py`.
- Still incomplete:
  - there is no single audited parity sweep covering linked/unlinked WASM execution plus artifact-contract behavior;
  - production Cloudflare verification still depends on a live deployment pass rather than repo-local proof alone;
  - size/startup budgets need one current benchmark-backed closure pass.

## Parallel tracks

### Track C1 - WASM parity sweep (independent)

- Re-run the focused WASM regression set for importlib, artifact validation, and linked/unlinked execution.
- Reopen implementation work only if a currently tracked parity case still fails.
- Validation:
  - `PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/test_wasm_importlib_machinery.py tests/test_wasm_link_validation.py tests/cli/test_cli_wasm_artifact_validation.py`

### Track C2 - Cloudflare deployment proof (depends on deploy credentials/environment)

- Keep the local verifier and live deploy verifier aligned to the same endpoint matrix.
- Run a real deploy verification against the current worker URL and capture the artifact under canonical roots.
- Validation:
  - `python3 tools/cloudflare_demo_verify.py --help`
  - `python3 tools/cloudflare_demo_deploy_verify.py --live-base-url <worker-url> --artifact-root logs/cloudflare_demo_verify/live`

### Track C3 - WASM size/startup budget (independent once C1 is green)

- Run one benchmark-backed pass over linked WASM output and record the current size/startup baseline.
- Only optimize if the measured result violates the currently accepted budget.
- Validation:
  - `PYTHONPATH=src uv run --python 3.12 python3 tools/bench_wasm.py --linked --output bench/results/wave_c_exit_gate.json`

## Exit gate

- C1 passes on the current tree.
- C2 has one real deployment proof artifact, or is explicitly blocked by missing deployment credentials.
- C3 produces a current benchmark artifact under `bench/results/`.
- If all open items are closed, delete this plan on the next audit pass.
