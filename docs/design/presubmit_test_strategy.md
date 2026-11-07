# Presubmit test strategy: the compiler oracle first

Status: design, 2026-11-07, for one Goldfish round. Owner: integrator (owner-directed
takeover, 2026-11-07).

## Problem (measured)

`tools/gate_yield.py` over 86 landing logs (11-06/07, 48.6 gate-hours):

| Area | Gate-hours | Share |
| --- | --- | --- |
| WASM end-to-end tests | 21.1 | 43% |
| Static/policy scanners (2,082 runs) | 8.9 | 18% |
| Proof-queue tests | 6.0 | 12% |
| Memory guard, retention, build-env probe, landing tooling | ~3.5 | ~7% |
| Differential programs against CPython | ~0 | ~0% |

The full receipt history: 1,382 gates, 235 gate-hours, 274 candidate failures, 6 flakes.
The differential corpus (1,061 basic, 2,099 stdlib, 19 proof-generated programs) runs only in
the nightly family (`nightly.differential.aggregate`, 16 shards). About 40% of presubmit tests
our own apparatus; one WASM test costs 131 s and 6.53 GB, most of it JIT-compiling an 89 MB
linked module that a 30-line program pulls in.

## Prior art

- Go `test/` (`// run`, `// errorcheck` programs), Rust compiletest, Zig behavior tests, LLVM
  lit + FileCheck, Cranelift filetests: the presubmit is a corpus of tiny hermetic
  program-to-expected-output tests that runs in minutes.
- Infrastructure code has few tests; expensive end-to-end and the OS/arch matrix run
  postsubmit on a farm (build.golang.org, Rust's bors builders); flakes are quarantined.
- Google TAP and Meta's predictive test selection select by dependency, order fail-fast, and
  demote costly tests that never catch a defect, from a per-test yield record.

## Target presubmit (budget: 10 minutes p90 per lane, owner decision 2026-11-05)

1. **Static gates first** (fail fast; already run by `consumer_gate run` before commit).
2. **Differential smoke per touched backend**: a fixed, coverage-chosen subset of the corpus
   (start: 150 basic + 100 stdlib programs) on native, plus WASM and LLVM when their crates
   or the shared frontend/midend change. Programs run against a cached runtime build
   (the existing product store) and share one compiled runtime per backend.
3. **First-hop importer tests** of the change (owner decision 2026-11-06), with apparatus
   tests narrowed (below).
4. Postsubmit on the farm: the full differential corpus, the OS/arch matrix, WASM end-to-end,
   data readers, and apparatus suites.

## Mechanisms

A. **WASM compiles once.** Ship the runtime and the prebuilt stdlib (HF-773's direction) as one
   precompiled module (`Module::serialize`/`.cwasm`, keyed by its bytes and engine config) and
   link each program's small module against it; tests then JIT only their own code. Meanwhile
   enable the Wasmtime cache for test runs (retries and unchanged programs across landings
   hit) with an explicit size bound, and shrink programs: target facts in import discovery
   (HF-226/HF-700), function-local imports as call-graph edges, whole-program dead-code
   elimination on the linked module.
B. **Differential smoke selection.** One declared subset per backend in `tools/proof_plan.toml`
   (typed, reviewed like any rule), chosen by measured coverage of op kinds and runtime entry
   points; `tools/gate_yield.py` tracks its yield. Selection by changed compiler component
   (frontend, midend, backend crate, runtime crate) widens the subset, never narrows it below
   the smoke floor.
C. **Apparatus tests narrow.** A suite whose subject is apparatus (proof queue, memory guard,
   retention, landing tooling, build-environment probe) runs in presubmit only when its own
   owned paths change; widely imported modules (`tools/proof_plan.py`, `tools/molt_dev.py`)
   stop selecting every suite that imports them by splitting the import (owners import the
   piece they use). Growth freezes: a new apparatus test needs a measured defect it would
   have caught. Tests of retired apparatus are deleted with it.
D. **Scanners consolidate.** About 40 separate static scanners each re-read the tree. One
   shared pass parses each file once and feeds every check; checks that a type can enforce
   (op kinds, identities, attribute keys) become types and their scanners are deleted
   (AGENTS.md: a source-scraping audit is a bridge).
E. **Yield drives policy.** `tools/gate_yield.py` runs after every landing; a gate costing more
   than 0.5 gate-hours per week with no candidate failure in 30 days moves to postsubmit by a
   reviewed policy change, not by hand.

## Order

A (cache now, shared module next) and E are in progress; B next (smoke subset and runner);
C and D follow, each one lane, each landing its own deletion. Each step reports the presubmit
wall time and gate-hours before and after from the yield table.

## Invariants

- No defect class loses its only presubmit oracle without a postsubmit owner and a yield record.
- Selection widens on uncertainty: an unmapped changed path selects the full smoke floor.

## Goldfish round 1 (2026-11-07): applied; revised plan

1. WASM (was A). The differential harness runs WASM under `node wasm/run_wasm.js`
   (`tools/compat/backends.py:951-986`); only the pytest end-to-end tests use molt-wasm-host
   (`tests/wasm_linked_runner.py`). The runtime half of "compile once" exists: `--split-runtime`
   and the precompiled runtime module (`ModuleRole::Runtime`, `MOLT_WASM_PRECOMPILED_RUNTIME_PATH`,
   `runtime/molt-wasm-host/src/precompiled.rs`; `tests/test_wasm_split_runtime.py` proves a
   byte-identical runtime across programs). Revised: first measure the 131 s split
   (`MOLT_BUILD_DIAGNOSTICS`: build vs wasm-ld vs JIT); point the pytest WASM tests at the split
   runtime with its precompiled module cached in the product store keyed by runtime identity;
   keep one linked-mode cell; give the Wasmtime cache a custody-owned config. No shared stdlib
   module: it conflicts with closed-world dead-code elimination (HF-226, HF-700). Choosing one
   differential WASM host (molt-wasm-host vs node) precedes any WASM smoke.
2. Budget (was step 2). A warm native build of a small program costs 14.7 s
   (`docs/design/dev_jit_tier.md`); 250 programs would take ~20 minutes. Size the smoke as
   (budget - cold build) x workers / measured per-program cost, admit programs with molt_diff's
   `--deadline-s`, and take only programs with calibrated passing verdicts
   (`tools/suite_honesty/`, native 560 of 861 passing; WASM 7 rows).
3. Verdict reuse (new, first). Reuse a program's verdict keyed by (program source, compiler
   identity, runtime identity, oracle), as Go's test cache and Bazel's test-result cache do;
   a tools-only lane then reruns no program, and a compiler lane reruns only programs whose
   compiler identity changed. The product store and frontend build identity already compute
   these keys.
4. Selection (was B). Start with a fixed subset as a `[[rule]]` with `precheck = true` that calls
   molt_diff (no new runner); record used runtime symbols and op kinds during the nightly run;
   then choose the subset by set cover. Stdlib programs map to paths by the enforced naming
   (`re_*.py` for runtime-regex).
5. Invariant 2 is withdrawn: "unmapped path selects the smoke floor" would charge every tools
   lane 10+ minutes and conflicts with the owner's importers-only presubmit (landing_queue.md
   9.10). Selection keys on compiler-input identity instead (item 3); any remaining conflict
   goes to the owner.
6. Apparatus (was C). `test_dependency_graph` already holds function-level nodes for name
   imports; prototype hunk-to-function selection and measure it before splitting modules;
   narrow the rule globs that match `tools/proof_plan.py` and `tools/molt_dev.py`.
7. Yield (was E). Key records per test and per phase (try run vs batch), attribute failures
   per test (not per `--maxfail=1` cohort), and exempt gates that also run before commit.
8. Postsubmit owner of the full corpus: the nightly family plus bisection.

## Exit condition and apparatus freeze (owner directive 2026-11-07)

The apparatus work ends when all four hold; the integrator owns it until then.

1. The common path works on the supported host (macOS arm64): edit, `uv run python
   tools/consumer_gate.py run -- PATHS`, commit with pathspecs, `tools/molt_dev.py land --fast`
   lands a small compiler or runtime change on the first try. Shown 2026-11-07 by the marshal fix
   (b00696d2fd: 30 gates, 624 s, first try).
2. CI returns real verdicts: the proof executor runs in the synced environment (3be76b90fb), and
   `main` CI failures are product failures, not provisioning.
3. One environment mechanism (`docs/design/one_environment.md`, stages 1-3).
4. The fixed small landing suite: WASM end-to-end in postsubmit
   except one linked cell, a calibrated CPython differential smoke in presubmit, first-hop
   importers selected by changed function, and `tools/gate_yield.py` showing presubmit p90 at or
   under 10 minutes for compiler lanes.

After that the apparatus is frozen: no new runner, receipt, ledger, scanner, dashboard or
generated projection; a change to the apparatus deletes something or fixes a demonstrated
defect. Capacity goes to the compiler.
