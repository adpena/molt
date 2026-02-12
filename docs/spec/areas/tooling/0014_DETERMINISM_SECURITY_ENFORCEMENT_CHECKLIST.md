# Determinism And Security Enforcement Checklist

Status: Active
Owner: tooling + runtime + security
Last updated: 2026-02-11

## Purpose
Provide one enforcement checklist for Month 1 determinism/security gates:
lockfiles, SBOM/signature posture, and capability gating.

## 1) Lockfile And Dependency Integrity
- [ ] Python lock is current:
  - `uv lock --check`
- [ ] Rust lock integrity is current:
  - `cargo metadata --locked`
- [ ] No unintended lockfile drift in working tree unless dependency changes are intentional.

## 2) Deterministic Build Controls
- [ ] Default deterministic hash seed behavior is preserved (`PYTHONHASHSEED=0` via CLI default, unless explicitly overridden by `MOLT_HASH_SEED`).
- [ ] Build profile is explicit and policy-compliant:
  - `--profile dev` for development workflows.
  - `--profile release` for release validation/benchmarks/published binaries.
- [ ] Reproducible artifact evidence is captured for release candidates (record command, commit, target triple, hash).

## 3) Capability Gating Enforcement
- [ ] New or changed I/O/network/process behavior is capability-gated and documented.
- [ ] Differential parity lanes include non-trusted validation when security-sensitive behavior changed:
  - Example: `MOLT_DEV_TRUSTED=0 MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic tests/differential/stdlib`
- [ ] No hidden host-Python fallback path is introduced for compiled binaries.

## 4) SBOM/Signing And Supply-Chain Evidence
- [ ] Packaged artifacts include SBOM sidecars and embedded metadata as documented in `docs/spec/areas/tooling/0009-packaging.md`.
- [ ] Signature metadata/verification policy is enforced for publish/verify lanes where applicable.
- [ ] Any supply-chain audit deltas are logged in release notes or task reports.

## 5) Required Reporting Artifacts
- [ ] Record command transcript references in task logs under `logs/agents/<task>/`.
- [ ] Record benchmark/profiling artifacts for performance-affecting security changes.
- [ ] Keep docs aligned (`README.md`, `docs/spec/STATUS.md`, `ROADMAP.md`) when policy or behavior changes.

## Related Docs
- `docs/spec/areas/core/0025_REPRODUCIBLE_AND_DETERMINISTIC_MODE.md`
- `docs/spec/areas/tooling/0009-packaging.md`
- `docs/spec/areas/security/0010-security.md`
- `docs/OPERATIONS.md`
