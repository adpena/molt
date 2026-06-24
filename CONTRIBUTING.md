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
- Do not optimize for tiny visible progress. Identify the whole bug class or
  duplicated-authority cluster first, then land an end-state subsystem cut that
  removes every sibling source of truth in that boundary.
- Convenient tiny-chip progress is a project failure mode: it preserves scattered
  authority while pretending to create velocity. When a maintainer says a change
  is too narrow, widen the work to the coherent structural class instead of
  defending or renaming the narrow cut.
- **Force a tiny slice, then rip it open.** The legitimate small unit is the
  smallest slice that still cuts through the REAL structure end-to-end (one case,
  one path, one invariant), *ripped fully open* — the missing IR fact, the one
  authority, the ownership boundary, and the whole bug class it exposes inside
  that boundary, with zero workarounds, until correct + measured + gated. That is
  the OPPOSITE of the convenient chip above: tiny in scope, total in depth, no
  sibling of its bug class left un-migrated. A plan is not a slice — investigation
  may *find* the slice, but the deliverable is the ripped-open slice. Shrink the
  scope and deepen the rip; never patch the surface, never boil the ocean.
- Crash recovery is the exception that proves the rule: after Codex, Claude,
  Desktop, WSL bridging, process custody, subagent orchestration, or a guarded
  command crashes, stalls, disappears, or gets manually killed, use tiny complete
  structural primitives. Each primitive must be staged, focused-tested, and
  committed before the next risky lane, with a death capsule in the canonical
  evidence roots.
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
