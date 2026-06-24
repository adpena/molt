<!-- Foundation blueprint 66. Arc: full deterministic CPython >=3.12 PARITY across ALL
backends (native/WASM/LLVM/Luau). Author: portfolio-architect. Date: 2026-06-23.
Status: design only / executable plan. Assigned path:
docs/design/foundation/66_compat_cpython_parity.md. This doc is the PARITY-PLANE counterpart to the
language-semantics audit (doc 30), the stdlib/surface audit (doc 16), the
ecosystem audit (doc 17/24), and the differential-fuzzing lane (doc 31). It does
NOT duplicate those: it builds the MATRIX, the single multi-backend ORACLE, and
the structural "parity fact" plane that makes backend-divergence UNEXPRESSIBLE,
then routes docs 16/24/30/31's per-case findings through that plane as CI-gated
debts. Read-only investigation + this one Write; the lead integrates. -->

# Foundation Blueprint 66 — Full Deterministic CPython >=3.12 Parity Across All Backends

**Arc owner surface:** frontend semantics + runtime stdlib + differential harness + CI gates
**Author:** portfolio-architect
**Date:** 2026-06-23
**Status:** design only / executable plan
**Composes with (cite, never duplicate):** doc 16 (CPython surface/stdlib/GPU gap audit), doc 17 + 24 (ecosystem/third-party compat), doc 30 (core-language feature/op portfolio — the per-family language parity audit), doc 31 (generative differential fuzzing), doc 00 (integrated parallel build program — the tier DAG and multi-agent model), doc 14 (target × profile parity audit), doc 51 (ten-year roadmap). Spec anchors: `docs/spec/areas/compat/README.md`, `docs/spec/areas/compat/contracts/{verified_subset_contract,dynamic_execution_policy_contract,compatibility_fallback_contract}.md`, `docs/spec/areas/compat/surfaces/language/{semantic_behavior_matrix,syntactic_features_matrix,type_coverage_matrix,language_surface_matrix}.md`, `docs/spec/areas/testing/{0007-testing,0008_MINIMUM_MUST_PASS_MATRIX,0504_DIFFERENTIAL_TESTING_ORACLE}.md`, `docs/spec/STATUS.md`.

---

## 0. The Time-Traveler Frame: The End-State, Stated Crisply

**END-STATE (the 5-to-100-year outcome this arc makes inevitable):**

> Any conforming Python 3.12+ program — restricted only by the four documented carve-outs (no `exec`/`eval`/`compile`, no runtime monkeypatching, no unrestricted reflection) — runs **byte-identically** on every molt backend (native/Cranelift, WASM, LLVM, Luau) across every supported CPython baseline version (3.12 / 3.13 / 3.14), with identical stdout, identical exception type+message, and a compatible exit-code contract. **Backend-specific behavioral divergence is not a bug we chase case-by-case; it is a state the IR cannot represent.**

Working backward from that outcome, the question is never "which failing test do we fix next?" It is: **which structural FACT, if it existed, would make a whole CLASS of parity divergence UNEXPRESSIBLE?** The compression ladder for this arc retires *categories* of wrongness, one per month, by adding first-class facts/representations:

| Class of wrongness to retire | The fact that makes it unexpressible |
|---|---|
| "Native passes, WASM/LLVM/Luau silently differ" | **Backend-Divergence Oracle as a first-class gate** + the **Semantic-Authority-Is-Shared invariant** (no backend may branch on semantic meaning; backends differ only in lowering of one shared TIR) |
| "A test fails on a backend and no gate goes red" | **The Parity Matrix is a single machine-readable artifact**, dimensioned `(test × backend × version)`, fed by ONE oracle, ratcheted down-only (extend the existing `check_suite_honesty` substrate to ALL four backends) |
| "We don't know our true parity surface; ~1897 stdlib tests UNCALIBRATED" | **Calibration is a build product, not a survey** — every backend produces its own `<backend>_calibration.jsonl` from the SAME run, so "uncalibrated" is a measured, loud gap not an unknown |
| "Two parity runners disagree (`molt_diff.py` vs `parity_gate.py`)" | **One oracle, one comparison law** (`molt_compat`), with the 3-tier comparison law as a *mode* of that one oracle, not a second tool |
| "A new opcode/feature silently routes to a default and miscompiles on one backend" | **The exhaustiveness obligation** — every parity-relevant opcode is in the registry (doc 25), every backend's classifier match is exhaustive, and the matrix has a row that would go red |
| "An edge/corner case nobody wrote a test for diverges in production" | **Systematic surface enumeration** — the parity surface is generated from the CPython grammar + datamodel + stdlib reference, so coverage gaps are *named cells*, not silent voids |

The deliverable of every phase is therefore **a new fact that makes a class of bad programs unexpressible**, not "more passing tests." Tests are how we *witness* the fact; the fact is the product.

---

## 1. Investigation Findings: What Exists, What Is Structurally Missing

This plan must advance and compose with the substantial parity machinery already in the tree. The audit below is the basis for every decision that follows; file/line anchors verified against HEAD.

### 1.1 What already exists (the substrate we build ON, never duplicate)

**The rich differential runner — `tests/molt_diff.py` (~4529 lines).**
- Drives CPython baseline + `molt build` + run, compares stdout byte-exactly (`diff_test` at `tests/molt_diff.py:3679`).
- **Version-gated stderr comparison**: `_stderr_matches` (`:423`) with `exception_signature` mode compares exception type+message only, ignoring frame/path formatting — the correct cross-engine stderr law (comment at `:434` explicitly notes wasm frame divergence is expected, type+message is not).
- **Per-file metadata**: `# MOLT_META:` parsed at `_collect_meta` (`:168`) — `skip`, `platforms`, `py`/`min_py`/`max_py`, `wasm`, `expect_fail=molt`/`xfail` (`:447`), `stdlib_profile`. `# MOLT_ENV:` env overrides (`:149`).
- **Version derivation**: `_molt_sys_env_for_python_exe` (`:298`) stamps `MOLT_PYTHON_VERSION` + `MOLT_SYS_VERSION_INFO` from the CPython under test, so molt's version-gated messages align with the baseline (3.12/3.13/3.14).
- **The honesty sink**: `MOLT_DIFF_RESULTS_JSONL` (`_diff_results_jsonl_path` at `:565`) writes one JSON line per test with its **RAW** status (before the xfail/xpass overlay), plus resolved status + reason tag (`_record_diff_result` at `:583`). This is the authoritative record of what molt actually did vs CPython.
- **Profile dimension**: `--build-profile` (`_diff_build_profile` at `:1635`) — dev / release-fast etc.
- **Host/memory discipline**: RSS measurement default-on, adaptive rlimit, daemon custody, build-lock pruning, dyld-quarantine retry pipeline (macOS), `harness_memory_guard` on every child.

**THE CRITICAL STRUCTURAL GAP — `molt_diff.py` is single-backend (native).** It has `--build-profile` but **no `--target` flag**. Backend dimensioning is done by *separate, divergent harnesses*:
- `tools/wasm_diff.py` (WASM, separate tool, produces `wasm_calibration.jsonl`),
- **no `luau_diff.py` exists, no `llvm_diff.py` exists** (verified: `ls tools/ | grep -iE "luau_diff|llvm_diff|backend_diff|target_diff"` → NONE).
- A *second, unrelated* parity runner exists: `tools/parity_gate.py` — a 3-tier (STRICT/RELAXED/EXCLUDED, marker-driven) gate that runs `molt run` (not `molt build` + execute) and re-implements CPython comparison, output normalization, and tier logic from scratch (`tools/parity_gate.py:271` `compare`). **This is a parallel source of truth** for the parity comparison law — the exact dual-truth anti-pattern the project forbids.

**The honesty ratchet — `tools/check_suite_honesty.py` + `tools/suite_honesty/`.** This is the single most important existing asset for this arc, and the proof that the END-STATE is already the project's declared intent:
- `tools/suite_honesty/differential_expectations.json` is the SINGLE SOURCE OF TRUTH for known-failing differential tests, **already dimensioned `(test × backend × version)`** where backends are exactly `native / llvm / wasm / luau` and versions are `<backend>@<cpython-version>` (the `_dimensions` field documents this verbatim). Each `fail` entry carries `tracking` + `root_cause` + `evidence`, machine-linted (anti-parking-lot).
- `tools/suite_honesty/honesty_baseline.json` is the **down-only ratchet**: per-dimension `expected_fail_ceiling` may only DECREASE.
- It consumes a **calibration JSONL** (produced by `molt_diff.py` with `MOLT_DIFF_RESULTS_JSONL`); `--check` never runs the suite (CI determinism), `--calibrate` regenerates the snapshot. A test that fails without a manifest entry → RED (untracked regression); a manifest entry that now passes → RED ("remove it, it's fixed").
- **The damning measured state (from the manifest's own `_calibration_scope`/`_dimensions` headers):** `native` is calibrated (basic + a 28–35-test stdlib slice on 3.14); `wasm` has 4 calibrated entries; **`llvm` and `luau` are UNCALIBRATED at the suite level**; **~1897 stdlib tests are UNCALIBRATED even on native.** The matrix *schema* exists; the matrix *data* is ~1/4 of one backend.

**The satellite-parity guard — `tools/check_satellite_parity.py`.** A fail-closed contract that two physical copies of feature-gated stdlib modules (in-tree `#[cfg(not(feature="stdlib_X"))]` vs satellite `runtime/molt-runtime-X/`) do not behaviorally drift. Normalizes access-layer differences, compares residual line-multiset, ratchets toward zero. This is the *intra-runtime* dual-truth guard; it is the precedent and sibling for the *inter-backend* parity guard this arc generalizes.

**Other existing tooling (compose, don't duplicate):** `tools/check_differential_suite_layout.py` (lane layout), `tools/check_ecosystem_compat.py` (ecosystem ratchet — the template `check_suite_honesty` was modeled on), `tools/verified_subset.py` (verified-subset manifest check/run), `tools/diff_coverage.py` + `tests/differential/COVERAGE_INDEX.yaml` (PEP/API → test map), `tools/stdlib_full_coverage_manifest.py` `TOO_DYNAMIC_EXPECTED_FAILURE_TESTS` (by-design exec/eval/compile exclusions — partitions the fail space with the honesty manifest, no overlap), `tools/gen_compat_platform_availability.py`, `tools/cpython_regrtest.py` (G5 gate — runs CPython's own regression suite).

**The spec compat plane — `docs/spec/areas/compat/`.** Canonical status vocabulary (`missing` / `api_shape_only` / `behavior_partial` / `behavior_full` / `intentional_divergence`), dimension tags (`py312/313/314`, `native`, `wasm_wasi`, `wasm_browser`, `linux/macos/windows`), generated-vs-hand-edited file discipline. The `semantic_behavior_matrix.md` is the hand-edited datamodel-semantics truth (eval order, scoping, object model, control flow, numeric tower, explicit divergences §7).

**The corpus shape (verified counts):** `tests/differential/basic/` 879 files, `tests/differential/stdlib/` 1957, `tests/differential/memory/` 33, `tests/differential/loop_overflow_peel/` 9, `tests/differential/pyperformance/` 4. `COVERAGE_ANALYSIS.md` claims 95.9% feature coverage (212/221) — but this is *native-only* feature coverage; the *backend* dimension is uncounted.

**The frontend semantics surface (the shared authority that MUST stay shared):** `src/molt/frontend/__init__.py` (`SimpleTIRGenerator`, ~26k lines, the canonical visitor), `visitors/{calls,classes,comprehensions,pattern_match}.py`, `lowering/serialization.py` (`map_ops_to_json` — the JSON-kind cross-component contract), `lowering/op_kinds_generated.py`, `sema/`, `_protocol*.py`, `cfg_analysis.py`. The TIR opcode enum is `runtime/molt-tir/src/tir/ops.rs`; the kind→opcode table is `op_kinds_generated.rs` (doc 25 registry).

### 1.2 The structural diagnosis (why case-by-case fails)

Three independent root facts, each generating a *class* of divergence:

1. **No single multi-backend oracle.** Four (really 1.5 — native + partial wasm) runners, two comparison laws (`molt_diff` byte-exact + `parity_gate` 3-tier). A parity claim cannot be made atomically across backends because no tool *measures* all backends in one pass. **Class generated:** any divergence on llvm/luau, and any drift between the two comparison laws, is structurally invisible.

2. **No "semantic authority is shared" invariant, enforced.** Doc 00 §4.2 states E1/module-phase/drop authority now runs through one shared `TirModule` pipeline across all four backends, and doc 25 shows the registry is the cross-component kind contract — **the architecture is already mostly right**. But there is no *gate* that forbids a backend from re-deriving a *semantic* decision (vs a lowering decision). The `matches!`-oracle silent-default class (doc 00 Risk 7: `effects.rs::opcode_may_throw` defaults unlisted opcodes to `false`; the `ModuleImportFrom` lesson) is exactly this: a semantic fact (does this op throw?) expressed as a per-site `matches!` that silently disagrees across consumers. **Class generated:** every new opcode is a potential per-backend semantic fork.

3. **Coverage is curated, not enumerated.** The corpus grew test-by-test (879+1957 files), tracked against a hand-maintained `COVERAGE_INDEX.yaml`. Doc 31's fuzzer attacks *random* programs; but there is no *systematic enumeration* of the CPython surface (grammar productions × datamodel dunder slots × stdlib documented behaviors × the corner-case axes — empty/singleton/boundary/exception/version-gated) that turns "we have no test for X" into a *named red cell* rather than a void. **Class generated:** unknown unknowns — edge cases that exist in the language but in nobody's test list.

These three are the targets. The phases below build the fact that retires each.

---

## 2. The Structural Facts / Representations / Mechanisms to Build

Each mechanism is tied to the class it retires. These are the *products*; phases (§3) are the dependency-ordered way to land them.

### FACT 1 — The Parity Matrix (single machine-readable artifact)

**Retires:** "a backend diverges and no gate goes red" + "we don't know our true parity surface."

The parity matrix is ONE artifact, the cartesian product of the parity surface × the execution dimensions, with a measured status per cell:

```
ParityCell = (
  surface_id,          # a stable id into the parity surface (see FACT 4)
  backend,             # native | llvm | wasm_wasi | wasm_browser | luau
  cpython_version,     # 3.12 | 3.13 | 3.14
  profile,             # dev-fast | release-fast | release-output
)  ->  status ∈ {
        behavior_full,        # byte-identical (stdout) + exception type+msg + exit-code-compatible
        behavior_partial,     # passes a documented subset; gaps named
        intentional_divergence,  # the 4 carve-outs + the semantic_behavior_matrix §7 set
        fail,                 # measured divergence (a DEBT WITH AN OWNER)
        uncalibrated,         # not yet measured on this dimension (LOUD, never silently absent)
      }
```

This is **not a new schema** — it is the *generalization of the existing* `tools/suite_honesty/differential_expectations.json` dimension model (`<backend>@<version>`) to (a) be complete (every backend, every test, the profile axis), (b) carry the spec status vocabulary, and (c) be the join target for both the test corpus and the generated surface (FACT 4). The honesty `differential_expectations.json` becomes the *known-fail projection* of this matrix; `honesty_baseline.json` becomes its *down-only ratchet*. **No second source of truth is created.**

### FACT 2 — `molt_compat`: the single multi-backend parity oracle

**Retires:** "no single oracle" + "two comparison laws disagree."

ONE tool — `tools/molt_compat.py` — that is the union and structural completion of `molt_diff.py` + `wasm_diff.py` + `parity_gate.py`, with a `--backends native,llvm,wasm,luau` axis and `--python-version` axis, producing per-backend calibration JSONL in ONE pass. It does **not** reimplement the comparison law: it *imports* the one comparison law (extracted from `molt_diff.py` into a shared `tools/compat/comparison.py` library — the `_stderr_matches` exception-signature law, the stdout canonicalization, the exit-code-compatibility law). `parity_gate.py`'s 3-tier STRICT/RELAXED/EXCLUDED becomes a *comparison mode* of that one library (`--law strict|relaxed`), and `parity_gate.py` is reduced to a thin CLI over it (or deleted in favor of `molt_compat --law strict`). `wasm_diff.py` and the (currently nonexistent) luau/llvm runners become *backend adapters* registered with the oracle, not separate tools.

The oracle's **Backend-Divergence sub-oracle** (the highest-yield, CPython-free check from doc 31 §3.2 Oracle 2): for each test, after computing each backend's output, it compares **backends against each other** (`stdout_native == stdout_wasm == stdout_llvm == stdout_luau`), with the same float-tolerance/byte-exact law per program class. A divergence here is a molt-internal bug that requires *zero* CPython runs to localize — and it is the direct witness of a FACT 3 violation.

### FACT 3 — The "Semantic Authority Is Shared" invariant, gated

**Retires:** "a backend re-derives a semantic decision and forks" (the deepest class — it makes whole categories of per-backend divergence *unexpressible* rather than *detected*).

The architecture already routes all four backends through one shared `TirModule` pipeline (doc 00 §4.2) and one kind registry (doc 25). This FACT makes that an *enforced invariant* with two structural rules:

- **Rule 3a — No semantic branch below the registry.** Every parity-relevant semantic fact (does an op throw? does it allocate a fresh owned ref? is it side-effecting? is it a fusion barrier? what is its result repr? what dunder protocol does it dispatch?) is owned by the **op-kind registry** (`runtime/molt-tir/src/tir/op_kinds.toml` → generated classifier tables) — *not* by per-pass/per-backend `matches!`. Doc 00 Risk 7 is the live exemplar and several of these already landed (STATUS.md: `fusion_barrier_opcodes`, `refcount_heap_exposure_opcodes`, `i64_zero_divisor_guard_opcodes`, `literal_payload_opcodes`, `exception_label_attr_opcodes` are all now generated exhaustive classifiers). This arc completes the set for the *remaining* parity-relevant facts and adds the gate (FACT 5) that forbids new `matches!`-on-opcode for a semantic fact.
- **Rule 3b — Backends lower, they do not decide.** A backend (native/wasm/llvm/luau codegen) may differ in *how* it emits an op (Cranelift IR vs WASM bytecode vs LLVM IR vs Luau source) but may not differ in *whether* the op throws / what exception / what value-equivalence class. The Backend-Divergence sub-oracle (FACT 2) is the dynamic witness; a static lint (FACT 5) flags any codegen site that reads program *values* to choose semantics rather than reading the shared fact.

The deep consequence: once FACT 3 holds, "native passes, luau diverges" is not a test we add — it is a state that cannot arise from a conforming op, because the semantic decision was made once, before lowering, in the shared pipeline.

### FACT 4 — The Parity Surface (systematic enumeration of the CPython contract)

**Retires:** "an edge/corner case nobody wrote a test for diverges in production" (unknown unknowns).

The parity surface is a *generated* enumeration of the CPython 3.12+ semantic contract, not a curated test list. It is the cross-product of:

- **Language axis** — grammar productions (from `docs/python_documentation/python-3.12-docs-text/reference/`) × the per-family op portfolio already audited in **doc 30** (operators/dunders, call protocol, classes/metaclass/descriptor/slots/dataclass, closures/scoping, strings/f-strings, iteration, exceptions, pattern matching, decorators/generators/comprehensions, numeric tower).
- **Datamodel axis** — every dunder slot (`__add__`/`__radd__`/`__iadd__`/.../`__getattribute__`/`__set_name__`/`__init_subclass__`/`__prepare__`/`__match_args__`/`__index__`/...) × its CPython-specified fallback chain and error contract.
- **Stdlib axis** — the documented public behavior per module, joined to the existing `docs/spec/areas/compat/surfaces/stdlib/` matrices and **doc 16**'s module inventory.
- **Corner-case axis (the multiplier that catches edges)** — every surface element is crossed with the systematic corner-case dimensions: **empty / singleton / boundary (e.g. `2**46`, `2**63`, max-recursion, max-arity) / exception-path / evaluation-order / version-gated (3.12 vs 3.13 vs 3.14 deltas) / NotImplemented-fallback / reflected-priority / subclass-priority**.

Each surface element gets a stable `surface_id`. The matrix (FACT 1) has a row per `surface_id`. A `surface_id` with no covering test on a dimension is a **named `uncalibrated` cell**, not a void. This converts doc 30's prose gap-findings and doc 16's NotImplementedError inventory into *addressable matrix coordinates*. The generator that emits the surface is `tools/gen_parity_surface.py`, fed by the CPython doc mirror + doc 30's family taxonomy + the spec compat matrices — reusing the *same* generated-vs-hand-edited discipline as `docs/spec/areas/compat/README.md`.

### FACT 5 — The CI parity gate (down-only, fail-closed, all backends)

**Retires:** "a backend diverges and no gate goes red" (the enforcement half of FACT 1).

The gate is the extension of `tools/check_suite_honesty.py` to the full matrix:
- consumes the per-backend calibration JSONL produced by `molt_compat` (FACT 2);
- a `fail` cell without a manifest entry → RED (untracked cross-backend regression);
- a manifest entry that now passes → RED ("remove it, it's fixed" — down-only);
- a `behavior_full` cell that regresses on *any* backend → RED (the P0 demotion rule from `verified_subset_contract.md` §5, generalized across backends);
- an `uncalibrated` cell is *loud* (counted, surfaced) but not blocking until its calibration lane runs — so the gap is measured, never silent;
- **the no-backend-specific-workaround contract** (CLAUDE.md "All backends must have parity. No backend-specific workarounds"): the manifest forbids an entry whose `root_cause` is "backend X only" *unless* it is a documented portable-IR target limitation (the doc 00 §4.5 / doc 14 model: a degradation must be an explicit IR fact, never a hidden exception). The lint rejects "wasm is just different here."

Plus the static half: a lint (`tools/check_semantic_authority.py`) enforcing Rule 3a/3b — no new `matches!`-on-opcode for a registry-owned semantic fact; no codegen site that branches semantics on runtime values.

---

## 3. Concrete Phases (Dependency Order, Each Independently Landable, Green Gates)

The arc decomposes into six phases. Each is a *complete structural piece* (per CLAUDE.md "structural change as the unit of work") that delivers a standalone fact and is independently landable behind green gates. Phases 0→1→2 are the spine (oracle + matrix + ratchet); 3→4→5 exploit it. Phase boundaries are chosen so no phase leaves a hybrid dual-truth state.

### Phase 0 — Unify the comparison law (delete the dual truth) [SPINE]

**Fact delivered:** one comparison law, importable, with the 3-tier mode folded in. Prerequisite for FACT 2; removes a live dual-truth before adding any new runner.

- Extract the comparison law from `tests/molt_diff.py` into `tools/compat/comparison.py`: `compare_stdout(cp, molt, mode)` (byte-exact + `pyperformance` canonicalization from `_canonicalize_stdout` at `molt_diff.py:389`), `compare_stderr(cp, molt, mode)` (the `_stderr_matches`/`_extract_exception_signature` exception-signature law at `:411`/`:423`), `compare_exit(cp_rc, molt_rc)` (the exit-code-compatibility law: CPython 0 ⇒ molt 0; CPython nonzero ⇒ molt nonzero, exact value not required — codify the rule doc 31 §3.2 Oracle 1 documents informally). Mode enum: `byte_exact` (default/STRICT), `relaxed` (the `parity_gate.py` address/refcount normalization at `parity_gate.py:117`), `pyperformance`.
- `tests/molt_diff.py` imports the extracted law (no behavior change — it currently inlines it). `tools/parity_gate.py` is reduced to `molt_compat --law strict` OR kept as a 6-line shim that calls the library; its private `compare`/`_normalize_relaxed` are deleted. The EXCLUDED tier becomes the `intentional_divergence` matrix status, sourced from the manifest + `# molt-parity: excluded` markers (one source).

**Gates:** `pytest tests/test_compat_comparison.py` (new unit tests for each law/mode, including the exception-signature and exit-code-compatibility edge cases); a byte-for-byte regression that `molt_diff.py` on a 50-test sample produces identical RAW statuses before/after the extraction (the law moved, the result must not). G1 lint clean. Down-only honesty ratchet (`tools/check_suite_honesty.py --check`) still green (native unchanged). **No new false greenness:** this phase touches only the *location* of the law, not coverage.

### Phase 1 — `molt_compat`: the single multi-backend oracle [SPINE]

**Fact delivered:** FACT 2. One tool runs every test across `native,llvm,wasm,luau` × `3.12,3.13,3.14`, producing per-backend calibration JSONL in one pass, with the Backend-Divergence sub-oracle.

- Create `tools/molt_compat.py` wrapping the rich `molt_diff.py` executor machinery (daemon custody, `harness_memory_guard`, RSS, dyld-retry, `MOLT_DIFF_RESULTS_JSONL`) — *reuse*, do not fork. Add a **backend adapter registry** `tools/compat/backends/`: `native.py` (the existing `molt build --target native` path), `wasm.py` (lift `wasm_diff.py`'s executor: `molt build --target wasm` + node/WASI runner), `llvm.py` (`molt build --target llvm` + `safe_run.py` on the artifact — gated on the keg-only LLVM toolchain per doc 00 §LLVM toolchain unblock), `luau.py` (`molt build --target luau` source emission + a Luau runtime runner — the first `luau_diff` execution path, which does not exist today). Available-backend detection cached at startup (skip-with-loud-`uncalibrated`, never silent skip).
- Each adapter emits `<backend>_calibration.jsonl` lines with the RAW status (the existing honesty JSONL schema), plus `backend` and `cpython_version` fields so the lines join directly into the matrix.
- **Backend-Divergence sub-oracle** (FACT 2): when ≥2 backends ran a test, compare their stdout to each other under the shared law (float-tolerance for float-containing programs per doc 31 Risk 6, byte-exact otherwise). Emit a `divergence` record naming the disagreeing backends — a zero-CPython bug witness.
- **Version dimension**: drive each backend at the requested CPython baseline via the existing `_molt_sys_env_for_python_exe` (`molt_diff.py:298`) so molt's version-gated messages match; the `# MOLT_META: py/min_py/max_py` gating already handles version-specific tests.
- **SIGURG/host discipline** (doc 31 §1.4): batch + re-exec, per-batch `MOLT_SESSION_ID`, JSONL append-safe; max-2-build-agents honored via the existing diff run-lock (`molt_diff.py:686`).

**Gates:** `molt_compat --backends native --python-version 3.12 <small-suite>` reproduces `molt_diff.py`'s RAW statuses byte-for-byte (proving the wrapper is faithful). `molt_compat --backends native,wasm` on the 4 known wasm-calibrated tests reproduces `wasm_calibration.jsonl`. The Backend-Divergence sub-oracle finds zero false positives on a 100-test deterministic sample (PYTHONHASHSEED=0, MOLT_DETERMINISTIC=1 set on all backends). New unit tests for the adapter registry + divergence record. **Crucially, this phase only *measures* — it cannot regress parity; its risk is false positives, gated above.**

### Phase 2 — The Parity Matrix + the all-backend ratchet [SPINE]

**Fact delivered:** FACT 1 + FACT 5 (the enforcement half). The honesty manifest is generalized to the full `(test × backend × version × profile)` matrix; the gate goes red on any backend.

- Promote `tools/suite_honesty/differential_expectations.json` to the full matrix schema (it already has `<backend>@<version>` dimensions — extend the *coverage*, not the schema). Add the profile axis where it is parity-relevant (release-output vs dev may diverge only as a documented IR fact, never silently — doc 00 §4.5). `honesty_baseline.json` gains per-backend ceilings (down-only each).
- Extend `tools/check_suite_honesty.py`: consume the four `<backend>_calibration.jsonl` files; apply the down-only ratchet per dimension; enforce the **no-backend-specific-workaround lint** (FACT 5: reject an entry whose only justification is "backend X differs" unless it cites a documented portable-IR limitation, mirroring doc 14's "documented target limitation" model). A `behavior_full` cell that regresses on *any* backend is a P0 (generalize `verified_subset_contract.md` §5 across backends).
- Wire the gate into CI as a first-class job and into `0008_MINIMUM_MUST_PASS_MATRIX.md` as a new gate **G3-multi** (the existing G3 is native-only differential; G3-multi adds the per-backend ratchet check, consuming calibration produced by the nightly `molt_compat` deep run). Cold *and* warm are not relevant here (correctness, not perf), but the matrix records per-cell evidence + date (the existing manifest discipline).
- **Calibrate the backlog as a build product, not a survey:** run `molt_compat` to seed `llvm`/`luau` dimensions and the ~1897 UNCALIBRATED stdlib tests on native — turning measured `uncalibrated` cells into measured `fail`/`pass`. Each newly seeded `fail` gets `tracking`+`root_cause`+`evidence` (the manifest lint enforces this). This is where the *true parity surface* becomes known for the first time across all four backends.

**Gates:** `check_suite_honesty --check` green against the seeded matrix; manifest self-lint green (every `fail` has owner+cause+evidence; no overlap with `TOO_DYNAMIC_EXPECTED_FAILURE_TESTS`; no backend-workaround entry without an IR-limitation citation). The matrix is reproducible: a second `molt_compat` calibration run produces the same RAW statuses (determinism — doc 31 Oracle 4 spirit). **This phase makes the parity surface *measured and gated*; from here, every divergence has a red cell.**

### Phase 3 — Enforce "Semantic Authority Is Shared" (close the fork class) [EXPLOIT]

**Fact delivered:** FACT 3 + the static half of FACT 5. The deepest class — per-backend semantic forks become unexpressible.

- **Complete the registry-owned semantic-fact set** (Rule 3a): audit every parity-relevant semantic fact still expressed as per-pass/per-backend `matches!`-on-opcode and migrate it to `runtime/molt-tir/src/tir/op_kinds.toml` → generated exhaustive classifier (the pattern already landed for fusion-barrier / refcount-heap-exposure / i64-guards / literal-payload / exception-label per STATUS.md). The remaining parity-critical facts to verify/migrate: `opcode_may_throw` + `opcode_is_side_effecting` (`effects.rs` — doc 00 Risk 7, the `ModuleImportFrom` exemplar), the FreshValue/owned-ref classifier (doc 30 §1d notes `slice` is in the FreshValue set on LLVM but the *generation* must be registry-owned, not per-backend), and the dunder-dispatch protocol table (so `__add__`/reflected/inplace dispatch is one fact consumed by all four backends — composes with doc 30's Batch A floordiv-schism and doc 34 inplace-op-completeness arcs).
- **Add the static lint** `tools/check_semantic_authority.py` (Rule 3a/3b): fail-closed if (a) a new `matches!` over an `OpCode` is used to derive a registry-owned semantic fact in any pass or backend (the lint knows the registry-owned fact set and flags re-derivation), or (b) a codegen site branches a *semantic* decision (throw / exception type / value-equivalence) on a runtime *value* rather than the shared fact. This is the structural complement to the dynamic Backend-Divergence sub-oracle: the oracle *witnesses* a fork, the lint *forbids* introducing one.
- **The exhaustiveness obligation**: extend the existing `OpCode::ALL` / exhaustive-`match` discipline (doc 00 Risk 7) so that any new opcode forces a decision in every registry classifier *at compile time* (the compiler enforces `match`; the new generated-classifier-coverage test enforces the `matches!`-prone tables, analogous to doc 31 Risk 4's OPCODES-palette-coverage test).

**Gates:** `check_semantic_authority --check` green; generated-classifier-coverage test green (every opcode classified in every parity-relevant table); the Backend-Divergence sub-oracle (Phase 1) finds zero divergences on the calibrated suite (the dynamic proof that the static invariant holds); full `check_suite_honesty --check` still green (no regression). Perf note (CLAUDE.md performance-is-correctness): migrating a `matches!` to a generated table is O(1) lookup — **must be measured to not regress** the touched hot paths (the registry tables are already the hot-path model; a no-perf-regression smoke on the affected ops is required, per the constitution's "every correctness landing answers what it did to speed").

### Phase 4 — The Parity Surface generator + edge/corner systematic coverage [EXPLOIT]

**Fact delivered:** FACT 4. Unknown unknowns become named red cells; coverage is enumerated, not curated.

- Create `tools/gen_parity_surface.py` emitting `surface_id`s from: (a) the CPython grammar/datamodel doc mirror (`docs/python_documentation/python-3.12-docs-text/`), (b) doc 30's family/op taxonomy, (c) the spec compat matrices (`semantic_behavior_matrix.md`, `syntactic_features_matrix.md`, `type_coverage_matrix.md`, stdlib surfaces), crossed with the corner-case axis (empty/singleton/boundary/exception/eval-order/version-gated/NotImplemented-fallback/reflected-priority/subclass-priority). Output is a generated artifact (`docs/spec/areas/compat/surfaces/parity_surface.generated.md` + a machine-readable `parity_surface.json`) under the existing generated-file discipline (no hand-editing of generated cells).
- Join the surface to the matrix (FACT 1): every `surface_id` gets a row; a `surface_id` with no covering test on a backend/version is a **named `uncalibrated` cell**. This is the bridge that converts doc 30's prose gaps (e.g. metaclass `__prepare__` not called §3a, manual `__slots__` not honored §3d, `__index__` coercion §1d/10d, inplace dunders §1e, multi-comparison heap-cell §1b) and doc 16's NotImplementedError inventory into *addressable, gated coordinates*.
- **Generate the edge/corner test skeletons** for the highest-impact uncovered cells (the gaps doc 30 already named as IMPORTANCE=3/GAP=2 and doc 16 ranked top-10). The generator emits differential test stubs (with the correct `# MOLT_META` version gating) into `tests/differential/{basic,stdlib}/`, each immediately runnable by `molt_compat`. This is *systematic* coverage of the corner-case axis — not a fuzzer (doc 31 owns randomized generation; this owns *enumerated* coverage of the documented contract).
- Reconcile with `tests/differential/COVERAGE_INDEX.yaml` + `tools/diff_coverage.py`: the surface becomes the authority the coverage index is *derived from* (one source of truth), closing the "curated index" gap.

**Gates:** `gen_parity_surface --check` (no hand-edit drift in generated cells, mirroring `docs/spec/areas/compat/README.md` generated-vs-hand-edited rule); the surface→matrix join is total (every `surface_id` has a matrix row, every matrix row has a `surface_id`); newly generated edge tests run under `molt_compat` and seed real `pass`/`fail` cells (each `fail` gets owner+cause+evidence). Coverage metric flips from "95.9% of curated features (native)" to "<N>% of *enumerated surface* × *4 backends* × *3 versions*" — a true, honest denominator.

### Phase 5 — Close parity-gap classes structurally (the compression ladder, ongoing) [EXPLOIT]

**Fact delivered:** the monthly retirement of one *class* of divergence, now that the matrix names them. This phase is the *continuous* lane the matrix enables; it composes directly with the already-commissioned fix arcs.

Each entry retires a *class* (not an instance), is one structural arc, and flips a whole group of matrix cells to `behavior_full` across all four backends at once:

- **Datamodel-dispatch class** — route ALL binary/unary/inplace/compare/contains/subscript dunder dispatch through one registry-owned protocol fact (composes with doc 30 Batch A floordiv-schism, doc 34 inplace-op completeness `//= %= **= <<= >>= @=`, doc 30 §1c contains-fast-path, §1d/10d `__index__` coercion). Retires "user-class operator overload diverges by op or by backend."
- **Class-creation class** — metaclass `__prepare__` pre-body hook (doc 30 §3a / doc 35), manual `__slots__` layout (doc 30 §3d / Batch B), `__init_subclass__`/`__set_name__` ordering. Retires "custom metaclass/slots namespace silently wrong."
- **Control-flow-as-data class** — pattern matching lowered to SSA-phi + typed `isinstance` guards instead of heap-cell flags (doc 30 §8 / doc 33), multi-comparison chaining via phi not LIST_NEW (doc 30 §1b). Retires "optimizer-opaque control construct diverges under optimization on some backend."
- **Generator/iterator class** — the os.walk OOM + itertools laziness (doc 16 rank-1/2) close when generator fusion / CoroElide lands (doc 07/D1, doc 00 §4.4). Retires "lazy-iteration semantics differ / OOM."
- **Stdlib-behavior classes** — asyncio runtime-heavy parity (doc 16 rank-4, the 33 seeded asyncio fails in the honesty manifest), regex advanced features (doc 16 rank-5 / doc 37), dir_fd already landed (doc 19). Each module's documented behavior becomes a `behavior_full` row group.

**Gates per class:** the relevant matrix cell-group flips to `behavior_full` on **all four backends** simultaneously (the no-backend-specific-workaround contract); the down-only ratchet drops the corresponding ceilings; the Backend-Divergence sub-oracle stays at zero divergences; and — per the performance constitution — the structural fix lands with its perf measurement on all targets (a parity fix that regresses a benchmark is INCOMPLETE; doc 30 already pairs each parity gap with its perf consequence, e.g. the floordiv schism is *both* a correctness and a perf gap).

---

## 4. Verification / Gates Per Phase (Measurement Discipline + Parity Oracle)

The arc's verification doctrine (binding, mirrors CLAUDE.md's measurement discipline and the honesty-ratchet model):

**Parity oracle law (one law, all phases).** Equivalence = identical stdout (byte-exact, or float-tolerant/`pyperformance`-canonical for the declared program class) **and** identical exception type+message (the `exception_signature` law, ignoring frame/path formatting) **and** exit-code-compatible (CPython-0 ⇒ molt-0; CPython-nonzero ⇒ molt-nonzero). This is `tools/compat/comparison.py` (Phase 0), the *only* comparison law in the tree after Phase 0.

**Determinism gate (every calibration).** All backends run with `PYTHONHASHSEED=0` + `MOLT_DETERMINISTIC=1`; dict/set output canonicalized; a calibration is only authoritative if a clean isolated (`jobs=1`, quiet-host) re-run reproduces it — the manifest's `_calibration_scope` already documents that ~85% of contended parallel fails were build-contention false-fails and were discarded. This arc *codifies* that rule: a `fail` cell requires a reproduced-in-isolation witness in its `evidence` field.

**Down-only ratchet (every phase, fail-closed).** `check_suite_honesty --check` is green at every phase boundary. A cell may only go `fail → pass` (debt retired) or `uncalibrated → measured`; never `pass → fail` silently (that is a RED untracked regression) and never `fail → silently-removed` (a fixed entry left in the manifest is RED).

**Backend-Divergence as the FACT 3 witness (Phases 1+).** Zero cross-backend stdout divergences on the calibrated suite is the dynamic proof that Semantic-Authority-Is-Shared holds. A nonzero divergence is a P0 (it means a semantic fork exists that the static lint missed — fix both the fork and the lint gap).

**No-backend-specific-workaround contract (Phases 2+, manifest-enforced).** Per CLAUDE.md "All backends must have parity. No backend-specific workarounds": a manifest entry justified only by "backend X differs" is rejected unless it cites a documented portable-IR target limitation (doc 14 / doc 00 §4.5 model). Parity is closed on all four backends together or not claimed.

**Perf-correctness coupling (Phases 3+, per the performance constitution).** Every structural parity fix lands with its perf measurement across native/WASM/LLVM/Luau and dev-fast/release-fast/release-output (`benchmark → target → backend → profile → CPython ratio → … → command/log artifact`). A parity fix that introduces a benchmark regression is INCOMPLETE (the floordiv-schism class is the canonical example — it is simultaneously a parity bug and a perf cliff; both must close together).

**Self-test of the harness (Phases 0–1).** The oracle is itself tested: Phase 0 proves the extracted law reproduces inline `molt_diff` results byte-for-byte; Phase 1 proves the `molt_compat` wrapper reproduces native `molt_diff` and the existing `wasm_calibration.jsonl`. A harness that cannot reproduce known results cannot certify new ones.

---

## 5. How It Composes With the Decomposition (21a–e) and the Parallel Multi-Agent Model

**Composition with the decomposition program (doc 21 / 21a–21e).** This arc is mostly *additive tooling + a registry-fact completion*, which keeps it orthogonal to the god-file decomposition:
- The **frontend semantics** authority (`src/molt/frontend/__init__.py`, ~26k lines) is being split by **doc 21c** (frontend mixin decomposition). This arc *reads* that surface (it never adds frontend code except the Phase-5 fix arcs, which are separately commissioned in docs 33/34/35) and *benefits* from the split: a smaller, mixin-decomposed visitor makes the Phase-3 "no semantic re-derivation" lint tractable. **Dependency edge:** Phase 3's lint should land after (or co-design with) 21c so it lints the decomposed modules, not a moving 26k-line file.
- The **op-kind registry** (doc 25) is the substrate for FACT 3; the registry's phase-2 (`op_kinds.toml` generation) is the landing venue for the remaining semantic-fact classifiers (doc 30 §Batch references it explicitly). **Dependency edge:** Phase 3 composes with doc 25 phase-2.
- The **CLI package** decomposition (21d) and **crate graph** (21b) own the build paths `molt_compat`'s backend adapters call (`molt build --target {native,wasm,llvm,luau}`); the adapters use the *public* CLI contract, so they survive the CLI decomposition unchanged.
- The **satellite-parity guard** (`check_satellite_parity.py`) is the *intra-runtime* dual-copy guard the decomposition program created; FACT 5's inter-backend guard is its sibling generalization. They share the "fail-closed contract, down-only ratchet, normalize-then-compare-residual" pattern — Phase 2 should reuse that pattern's code where possible.

**Composition with the parallel multi-agent execution model (doc 00 three-lane + the A/B/C council lanes).** The arc maps cleanly onto the non-overlapping-files lane model:
- **Lane C (infra/scoreboards/decomposition)** owns Phases 0–2 + 4 (the oracle, matrix, ratchet, surface generator) — these are *tooling and contracts*, touch `tools/` + `tests/differential/` + `docs/spec/areas/compat/`, and do not collide with the correctness (Lane A) or perf (Lane B) frontiers. This is the natural home: doc 00 says "C is never decorative" — the parity matrix is exactly the measurement path that makes A and B's parity claims trustworthy.
- **Lane A (P0 semantic safety)** owns the Phase-3 semantic-authority invariant (it is a correctness invariant) and the Phase-5 finalizer/generator parity classes (which intersect the RC/ownership front).
- **Lane B (performance frontier)** owns the perf-coupling of every Phase-5 fix (the floordiv-schism/contains-fast-path/inplace-op classes are doc 30's *fix-only perf batches*).
- **Build discipline (CLAUDE.md):** `molt_compat`'s multi-backend calibration is build-heavy (4 backends × N tests); it MUST honor max-2-build-agents (the existing `molt_diff` run-lock at `molt_diff.py:686`), per-agent `MOLT_SESSION_ID`, and never run raw binaries (luau/llvm artifact execution routes through `tools/safe_run.py`). The nightly deep calibration is the natural place for the full 4-backend sweep; PR-time runs a native + (if available) wasm smoke, matching doc 31's PR-smoke/nightly-deep tiering.
- **The matrix is the coordination artifact.** Like doc 00 is the master schedule for the foundation program, the parity matrix becomes the master schedule for the parity arc: any agent can pick an `uncalibrated`/`fail` cell-group, own it, and flip it green — the manifest's `tracking`/`root_cause`/`evidence` fields are the per-cell baton.

---

## 6. Risks + Structural (Not Band-Aid) Treatment

**Risk 1 — The oracle becomes a third dual-truth instead of unifying the two.**
*Band-aid:* add `molt_compat` alongside `molt_diff`+`parity_gate` and let all three drift.
*Structural treatment:* Phase 0 *extracts* the one comparison law into a library and *reduces* `parity_gate.py` to a mode of it before Phase 1 adds any backend; `wasm_diff.py`/luau/llvm become *adapters registered with the one oracle*, not standalone tools. The acceptance gate is "byte-for-byte reproduction of existing results" — the law moved, results must not. There is exactly one comparison law and one runner after Phase 1.

**Risk 2 — Calibration false-fails from build contention poison the matrix.**
*Band-aid:* accept a noisy survey as the matrix.
*Structural treatment:* the determinism gate (a `fail` requires a reproduced-in-isolation witness in `evidence`) is the same rule the existing honesty manifest already learned the hard way (`_calibration_scope` documents discarding ~85% contended false-fails). The matrix is fed by *quiet-host isolated `jobs=1`* calibration; contended runs are surveys, not authority. This is codified in the manifest lint, not left to operator discipline.

**Risk 3 — Backend-specific divergence gets parked as an "accepted" matrix entry.**
*Band-aid:* mark luau/wasm divergences as `intentional_divergence` to make the gate green.
*Structural treatment:* `intentional_divergence` is restricted to the *documented* carve-out set (the 4 dynamic-execution exclusions + `semantic_behavior_matrix.md` §7's explicit memory-layout/refcount-cycle/bytecode/stack-depth divergences). The no-backend-specific-workaround lint (FACT 5) rejects any *new* `intentional_divergence` that is really "backend X is just different" — it must cite a documented portable-IR limitation (doc 14 model) or it is a `fail` debt with an owner. Parity is all-four-backends or not claimed (CLAUDE.md).

**Risk 4 — The semantic-authority lint (Phase 3) has false positives / negatives.**
*Band-aid:* a heuristic grep for `matches!` that misses re-derivations or flags benign ones.
*Structural treatment:* the lint is grounded in the *registry's own fact set* (it knows exactly which semantic facts are registry-owned), and is paired with the *dynamic* Backend-Divergence sub-oracle as the ground-truth witness. A false negative in the lint surfaces as a real divergence in the oracle (and triggers a lint-gap fix); a false positive is resolved by either migrating the fact to the registry (the correct fix) or proving the `matches!` is a *lowering* not a *semantic* decision (the Rule 3b distinction). The two halves cross-check each other — neither alone is authority.

**Risk 5 — The parity surface generator (Phase 4) generates an unbounded or noisy surface.**
*Band-aid:* enumerate a combinatorial explosion of meaningless cells, drowning the matrix.
*Structural treatment:* the surface is *bounded by the documented contract* (CPython reference + datamodel slots + stdlib documented behavior + doc 30's already-curated family taxonomy), crossed with a *fixed, finite* corner-case axis (8 dimensions). It is generated under the same generated-vs-hand-edited discipline as the existing spec matrices, so it is reviewable and stable. Priority is driven by doc 30/16's *already-ranked* IMPORTANCE×GAP scores — the generator emits *all* cells but the Phase-4 test-skeleton emission targets the ranked-high cells first. The denominator is honest (enumerated), the work order is prioritized (ranked).

**Risk 6 — The 4-backend calibration is too build-expensive to run in CI.**
*Band-aid:* run native-only in CI and never gate the other backends (the current state).
*Structural treatment:* the doc-31 tiering model — PR-smoke (native + wasm-if-available, small) is fast and blocking on regressions; nightly-deep (all 4 backends, full surface) is the authority that seeds the matrix; the matrix is *committed*, so the PR-time `check_suite_honesty --check` reads the committed matrix without rerunning the suite (the existing `--check`-never-runs-the-suite separation). Build cost is paid nightly, not per-PR. The `<2MB/cold-start` and DX-build-speed arcs (doc 08, doc 00 §4.3) reduce the per-build cost over time, compounding.

**Risk 7 — A parity fix (Phase 5) closes correctness but regresses perf on one backend.**
*Band-aid:* land the parity fix, file the perf regression as "later."
*Structural treatment:* the performance constitution is binding — a parity fix that regresses a benchmark is INCOMPLETE. Doc 30 already pairs each parity gap with its perf consequence (floordiv schism, contains dispatch, string O(n²), enumerate/zip allocation), so the Phase-5 arcs are *jointly* parity+perf arcs by construction. The landing report format (CLAUDE.md) requires the full perf matrix green alongside the parity cells. The matrix and the perf scoreboards are checked together at every Phase-5 landing.

**Risk 8 — `matches!`-on-opcode silent-default reintroduces a fork after Phase 3 (the regression of the regression).**
*Band-aid:* fix the current `matches!` sites and hope no new ones appear.
*Structural treatment:* the exhaustiveness obligation makes a *new* opcode force a decision in every registry classifier at compile time (compiler-enforced `match` + the generated-classifier-coverage test for the `matches!`-prone tables, the doc-31-Risk-4 OPCODES-palette-coverage-test pattern). The static lint (`check_semantic_authority`) blocks *new* `matches!`-on-opcode for a registry-owned fact. The class is closed structurally: you cannot add an opcode without classifying it everywhere, and you cannot re-derive its semantics per-backend.

---

## 7. Build Sequence Summary (Dependency-Ordered, One-Line Per Phase)

```
Phase 0  [Lane C]  Extract ONE comparison law (tools/compat/comparison.py); fold parity_gate's
                   3-tier into it as a mode; delete the dual truth.            [no coverage change]
Phase 1  [Lane C]  tools/molt_compat.py: one oracle, --backends native,llvm,wasm,luau ×
                   --python-version, per-backend calibration JSONL + Backend-Divergence sub-oracle.
Phase 2  [Lane C]  Generalize the honesty manifest+ratchet to the full (test×backend×version×profile)
                   matrix; CI gate G3-multi; calibrate llvm/luau + the ~1897 uncalibrated stdlib cells.
Phase 3  [Lane A]  Enforce Semantic-Authority-Is-Shared: complete registry-owned semantic facts +
                   tools/check_semantic_authority.py (Rule 3a/3b) + exhaustiveness obligation.
Phase 4  [Lane C]  tools/gen_parity_surface.py: enumerate the CPython contract × corner-case axis;
                   join to the matrix; generate ranked-high edge/corner test skeletons.
Phase 5  [Lanes A+B]  Close parity-gap CLASSES structurally (compose with docs 33/34/35/07/16/30/37),
                   each flipping a cell-group to behavior_full on ALL four backends + perf-green.
```

**The single highest-leverage ordering:** unify the comparison law (Phase 0) → build the one multi-backend oracle (Phase 1) → make the parity matrix measured-and-gated across all four backends (Phase 2) → make per-backend semantic forks *unexpressible* (Phase 3) → enumerate the surface so unknowns become named cells (Phase 4) → retire one divergence *class* per month on the now-measured matrix (Phase 5). Phases 0–2 are the spine and unblock everything; they are pure measurement+contract (cannot regress parity, only reveal it). Phase 3 is the deep structural win (closes the fork class). Phases 4–5 are the continuous compression ladder. **Correctness parity is the floor; this arc makes the floor *measured, gated, and unforgeable across every backend* — which is the precondition for the perf-dominance product contract on top of it.**
