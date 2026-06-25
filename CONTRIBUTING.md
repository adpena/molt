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
6. Start every nontrivial change by naming one tiny aperture: the invariant,
   command family, file cluster, authority surface, or failing execution path
   that exposes the real structure.
7. If a maintainer says "tiny slice", "rip it open", or says the work is being
   sliced too small, treat that as a binding scope correction: finish the live
   structural rip exposed by the aperture instead of converting it into
   checkpoint commits.

## Quality Bar

- Prefer canonical homes over ad hoc files or directories.
- No silent fallback paths for missing features or missing infrastructure.
- No test-only hacks, compatibility shims, or narrow fixes that leave structural debt behind.
- Do not optimize for tiny visible progress. Use the tiny slice only as the
  opening aperture, then identify the whole bug class or duplicated-authority
  cluster behind it and land an end-state subsystem cut that removes every
  sibling source of truth in that boundary.
- Convenient tiny-chip progress is a project failure mode: it preserves scattered
  authority while pretending to create velocity. When a maintainer says a change
  is too narrow, widen the work to the coherent structural class instead of
  defending or renaming the narrow cut.
- **Structural aperture, full rip.** The legitimate bounded unit is a
  complete structural rip through one invariant, authority cluster, or execution
  path: the missing IR fact, the one authority, the ownership boundary, and the
  whole bug class it exposes inside that boundary, with zero workarounds. A plan
  is not a deliverable. Narrow the entry point only to expose the structure; once
  exposed, widen to every sibling authority and consumer needed to delete the old
  lane. Never patch the surface, never boil the ocean, and never mistake a
  checkpoint chip for engineering progress.
- **Tiny slice and rip it open.** This is binding operator policy. A tiny slice
  is the entry aperture, not the deliverable or work-size limit. It means one
  concrete invariant, command family, file cluster, authority surface, or failing
  execution path that exposes the real structure; then rip through all sibling
  consumers needed to delete or unify the duplicate lane before moving on.
- Crash recovery is the exception that proves the rule: after Codex, Claude,
  Desktop, WSL bridging, process custody, subagent orchestration, or a guarded
  command crashes, stalls, disappears, or gets manually killed, stabilize the
  control plane by reducing concurrency and recording death-capsule evidence.
  Recovery does not authorize tiny chips; the landing still has to delete or
  unify a real authority without leaving duplicate paths behind.
- In recovery mode, keep exactly one active structural arc and one bounded proof
  lane. Use subagents only for disjoint mapping or migration inside that arc,
  not for parallel proof storms or status traffic.
- Process cleanup is allowed only for live-proved Molt-owned workers: build,
  test, bench, backend-daemon, runtime-child, or guard-owned process groups whose
  current identity proves they belong to this repo's Molt work. Codex, Claude,
  app-server, renderer, node-repl, MCP/plugin helpers, shell hosts, Git pollers,
  and ancestor/control-plane processes are never cleanup targets.
- Keep repo-facing docs and examples accurate after every move or rename.
- Verify only the claims you make. Fresh command output is required for
  correctness/performance/support claims, but broad proof lanes are not a
  substitute for finishing the structural change.

## Required Verification

- Use a bounded evidence budget: run one static or targeted check that can catch
  integration mistakes for the complete structural work class you touched. Do
  not shrink the implementation to match an easier proof lane.
- Keep pre-commit hooks read-only. Formatting and automatic fixes must be run
  explicitly before staging so commit hooks cannot rewrite files mid-commit.
- Use the canonical CLI DX surface for repo-wide proof only when making a
  repo-wide claim, preparing release/merge evidence, or responding to an
  explicit maintainer request:
  - `molt setup`
	  - `molt doctor`
	  - `molt validate --suite smoke`
	  - `molt validate --suite custody-proof`
	  - `molt validate --suite smoke --backend luau`
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
