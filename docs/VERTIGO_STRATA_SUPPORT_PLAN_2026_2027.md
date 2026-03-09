# Molt Support Plan For Vertigo/Strata Browser + CLI (2026-2027)

Last updated: 2026-03-09
Owner: Worker 2 documentation lane

## 1. Purpose

Define a focused, measurable plan for how Molt supports Vertigo/Strata delivery across browser and CLI surfaces, with explicit scope boundaries, acceptance gates, and integration risk controls.

## 2. Deadline Alignment

Assumed delivery windows for this plan:

- Browser + CLI go/no-go handoff: 2026-12-15
- Stabilization completion: 2027-06-30

If product leadership moves these dates, keep the same gates and shift phase windows proportionally.

## 3. Support Scope (Where Molt Helps)

Molt contributes in two primary lanes.

### 3.1 Tooling Lane (Build, Validate, Ship)

1. Deterministic compile/test/diff workflows for browser (`wasm_browser`) and CLI (`native`) targets.
2. Structured diagnostics for triage (compile profile/tier/degrade and pass hotspot reporting).
3. Cross-target artifact packaging contract (build metadata, capabilities, target manifest).
4. CI-friendly gate automation (fast fail on correctness/perf regressions before integration).

### 3.2 Runtime Lane (Execute, Interop, Observe)

1. Native runtime lane for CLI orchestration tasks and deterministic execution.
2. WASM browser host lane for browser execution with explicit capability constraints.
3. Shared boundary contracts for cancellation, deadlines, error classes, and payload schema consistency.
4. Runtime telemetry required for deadline-era operations (latency, failures, capability denials, reset behavior).

## 4. Explicitly Deferred (Out Of Deadline Scope)

The following remain deferred unless they block critical Vertigo/Strata scenarios:

1. Full browser socket parity (UDP/listen/server socket parity beyond current scoped support).
2. Broad stdlib parity not needed by Vertigo/Strata browser + CLI flows.
3. Nonessential cross-target UX polish in CLI beyond required operational diagnostics.
4. GPU/MLIR acceleration lanes and noncritical optimization experiments.
5. Expansion to additional product surfaces outside browser + CLI (for example mobile-specific runtime work).

Re-entry criteria for any deferred item:

1. It is a direct blocker for a signed acceptance gate.
2. It can be delivered without moving the 2026-12-15 handoff date.
3. It has measurable ROI against latency, correctness, or operator workload.

## 5. Work Packages And Milestones

### Phase 0: Baseline Lock (2026-03-09 to 2026-04-30)

1. Lock reference workloads for Vertigo/Strata browser + CLI smoke lanes.
2. Publish baseline metrics:
   - compile latency p50/p95 per target,
   - runtime request latency p50/p95,
   - crash/timeout rates,
   - deterministic build hash stability.
3. Define owner matrix and escalation path for gate failures.

Exit criteria:

1. Baseline artifact published and reproducible by two independent operators.
2. Gate definitions frozen and versioned in this document.

### Phase 1: Tooling Hardening (2026-05-01 to 2026-07-31)

1. Stabilize deterministic CLI build/test lanes for both `native` and `wasm_browser`.
2. Finish first-class diagnostics required for incident triage.
3. Provide CI gate command set for one-command validation.

Exit criteria:

1. Tooling acceptance gates A1-A4 (Section 6) all pass for 14 consecutive days.
2. No P0/P1 tooling regressions open for more than 72 hours.

### Phase 2: Runtime Hardening (2026-08-01 to 2026-10-31)

1. Harden browser host runtime path used by Vertigo/Strata.
2. Harden CLI runtime path for deterministic operation under load.
3. Validate shared contract parity (error semantics, cancellation/deadlines, payload shape).

Exit criteria:

1. Runtime acceptance gates B1-B5 all pass in nightly runs for 21 consecutive days.
2. Integration risk R1-R4 (Section 7) each has active mitigation evidence.

### Phase 3: Integrated Launch Readiness (2026-11-01 to 2026-12-15)

1. Run end-to-end browser + CLI integration rehearsals with production-like config.
2. Enforce release-candidate freeze policy for noncritical changes.
3. Prove rollback and operator recovery procedures.

Exit criteria:

1. All launch gates C1-C5 pass on three consecutive release candidates.
2. No unresolved high-risk item in Section 7 at go/no-go review.

### Phase 4: Stabilization (2027-01-01 to 2027-06-30)

1. Reduce operational toil (incident volume, manual interventions, flaky tests).
2. Burn down deferred-but-now-required gaps based on production evidence.
3. Maintain gate performance and correctness under growth.

Exit criteria:

1. 90-day rolling SLOs meet threshold (Section 6, D-lane gates).
2. Deferred items are either closed or explicitly re-deferred with sign-off.

## 6. Acceptance Gates (Hard, Measurable)

### A. Tooling Gates

- A1 Determinism:
  - Same commit + same config produces identical artifact hash in 100/100 rebuild attempts per target.
- A2 Compile latency:
  - `native` compile p95 does not regress by more than 10% from locked baseline.
  - `wasm_browser` compile p95 does not regress by more than 15% from locked baseline.
- A3 Diagnostics completeness:
  - 100% of failed builds emit machine-readable cause class and top offending pass/module.
- A4 CI reliability:
  - Gate pipeline success-on-clean-code rate >= 98% over trailing 30 days.

### B. Runtime Gates

- B1 Browser correctness:
  - Browser runtime smoke suite pass rate >= 99% over trailing 14 days.
- B2 CLI correctness:
  - CLI runtime smoke suite pass rate >= 99.5% over trailing 14 days.
- B3 Deadline/cancellation semantics:
  - Contract tests pass 100% for timeout/cancel propagation in browser and CLI lanes.
- B4 Error parity:
  - Contract error taxonomy mismatch rate <= 1% across cross-surface replay corpus.
- B5 Capability enforcement:
  - 100% of disallowed operations fail with explicit capability-denied signatures (no silent fallback).

### C. Launch Gates

- C1 Release candidate stability:
  - Three consecutive RC runs with zero P0 failures and <= 2 P1 failures each, each P1 with approved mitigation.
- C2 Recovery readiness:
  - Rollback drill MTTR <= 15 minutes in two independent rehearsals.
- C3 Observability coverage:
  - Required runtime and tooling telemetry fields present in >= 99% of runs.
- C4 Integration soak:
  - 72-hour continuous integration soak without unrecovered crash loops.
- C5 Sign-off package:
  - Go/no-go packet includes reproducible evidence for all A/B/C gates.

### D. 2027 Stabilization Gates

- D1 Incident rate:
  - P0 + P1 incident count reduced by >= 50% versus first 30 days post-handoff.
- D2 Operational toil:
  - Manual recovery actions reduced to <= 2 per week average.
- D3 Performance:
  - p95 end-to-end latency remains within +/-10% band versus accepted launch baseline.

## 7. Integration Risks And Controls

| ID | Risk | Trigger Signal | Mitigation | Escalation Threshold |
|---|---|---|---|---|
| R1 | Browser host feature mismatch for required Vertigo/Strata flows | Contract tests fail in browser but pass in CLI/native | Freeze feature scope, add contract-first shim, block noncritical merges | 2 consecutive nightly failures |
| R2 | Cross-target behavior drift (`native` vs `wasm_browser`) | Error taxonomy mismatch >1% or replay divergence | Expand replay corpus and enforce pre-merge parity gate | >1% mismatch for 3 days |
| R3 | Compile/packaging latency drift breaks release cadence | p95 compile latency beyond A2 thresholds | Enable profiling lane, bisect commits, temporary merge rate limit | Threshold breach for 48 hours |
| R4 | Incomplete diagnostics slows triage during incidents | Failed builds/runs missing cause class or hotspot payload | Treat as release-blocking bug in tooling lane | Missing payload rate >1% daily |
| R5 | Over-expansion into noncritical parity work | Throughput shifts to items not tied to gates | Weekly scope review tied to A/B/C gate map | >20% sprint capacity outside mapped gates |
| R6 | Flaky integration tests mask regressions | Gate flake rate increases and reruns hide defects | Quarantine + root-cause SLA + flake budget tracking | Flake rate >2% over 7 days |
| R7 | Rollback path unproven before handoff | Rehearsal cannot restore service fast enough | Mandatory rollback rehearsals in Phase 3 | MTTR >15 minutes in any rehearsal |
| R8 | Ownership ambiguity across tooling/runtime lanes | Incidents bounce between teams without action | Single DRI per gate with named backup | Unowned P0/P1 for >2 hours |

## 8. Operating Model

1. Weekly gate review:
   - Review A/B/C/D gate trends, open risk IDs, and mitigation evidence.
2. Biweekly integration rehearsal:
   - Browser + CLI combined validation run with evidence bundle.
3. Monthly scope lock:
   - Reconfirm deferred list and ensure no unapproved scope creep.
4. Evidence storage:
   - Every gate report must include timestamp, commit SHA, target, and reproducible command set.

## 9. Minimum Evidence Packet For Handoff

1. Gate report covering all A/B/C metrics with trailing windows.
2. Replay/parity summary showing cross-target consistency.
3. Rollback rehearsal logs with MTTR proof.
4. Known-risk ledger (open/closed) with sign-off owners.
5. Deferred scope ledger with rationale and revisit date.

## 10. Plan Change Control

This plan can be changed only when one of the following is true:

1. Product deadline changed and new dates are explicitly approved.
2. A gate threshold is proven unrealistic with reproducible evidence from at least two independent runs.
3. A new blocker is discovered that directly prevents Vertigo/Strata browser + CLI handoff.

Any change must preserve measurable acceptance criteria and explicit risk ownership.
