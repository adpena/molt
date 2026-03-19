# Differential Test Index

Last updated: 2026-03-19

This directory is the canonical home for Molt differential tests.

## Lanes

- `tests/differential/basic/`: core language and builtin semantics.
- `tests/differential/stdlib/`: stdlib module and submodule semantics.
- `tests/differential/pyperformance/`: targeted pyperformance smoke inputs.

## Running

Use canonical artifact roots before long sweeps:

```bash
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
export MOLT_DIFF_MEASURE_RSS=1
export MOLT_DIFF_RLIMIT_GB=10
```

Example targeted run:

```bash
UV_NO_SYNC=1 uv run --python 3.12 python3 -u tests/molt_diff.py \
  tests/differential/basic/re_parity.py \
  tests/differential/basic/dataclasses_parity.py \
  --python-version 3.12 \
  --build-profile dev \
  --jobs 1 \
  --live
```

## Fresh Evidence

- 2026-03-19 targeted recovery:
  - serializer blocker fixed for persisted module-analysis caches with `bytes` defaults;
  - targeted builtins-symbol differential cases now pass:
    - `builtins_symbol_open_5fc7e38b.py`
    - `builtins_symbol_property_269fe373.py`
    - `builtins_symbol_compile_36d0981b.py`
  - focused regressions now pass:
    - `tests/differential/stdlib/re_match_metadata.py`
    - `tests/differential/basic/dataclasses_frozen_instance_error.py`
  - original parity regressions now pass:
    - `tests/differential/basic/re_parity.py`
    - `tests/differential/basic/dataclasses_parity.py`
  - evidence artifacts:
    - `logs/diff-targeted-builtins/`
    - `tmp/diff-targeted-builtins/summary.json`
    - `logs/diff-regressions-green3/`
    - `tmp/diff-regressions-green3/summary.json`
    - `logs/diff-parity-rerun/`
    - `tmp/diff-parity-rerun/summary.json`

## Open Operational Risk

- Full-suite differential throughput remains expensive on this host. Recent targeted reruns show build RSS peaks above 6 GB for some parity cases, so broad sweeps should continue to use RSS measurement, canonical temp roots, and low parallelism when triaging.
