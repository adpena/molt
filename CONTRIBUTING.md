# Contributing

Molt expects production-grade changes, not opportunistic patches.

## Workflow

1. Read [AGENTS.md](AGENTS.md), [docs/CANONICALS.md](docs/CANONICALS.md), and [docs/ROOT_LAYOUT.md](docs/ROOT_LAYOUT.md) before changing behavior, structure, or docs.
2. Keep work on `main` unless explicit user approval says otherwise.
3. Use canonical artifact roots only:
   - `target/` for Cargo/build state
   - `bench/results/` for benchmark outputs
   - `logs/` for durable logs
   - `tmp/` for scratch and quarantine material
4. Update docs in the same change when structure, workflow, or semantics move.
5. Remove dead files, duplicate paths, and stale references instead of preserving legacy layout.

## Quality Bar

- Prefer canonical homes over ad hoc files or directories.
- No silent fallback paths for missing features or missing infrastructure.
- No test-only hacks, compatibility shims, or narrow fixes that leave structural debt behind.
- Keep repo-facing docs and examples accurate after every move or rename.
- Verify claims with fresh command output before committing.

## Required Verification

- Run the smallest command set that proves the touched paths still work.
- Prefer the canonical CLI DX surface for repo-wide proof:
  - `molt setup`
  - `molt doctor`
  - `molt validate --suite smoke`
  - `molt validate`
- For repo-structure changes, verify:
  - moved paths are updated everywhere relevant
  - canonical docs link to live files
  - no stale references remain to removed directories
  - artifact and cache locations follow policy

## Documentation Contract

- `docs/` is canonical, reader-facing documentation only.
- Internal planning, agent memory, scratch notes, and quarantine bundles stay outside the repo.
- Root should stay limited to stable entrypoints, manifests, and approved top-level directories.
