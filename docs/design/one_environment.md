# One development environment per checkout

Status: design, 2026-11-07. Owner: integrator. Owner decision (2026-11-07): replace the custom
development-image and custody system with one uv-managed environment per checkout, plain
`cargo test`, and a small fixed landing suite, in staged deletions.

## Problem (measured 2026-11-07)

- A checkout had five Python environments: the checkout `.venv`, the content-keyed development
  image (`build-environments/development/<checkout>/<digest>`), the pinned controller image, CI's
  bare provisioned `python3`, and each farm host's copy. Each drifted on its own the same day:
  `.venv` stayed on 3.12.15 after the pin moved to 3.12.16; CI's executor had no pytest (every
  `python_unit` command failed before starting, 200 of 200 CI runs red); a pin move left images
  "not selected" and `tools/venv_exec.py` refused instead of building one (provisioning imports
  `packaging`, which its bootstrap interpreter lacks).
- The image system is about 8,000 lines (`src/molt/source_build_environment*.py`,
  `python_environment*.py`, `checkout_provisioning.py`, `python_uv_lock_identity.py`) with about
  34 consumer files. It attests runtime closures and even pytest's rewritten bytecode (HF-522).
- The environment, guard, precheck and landing core is 46 files and 51,234 lines; `tools/` is
  329,265 lines against about 249,000 for the frontend, TIR passes and native backend.

## Prior art

- uv: `uv.lock` and `.python-version` define the environment exactly; `uv sync --locked` builds or
  updates `.venv` and is a no-op in milliseconds when current; `uv run` syncs before running.
- Cargo and Go: one toolchain file and one lock; the build tool reconciles stale state on demand;
  no separate image registry, selection record or custody stamp.
- Bazel uses content-addressed toolchains because it caches remotely across machines; Molt does
  not need that for development, and release builds are made fresh in CI.

## Design

1. **One environment**: uv's default project environment, `.venv` in the checkout, from
   `uv.lock` and `.python-version`. Every entry point (`uv run python tools/...`, the consumer
   gate, the landing executor, CI after `setup-project`, farm hosts) uses it. `uv run` reconciles
   a stale environment itself, so no tool refuses a moved pin.
2. **No selection layer**: delete development-image generations, selection records, provisioning
   stamps for the Python environment, runtime-closure and bytecode attestation, and
   `tools/venv_exec.py`'s managed mode. `checkout_provisioning` keeps only inputs uv does not own
   (Acorn, Git hooks), or those move to their owners.
3. **Landing runs in the lane's `.venv`** after `uv sync --locked`; a gate's interpreter is
   `.venv/bin/python`. The executor's toolchain contract checks the interpreter's exact version
   against `.python-version` (one comparison), not an image identity.
4. **CI**: `astral-sh/setup-uv` with `activate-environment` and `uv sync --locked`; drop the bespoke
   bootstrap interpreter for steps after the sync.

## Guarantees kept, and their owners

- Exact interpreter and dependency versions: `.python-version` and `uv.lock` (uv enforces).
- No network or download during tests: `UV_OFFLINE`/`UV_PYTHON_DOWNLOADS=never` after setup.
- Release reproducibility: the release workflow records `uv.lock`'s digest and the interpreter
  version in its manifest; it builds in a fresh CI environment.

## Stages (each lands with its deletions and their implementation-only tests)

1. Entry points: AGENTS.md, the consumer gate, `venv_exec`, `molt_dev` gate environment and the
   landing executor resolve `.venv` through one function; the image path becomes unused.
2. Delete the image system and its tests; keep Acorn/hooks provisioning as plain checks.
3. CI and farm hosts: `setup-uv` + `uv sync --locked`; delete the bootstrap interpreter steps.
4. Landing suite: fixed and small (static checks with a demonstrated catch, changed crate tests,
   first-hop importers by changed function, a calibrated CPython differential smoke); measured
   before and after with `tools/gate_yield.py`.

## Risks

- Concurrent readers during a re-sync: a lane re-syncs only when `uv.lock` or the pin changes;
  uv locks the environment while it writes.
- Tests that assumed the image path or attested closures: they are implementation tests of the
  deleted layer and go with it; behavior tests keep running under `.venv`.
