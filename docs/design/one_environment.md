# One environment mechanism

Status: stage 1 landed; stage 3a landed with this revision (2026-11-21); stages 2 and 3b
pending. Owner: integrator. Owner decision (2026-11-07): replace the custom development-image
and custody system with uv-managed environments, plain `cargo test`, and a small fixed landing
suite, in staged deletions. Owner direction (2026-11-20): one environment, not one per
worktree ("Shouldn't there just be one?").

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
| One environment per admitted target CPython | `tests/molt_diff.py:247-253`, `tools/compat/backends.py:742-756`, `tools/dev.py:483-499`, CI target-pythons | key `(python=X, groups=default)` |
| One environment per dependency group (pact-witness, source-build-numpy, source-build-scipy) | `src/molt/cli/source_extension_producer.py:2347`, `tools/proof_queue_pkg/pact.py:171,603`, `command_admission.py:1386-1409` | key `(python=pin, groups=G)` |
| Interpreter identity probe | `molt compare` (`script_commands.py:1061-1067`), `startup_bench.py:228`, `perf_instructions._resolve_cpython`, `cli/deps.py:35`, `python_interpreter.py` | one small module, moved out of the deleted layer |
| Release interpreter provenance | `tools/release/verify_consumer.py:61-63`, `python_capture.py:54-61` | recorded from the identity probe plus `uv.lock` (already in the release payload, `compiler_payload.py:53,70`) |
| Receipt identity for gates | `tools/proof_queue_pkg/command_identity.py:34,48,670-724` | `uv.lock` digest plus interpreter version |
| Non-uv checkout inputs: Acorn, edgebox-typescript, Git hooks | `checkout_provisioning.py:59-61`, `ci.yml:59`, `molt-wasm-ci.yml:66` | plain idempotent install checks with their owners |

## Design

1. **One function** `molt.uv_environment.environment(checkout, key) -> Path` with
   `key = (python, groups)`: an exact CPython release and sorted dependency groups. The
   directory is `<python-environments>/<python>-<groups>-<lock digest>`, where
   `<python-environments>` is the `python-environment` storage class placed through
   `default_paths.hot_path` (the internal disk once the artifact root has a hot-state record,
   HF-942; outside every checkout, as CI and `dx` require). The lock digest covers `uv.lock`
   and the pyproject entries that decide contents (`requires-python`, dependencies, optional
   dependencies, dependency groups, `[tool.uv]`), so a tool-setting edit keeps the directory and
   a lock change selects a new one. The first use takes the directory's lock and runs
   `uv sync --locked --no-install-project --managed-python --python <python>
   --no-default-groups --group ...`, records the interpreter's identity from one child, and
   publishes `molt-environment.json` last. The record proves the directory current, so the warm
   path is one `stat` (2.2 ms with the lock digest, measured 2026-11-21) and runs no uv. The
   project is not installed: each checkout puts its own `src` first on `PYTHONPATH`
   (`dx.ensure_repo_src_pythonpath`), so every worktree with the same lock shares one
   environment. The default key is `(.python-version, [tool.molt.dx].dependency_groups)`.
   A minor selector (`3.13`) resolves once to uv's managed release.
2. **Sync points, then no sync**: entry points (`tools/venv_exec.py`, `tools/uv_project_env.py`,
   `checkout_provisioning ensure`, `molt_diff` frontend selection, `dev.py`) call
   `environment(key)` once; commands they launch run that environment's interpreter directly
   (`bin/python`, `Scripts\python.exe` on Windows) with `UV_NO_SYNC=1`.
3. **Bytecode**: one writable cache beside the environments (`<python-environments>/.bytecode`),
   selected by `PYTHONPYCACHEPREFIX` and, for a managed Python command, by
   `-X pycache_prefix=` (which `-I` cannot drop). CPython names cache files by absolute source
   path and cache tag, so checkouts and interpreters never collide. The old images ran
   source-only (`-B` and an empty prefix): every process recompiled everything it imported,
   0.62 s against 0.11 s for `import molt.dx, molt.frontend.generator` (2026-11-21).
4. **Not checkout state**: a shared environment has no checkout owner, lease or retirement;
   `python-environment` left `CheckoutStateKind`, and retiring a worktree never touches one.
5. **Delete**: development-image generations, selection records, provisioning stamps and leases
   for Python environments (`checkout_state.py`, `custody_layout.py:280-291`), runtime-closure
   and bytecode attestation, `tools/venv_exec.py` (both modes), `tools/uv_project_env.py`'s
   wrapper kind once its callers move, and the environment-registry rows that only describe them.
6. **The pinned controller** keeps its own environment because it must run an older main's
   driver; it uses `environment(key)` like everything else.

## Stages (each lands with its deletions; no stage leaves two live mechanisms)

1. **CI first** (independent, worst measured failure): `setup-project` activates the synced
   environment (landed in lane apparatus-exit, 3be76b90fb); then `setup-uv` and one `uv sync`.
2. **Identity probe and receipt key**: move the probe out of the layer; switch receipt identity to
   `uv.lock` digest plus interpreter version; switch release provenance to the probe.
3. **`environment(key)` and every consumer family.** 3a (landed 2026-11-21): development and
   target-version environments, every reader of their selection (`venv_exec`,
   `uv_project_env`, `checkout_provisioning`, `dx`, `dev.py`, `molt_dev`, proof-command argv,
   proof admission and custody), deleting selection records, runtime refresh, drift
   retirement, editable bindings, the source-only bytecode policy and checkout-state ownership
   of environments. `venv_exec.py` stays as the thin documented entry. Measured on macOS arm64:
   a new lock costs one sync for every worktree (38 s cross-volume, 12.9 s with the cache on
   the environment's volume) where each checkout built its own image (39 s each, 0.35 s warm);
   the warm entry is 0.14 s end to end. 3b (next): source-build group environments
   (`source_build_environment`, pact, extension producers) move onto `environment(key)`, with
   the build-tool closure captured at the producer's run instead of at image publication; then
   the image modules and their tests go.

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
