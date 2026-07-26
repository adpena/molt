# 74 — Build Work Deduplication Doctrine (canonical)

**Status:** BINDING doctrine, 2026-07-10. Owner: orchestrator. Implements the
operator mandate: *work done successfully is NEVER redone; compatible artifacts
are reused across configs; failure does not forfeit progress.*

## The four laws

Every build step in every Molt pipeline (runtime crates, backend, C-ext seals,
frontend lowering, app link, wasm-opt, witness E2E) MUST satisfy:

1. **Content-addressed.** A step's output is keyed by the content hash of its
   true inputs (sources + config + tooling fingerprint). Identical key ⇒ the
   step is SKIPPED and the artifact reused — across runs, sessions, worktrees,
   and machines. A step that re-executes on an unchanged key is a bug (the
   "redo class") and gets a failing attestation.

2. **Failure-resilient (partial-progress persistence).** A failed BUILD must
   not forfeit its successful STEPS. Every sub-artifact (a module's lowering,
   a crate's compile, a sealed extension) is committed to cache the moment IT
   succeeds — not at end-of-build. The retry after a failure pays only for the
   failed step forward. "Cold compile once, incremental forever — even through
   failures."

3. **Config-lattice aware.** Configs form a partial order (opt level, feature
   superset, debug-info). A request MAY be satisfied by an existing artifact
   that is *compatible-or-better* (e.g. a `dev-fast` iteration request served
   by an existing `release-output` runtime; a `micro`-profile request served
   by a `full`-stdlib artifact whose feature set is a superset) when semantics
   are identical for the consumer. Lattice reuse is opt-in per consumer
   (`MOLT_BUILD_REUSE_COMPATIBLE=1` for heavy dev environments) until proven
   safe per edge, then default-on. Runtime WASM publication is explicitly not a
   lattice consumer: its integrity receipt and every reuse/hydration path pin
   exact source/config/toolchain identity (M05).

4. **Attested.** Every step emits `{key, hit|miss, reason-if-miss, wall}` into
   the build diagnostics (MOLT_BUILD_DIAGNOSTICS). "Configured ≠ effective" is
   the known failure class (M34/M55): a cache that exists but cold-starts every
   session is a FAILING cache. Hit-rate is a gated metric, not a hope.

## Known violations to burn down (P0 lanes)

| # | Violation | Law | Lane |
|---|---|---|---|
| V1 | Runtime-wasm dual pass: shared/cdylib pass recompiles the whole crate because link-args live in RUSTFLAGS (fingerprint poison); crate-type override splits what Cargo.toml already declares | 1 | B |
| V2 | Fresh session/worktree ⇒ cold CARGO_TARGET_DIR ⇒ ALL dependency crates recompile despite identical sources+toolchain | 1 | B |
| V3 | `dev`/`dev-fast` iteration rebuilds runtime even when a release-output artifact for identical sources exists (no lattice lookup) | 3 | B |
| V4 | Witness/frontend lowering cold-starts per session (M55 residual); hit-rate unattested | 1,4 | C |
| V5 | Failed witness build forfeits completed module lowerings/seals ⇒ full re-lower on retry | 2 | C |
| V6 | E2E pipeline steps (C-ext seals, app link, wasm-opt) re-execute without a key check on unchanged inputs | 1,4 | C |

## Non-negotiables

- No weakened determinism for acceptance: proof/acceptance paths pin exact
  content identity; lattice reuse never substitutes there (M05).
- Reuse must be *provably identical or better* for the consumer — an artifact
  with more features/higher opt is reusable only where the consumer's
  semantics don't observe the difference; each lattice edge lands with a test.
- Every cache gets an eviction policy and a kill-switch env var.
- Attestation is part of the definition of done for any cache work: a lane
  that lands a cache without hit-rate attestation has not landed (M12).
