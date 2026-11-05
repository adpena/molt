# Locked artifact identity for Python environments

Ledger row: HF-771, family F (observed inputs and content identity).

## Problem

`molt.python_uv_lock_identity` derives a development or source-build image's
identity from `uv.lock`. For each selected package, `_locked_artifact` picks the
locked wheel whose tags best match the active target, and it refuses the package
when no wheel matches.

uv does not refuse. When no locked wheel matches the target, uv installs the
locked source distribution: it builds a wheel on the host, then installs that.
So the identity rejects an environment that uv builds and installs correctly.

Measured on 2026-11-04 on the Intel Mac mini (macOS x86_64, CPython 3.12.15):
`uv sync --locked --group dev` built `cbor2==6.1.5` from its sdist. The worker's
image publication then refused with "locked package 'cbor2' has no wheel for the
active OS/arch/ABI". cbor2 6.1.5 publishes wheels for macOS arm64, Linux and
Windows only. macOS x86_64 is a v1 support cell, so its farm cell cannot run.
Every sdist-only package on any target fails the same way.

## Review record

Two Goldfish reviews (design, codebase details) on 2026-11-04. Accepted: the
match proves tags, not bytes; build provenance and host-local images; universal
tags; uv's build options; the missing `wheels` key; the contract gate as the
invalidation path; kind-first validation. Answered: a second v4 read path is not
needed, because a contract change already republishes images. Not adopted:
snapshotting installed `RECORD` hashes, since no consumer verifies installed
files against the lock today.

## Prior art

- uv: `Package::to_dist` in `crates/uv-lock/src/lock/mod.rs` takes the best
  wheel by tag priority (`find_best_wheel`), then the greatest build tag. With
  no compatible wheel it falls back to the locked sdist, unless `no-build`
  forbids building. `_locked_artifact` already mirrors the wheel half.
- PEP 751 (`pylock.toml`): a package lists `wheels` and an optional `sdist`. An
  installer selects a compatible wheel, and otherwise may build the sdist.
- pip: prefers a compatible wheel and builds the sdist otherwise
  (`--only-binary` turns that off).

## Decision

The locked artifact records which kind of file uv installs from. It is a closed
type: `kind` is `"wheel"` or `"sdist"`.

1. **Selection.** Keep today's wheel choice. When no locked wheel matches the
   target's tags, select the row's `sdist` (`url`, `hash`, `size`) as
   `kind = "sdist"`. uv omits `wheels` from an sdist-only row, so a missing
   `wheels` key means no wheels, not an invalid row. When the row has neither a
   matching wheel nor an `sdist`, refuse with "locked package X has neither a
   wheel for the active OS/arch/ABI nor an sdist". The lock's single `sdist`
   entry is authoritative. Rows keep today's registry-source and hash checks: a
   git, path, URL or editable source, or a missing hash, is refused.
2. **uv's build options.** `no-build`, `no-binary` and their `-package` forms
   change uv's choice: they forbid the sdist or force it over a matching wheel.
   Molt sets none of them, and the identity does not model them. Selection reads
   `[tool.uv]` in `pyproject.toml` and refuses when any of those four keys is
   present, or when a `uv.toml` exists beside it. `UV_NO_BUILD` and
   `UV_NO_BINARY` in the provisioning process are an unverified coordinate: this
   change does not read the provisioning environment.
3. **Closure record.** Each package's `artifact` is
   `{kind, filename, size, sha256}`. For a wheel, the filename must parse as a
   wheel, its name and version must equal the row's, and one of its tags must be
   a target tag (unchanged). For an sdist, the filename must parse as an sdist
   (`packaging.utils.parse_sdist_filename`), and its name and version must equal
   the row's. An sdist carries no tags. The validator checks `kind` before it
   parses the filename, so an sdist name never reaches the wheel parser.
4. **Installed environment match.** `environment_matches_lock_closure` compares
   each installed distribution's `WHEEL` metadata with its artifact.
   - Wheel: unchanged. Installed tags lie within the filename's tags, at least
     one is a target tag, and the build tags are equal.
   - Sdist: the wheel uv built must carry at least one tag, all its tags must be
     target tags, and it has no build tag. The target's tag set includes the
     universal tags (`py3-none-any`), so a pure-Python sdist matches. This shows
     the installed wheel was built for this target. It does not prove which
     bytes were built: as for a locked wheel today, the match compares name,
     version and tags, never file hashes. uv enforces the lock's hash when it
     downloads either file (`uv sync --locked`).
5. **Schema.** `UV_LOCK_GROUP_CLOSURE_SCHEMA` moves from v4 to v5. It is part of
   `source_build_environment_contract()`, the `producer_contract` in every
   recipe and manifest. A contract change gives each image a new selection
   stamp, so the next provision publishes a new image, and the old one is
   unreferenced (`tests/test_checkout_provisioning.py` proves this for a version
   bump). Nothing is stranded, and no second read path is kept.

## Invariants

- The identity's per-package artifact is exactly the file uv installs from for
  this target: the same wheel, or the sdist when uv falls back to building.
- A content change to either file changes `sha256`, so the closure hash and the
  image address change.
- An sdist-built distribution is not bit-reproducible across hosts. The host's
  compiler and the build requirements uv resolves at build time shape it, and
  `uv.lock` records neither. Building also runs the package's build backend, as
  `uv sync` already does today. The image is already per-host: its address
  binds the runtime closure and the target tags. Its identity is the sdist's
  content plus that host binding, not the built wheel's bytes. `kind = "sdist"`
  marks such a package, so a consumer that needs reproducible bytes can refuse
  it. Development images are never shared between hosts.

## Consumers migrated in one change

`_locked_artifact`, `validate_uv_lock_group_closure` and
`environment_matches_lock_closure` in `src/molt/python_uv_lock_identity.py`, plus
the schema constant in `src/molt/python_environment_contract.py`. Only this
module reads `packages[*].artifact`. The contract gates
(`source_build_environment_contract_matches` in
`source_build_environment_schema.py`, `source_build_environment.py`,
`source_build_environment_selection.py` and `checkout_provisioning.py`) compare
the schema through `producer_contract` and need no change. Other closure readers
take only the extras and package names (`source_build_environment_schema.py`,
`cli/source_build_inventory.py`, `source_build_environment.py`) or re-export the
functions (`python_environment_identity.py`). Test
authorities and fixtures that build or inspect the artifact:
`tests/python_environment_test_support.py`,
`tests/test_python_uv_lock_identity.py` and
`tests/test_python_identity_adversarial.py`. Real-lock readers re-run unchanged:
`tests/cli/test_source_build_environment_probe.py`,
`tests/cli/test_source_extension_producer.py` and
`tests/tools/test_proof_queue.py`.

## Test plan

- Selection: a row with a compatible wheel selects the wheel. A row with only
  incompatible wheels plus an sdist selects the sdist, and so does a row with an
  sdist and no `wheels` key. A row with neither refuses. A build-tag tie keeps
  uv's order (existing test). A `[tool.uv]` build option or a `uv.toml` refuses.
- Validator: refuses an unknown kind, a wheel filename recorded as an sdist and
  the reverse, an sdist whose name or version differs from the row, and a v4
  record.
- Match: an sdist-built installed wheel whose tags are all target tags matches,
  including `py3-none-any`. A build tag fails, a non-target tag fails, and no
  tags fails.
- Real lock: the closure for `uv.lock`'s dev group on a macOS x86_64 target tag
  set selects cbor2's sdist and every other package's wheel.
- Live: the macmini `portability.queue.macos` cell gets past image publication.
