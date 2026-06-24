<!-- Foundation blueprint 58 — Arc: KILLER DEMOS that make a Python dev switch.
Author: portfolio-architect. Date: 2026-06-23. Status: design only / executable plan.
Read-only investigation + single Write; the lead integrates (no git add / no build here).
This doc COMPOSES with: 51 (ten-year roadmap / compression ladder), 52 (autonomous
operating charter), 00 (integrated parallel program), 16 (CPython surface / stdlib / GPU
gap audit), 33 (threading & parallelism ladder), 08 + dx_baseline (DX / build-speed),
20/27/45 (RC ownership / Perceus / exception-region), 07 (D1 generator fusion). The sibling
perf/ML/throughput/DX arcs the prompt cites as 51/53/54/56 are, at authoring time, present
only as: 51 (the roadmap, the perf NORTH STAR) and the live capability surface; arcs 53
(ML), 54 (throughput), 56 (DX) are NOT yet written as standalone docs — this plan therefore
binds to the ROADMAP facts (51 §5/§6), doc 33 (the throughput substrate), and docs 08 +
dx_baseline (the DX substrate), and names the exact facts each demo consumes so it slots in
cleanly when those sibling arcs land. -->

# 58 — Killer Demos: the portfolio of undeniable showcases

Status: NORTH-STAR DEMO PROGRAM (2026-06-23). Design only / executable plan.

> "The verifier is the product; the agent is the commodity." (doc 52 §intro) — for a
> *demo*, the analogue is: **the reproducible evidence artifact is the product; the prose is
> the commodity.** A demo that cannot be re-run by a stranger with one command, producing the
> same headline number, is marketing, not engineering. Every demo in this portfolio ships as
> a *runnable evidence harness* under `tools/`/`examples/`, gated in CI, with its headline
> number emitted as machine-readable JSON — never a screenshot, never a hand-typed figure.

---

## 0. The END-STATE outcome (stated crisply)

**A Python developer who has never heard of molt watches five demos and cannot construct a
reason to stay on CPython.** Concretely, the five-to-hundred-year end state:

1. **`tinygrad` ML inference demo**: a real model (GPT-2 / Llama-class) runs token-generation
   on GPU through molt's tinygrad-conformant stack, with **DFlash speculative decoding** giving
   a *measured* end-to-end tokens/sec multiple over the same model on CPython+tinygrad, on the
   same hardware — with the speculative output proven **byte-identical** to non-speculative
   greedy decode (lossless). The wow: "your `import tinygrad` code, unchanged, faster, and the
   speculative path is provably the same tokens."

2. **Python-in-the-browser demo**: a *real* interactive app (not a hello-world) compiled to
   WASM by molt, loading in **< 1 s cold**, binary **< 2 MB gzipped**, running at **>1.00×
   CPython** on the in-app hot loop — deployed live to Cloudflare Workers / static edge. The
   wow: "this is Python, in your browser tab, with no 10 MB Pyodide download and no
   interpreter — it's compiled."

3. **Head-to-head perf demo**: a one-command scoreboard that builds the curated suite + a set
   of *real* workloads (not just microbenchmarks) and renders molt vs CPython vs PyPy vs Codon
   as a live table, **every molt cell ≥ 1.00× CPython** (the contract floor), with the cold/warm/
   RSS/binary-size/compile-time columns the Performance Constitution mandates. The wow: "every
   single row is green, on native AND WASM AND LLVM, and here's the JSON so you can re-run it."

4. **GIL-free parallelism demo**: a shared-memory data-parallel workload (the canonical case
   doc 33 §1.1 names — "a 10 GB array two threads both read") that scales **near-linearly with
   cores** under `molt build --unleashed`, against a CPython run that flat-lines at 1× (GIL),
   while the **DEFAULT-tier** build of the same program is proven byte-identical to CPython. The
   wow: "same code; opt into a flag; N× throughput; and the default build is still exactly
   CPython."

5. **DX demo**: a screen-recording-grade live loop where an edit to a Python file rebuilds and
   re-runs in **sub-second** incremental time, and **N agents** (the doc 52 §Resources model)
   develop in parallel worktrees with **zero cross-session build interference**. The wow: "this
   compiles, and it *still* iterates faster than your interpreter's edit-run loop."

Each demo is **a forcing function for a structural fact**, not a one-off script. A demo that
"works once on the author's laptop" is rejected by the same anti-Goodhart rails as a
cherry-picked benchmark (doc 52 §A.4). The portfolio's job is to make the roadmap's contract
(doc 51 §0) *visible and irrefutable*.

### 0.1 What this arc is NOT

- **NOT new compiler capability.** This arc builds *evidence harnesses and showcase apps on
  top of* the capabilities arcs 51/33/08/16/07/20/27 deliver. Where a demo needs a capability
  that does not yet exist, this plan **names the exact blocking fact and the owning arc** and
  gates the demo on it (§7 risk table) — it never fakes the capability (the DFlash-fidelity and
  zero-workaround mandates, CLAUDE.md). A faked demo is worse than no demo: it is a lie in the
  one artifact whose entire purpose is to be trusted.
- **NOT a replacement for the scoreboards.** Demo 3 *drives* the existing four+1 scoreboards
  (doc 51 §3, `tools/perf_scoreboard.py`); it does not invent a parallel perf authority.
- **NOT marketing copy.** No screenshots-as-evidence; every headline number is a CI-gated JSON
  field with a reproducing command (doc 52 honesty protocol).

---

## 1. The method (time-traveler: end-state → the facts that make it inevitable)

Working backward from "a Python dev cannot construct a reason to stay on CPython", the demos
are *consumers* of the semantic fact plane (doc 51 §1). The compression-ladder framing applies:
**each demo retires a CLASS of "molt can't be shown to do X" objections by adding a first-class,
re-runnable EVIDENCE FACT**, not a one-off script. The evidence facts this arc introduces:

| Evidence fact (new) | Lives in | Retires the objection class |
|---|---|---|
| **DemoManifest** (`tools/demos/manifest.toml`) — every demo: id, headline metric, exact build+run commands, expected JSON shape, CI gate, owning capability arc, blocking-facts list | new `tools/demos/` | "the demo isn't reproducible / I can't re-run it" — the whole class of unfalsifiable showcases |
| **EvidenceCapsule** (per-demo JSON: metric, value, units, oracle-equivalence proof, target×backend×profile, cold/warm/RSS/size/compile, host fingerprint, command, git SHA) | `bench/results/demos/<demo>/<date>.json` | "the number is cherry-picked / warm-only / on a different machine" |
| **OracleEquivalence** (a demo whose output has a CPython oracle proves byte-identity; a lossless-accelerated demo, e.g. DFlash, proves output == the unaccelerated path) | demo harness + `tests/differential/demos/` | "it's fast because it's wrong / it cut a corner" — the silent-divergence class (doc 52 §A.1) |
| **DemoGate** (CI job: each enabled demo's headline metric must hold ≥ its registered floor, equivalence must pass, or the gate is RED) | CI + `tools/demos/run_demo.py --gate` | "the demo rotted three months ago and nobody noticed" — the bit-rot class |

The thesis, stated once: **a demo is a benchmark with a narrative and an oracle.** It obeys the
same measurement discipline (pyperf, cold+warm, repeated, quiescent, classified GREEN/RED_STABLE/
RED_NOISY/TIE/DIMENSIONAL_WIN — doc 52 §A.4 / CLAUDE.md Council Doctrine) PLUS a correctness
oracle PLUS a story. The narrative is for humans; the oracle + the EvidenceCapsule are for the
gate. This is the structural reason the portfolio cannot drift into marketing.

### 1.1 The leverage ordering (why this order)

Demos are ordered by **(proof-strength × wow) / (capability-gap-to-close)** — i.e. by how much
undeniable switching-pressure each delivers per unit of still-missing capability:

1. **Demo 3 (head-to-head perf)** — FIRST. It is almost entirely *harness*, sits on the already-
   built `tools/perf_scoreboard.py` + `tools/bench.py`, and produces the single most universal
   switching argument ("it's faster, here's every row green"). Lowest capability gap, immediate.
2. **Demo 5 (DX)** — SECOND. Largely harness over docs 08 + dx_baseline; the sub-second-loop and
   N-agent story is independently shippable and is the argument that *removes the #1 objection to
   compilers* ("but the edit-run loop is slow"). Low capability gap.
3. **Demo 2 (Python-in-browser)** — THIRD. The cloudflare-demo + microgpt scaffolding already
   exists (`examples/cloudflare-demo`, `examples/microgpt`, the `deploy` skill); the gap is
   "make it a *real* app, < 1 s / < 2 MB, with an in-browser hot-loop perf cell." Medium gap.
4. **Demo 1 (tinygrad + DFlash ML inference)** — FOURTH. The richest wow and the deepest moat,
   but the largest capability gap: real GPU codegen on a real device, a *trained* DFlash drafter
   (today: contract-only, no trained drafter shipped — doc 16 §3.3), and the e2e tokens/sec
   oracle. Gated, staged, fidelity-blocking.
5. **Demo 4 (GIL-free parallelism)** — FIFTH on the *full* form, because the UNLEASHED tier (doc
   33 Layer C) is design-only and gated on biased-RC + the §3.5 memory model. BUT its **Layer-B
   form** (isolates / message-passing, the AOT re-exec "superpower" doc 33 §1.1) is far closer
   and is scheduled earlier as Demo 4a (§5.4).

Cross-arc dependency note (binding): **Demo 1 depends on the ML stack (doc 16 §3 + GPU codegen,
roadmap Y2-3); Demo 3's PyPy/Codon columns depend on the reference-harness arc (doc 52 §C.1
"crater-equivalent"); Demo 4 depends on the throughput ladder (doc 33); Demo 2 and Demo 5 depend
on the size/cold-start arc (W3 / RuntimeSurfacePlan, doc 00 §4.3) and DX arc (doc 08).** Demos do
not unblock capability arcs; they *consume and prove* them. A demo's "build path" below is the
*demo-side* work; its "blocking facts" are the capability-side work owned elsewhere.

---

## 2. Current capability surface (verified against the tree, 2026-06-23)

Anchored to live files (code beats docs — doc 52 RECON rule). This is what the demos build ON.

**Perf / scoreboards (Demo 3 substrate) — STRONG:**
- `tools/perf_scoreboard.py` — the CPython-floor scoreboard, exactly the Performance-Constitution
  operationalization (header verified): `speedup = cpython_time / molt_time`, any cell < 1.00 RED,
  cold+warm, ≥5 samples, median+stdev+CoV instability flag, keyed `benchmark × target × backend ×
  profile`, runs every binary through `tools/safe_run.py --json` (RSS+timeout), `native + llvm`
  today (WASM `run-blocked`, Luau in `tools/benchmark_luau_vs_cpython.py`). **PyPy/Codon columns
  are present-but-nullable; neither installed on host** (header §"PyPy / Codon columns").
- `tools/bench.py`, `tools/bench_suites.py` (curated suite), `tools/bench_dashboard.py`,
  `tools/bench_friends.py`, `tools/bench_reference.py` (reference-lane planner, reads
  `bench/results/reference_manifest.json`), `tools/cold_start_decompose.py`,
  `tools/binary_size_analysis.py`, `tools/output_startup_size_audit.py`, `tools/wasm_size_audit.py`.
- `bench/friends/manifest.toml` — codon + pypy suites scaffolded but `enabled = false`
  (repo_ref `PINNED_COMMIT_REQUIRED`, runner cmds unconfigured, `semantic_mode =
  requires_adapter`). `bench/friends/repos/tinygrad_off_the_shelf` present.
- `bench/scoreboard/` exists; `tests/tools/test_perf_scoreboard.py` gates the tool.

**WASM / browser (Demo 2 substrate) — MEDIUM:**
- `examples/cloudflare-demo/` — `src/app.py` + `wrangler.jsonc`; the `deploy` skill +
  `.agents/skills/source-command-deploy/SKILL.md` give the exact build line:
  `molt build … --target wasm --stdlib-profile micro --output …wasm --linked-output …linked.wasm`
  then `wasm-opt -Oz …` then size report (gzip) then `wrangler deploy`. URL path → `sys.argv[1]`,
  query → `QUERY_STRING`, `print()` → HTTP body, VFS `/bundle` + `/tmp`.
- `examples/microgpt/` — a **pure-Python 1-layer GPT** (4-head attn, 16-dim, ~4192 params,
  `list[float]` kernels, KV cache) with `inference.py` + `inference_wasm.py` (weights embedded as
  CSV, zero I/O). This is *already* a browser-grade ML-ish app skeleton. `examples/edgebox/` is a
  larger edge box (Cloudflare Worker + Python boxes).
- `drivers/wasm`, `drivers/browser/wasm_cpu`, `runtime/molt-wasm-host` exist. WASM run-path has a
  known socket-import instantiation gap (scoreboard header) and asyncio-wasm has 4 blockers (doc
  18 / doc 16 §1.4 rank 4).
- Size/cold-start targets: ROADMAP "ratcheting toward < 50 ms cold start and < 2 MB" (ROADMAP
  §13-16); `empty.py` floor ~4.31 MB today (doc 00 §4.3), W3 per-attr DCE (doc 09/13) expected
  650 KB–1.1 MB reduction; RuntimeSurfacePlan is the per-intrinsic reachability lever.

**ML / GPU / DFlash (Demo 1 substrate) — MEDIUM, fidelity-gated:**
- `runtime/molt-gpu/` (Rust): LazyOp DAG, scheduler, fusion, ShapeTracker, **26 primitives × 8
  backends green** (CPU/WASM-CPU/Metal/WebGPU/WebGL2/CUDA/HIP/OpenCL + experimental ANE), all
  layers green under test (doc 16 §3.1). **CPU executor is the reference; real-device GPU codegen
  exists per-backend but the demo-grade e2e device path is the gap.**
- `src/molt/gpu/` (Python): `tensor.py` (107 KB, 80+ Tensor methods incl. matmul, attention,
  conv2d, KV-cache ops, 4-bit TurboQuant), `nn.py`, `kv_cache.py` (tiered H2O + DDTree
  block-diagonal), `generate.py` (greedy / top-k / top-p / lossless block-speculative),
  `dflash/` (contracts/adapters/runtime). `src/tinygrad/` shim re-exports `molt.gpu.Tensor` for
  `import tinygrad` / `from tinygrad import Tensor` with exact-case import custody.
- **DFlash (doc 16 §3.3): contract paper-faithful (target-conditioned drafting, verifier/drafter
  separation, hidden-feature conditioning, KV injection, position IDs, last-verified-token
  type-checked, fail-closed adapter resolution — all VERIFIED present), but NO trained drafter
  shipped** (contract only; "drafter models require external training"). `contracts.py` cites
  arXiv:2602.06036 + z-lab.ai/projects/dflash. **This is the hard fidelity gate for Demo 1's
  speculative-speedup claim** (CLAUDE.md tinygrad+DFlash mandate).
- Autograd not implemented (inference-only); `bench/friends/repos/tinygrad_off_the_shelf` is the
  real-tinygrad compile lane (ROADMAP §13/§17 burn-down in progress: upstream `upat_compile`
  static-exec registry `tools/tinygrad_upat_static_exec_registry.py`).
- `examples/gpu_*.py` (vector_add, mnist_inference, transformer, dataframe, distributed,
  rapids_style) — exploration examples on `molt.gpu`.

**Throughput / parallelism (Demo 4 substrate) — DESIGN-ONLY (doc 33):**
- Doc 33 = the throughput north star: **two-tier model** (DEFAULT = byte-identical CPython-GIL;
  UNLEASHED = opt-in free-threading) + **three layers** (A: per-interpreter GIL; B: N isolates
  message-passing = the AOT re-exec "superpower", re-exec ~4 ms; C: in-interpreter free-threading
  on biased RC + per-object locks + the §3.5 memory model). Layer A authoritative impl
  `runtime/molt-runtime/src/concurrency/gil.rs` (`PREINIT_GIL`, single-thread fastpath). **Layer C
  is design-only; the nogil tax is repaid by Perceus borrow inference (doc 27), itself
  design-only.** `runtime/molt-runtime/src/concurrency/locks.rs` (std Mutex+Condvar) is dirty in
  the working tree.
- `threading.py` partial; `multiprocessing` fork/forkserver → spawn, Queue semantics divergent
  (doc 16 §1.4 rank 9); `concurrent.futures` thread pools OK, process pools pending.

**DX (Demo 5 substrate) — MEDIUM, measured:**
- doc 08 + dx_baseline (MEASURED on M5 Max): cold full daemon build **196 s**; incremental after
  touching one molt-backend file **~123–167 s** (the long pole is the daemon-bin **fat LTO**,
  146.8 s); **molt-runtime edits are a 0.10 s no-op for the daemon** (decoupled). The Phase-1
  lever is `release-fast` **fat→thin LTO** (measured **137 s → 81 s, ~41%**, dx_baseline §A/B).
  Phase-2 "split function_compiler.rs" gives ~0 incremental benefit alone because
  `compile_func_inner` is ONE ~34K-line function = ONE codegen unit (dx_baseline §3); the real
  lever is L3 extract `molt-backend-native` crate. **sccache not installed on host.**
- **Crucially for the DX *demo*: the sub-second loop the END-STATE promises is the
  Python-edit→rerun loop (frontend + cached backend), NOT the Rust daemon rebuild.** The daemon
  is the compiler; a user editing `.py` hits frontend lowering + the cached/daemon backend, which
  is the loop to instrument. The N-agent story is `MOLT_SESSION_ID` worktree isolation (CLAUDE.md
  Concurrent Development; doc 52 §Resources ≤3 agents, ≤2 build-triggering, non-overlapping lanes).

---

## 3. Cross-cutting infrastructure (built once, used by all five demos)

This is the §1 evidence-fact plane, made concrete. **It is the first thing built (Phase D0)** so
every later demo lands as data + a gate, not a one-off.

### 3.1 `tools/demos/` package (the demo institution)

```
tools/demos/
  manifest.toml          # DemoManifest: the registry of all demos
  run_demo.py            # build + run + capture + (optional) --gate one demo
  capsule.py             # EvidenceCapsule schema + writer/validator (schema_version=1)
  oracle.py              # OracleEquivalence helpers (CPython byte-identity; lossless==unaccel)
  render.py              # render a DemoManifest run into a human table + a shareable HTML/MD card
  __init__.py
bench/results/demos/<demo_id>/<UTC-date>.json   # the capsules (durable, git-tracked summaries)
tests/tools/test_demo_harness.py                # gates the harness itself
tests/differential/demos/                       # oracle-equivalence differential anchors
```

`DemoManifest` per-entry fields (TOML): `id`, `display_name`, `headline_metric` (e.g.
`tokens_per_sec_speedup_vs_cpython`), `headline_floor` (the CI-red threshold, e.g. `1.00`),
`build_cmd[]`, `run_cmd[]`, `oracle` (`cpython_byte_identical` | `lossless_vs_unaccelerated` |
`none_pure_perf`), `targets[]`, `backends[]`, `profiles[]`, `owning_capability_arc` (e.g.
`doc16-gpu`, `doc33-throughput`, `doc08-dx`), `blocking_facts[]` (named capability gaps that keep
it `enabled=false`), `enabled` (bool — like `bench/friends/manifest.toml`, demos stay disabled
until reproducible).

`EvidenceCapsule` JSON (schema_version=1), the universal demo output — one record carries
*everything the Performance Constitution methodology line mandates* plus the oracle:
```
{ "schema_version":1, "demo_id":..., "git_sha":..., "utc":...,
  "host": {"os":..., "cpu":..., "ram_gb":..., "gpu":...},        # fingerprint
  "metric": {"name":..., "value":..., "units":..., "floor":..., "classification":
             "GREEN|RED_STABLE|RED_NOISY|TIE|DIMENSIONAL_WIN"},
  "matrix": {"target":..., "backend":..., "profile":...},
  "perf": {"cpython_ratio":..., "pypy_ratio":null, "codon_ratio":null,
           "cold_s":..., "warm_s":..., "peak_rss_mib":..., "binary_bytes":...,
           "binary_gzip_bytes":..., "compile_s":..., "samples":..., "cov":...},
  "oracle": {"kind":..., "passed":true, "detail":...},           # equivalence proof
  "command": "…exact reproducing command…", "log_artifact":"…path…" }
```

**Reuse, never duplicate:** `run_demo.py` builds via `tools/bench.py`'s daemon-batch path,
times every run via `tools/safe_run.py --json` (RSS+timeout, mandated), and for Demo 3 *is a thin
caller of `tools/perf_scoreboard.py`* — the capsule's perf block is filled from the scoreboard's
own output. No second perf authority is created (doc 49 single-authority rule; doc 51 §1 "no
second authority for any fact").

### 3.2 `DemoGate` (CI)

One CI job iterates `manifest.toml` over `enabled=true` demos, runs `run_demo.py --gate`, and is
RED if any headline metric < floor OR any oracle fails OR a `RED_NOISY` classification cannot be
resolved to a stable measurement. This is the bit-rot retirement: a demo that regresses fails CI
exactly like a scoreboard cell (doc 51 §3, doc 52 §A.2 ratchets). Cost discipline: demos run in
the **fast/subsampled tier** for PRs (doc 52 §C.3 economic sustainability — e.g. 3 samples,
single profile) and the **full matrix nightly**.

### 3.3 The shareable artifact (`render.py`)

Renders a manifest run into (a) a terminal table and (b) a self-contained HTML/Markdown "demo
card" (the thing a human pastes into a README / tweet / slide) whose every number is sourced from
a capsule and stamped with the git SHA + host fingerprint + reproducing command. **The card is
generated, never hand-edited** — this is the structural defense against marketing drift, the same
way generated op_kinds tables defend against pass-local fact drift (doc 51 §1).

---

## 4. Phasing overview (dependency order; each phase independently landable + green)

| Phase | Deliverable | Depends on | Gate (pre-registered, doc 52 §B loop step 2) |
|---|---|---|---|
| **D0** | `tools/demos/` harness + manifest + capsule + gate + tests (§3) | nothing (pure infra) | `tests/tools/test_demo_harness.py` green; capsule schema round-trips; `--gate` exits nonzero on a synthetic RED |
| **D1** | **Demo 3** head-to-head perf card (native+LLVM, CPython floor; +WASM cell when run-path lands; PyPy/Codon when installed) | D0; `perf_scoreboard.py` (built) | every molt cell ≥1.00× CPython on the curated suite; capsule per cell; card renders; CI fast-tier green |
| **D2** | **Demo 5** DX: sub-second Python-edit→rerun loop timer + N-agent parallel-build evidence | D0; docs 08/dx_baseline (thin-LTO + crate split land in doc-08 arc) | edit→rerun median < target on a fixed `.py`; N=3 parallel sessions complete with zero shared-target collision (assertion); capsule |
| **D3** | **Demo 2** Python-in-browser: a real app, <1 s cold / <2 MB gzip, in-app hot-loop ≥1.00× CPython, live on Workers | D0; WASM run-path (scoreboard gap), W3/RuntimeSurfacePlan (size), microgpt/cloudflare scaffold | app loads in browser; size + cold + hot-loop capsule; oracle = CPython byte-identical on the app's pure compute |
| **D4a** | **Demo 4a** isolates/throughput: N-isolate message-passing scales ~linearly (Layer B, the AOT re-exec superpower) | D0; doc 33 Layer B (isolates) | throughput vs cores curve; DEFAULT-tier byte-identical oracle; capsule |
| **D5** | **Demo 1** tinygrad+DFlash ML inference: e2e tokens/sec multiple, lossless oracle | D0; GPU device codegen (doc 16 §3, roadmap Y2-3); **a trained DFlash drafter** (doc 16 §3.3 gate) | tokens/sec speedup capsule; **DFlash output == greedy output byte-identical**; tinygrad API unchanged |
| **D4b** | **Demo 4** GIL-free (full): Layer-C free-threading shared-memory near-linear scaling | D0; doc 33 Layer C + biased RC + Perceus (doc 27) — all design-only today | scaling curve; §3.5 memory-model conformance; DEFAULT build byte-identical |

D0→D1→D2 are landable now (low capability gap). D3 lands as capabilities (WASM run-path, size)
arrive. D4a lands with doc 33 Layer B. D5 and D4b are the deepest, gated on the ML and
free-threading capability arcs respectively; this plan specifies them fully so they slot in
without redesign, but **does not pretend their capability gates are closed** (the honesty
protocol — a demo gated on a missing trained drafter is reported as gated, never faked).

---

## 5. Demo specifications (each: what it proves · structural capability · wow · build path · gate · risk)

### 5.1 Demo 3 — Head-to-head perf (FIRST; lowest gap, highest universality)

**What it proves.** molt is faster than CPython on *every* row of a curated + real-workload
suite, across native/LLVM (and WASM/Luau as run-paths land), with the full cold/warm/RSS/size/
compile columns — and approaches/relates honestly to PyPy and Codon where their models apply.
This is the Performance Constitution made *visible* (doc 51 §0, CLAUDE.md).

**Structural capability it requires (and the owning arc).**
- The CPython-floor scoreboard: **EXISTS** (`tools/perf_scoreboard.py`).
- Real-workload rows beyond microbenchmarks: a small curated set of *recognizable* programs
  (e.g. a JSON-heavy ETL, a text/regex pipeline, an N-body / numeric kernel, a small interpreter
  loop) added to `tools/bench_suites.py`. These exercise the very facts the roadmap is retiring
  (ShapeFacts → etl_orders 0.60×, exception-region → exception_heavy 0.68× — doc 51 §5
  warm-reds), so the demo doubles as a forcing function: **a real-workload row that is RED is a
  roadmap task, surfaced** (doc 51 §perf-triage).
- PyPy/Codon columns: depend on the **reference-harness arc** (doc 52 §C.1 "crater-equivalent";
  `bench/friends/manifest.toml` codon/pypy suites + `tools/bench_reference.py`). **Blocking fact:
  PyPy/Codon not installed on host + suites `enabled=false` + `repo_ref PINNED_COMMIT_REQUIRED`.**
  The capsule schema already carries `pypy_ratio`/`codon_ratio` as nullable — they render as "—"
  until the harness lands, never as a fabricated number.

**Wow.** "Open the card. Every molt cell is green vs CPython — native and LLVM. Here's cold AND
warm, RSS, binary size, compile time. Re-run it yourself: one command, JSON out." The
universality is the wow: not one benchmark, the *whole board*.

**Build path (demo-side).**
1. (D0 done.) Register `perf_headtohead` in `manifest.toml`: `headline_metric =
   min_cpython_ratio_over_suite`, `headline_floor = 1.00`, `oracle = cpython_byte_identical`
   (the scoreboard already diffs stdout via the differential path), `owning_capability_arc =
   doc51-perf`.
2. Add 4–6 real-workload programs to `tools/bench_suites.py` (curated, deterministic, seeded);
   each gets a CPython oracle (byte-identical stdout) so a "win" can never be a wrong answer.
3. `run_demo.py perf_headtohead` calls `tools/perf_scoreboard.py` for native+LLVM, captures one
   capsule per `(benchmark × backend × profile)` cell, classifies each (GREEN/RED_STABLE/…),
   and fails the gate on any RED_STABLE < 1.00.
4. `render.py` emits the card (table + HTML) with the min-ratio headline and the full matrix.
5. WASM cell: add as `run-blocked` placeholder; flips to live when the WASM run-path lands
   (scoreboard header gap; ties to Demo 2). Luau cell via `tools/benchmark_luau_vs_cpython.py`.

**Pre-registered gate.** `tools/demos/run_demo.py perf_headtohead --gate` exits 0 iff every
native+LLVM cell ≥ 1.00× CPython (quiescent, ≥5 samples, CoV-stable) AND every oracle passes.
Capsules written to `bench/results/demos/perf_headtohead/`. Fast-tier (3 samples, release-fast)
on PR; full matrix (native+LLVM+Luau, release-fast+release-output, cold+warm) nightly.

**Composition.** This demo is the *consumer face* of doc 51 §3 scoreboards; it adds the
real-workload rows and the shareable card but creates **no second perf authority**. It composes
with the decomposition (21a–e) trivially — it only reads built artifacts.

---

### 5.2 Demo 5 — DX: instant incremental loop + N-agent parallel dev (SECOND)

**What it proves.** molt's *developer* loop — edit a `.py`, rebuild, re-run — is **sub-second**,
and **N agents** develop in parallel with zero interference. This removes the single biggest
objection to "compile your Python": "but then my edit-run loop is slow." We show it is *faster
than the interpreter's*, because the backend is cached/daemonized and only the changed Python is
re-lowered.

**Structural capability it requires (and the owning arc).**
- The **Python-edit→rerun** loop time = frontend lowering of the changed module + the cached/
  daemon backend + link + guarded run. The capability lever is **the frontend+backend caches +
  the daemon** (CLAUDE.md Build & Test; doc 00 §4.2 module-phase; the frontend cache lookup in
  the build path). **Owning arc: doc 08 DX-buildspeed + the caching infra.** The demo does NOT
  require the Rust daemon to rebuild sub-second (it doesn't, and shouldn't have to — dx_baseline
  proves the daemon build is a separate, infrequent cost).
- The **N-agent** story = `MOLT_SESSION_ID` per-session `target/sessions/<id>/` isolation
  (CLAUDE.md Concurrent Development) + the doc 52 §Resources model (≤3 agents, ≤2 build-triggering,
  non-overlapping file lanes, agents never push). **This EXISTS** — the demo *measures and proves*
  the isolation (no shared-target lock collision, no artifact clobbering across sessions).
- The daemon-build speed wins (thin LTO 137 s→81 s, crate split) are **doc 08's** deliverable; the
  demo *reports* them as a secondary "compiler self-build" capsule but does not own them.

**Wow.** Split-screen: (left) molt — save `.py`, output updates in well under a second; (right)
the same program under CPython's import+run. Then: three terminals, three `MOLT_SESSION_ID`
worktrees, three builds running concurrently, none stalling the others, all finishing. "It
compiles, and it iterates faster than your interpreter — and three of you can work at once with
zero collisions."

**Build path (demo-side).**
1. `tools/demos/dx_loop_timer.py`: a fixed representative `.py`; loop = {touch a function body →
   `molt run` (guarded) → capture wall-time} × N; report median edit→rerun, cold (first) + warm
   (steady), via `safe_run.py --json`. Capsule `metric = edit_rerun_warm_s`, `floor` = a target
   (e.g. < 1.0 s warm; the exact floor set from the first measured run — doc 52: create the
   measurement path before claiming).
2. `tools/demos/dx_parallel_agents.py`: spawn N (=3) sessions each with a distinct
   `MOLT_SESSION_ID`/`CARGO_TARGET_DIR`, each building+running a small program; assert each used
   its own `target/sessions/<id>/`, no cross-session daemon-socket conflict, all exit 0; capsule
   `metric = parallel_sessions_no_collision` (boolean → must be true) + per-session wall-times.
3. Optional secondary capsule: re-run dx_baseline's thin-vs-fat LTO A/B as a "compiler self-build"
   number (sourced from doc 08's arc, not owned here).
4. `render.py` card: the edit→rerun median, the N-agent isolation proof, the self-build delta.

**Pre-registered gate.** `dx_loop_timer --gate`: warm edit→rerun median ≤ floor (RED_NOISY
re-measured); `dx_parallel_agents --gate`: all N sessions exit 0, distinct target dirs asserted,
zero socket collision. (CI runs N=2 to respect the ≤2-build-triggering rule — CLAUDE.md.)

**Composition.** Directly composes with the **decomposition program (21a–e)** and doc 08: as
`function_compiler.rs`/`molt-backend-native` split (21a / doc 08 L3) lands, the self-build capsule
improves; the demo is the *evidence* that the decomposition delivered its DX promise. It also
composes with the **multi-agent execution model** (doc 52 §Resources) — it is literally a test of
it. **Risk: must respect ≤2 build-triggering agents in CI** (treated structurally: the parallel
demo caps concurrency at 2 in the gate, N=3 only in the human-facing recording).

---

### 5.3 Demo 2 — Python-in-the-browser via WASM (THIRD)

**What it proves.** A *real, interactive* Python app runs in a browser tab, compiled (not
interpreted), loading in **< 1 s cold** at **< 2 MB gzipped**, with its in-app hot loop **≥
1.00× CPython**, deployed live to the edge. This is the "Python everywhere, fast, tiny" claim
(doc 51 §0 "all four footprint dimensions world-class").

**Structural capability it requires (and the owning arc).**
- **WASM run-path**: the scoreboard records WASM as `run-blocked` (socket-import instantiation
  gap); the demo needs a *runnable* WASM artifact in-browser. The cloudflare-demo path already
  *runs* compute WASM on Workers (the `deploy` skill works), so the gap is the general run-path +
  the in-browser harness, not "WASM doesn't run at all." **Owning arc: WASM run-path /
  asyncio-wasm (doc 18) for any async; the deploy skill for the edge path.**
- **Size**: < 2 MB gzip needs **W3 per-attribute DCE (doc 09/13)** + **RuntimeSurfacePlan**
  (per-intrinsic reachability so a tiny app stops linking async/GPU/net it can't reach — doc 00
  §4.3). Today `empty.py` floor ~4.31 MB; W3 expected −650 KB–1.1 MB; `wasm-opt -Oz` + gzip close
  more. **Blocking fact: the < 2 MB gzip target depends on W3 + RuntimeSurfacePlan landing.** The
  demo's first capsule records the *actual* size honestly; the < 2 MB floor is the gate that goes
  green when the size arc lands (not faked).
- **Cold start < 1 s**: artifact-footprint/page-in problem (doc 51 §62, CLAUDE.md "cold-start is
  an artifact-footprint/page-in/codesign problem, NOT a runtime-init problem — runtime init
  0.127 ms"); driven by size + the WASM streaming-instantiate path.
- **The app**: build on `examples/microgpt/inference_wasm.py` (a real char-level GPT, pure
  Python, weights embedded, zero I/O → ideal WASM payload) and/or `examples/cloudflare-demo`. A
  compelling interactive form: a **microGPT text generator in the browser** (type a seed, watch
  it generate) — recognizably "an ML model running as compiled Python in your tab."

**Wow.** "This tab is running a GPT in Python. No Pyodide, no 10 MB download, no interpreter —
it's compiled to a < 2 MB WASM. It loaded in under a second. And the same code runs faster than
CPython." For a Python dev, "my code, in the browser, tiny and fast" is a category they didn't
think they had.

**Build path (demo-side).**
1. Promote `examples/microgpt/inference_wasm.py` to an interactive app: a tiny JS/HTML shell that
   feeds a seed string as `sys.argv[1]` (the deploy skill's existing convention) and renders the
   generated text; optionally a streaming token loop.
2. Build via the **deploy skill's exact line** (`--target wasm --stdlib-profile micro
   --linked-output …`), `wasm-opt -Oz` (the skill's flag set), gzip, capture size.
3. `tools/demos/wasm_app_bench.py`: (a) cold load time in a headless browser (reuse
   `tools/cold_start_decompose.py` / a Playwright-style harness), (b) binary + gzip size (reuse
   `tools/wasm_size_audit.py` / `tools/binary_size_analysis.py`), (c) in-app hot-loop time vs the
   same loop under CPython, (d) **oracle: the generated text == CPython's generated text on the
   same seed (byte-identical)** — the lossless proof that the WASM build didn't cut a corner.
4. Deploy live via `wrangler deploy` (the skill's step 5); record the URL in the card.
5. `render.py` card: size (raw + gzip), cold-load, hot-loop ratio, the live URL, the oracle pass.

**Pre-registered gate.** `wasm_app_bench --gate`: gzip size ≤ 2 MB (the floor that flips green
when W3/RuntimeSurfacePlan land — until then the capsule records actual size and the gate floor
is set to the current best with a tracked owner, never silently passed), cold-load ≤ 1 s, hot-loop
≥ 1.00× CPython, **oracle byte-identical**. WASM-run differential via `tools/wasm_diff.py`
(ROADMAP §wasm leak-loop differential already passes for some cases).

**Composition.** Composes with the size arc (W3, doc 09/13), RuntimeSurfacePlan (doc 00 §4.3),
the asyncio-wasm arc (doc 18) for any interactivity needing async, and reuses the deploy skill.
It is the consumer face of "binary < 2 MB / cold < 50 ms" (the demo's < 1 s cold-load is the
*browser* number; the < 50 ms is the artifact instantiate number — both reported).

---

### 5.4 Demo 4a — Isolates / message-passing throughput (the AOT superpower) (FOURTH)

**What it proves.** molt runs **N true-parallel isolates** (each its own GIL + heap, doc 33
Layer B) communicating by message passing, scaling **~linearly with cores** on an
embarrassingly-parallel workload — while CPython's GIL flat-lines a *threaded* version at 1×.
Crucially this is the **DEFAULT tier** (PEP 734 fidelity), so it needs *no* unleashed/free-
threading capability and the per-isolate execution is byte-identical to CPython. The "superpower"
(doc 33 §1.1): AOT re-exec is ~4 ms, so spinning up isolates is near-free vs CPython's process
fork + interpreter re-init.

**Structural capability it requires (and the owning arc).**
- **Layer B isolates + message passing** (doc 33 Layer B; PEP 734 `interpreters` / channels).
  **Owning arc: doc 33 (throughput ladder).** Today: `concurrency/gil.rs` is per-thread-GIL with
  a single-thread fastpath; the per-interpreter-GIL scoping + isolate spawn + channel transport
  is doc 33's Phase work. **Blocking fact: isolate spawn + cross-isolate channels not yet a
  shipped Layer-B product** (doc 33 is design-only).
- The AOT re-exec path (the ~4 ms claim) — doc 33 §4-c/§4-d.

**Wow.** "Two charts. CPython threads: a flat line at one core's throughput (the GIL). molt
isolates: a straight diagonal — 8 cores, ~8× throughput. Same algorithm. And each isolate's
output is exactly CPython's."

**Build path (demo-side).**
1. A canonical embarrassingly-parallel workload (e.g. parallel map-reduce over a partitioned
   dataset, or N independent Monte-Carlo seeds) expressed once; a CPython `threading` baseline (to
   show the GIL flat-line) and a CPython `multiprocessing` baseline (the fair process comparison).
2. `tools/demos/throughput_isolates.py`: sweep worker count 1..N_cores; measure throughput
   (items/sec) for molt-isolates vs CPython-threads vs CPython-procs; capsule `metric =
   throughput_scaling_slope` (and absolute items/sec at N_cores).
3. **Oracle: each isolate's per-partition result == CPython's** (DEFAULT-tier byte-identity) +
   the aggregated result is identical regardless of worker count.
4. `render.py` card: the scaling chart + the byte-identity proof + the re-exec cost.

**Pre-registered gate.** `throughput_isolates --gate`: scaling slope ≥ a floor (near-linear), at
N_cores throughput ≥ K× single-isolate, **oracle byte-identical**, and the molt-isolates result
invariant to worker count.

**Composition.** Composes with doc 33 Layer B; uses the RC substrate (docs 20/27) only in its
default form (per-isolate, no atomic-RC tax). It is the **first, safe** half of the GIL-free
story; Demo 4b (§5.6) is the second, deeper half.

---

### 5.5 Demo 1 — tinygrad + DFlash ML inference (FIFTH; deepest wow, deepest gate)

**What it proves.** A real model's `import tinygrad` inference runs on **GPU** through molt's
tinygrad-conformant stack, and **DFlash speculative decoding** delivers a *measured* end-to-end
tokens/sec multiple over the same model under CPython+tinygrad on the same GPU — with the
speculative output **provably byte-identical** to non-speculative greedy decode (lossless). The
moat: exact tinygrad semantics + exact DFlash fidelity (CLAUDE.md turn-blocking mandate).

**Structural capability it requires (and the owning arc).**
- **Real-device GPU codegen e2e**: `runtime/molt-gpu` has 26 prims × 8 backends green and the CPU
  reference executor; the demo needs a *real device* (Metal on the dev M5 Max, or CUDA/HIP)
  running a full model forward. **Owning arc: doc 16 §3 GPU + roadmap Y2-3 (`molt-gpu`
  Movement/Contiguous device path).** **Blocking fact: demo-grade real-device e2e forward for a
  GPT-2/Llama-class model is the gap** (per-backend renderers exist; the integrated device run is
  the work).
- **A trained DFlash drafter**: doc 16 §3.3 — the DFlash *contract* is paper-faithful and
  fail-closed, but **no trained drafter is shipped** ("drafter models require external training").
  **This is the hard fidelity gate.** Per CLAUDE.md: "If a model lacks a real trained DFlash
  drafter, say so explicitly and do not fake support."  **Therefore Demo 1 ships in two stages:**
  - **Stage 1 (capability-true, available sooner): the *lossless block-speculative* path**
    (`gpu/generate.py::speculative_decode_greedy`) — molt's own lossless block-speculative decode,
    which IS shippable without an external trained drafter, proves the *speedup-with-lossless-
    equivalence* mechanism end to end. The oracle (spec output == greedy output) is the headline
    correctness proof. This is honestly labeled "lossless block-speculative", NOT "DFlash".
  - **Stage 2 (DFlash-true): the full DFlash path** with a real trained drafter/verifier adapter
    (`gpu/dflash/`), conditioned per the contract. Ships only when a real adapter exists; until
    then the manifest entry is `enabled=false` with `blocking_facts = ["trained_dflash_drafter"]`.
    No generic speculative decoding is ever labeled DFlash (the mandate).
- **tinygrad API unchanged**: `src/tinygrad/` shim must keep `import tinygrad` / `from tinygrad
  import Tensor` exact (doc 16 §3.2; ROADMAP tinygrad-off-the-shelf burn-down). **Owning arc:
  doc 16 / the tinygrad-friend lane.**

**Wow.** "This is `import tinygrad` Python — your code, unchanged — generating tokens on the GPU
faster than CPython. And the speculative decode? Here's the proof it produced the *exact same
tokens* as the slow path. Lossless speedup." Stage 2 adds: "with a trained DFlash drafter,
target-conditioned, the paper's algorithm exactly."

**Build path (demo-side).**
1. Stage 1: a small GPT (the `examples/microgpt` weights, or a real GPT-2 small via the
   off-the-shelf lane) on `molt.gpu.Tensor`; greedy decode + `speculative_decode_greedy`;
   `tools/demos/ml_inference_bench.py` measures tokens/sec for {molt greedy, molt lossless-spec,
   CPython+tinygrad greedy}; capsule `metric = tokens_per_sec_speedup_vs_cpython` + a *second*
   `metric` for the spec-vs-greedy speedup; **oracle: lossless-spec tokens == greedy tokens
   (byte-identical)** via `tests/differential/demos/`.
2. Device sweep: CPU executor (always), then real device (Metal) when the e2e device path lands;
   each a capsule cell `(device × model)`.
3. Stage 2: register the DFlash entry (disabled until a trained adapter exists); when it lands,
   the same harness runs the DFlash runtime and adds a DFlash capsule with the contract-conformance
   assertion (`require_dflash_conditioning` present; verifier/drafter separation exercised).
4. `render.py` card: tokens/sec vs CPython, spec-vs-greedy multiple, the lossless oracle, the
   device, and (Stage 2) the DFlash-fidelity attestation.

**Pre-registered gate.** Stage 1 `ml_inference_bench --gate`: tokens/sec ≥ 1.00× CPython on the
target device, lossless-spec speedup ≥ a floor, **spec output byte-identical to greedy**, tinygrad
import surface unchanged (`tests` assert the shim). Stage 2 gate adds DFlash contract conformance
+ the trained-drafter presence check (fail-closed if absent — never faked).

**Composition.** Composes with doc 16 (GPU/tinygrad/DFlash), the roadmap ML horizon (Y2-3), and
the tinygrad-off-the-shelf friend lane (ROADMAP §13/§17). **The fidelity mandate is enforced
structurally**: the Stage-1/Stage-2 split means molt never claims DFlash without a trained
drafter, and the lossless oracle means a "fast" ML demo can never be a wrong-answer ML demo.

---

### 5.6 Demo 4b — GIL-free free-threading shared-memory (the full Demo 4; deepest concurrency gate)

**What it proves.** Within ONE interpreter, under `molt build --unleashed` (doc 33 Layer C), a
shared-memory data-parallel workload (the doc-33 §1.1 canonical "10 GB array, two threads both
read") scales **near-linearly with cores** with **no copying**, beating not just CPython-GIL but
the isolate approach for genuinely-shared-mutable data — while the **DEFAULT build of the same
code is byte-identical to CPython** and the unleashed memory model (doc 33 §3.5) is precisely
specified (which races are defined vs UB).

**Structural capability it requires (and the owning arc).**
- **Layer C free-threading**: biased reference counting (doc 33 §3.2), per-object container locks
  (§3.4), the §3.5 memory model, and — the thesis (doc 33 §0.1) — **Perceus borrow inference
  (doc 27)** to delete the majority of atomic-RC ops so molt's free-threading is *faster than
  CPython 3.13t's*, not merely as fast. **Owning arcs: doc 33 (Layer C) + doc 27 (Perceus) +
  docs 20/45 (RC/exception-region substrate).** **Blocking facts: Layer C, biased RC, and Perceus
  are ALL design-only today.** This is the deepest gate in the portfolio.

**Wow.** "Same code. `molt build --unleashed`. A 10 GB array, eight threads reading it in
parallel — no copy, near-8× — and it's *faster* than CPython's no-GIL build because molt proved
most reference counts away at compile time. Drop the flag: it's exactly CPython again."

**Build path (demo-side).** Identical harness shape to Demo 4a (`throughput_*`), but the workload
is genuinely shared-mutable (true zero-copy shared read/write), built `--unleashed`; the capsule
adds a §3.5 memory-model-conformance assertion and a *Perceus-attribution* field (RC ops emitted
vs CPython, the thesis number). Oracle: DEFAULT build byte-identical; UNLEASHED build correct
under the specified memory model (the defined-race tests, doc 33 §3.5).

**Pre-registered gate.** Disabled (`enabled=false`, `blocking_facts = ["doc33_layer_c",
"biased_rc", "doc27_perceus"]`) until the capability lands; when it does, the gate is: near-linear
scaling, DEFAULT byte-identical, UNLEASHED memory-model tests green, RC-op count materially below
CPython 3.13t (the thesis).

**Composition.** This is where the demo portfolio *proves the deepest roadmap claim* (doc 33 §0.1
thesis). It composes with the entire MM ladder (doc 33 §0.1 table: rung 0/1 landed, rung 2 =
Perceus design-only). It is correctly *last*: a free-threading demo that races dict internals into
UB is "a memory-safety hole wearing a flag" (doc 33 §0) — the gate refuses to ship it before the
memory model is specified and the substrate is sound.

---

## 6. How the portfolio composes (decomposition 21a–e · parallel execution · cross-arc deps)

### 6.1 With the decomposition program (21a–e, doc 21)

The demos are **read-mostly consumers of built artifacts**, so they compose cleanly with the
decomposition and even *prove its payoff*:
- **21a (`function_compiler.rs` split)** + **doc 08 L3 (`molt-backend-native` crate)**: Demo 5's
  "compiler self-build" capsule is the *evidence* that the split delivered the incremental-build
  win. As 21a/L3 land, that capsule improves; the demo is the scoreboard for the decomposition's
  DX promise.
- **21b (crate-graph)**, **21c (frontend mixin)**, **21d (CLI package)**: the demo harness
  (`tools/demos/`) is new, isolated tooling with **no overlap** with these decomposition lanes —
  it can be built in parallel with them on a non-overlapping file lane (doc 52 §Resources).
- The demo harness deliberately lives under `tools/demos/` (alongside `tools/bench*.py`) and
  `examples/` — directories the decomposition does not churn — to avoid lane collisions.

### 6.2 With the parallel multi-agent execution model (doc 52 §Resources)

- The demos map onto the **three-lane model** (doc 51 §9 / CLAUDE.md Council Doctrine) as **Lane
  C (infra/scoreboards)** work: the harness (D0), Demo 3, Demo 5, Demo 2 are infra-grade and
  *make A&B faster* by surfacing reds as roadmap tasks. Demo 4a/4b and Demo 1 *consume* Lane-A
  (safety) and Lane-B (perf) deliverables.
- **Non-overlapping file lanes**: D0 (`tools/demos/`), Demo 3 (`tools/bench_suites.py` +
  `tools/demos/`), Demo 5 (`tools/demos/dx_*.py`), Demo 2 (`examples/microgpt` + `tools/demos/`),
  Demo 1 (`examples/` + `tools/demos/`), Demo 4 (`tools/demos/`). These do not touch
  `function_compiler.rs`, TIR passes, or the runtime — so demo agents are **never build-triggering
  for the daemon** and can run alongside the ≤2 build-triggering compiler agents without counting
  against that budget (CLAUDE.md). **Exception: Demo 5's parallel-agent gate caps at N=2 in CI.**
- **Agents never push; the lead integrates** (doc 52 §Resources). Each demo's `enabled=false`-
  until-reproducible discipline (mirroring `bench/friends/manifest.toml`) means a half-built demo
  cannot turn a gate red on main — it simply stays disabled until its capsule + oracle are green.

### 6.3 Cross-arc dependency graph (binding)

```
Demo 3 (perf)      -- reads --> perf_scoreboard.py (BUILT) ; PyPy/Codon cols -- need --> doc52 §C.1 reference harness
Demo 5 (DX)        -- reads --> doc08/dx_baseline (thin-LTO, crate split) ; MOLT_SESSION_ID isolation (BUILT)
Demo 2 (browser)   -- needs --> WASM run-path (scoreboard gap) + W3/RuntimeSurfacePlan size (doc09/13, doc00 §4.3) + deploy skill (BUILT)
Demo 4a (isolates) -- needs --> doc33 Layer B (isolates + channels)  [design-only]
Demo 1 (ML/DFlash) -- needs --> doc16 §3 GPU device codegen (roadmap Y2-3) + a TRAINED DFlash drafter [gap] ; tinygrad shim (BUILT)
Demo 4b (nogil)    -- needs --> doc33 Layer C + biased RC + doc27 Perceus  [all design-only]
ALL                -- on -----> D0 tools/demos/ harness (this arc) + safe_run.py (BUILT) + bench.py (BUILT)
```

The single ordering rule: **a demo is enabled (CI-gated, claimable) only when its capability arc
has shipped the fact it consumes.** Until then it is a fully-specified, disabled manifest entry
with named `blocking_facts` — which itself is valuable: it is a *standing forcing function* on the
owning arc (the demo is the acceptance test the capability arc is trying to turn green).

---

## 7. Risks + structural (not band-aid) treatment

| # | Risk | Band-aid (rejected) | Structural treatment (this plan) |
|---|---|---|---|
| R1 | **A demo "works on my laptop" and rots** | screenshot in the README | Every demo is a `manifest.toml` entry + `EvidenceCapsule` + `DemoGate` CI job; bit-rot turns a gate RED exactly like a scoreboard cell (§3.2). The card is *generated* from capsules, never hand-edited (§3.3). |
| R2 | **Cherry-picked / warm-only / wrong-machine numbers** (the SpecBench/METR gaming class, doc 52 §A.4) | "trust me, it's faster" | Capsule mandates cold+warm, ≥5 samples, CoV stability, host fingerprint, git SHA, reproducing command, GREEN/RED_STABLE/RED_NOISY/TIE/DIMENSIONAL_WIN classification — the full Constitution methodology line (§3.1). A DIMENSIONAL_WIN is reported as dimensional, never as a speed heal (CLAUDE.md). |
| R3 | **A "fast" demo that is secretly wrong** (silent divergence, doc 52 §A.1 P0²) | skip the oracle to get a bigger number | Every demo with a definable oracle proves byte-identity (CPython) or lossless-equivalence (spec==greedy); the oracle pass is a gate field (§3.1 OracleEquivalence). No oracle ⇒ `oracle=none_pure_perf` declared explicitly, not omitted. |
| R4 | **Faking DFlash / drifting from tinygrad** (CLAUDE.md turn-blocking) | label molt's generic lossless-spec as "DFlash"; tweak tinygrad API for convenience | Demo 1 is split Stage-1 (honest "lossless block-speculative") vs Stage-2 (DFlash, gated on a *trained* drafter, `enabled=false` until it exists); tinygrad shim has a surface-unchanged test; DFlash entry asserts contract conformance + trained-drafter presence, fail-closed (§5.5). |
| R5 | **Demo build-load OOMs/hangs the shared host** (the 97 GB/139 GB incidents, CLAUDE.md) | run the binary directly to "save time" | Every demo run goes through `tools/safe_run.py --json` (RSS cap + wall-timeout) — the harness does not offer a raw-binary path. Demo 5's parallel gate caps at N=2 (≤2 build-triggering). |
| R6 | **Size/cold-start floor (< 2 MB, < 1 s) not yet reachable** ⇒ temptation to fudge the number | hardcode "< 2 MB" in the card | The capsule records *actual* size/cold; the gate floor for an unreached target is set to the current measured best with a tracked owner (the size arc), and flips to the contract floor when W3/RuntimeSurfacePlan land — never silently passed (the rustc-perf "justified-or-reverted" rule, doc 52 §A.1). |
| R7 | **PyPy/Codon columns fabricated** because not installed | type in numbers from a blog post | Schema carries them as nullable; they render "—" until the reference harness (doc 52 §C.1) installs+pins them; a Codon comparison on non-equivalent semantics is marked "non-equivalent", never win/loss (doc 51 §0). |
| R8 | **Demo agent collides with compiler-build agents / decomposition lanes** | run anyway and hope | Demos live in `tools/demos/` + `examples/` (non-churned dirs), are non-build-triggering for the daemon, and respect the ≤3-agent / ≤2-build / non-overlapping-lane model (§6.2). Demo entries stay `enabled=false` until reproducible, so a WIP demo can't red main. |
| R9 | **A demo masks a real RED instead of surfacing it** (e.g. a real-workload row is slow) | drop the row from the suite | A RED real-workload row is the *intended* output: it is a roadmap perf-triage task (doc 51 §5/§perf-triage), surfaced by the gate, not hidden. Removing a row to make the gate green is a test-immutability violation (doc 52 §A.1.4). |

---

## 8. Verification & gates per phase (measurement discipline, doc 52 §A / CLAUDE.md)

Every phase's done-contract is **pre-registered** (doc 52 §B loop step 2) in its `manifest.toml`
entry: the exact build+run commands, the expected capsule shape, the headline floor, and the
falsifier. Universal gates:

- **Measurement**: pyperf discipline — ≥5 samples (3 in PR fast-tier), median+stdev+CoV, cold AND
  warm, quiescent host (preflight load check; demos are not built-triggering so they tolerate
  background compiler agents but still report CoV), every run via `safe_run.py --json`. Classify
  every result; no optimizing/claiming from a `RED_NOISY` (CLAUDE.md). (`tools/perf_scoreboard.py`
  already embodies this for Demo 3; the harness reuses its sampling for the rest.)
- **Oracle**: byte-identical vs system CPython (the un-gameable differential oracle, doc 52 §A.1)
  for Demos 2/3/4a/4b default-tier; lossless-equivalence (spec==greedy) for Demo 1; declared
  `none_pure_perf` only where genuinely no oracle exists.
- **Harness self-test**: `tests/tools/test_demo_harness.py` (capsule round-trip, gate
  exits-nonzero-on-synthetic-RED, manifest schema), `tests/differential/demos/` (the oracle
  anchors).
- **CI tiers** (cost discipline, doc 52 §C.3): fast/subsampled per-PR (enabled demos, 3 samples,
  release-fast, single backend), full matrix nightly (all enabled demos × all live targets ×
  release-fast+release-output × cold+warm), reference-scale (PyPy/Codon, real-tinygrad) per
  release when the harness lands.
- **Landing report** (doc 51 §landing-report / CLAUDE.md): each demo lands with "harness tests
  green; capsule written; headline ≥ floor; oracle passed; no CPython-red cell introduced;
  blocking-facts for disabled stages named with owning arc."

---

## 9. Concrete first three landable units (what an agent picks up first)

Per doc 52 §B "pick the smallest complete structural change with the largest class-kill":

1. **D0 — `tools/demos/` harness** (the institution). Smallest complete unit: `manifest.toml`
   (schema), `capsule.py` (EvidenceCapsule schema_version=1 + writer + validator), `run_demo.py`
   (build via `bench.py`, run via `safe_run.py --json`, write capsule), `oracle.py`, `render.py`,
   `tests/tools/test_demo_harness.py`. **Class-kill: "demos aren't reproducible."** Done-contract:
   harness tests green; capsule round-trips; `--gate` exits nonzero on a synthetic RED; one
   trivial `enabled=true` demo (`hello.py` "runs and matches CPython") proves the full pipeline.
2. **D1 — Demo 3 perf card** over `perf_scoreboard.py`, native+LLVM, curated suite + 4–6
   real-workload rows. **Class-kill: "I can't see molt beat CPython across the board."**
   Done-contract: every native+LLVM cell ≥ 1.00× (or the RED row is filed as a roadmap task);
   card renders; CI fast-tier green.
3. **D2 — Demo 5 DX loop + N-agent proof.** **Class-kill: "compiling Python means a slow
   edit-run loop / no parallel dev."** Done-contract: edit→rerun warm median measured + floor
   set; N=2 parallel sessions proven collision-free; capsules + card.

These three are fully landable on **today's** capabilities (no blocking facts), are
non-build-triggering for the daemon, and immediately produce the two most universal switching
arguments (faster + still-fast-to-iterate). D3/D4a/D5/D4b follow as their capability arcs ship,
each already fully specified above so they slot in without redesign.

---

## 10. The one-line statement of this arc

**Make the roadmap's contract (doc 51 §0) irrefutable by turning each headline claim into a
re-runnable, oracle-backed, CI-gated EvidenceCapsule — so that "molt is faster, smaller, parallel,
GPU-capable, and pleasant to develop in" is not a sentence a reader has to trust, but a command a
reader can run.**
