# Binary Size And Cold Start
**Spec ID:** 0604
**Status:** Draft
**Priority:** P1
**Audience:** performance engineers, runtime engineers, release engineers
**Goal:** Define required size and cold-start metrics with regression rules.

---

## 1. Metrics

### 1.1 Binary Size
- Native: stripped and unstripped sizes.
- WASM: raw size and gzip/brotli size.
- Report sizes for both runtime and compiled artifacts.

### 1.2 Cold Start
Define cold start as:
- **Native**: time from process start to first request handler invocation.
- **WASM**: time from module instantiation to first handler invocation.

---

## 2. Measurement Rules
- Record hardware, OS, and toolchain version.
- Use the same workload for each baseline comparison.
- Always report median and P95 across 10 samples.

---

## 3. Regression Gates
- Size regressions over 10% require investigation and explicit approval.
- Cold-start regressions over 10% require investigation and explicit approval.
- Bench data must be recorded in JSON and committed for release candidates.

---

## 4. Reporting
- Include size and cold-start metrics in `bench/results/`.
- Summaries must be updated in `README.md` for releases.
