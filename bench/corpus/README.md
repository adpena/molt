# bench/corpus — molt's benchmark corpus

The **benchmark corpus catalog** records intended external Python benchmarks,
CPython regression tests and real applications. Registration does not mean a
workload is executable or measured. HTML5lib and SQLGlot have explicit executable
recipes consumed by `tools/benchmark_applications.py` and selected through the
existing benchmark runners. The other catalog rows remain intended coverage.
The diagnostic core belongs to `tools/bench_suites.py`; friend recipes belong to
`bench/friends/manifest.toml`.
The comprehensive optimization portfolio and next application tranche are in
[doc 69](../../docs/design/foundation/69_benchmark_corpus_union_and_dynamic_calibration.md);
the full external-suite catalog is [doc 69A](../../docs/design/foundation/69A_benchmark_corpus_catalog.md).

## Layout

| Path | What |
|---|---|
| `registry.toml` | intended corpus plus explicit application execution recipes and size/SHA-pinned package/data archives. A recipe supplies inputs, not qualification receipts. |
| `clbg/` | the 10 Computer Language Benchmarks Game kernels, vendored byte-exact (see `clbg/README.md`). |
| (adapters) | `tools/pyperformance_adapter.py`, `tools/codon_adapter.py`, `tools/regrtest_adapter.py` — one per external suite. |
| (harness) | `tools/bench_friends.py` + `bench/friends/manifest.toml` — the "beat them on their own suite" runner. |
| (calibration) | `tools/perf_calibration.py` provides measurement primitives used by selected adapters. The pyperformance in-process shim does not use that process measurement protocol. |

## Quick start (CPython reference diagnostics)

```bash
# 1. Get the Python bench deps (pyperformance + pyperf), pinned:
uv sync --group bench

# 2. Run a friend suite's CPython reference lane (no molt build needed):
uv run --python 3.12 python tools/bench_friends.py \
  --manifest bench/friends/manifest.toml --suite pyperformance_benchmarks --dry-run
uv run --python 3.12 python tools/bench_friends.py \
  --manifest bench/friends/manifest.toml --suite pyperformance_benchmarks --checkout --fetch

# Or drive an adapter directly:
uv run --python 3.12 python tools/pyperformance_adapter.py run-group --group math --json
uv run --python 3.12 python tools/regrtest_adapter.py run --runner cpython --python python --json
```

A host with only CPython can collect reference diagnostics. Engine-specific
lanes may skip with explicit reasons; skips remain missing coverage. The
pyperformance adapter now fails on workload errors. Its in-process shim and
fingerprints do not substitute for upstream pyperf methodology or independent
semantic output checks.

## Declared application diagnostics

Use the active project interpreter. Acquisition and staging happen before timed
samples, through the existing archive and source-package-seal authorities:

```bash
python tools/bench.py --applications --samples 3 --warmup 1
python tools/perf_scoreboard.py --set applications --backend native \
  --profile release-fast --samples 3 --warmup 1
```

Individual, WASM and legacy benchmark runners also expose `--applications`.
These diagnostics use complete package/resource trees, datasets, project
configuration, provenance and output fixtures under one sealed project. The
same admitted paths, import roots, arguments and cwd drive CPython and Molt.
Unqualified comparator or transformed-source protocols refuse explicitly.
This does not expand the canonical release gate or qualify an execution target.

The HTML driver emits the complete parsed tree, including attributes, text and
tails. The SQL driver emits complete parsed/optimized AST dumps and generated
Spark SQL for the upstream TPC-H input. SQLGlot 30.24.0 is an explicit modern
variant, not upstream pyperformance's historical `sqlglot_v2`/4.6.0 result.
References must match the full sealed stdout fixture with empty semantic stderr;
all guest warmups and samples must then match the qualified reference.

Regenerate expected outputs explicitly with the selected reference interpreter:

```bash
python tools/benchmark_applications.py --write-oracle html5lib
python tools/benchmark_applications.py --write-oracle sqlglot
```

Review oracle changes as semantic changes. Generation does not qualify Molt or
replace upstream conformance tests. Both application runtime-performance cells
remain unqualified. In particular, the pinned HTML5lib dependency `six` executes
source with `exec`; Molt intentionally rejects it under the [AOT policy](../../docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md).
Retain the failure without a timing score or a package/compiler workaround.
Failures within the intended subset remain defects, not new policy exclusions.

## Suites + lane status

| Suite | CPython (oracle) | molt | reference engine |
|---|---|---|---|
| `pyperformance_benchmarks` | ✅ active (97 IDs, full v1.14.0) | ⏸ deferred (needs build + `pyperformance` importable under molt) | — |
| `codon_benchmarks` | ✅ active | ⏸ deferred (needs build) | codon — ✅ active, self-skips if absent |
| `cpython_regrtest` (adapter landed) | ✅ active (`tools/regrtest_adapter.py`, version-gated) | ⏸ deferred (libregrtest via molt import boundary) | — |
| `clbg/` kernels | vendored source; no registry-driven execution | — | — |

molt lanes are deferred until each is **built and verified per suite** — activated
incrementally, never enabled unverified. The molt *native* board
(`tools/perf_scoreboard.py`) benchmarks molt directly today; these friend lanes are the
cross-engine comparison.

## Optional reference engines (out-of-band, lanes self-skip if absent)

- **Codon** (AOT north-star) — Linux/macOS: `bash -c "$(curl -fsSL https://exaloop.io/install.sh)"`, ensure `codon` on PATH. (No Windows build; the codon lane self-skips there.)
- **PyPy** (dynamic reference) — `uv run --python pypy@3.11 ...` (auto-probed by `tools/bench.py`).

## The rules (from doc 69 / the Performance Constitution)

- **CPython is the universal oracle.** A comparison is a **win/loss only where both
  engines are semantically equivalent** on that benchmark. Codon's three non-equivalent
  zones — **bignum**, **Unicode**, **C-semantics negative floor-div/overflow** — are
  marked in `registry.toml` and carry `scored_against_codon=false`; they are reported,
  **never** scored as a Codon win/loss.
- **Measure time AND memory.** This is the target contract. Check the actual
  producer: process observations, in-process elapsed time, confidence intervals
  and peak RSS are different evidence. A pyperformance shim result does not
  supply all of these or inherit them from a process-level suite receipt.
- **Version-gated.** Parity + perf are dimensioned by Python version (3.12/3.13/3.14).

## Adding a suite

1. Add a `[[suite]]` block to `bench/friends/manifest.toml` with a **pinned immutable
   `repo_ref`** + per-runner lanes. Resolve reference dependencies before running;
   preserve failures and explicit unavailable-engine reasons as missing coverage.
2. Write `tools/<suite>_adapter.py` (follow `codon_adapter.py` / `regrtest_adapter.py`):
   reuse `perf_calibration.run_and_measure`; emit the `bench_friends` JSON shape; mark
   non-equivalent comparisons `scored_against_*=false`.
3. Register its benchmarks in `registry.toml` (dedup against existing canonical ids;
   tag `source_suites` + `semantic_tier`).
4. Verify the CPython lane runs green before enabling; defer engine lanes (with a clear
   `skip_reason`) until each is built + verified.
