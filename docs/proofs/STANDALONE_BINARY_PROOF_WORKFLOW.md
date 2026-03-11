# Standalone Binary Proof Workflow

Last updated: 2026-03-11

This is the operator workflow for proving that Molt produces standalone binaries with no host Python fallback.

Use this workflow when:
- claiming a feature is supported in production-style native output
- validating release candidates
- checking that a refactor did not silently reintroduce host-Python dependence
- comparing native and WASM contract behavior on the same workload

## Proof Standard
A standalone-binary claim is only credible if all of the following hold:
- the produced artifact runs without invoking host Python
- behavior does not depend on a host stdlib or `PYTHONPATH`
- missing capabilities/resources fail explicitly, not by falling back
- claimed cross-target features have a native/WASM contract check where applicable

## Proof Targets
Use at least one target from each bucket below when doing a release-style proof pass.

### Bucket A: trivial self-contained program
Examples:
- `examples/hello.py`
- a tiny arithmetic/container/control-flow differential probe

Purpose:
- catch obvious packaging/runtime bootstrap regressions fast

### Bucket B: stdlib-heavy compiled program
Examples:
- a representative differential file from `tests/differential/basic/`
- a representative stdlib-heavy script that exercises imports and runtime-owned shims

Purpose:
- prove no host-stdlib fallback snuck in

### Bucket C: service/offload-oriented target
Examples:
- a `molt_worker` demo/export path
- a demo endpoint or integration script from `demo/`

Purpose:
- prove Molt's practical operator lane, not just toy programs

### Bucket D: claimed cross-target native/WASM workload
Examples:
- a workload already covered by `tests/test_wasm_*`
- a linked WASM runnable that mirrors the native target's contract

Purpose:
- prove same-contract behavior where Molt claims both targets

## Native Proof Checklist

### 1. Build the binary
Use the canonical build path for the target under test.
Prefer the same command family CI/release uses.

Example pattern:
```bash
molt build examples/hello.py
```

Record:
- target input
- build profile/config
- output artifact path
- git commit SHA

### 2. Confirm the run path is binary-first
Run the produced artifact directly.
Do not validate by calling it through Python.

Record:
- exact command used
- stdout/stderr
- exit status

### 3. Prove no host Python dependency
Run the artifact in a deliberately hostile shell environment.
At minimum, unset or neutralize:
- `PYTHONPATH`
- `PYTHONHOME`
- `VIRTUAL_ENV`
- any Molt-specific env that would intentionally widen the search path

Also verify that success does not depend on `python` or `python3` being used as a launcher.

Suggested hostile-shell pattern:
```bash
env -i PATH="$PATH" HOME="$HOME" ./path/to/output-binary
```

If the target needs explicit capabilities or app env vars, add only the minimum required variables back.

### 4. Check for no hidden interpreter fallback
The binary should not succeed by silently delegating to host Python.
Useful checks include:
- inspect logs/error text for fallback wording
- run with host Python-related env vars absent
- optionally trace child-process creation if regression suspicion exists

A standalone proof fails if success depends on spawning or consulting host Python.

### 5. Exercise at least one stdlib/runtime-owned path
Pick a target that imports meaningful stdlib surface already claimed by Molt.
This catches regressions where bootstrap works but a later import silently depends on host behavior.

### 6. Record artifact evidence
For operator-grade proof, capture:
- command transcript
- binary path and size
- target triple/profile
- pass/fail summary
- any known caveats

## WASM Same-Contract Addendum
When a feature is claimed for both native and WASM:

### 1. Build the linked WASM artifact
Use the linked-runner path Molt already treats as canonical.

### 2. Run the same logical workload
Inputs and expected outputs should match the native proof target unless a documented capability/platform difference applies.

### 3. Compare contract, not implementation trivia
Check:
- output values
- error shape/class where applicable
- capability-denied behavior
- absence of silent divergence

### 4. Document justified differences
If behavior differs because of platform constraints, the difference must be:
- explicit
- already documented in status/specs
- still within the same stated contract boundary

## Failure Conditions
A proof attempt should be treated as failed if any of the following occur:
- the artifact only works when launched through Python
- success depends on host stdlib discovery or `PYTHONPATH`
- behavior changes materially between normal and hostile-shell runs
- native passes but WASM silently diverges on a cross-target claim
- a bridge/fallback path activates without explicit operator choice

## Release-Ready Evidence Bundle
For a meaningful release or milestone claim, keep a short bundle containing:
- git SHA
- proof date
- native targets exercised
- WASM targets exercised
- commands used
- pass/fail result
- caveats or exceptions

## Relationship to other docs
- Support contract: [../../SUPPORTED.md](../../SUPPORTED.md)
- Compatibility corpus manifest: [../COMPATIBILITY_CORPUS_MANIFEST.md](../COMPATIBILITY_CORPUS_MANIFEST.md)
- Deep status: [../spec/STATUS.md](../spec/STATUS.md)
- `libmolt` ABI contract: [../spec/areas/compat/contracts/libmolt_extension_abi_contract.md](../spec/areas/compat/contracts/libmolt_extension_abi_contract.md)
- Dynamic execution policy: [../spec/areas/compat/contracts/dynamic_execution_policy_contract.md](../spec/areas/compat/contracts/dynamic_execution_policy_contract.md)
