# Symphony Human Role (Molt)

Last updated: 2026-03-04

This document defines the human operator role for Molt's Symphony + Linear workflow.

## Why A Human Is Required

Symphony is an execution orchestrator, not a product owner.
The human remains accountable for:

- Scope and sequencing decisions.
- Policy and security boundaries.
- Acceptance criteria and release decisions.
- Quality bars from Molt specs (correctness, determinism, performance, compatibility).

## Non-Delegable Human Responsibilities

1. Decide backlog priority and sequencing using canonical docs:
   - `docs/spec/STATUS.md` (current truth)
   - `ROADMAP.md` (forward plan)
   - `OPTIMIZATIONS_PLAN.md` (optimization execution)
   - `docs/spec/areas/compat/README.md` + compatibility matrices
2. Enforce policy constraints:
   - No host-Python fallback for compiled binaries.
   - Rust-first stdlib lowering requirements.
   - Python 3.12+ target policy.
3. Own risk decisions when changes touch:
   - Security/capability surfaces.
   - Dynamic execution policy boundaries.
   - Release/signing/supply-chain decisions.
4. Approve completion only with evidence:
   - Minimum gate matrix (`docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md`)
   - Determinism/security checklist (`docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md`)

## Human Control Loop (Daily)

1. Review new/active Linear issues.
2. Confirm issue metadata quality: area/owner/milestone/priority/status.
3. Confirm selected issues are still aligned with `STATUS.md` + `ROADMAP.md`.
4. Let Symphony run active issues.
5. Review outcomes (code + tests + docs + benchmark artifacts).
6. Transition issue state:
   - Keep active if more execution is needed.
   - Move to `Done`/terminal only after evidence gates pass.

## Human Control Loop (Weekly)

1. Reconcile Linear board against canonical docs:
   - `STATUS.md`, `ROADMAP.md`, `OPTIMIZATIONS_PLAN.md`.
2. Re-prioritize based on blockers and strategic milestones (`RT*`, `SL*`, `LF*`, `TL*`, etc.).
3. Audit compatibility claims against matrices:
   - language surface matrix
   - stdlib surface index/intrinsics audit
4. Run a deterministic quality sweep for high-risk lanes.
5. Prune or merge low-quality/duplicate issues.

## Acceptance Gate The Human Must Enforce

Before marking a substantial engineering issue complete, require evidence for:

- Build/lint/tests as required by the minimum matrix.
- Differential parity with RSS profiling where applicable.
- Compatibility matrix/doc synchronization for behavior changes.
- Security/determinism checklist completion for high-risk changes.

"Looks good" is not a valid acceptance decision.

## Escalation Rules

Human escalation is required when:

- A required intrinsic is missing for stdlib behavior.
- A change would widen dynamic execution/reflection scope.
- Cross-layer architecture coherence is at risk.
- Required verification cannot be produced in-turn.

In these cases, the issue should move to a blocker state with explicit closure criteria.

## Operator Hygiene

- Keep secrets out of repo and prompts (`LINEAR_API_KEY`, other tokens).
- Use external volume paths for heavy artifacts/caches.
- Keep task artifacts and reproducible commands in logs.

## Relationship To Symphony

Symphony executes policy-defined work. The human owns policy correctness.
Human authority overrides automation when quality, safety, or scope integrity is uncertain.
