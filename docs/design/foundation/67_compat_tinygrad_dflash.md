<!--
arc: TINYGRAD + DFLASH FIDELITY (CLAUDE.md "Top Priority", turn-blocking)
author: portfolio-architect
date: 2026-06-23
status: design only / executable plan
doc: 53
companions: docs/superpowers/specs/2026-04-14-tinygrad-gpu-primitives-design.md;
  docs/superpowers/plans/2026-04-14-tinygrad-gpu-primitives-plan-{1..4}.md;
  docs/superpowers/archive/2026-04-25-tinygrad-gpu-canonicalization{,-design}.md;
  docs/design/foundation/51_ten_year_roadmap.md (north star);
  docs/design/foundation/46_semantic_control_plane.md (fact-plane);
  docs/design/foundation/16_cpython-surface-stdlib-gpu-gap-audit.md;
  docs/design/foundation/21*_decomposition (god-file split);
  docs/design/foundation/00_integrated_parallel_program.md (lane model).
note: design only. Do NOT build or commit from this doc; the lead integrates.
-->

# 53 — Tinygrad + DFlash Fidelity: the parity-as-a-fact compression ladder

**Arc owner role:** `compat/ml` (tinygrad public contract + DFlash algorithmic fidelity).
**Constitution anchor:** CLAUDE.md §"Top Priority: Tinygrad + DFlash Fidelity"
(turn-blocking) + §"ABSOLUTE NON-NEGOTIABLE: Zero Workarounds" + the Performance
Constitution. North star: doc 51 §0 ("Run everywhere tinygrad runs and more — GPU/ML
(tinygrad + DFlash) is a first-class, exact-fidelity product, not a bolt-on").

---

## 0. END-STATE (stated crisply, the time-traveler's destination)

Five-to-hundred years out, the following are **structurally true and impossible to
violate silently**:

1. **`import tinygrad` IS upstream tinygrad's public contract, byte-for-byte in
   semantics.** Every public name, signature, default, dtype-promotion rule, NaN/inf
   edge, reduce/movement/elementwise numeric result, and error type under the `tinygrad`
   package equals the *pinned upstream tinygrad revision* — and that equality is
   **mechanically proven on every CI run by executing the same program through both
   stock CPython+upstream-tinygrad and through molt**, never asserted by a human-written
   table. molt is faster on every one of those programs (Performance Constitution),
   across native / WASM / LLVM / Luau and every profile.

2. **`molt.gpu` is provably a pure *implementation substrate* of that contract — it can
   never introduce a second, divergent semantics.** Any tinygrad-owned behavior that
   `molt.gpu` re-expresses is checked, at build time, to be *delegation-equivalent* to
   the tinygrad surface (one authority, doc 49 discipline applied to the ML surface).

3. **`molt.gpu.dflash` IS DFlash — target-conditioned block-diffusion drafting with
   verifier/drafter separation, hidden-feature conditioning, KV injection, and a *real
   trained drafter* — or it raises.** Generic speculative decoding can never be imported,
   resolved, or run under the DFlash name. "No trained drafter present" is a first-class,
   typed, fail-closed state, never a silent fallback to a faked drafter.

4. **Drift is *unexpressible*, not *discouraged*.** The parity contract is a **generated,
   checkable FACT** (doc 51's "semantic fact plane" applied to the ML surface): a
   build-gated artifact derived from the pinned upstream source + the DFlash paper's
   algorithm, with an Alive2-style obligation per fact (doc 51 §1 "no fact without a
   validator"). A molt change that drifts from upstream tinygrad or from DFlash fidelity
   fails a gate *mechanically* — the same way `tools/gen_op_kinds.py --check` makes a
   mis-classified opcode unbuildable.

**The class this arc retires (the compression-ladder rung):** *"silent ML-contract
drift"* — the entire class of bugs where molt's ML surface diverges from its declared
source-of-truth (upstream tinygrad numerics/API, or DFlash algorithm) **without a gate
catching it**, including the subtler class *"fidelity theater"* (a real-looking adapter
or op table that is actually a hand-maintained approximation that rots silently against
the upstream pin). After this arc, both classes are made impossible by construction:
parity is derived and checked, not declared.

---

## 1. WHERE WE ARE (investigated; cite-anchored ground truth)

This arc must **advance and compose** with substantial existing work, not duplicate it.
The current state, verified against the tree on 2026-06-23:

### 1.1 What exists and is load-bearing (keep, build on)

- **The primitive design + plans** — `docs/superpowers/specs/2026-04-14-tinygrad-gpu-primitives-design.md`
  (the "3 OpTypes / 26 primitives / ShapeTracker / LazyOp / fusion / renderers / MLIR"
  architecture) and Plans 1–4. Plan 4 §Task 4 already states the DFlash provenance and
  the no-fake rule. **Status: implemented in substantial part** (below).
- **The Rust GPU crate** `runtime/molt-gpu/` exists with the intended module shape:
  `ops.rs`, `dtype.rs`, `shapetracker.rs`, `lazy.rs`, `schedule.rs`, `fuse.rs`,
  `dce.rs`, `mlir.rs`, `render/`, `device/`. This matches the spec §6 target layout
  (the spec's "NEW CRATE" already landed).
- **The runtime intrinsic boundary** `runtime/molt-runtime/src/builtins/gpu_primitives.rs`
  (2,578 lines) exposes `molt_gpu_prim_*` intrinsics: `binary`, `unary`, `ternary`,
  `cast`, `reduce`, `reduce_all`, `reshape/permute/expand/pad/shrink/flip`, `contiguous`,
  `create_tensor{,_raw}`, `read_data{,_raw}`, `realize`, `zeros{,_dtype}`, `device`,
  `dtype`, `shape`, `numel`, `nbytes`, `free`, `tensor_count`. This is the
  Python↔Rust seam the Tensor class calls.
- **The Python tinygrad shim** `src/molt/stdlib/tinygrad/**` — the canonicalization doc
  (`2026-04-25-...`) declares the binding rule: *"Any API exposed under the `tinygrad`
  package must match the targeted upstream tinygrad signature, call shape, and numerical
  behavior… If Molt does not yet implement a required tinygrad semantic, it must raise a
  clear unsupported error."* Leaves present: `tensor.py`, `dtypes.py`, `device.py`,
  `lazy.py`, `realize.py`, `nn/`, plus higher-level ML modules (`flash_attention.py`,
  `eagle.py`, `kv_cache.py`, `tree_attention.py`, `turbo_quant.py`, `speculative.py`,
  `onnx_interpreter.py`, `paddleocr*.py`, `whisper_demo.py`, examples).
- **The DFlash adapter contract trio** `src/molt/gpu/dflash/{contracts,adapters,runtime}.py`
  + `__init__.py`. This is **already well-architected and fail-closed**:
  - `contracts.py:39` `DFlashConditioning(SpeculativeConditioning)` **requires**
    `target_features`, `target_kv`, `position_ids`, `last_verified_token` (raises
    `ValueError`/`TypeError` otherwise). `require_dflash_conditioning` (`:77`) enforces
    the same at every boundary. `DFlashRuntime` (`:142`) requires callable
    `draft_step`/`verify_step` + a validated `DFlashConditioning`.
  - `adapters.py` is a typed registry (`DFlashAdapterSpec`, `register/resolve/
    build_dflash_runtime`) — model-specific adapters register; absence resolves to
    `None`/`LookupError`, never a generic fallback.
  - `runtime.py` has `speculative_decode_greedy_conditioned` (`:143`) which
    **re-validates `DFlashConditioning` on every refreshed conditioning** when the
    initial conditioning is DFlash (`:209-215`) — verifier/drafter separation with
    target-owned conditioning is wired in.
- **The fail-closed alias** `src/molt/stdlib/tinygrad/dflash.py` raises `ImportError`
  pointing at `molt.gpu.dflash` (paper-faithful) vs `tinygrad.speculative` (generic) —
  exactly the no-mislabel guard the constitution demands.
- **The pinned upstream oracle is already in-tree:**
  `bench/friends/repos/tinygrad_off_the_shelf/` is **tinygrad 0.13.0** (verified
  `pyproject.toml`: `name="tinygrad" version="0.13.0" authors=[George Hotz]`), with full
  source: the op enum `tinygrad/uop/__init__.py:13 class Ops(FastEnum)` and the backend
  op contract `tinygrad/renderer/cstyle.py:~140 code_for_op: dict`.
- **The test oracle harness** `tests/helpers/tinygrad_stdlib_loader.py` loads the molt
  stdlib tinygrad leaves in isolation with mockable intrinsics — the scaffold a
  differential oracle plugs into.

### 1.2 The gaps this arc must close (the drift surface)

These are the concrete holes between "looks faithful" and "provably faithful". Each is
tied below to a structural fact, not a patch.

1. **The "26 primitives == tinygrad's `code_for_op`" claim is already DRIFTED against the
   pinned 0.13.0 source — and nothing catches it.** Verified divergences in
   `bench/friends/repos/tinygrad_off_the_shelf/tinygrad/renderer/cstyle.py` `code_for_op`
   and `tinygrad/uop/__init__.py` `Ops`:
   - Upstream `code_for_op` uses **`Ops.CMOD`** (`a%b`) and **`Ops.CDIV`** (`a/b`) — the
     design doc names these **`MOD`** and **`IDIV`**. The names diverge from the pin.
   - Upstream `code_for_op` has **no `MAX` entry** and **no `IDIV`/`MOD` entry**; `MAX`
     and integer div/mod are produced by upstream pattern rewrites, not the renderer dict.
     The design's "every op in tinygrad's renderer is a primitive here, no fewer no more"
     is therefore **false against 0.13.0**.
   - Upstream `Ops` additionally has `FDIV`, `POW`, `THREEFRY`, `FLOORDIV`, `FLOORMOD`,
     `SUB`, `MULACC`, `WMMA` as ALU/math members. The design enumerates a hand-curated 26
     and explicitly *drops* `MULACC` (composed) — a defensible engineering choice, but it
     is **a choice not pinned to and checked against the upstream revision**. That is the
     exact "fidelity theater" failure mode: a reasonable-looking hand table that *will*
     rot when upstream bumps.
   This is the keystone finding: **the parity claim is human-asserted in prose, so it is
   already wrong and undetectably so.** The fix is not "edit the 26 to 28" — that is a
   bug-instance patch. The fix is to make the primitive set + their semantics a
   **generated fact derived from the pin** (§3.1).

2. **No differential oracle exists.** The tinygrad tests in `tests/` (`test_tinygrad_*`,
   `test_gpu_*`) encode **hand-written expected values and signatures**, not a
   side-by-side execution against `bench/friends/repos/tinygrad_off_the_shelf`. Confirmed:
   the off-the-shelf 0.13.0 source is never imported by any test as an oracle. So
   numeric/API parity is *claimed*, never *measured against the source of truth*. This is
   precisely the prohibited "no compatibility claim without fresh command output"
   (canonicalization doc §Verification) being unmet at the system level.

3. **No DFlash differential / fidelity tests exist at all.** `find tests -iname '*dflash*'`
   → empty. The adapter contract is sound, but there is **zero executable proof** that
   (a) the conditioning fields are actually *consumed* the way the paper requires
   (KV injection into draft layers, hidden-feature fusion), (b) the verifier/drafter
   loop is lossless against a reference, or (c) the "no trained drafter ⇒ raise" path is
   exercised. Adapter scaffolding without a fidelity oracle is "status, not state change"
   (Council Tranche standard) for the *algorithm* even though the *contract* is real.

4. **`molt.gpu` ↔ `tinygrad` single-authority is asserted, not enforced.** The
   canonicalization doc says `molt.gpu` "must not define alternate semantics for
   tinygrad-owned behavior," and prior drift (`molt.gpu.nn.Conv2d`) was hand-removed. But
   there is **no gate** preventing the next divergence — exactly the
   `duplicate_authorities` problem the Structural Audit Board (doc 46 instrument) tracks
   for op-kinds, not yet applied to the ML surface.

5. **`runtime/molt-runtime/src/builtins/gpu.rs` is an 11,817-line god-file**
   (`STRUCTURAL_AUDIT_BOARD.md` row: *medium, ceiling 4000*). It is the legacy ad-hoc GPU
   pipeline the primitive spec §5.1 marked for deletion (the spec lists
   `molt-runtime/src/builtins/gpu.rs | 8990 | Replaced by Tensor + MoltDevice`). It has
   since *grown*, not shrunk — the replacement landed *alongside* the legacy file instead
   of deleting it. This is a live "two parallel sources of truth" violation
   (CLAUDE.md §"Splitting an atomic refactor"). It composes directly with the 21*
   decomposition program and must be retired here, not left.

### 1.3 The one-line diagnosis

The ML surface has **real structure** (crate, intrinsics, adapter contract) but its
**fidelity is declared in prose and hand-written tests**, so drift is *invisible*. The
arc converts every fidelity claim into a **derived, gated FACT**.

---

## 2. METHOD: parity as a fact, not a promise (time-traveler back-cast)

Working backward from §0: for drift to be *unexpressible*, the chain of necessity is:

```
END-STATE: drift impossible
  ⇐ every fidelity claim is a build-GATED check (fails the build on divergence)
    ⇐ every claim is DERIVED from a pinned source-of-truth artifact, not hand-written
      ⇐ there exists a single pinned upstream-tinygrad revision + a single pinned
        DFlash algorithm spec, each with a machine-readable extraction
        ⇐ there exists a DIFFERENTIAL ORACLE that executes the same program through
          {stock CPython + upstream tinygrad} and {molt}, and a DFLASH REFERENCE
          that executes the paper algorithm, both bit/tolerance-comparable
```

So the build order is forced: **(A) pin + extract → (B) oracle harness → (C) generated
parity facts + gates → (D) substrate single-authority gate → (E) DFlash fidelity facts +
reference → (F) god-file retirement → (G) perf contract closure.** Each is independently
landable with its own green gate, and each adds *one fact family* that makes one slice of
drift unexpressible (doc 51 cadence: one class/month).

This is the **same machinery doc 51 §1 prescribes** (the semantic fact plane:
"generated, checkable facts… each makes a CLASS unexpressible") and doc 46 implements for
op-kinds (`gen_op_kinds.py --check`). We are adding the **`gpu_contract` fact family** to
that plane. We reuse the exact discipline: *discovery may be heuristic; authority may not*
(STRUCTURAL_AUDIT_BOARD §Discovery-vs-authority rule).

---

## 3. THE STRUCTURAL FACTS / MECHANISMS TO BUILD (each tied to the class it retires)

These are the new first-class artifacts. They are the deliverable; the phases (§4) build
them in dependency order.

### 3.1 FACT FAMILY 1 — `gpu_op_contract` (the primitive-set parity fact)

**Retires:** the class "molt's GPU primitive set / op semantics silently diverge from the
pinned upstream tinygrad op contract" (the §1.2.1 finding generalized).

**What it is:** a generated, checked registry `runtime/molt-gpu/op_contract.toml`
(authoritative) + a generator `tools/gen_gpu_op_contract.py` that **reads the pinned
upstream source** (`bench/friends/repos/tinygrad_off_the_shelf/tinygrad/uop/__init__.py`
`Ops` enum + `renderer/cstyle.py` `code_for_op`) and emits, per molt primitive:

- the upstream `Ops` member it corresponds to (or `COMPOSED_FROM = [...]` with the exact
  upstream op chain, e.g. molt `DIV` → `MUL(x, RECIPROCAL(y))`, or `MULACC` →
  `ADD(MUL(a,b),c)`);
- the upstream renderer C-pattern (string from `code_for_op`) it must match;
- the dtype rule (e.g. `CMPLT/CMPNE/CMPEQ → dtypes.bool`), cross-referenced to the
  upstream `dtypes` promotion logic;
- the IEEE-754 edge contract (NaN/inf/`-0.0`) the spec §2.2 enumerates, now *anchored to
  the upstream op* rather than free-floating prose.

**The gate:** `tools/gen_gpu_op_contract.py --check` (mirrors `gen_op_kinds.py --check`)
fails the build if (a) molt's `runtime/molt-gpu/src/ops.rs` `PrimitiveOp` enum is not a
*provable cover* of the upstream op set under the declared composition mapping, or (b) the
upstream pin's op set changed and the contract was not regenerated. **This is what makes
"the 26 are right" a checked fact instead of a prose claim** — and it would have caught
the §1.2.1 `CMOD`/`CDIV`/`MAX` divergence on day one.

**The Alive2-style obligation (doc 51 §1):** for each `COMPOSED_FROM` entry, a property
test asserts the composition equals the upstream op *numerically on a fuzzed input grid
including all IEEE edges* (e.g. molt `DIV` vs upstream `FDIV` over `{0, -0, inf, -inf,
nan, denormals, ±max}`). A composition that is not numerically equal to the upstream op
it claims to replace is a hard failure, not a tolerance fudge.

### 3.2 FACT FAMILY 2 — `gpu_api_contract` (the Tensor/nn surface parity fact)

**Retires:** the class "a public `tinygrad.*` name/signature/default/error-type drifts
from the pinned upstream without detection" (§1.2.2 generalized to the API surface).

**What it is:** a generated `src/molt/stdlib/tinygrad/api_contract.json` + generator
`tools/gen_tinygrad_api_contract.py` that **introspects the pinned upstream tinygrad
0.13.0** (`bench/.../tinygrad/`) and records, for every public symbol molt claims to
support: fully-qualified name, signature (params, defaults, kw-only-ness), and the
*declared support status* (`supported` / `raises-unsupported`). A molt symbol marked
`supported` whose signature differs from the pin is a gate failure; a *new* public
upstream symbol not present in molt's contract is surfaced as `unclassified` (must be
either implemented or explicitly marked `raises-unsupported` — never silently absent).

**The gate:** `tools/gen_tinygrad_api_contract.py --check`. This is the API-shape twin of
3.1's numeric gate. Together they enforce CLAUDE.md "Exact tinygrad semantics AND API
shape are the public ML contract."

### 3.3 MECHANISM 3 — the tinygrad differential oracle

**Retires:** the class "parity is asserted by hand-written expected values that rot"
(§1.2.2).

**What it is:** `tools/tinygrad_diff_oracle.py` + a pytest plugin
`tests/gpu/parity/conftest.py` providing a `tinygrad_parity` fixture. Given a
**parametrized corpus of tinygrad programs** (`tests/gpu/parity/corpus/*.py` — pure
tinygrad public-API snippets), it:

1. executes each through **stock CPython importing `bench/.../tinygrad_off_the_shelf`**
   (the 0.13.0 pin) on the CPU/CLANG reference device → captures `.numpy()`/`.tolist()`
   outputs + raised exception types;
2. executes the *same snippet* through **molt** (`molt run`/`molt build` per
   CLAUDE.md Safe Execution, via `tools/safe_run.py`) importing molt's `tinygrad` shim,
   across the target matrix (native, WASM, LLVM, Luau where the device exists);
3. asserts equality: **bit-exact for integer/movement ops; ULP-bounded (default 0 ULP for
   the ops upstream renders as the identical C expression, documented ULP for `EXP2`/
   `LOG2`/`SIN` where the host libm differs) for float**; identical exception *type* for
   error cases.

**Why this is the oracle, not the existing tests:** it removes the human from the
expected-value loop entirely. The source of truth is *the pin executing the same bytes*.
Corpus coverage is itself a tracked, ratcheting metric (every public op + every
composition in 3.1 must appear in the corpus). This satisfies the canonicalization doc's
"no compatibility claim without fresh command output" at the system level, every CI run.

### 3.4 FACT FAMILY 4 — `gpu_substrate_authority` (the single-authority fact)

**Retires:** the class "`molt.gpu` grows a second, divergent semantics for a
tinygrad-owned behavior" (§1.2.4).

**What it is:** an extension to the Structural Audit Board's `duplicate_authorities`
probe (doc 46 / `tools/structural_audit.py`) that treats the **`tinygrad` package as the
sole authority for ML semantics** and flags any `src/molt/gpu/**` symbol that
*re-implements* (rather than *delegates to*) a tinygrad-owned behavior. Concretely: a
manifest `src/molt/gpu/AUTHORITY.toml` declares, per `molt.gpu` public symbol, either
`delegates_to = "tinygrad.<x>"` (must call through) or `substrate_only = true` (buffers/
intrinsics/device — no tinygrad-visible semantics). The gate (folded into
`structural_audit.py --check`) fails if a `delegates_to` symbol contains an independent
numeric implementation, keeping `duplicate_authorities` at 0 *for the ML surface too*
(it is already 0 globally per the board; this prevents regression on the ML axis).

### 3.5 FACT FAMILY 5 — `dflash_fidelity` (the DFlash algorithm fact)

**Retires:** the classes "generic speculative decoding is mislabeled DFlash" and "DFlash
fidelity is claimed without an executable check against the paper algorithm" (§1.2.3).

**What it is:** the missing *algorithmic* half on top of the existing *contract* half
(`src/molt/gpu/dflash/`). Three artifacts:

1. **`src/molt/gpu/dflash/SPEC.md`** — a machine-checkable transcription of the DFlash
   algorithm from the pinned provenance (arXiv:2602.06036; z-lab.ai/projects/dflash),
   enumerating the **non-negotiable fidelity invariants** as named obligations:
   - `F1 target_conditioning`: the drafter forward pass *consumes* `target_features`
     (hidden-feature fusion), not just receives them.
   - `F2 kv_injection`: target hidden features are *injected into each draft layer's KV
     cache* (the paper's mechanism), verifiable by a KV-state assertion.
   - `F3 verifier_drafter_separation`: target verification logic and draft logic are
     distinct callables with target-owned conditioning (already structurally true via
     `DFlashRuntime.draft_step`/`verify_step`; SPEC pins it as an invariant).
   - `F4 block_diffusion`: drafting produces a *block* of candidate tokens per step via
     the block-diffusion process (not autoregressive single-token), conditioned on the
     last verified token.
   - `F5 losslessness`: the verified output token stream equals greedy target-model
     decoding (the speculative-decoding correctness guarantee) — checked against a
     reference target run.
   - `F6 trained_drafter_required`: a DFlash runtime cannot be built from an untrained /
     absent drafter; `F6` is the typed fail-closed state.
2. **`tests/gpu/dflash/test_dflash_fidelity.py`** — one test per `F1..F6`, plus the
   **fail-closed corpus**: attempts to (a) build a `DFlashRuntime` with `None`
   conditioning fields, (b) register a generic (non-target-conditioned) adapter and
   resolve it under the DFlash name, (c) import `tinygrad.dflash` — each must raise the
   exact typed error. These exercise the *existing* `contracts.py`/`adapters.py` guards
   (currently untested) and the new algorithm.
3. **`tests/gpu/dflash/reference/`** — a **tiny, fully-specified reference model** (a
   minimal trained-or-deterministically-seeded target+drafter pair small enough to run in
   CI under `safe_run.py`) whose DFlash run is the fidelity oracle for `F1..F5`. Where no
   real trained drafter is available for a production model, the production path **raises
   `F6`** and the reference model is the *only* thing claiming a DFlash speedup — so molt
   never reports DFlash support for a model that lacks a real drafter. This is the literal
   encoding of CLAUDE.md "If a model lacks a real trained DFlash drafter, say so
   explicitly and do not fake support."

**The gate:** `pytest tests/gpu/dflash/` green (fidelity + fail-closed) is release-blocking
for any change touching `src/molt/gpu/dflash/**` or the speculative modules.

### 3.6 Crosswalk: facts ↔ retired classes

| Fact / mechanism | File(s) | Class made unexpressible |
|---|---|---|
| `gpu_op_contract` (3.1) | `runtime/molt-gpu/op_contract.toml`, `tools/gen_gpu_op_contract.py` | primitive-set / op-semantics drift vs upstream pin |
| `gpu_api_contract` (3.2) | `src/molt/stdlib/tinygrad/api_contract.json`, `tools/gen_tinygrad_api_contract.py` | public API shape/default/error drift |
| diff oracle (3.3) | `tools/tinygrad_diff_oracle.py`, `tests/gpu/parity/` | hand-written-expected-value rot |
| `gpu_substrate_authority` (3.4) | `src/molt/gpu/AUTHORITY.toml`, `structural_audit.py` ext | `molt.gpu` second-authority drift |
| `dflash_fidelity` (3.5) | `src/molt/gpu/dflash/SPEC.md`, `tests/gpu/dflash/**` | DFlash mislabeling + unverified-fidelity |
| god-file retirement (§4.6) | delete `builtins/gpu.rs` legacy paths | two-parallel-GPU-pipelines drift |

---

## 4. PHASES (dependency order; each independently landable with green gates)

Each phase is a **complete structural change** (CLAUDE.md §"Structural change as the unit
of work"), not a partial slice. Sub-steps within a phase land atomically.

### Phase 0 — Pin the source-of-truth (foundation; no code change to molt)
**Goal:** make "the upstream tinygrad revision" and "the DFlash algorithm" *named,
immutable, in-tree* references the later phases derive from.
- 0a. Add `docs/spec/tinygrad_pin.md` recording: upstream = tinygrad **0.13.0**
  (`bench/friends/repos/tinygrad_off_the_shelf/pyproject.toml`), the exact commit/source
  snapshot, and the upgrade protocol (bump pin → regenerate all `gpu_*_contract` facts →
  re-run diff oracle → land as one change). This *is* the canonicalization doc's "exact
  upstream revision used for verification," promoted from a sentence to a pinned fact.
- 0b. Add `src/molt/gpu/dflash/SPEC.md` (3.5.1) transcribing `F1..F6` from the paper +
  project page, with citation lines.
- **Gate:** docs lint + a `tools/check_tinygrad_pin.py` that asserts the off-the-shelf
  `pyproject.toml` version equals the pinned string (so a silent dependency bump fails).
- **Composes with:** decomposition doc 21 (adds no god-file); doc 16 (the GPU-gap audit)
  which this pin makes precise.

### Phase 1 — `gpu_op_contract` fact + gate (retires §1.2.1)
**Goal:** the primitive set and per-op semantics become a *derived, checked* fact.
- 1a. Write `tools/gen_gpu_op_contract.py`: parse upstream `Ops` enum + `code_for_op`
  (AST parse, not regex — STRUCTURAL_AUDIT_BOARD discovery-vs-authority rule), emit
  `runtime/molt-gpu/op_contract.toml`.
- 1b. Reconcile the *actual* divergences found in §1.2.1: decide and **record in the
  contract** (a) molt `MOD`↔upstream `CMOD`, `IDIV`↔`CDIV` naming, (b) that `MAX`/integer
  div/mod are upstream *rewrites* (so molt either matches the rewrite or pins them as
  first-class with a `COMPOSED_FROM`/`REWRITE_OF` annotation), (c) `MULACC`/`POW`/`FDIV`/
  `THREEFRY`/`FLOORDIV`/`FLOORMOD` dispositions. **This is the real-fix of the existing
  drift, done structurally** (update the representation), not a prose edit.
- 1c. Wire `--check` into the gate set (mirror `gen_op_kinds.py --check`); add the
  Alive2-style composition property tests (3.1 obligation) under
  `runtime/molt-gpu/tests/op_contract_equiv.rs` and/or `tests/gpu/op_contract/`.
- 1d. Update `runtime/molt-gpu/src/ops.rs` so `PrimitiveOp` provably covers the contract
  (if 1b reveals a missing op, add it *with* its renderer + MLIR mapping across
  `render/{msl,wgsl,cuda,hip}.rs` + `mlir.rs` — symmetric coverage, CLAUDE.md
  §"Asymmetric coverage").
- **Gate:** `gen_gpu_op_contract.py --check` green; composition-equiv property tests
  green; `cargo test -p molt-gpu` green; no renderer asymmetry.
- **Perf note:** op additions must not regress the GPU primitive scoreboard
  (`runtime/molt-gpu/src/test_perf_regression.rs` exists — extend it).

### Phase 2 — tinygrad differential oracle (retires §1.2.2 numerics)
**Goal:** parity is *measured against the pin executing the same program*, every CI run.
- 2a. Build `tools/tinygrad_diff_oracle.py` (3.3) and the pytest plugin
  `tests/gpu/parity/conftest.py` with the `tinygrad_parity` fixture.
- 2b. Seed `tests/gpu/parity/corpus/` with one snippet per primitive (from the 3.1
  contract) + the composed ops in spec §2.2 (`relu`, `sigmoid`, `softmax`, `matmul`,
  `floor`, `gelu`, `tanh`, `layernorm`, `scaled_dot_product_attention`, `conv2d`,
  `conv_transpose2d`, `GroupNorm` — the canonicalization slice) + every dtype/edge case.
  Corpus coverage is a ratcheting metric.
- 2c. Run the matrix: CPython+pin (reference) vs molt {native, WASM, LLVM, Luau}; classify
  every result GREEN / RED_STABLE / RED_NOISY / TIE / DIMENSIONAL_WIN (Council perf-claims
  discipline). Record ULP budgets for transcendental ops *with their justification* in
  `tinygrad_pin.md`.
- **Gate:** `pytest tests/gpu/parity/` green on every target with a CPU/CLANG device;
  any numeric divergence is a hard RED. This gate *replaces* the trust currently placed in
  hand-written `test_tinygrad_*` expected values (those become redundant and are folded
  into the corpus or deleted — no two authorities).
- **Composes with:** doc 31 (differential-fuzzing lane) — the oracle is the ML-surface
  instance of that lane's methodology; share the fuzz-input + classification machinery.

### Phase 3 — `gpu_api_contract` fact + gate (retires §1.2.2 API shape)
**Goal:** API shape/defaults/error-types are a derived, checked fact.
- 3a. `tools/gen_tinygrad_api_contract.py` (3.2): introspect the pin, emit
  `src/molt/stdlib/tinygrad/api_contract.json`.
- 3b. Reconcile every `unclassified` upstream symbol: implement, or mark
  `raises-unsupported` with a clear boundary error (canonicalization doc rule). No silent
  absence.
- 3c. `--check` into the gate set.
- **Gate:** `gen_tinygrad_api_contract.py --check` green; the existing
  `tests/test_tinygrad_import_shim.py` signature tests are subsumed by the generated
  contract (fold/delete duplicates).

### Phase 4 — `gpu_substrate_authority` gate (retires §1.2.4)
**Goal:** `molt.gpu` cannot grow a second ML semantics.
- 4a. Author `src/molt/gpu/AUTHORITY.toml` (3.4) classifying every `molt.gpu` public
  symbol `delegates_to` / `substrate_only`.
- 4b. Extend `tools/structural_audit.py` with the ML-authority probe; fold into
  `--check`. Add the result to `STRUCTURAL_AUDIT_BOARD.md` ratchet metrics
  (`ml_duplicate_authorities`, may only go down).
- **Gate:** `structural_audit.py --check` green with the new probe; board ratchet holds.
- **Composes with:** doc 46 (semantic control plane — this is a new probe under it) and
  doc 49 (single-authority discipline).

### Phase 5 — DFlash algorithmic fidelity (retires §1.2.3) — **the turn-blocking core**
**Goal:** `molt.gpu.dflash` provably *is* DFlash or raises.
- 5a. Implement the DFlash drafter forward pass + KV injection (`F1`/`F2`/`F4`) as a
  composition of the 3.1 primitives (per spec §4.1: linear=`dot`, attention=`sdpa`,
  RMSNorm/softmax/RoPE composed) inside `src/molt/gpu/dflash/runtime.py` (or a new
  `drafter.py`), consuming `DFlashConditioning.target_features`/`target_kv`/
  `position_ids`/`last_verified_token`. The conditioning *plumbing already exists*
  (contracts.py); this adds the *consumption*.
- 5b. Build the `tests/gpu/dflash/reference/` tiny model (3.5.3) and
  `test_dflash_fidelity.py` covering `F1..F6` + the fail-closed corpus. Wire `F5`
  losslessness against a reference greedy target decode.
- 5c. Verify the fail-closed paths end-to-end: `tinygrad.dflash` import raises; generic
  adapter under DFlash name raises; missing trained drafter raises `F6`. (These guards
  exist in `contracts.py`/`adapters.py`/`dflash.py` but are *untested* today — 5b makes
  them gated.)
- 5d. **Drift sweep (CLAUDE.md §Top Priority: "clean that drift up before adding more
  code"):** audit `tinygrad/{speculative,eagle,mirror_sd,tree_attention,flash_attention,
  kv_cache}.py` for any path that could be *mistaken for* DFlash; ensure each is clearly
  labeled generic/EAGLE/etc. and cannot satisfy the DFlash conditioning contract by
  accident.
- **Gate:** `pytest tests/gpu/dflash/` green (fidelity + fail-closed), run under
  `safe_run.py` (CLAUDE.md Safe Execution — the reference model is a compiled binary).
  Release-blocking for any `dflash/**` or speculative change.

### Phase 6 — Retire the legacy GPU god-file (retires §1.2.5; composes with 21*)
**Goal:** one GPU pipeline, not two. Delete the legacy ad-hoc path the primitive spec
§5.1 already marked for deletion.
- 6a. Census `runtime/molt-runtime/src/builtins/gpu.rs` (11,817 lines): identify which
  symbols are (i) still reachable, (ii) duplicated by `molt-gpu` + `gpu_primitives.rs`,
  (iii) dead. (Use the 21-decomposition tooling + the dispatch-roots probe already used
  for tinygrad, evidenced by `tmp/.../tinygrad-dispatch-roots`.)
- 6b. Migrate any still-needed behavior onto the `molt-gpu` crate + `gpu_primitives.rs`
  intrinsics (symmetric: native + WASM + Luau + LLVM call sites together).
- 6c. Delete the legacy file/paths; update `STRUCTURAL_AUDIT_BOARD.md` (the
  `max_god_file_lines` / `god_files` ratchet must drop, never grow).
- **Gate:** full backend build (`cargo build --profile release-fast -p molt-backend
  --features native-backend`) + `pytest tests/gpu/` + the Phase-2 diff oracle green
  (proves the migration preserved semantics); board ratchet drops.
- **Composes with:** docs 21/21a–21e — this is the GPU instance of the decomposition
  program; it must not violate the crate-extraction precise-visibility rule
  (memory: `crate-extraction-precise-visibility`).
- **Note (Council):** this phase is allowed to be its own multi-tranche arc *only because*
  each tranche (census / migrate-one-subsystem / delete) is itself a complete structural
  piece with a green oracle, never a partial fix toward the next.

### Phase 7 — Performance contract closure (Performance Constitution, release-blocking)
**Goal:** every parity win is also a *speed* win across the full matrix — the constitution
forbids declaring this arc done on parity alone.
- 7a. Stand up the **GPU/ML scoreboard**: for the parity corpus (Phase 2) + the DFlash
  reference run + TurboQuant/DDTree (Plan 4) — report per benchmark → target → backend →
  profile → CPython ratio → (where semantically comparable) upstream-tinygrad ratio →
  binary size → peak RSS → compile time → log artifact (Performance Constitution
  methodology). Extend `runtime/molt-gpu/src/test_perf_regression.rs` + a
  `bench/friends` harness against `tinygrad_off_the_shelf`.
- 7b. Any benchmark <1.00× vs CPython is RED and blocks the arc; any regression vs the
  prior scoreboard is RED. Where upstream tinygrad wins, **name the missing fact**
  (fusion eligibility, ShapeTracker view-merge, kernel-cache hit, dtype-narrowing) per the
  constitution's "fix the REPRESENTATION" posture — do not peephole.
- **Gate:** the GPU/ML scoreboard green (no CPython-reds, zero regressions) on
  native/WASM/LLVM/Luau × dev/release-fast/release-output.
- **Composes with:** doc 51 §3 scoreboards (this is the ML row) and the Council
  "PERF/SPEED STATUS block" every batch must report.

### Phase dependency graph
```
P0 (pin) ─┬─> P1 (op_contract) ──┐
          ├─> P3 (api_contract)  ├─> P2 (diff oracle) ──> P6 (god-file retire) ─┐
          ├─> P4 (substrate auth)┘                                              ├─> P7 (perf)
          └─> P5 (dflash fidelity, uses P1 primitives) ─────────────────────────┘
```
P0 gates everything. P1/P3/P4 are parallelizable (non-overlapping files). P2 needs P1
(corpus references the op contract). P5 needs P1 (drafter composes primitives) but is
otherwise independent. P6 needs P2 (oracle proves migration). P7 needs P2+P5+P6.

---

## 5. VERIFICATION / GATES PER PHASE (measurement discipline)

The arc's parity oracle is **the pin executing the same program** (§3.3), never a human.
Per-phase gates are listed inline in §4; the cross-cutting discipline:

- **Parity oracle (numeric):** bit-exact for integer/movement/comparison ops and for any
  op upstream renders as an identical C expression; documented-ULP for `EXP2/LOG2/SIN`
  with the budget + justification pinned in `tinygrad_pin.md`. **No tolerance is allowed
  to hide a real divergence** — a widened ULP must cite the libm difference.
- **Parity oracle (API):** generated-contract `--check` (3.2) + signature execution.
- **Parity oracle (error semantics):** identical exception *type* across pin vs molt.
- **DFlash fidelity oracle:** the reference model run + `F1..F6` named obligations
  (§3.5); fail-closed corpus exercises every typed refusal.
- **Drift gates (the fact-plane `--check` family):** `gen_gpu_op_contract.py --check`,
  `gen_tinygrad_api_contract.py --check`, `structural_audit.py --check` (ML-authority
  probe), `check_tinygrad_pin.py`. Each is wired into the same CI gate group as
  `gen_op_kinds.py --check`. **An unrun gate is never implied green** (Council Gates rule).
- **Perf gates:** the GPU/ML scoreboard (§4.7); classify every result
  GREEN/RED_STABLE/RED_NOISY/TIE/DIMENSIONAL_WIN; quiescent, repeated, attributed
  (Council perf-claims discipline). Cold AND warm; never alloc-counter-only for a warm
  claim.
- **Safe execution:** every compiled-binary run (reference DFlash model, oracle native
  runs) goes through `tools/safe_run.py --rss-mb … --timeout …` (CLAUDE.md
  non-negotiable). The DFlash reference must be sized to run well under the cap.
- **Landing report format (Council):** "tests green; parity oracle green (N corpus
  programs × M targets, 0 RED); DFlash F1–F6 green; perf matrix green, 0 CPython-reds, 0
  regressions; drift gates green."

---

## 6. HOW IT COMPOSES (decomposition 21a–e + multi-agent execution)

### 6.1 With the decomposition program (21, 21a–21e)
- Phase 6 (god-file retirement) **is** the GPU-subsystem instance of doc 21's program. It
  must obey 21b's crate-graph blueprint (the `molt-gpu` crate already exists per the
  spec) and the crate-extraction precise-visibility rule (memory note
  `crate-extraction-precise-visibility`: moved files stay pure renames; widen `pub`
  precisely; use the `test-util` feature for cross-crate `#[cfg(test)]` accessors). It
  decrements `STRUCTURAL_AUDIT_BOARD` ratchets (`god_files`, `max_god_file_lines`) — never
  increments. No phase here may *add* a god-file; new generators are scripts, new
  contracts are data files.
- The new fact families (3.1–3.4) extend doc 46's semantic control plane and reuse
  `tools/structural_audit.py` — they *add probes/ratchets to the existing instrument*,
  they do not create a parallel audit system.

### 6.2 With the parallel multi-agent execution model (doc 00, Council three-lane)
- This arc maps cleanly onto the Council three-lane model (CLAUDE.md §Council):
  - **Lane C (infra/scoreboards/decomposition):** P0, P1, P2, P3, P4 (generators, oracle,
    audit probes, god-file census) — they make later correctness+perf work *checkable*.
  - **Lane A (semantic safety):** P5 DFlash fidelity (a correctness/fail-closed contract)
    + P6 migration soundness.
  - **Lane B (performance frontier):** P7 (GPU/ML scoreboard, CPython-reds, the missing
    fusion/view facts).
- **Non-overlapping files (Council requirement):** the phases touch disjoint trees —
  `tools/gen_*` + `tests/gpu/parity/` (Lane C) vs `src/molt/gpu/dflash/**` +
  `tests/gpu/dflash/**` (Lane A) vs `runtime/molt-gpu/src/test_perf_regression.rs` +
  `bench/friends` (Lane B). So **P1/P3/P4/P5 can run as concurrent agents** with no
  collision; P2 serializes after P1; P6/P7 serialize last.
- **Build-agent discipline (CLAUDE.md):** max 2 build-triggering agents; each exports
  `MOLT_SESSION_ID`; Phase 6/7 (which build `molt-backend`) must coordinate to stay under
  the cap and drain stale workers via `molt clean --apply --kill-processes`.

### 6.3 Cross-arc dependencies (what this arc needs from / gives to others)
- **Depends on the fact-plane arc (doc 51 §1 / doc 46):** the `--check` gate machinery,
  the `gen_op_kinds.py` pattern, and `structural_audit.py` are the substrate the new
  `gpu_*_contract` facts plug into. If that machinery changes, these generators follow.
- **Depends on the perf arc (doc 51 §3 scoreboards):** P7's GPU/ML scoreboard is a *row*
  in the four+1 scoreboard system, not a private dashboard. The missing-fact attribution
  (P7b) feeds the perf arc's triage.
- **Gives to demos/ecosystem (doc 16/17/29):** a *trustworthy* tinygrad surface is the
  precondition for the Falcon-OCR / PaddleOCR / Whisper / openpilot demos already in
  `tinygrad/examples/` and the model-cards (`docs/model-cards/*-tinygrad.md`). Those demos
  depend on **P2 (numeric parity) + P7 (perf)**; do not ship a demo as "working" until its
  ops are in the parity corpus and green.
- **Depends on Safe-Execution + Bootstrap arcs:** the tinygrad shim imports through the
  runtime import boundary (`MODULE_IMPORT`, CLAUDE.md Bootstrap Authority); any
  bootstrap-critical intrinsic (`molt_gpu_prim_device`) change adds a native bootstrap
  regression in the same change.

---

## 7. RISKS + STRUCTURAL (not band-aid) TREATMENT

| # | Risk | Band-aid (REJECTED) | Structural treatment |
|---|---|---|---|
| R1 | Upstream tinygrad bumps (0.13.0 → next) and the hand-curated primitive set silently rots (the §1.2.1 failure, recurring) | Edit the prose "26 ops" by hand each bump | **`gpu_op_contract` is generated from the pin (3.1); `check_tinygrad_pin.py` fails the build on an un-regenerated bump.** Drift becomes a red gate, not a latent bug. |
| R2 | Float ops differ in the last ULP between molt's renderer libm and the host libm, tempting a blanket tolerance | Widen ULP globally until green | **Per-op ULP budget pinned with its libm justification (§5); bit-exact required for everything upstream renders as identical C.** A widened tolerance must cite the difference; unexplained divergence stays RED. |
| R3 | DFlash is *expensive* to verify; temptation to ship the adapter contract as "DFlash support" without the algorithm (the current state) | Call the contract layer "done" | **`dflash_fidelity` requires `F1..F6` executable proof against a reference model (3.5); the contract layer alone cannot satisfy the gate.** No algorithm ⇒ no DFlash claim. |
| R4 | A real production model lacks a trained DFlash drafter; pressure to "approximate" it | Wire a generic drafter behind the DFlash name | **`F6 trained_drafter_required` is a typed fail-closed state (already structurally enforced by `contracts.py`, now *gated* by 5c); the production path raises, the reference model is the only DFlash claimant.** Literal CLAUDE.md mandate. |
| R5 | `molt.gpu` re-implements a tinygrad behavior for a "quick" perf win, forking semantics | "Just for the hot path" duplicate impl | **`gpu_substrate_authority` (3.4) fails `structural_audit.py --check` on any `delegates_to` symbol with an independent impl.** Perf wins go *into* the tinygrad-owned path (or into the substrate below it), never beside it. |
| R6 | Phase 6 god-file deletion regresses a behavior only that file had | Keep the legacy file "just in case" (two authorities — the current state) | **The Phase-2 diff oracle is the equivalence gate for the migration (§4.6); deletion is allowed only when the oracle proves preservation.** This is the doc-21 "replace, don't just delete" rule. |
| R7 | Parity green but molt is *slower* than CPython/upstream on an ML kernel | Declare parity done, defer perf | **Performance Constitution: P7 scoreboard is release-blocking; any <1.00× vs CPython is RED and blocks the arc.** Parity is the floor, not the finish. |
| R8 | The diff oracle's corpus under-covers (green but incomplete) | Ship with a thin corpus | **Corpus coverage is a ratcheting metric tied to the 3.1 op contract + 3.2 API contract (every public op/symbol must appear); coverage can only go up.** Incompleteness is a visible, ratcheted number. |
| R9 | WASM/Luau lack a GPU device, so parity "can't" be checked there | Exempt WASM/Luau silently | **A backend gap is a DOCUMENTED portable-IR fact (Performance Constitution), recorded in `tinygrad_pin.md`; the CPU/CLANG reference device runs on every target, and any genuinely device-absent op is an explicit, tracked limitation — never a hidden exception.** |

---

## 8. WHAT THIS ARC EXPLICITLY DOES NOT DO (scope discipline)
- It does **not** widen the tinygrad API surface beyond what the pin defines (no molt-only
  tensor methods under the `tinygrad` name).
- It does **not** add autograd/training (spec §9 non-goal) — inference parity only, unless
  the pin's public surface requires a piece, in which case the API contract (3.2) forces
  the decision explicitly.
- It does **not** implement new production DFlash drafters; it builds the *fidelity gate*
  and the *reference* so any future drafter is provably DFlash or raises.
- It does **not** invent a second audit/fact system — it extends doc 46 / `gen_op_kinds`
  machinery.

---

## 9. FIRST CONCRETE MOVE (for the implementing agent)
Land **Phase 0** (pin + DFlash SPEC + `check_tinygrad_pin.py`) and **Phase 1a/1b** (the
`gpu_op_contract` generator that *reads the 0.13.0 source* and surfaces the already-present
`CMOD`/`CDIV`/`MAX` divergence as a checked fact) in the first tranche — because that
single move converts the keystone "26 primitives" prose claim into a gated fact and
**proves the arc's thesis on day one**: drift that was invisible becomes a red gate.
Everything else composes on top.
