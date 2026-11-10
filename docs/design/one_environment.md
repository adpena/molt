# One environment mechanism

Status: design, 2026-11-07, revised after one Goldfish round (zoom-out and codebase-details;
both "revise"; findings below). Owner: integrator. Owner decision (2026-11-07): replace the
custom development-image and custody system with uv-managed environments, plain `cargo test`,
and a small fixed landing suite, in staged deletions.

## Problem (measured 2026-11-07)

- Development images accumulate until stage 3 lands: the retired HF-694 retention branch's
  inventory counted 174 images (26.47 GiB) on one host, all under 30 days old, because
  custody_layout.py keeps them `Regenerable(month)`. Stage 3's single synced environment per
  key removes the pile-up; until then the cheap lever is that one retention age, not a new
  cleaner.
- One checkout had five Python environments (checkout `.venv`, content-keyed development image,
  pinned controller `.venv`, CI's bare provisioned `python3`, each farm host's image). They drifted
  the same day: `.venv` stayed on 3.12.15 after the pin moved to 3.12.16; CI's proof executor had
  no pytest (200 of 200 `main` CI runs red); a pin move left images "not selected", and
  `tools/venv_exec.py` refused instead of building one.
- The image layer is 7,981 lines (`src/molt/source_build_environment*.py`,
  `python_environment*.py`, `checkout_provisioning.py`, `python_uv_lock_identity.py`) with at
  least 44 non-test consumer files (13 `src/molt`, 28 `tools`, 3 test harnesses). It attests
  runtime closures and pytest's rewritten bytecode (HF-522).

## What the layer does that has a real consumer (keep the guarantee, not the layer)

| Need | Consumers | Kept as |
| --- | --- | --- |
| Development environment (pin + `dev` and `bench` groups) | every tool, gate, landing | the environment of the default key |
| One environment per target CPython (3.12, 3.13, 3.14) | `tests/molt_diff.py:247-253`, `tools/compat/backends.py:742-756`, `tools/dev.py:483-499`, CI target-pythons | key `(python=X, groups=default)` |
| One environment per dependency group (pact-witness, source-build-numpy, source-build-scipy) | `src/molt/cli/source_extension_producer.py:2347`, `tools/proof_queue_pkg/pact.py:171,603`, `command_admission.py:1386-1409` | key `(python=pin, groups=G)` |
| Interpreter identity probe | `molt compare` (`script_commands.py:1061-1067`), `startup_bench.py:228`, `perf_instructions._resolve_cpython`, `cli/deps.py:35`, `python_interpreter.py` | one small module, moved out of the deleted layer |
| Release interpreter provenance | `tools/release/verify_consumer.py:61-63`, `python_capture.py:54-61` | recorded from the identity probe plus `uv.lock` (already in the release payload, `compiler_payload.py:53,70`) |
| Receipt identity for gates | `tools/proof_queue_pkg/command_identity.py:34,48,670-724` | `uv.lock` digest plus interpreter version |
| Non-uv checkout inputs: Acorn, edgebox-typescript, Git hooks | `checkout_provisioning.py:59-61`, `ci.yml:59`, `molt-wasm-ci.yml:66` | plain idempotent install checks with their owners |

## Design

1. **One function** `environment(key) -> Path` with `key = (python, groups)`. It returns
   `<artifact root>/venvs/<checkout>/<python>-<groups digest>` (outside the checkout, as CI and
   `dx` require: `dx.py:2822-2829`, `tests/test_ci_workflow_topology.py:3988-3989`) and runs
   `uv sync --locked --python <python> --no-default-groups --group ...` with
   `UV_PROJECT_ENVIRONMENT` set to that path. uv no-ops in milliseconds when current and
   rebuilds when the lock or pin changed, so no entry point refuses a moved pin. The default key
   is `(.python-version, [dev, bench])`; `pyproject.toml` declares `[tool.uv] default-groups` to
   match, so plain `uv run` in a shell uses the same groups.
2. **Sync points, then no sync**: entry points (gate runner, landing, CI setup, farm host
   setup, `molt_diff` frontend selection) call `environment(key)` once; commands they launch run
   that environment's interpreter directly (`bin/python`, `Scripts\python.exe` on Windows) with
   `UV_NO_SYNC=1`, as today.
3. **Delete**: development-image generations, selection records, provisioning stamps and leases
   for Python environments (`checkout_state.py`, `custody_layout.py:280-291`), runtime-closure
   and bytecode attestation, `tools/venv_exec.py` (both modes), `tools/uv_project_env.py`'s
   wrapper kind once its callers move, and the environment-registry rows that only describe them.
4. **The pinned controller** keeps its own environment because it must run an older main's
   driver; it uses `environment(key)` like everything else.

## Stages (each lands with its deletions; no stage leaves two live mechanisms)

1. **CI first** (independent, worst measured failure): `setup-project` activates the synced
   environment (landed in lane apparatus-exit, 3be76b90fb); then `setup-uv` and one `uv sync`.
2. **Identity probe and receipt key**: move the probe out of the layer; switch receipt identity to
   `uv.lock` digest plus interpreter version; switch release provenance to the probe.
3. **`environment(key)` and every consumer family at once** (development, target-version and
   group environments; gate runner, landing executor, `molt_diff`, extension builds, pact,
   farm hosts, hooks), deleting the image layer, `venv_exec.py` and their implementation tests in
   the same landing.

The landing suite is owned by `docs/design/presubmit_test_strategy.md`; this design does not
restate it.

## Goldfish round 1 (2026-11-07): applied

Zoom-out: stage 4 duplicated the strategy (removed); delete `venv_exec.py` entirely (applied);
version and group environments were dropped (now keys); the deletion globs caught product and
release code (now a consumer table); stages left half-migrated states (stage 3 is one migration);
fix CI first (stage 1). Codebase-details: consumer count corrected to 44+; edgebox-typescript
added; `uv run` does not reconcile today because entry points force `UV_NO_SYNC` (design 2 makes
sync points explicit); Windows interpreter path; release provenance and the identity probe have
owners; checkout `.venv` conflicts with `dx` and CI topology (environments live under the artifact
root); `uv sync` would drop `bench` (default-groups); `build_inputs.py:1170-1173` site-packages
roots are unaffected because environments stay outside the checkout.
