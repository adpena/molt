# Molt Documentation Architecture: Single-Source-Of-Truth And Anti-Drift

**Status:** Draft — Pending Review
**Date:** 2026-04-03
**Scope:** Repository documentation architecture for external newcomers and project users
**Primary goal:** Make Molt's documentation useful, professional, and operationally hard to drift

---

## 1. Goal

Redesign Molt's documentation system so it is optimized first for external
newcomers and users, while remaining workable for active compiler/runtime
development.

The system must satisfy four constraints:

1. A newcomer should understand what Molt is in under two minutes.
2. A new user should reach a successful install/build/run path in under five minutes.
3. Every important fact should have one owning source of truth.
4. Documentation drift should be mechanically difficult, and visible in CI when it happens.

This is not a copy-editing pass. It is a structural simplification of the
documentation stack.

## 2. Non-Goals

- Building a full docs website or portal in this phase.
- Preserving current documentation structure for backwards compatibility.
- Rewriting every historical design/spec document in one pass.
- Encoding unstable implementation details as hand-maintained prose.
- Treating README as a comprehensive developer manual.

## 3. Problem Statement

Molt's current docs drift because the same classes of facts are written in too
many places:

- `README.md` currently acts as landing page, status ledger, roadmap summary,
  architecture index, capability guide, and sprint log.
- `docs/spec/STATUS.md` mixes current state with historical narrative and admits
  partial staleness.
- `ROADMAP.md` contains forward plan material but also repeats current-state facts.
- Compatibility status is spread across many hand-authored surfaces, some of
  which are too volatile to remain correct without constant manual care.

The result is predictable:

- external readers have too much to parse before they can use Molt;
- maintainers must update multiple prose surfaces for one implementation change;
- contradictions are easy to introduce and hard to detect;
- “cross-referenced” documents still drift because references do not create ownership.

## 4. Design Principles

1. **One owner per fact.**
   Every substantive fact has exactly one owning document or generated artifact.

2. **Stable doctrine vs volatile facts.**
   Stable material stays hand-authored. Volatile material is generated from code,
   manifests, audits, and tests whenever possible.

3. **Docs are layered by job, not by habit.**
   A document should explain, state, plan, prove, or navigate. It should not do
   several of those at once.

4. **Newcomer-first top layer.**
   Top-level docs should optimize for “What is this?”, “Can I use it?”, and “How
   do I run it?” before internal implementation history.

5. **CI must enforce the contract.**
   If documentation architecture matters, violations must block.

## 5. Canonical Documentation Model

### 5.1 Top-level roles

| Document | Role | Audience | Allowed Content | Forbidden Content |
| --- | --- | --- | --- | --- |
| `README.md` | Landing page | Newcomers, users | project definition, differentiators, design constraints, short status snapshot, quickstart, install links, deeper-doc links | long compatibility inventories, sprint logs, benchmark tables, per-module status, roadmap detail |
| `docs/getting-started.md` | First-run guide | New users | install, verify, build/run hello, platform pitfalls, common recovery | roadmap, architecture deep dives, broad compatibility inventories |
| `docs/INDEX.md` | Navigation hub | Contributors, operators | map of canonical docs | substantive status claims beyond a one-line descriptor |
| `docs/spec/STATUS.md` | Current-state ledger | Users, contributors | supported now, unsupported now, active blockers, validation summary, generated status blocks | roadmap sequencing, long historical diaries, duplicated install/quickstart |
| `ROADMAP.md` | Forward plan | Contributors, users tracking direction | priorities, milestones, blockers, sequencing, future work | claims about current support except by linking to `STATUS.md` |

### 5.2 Compatibility surfaces

Compatibility documentation under
`docs/spec/areas/compat/` is split into two kinds:

- **Hand-authored interpretive docs**
  - contracts
  - policy boundaries
  - indexes and explanatory guides
  - execution plans

- **Generated or tightly structured evidence docs**
  - stdlib coverage summaries
  - platform/version availability
  - intrinsic backing counts
  - differential coverage summaries
  - backend/native/wasm parity rollups

Hand-authored compatibility docs should explain how to read the system, not
duplicate large mutable inventories in prose.

### 5.3 Core ownership rule

Molt documentation adopts this invariant:

- `README.md` explains.
- `STATUS.md` states.
- `ROADMAP.md` plans.
- compatibility surfaces prove.
- generated docs count and enumerate.
- `docs/INDEX.md` navigates.

No document may do more than one of those jobs as its primary function.

## 6. Source-Of-Truth Rules

### 6.1 Stable facts

These may live in hand-authored docs:

- project definition;
- explicit design exclusions (`exec`, `eval`, runtime monkeypatching, unrestricted reflection);
- architectural intent;
- policy boundaries;
- documentation navigation;
- roadmap priorities.

### 6.2 Volatile facts

These should be generated or inserted from generated sources wherever possible:

- compatibility counts;
- per-module stdlib status summaries;
- per-version/per-target availability;
- benchmark rollups and performance deltas;
- current validation pass/fail aggregates;
- backend/native/wasm parity snapshots;
- inventory-style coverage tables.

### 6.3 Duplication policy

If a fact is volatile, it must not be hand-maintained in more than one place.

Examples:

- README may say “Molt targets CPython 3.12+ semantics with explicit design exclusions.”
- README may not carry a hand-written 80-line limitations inventory copied from `STATUS.md`.
- ROADMAP may reference an active blocker class, but the canonical statement of
  current support must remain in `STATUS.md`.

## 7. Anti-Drift Enforcement

### 7.1 Generated summary blocks

`docs/spec/STATUS.md` should contain generated blocks for volatile summary data,
for example:

- compatibility summary;
- validation summary;
- benchmark summary;
- backend parity summary.

Recommended mechanism:

- checked-in marker blocks such as
  `<!-- GENERATED:compat-summary:start -->`
  and
  `<!-- GENERATED:compat-summary:end -->`;
- a tooling script rewrites only the marked regions;
- CI fails if generated regions are stale.

This keeps `STATUS.md` readable without making humans sync clerks.

### 7.2 Documentation lint gates

Add a repo-local docs gate script that fails on:

1. stale generated documentation blocks;
2. banned sections or banned content classes in `README.md`;
3. current-state claims duplicated into `ROADMAP.md`;
4. roadmap sequencing material duplicated into `STATUS.md`;
5. missing cross-links to canonical owners where required.

Examples of enforceable bans:

- `README.md` must not contain “Optimization Program Kickoff”.
- `README.md` must not contain large module-by-module compatibility tables.
- `ROADMAP.md` must not contain “Last updated” state ledgers or capability lists.

### 7.3 Documentation update workflow

Any change that materially affects support, compatibility, or positioning must
update one of three owners:

- `README.md` for newcomer-visible framing changes;
- `docs/spec/STATUS.md` for current-state changes;
- `ROADMAP.md` for forward-plan changes.

Generated evidence updates happen through tooling, not freehand edits.

## 8. Proposed Document Shapes

### 8.1 `README.md`

Recommended sections:

1. One-paragraph project definition.
2. Why Molt exists / differentiators.
3. What Molt supports today.
4. What Molt intentionally does not support.
5. Five-minute quickstart.
6. Install options.
7. Honest status snapshot with links to `STATUS.md` and compatibility docs.
8. Links to deeper docs.

Target style:

- short;
- confident but honest;
- minimal numbers unless they are stable and high-signal;
- no internal sprint-management prose.

### 8.2 `docs/getting-started.md`

Recommended sections:

1. Prerequisites.
2. Install.
3. Verify installation.
4. Build and run a hello-world example.
5. Run a simple benchmark or compare flow.
6. Platform pitfalls and common recovery paths.

### 8.3 `docs/spec/STATUS.md`

Recommended sections:

1. Project scope and target.
2. Supported today.
3. Intentionally unsupported.
4. Known major gaps / blockers.
5. Validation summary block.
6. Compatibility summary block.
7. Performance summary block.
8. Pointers to detailed compat surfaces.

The document should stop being a commit-history narrative.

### 8.4 `ROADMAP.md`

Recommended sections:

1. Strategic target.
2. Current top priorities.
3. Milestone sequencing.
4. Active blockers.
5. Deferred work / non-goals.

The document should stop being a mixed roadmap-plus-status artifact.

## 9. Compatibility Simplification Strategy

The compatibility layer is necessary, but it needs stronger structure.

### 9.1 Keep

- contracts;
- surface indexes;
- execution plans;
- generated audit files;
- clearly-scoped hand-authored matrices where generation is not yet practical.

### 9.2 Consolidate

- overlapping prose descriptions of the same surface;
- historical notes that belong in plans or commit history, not in active matrices;
- hand-written rollup summaries that can be derived from generated artifacts.

### 9.3 Generate next

Priority generated summaries:

1. top-level compatibility rollup for `STATUS.md`;
2. stdlib module support summary by status bucket;
3. backend parity summary (native / wasm_wasi / wasm_browser where applicable);
4. validation summary from differential/conformance tooling;
5. benchmark summary from canonical result artifacts.

## 10. Migration Plan

### Phase 1: Establish ownership and simplify top-level docs

- Rewrite `README.md` as a true OSS landing page.
- Add `docs/getting-started.md`.
- Rewrite `docs/spec/STATUS.md` into a short current-state ledger.
- Rewrite `ROADMAP.md` into future-only planning.
- Trim `docs/INDEX.md` into a pure navigation surface.

### Phase 2: Add anti-drift tooling

- Add a docs state generator for `STATUS.md` summary blocks.
- Add docs lint rules for banned duplication patterns.
- Add CI enforcement for stale generated docs and ownership violations.

### Phase 3: Simplify compatibility surfaces

- Collapse overlapping hand-authored summaries.
- Move rollup content out of prose matrices and into generated summaries.
- Keep detail where it is valuable, but remove duplicate narrative.

### Phase 4: Iterate on evidence quality

- Improve generated summaries as coverage metadata becomes more structured.
- Expand parity reporting across native and wasm lanes.
- Tighten CI rules as the generated pipeline becomes reliable.

## 11. Acceptance Criteria

This design is successful when all of the following are true:

1. A newcomer can read `README.md` and understand Molt without reading sprint logs.
2. A new user can follow `docs/getting-started.md` to a successful first run.
3. `STATUS.md` is short, current-state only, and partly generator-owned.
4. `ROADMAP.md` is forward-looking only.
5. The repo has an automated docs gate that catches stale generated state.
6. Volatile status facts no longer appear as hand-maintained prose in multiple top-level docs.
7. Compatibility summaries are easier to consume without weakening evidentiary detail.

## 12. Risks And Mitigations

### Risk: over-generation produces unreadable docs

Mitigation:

- generate summaries, not full prose;
- keep interpretive and narrative writing hand-authored;
- use generated blocks only for volatile state.

### Risk: migration churn breaks links or discoverability

Mitigation:

- keep canonical paths stable where possible;
- prefer rewriting content in place over moving everything at once;
- preserve navigation from `docs/INDEX.md`.

### Risk: “single source of truth” becomes aspirational only

Mitigation:

- encode ownership in docs and CI;
- ban duplicated volatile claims in top-level docs;
- fail the build on stale generated blocks.

## 13. Recommendation

Proceed with the documentation overhaul as a focused architecture change, not as
a broad editorial sweep.

Implementation order should be:

1. rewrite top-level docs into their new roles;
2. add anti-drift tooling and CI enforcement;
3. consolidate compatibility rollups behind generated summaries.

This yields the biggest improvement for external users immediately while also
reducing long-term maintenance cost.
