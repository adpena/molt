<!-- Appendix A of doc 69 (benchmark corpus union + dynamic calibration). The complete
external-suite landscape from the 2026-06-24 corpus-research sweep. Seeds the canonical
registry R (bench/corpus/registry.toml) + the suite adapters S1-S6. Verbatim research
artifact — do not condense. -->

# 69A — Benchmark Corpus Research Catalog (Appendix A of doc 69)

> Seeds the canonical-benchmark registry `R` (`bench/corpus/registry.toml`) and the suite
> adapters `S1–S6` of [doc 69](69_benchmark_corpus_union_and_dynamic_calibration.md). Each
> suite is given as: origin/license/acquisition (for source-custody pinning) → benchmark
> inventory → methodology → **semantic tier per engine** (`runs_unmodified` /
> `requires_adapter` / `unsupported_by_molt`; for Codon/PyPy additionally **equivalent vs
> non-equivalent** — a comparison is a win/loss ONLY where both engines are semantically
> equivalent) → cross-suite overlap (for dedup). Research 2026-06-24; lists cross-verified
> against primary sources (repos, MANIFESTs, papers, project docs).

## A.0 — Executive findings (the load-bearing facts)

1. **Four CLBG-rooted kernels are the spine of the entire ecosystem.** `nbody`,
   `fannkuch-redux`, `spectral-norm`, `mandelbrot`, `pidigits`, `binary-trees`,
   `meteor-contest` originate in the Computer Language Benchmarks Game and propagate (often
   verbatim, with attribution) into pyperformance, the PyPy suite, and the Codon paper.
   **Register each ONCE** in `R` tagged with every source suite — the primary dedup
   obligation.
2. **Three semantic "non-equivalent on Codon" zones must be marked, never scored:**
   (a) **arbitrary-precision integers** — Codon `int` is 64-bit, wraps silently (pidigits,
   factorial, RSA/modpow, big-fib, Karatsuba, int↔str CVE); (b) **Unicode** — Codon `str`
   is ASCII-only (unicode normalization, codec encode/decode, most text/regex); (c)
   **C-semantics numerics** — Codon floor-div/modulo of negatives truncates toward zero
   (Python floors toward −∞), int overflow wraps (Python promotes). These are *correctness
   divergences*, not perf — a Codon "win" on any of them is meaningless.
3. **"C-extension-bound" ≠ "interpreter speed."** A large fraction of
   real-world/scientific/serialization benchmarks do their heavy lifting in
   C/Rust/Fortran/BLAS/GPU (numpy, pandas, orjson, msgpack, protobuf-upb, uvloop, greenlet,
   pydantic-core, PyTorch, DuckDB/Polars). For these the honest molt comparison is **"molt's
   implementation vs the native tool"**, reported in a separate column from **"molt vs
   CPython's bytecode interpreter."** Conflating them is a methodology violation.
4. **Codon's native NumPy changes the scientific tier.** Codon ships a from-scratch,
   fully-compiled NumPy (operator/expression fusion + LLVM auto-vectorization), so
   NPBench-style array benchmarks ARE Codon-comparable — but as "Codon-native-NumPy vs
   molt-NumPy vs CPython-calls-C-NumPy," a three-way distinction that must be labeled.
5. **Each engine must be measured on its OWN suite with its OWN methodology** (doc 69 §2).
   pyperformance → pyperf; PyPy → its long-JIT-warmup runner + speed.pypy.org; Codon → its
   `bench/run.sh` + release mode; CLBG → fixed-N single-shot wall-clock. "Beat them on their
   own turf" is the unimpeachable claim.
6. **Methodology references worth adopting wholesale:** pyperf's calibration/worker/
   statistics discipline (the doc 69 §3 target); ASV's per-commit-isolation + peakmem
   tracking; NPBench's **validate-every-output-against-a-CPython-reference** rule (aligns
   with the differential oracle, doc 66).

## A.1 — pyperformance (the official CPython suite — the CPython-reference axis)

| Field | Value |
|---|---|
| **Origin** | `github.com/python/pyperformance` (PSF / Faster-CPython; Victor Stinner). Descends from Unladen Swallow via `python/performance`. |
| **License** | **MIT** — fully vendorable. |
| **Version to pin** | **1.14.0** (2026-02-07). `repo_ref = "1.14.0"`. |
| **Acquisition** | `pip install pyperformance`; `git clone https://github.com/python/pyperformance`. Benchmarks at `pyperformance/data-files/benchmarks/bm_<name>/`. **Authoritative list = the `…/benchmarks/MANIFEST`** (the docs page lags). |
| **pyperf** | the timing/statistics engine (workers, calibration, JSON, stats); pyperformance = harness+corpus on top. Pin both. |
| **Adapter** | `S1` — extend `tools/pyperformance_adapter.py` past the `nbody,fannkuch` smoke subset to the full set. |

**Methodology (pyperf engine).** Never runs two benchmarks concurrently. Per benchmark: a
calibration worker computes the outer-loop count via **autorange** (scale loops until one
*value* ≈ 100 ms, powers of two); then measurement workers run sequentially, each a fresh
process. **Defaults (CPython):** 20 processes × 3 values = **60 values**, 1 warmup/process
discarded. **With a JIT (PyPy):** 6 processes × 10 values = 60, **10 warmups**.
`--rigorous` doubles processes; `--fast` halves them. Inner loops hand-unrolled per
benchmark (`inner_loops`). Reports `Mean ± std dev`; API exposes mean/median/stdev/
percentiles. Instability warnings on large stdev/outliers/short-vs-resolution. JSON:
`{version, metadata, benchmarks:[{metadata, runs:[{warmups:[[loops,val]], values:[...]}]}]}`.
Analysis: `compare_to` (the molt-vs-CPython ratio tool), `stats`, `hist`, `dump`, `slowest`.
**pyperformance is explicitly NOT tuned for PyPy** — use `pypy/benchmarks` for PyPy.

**FULL inventory (v1.14.0 MANIFEST)** — ~95 dirs expanding to ~120+ runnable IDs:
- **startup:** `python_startup`, `python_startup_no_site`, `stdlib_startup`, `hg_startup`(legacy).
- **asyncio:** `asyncio_tcp`, `asyncio_tcp_ssl`, `asyncio_websockets`, `async_generators`,
  `coroutines`, `generators`, `async_tree`, `async_tree_io`, `async_tree_memoization`,
  `async_tree_cpu_io_mixed`, the `_eager` flavors, the `_tg` (TaskGroup) flavors of all
  eight tree variants, `concurrent_imap`.
- **template:** `chameleon`, `django_template`, `genshi_text`, `genshi_xml`, `mako`.
- **serialize:** `json_dumps`, `json_loads`, `pickle`, `pickle_dict`, `pickle_list`,
  `pickle_pure_python`, `unpickle`, `unpickle_list`, `unpickle_pure_python`,
  `xml_etree_parse`, `xml_etree_iterparse`, `xml_etree_generate`, `xml_etree_process`,
  `tomli_loads`, **`yaml`**(NEW 1.14).
- **encoding (NEW 1.14):** `base64`, `base32`, `base16`, `ascii85`, `base85` (+size variants).
- **regex:** `regex_compile`, `regex_dna`, `regex_effbot`, `regex_v8`.
- **math:** `float`, `nbody`, **`nbody` Barnes–Hut quadtree**(NEW 1.12), `pidigits`,
  `spectral_norm`, `pyflate`, `crypto_pyaes`, `telco`, **`decimal`**(NEW 1.12).
- **scimark:** `scimark_sor`, `scimark_sparse_mat_mult`, `scimark_monte_carlo`,
  `scimark_lu`, `scimark_fft`.
- **algorithms/puzzles:** `richards`, `richards_super`, `deltablue`, `chaos`, `raytrace`,
  `go`, `hexiom`, `nqueens`, `fannkuch`, `meteor_contest`, `comprehensions`, `deepcopy`,
  `unpack_sequence`, `mdp`, `typing_runtime_protocols`, `coverage`, `gc_traversal`,
  `gc_collect`.
- **data structures:** `btree`, `btree_gc_only`, **`bpe_tokeniser`**(NEW 1.12, LLM-relevant).
- **apps:** `2to3`, `docutils`, `html5lib`(re-enabled 1.14), `tornado_http`,
  **`fastapi`**(NEW 1.14), **`sphinx`**(NEW 1.12), `dulwich_log`, `pprint`, `pathlib`,
  `logging_format`, `logging_simple`, `logging_silent`, **`argparse`**/`argparse_subparsers`(NEW 1.12).
- **symbolic/ORM/SQL/data-eng/compilers:** `sympy_expand`, `sympy_integrate`, `sympy_str`,
  `sympy_sum`, `sqlalchemy_declarative`, `sqlalchemy_imperative`, `sqlite_synth`,
  `sqlglot_v2`/`_parse`/`_transpile`/`_optimize`, `dask`(≥3.12; skipped Win/3.13),
  **`networkx`**/`networkx_connected_components`/`networkx_k_core`(NEW 1.12),
  **`xdsl`**(NEW 1.12, re-enabled 1.13).
- *Historically removed (do NOT expect):* `call_simple`/`call_method`/`call_method_slots`/
  `call_method_unknown`, `pybench`, `spambayes`, the `2n3` group.

**Semantic tier:** CPython all `runs_unmodified` (the oracle). PyPy: the pure-Python set +
equivalent. molt: pure-Python set is the core target; **flag the C-accelerated** ones
(`json_*`, `pickle`/`unpickle` vs their `*_pure_python` variants, `decimal`/`telco`,
`xml_etree_*` race CPython's *C accelerator*, not its interpreter — decide per-benchmark
which bar). Codon `requires_adapter`+equivalent (numeric/loop): `float`, `nbody`,
`spectral_norm`, `fannkuch`, `nqueens`, `scimark_*`, `crypto_pyaes`, `chaos`, `raytrace`,
`go`, `mdp`, `meteor_contest`, `unpack_sequence`, monomorphic `deltablue`/`richards`;
`pidigits` NON-equivalent (bignum). Codon non-equivalent/unsupported: `sympy_*`,
`django_template`, `sqlalchemy_*`, `fastapi`, `dask`, `networkx*`, `xdsl`, `sphinx`,
`docutils`, `2to3`, `coverage`, `typing_runtime_protocols`, all `async*`/asyncio,
`genshi_*`, `html5lib`, `tornado_http`, `dulwich_log`, `sqlglot_v2*`, pickle family.

**Toughest:** `richards`/`deltablue` (megamorphic OO dispatch — PyPy turf), `go` (dict
churn), `pyflate` (pure-Python bit/byte), `nbody`/`spectral_norm`/`float` (Codon FP
bake-off), `gc_collect`/`gc_traversal` (cycle collector — Codon N/A), `async_tree` non-IO.
**C-extension-flagged:** `sqlalchemy_*`(Cython), `tornado_http`, `dulwich_log`, `genshi_*`,
`dask`(numpy/pandas), `networkx*`, `fastapi`(**pydantic-core=Rust**), `sphinx`(lxml),
`yaml`(libyaml), `sqlglot_v2*`(opt Rust), `coverage`(C tracer).

## A.2 — Computer Language Benchmarks Game (CLBG — root of the recurring kernels)

| Field | Value |
|---|---|
| **Origin** | `benchmarksgame-team.pages.debian.net`; source `salsa.debian.org/benchmarksgame-team/benchmarksgame`. Bagley(2001)→Fulgham(2004)→**Gouy**(2008–). |
| **License** | **Revised BSD (3-clause)** © 2004–08 Fulgham, 2005–24 Gouy. Vendorable; retain copyright+license+disclaimer; **name-endorsement clause**. |
| **Acquisition** | `git clone https://salsa.debian.org/benchmarksgame-team/benchmarksgame.git`. Multiple Python entries per problem ("Python 3 #N"). |
| **Adapter** | `S4` — vendor the Python sources (permissive) + register. Parameterize at both CLBG timed-N and the smaller pyperformance N. |

**Ten canonical problems** (timed-N / stresses): `n-body`(50M/scalar FP loop),
`fannkuch-redux`(12/int+array+in-place reverse), `spectral-norm`(5500/FP+nested+sqrt),
`mandelbrot`(16000/FP+complex+bit-packed out), `fasta`(25M/LCG RNG+cumulative lookup+bulk
str), `k-nucleotide`(fasta25M/dict+str-hash+count-in-place), `regex-redux`(~5MB
fasta/**regex engine**), `pidigits`(10000/**bignum**), `reverse-complement`(~25MB/byte
translate+buffered IO+reverse), `binary-trees`(21/**alloc+GC stress**, Boehm GCBench
lineage; pools/arenas prohibited). *Retired-era:* `chameneos-redux`, `thread-ring`,
`meteor-contest`(still in pyperformance).

**Variants** (verified): pure-Python (`nbody-python3-1` imports only `sys`, lists+dict — the
exact program pyperformance adopted); **numpy** variants for FP problems;
**multiprocessing** variants (`spectralnorm-python3-4`). Always read the source — the table
doesn't label deps.

**Methodology (NOT pyperf):** single quad-core (i5-3330), **BenchExec** (cgroups+namespaces;
caches/swap cleared per run). Metrics: elapsed wall, CPU time (summed threads), **peak
memory**, **gzipped source size**. Run **12×**, discard first, stats over 11; report lowest
elapsed or 95% CI over CPU. **Cold start + binary/source footprint are first-class.**

**Semantic tier:** CPython/PyPy all `runs_unmodified` (FP/alloc kernels = PyPy showcase).
Codon-equivalent: `n-body`, `spectral-norm`, `mandelbrot`, `fannkuch-redux`,
`binary-trees`, `fasta`, `k-nucleotide`, `reverse-complement`(ASCII). Codon-NON-equivalent:
**`pidigits`**(bignum), **`regex-redux`**(engine-dependent). **Toughest:** `binary-trees`
(the RC-overhead exposer — ownership-lattice fixture), `pidigits`(bignum parity),
`n-body`/`spectral-norm`/`mandelbrot`(Codon/native FP bake-off), `regex-redux`(engine fidelity).

## A.3 — PyPy benchmark suite (the PyPy-reference / dynamic axis)

| Field | Value |
|---|---|
| **Origin** | current `foss.heptapod.net/pypy/benchmarks` (hg); historical `bitbucket.org/pypy/benchmarks`(dead). A fork of Google's Unladen Swallow suite. Dashboard **speed.pypy.org** (Codespeed). |
| **License** | **mixed/composite** — permissive per-file but bundles whole apps (Django, Twisted, SymPy, Mako, Genshi, spitfire, spambayes) each with its own license. **Vendor per-benchmark, NOT suite-wide.** |
| **Acquisition** | `hg clone https://foss.heptapod.net/pypy/benchmarks`. **No pip.** (Heptapod is bot-walled; benchmark *names* confirmed via speed.pypy.org + Unladen lineage — a real `hg clone` is needed for exact `own/` filenames.) Pin a commit. |
| **Adapter** | `S2` — pin + adapter (tinygrad/numpy pattern); long-JIT-warmup runner. |

**Inventory:** *kernels (own/Unladen):* `richards`, `deltablue`, `chaos`, `go`,
`raytrace-simple`, `pyflate-fast`, `spambayes`, `spectral-norm`, `nbody_modified`, `telco`,
`crypto_pyaes`, `hexiom2`, `nqueens`, `pidigits`, `fannkuch`, `float`, `meteor-contest`,
`ai`, `bm_mdp`, `eparse`. *app/library:* `django`, `rietveld`, `bm_mako`, `bm_chameleon`,
`genshi_text`/`genshi_xml`, `html5lib`, `json_bench`, `sympy_*`, `sqlalchemy_*`,
`sqlitesynth`, `slowspitfire`/`spitfire`/`spitfire_cstringio`(+`2`), `sphinx`. *Twisted:*
`twisted_iteration`, `twisted_names`(DNS), `twisted_pb`(RPC), `twisted_tcp`. *SciMark:*
`scimark_{fft,lu,montecarlo,sor,sparsematmult}`. **Marquee real workload — `translate`/
`trans2`:** translating/compiling the **PyPy interpreter itself** through RPython
(`trans_annotate`/`_rtype`/`_backendopt`/`_database`/`_source`) — multi-minute, multi-GB
compile of a production codebase; **unique to PyPy; molt cannot run it (RPython), but it's
the reference for compiler-on-compiler scale.** Plus `pypy_interp` bootstrap.

**Methodology:** historically a custom Unladen `runner.py`/`saveresults.py` (not pyperf);
modern CI → Codespeed. **JIT warmup is the defining concern** (many iters to steady state;
short benchmarks distrusted). Reports per-benchmark ratio vs CPython + **geometric mean**;
speed.pypy.org tracks the commit timeline. **Semantic tier:** deliberately pure-Python
dynamic. CPython all `runs_unmodified`; PyPy = reference engine; **molt = the ideal "PyPy
dynamic reference" set** (every benchmark exercises dynamic dispatch / attr lookup / alloc —
exactly the facts molt recovers: IC tiering, class-version guards, borrow inference, loop
specialization); Codon: app/Twisted/sympy/sqlalchemy non-equivalent, kernel subset overlaps
CLBG. **Toughest:** `translate`, `richards`/`deltablue`, `spitfire`/`twisted_*`.

## A.4 — Pyston macrobenchmarks (real-application macro axis)

| Field | Value |
|---|---|
| **Origin** | `github.com/pyston/python-macrobenchmarks` (mirror `faster-cpython/...`). Pyston v2 launch (Oct 2020) — built because existing benchmarks were "too small." speed.pyston.org. |
| **License** | **MIT** (suite-wide, clean); pip deps (torch, Pyramid, gevent…) carry their own. |
| **Acquisition** | `git clone …/python-macrobenchmarks`; `benchmarks/bm_<name>/` + pinned `requirements.txt`. Run via `pyperformance run --manifest benchmarks/MANIFEST -b pyston_standard`. Pin a commit. |
| **Adapter** | folds into `S5` (real-world apps). |

**13 macrobenchmarks:** `bm_flaskblogging`, `bm_djangocms`, `bm_kinto`(Pyramid), `bm_aiohttp`,
`bm_gunicorn`, `bm_gevent_hub`(cancel_wait/switch/wait_func_ready/wait_ready), `bm_mypy2`,
`bm_pylint`, `bm_pycparser`, `bm_thrift`, `bm_json`, `bm_pytorch_alexnet_inference`, +mypyc.
`[group pyston_standard]` = djangocms+flaskblogging+kinto (the web triad). **Methodology:**
pyperformance+pyperf; web benchmarks report **mean AND p99 latency**; warmup="time to 95% of
peak." **Semantic tier:** CPython all `runs_unmodified`; molt-comparable =
djangocms/flaskblogging/kinto/pylint/pycparser (framework dispatch is pure Python).
**C-ext-dominated, mark "non-equivalent":** `bm_pytorch_alexnet_inference`(torch/numpy/Pillow),
`bm_mypy2`/mypyc, `bm_gevent_hub`(greenlet C), `bm_kinto`(cffi/cryptography/uWSGI/…),
`bm_thrift`(C codec), `bm_aiohttp`(C `_http_parser`). Codon: ~none unmodified. **Overlap:
near-zero** with PyPy/CLBG (intentional) — a clean third axis.

## A.5 — Codon benchmarks (the AOT/native-reference axis)

| Field | Value |
|---|---|
| **Origin** | `github.com/exaloop/codon` (Exaloop). Paper: Shajii et al., **"Codon: A Compiler for High-Performance Pythonic Applications and DSLs," CC 2023** (SIGPLAN Compiler Construction — *NOT CGO*), **DOI 10.1145/3578360.3580275**. |
| **License** | **Apache-2.0 as of 2025** (relicensed from BUSL/BSL 1.1). **Vendor from a CURRENT Apache checkout** — pre-2025 BUSL-era tags carry non-commercial restriction. Paper text CC BY-NC-ND. |
| **Acquisition** | `bash -c "$(curl -fsSL https://exaloop.io/install.sh)"`; **benchmarks in `bench/`** on `develop` (`run.sh`, `codon/`). Pin a current commit. |
| **Adapter** | `S3` — pin + adapter; **mark non-equivalent semantics** per benchmark. |

**Repo `bench/` set (current):** `chaos`, `float`, `go`, `nbody`, `spectral_norm`,
`mandelbrot`(`@par(gpu=True)`), `set_partition`, `sum`(1..50M loop), `taq`(NYSE TAQ peak
detect), `binary_trees`(Boehm), `fannkuch`(`@par dynamic`), `word_count`(dict freq),
`primes`(`@par dynamic`). **Paper Fig 5** (vs CPython 3.10, PyPy 7.3, C++ where applic.):
`loop`, `go`, `nbody`, `chaos`, `spectral_norm`, `set_partition`, `primes`, `binary_trees`,
`fannkuch`, `word_count`, `taq`. **Fig 5 is an unlabeled bar chart — do NOT cite specific
per-bar ratios** (detailed data in Appendix B / Seq OOPSLA'19 / Nature Biotech'21). Prose:
"always faster, sometimes orders of magnitude, vs CPython & PyPy; C++-class where C++
provided." **DSL results (exact):** BWA-MEM **2× faster than optimized C, up to 4× shorter**;
FM-index ~2× with `@prefetch`; Sequre GWAS **4× faster, 7× less code vs C++ SOTA**; CoLa
1.06/0.67/0.91× vs reference C. **2025 NumPy:** from-scratch compiled NumPy (ndarray =
{shape,strides,data}, dim as type param; fusion IR + LLVM auto-vec + Highway); **NPBench:
2.4× geomean, up to 900×, single-thread, vs Py3.12+NumPy1.26.**

**Methodology:** CPython 3.10 + PyPy 7.3 comparators; C++ subset; **release mode** (−O3-class,
drops safety/debug); single-thread headline (`@par`/GPU is a separate axis); Boehm GC; forked
LLVM. docs headline `fib(40)` 17.98s→0.28s ≈65×. **Semantic divergences (decisive):**
(1) 64-bit signed `int`, not bignum (`-numerics=py` doesn't change this); (2) C-semantics
numerics by default (div/mod sign, overflow), `-numerics=py` restores Python div/overflow-check
but NOT bignum; (3) ASCII `str`; (4) `dict` does NOT preserve insertion order; (5) tuples→fixed
structs; (6) static typing/monomorphization — no runtime polymorphism/reflection/metaclasses/
monkeypatching/class-decorators; (7) no full C-API (`from python import` runs at *CPython
speed*); (8) no exec/eval/compile.

**Tier (win/loss map):** *equivalent:* `nbody`, `spectral_norm`, `mandelbrot`, `float`(IEEE
double), `fannkuch`, `binary_trees`, `go`, `chaos`, `set_partition`, `sum`, `primes`(within
64-bit), ASCII order-independent `word_count`. *NON-equivalent (never win/loss):* anything
bignum, Unicode/text, dict-order-observing, dynamic-feature, bridged-library (CPython speed),
the DSL results (vs hand-C, a different axis). **Toughest for molt:** the FP kernels +
cold-start/binary-size (both AOT) + the genomics DSL set — Codon's flagship turf, the AOT
north star molt must approach/exceed.

## A.6 — Unladen Swallow legacy (lineage root — provenance, not a live suite)

Google project (Winter & Yasskin, Dec 2008; PEP 3146). **Not separately maintained** — the
ancestor of both pyperformance and the PyPy suite; migrated to `python/performance` (2016),
became pyperformance's seed. Tool `perf.py` = the direct ancestor of `pyperf`. Kernels
(richards, nbody, …) survive in pyperformance/PyPy today. **No separate adapter** — record
as the *provenance root* tag on shared kernels so the lineage CLBG→Unladen→{pyperformance,
PyPy} is explicit in `R`.

## A.7 — Scientific / numerical (the NumPy / scientific axis)

**Three comparability tiers for ALL scientific benchmarks (load-bearing):** **Tier A**
pure-Python numeric loops (clean compiler-vs-compiler); **Tier B** idiomatic NumPy (CPython
baseline = C-NumPy → "molt-NumPy vs C-NumPy"; **Codon uniquely comparable** via native
compiled NumPy); **Tier C** library-internal (the benchmark *is* the C/Cython/BLAS library —
ecosystem-parity only, never an interpreter-speed claim).

**A.7.1 NPBench (Codon's 2025 reference; the canonical NumPy suite).** `github.com/spcl/npbench`
(ETH SPCL; Ziogas et al. ICS'21). **BSD-3** — vendorable. **52 benchmarks / 8 domains**, 4 size
presets; run 10×, **median**, speedup vs NumPy median, **95% CI via bootstrapping**; **every
result validated against the NumPy reference** (matches the oracle, doc 66). *PolyBench (32):*
`adi`,`atax`,`bicg`,`cholesky`,`cholesky2`,`correlation`,`covariance`,`deriche`,`doitgen`,
`durbin`,`fdtd_2d`,`floyd_warshall`,`gemm`,`gemver`,`gesummv`,`gramschmidt`,`heat_3d`,
`jacobi_1d`,`jacobi_2d`,`k2mm`,`k3mm`,`lu`,`ludcmp`,`mvt`,`nussinov`,`seidel_2d`,`symm`,
`syr2k`,`syrk`,`trisolv`,`trmm`. *DL(5):* `conv2d_bias`,`lenet`,`mlp`,`resnet`,`softmax`.
*Stencils:* `hdiff`,`vadv`. *Others:* `arc_distance`,`azimint_naive`/`_hist`,`stockham_fft`,
`cavity_flow`/`channel_flow`,`nbody`,`scattering_self_energies`/`contour_integral`,`spmv`,
`mandelbrot1`/`mandelbrot2`,`crc16`,`go_fast`,`compute`,`clipping`. **PolyBench ships both
`*_numpy.py` (Tier B) and explicit-loop Python (Tier A — cleanest molt-vs-Codon-vs-PyPy).**

**A.7.2 Numba benchmarks (the Tier-A jackpot).** `github.com/numba/numba-benchmark` —
**BSD-2**, ASV-based. The cleanest molt-comparable set across all four engines (explicit
scalar Python loops, no C-ext, matched semantics): `bench_blackscholes`, `bench_nbody`,
`bench_laplace`(stencil), `bench_centdiff`, `bench_gameoflife`, `bench_ising`. (Also
`bench_arrayexprs`/`vectorize`/`dispatch`/`compiling`/`iterating`/`lists`/`sets`/`sorting`/
`random`/`jitclass`/`cuda`; `compiling`/`dispatch` are JIT-warmup-specific, `cuda`/downstream
pull heavy deps.)

**A.7.3 ASV ecosystem suites (Tier C).** **ASV** = clone repo, isolated venv per commit, track
perf across history; `time_*` (calibrated+warmup), `peakmem_*`(max RSS), `mem_*`, `track_*`;
CPU affinity, instability detection, `setup`/`setup_cache` — **a doc 69 §3 methodology
reference** (per-commit isolation + peakmem). NumPy `benchmarks/`(bench_core/ufunc/linalg/…),
SciPy `benchmarks/`(optimize/linalg/sparse/fft/…), scikit-learn `asv_benchmarks/`(cluster/
ensemble/linear_model/…) — all BSD-3, all Tier C (C/Fortran/Cython/BLAS internals).

**A.7.4 pandas / dataframe / ETL (Tier C).** pandas `asv_bench/`(groupby/join_merge/indexing/
io/rolling/…, BSD-3) — dispatches to NumPy/C/Cython/PyArrow. **H2O.ai db-benchmark**
(`duckdblabs/db-benchmark`, **MPL-2.0**) — canonical groupby+join at scale (0.5/5/50 GB; 10
groupby + 5 join questions). **TPC-H / Polars PDS-H** (22 analytical queries). All Tier C
(query engines).

**Scientific guidance for `R`:** build the molt-vs-all **Tier-A** core from `numba-benchmark`
(Black-Scholes, n-body, Laplace/central-diff, Game-of-Life, Ising) + NPBench PolyBench **loop
forms** (cholesky, jacobi, seidel, heat_3d, floyd_warshall, nussinov) + standalone
Mandelbrot/Monte-Carlo-pi (permissive, no C-ext floor, no bignum/dict-order). Gate **Tier B**
(NPBench `*_numpy.py`) behind molt's NumPy story, **Codon as the apples-to-apples AOT peer**.
Use **Tier C** for ecosystem-parity correctness only.

## A.8 — Web / serialization / async (framework-dispatch, I/O, codec axes)

**Cross-cutting flag:** web & async are **I/O-bound** (framework dispatch + event-loop
scheduling, not raw compute); molt wins show as reduced per-request CPU. **Templating engines
are the CPU-bound exception** (string building) — a clean interpreter comparison.

**A.8.1 Web frameworks.** **TechEmpower (TFB)** `github.com/TechEmpower/FrameworkBenchmarks`
(**BSD-3**; archived read-only 2026-03-24; latest **Round 23**). 7 test types (JSON, single/
multiple-query, fortunes=templating, updates, plaintext=pipelining, cached-query); load gen
`wrk`; req/sec + latency. **36 Python framework dirs** (aiohttp, blacksheep, bottle, cherrypy,
django, emmett, falcon, fastapi, flask, granian, litestar, pyramid, quart, responder, robyn,
sanic, socketify, starlette, tornado, turbogears, uvicorn, uwsgi, web2py, wsgi, …). **Flag:**
the *fast* ones are dominated by C/Rust server cores (uvicorn=uvloop+httptools,
blacksheep=Cython, granian/robyn/socketify=Rust/C). **Pure-Python dispatch (molt-measurable):**
flask, bottle, django, falcon, pyramid, tornado, cherrypy + the app layer of
starlette/fastapi/sanic/quart. Codon: ~none unmodified. **Templating** (CPU-bound web subset):
**Jinja2** (compiles templates→Python — single most molt-relevant template target), **Mako**,
**Chameleon**, Django templates, Genshi. **Flag:** Jinja2/Mako/Chameleon do runtime
`compile`/`exec` → conflicts with molt's no-exec subset AND Codon's static model →
**pre-compile the template to a Python module ahead of time, then compile that.**

**A.8.2 Serialization.** **stdlib `json`** has a C accelerator + pure-Python fallback — **the
pure-Python path is the clean molt-vs-interpreter target.** Native libs (separate
"best-native-tool" column): **orjson**(Rust), **ujson**(C), **python-rapidjson**(C++),
**msgspec**(C). **orjson's `bench/` standard 4-file corpus is reusable:** `twitter.json`(631K),
`github.json`(55K), `citm_catalog.json`(1.7M), `canada.json`(2.2M float-heavy). **pickle** (C +
pure-Python; **`pickle_pure_python`/`unpickle_pure_python` = the cleanest interpreter
comparison**), msgpack-python (Cython + pure fallback), protobuf (now upb C/C++), pycapnp(C++),
flatbuffers (mostly pure-Python offset arithmetic), cbor2 (opt C + fallback), `tomli_loads`.
**Pure-Python targets:** stdlib json pure path, `pickle_pure_python`, msgpack-fallback,
cbor2-fallback, FlatBuffers runtime, `tomli_loads`. Codon: pickle pure path + FlatBuffers
offset arithmetic are the only candidates.

**A.8.3 Async / concurrency.** asyncio (pyperformance): `asyncio_tcp`(+`_ssl`=OpenSSL C),
`asyncio_websockets`, `async_tree` family, `coroutines`, `generators`, `async_generators`.
**Clean pure-Python async:** `async_tree`(no-IO + memoization), `coroutines`, `generators`,
`async_generators`. `_io` variants are sleep/timer-bound. **Codon: no asyncio — the entire
async family is molt-vs-CPython(/PyPy) only, never a Codon win/loss.** Event loops: **uvloop**
(Cython+libuv — best-native-tool column; reusable harness `examples/bench/echoserver.py`+
`echoclient.py`), trio (pure-Python but non-equivalent — nurseries ≠ gather). Concurrency:
threading/multiprocessing/asyncio throughput, **GIL-contention** (a molt-architecture-specific
differentiator you author — cross-ref `concurrency/locks.rs` + PEP-703), greenlet/gevent (C —
not interpreter-speed, not Codon-buildable).

## A.9 — "Hard"/discriminating microbenchmarks + real-world/ML (the stress + molt-stress axis)

Seed `R`'s "hardest case per category" + the **molt-stress corpus** (programs that broke molt).
Most are **constructed** (no canonical repo) — doc 69's corpus authors them, parameterized by
size. Most-discriminating per category bolded.

- **GC/alloc:** `binary-trees`(RC-overhead exposer — ownership-lattice lane), GCBench,
  `gc_collect`/`gc_traversal`(Codon N/A), `barnes_hut`, **cyclic-garbage + finalizer-ordering**
  (the resurrection/finalizer/weakref P0 corruption surface — correctness discriminator;
  Codon won't match finalizer/weakref semantics).
- **Megamorphic dispatch:** `richards`/`richards_super`, `deltablue`, `typing_runtime_protocols`,
  **purpose-built ≥4-receiver-shape call site**, `__getattr__`/`__getattribute__`. Exposes class
  identity/version/shape facts vs dict-lookup degrade. PyPy turf.
- **Bignum — ALL Codon-NON-equivalent:** `pidigits`, factorial, big-fib(>F(93)), **RSA/modpow**,
  Karatsuba multiply, **int↔str of huge ints (CVE-2020-10735 — molt must replicate the 4300-digit
  limit + `PYTHONINTMAXSTRDIGITS`)**, Mersenne/Lucas-Lehmer.
- **Deep recursion:** `ackermann` (**molt must replicate `sys.setrecursionlimit`=1000 +
  `RecursionError` + C-stack guard**), `tak`/`takfp`, recursive fib/tree-walk, recursive-descent
  parsers. **`ackermann` with raised limit** — resumable-frame ownership; PyPy-favorable.
- **Generators/coroutines:** `generators`/`coroutines`/`async_generators`/`async_tree`, itertools
  chains, **`yield from` delegation chains** (generator-fusion eligibility vs full resume cost).
- **Exceptions:** try/except in hot loops (**must match CPython 3.11+ zero-cost-on-no-throw**),
  **StopIteration as control flow** (+PEP 479), deep propagation, **raise/catch churn**.
- **String/text/regex — Unicode is Codon-NON-equivalent:** **O(n²) `+=` concat vs `''.join`**
  (molt must match CPython's in-place `+=` str opt), `%`/`.format`/f-string, **unicode
  normalization** (Codon ASCII), `regex_v8`/`effbot`/`dna`/redux, **catastrophic backtracking**
  (`(a+)+$`), encode/decode.
- **Numeric edge — Codon-NON-equivalent on negatives:** the FP kernels (Codon's strongest
  *equivalent* turf), **overflow** (Codon wraps, Python promotes), **negative modulo/floor-div**
  (Python floors toward −∞, Codon truncates toward 0 — never a Codon win), divide-by-zero.
- **Startup/import — molt is AOT (footprint/page-in, not runtime-init):** `python_startup`/
  `_no_site`/`stdlib_startup`. Per the council ruling cold-start is artifact-footprint/codesign
  (runtime init=0.127ms), **WARN under v0 budget, not an execution red**; report binary size +
  RSS + page-in. **Codon directly comparable (both AOT) — cleanest cold-start/binary-size
  bake-off.** **`stdlib_startup`** = does molt's tree-shaking drop unused stdlib?
- **Memory/data-structure:** **megamorphic dict**, list/comprehension, set, deque/heapq, `btree`,
  **hash-collision attack** (CVE-2012-1150 — molt must match `PYTHONHASHSEED` randomization).

**Real-world application + ML-inference:**
- **ETL/data pipelines:** H2O db-benchmark + TPC-H (C/Rust engines — flag), CSV/JSON parsing
  (string-heavy — Codon ASCII flag), log/word-count-at-scale (PyPy-favorable). **pure-Python
  groupby/join** is most discriminating.
- **ML inference — molt's tinygrad + DFlash path** (separate **Python orchestration** time from
  **tensor-kernel (GPU/BLAS)** time — the Pyston PyTorch precedent):
  - **tinygrad** (`github.com/tinygrad/tinygrad`, **MIT** — the public ML contract,
    exact-semantics-no-drift per CLAUDE.md). Fidelity matrix: `beautiful_mnist.py` →
    `test_efficientnet.py`/`train_resnet.py` → `gpt2.py` → `llama*.py`/`mixtral.py` →
    `stable_diffusion.py`/`sdxl.py` → `whisper.py`/`yolov8.py`; model tests; `examples/mlperf/`.
    **The real compiler test = tinygrad's pure-Python scheduler/kernel-fusion graph build**, not
    raw matmul.
  - **DFlash** — **"Block Diffusion for Flash Speculative Decoding"** (Z-Lab, arXiv 2602.06036;
    **NOT generic speculative decoding**). Per CLAUDE.md: drafter = a **shallow diffusion LLM**
    predicting a whole token block in one parallel non-causal forward pass, reusing the target's
    embedding+LM-head, **only intermediate layers trained** (a *real trained drafter*);
    **hidden-feature conditioning + KV injection** from uniformly-sampled target layers into
    *every* drafter layer's K/V (vs EAGLE-3 first-layer-only); verifier = full target (lossless).
    Benchmarks GSM8K/MATH-500/AIME/HumanEval/MBPP/LiveCodeBench/SWE-Bench/MT-Bench/Alpaca;
    metrics **speedup(×) + acceptance length(τ)**. **molt flag: if a model lacks a real trained
    DFlash drafter, say so.** **The discriminating molt metric = the Python draft/verify
    orchestration-loop cost**, distinct from kernel throughput.
  - **MLPerf Inference** (`github.com/mlcommons/inference`, Apache-2.0): ResNet-50/RetinaNet/
    3D-UNet/BERT/GPT-J/Llama-2-70B/SDXL/DLRM/Mixtral. **The LoadGen is the Python-facing
    component** — measure orchestration overhead there (kernel time dominates → flag).
  - Pyston `pytorch_alexnet_inference` (C-ext-bound; the orchestration-vs-kernel demonstrator).
- **Scientific pipelines (Codon's home turf — the AOT bake-off):** **Smith-Waterman** (Codon/Seq
  flagship — order-of-magnitude over hand-C; ASCII sidesteps Unicode), **BWA-MEM/FM-index**
  (pointer-chasing+bit-twiddling), k-mer counting (dict-heavy), **GenomicsBench**. **Smith-Waterman
  = the AOT-vs-AOT bake-off molt must approach/exceed on matched typed-numeric semantics.**
- **Interpreter/compiler workloads:** **mypy**(partly C-ext, pure path clean), **sqlglot**
  (pyperformance `sqlglot_v2` — excellent molt target: pure-Python recursive-descent + megamorphic
  AST + deep recursion + ASCII SQL → Codon-plausible), **pycparser**(PyPy), **pylint**(Pyston),
  `2to3`/`docutils`/`sphinx`/`sympy`(sympy bignum → Codon non-equivalent), **PyPy `translate`**
  (ultimate compiler-on-compiler scale; molt can't run RPython). **sqlglot + mypy** = punish
  dispatch+recursion+allocation simultaneously.

## A.10 — Cross-suite dedup map (register each canonical benchmark ONCE)

| Canonical id | CLBG | pyperf | PyPy | Codon | Origin |
|---|:--:|:--:|:--:|:--:|---|
| `nbody` | ✓ | ✓ | ✓ | ✓ | CLBG |
| `spectral_norm` | ✓ | ✓ | ✓ | ✓ | CLBG |
| `fannkuch` | ✓ | ✓ | ✓ | ✓ | CLBG |
| `mandelbrot` | ✓ | (NPBench/Codon) | — | ✓ | CLBG |
| `pidigits` ⚠bignum | ✓ | ✓ | ✓ | (non-equiv) | CLBG |
| `binary_trees` | ✓ | (`barnes_hut`) | — | ✓ | CLBG(Boehm) |
| `meteor_contest` | ✓(hist) | ✓ | ✓ | — | CLBG |
| `richards`/`_super` | — | ✓ | ✓ | (adjacent) | Unladen/Smalltalk |
| `deltablue` | — | ✓ | ✓ | — | V8/Smalltalk |
| `chaos` | — | ✓ | ✓ | ✓ | Unladen |
| `go` | — | ✓ | ✓ | ✓ | Unladen |
| `raytrace` | — | ✓ | ✓ | — | Unladen |
| `pyflate` | — | ✓ | ✓ | — | Unladen |
| `crypto_pyaes` | — | ✓ | ✓ | — | Unladen |
| `hexiom` | — | ✓ | ✓ | — | Unladen |
| `nqueens` | — | ✓ | ✓ | — | Unladen |
| `float` | — | ✓ | ✓ | ✓ | Unladen(Factor) |
| `telco` | — | ✓ | ✓ | — | Unladen(decimal) |
| `mako`/`chameleon`/`genshi`/`django_template` | — | ✓ | ✓ | — | template |
| `html5lib` | — | ✓ | ✓ | — | parser |
| `sympy_*` ⚠bignum | — | ✓ | ✓ | (non-equiv) | symbolic |
| `sqlalchemy_*` | — | ✓ | ✓ | — | ORM |
| `scimark_*` | — | ✓ | ✓ | — | numeric |
| `regex_dna`/`regex-redux` | ✓ | ✓ | — | — | CLBG |
| `word_count`/`set_partition`/`primes`/`sum`/`taq` | — | — | — | ✓ | Codon |
| `spambayes`/`spitfire`/`twisted_*`/`translate` | — | — | ✓ | — | **PyPy-only** |
| Pyston web triad + tooling | — | — | — | — | **Pyston-only** |

**Dedup rule:** a `richards`/`nbody`/`spectral_norm` win counts ONCE across the CPython + PyPy +
Codon boards (tagged with all sources), never triple-counted. PyPy-only (`translate`,
`twisted_*`, `spitfire`) and Pyston-only (web macros) form clean non-overlapping axes.

## A.11 — Pinned `repo_ref`s + acquisition (source custody, doc 69 §2)

| Suite | Acquire | Pin | License (vendor?) | Adapter |
|---|---|---|---|---|
| pyperformance | `pip install pyperformance` / git | **`1.14.0`** | MIT (yes) | `S1` |
| pyperf | `pip install pyperf` | latest stable | MIT (yes) | (S1 engine) |
| CLBG | `git salsa.debian.org/benchmarksgame-team/benchmarksgame` | pin commit | **Revised BSD** (yes, name-clause) | `S4` |
| PyPy suite | `hg foss.heptapod.net/pypy/benchmarks` | pin commit | mixed (per-benchmark only) | `S2` |
| Pyston macro | `git github.com/pyston/python-macrobenchmarks` | pin commit | MIT (yes) | `S5` |
| Codon `bench/` | install.sh + `git github.com/exaloop/codon` | pin **current Apache commit** | **Apache-2.0** (yes — not BUSL on current) | `S3` |
| NPBench | `git github.com/spcl/npbench` | pin commit | BSD-3 (yes) | sci lane |
| numba-benchmark | `git github.com/numba/numba-benchmark` | pin commit | BSD-2 (yes) | Tier-A lane |
| TFB | `git github.com/TechEmpower/FrameworkBenchmarks` | **Round 23** (archived) | BSD-3 (yes) | `S5` |
| orjson corpus | `git github.com/ijl/orjson` (`bench/data`) | pin commit | Apache/MPL/MIT (yes) | JSON payloads |
| uvloop harness | `git github.com/MagicStack/uvloop` (`examples/bench`) | pin commit | MIT/Apache (yes) | async harness |
| tinygrad | `git github.com/tinygrad/tinygrad` | pin commit | MIT (yes) | ML lane (existing adapter) |
| MLPerf Inference | `git github.com/mlcommons/inference` | pin commit | Apache-2.0 (yes) | ML lane |
| CPython regrtest | CPython `Lib/test` via `libregrtest` | per Python version | PSF (yes) | `S6` |

**Two corrections to common assumptions:** the **Codon paper is CC 2023, not CGO 2023** (DOI
10.1145/3578360.3580275), and **Codon is now Apache-2.0, not BUSL** (relicensed 2025 — vendorable
from a current checkout). **Two honestly-bounded verification gaps:** the PyPy `own/` literal
file tree (Heptapod bot-walled — names via speed.pypy.org + lineage; a real `hg clone` confirms
exact filenames), and Codon paper Fig 5 per-bar speedups (unlabeled bar chart — exact numbers in
Appendix B, not the public 12-page text; the DSL numbers ARE exact).
