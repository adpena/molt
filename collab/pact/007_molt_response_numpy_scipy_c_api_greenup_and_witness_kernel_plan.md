# 007 - Molt response: NumPy/SciPy C-API greenup and witness kernel plan

Status: Molt-side enabling layer delivered on `pact-collab`, 2026-06-27.
Kernel A WASM parity remains the next acceptance milestone.

This is the Molt-team reply to `006_precise_contract_full_witness_pipeline.md`.
Pact has now supplied the exact kernel bundle, fixture/reference generators, and
oracle. Molt owns the compiler/runtime/browser side from here.

## Ownership boundary

Pact owns:

- the exact Python kernels in `pact_witness_kernel/`;
- deterministic fixture and reference output generators;
- `check_parity.py` as the acceptance oracle;
- `verify_against_tac.py` as the Kernel B bit-identity proof against Pact/TAC;
- regenerated `reference_outputs.npz` plus `check_parity.py` as the Kernel A
  authority, because Kernel A is a faithful field-solve extract with intentional
  deterministic tie/eigvec canonicalizations rather than a byte-for-byte copy of
  the visualization code.

Molt owns:

- compiling the kernels through the verified Molt subset;
- the no-host Python/NumPy/SciPy package-source and native-extension custody
  path;
- WASM-CPU determinism authority and any WebGPU/WGSL speed path;
- prebuilt `molt-embed` example artifacts and the small JS loader story;
- proof artifacts that make the handoff reproducible without source builds on
  Pact's shared GPU machine.

## Delivered in this greenup

The missing-symbol layer for pinned upstream NumPy and SciPy source scans is now
green under a stricter scanner.

- Molt's CPython compatibility header exposes the CPython C-API surface NumPy
  reaches during source compilation: buffer flags, GC/object constructors,
  type/object helpers, capsule rename support, Unicode helpers, tuple slicing,
  thread macros, fatal/error helpers, datetime helpers, and Windows
  `-Werror`-clean errno text.
- Molt's NumPy source-compile headers expose the SciPy-facing array/ufunc
  surface: dtype aliases, array shape/dtype/contiguity macros, multi-iterator
  and neighborhood iterator helpers, aligned allocators, array construction
  from existing data, cast/copy/return/scalar helpers, dtype promotion/cast
  checks, `PyArray_UpdateFlags`, NumPy 2 import helpers, and `PyUFunc_getfperr`.
- `molt extension scan` now has package-source boundaries: repeatable
  `--exclude-dir`, deterministic non-UTF8 reads, comment/string/docstring
  filtering, macro-body stripping for definition extraction, guarded-body
  scanning, global project definitions, and per-file local symbols. This
  prevents the old false-green class where package-local macros or local
  variables hid real C-API requirements.
- `bench/friends/manifest.toml` now has an enabled pinned SciPy C-API probe
  beside the pinned NumPy probe, and scan-only suites are classified as
  `c_api_probe` rather than `runs_unmodified`.

## Verified proof

Strict NumPy C-API probe:

```bash
uv run --project . --python 3.12 python tools/bench_friends.py --suite numpy_off_the_shelf --runner c_api_scan --output-root tmp\pact_numpy_capi_scan_strict3 --timeout-sec 300 --checkout --fail-fast
```

Result: 447 scanned source files, 1,258 required symbols, 1,258 supported,
zero missing, zero fail-fast.

Strict SciPy C-API probe:

```bash
uv run --project . --python 3.12 python tools/bench_friends.py --suite scipy_off_the_shelf --runner c_api_scan --output-root tmp\pact_scipy_capi_scan_strict3 --timeout-sec 600 --checkout --fail-fast
```

Result: 592 scanned source files, 321 required symbols, 321 supported, zero
missing, zero fail-fast.

Stress repeat:

```bash
uv run --project . --python 3.12 python tools/bench_friends.py --suite numpy_off_the_shelf --suite scipy_off_the_shelf --runner c_api_scan --output-root tmp\pact_numpy_scipy_capi_scan_c_api_probe_repeat2 --timeout-sec 600 --checkout --repeat 2 --fail-fast
```

Result: NumPy ok twice (`10.094s`, `3.766s`); SciPy ok twice (`9.703s`,
`3.641s`). The generated summary reports both suites as `c_api_probe`.

Profiled direct scanner passes:

- NumPy: `tmp\pact_numpy_extension_scan_c_api_probe.prof`, 447 files, 1,258
  required symbols, zero missing/fail-fast, 7.564s under `cProfile`.
- SciPy: `tmp\pact_scipy_extension_scan_c_api_probe.prof`, 592 files, 321
  required symbols, zero missing/fail-fast, 6.019s under `cProfile`.
- Top cumulative costs are the scanner's structural passes:
  `_extract_project_defined_py_c_symbols`,
  `_strip_c_like_comments_and_literals`, `_extract_file_local_py_c_symbols`,
  and `_strip_cython_hash_comments`. Those are the right future optimization
  targets for scan-DX speed without weakening the false-green protections.

Guardrails:

```bash
uv run --project . --python 3.12 python -m pytest tests/cli/test_cli_extension_commands.py -q -k "extension_scan or numpy_header_arrayobject_smoke or python_header_type_module_smoke or datetime_header_smoke"
uv run --project . --python 3.12 python -m ruff check src/molt/cli/extension_scan_surface.py src/molt/cli/extension_scan.py src/molt/cli/entrypoint_parser.py src/molt/cli/entrypoint_dispatch.py tests/cli/test_cli_extension_commands.py
```

Result: 11 pytest checks passed; Ruff passed.

Pact kernel oracle sanity:

```bash
cd collab/pact/pact_witness_kernel
uv run --project ..\..\.. --python 3.12 --with numpy==1.26.4 python make_fixture.py
uv run --project ..\..\.. --python 3.12 --with numpy==1.26.4 --with scipy==1.17.1 python field_solve.py lstar_sample.npz
uv run --project ..\..\.. --python 3.12 --with numpy==1.26.4 python check_parity.py reference_outputs.npz
uv run --project ..\..\.. --python 3.12 --with numpy==1.26.4 python make_weights_fixture.py
uv run --project ..\..\.. --python 3.12 --with numpy==1.26.4 python witness_forward.py witness_weights_sample.npz
```

Result: regenerated Kernel A reference passed the updated, order-robust
`check_parity.py`; regenerated Kernel B output matched
`witness_forward_reference.npz["lstar"]` exactly (`mismatch_px=0`).

## Boundary of the claim

This closes the NumPy/SciPy C-API scan and missing-symbol closure layer. It does
not claim package build, link, import, or runtime execution of unchanged NumPy
or SciPy through Molt-WASM yet. It also does not claim `field_solve.py` has
already passed `check_parity.py` from a WASM run.

The new Pact acceptance milestone is sharper:

```bash
python collab/pact/pact_witness_kernel/check_parity.py candidate_outputs.npz
```

The next Molt milestone is Kernel A first:

1. Treat `pact_witness_kernel/field_solve.py` as the executable contract.
2. Compile `field_solve(lstar)` through the Molt path chosen for determinism.
3. Run on generated `pact_witness_kernel/lstar_sample.npz`.
4. Save all 11 output keys to `candidate_outputs.npz`.
5. Pass `check_parity.py` against `reference_outputs.npz`.

Kernel B (`witness_forward.py`) follows after Kernel A, with exact uint8
agreement against `witness_forward_reference.npz`.

## Molt implementation direction

The Molt side should keep two lanes separate:

- Authority lane: WASM-CPU determinism first, using the Pact oracle. This lane
  proves exact semantics for `distance_transform_edt`, reflect-mode filters,
  label connectivity, percentile, stable top-k sorting, gradient/eigh output
  ordering, and integer coordinate gates.
- Speed lane: WebGPU/WGSL and SIMD for embarrassingly parallel pieces once the
  authority lane exists. A faster preview path is welcome only if the authority
  path remains available and green.

The package-source rule stays fixed: compile only the package code needed by the
user's program, admit only source-recompiled native extension artifacts with
explicit custody sidecars, and keep tree-shaking/deforestation intact from
Python source to browser payload. No host-Python fallback, patched NumPy/SciPy
sources, or compatibility crutches are completion paths.

## Handoff path

Use this file as the Molt response for the current review:

`collab/pact/007_molt_response_numpy_scipy_c_api_greenup_and_witness_kernel_plan.md`

The prior ask is now unblocked at the missing-symbol layer; the current shared
exit criterion is the Pact oracle PASS for Kernel A from a Molt-produced
candidate artifact.
