<!-- Foundation blueprint 21d. Architect: cli-architect (Plan agent), 2026-06-23. Arc:
decomposition of src/molt/cli.py (the repo's largest god-file, 41,640 lines) into a cli/
package. Refines doc 21 §2.3 against the measured tree. Move-only / zero-behavior-change
(byte-identical CLI help + exit codes + import surface). Design only. -->

# 21d — Decompose `src/molt/cli.py` into a `cli/` package (Move, file-split)

## 0. Safe to decompose now
- 41,640 lines (doc 21 §1.1 recorded 39,238; grew +2,402) — the largest Python god-file.
- Working tree clean for `cli.py` + all 7 `cli_*.py` siblings; no active editor (recent commits
  are older CLI fixes + Windows hardening). The only in-flight tree work is Rust (disjoint).
- ZERO top-level executable statements (lines 1–385 are imports + module-level constants only);
  importing future submodules has no side effects to reorder.

## 1. Real structure (doc 21 §2.3 was only half-right)
**1a. Extraction already began** — 7 sibling modules (~4,239 lines) already pulled out and
re-imported at `cli.py` lines 118–201: `cli_wasm_split.py`(1561), `cli_deps.py`(1238),
`cli_native_toolchain.py`(469), `cli_completion.py`(367), `cli_arg_helpers.py`(322),
`cli_maintenance.py`(225), `cli_debug_helpers.py`(57). The re-export-into-`molt.cli` pattern is
proven + test-green (e.g. `tests/test_generate_worker.py` imports `_generate_split_worker_js`
which physically lives in `cli_wasm_split.py`). Extend this pattern; don't reinvent it.

**1b. The residual is ~70% ONE concern (the build/compile pipeline), NOT N subcommands.** Of the
997 top-level defs, the `_backend_*`(60)/`_build_*`(43)/`_module_*`(31)/`_runtime_*`(30)/
`_prepare_*`(22)/`_resolve_*`(66) clusters are the machinery behind `molt build` (run/test/diff
funnel through it). The subcommand handlers are thin (`build()` ~515 lines, `test()` ~43). Largest
fn is `main()` at **2,695 lines** (argparse construction + the `if args.command ==` dispatch);
then a long tail of 200–600-line pipeline helpers. So this is a **file-split** target (not a
single-giant-function extraction like function_compiler), split by **pipeline stage** primarily +
**command family** secondarily.

**1c. Entry point invariant (preserve byte-for-byte):**
- `pyproject.toml`: `[project.scripts] molt = "molt.cli:main"`.
- `src/molt/__main__.py`: `from molt.cli import main`.
- **20 test files run `python -m molt.cli`** → a *package* runs `cli/__main__.py` (NOT
  `__init__.py`), so the package MUST contain `cli/__main__.py`. THIS dictates the shape.
- `main()` (L38833): `ArgumentParser(prog="molt")`, `add_subparsers(dest="command")` (L38856),
  all subparsers inline, `parse_args()` (L40576), flat `if args.command == ...` dispatch.
- Subcommands (29 + 2 nested groups `extension`{build,audit,scan}, `debug`): build, extension,
  internal-batch-build-server, debug, check, run, repl, compare, parity-run, test, diff, bench,
  profile, lint, setup, doctor, update, validate, package, publish, verify, deps, vendor, install,
  clean, config, completion, deploy, harness. (No `pgo`/`daemon` subcommand — PGO is build flags;
  daemon is internal `internal-batch-build-server`.)

**1d. Module-level state (lines 1–385, non-executable)** — must travel WITH consumers: the 5
`_BACKEND_*_ENV_KNOBS`/`_NATIVE_CODEGEN_ENV_KNOBS`/`_WASM_CODEGEN_ENV_KNOBS` tuples → daemon
module; `_*_CACHE_SCHEMA_VERSION` ints → cache module; `_DEAD_FUNCTION_ELIM_REFERENCE_KINDS` +
`_reachable_function_names_for_stdlib_cache` + `_SHARED_STDLIB_CACHE_SCHEMA_VERSION` → stdlib-cache
module (keep co-located; docs 09/13 mandate the Python BFS mirror the Rust DFE).

**1e. External name-pins (must not break):**
- 15 test-pinned `from molt.cli import _X` names (verified list): `_MICRO_BASE_RUNTIME_FEATURES`,
  `_VALID_AUDIT_SINKS`, `_backend_codegen_env_digest`, `_build_cache_variant`,
  `_build_isolate_import_ops`, `_download_artifact`, `_effective_split_worker_table_base`,
  `_generate_snapshot_header`, `_generate_split_worker_js`, `_generate_split_wrangler_jsonc`,
  `_is_private_ip`, `_isolate_import_module_order`, `_parse_audit_log_flag`, `_parse_io_mode_flag`,
  `_parse_type_gate_flag` -> all re-exported from the package `__init__`.
- WASM binary/artifact facts are no longer CLI pins. `_wasm_export_function_signatures`,
  `_wasm_import_function_result_kinds`, `_wasm_import_function_signatures`, and sibling binary
  readers live in `molt.wasm_artifact`; internal consumers and tests must import them there rather
  than rebuilding a `molt.cli` or `molt.cli.wasm` shadow API.
- 3 monkeypatches of `molt.cli.subprocess.run` → `__init__` keeps `import subprocess`/`shutil`/`os`
  so `molt.cli.<name>` targets resolve; re-verify `test_generate_worker.py` after the wasm phase.
- `cli.py:LINE` references in docs 06/08/09/13/17/18/20/22/24 are docs, not code — accepted stale
  collateral of a move-only commit; do NOT rewrite them in the move.

## 2. Target layout — `cli.py` → `cli/` package (with `__init__.py` AND `__main__.py`)
```
src/molt/cli/
  __init__.py     # import anchor: main() (parser construction + dispatch chain, kept WHOLE) +
                  #   re-export of the 18 pinned names + handlers; keep import subprocess/shutil/os.
  __main__.py     # `from molt.cli import main; raise SystemExit(main())` — preserves -m molt.cli.
  _shared.py      # strict leaf (stdlib + molt.compat only, NEVER molt.cli.*): _emit_json, _fail,
                  #   _json_payload, _run_command, _base_env, _with_memory_guard_env, _coerce_*,
                  #   type aliases. The cycle-breaker (like frontend/_types.py).
  pipeline.py     # THE BIG ONE (~18-22K): _collect_imports, _run_backend_pipeline,
                  #   _execute_backend_compile, _prepare_*, _emit_build_diagnostics,
                  #   module-graph discovery, stdlib-cache + DFE BFS. (Phase 3b: sub-split into
                  #   pipeline/{imports,backend,link,runtime,stdlib_cache,diagnostics}.py.)
  daemon.py       # ~50 _backend_daemon_* + _start_backend_daemon + _compile_with_backend_daemon
                  #   + the _BACKEND_*_ENV_KNOBS tuples.
  build.py run.py test.py validate.py package.py deploy.py debug.py  # thin handler families
  wasm.py deps.py native_toolchain.py arg_helpers.py maintenance.py completion.py debug_helpers.py
                  #   = the 7 existing cli_*.py, git mv'd into the package
```
Only `pipeline/` earns a subdir (the one cluster big enough); the rest is one package level.

## 3. Mechanics (move-only, no behavior change, no compat shims)
1. Move fns verbatim into the target submodule; move the constants they consume WITH them (§1d).
2. `_shared.py` is a strict leaf (never imports molt.cli.*); every submodule imports from it.
3. Submodules import helpers by explicit name from `_shared` + siblings (never `from molt.cli import *`).
4. **Keep the argparse parser construction WHOLE in `__init__`** — do NOT adopt a
   `register(subparsers)` pattern: argparse `--help` is order-sensitive and a per-module register
   would reorder add_parser/add_argument calls. Handlers move; the parser does not. This makes the
   gate pass by construction.
5. **Delete the inline def as it moves** (no stub, no back-compat re-export). The ONLY re-exports
   in `__init__` are the 18 pinned names + the handlers `__init__`'s dispatch already imports.
6. DAG: `_shared`(leaf) ← {pipeline, daemon, wasm, …} ← {build, run, test, …handlers} ← `__init__`
   (nothing imports `__init__`). Handler↔handler calls go through the pipeline fn, not the sibling
   handler (avoids cycles).

## 4. Entry-point invariant + gate (run BEFORE first move to capture oracle, AFTER every commit)
Capture into a scratch dir OUTSIDE the repo: `python -m molt --help`, `python -m molt.cli --help`,
no-args `python -m molt` (prints help, exit 0), `python -m molt <each of 29 subcmds> --help` +
exit codes, `python -m molt extension {build,audit,scan} --help`, each `debug` subgroup --help, and
the import-surface probe (`import molt.cli as c; assert c.subprocess is subprocess;` + getattr all
18 pinned names → `SURFACE_OK`). After each commit, re-run identically and `diff -r oracle now`
**MUST be empty**. Plus: `pytest tests/cli/ tests/test_generate_worker.py
tests/test_manifest_cli_integration.py tests/test_cli_build_profile_policy.py` green, and all three
entries succeed: `python -m molt --version`, `python -m molt.cli --version`, `molt --version`.
Any diff or red test ⇒ not move-only ⇒ reject.

## 5. Ordering (each its own move-only commit, each leaving green gates)
- **Phase 0** (riskiest surface, do alone, gate hard): `cli.py` → `cli/__init__.py`; add
  `cli/__main__.py` (3 lines); `git mv` the 7 `cli_*.py` → `cli/*.py`; update import paths in
  `__init__` (`from molt.cli_deps import` → `from molt.cli.deps import`). FIRST grep
  `from molt.cli_` across tests/tools — if non-empty, update those imports in the SAME commit (the
  only acceptable test-touch; the module genuinely moved). Verify `python -m molt.cli --version`.
- **Phase 1**: extract `_shared.py` (leaf). 
- **Phase 2**: extract `daemon.py` (most self-contained large cluster; daemon regressions are a
  recurring theme → highest contention relief).
- **Phase 3**: extract `pipeline.py` (the ~20K engine) as ONE move (empties most of `__init__`).
  **Phase 3b–3g**: sub-split `pipeline/` by stage (parallelizable across agents after 3 lands).
- **Phase 4**: extract handler families one per commit: build, run, test (verification family:
  test/diff/compare/parity_run/bench/profile/lint), validate (doctor/setup), package
  (publish/verify/update), deploy, debug. Dispatch stays in `__init__` calling imported handlers.

## 6. Top risks
- **`-m molt.cli` breaks on package conversion** (20 tests) → MUST add `cli/__main__.py`; #1 risk.
- **`molt.cli.subprocess` monkeypatch no-ops** (test_generate_worker, 3 patches) → keep
  `import subprocess`/`shutil` in `__init__`; re-run that test after the wasm phase; a patch-target
  change is a SEPARATE flagged commit, never folded into the move.
- **18 pinned names vanish** → enumerate (done) + re-export from `__init__`; the surface probe asserts all 18.
- **argparse `--help` reorders** → keep the parser whole in `__init__`; `diff -r` on all 29 `--help`.
- **`pipeline.py` stays a 20K god-file** → Phase 3b–3g sub-split; don't stop at Phase 3.
- **DFE-BFS/stdlib-cache constants split apart** → move the 3 together into `pipeline/stdlib_cache.py`.
- **daemon env-knob tuples split from request builder** → move the 5 `_BACKEND_*_ENV_KNOBS` with daemon.py.

## Adaptation note
cli.py is NOT one giant function (unlike function_compiler → 21a). Correct shape = file-split
(package), per doc 21 §2.3 — but §2.3's "split by subcommand" is half-right: handlers are thin;
~70% is the single build/compile pipeline. Split primarily by pipeline stage, secondarily by
command family, folding the 7 existing siblings in. Edit-locality + parallel-ownership win only
(Python, no compile step).
