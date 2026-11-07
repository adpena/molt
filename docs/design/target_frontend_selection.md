# Target frontend selection for differential cells (HF-726 slice 1)

Status: implemented (slice 1). Owners: `tests/molt_diff.py` (harness) and
`molt.checkout_provisioning` (images). Ledger row: HF-726, family T.

## Problem

A differential cell for a newer target cannot run from the default harness.

- The frontend refuses a target newer than its own interpreter:
  `src/molt/target_python.py` `_parse_source_for_target` raises
  "Molt target Python 3.13 requires a Python 3.13+ frontend".
- `tests/molt_diff.py` runs the compiler under `sys.executable`
  (`_resolve_molt_cli_python`) unless `MOLT_DIFF_MOLT_PYTHON` names another
  executable by hand.
- The plan lanes `r6.target-version-parity.py313` and `.py314`
  (`tools/proof_plan.toml`) start the harness on CPython 3.12 (`uv run
  --python 3.12`) and ask for target 3.13 or 3.14 (`molt_diff.py
  --python-version`). Nothing selects a 3.13 or 3.14 compiler. They cannot
  pass.

Measured evidence (this host, 2026-11-04):

- Lane hf720-carrier: HF-647's six programs on 3.13 and HF-665's
  `public_native_types.py` on 3.14 fail to build with that refusal
  (`logs/agents/lane-hf720-carrier/artifacts/cells-20261104/hf647-py313-canonical.log`,
  `hf665-py314.log`). The oracle was
  `~/.local/share/uv/python/cpython-3.13-macos-aarch64-none/bin/python3.13`.
- `molt_diff` asks for a minor (`resolve_target_python_command`), so uv returns
  its minor link, which points at the newest installed patch:
  `cpython-3.12-…` → 3.12.16, `cpython-3.13-…` → 3.13.18,
  `cpython-3.14-…` → 3.14.10. The pinned target releases are
  `CPYTHON_RELEASES` (`src/molt/_python_release_generated.py`, derived from each
  pinned source's `Include/patchlevel.h`; see
  [python_version_authority.md](python_version_authority.md)). Each
  `uv python install` of a newer patch moved the oracle. That is the HF-760
  mechanism, in the oracle selection.
- Image costs with the existing authority (`ensure("uv-environment",
  python=..., groups=())`): 3.13.17 published in 24.8 s (26.3 s process wall,
  2.96 s user), 36 MB. The fast path on a current image, in a fresh process:
  0.41 to 0.45 s (three runs). With the default `dev` and `bench` groups the
  same release took 88.2 s and 156 MB.
- 3.14.9 cannot publish an image today. The runtime identity probe fails:
  "CPython's configured dynamic-extension root is outside its configured
  exec_prefix: …/cpython-3.14.9-…/lib/python3.14/lib-dynload not under
  <image root>". Cause: `python_runtime_identity._base_runtime_paths` (25854ece07,
  HF-766) reads the configured anchor as `sysconfig.get_config_var
  ("exec_prefix")`. On 3.12 and 3.13, `_init_posix` overwrites that variable
  with the build-time value. From 3.14 on (GH-126920), the build-time data no
  longer overwrites keys already set, so the variable is the live
  `sys.exec_prefix`; in a venv that is the venv. I read this in each pinned
  install's `sysconfig/__init__.py`.

## Alternatives considered

- **Start the harness on the target release.** The whole harness (guards,
  scheduling, tooling) would then run on 3.13 or 3.14, the queue admits only
  the 3.12.15 `python` toolchain, and each lane would need its own full harness
  image. HF-726 asks that molt_diff take the compiler from a selection, so a
  newer-target cell needs no hand-set interpreter.
- **Compile with the oracle interpreter.** The oracle is a bare CPython without
  the project's dependencies, so it cannot import `molt`. The frontend is the
  whole compiler, not a parse step.
- **Select per item.** The fast path costs 0.4 to 1 s per call. One selection
  per run carries the same fact for free.

## Prior art

- Nuitka compiles with the CPython that runs it; to target 3.13 you run Nuitka
  under 3.13. Molt's frontend has the same limit for newer grammar.
- tox gives each environment its own interpreter (`basepython`) and its own
  virtual environment, built on demand from declared dependencies. A missing
  interpreter is an error unless the user opts in to skipping it.
- cibuildwheel pins an exact CPython patch per minor in each of its releases,
  so a build is reproducible across hosts.
- rustup pins an exact toolchain in `rust-toolchain.toml`; a missing toolchain
  fails with its install command (or installs it, by policy).
- uv binds a project environment to one interpreter. Its minor links retarget on
  each patch install (uv 0.13.6 `install.rs` 720-729, `managed.rs` 890-912, read
  for HF-760). An exact request (`--python 3.13.17 --no-python-downloads`)
  names one patch directory.

Molt already has the image half: `checkout_provisioning.ensure(
"uv-environment", checkout, env, python=..., groups=...)` publishes an
immutable image bound to an exact patch (HF-760), replaces a drifted image, and
fails with ``run `uv python install X.Y.Z` `` when the patch is absent
(`source_build_environment._selected_python_recipe`). This design reuses it and
adds no second publisher, registry or cache.

## Decisions

1. **An explicit version selects an exact release.** For
   `molt_diff.py --python-version V`: a patch in `V` is exact
   (`tools/verified_subset.py` passes the policy's reference patch, which
   is the minor's `CPYTHON_RELEASES` entry); a bare minor selects the target's pinned release from
   `TargetPythonVersion` (`CPYTHON_RELEASES`, which
   `tests/test_target_python_oracle.py` already checks against
   `config/cpython_regrtest_sources.toml`). This is the rule that
   `tests/python_environment_test_support.py` already uses for synthetic
   identities. New `python_interpreter.target_release_request(target, value)`
   returns that `PythonInterpreterRequest`.
2. **The oracle uses that exact release.** `_resolve_diff_python_command`
   calls `resolve_python_interpreter(request, override=MOLT_DIFF_PYTHON, ...)`
   in place of `resolve_target_python_command`. The existing version check also
   applies to the override, so `MOLT_DIFF_PYTHON` must report that release.
   The product CLI's `resolve_target_python` (`molt compare`,
   `tools/molt_dev.py python-oracle`) keeps
   its minor request: what an installed Molt demands of its host is the later
   HF-726 slice.
3. **The compiler frontend runs the oracle's CPython release.** The oracle
   fixes the target minor (`resolve_target_python_for_oracle`), and the
   frontend parses with the same release. Let `R` be the exact release of
   decision 1 for the run's `--python-version`, or the target's pinned release
   when no version is given. The oracle's version comes from the cached
   identity probe (`_run_metadata_probe`, `PythonInterpreter.version`). Then:
   - the harness interpreter serves when it is CPython and its
     `platform.python_version()` equals the oracle's version (the R6 3.12 lane,
     `tools/dev.py test` without a version, `verified_subset`, direct
     `diff_test` callers that pass the harness): one string compare;
   - otherwise, when the oracle's version equals `R`, the harness selects the
     managed image of `R`;
   - otherwise the run refuses: an API caller passed an oracle (`run_diff` or
     `diff_test` `python_exe`, which `resolve_python_selector` resolves by
     minor) whose release is neither the harness nor `R`. The message names
     both releases and says to pass `--python-version`. So images exist only
     for pinned releases and explicit patches, never for whatever patch a minor
     link names today. The `__main__` path always passes the harness as
     `python_exe`.
   Why the same patch, not the same minor: the frontend's behavior is that
   release's parser, tokenizer and `ast`, and patch releases change them. Each
   run records which release parsed it (decision 5).
4. **One image selection for a release.** New
   `checkout_provisioning.ensure_python_release(checkout, environ, request)`:
   - refuses a request without a patch;
   - drops `UV_PROJECT_ENVIRONMENT` (it names the harness image, and
     `_current_development_environment` reads it before any interpreter
     resolution) and `MOLT_UV_PROJECT_PURPOSE` (so one release has one image
     per checkout);
   - calls `ensure("uv-environment", checkout, env, python=request.selector,
     groups=())`.
   The frontend needs only the project's locked runtime dependencies (click,
   msgspec, packaging, rich, ty). The group set is part of both the selection
   key (`_development_intent`, hashed by `_development_selection_path`) and the
   image address (`environment_id` from custody `dependency_groups`), so this
   image coexists with a `dev`/`bench` image of the same release; neither
   replaces the other. Images live under the checkout's own component, and the
   project is an editable install of that checkout
   (`molt-development-selection.json` `editable_binding.source_root`). The item
   environment also sets `PYTHONPATH=src` and the repository as working
   directory, so the compiler imports this checkout's `src/molt`.
   Concurrency and atomicity come from the existing publisher: a per-input file
   lock (`_ProvisioningLock`), a provisional record, and an attestation written
   last. After an interrupted publication, the publisher deletes the address
   only when its provisional record matches exactly; a missing or foreign
   record, or a manifest that fails admission, is a refusal that needs manual
   cleanup (`source_build_environment.py` 1154-1173). Those refusals reach the
   user through decision 8 with the publisher's own message.
   Other inherited variables (`UV_PYTHON_INSTALL_DIR`, `UV_PYTHON_PREFERENCE`,
   `UV_CACHE_DIR`) are host configuration and stay; the exact version check
   guards the result, and the publisher already drops `VIRTUAL_ENV`.
5. **Select once per run, carry the value, hold the image.** `run_diff`
   selects the frontend in its preflight, after the oracle and the target, and
   passes a typed `compat_backends.CompilerFrontend(python, environment_root)`
   to the serial path, the process pool and the warm-cache pass. For an image
   it holds a shared checkout-state lease on the image root until the run ends:
   `checkout_state_use(checkout_state_environment_roots(checkout,
   {"UV_PROJECT_ENVIRONMENT": image}, include_current_process=False))`, built
   from the image, not from the harness environment. After it takes the lease,
   it checks that the image interpreter still exists and refuses otherwise,
   which closes the gap after `ensure` releases its lock. Each compiler child
   also holds its own lease through its launcher (`tools/memory_guard.py`,
   `tools/batch_compile_client.py`, by `checkout_state_command_roots`). A
   direct `diff_test` call with no frontend selects one per call: free in the
   harness case, 0.4 to 1 s otherwise. Selection runs inside the preflight. It
   is not interruptible; if the preflight deadline passes during it, the run
   reports every item as not run, as it does today. One run has one target.
   The summary's `config` gains `compiler_python`, `compiler_python_version`
   and `compiler_environment_root`, and each item record gains
   `compiler_python`, so every cell names the release that parsed it.
6. **Only compiler children run inside the image.** `BackendExecutionContext`
   gains `compiler_environment_root: Path | None` (`None`: the harness serves).
   `compat_backends.compiler_environment(context, env)` returns `env` for the
   harness case and `DxProject.projected_uv_run_env(env, root=image)` for an
   image: `UV_PROJECT_ENVIRONMENT`, `VIRTUAL_ENV`, `PYTHONPYCACHEPREFIX`,
   `PYTHONDONTWRITEBYTECODE=1`, `UV_NO_SYNC=1`, a default
   `PYTHONUNBUFFERED`, and no `PYTHONHOME` or conda variables. Every compiler
   launch uses it: the native build and the batch compile server in
   `tests/molt_diff.py`, and the WASM, LLVM, JIT and Luau `molt.cli build`
   commands in `tools/compat/backends.py`. The JIT runner, which compiles
   inside its run phase, is a Rust guest-side binary and stays unprojected.
   `BackendExecutionContext.molt_cli_python` stays as the one executable; the
   image root travels beside it. Guest runs keep the unprojected item
   environment, which matters: the runtime reads `PYTHONDONTWRITEBYTECODE` into
   `sys.flags` (`runtime/molt-runtime/src/object/ops_sys.rs:2148`), and the
   oracle environment is not projected either. `UV_NO_SYNC=1` keeps a nested
   `uv run` from syncing groups into the image. I use `projected_uv_run_env`,
   not `normalized_uv_run_env`, because the second validates the image against
   the default `dev`/`bench` groups and would refuse a `groups=()` image.
7. **Delete `MOLT_DIFF_MOLT_PYTHON`.** The selection replaces the hand-set
   path; no plan lane or tool sets it. `_resolve_molt_cli_python` goes with it.
   The two context-free build paths (`run_molt` without an execution context,
   used only by unit tests) name no target and use `sys.executable`. Accepted
   loss: no way remains to run the frontend under a debug, sanitizer,
   free-threaded or pre-release CPython build; no lane uses one. The variable
   is removed without a refusal shim (early stage, no users outside tests).
8. **Fail at once, loudly.** Both `[LOCK]` handlers in molt_diff's `__main__`
   block (around `run_diff`) gain an earlier handler for
   `PythonInterpreterError` and `ProvisioningError` that prints
   `[PYTHON] <message>` and exits 2. Today these errors already exit 2, but
   labelled `[LOCK]`. The message carries the cause (a missing release, a
   publisher refusal, a lock timeout). The plan lanes treat any nonzero exit as
   a failure. The install hint moves to one layer: `resolve_python_interpreter`
   appends ``; run `uv python install <selector>` `` to its "no verified
   CPython" error, and `_selected_python_recipe` stops appending its own copy.
   So a missing release reads, for the oracle, `no verified CPython 3.13.17
   command available (…); run `uv python install 3.13.17`` and, for the
   frontend, `uv-environment: cannot bind requested CPython 3.13.17: no
   verified … ; run `uv python install 3.13.17``, each once.
9. **Repair the 3.14 runtime identity.** `_base_runtime_paths` reads the
   configured pair (`DESTSHARED`, `exec_prefix`) from CPython's build-time data,
   not from the runtime view. From 3.14 on it calls
   `sysconfig._get_sysconfigdata()` (present in 3.14.9 and 3.15.1); on 3.12
   and 3.13 the runtime view `sysconfig.get_config_vars()` already holds the
   build-time values. A computed `importlib.import_module` of the data
   module's name would read the same values, but the WASM linker's static
   tooling import closure refuses a non-literal import, so every WASM build
   failed with it (first HF-726 WASM cell run). A test checks both sources
   against the data module in a venv of each supported target's pinned
   release. Correction to the evidence above: in 3.14.9 `_init_config_vars`
   runs `_init_posix` first and then assigns the live `exec_prefix`; on 3.12
   and 3.13 the later `_init_posix` overwrote it with
   the build-time value. The rebase onto the live `sys.base_exec_prefix` and
   the fail-closed check stay. On 3.12 and 3.13 the build-time pair equals what
   `get_config_var` returned, so no recorded identity changes and no existing
   image is retired. The branch is POSIX only (Windows uses `base-dlls`). The
   cross-build variable `_PYTHON_SYSCONFIGDATA_PATH` is outside this probe's
   contract: it captures host interpreters only. PEP 739 `build-details.json`
   (3.14+) is the public form; 3.12 and 3.13 lack it, so it can replace this
   read once 3.14 is the floor. This lands as its own commit.

## Invariants

- I1. For every item, the compiler interpreter reports the oracle's CPython
  version.
- I2. With `--python-version V`, the oracle and the frontend report the exact
  release of decision 1.
- I3. An exact request is checked against the reported version at selection.
  An image binds the resolved patch directory, never a minor link
  (`_provision_source_build_environment` resolves the base executable and
  passes the exact version to `uv sync`).
- I4. One run makes at most one image selection, before the first item, and
  holds the image until the run ends.
- I5. In the image case, a compiler child's `UV_PROJECT_ENVIRONMENT` and
  `PYTHONPYCACHEPREFIX` name the image whose interpreter it runs. In the
  harness case, the compiler child inherits the harness environment, as today.
- I6. Guest runs never see the image projection.
- I7. A missing release stops the run before any item, names the release and
  `uv python install <release>`, and exits 2.

## Changes (C create, M modify, D delete)

- M `src/molt/python_interpreter.py`: `target_release_request`; the
  "no verified CPython" error gains the uv fix.
- M `src/molt/source_build_environment.py`: `_selected_python_recipe` drops its
  copy of the hint.
- M `src/molt/checkout_provisioning.py`: `ensure_python_release`.
- M `src/molt/python_runtime_identity.py`: decision 9.
- M `tests/test_checkout_provisioning.py`, `tests/test_python_runtime_identity.py`,
  `tests/test_molt_diff_expected_failures.py`: new tests.
- M `tools/compat/backends.py`: `CompilerFrontend`,
  `BackendExecutionContext.compiler_environment_root`,
  `compiler_environment`; four build sites.
- M `tests/molt_diff.py`: decisions 2, 3, 5, 6, 7 and 8.
- M `src/molt/environment_registry.toml`, regenerated
  `src/molt/_environment_registry.py` and
  `docs/environment-variables.generated.md`: D `MOLT_DIFF_MOLT_PYTHON`; the
  `MOLT_DIFF_METADATA_PROBE_TIMEOUT_SEC` summary describes the CLI interpreter
  selection this slice replaces, so it changes; `MOLT_DIFF_PYTHON` says the
  override must report the exact release.
- M tests that stub `_resolve_molt_cli_python`, set `MOLT_DIFF_MOLT_PYTHON` or
  stub `resolve_target_python_command` for molt_diff:
  `tests/test_molt_diff_memory_guard.py`,
  `tests/test_molt_diff_expected_failures.py`,
  `tests/test_compat_multibackend_oracle.py`,
  `tests/test_python_interpreter.py`; positional
  `_backend_execution_context` callers in `tests/test_molt_diff_scheduling.py`;
  `tests/test_python_runtime_identity.py` for decision 9.
- M `tools/proof_plan.toml`: the three R6 lane descriptions state the
  selection; their argv does not change.
- M `docs/getting-started.md`: the harness selects the frontend; a host needs
  `uv python install 3.12.15 3.13.17 3.14.9` for every cell.
- C `docs/design/target_frontend_selection.md` (this file).

Look-alike sites left unchanged:

- `tools/dev.py test` runs the suite under each supported minor. That is a
  host-support matrix for tooling, and its minor request is its contract. A run
  without `--python-version` there keeps the harness as frontend (decision 3).
- The product CLI oracle: decision 2.
- `[[toolchain_policy]] target-pythons` admits any patch of each minor for the
  Unicode generator. It is not a frontend.

A sibling with the same defect, not migrated in slice 1:
`tests/compliance/py313` and `py314` `test_spec.py` compile with
`python_interpreter.python_command_for_min_version`. From a 3.12 harness that
returns a bare uv minor link (measured 2026-11-04: 3.13.18 and 3.14.10, neither
with `msgspec`), so 11 of their cases fail to import the compiler. No proof
cell runs that tree (HF-99), and the test dependency graph excludes it from
landing selection. Moving it onto `ensure_python_release` is a later HF-726
slice.

Other `--python-version` callers: the three R6 lanes,
`tools/verified_subset.py` (already exact), and two developer commands that
forward the flag to `tests/molt_diff.py`: `molt diff`
(`src/molt/cli/script_commands.py`) and `molt test` with the `diff` suite
(`src/molt/cli/quality_commands.py`). Both run the harness from a source
checkout, so they take the harness contract now: a bare minor needs the pinned
patch installed. `molt compare` is not a harness and keeps its minor request
(decision 2).

## Test plan

Unit tests (independent oracles):

- `ensure_python_release` on a fixture Git checkout (`uv.lock`,
  `pyproject.toml`, `.python-version`) with a real `uv` and a release that
  cannot exist (3.12.999): the error names 3.12.999 and
  `uv python install 3.12.999`. The input also carries a bogus
  `UV_PROJECT_ENVIRONMENT`; had the selector survived,
  `_current_development_environment` would fail first on that path with a
  selection error, so the bind error shows the selector was dropped.
- `_base_runtime_paths` in a real venv of each installed pinned release
  (skipped when the release is absent): `base-lib-dynload` equals the
  `lib-dynload` entry that CPython itself put on that venv's `sys.path`.
- `_compiler_frontend`: an oracle with the harness's version returns
  `sys.executable` and never selects an image; another version selects once,
  with that exact release, and returns `environment_python_path(image)`.
- `_compiler_frontend` refuses an oracle whose release is neither the harness
  nor `R`, naming both releases.
- `compiler_environment` and the build sites: with an image frontend, the
  native build, batch server and the WASM, LLVM, JIT and Luau build commands
  get the image's `UV_PROJECT_ENVIRONMENT` and `PYTHONPYCACHEPREFIX`; their
  guest run environments do not.
- `run_diff`: one selection for a multi-item run on each path (serial, pool,
  warm cache); a selection failure exits 2 with `[PYTHON]` before any item; the
  summary names the compiler interpreter and its version.
- `_resolve_diff_python_command("3.13")` requests 3.13.17;
  `("3.13.16")` requests 3.13.16.
- The decision 9 test is skipped where a pinned release is absent; the cells
  below run it for real on 3.14.9.

Cells (real runs), from the 3.12.15 harness:

- R6 3.12, 3.13 and 3.14 lanes, by their plan argv. Record the oracle and
  compiler versions each run reports.
- HF-647's six programs on 3.13, native and WASM: `public_native_types`,
  `public_object_identity`, `copyreg_helpers`,
  `object_reduce_implicit_copyreg`, `pickle_basic`, `time_timegm`.
- HF-665's `public_native_types.py` on 3.14, native.
- HF-661's `copy_version_surface.py` and `code_replace_execution_policy.py` on
  3.13, native and WASM.
- Witnesses: the image's interpreter, under the projected environment from
  the checkout, imports `molt` from this checkout's `src`. A missing frontend
  release (oracle named by `MOLT_DIFF_PYTHON`, a fresh artifact root, and uv
  unable to find the release) and a missing oracle release each exit 2 with
  the uv command.

Gate: `python -B tools/molt_dev.py land --repo <slot> --precheck-only`.

## Costs

- Harness case: one string compare per run.
- Image case, first run in a checkout: one image publication per release and
  per generation of the declared inputs (`pyproject.toml`, `uv.lock`,
  `.python-version`) and artifact root. Measured with a warm uv cache:
  24.8 s and 36 MB for 3.13.17; 28.7 s and 36 MB for 3.14.9 after
  decision 9. Each slot publishes its own; an edit to a
  declared input publishes a new generation. Superseded generations age out
  through the existing checkout-state retention by last use; this slice adds
  no retention rule.
- Image case, later runs: one fast path per run (measured 0.41 to 0.45 s in a
  fresh process).
- Per item: no added selection. The image's compiler child is source-only
  (`PYTHONDONTWRITEBYTECODE=1` from the projection). The harness child behaves
  as today; the lane environment already sets that variable, but an ordinary
  shell may not. The cells record build times.

## Out of scope

- The installed product's host-Python requirement (`packaging/INSTALL.md`): a
  later HF-726 slice.
- Queued R6 runs. Under the queue, the preflight would publish or select the
  image inside queue custody; whether that custody admits the publication is
  unverified. Direct runs prove the harness contract; the parent decides
  whether HF-726's R6 evidence needs a queued run. Publication stays on demand
  (as `tools/dev.py` and `tools/uv_project_env.py` use `ensure` today); the R6
  lanes set no `--deadline-s`, and their timeout is 900 s.
- The build cache key does not include the frontend interpreter's release
  (`src/molt/cli/cache_fingerprints.py`). A cached artifact from one frontend
  release can serve another release of the same target minor. This predates
  the slice (a 3.14 harness already compiles 3.12 targets into the same keys);
  it is reported as a sibling, not fixed here.
- Real LLVM, JIT and Luau cells on 3.13: their build commands are wired and
  unit-tested, not run.

## Review record

Round 1 (egm-design, egm-codebase-details), 2026-11-04: no design blocker from
the design reviewer; one blocker from the codebase reviewer (the projection
reached guest runs, and the runtime reads `PYTHONDONTWRITEBYTECODE`). Fixed in
decision 6. Other findings answered: alternatives; the exact-release rule and
override check (decisions 1-3); group set in the image key, checkout source,
publication locking (decision 4); run-wide lease, deadline, direct callers
(decision 5); `UV_NO_SYNC` (decision 6); error label (decision 8); I3 and I5
narrowed; measured costs replace estimates; test-plan gaps (selector drop,
witness paths, R6 3.12 cell, per-path selection count). New in round 2: the
3.14 runtime identity defect (decision 9), found while measuring.

Round 2 (egm-design, egm-codebase-details), 2026-11-04: one blocker from the
codebase reviewer (an API caller's minor-selected oracle would publish images
for arbitrary patches). Fixed in decision 3: images exist only for `R`, and
other oracles are refused. Majors answered: per-checkout cost and retention
(Costs); the frontend identity is now recorded (decision 5); the install hint
is emitted once (decision 8); developer commands that forward
`--python-version` are listed; all build sites are projected and tested
(decision 6); publisher recovery stated as the code behaves (decision 4); the
lease is built from the image root and re-checked (decision 5); decision 9's
evidence corrected, with stability, platform and PEP 739 notes. Answered
without change: publication stays on demand, not a separate step; the oracle
path is checked at selection (I3); the build-cache key gap and the LLVM, JIT
and Luau cells are out of scope. Two rounds are spent; the remaining scope is
as written.
