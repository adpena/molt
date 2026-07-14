# Pact Git signal harvest — 2026-07-14

This is the loss-proof ledger for retiring the Pact collaboration branch,
worktree, and stash estate. The audit was performed against integration commit
`fc4da5d084` and refreshed `origin/main` at `4b0df34f76`.

## Inventory

- 29 registered worktrees
- 44 local branches
- 198 non-main remote branches
- 4 shared stashes
- 276 reviewed integration paths committed in `fc4da5d084`
- 188 initially patch-ID-positive remote commits across 130 refs

Patch ID was used only as an initial filter. Every positive family was checked
against the current postimage, sibling consumers, and newer structural commits;
old commits were not replayed merely because their patch ID differed.

## Replayed signal

One genuinely surviving production authority family remained:

- `2c50868470` exposed duplicate `fnmatch` runtime/stdlib ABI authorities.
  `f33e8828e1` manually moved the stdlib consumer to the bytes-aware
  `molt_fnmatch` / `molt_fnmatchcase` authority, deleted the legacy Rust module
  and symbols, and regenerated every intrinsic and WASM ABI consumer. The stale
  local-dependency scanner portion of the old commit was not replayed because
  current runtime wrappers already own that export boundary.

Unique uncommitted evidence was also preserved:

- `16cfe26803` copied the DTypeMeta witness memory-guard artifact byte-for-byte
  and recorded its frontier hash. It contained no production source.

## Represented or superseded families

- 148 decomposition patch groups from 116 refs: 42 groups were represented
  byte-for-byte; 106 were superseded by the current complete WASM, passes,
  backend, concurrency, Luau, ndimage, and split-runtime authority cuts.
- L7/numeric/object changes were already represented by the full L7 harvest.
  Older numeric-carrier, ndarray/buffer, native-division, APDataStore/RunContext,
  build-cache, and app-route branches would restore obsolete ownership or root
  policy and were not replayed.
- GPU text source rendering, exception runtime state, path-scoped structural
  diagnostics, linked Meson static-library closure, ndimage callable
  reachability, wasm comparison module paths, and stdlib intrinsic surface
  enforcement all have equal or deeper current implementations.
- Cargo test failure classification is now the broader `rust-test-failure`
  authority. The old type-facts custody message has no current producer. Missing
  Pact fixtures already have the canonical `pact-witness-fixture-missing`
  diagnosis.
- Old claim-only/status-only commits, instrumentation-only commits, and ad hoc
  source-extension probe scripts contain no product authority.

## Dirty worktrees

- Canonical main: one `ABIHASH` address/slot diagnostic in `typeobj.rs`; the real
  type identity-hash fix and regression are already canonical.
- `wt-codex-l7`: five-file pre-harvest WIP, wholly superseded by the newer L7
  runtime hooks, integer parsing, float narrowing, complex error semantics, and
  generated ABI tables.
- `wt-approute-v2`: two invalid probe checksum files.
- `wt-e1-dtypemeta`: ad hoc launch/build/replay files; only the unique guard
  evidence was retained. The semantic frontier is already in `CLAIMS.md`.
- `wt-e1-instr` and `wt-e1-silentfail`: generated keyed runtime integrity pins.

No dirty worktree contains surviving uncommitted production source.

## Shared stashes

- `stash@{0}` perf-matrix R6 content is represented in current optimization
  matrices, claims, and evidence JSON.
- `stash@{1}` apparatus learning/gate-liveness content is represented by the
  current apparatus gates and tests.
- `stash@{2}` AST cache idempotence/trace content is represented by the current
  cache authority.
- `stash@{3}` ABI functions and manifest entries are represented; its ad hoc
  ccallback build script was superseded by the generic source-extension
  producer.

All four stashes are safe to drop after the integration head is landed.

## Proof for the surviving production replay

- Intrinsic generator check: in sync
- WASM ABI generator check: in sync
- Focused pytest: 28 passed
- Guarded `cargo check -p molt-runtime --profile dev-fast`: return code 0 in
  51.547 seconds; peak process RSS 1.084 GiB. The guard recorded and cleaned one
  tracked post-Cargo orphan descendant; the incident remains preserved at
  `logs/proof_queue/fnmatch_authority_cargo_check.memory_guard.json`.

After the integration head is fast-forwarded to `origin/main`, this ledger is
the authority to delete all non-main worktrees, local branches, remote branches,
and shared stashes without retaining legacy fallback refs or bundles.
