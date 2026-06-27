<!-- Foundation blueprint 60. Arc: TREE-SHAKING / WHOLE-PROGRAM DEAD-CODE
ELIMINATION — the shipped artifact contains ONLY code reachable from the program
entry, on every backend (native/Cranelift, LLVM, WASM, Luau) and every profile
(dev-fast/release-fast/release-output). The deliverable is ONE generated
whole-program REACHABILITY FACT (call-graph + dynamic-dispatch/getattr-aware
liveness from entry; FactValue-typed) that every elimination tier consumes,
retiring the class "reachability is re-derived per tier and the copies drift."
Author: portfolio-architect. Date: 2026-06-24. Status: DESIGN ONLY / EXECUTABLE
PLAN — no code written in the session that produced it; the lead integrates.
Every load-bearing file:line claim was verified read-only against the worktree
snapshot on 2026-06-24 (HEAD 1d92bc5cf). Code beats this doc when it drifts —
re-verify against current files and executable tests before acting.

NUMBERING (authoritative as of 2026-06-24): this doc is 60. The perf-measurement
plane is 64 (64_perf_scoreboards_and_harness.md); the perf compression ladder is
65 (65_perf_compression_ladder.md). (An earlier draft of this doc cited "doc 53"
for both — that was the pre-renumber slot and is corrected throughout to 64/65.)
DEEPENS: 65 Rung 8 (artifact-footprint facts) + 59 (one generated authority per
invariant). FEEDS: 61 (the Size board measures the byte/symbol drop this arc
produces) + 62 (smaller, ordered artifact ⇒ shorter cold page-in tail). Composes
with 21b (crate-graph), 21d (cli package), 21e (satellite link-only-what's-used),
63 (deforestation — its fused loops leave fewer reachable helpers). -->

# 60 — Tree-Shaking / Whole-Program Dead-Code Elimination: one reachability fact, every tier

## 0. The end-state outcome (the time-traveler's destination)

**In the end state, a function / method / class / stdlib-symbol / intrinsic that is
not reachable from the program entry CANNOT appear in the shipped artifact — on any
backend, in any profile — because "is this symbol live?" is answered exactly once, by
a single generated whole-program `Reachability` fact, and every elimination tier
(SimpleIR dead-function-elim, the stdlib-cache key, the IPO call-graph, the
address-taken-intrinsics manifest, the native linker root/export set, the LLVM
internalize/globaldce mask, the WASM tree-shake, the Luau emit-set) *consumes that
fact* rather than re-deriving it.** "I added a symbol to the dead-set in tier A but
tier B kept it alive" stops being expressible: the reference-edge vocabulary and the
root set are generated authorities (rendered to both Rust and Python and asserted
identical), and a backend that emits a symbol absent from the `Reachability` set fails
a build-time audit.

Concretely, at the destination:

- **One reachability authority, six+ consumers.** The `Reachability` fact (call-graph
  + dynamic-dispatch/getattr-aware liveness + roots, `LiveReason`-tagged per symbol) is
  built once per module on `SimpleIR` and is the single source for:
  `eliminate_dead_functions` (`runtime/molt-tir/src/passes.rs:2503`),
  `CallGraph::reachable_from` (`runtime/molt-passes/src/tir/call_graph.rs:564`), the Python
  stdlib-cache reachability `_reachable_function_names_for_stdlib_cache`
  (`src/molt/cli/__init__.py:19218`), `compute_intrinsic_manifest` (`passes.rs:4534`),
  the native linker root/export set (`cli/__init__.py:20202`/`:20236`), the LLVM module
  internalize mask (today implicit in the `default<O2>/<O3>` pipeline string — §2.2),
  the WASM export/import tree-shake (`wasm.rs:2436-2438` + the `add_import` surface at
  `:3112`), and the Luau emit-set (`luau.rs:86-90`, which today emits *every* function —
  §2.2). Today these are **four hand-mirrored traversals plus three backend-local keep
  policies** (§2.1) — the duplicate-authority drift class doc 59 exists to kill.

- **The reference-edge vocabulary is a generated `op_kinds.toml` table, not a
  hand-`match` duplicated in two languages.** "Which SimpleIR op-kind references a
  function by name (and how to derive the referenced name, and whether it implies a
  `{name}_poll`)" lives once in the op-kind registry
  (`runtime/molt-ir/src/tir/op_kinds.toml`) and is rendered to both the Rust classifier
  and the Python one — so `_DEAD_FUNCTION_ELIM_REFERENCE_KINDS`
  (`cli/__init__.py:19180`, a 26-entry frozenset) can no longer drift from the Rust
  `match op.kind.as_str()` arms (`passes.rs:2519-2573`, the *same* 26 strings, today
  byte-identical — this fact LOCKS that identity).

- **The artifact is provably minimal, measured against the CPython floor.** Binary size
  (native stripped/unstripped; WASM raw/gzip/brotli), function count, import count, and
  cold-start page-in are scoreboard dimensions (doc 65 Rung 8; the doc 61 Size board
  projection), green vs the CPython floor and ratcheted down. A symbol that survives is a
  *recorded reachability fact* (`LiveReason`), not a linker accident.

- **The Pythonista keeps dynamism; the Rustacean proves the rest dead.** Dynamic
  `getattr`, `importlib.import_module(name)`, `__init_subclass__`, `@register`
  registries, `__all__` re-exports — every escape hatch that makes a symbol
  reachable-by-name is a *recorded edge* (a `DynReachRoot` fact, §3 F3), not a reason to
  keep the whole stdlib. What the facts cannot prove reachable-by-a-dynamic-path is
  `Unknown` → conservatively kept (fail-closed); what they *can* prove dead is gone
  everywhere. This is the synthesis of DESIGN_DOCTRINE §2: the Pythonista's exact dynamic
  semantics preserved, the Rustacean's reachability proven precisely enough to compile
  away the dead rest.

This document is the executable plan that builds that one fact and routes every tier
through it. It is **Rung 8's reachability substrate** (doc 65 §3 Rung 8 names
"whole-program reachability/DCE → <2MB binary + per-attr liveness" and
"address-taken-intrinsics" as *the facts*; this doc *is* those facts) and a **doc 59
fact-family** (one generated authority per invariant, drift uncompilable).

### 0.1 What this doc is NOT (anti-duplication contract)

- It does **not** re-derive the perf compression ladder (doc 65). It supplies the
  *reachability fact* Rung 8 consumes; Rungs 1–7 (RC/dispatch/boxing/shape/loop/
  generator/portable-IR) are referenced, not restated.
- It does **not** re-derive the perf measurement plane (doc 64) or the Size plane
  (doc 61). It *consumes* their `PerfCell`/Size-board projections to gate its byte/
  symbol-count drop; it does not build a parallel size loop (doc 61 owns the Size board;
  this arc supplies the symbols it weighs — doc 61 §6 names this exact seam).
- It does **not** re-specify the fact-plane machinery (doc 59). It *uses* the
  op-kind-registry generator (`tools/gen_op_kinds.py`, doc 25/59 §2.1) and the
  generator-manifest meta-gate (doc 59 §3 F1, `tools/generator_manifest.toml` +
  `tools/check_generator_manifest.py` — **proposed there, not yet on disk**) as the
  carriers for its generated authorities; it registers its new authorities in that
  manifest when it lands, and falls back to a standalone `--check` until then (§4).
- It does **not** restate the satellite dedup arc (doc 21e). It composes with 21e's
  `LINK_AFFECTING_FEATURES` / tier-feature gating as the *crate-granularity* dual of this
  doc's *symbol-granularity* shaking (§5), and must not disturb 21e's parity guard (it
  reduces what links; it never changes satellite↔in-tree equivalence).
- It does **not** re-open the function/crate decomposition (doc 21a/21b). It obeys the
  structural-audit ratchet: its new fact lands as a focused module
  (`reachability_fact.rs`), never in a god-file; it *shrinks* the `cli/__init__.py`
  god-file (`:19180-19342` Python reachability becomes a thin FFI call, §3 F2).

---

## 1. Time-traveler derivation: from the end-state back to the facts to build

Working **backward** from "an unreachable symbol cannot appear in the artifact, and
reachability is answered exactly once":

1. **For "reachability answered once" to hold, the multiple traversals must collapse to
   one producer + many consumers.** → There must be a single typed `Reachability` record
   (a whole-program fact), built by one function, that the SimpleIR DFE, the IPO
   call-graph, the intrinsic manifest, the stdlib-cache key, and the linker/WASM/Luau
   root sets all read. (Today: §2.1 shows four hand-mirrored BFS implementations plus
   three backend-local keep policies; this is the structural defect.)

2. **For the reference-edge vocabulary not to drift across producers, it must be
   generated, not hand-written in each language.** → "Which op-kind references a function
   by name, how to extract the name, and whether it derives a `_poll`" is a per-op-kind
   fact → an `op_kinds.toml` table rendered to Rust *and* Python. (Today:
   `passes.rs:2519-2573` Rust `match` and `cli/__init__.py:19180-19205` Python `frozenset`
   are two hand-maintained copies of the *same* 26-string list — the exact "two tables
   that can disagree" class doc 59 §0 retires. They are byte-identical *today*; nothing
   but discipline keeps them so.)

3. **For dynamic reachability (getattr / import_module / subclass-registry / re-export)
   to be sound AND precise, every dynamic-keep must be an explicit recorded root, not a
   blanket "keep everything that might be reached dynamically."** → A `DynReachRoot` fact
   family: the frontend/runtime records *which* names a dynamic site can resolve (a
   string constant flowing to `importlib`/`getattr`, a `@register`-decorated class, an
   `__all__` re-export), seeding the BFS; anything not so recorded and not statically
   reachable is provably dead. (Today: `compute_intrinsic_manifest`,
   `passes.rs:4534-4612`, *already does exactly this for intrinsic names* — every
   `const_str` whose value is a real intrinsic symbol is a recorded address-taken root,
   validated against the linked staticlib's symbol set, failing the build **closed** on
   an unknown set, `passes.rs:4571`. That mechanism is the proven template to generalize
   to *Python* dynamic reachability.)

4. **For the fact to survive to every backend, it must live in portable scope (SimpleIR,
   before the backend split) and be lowered identically by each backend's keep/export/emit
   step.** → The `Reachability` fact carries, per symbol, *why* it is live
   (`RuntimeEntrypoint` / `EntryModule` / `StaticEdge` / `AddressTaken` / `DynRoot` /
   `Export`), and each backend's root/export/internalize/emit set is *derived from the
   fact*, never re-scraped from backend-local state (doc 65 Rung 7, the
   portable-IR-fact-parity rule; doc 46 §4.7 "a native win shadowed by a WASM regression
   is a portable-IR fact gap").

5. **For the fact to be TRUSTED (not a heuristic that silently keeps too much or drops
   too much), it must fail closed and be validated.** → `Unknown` reachability ⇒ keep (a
   missed shake = a size miss, never a correctness bug); a symbol the fact marks dead but
   a backend still references is a *validator failure* (a build-time assertion, not a
   runtime crash). The intrinsic-manifest precedent already fails the build closed on an
   unknown symbol set (`passes.rs:4571`, "fails the build closed … rather than guessing
   and re-creating the dangling-relocation corruption") — generalize that discipline to
   the whole reachability set.

6. **For the win to be real and durable, the size/footprint dimensions must be measured
   against the CPython floor and ratcheted.** → Each tier's landing reports binary size /
   function count / import count / cold page-in vs CPython, classified GREEN / RED_STABLE
   / DIMENSIONAL_WIN (doc 64 §1, CLAUDE.md tranche standard); a regression is a failed
   landing.

Items 1–2 are the **core structural collapse** (§3 F1/F2). Item 3 is the **dynamic
reachability completeness** generalizing the intrinsic-manifest template (§3 F3). Item 4
is the **portable-scope backend closure** (§3 F4). Item 5 is the **fail-closed
validator** (§3 F5). Item 6 is the **measurement bridge** to doc 65 Rung 8 / doc 61 Size
board (§7).

---

## 2. Current state (what exists — verified read-only against `main`, HEAD 1d92bc5cf)

The substrate is real but **fragmented into four+ hand-mirrored reachability traversals
plus three backend-local keep policies**. This arc is *unification + completion*, not
greenfield.

### 2.1 The reachability traversals + keep policies that hand-mirror each other (the defect)

| # | authority | where (verified) | what it does | drift surface |
|---|---|---|---|---|
| 1 | `eliminate_dead_functions` | `passes.rs:2503` (SimpleIR) | the production DFE: build name→referenced-names via `match op.kind.as_str()` (`:2519-2573`), BFS from roots (`:2578-2621`), `ir.functions.retain(reachable)` (`:2624`); env hatches `MOLT_DISABLE_DEAD_FUNC_ELIM` (`:2504`) / `MOLT_DEBUG_DEAD_FUNC_ELIM` (`:2627`) | the 26-entry `match` reference-kind list (`:2520-2565`) + `is_protected_runtime_entrypoint` (`:2635-2643`) |
| 2 | `_reachable_function_names_for_stdlib_cache` | `cli/__init__.py:19218` (Python) | re-implements #1's BFS to decide which stdlib functions enter the shared-stdlib cache key; result filters the cache payload at `:19342` (`if reachable and name not in reachable: continue`) | `_DEAD_FUNCTION_ELIM_REFERENCE_KINDS` frozenset (`:19180`, **26 strings, byte-identical to #1 today**) + the `{name}_poll` rule (`:19258-19264`) + `_is_protected_runtime_entrypoint` (`:19212`, hand-mirrors #1's `is_protected_runtime_entrypoint`) |
| 3 | `CallGraph` + `reachable_from` | `call_graph.rs:241` build, `:93` `classify_call_op`, `:564` BFS, `:193` `alloc_task_poll_target` (TIR) | the IPO call-graph: `classify_call_op` produces `CallEdge::{StaticDirect(name), Opaque}`; `reachable_from` is "the same traversal shape as `passes::eliminate_dead_functions`" (`:557-558`, its own comment) | `classify_call_op`'s edge vocabulary is a *third* copy of "which op references a function"; `alloc_task_poll_target` (`:193`) re-derives the `_poll` rule #1 has at `:2546` |
| 4 | `compute_intrinsic_manifest` | `passes.rs:4534` (+ `compute_intrinsic_manifest_checked` at `:4530`) | the address-taken-intrinsics root: every `const_str` naming a real intrinsic (`:4577-4586`), filtered by exact membership in the linked staticlib's intrinsic symbol set (`is_candidate_intrinsic_name`, `:4610`) → kept-alive manifest → per-app resolver | a *separate kind* of reachability (address-taken, not call-edge) but **not unified** with #1's root machinery |
| 5 | native linker keep policy | `cli/__init__.py` `_build_native_link_driver_command` (`:19912`), `_finalize_native_link` | macOS `-Wl,-dead_strip` + `-exported_symbols_list` containing only `_main` (`:20202`/`:20208`); Linux `--gc-sections` + version-script `{ global: main; local: *; }` (`:20221`/`:20236`); Windows `/OPT:REF` (`:20241`); archive linked WITHOUT `--whole-archive`/`-force_load` (`:20165`) so only referenced objects are pulled | the export root is the **hand-listed** `_main`/`main` only — not derived from the live `Export ∪ RuntimeEntrypoint` set |
| 6 | LLVM module keep policy | `llvm_backend/mod.rs` | `internalize`/`globaldce` run **implicitly** via the `default<O2>/<O3>` pass-pipeline string (`:171-175`); intrinsics preserved by `dllexport`-class linkage before opt (`:187-198`); `emit_app_resolver_function` (`:315`) takes intrinsic addresses; the comment "`-dead_strip`/`--gc-sections` still removes every intrinsic whose name appears in no manifest record" (`:308-310`) | the internalize *mask* (what stays external) is implicit in the O-level, NOT derived from `LiveReason::Export ∪ RuntimeEntrypoint ∪ AddressTaken` — so it cannot tighten beyond what the default pipeline's heuristic internalizer keeps |
| 7 | Luau emit policy | `luau.rs:86-90` | **emits EVERY function** (the only filter is `__annotate__`); local dead-code stripping (`:249-251`, `:5656-5676`, `:9791-9797`) removes unreachable *statements within bodies*, not whole functions. There is no linker, so the SimpleIR DFE is the ONLY whole-program tier — and Luau does not currently consume it for emit-selection | no function-level reachability is applied at Luau emit at all |

**The class:** *reachability re-derivation + backend-local keep policy.* Four traversals
each reconstruct "what is live from entry"; the reference-edge vocabulary exists in three
hand-maintained copies (Rust match #1, Python frozenset #2, `classify_call_op` #3) and
the root set in two (#1 Rust, #2 Python); the backend keep/export/emit sets (#5/#6/#7)
are three independent policies that do *not* read a shared liveness fact. A new call-like
op-kind (e.g. a new `call_*` variant) added to #1 but not #2 silently changes the cache
key vs the actual DFE; added to #1 but not #3 makes the IPO tier under-approximate
reachability; a symbol the fact *could* prove dead is kept by #6's coarse O-level
internalize and emitted unconditionally by #7. This is precisely doc 59 §0's "two
authorities for one invariant" and "a new member the oracle silently defaults" — here
applied to *reachability edges*, *roots*, and *backend keep policy*.

### 2.2 The elimination tiers that consume reachability (where the fact lands)

- **SimpleIR tier (native).** `eliminate_dead_functions` runs in the native pipeline:
  `simple_backend.rs:2279` (in `partition_stdlib_functions`), `:2852` (in `lower_to_tir`,
  pre), `:3233` (post-inline; gated on `skip_ir_passes`). `eliminate_dead_imports`
  (`passes.rs:3462`) prunes unconsumed `import_name`/`import_from` ops per function. Both
  gated by env (`MOLT_DISABLE_*`) — diagnostic escape hatches only.
- **SimpleIR tier (WASM).** `eliminate_dead_functions` + `eliminate_dead_imports`
  (+ `eliminate_dead_ops`) run at `wasm.rs:2436-2438` (after inlining). The WASM DFE BFS
  is therefore the *same* `passes.rs` authority #1 — **good; the WASM tier already shares
  the SimpleIR authority.** The WASM *import* surface (`add_import`,
  `wasm.rs:3112`/`:3165`/`:3338`; `docs/architecture/wasm-import-stripping.md`) is a
  separate tree-shake handled by `--wasm-profile pure` (skip `add_import` for process/db/
  ws/socket/time categories + emit `unreachable` at stripped call sites) + post-link
  `wasm-opt --remove-unused-module-elements`.
- **Linker tier (native).** Per-*symbol* DCE via section GC: macOS `-Wl,-dead_strip` +
  `-exported_symbols_list` (`cli/__init__.py:20202`/`:20208`), Linux
  `-ffunction-sections`/`-fdata-sections` + `--gc-sections` + version-script `{ global:
  main; local: *; }` (`:20221`/`:20236`), Windows `/OPT:REF` (`:20241`), plus
  `_post_link_strip` (`:20252`). The archive is linked WITHOUT `--whole-archive`/
  `-force_load` (`:20165`) so the linker only pulls referenced objects. The intrinsic
  manifest (#4) is what keeps name-resolved intrinsics from being stripped
  (`llvm_backend/mod.rs:308-310`).
- **Linker tier (LLVM).** Same native link path; the LLVM backend additionally has the
  app-resolver `emit_app_resolver_function` (`llvm_backend/mod.rs:315`) taking intrinsic
  addresses. **LLVM `internalize`/`globaldce` run only implicitly via the `default<O2>/
  <O3>` pipeline string (`:171-175`)** — there is no *explicit* module internalize mask
  derived from a liveness fact (verified: `internalize`/`globaldce` appear nowhere in
  `llvm_backend/` as explicit code). §3 F4 adds the explicit, fact-derived internalize
  mask so the LLVM module is shaken to the *live* set, not merely to what the default
  pipeline's heuristic internalizer keeps.
- **Linker tier (WASM).** `wasm-ld --gc-sections` (default for size),
  `--export-if-defined` for optional exports, no `--export-all`
  (`0931_LINKER_OPTIMIZATION_CONTRACT.md` §"WASM Linking", lines 48-62); post-link
  `wasm-opt --remove-unused-module-elements`.
- **Emit tier (Luau).** Luau transpiles to a script; there is no linker, so emit *is* the
  only whole-program tier — and it currently emits **all** functions (`luau.rs:86-90`).
  §3 F4 makes Luau emit only `reachability.live`, making the fact correctness-critical
  there (no linker backstop) per doc 65 Rung 7.
- **Satellite tier (21e).** Crate-granularity: `LINK_AFFECTING_FEATURES`
  (`_runtime_feature_gates.py:176`) + `RUNTIME_FEATURE_GATES` (`:36`, symbol-prefix →
  `stdlib_*` feature) decide *which satellite crates* link into a tier (doc 21e §1.3).
  This is the coarse dual of symbol shaking (§5).

### 2.3 The DCE/reachability passes (the intra-function complement, already sound)

- `tir/passes/dce.rs` — intra-function dead-op removal (use-count fixpoint,
  `build_use_counts` `:117-128`, ≤10 rounds, `run` `:145`) + effect-awareness via the
  central effects oracle (`op_has_observable_effect_when_dead`, `op_may_throw` — imported
  `:18`, used `:33`). **Correct and not part of the drift problem** — it operates
  *within* a function; this arc is *whole-program* (inter-function / inter-module). They
  compose: DCE shrinks bodies, DFE removes whole functions.
- `tir/passes/dead_store_elim.rs` — dead-store elimination (orthogonal; memory writes,
  the typed-slot-store + stack-confined-alloc patterns, `:1-73`).
- `tir/passes/reachability.rs` — *block* reachability within a function (the CFG-edge
  BFS, `metadata_preserving_reachable_blocks` `:33-60`). **This is block-level, not
  function-level** — name collision with this arc, but a different scope. The
  whole-program fact (§3) is a NEW module (`reachability_fact.rs`) to avoid overloading
  this one.

### 2.4 The op-kind registry (the generator this arc's authorities ride on)

- `op_kinds.toml` (3146 lines) + `gen_op_kinds.py` (2760 lines) render
  `op_kinds_generated.rs` (`OUT_RS`, `gen_op_kinds.py:55`) + `op_kinds_generated.py`
  (`OUT_PY = src/molt/frontend/lowering/op_kinds_generated.py`, `:56`), written at
  `:2752-2755`, `--check`-gated (`_CHECK_MODE`, `:868`). Already renders per-OpCode facts
  and the frontend wire-kind string tables (`binary_op` `:1027`, `frontend_raising_kind`
  `:924`) to BOTH Rust and Python — exactly the carrier needed for the reference-edge
  table (§3 F1). This proves the SimpleIR-`kind`-string → generated-Python-predicate path
  already exists.
- **Caveat (verified):** #1/#2/#3 key on the *SimpleIR* `op.kind` *string* (`"call"`,
  `"call_internal"`, `"generator_create"`, …), not the TIR `OpCode` enum. The op-kind
  registry's primary domain is the TIR `OpCode` enum (closed, exhaustive-match); the
  frontend wire-kind tables are the *open-domain* string side (doc 59 §5.2). The
  reference-edge fact is therefore an **open-domain** fact (a `[[reference_kind]]` table
  over SimpleIR kind strings) with the fail-closed `tools/audit_op_kinds.py`
  producer-drift complement — NOT a closed-enum exhaustive match. This is the correct
  shape and is called out so the implementer does not force it into the closed-`OpCode`
  mold. `op_kinds.toml` today has **no** `[[reference_kind]]` or `[[reachability_root]]`
  section (verified absent) — these are new.

---

## 3. The structural facts / mechanisms this arc builds (each tied to the class it retires)

The deliverable is **not "smaller binaries"** — it is **one whole-program reachability
fact that makes "reachability re-derived per tier" unexpressible.** Five mechanisms.

### F1. The generated reference-edge + root vocabulary — retires *"the reachability edge/root list drifts across Rust, Python, and the IPO call-graph"*

The single declarative source for "which op-kind references a function by name, how to
derive the name, and what the reachability roots are."

- **Artifact:** new sections in `op_kinds.toml`:
  - `[[reference_kind]]` rows: one per SimpleIR `op.kind` string that can reference a
    function by name. Each row carries `kind = "..."`, `name_source = "s_value"` (the
    field the referenced name comes from — all 26 current kinds read `s_value`), and
    `derives_poll = true|false` (whether `{name}_poll` is also implied — `true` for
    exactly `generator_create`/`coro_create`, the rule at `passes.rs:2546` /
    `cli/__init__.py:19258`). The **26 kinds** are exactly the union of
    `passes.rs:2520-2565` and `cli/__init__.py:19180-19205` (verified byte-identical
    today): `call`, `call_internal`, `func_new`, `func_new_closure`, `func_new_builtin`,
    `code_new`, `call_guarded`, `call_indirect`, `alloc_task`, `generator_create`,
    `coro_create`, `fn_ptr_code_set`, `asyncgen_locals_register`, `gen_locals_register`,
    `task_new`, `generator_send`, `spawn`, `call_func`, `call_method`, `import_from`,
    `import_name`, `class_def`, `decorator`, `super_call`, `yield_from`, `await`. (Note
    `alloc_task` reads the poll name *directly* in `s_value`; `generator_create`/
    `coro_create` read the *base* and derive `_poll` — encode this as
    `name_is_poll_direct = true` on `alloc_task` so the generator does not double-apply
    the suffix, matching `passes.rs:2535-2552`.)
  - `[[reachability_root]]` rows: the root set — exact names (`molt_main`,
    `molt_host_init`, `_start`) + prefixes (`molt_isolate_`) + the entry-function rule
    (`functions[0]`) — replacing the hand-duplicated `is_protected_runtime_entrypoint`
    (`passes.rs:2635-2643`) and `_is_protected_runtime_entrypoint`
    (`cli/__init__.py:19212`). The doc-comment invariant "`molt_init_*` are NOT
    blanket-kept; they are discovered via static CALL edges" (`passes.rs:2596-2606`) is
    preserved by *omitting* a `molt_init_` prefix row (the BFS discovers them through
    F2's `StaticEdge`).
- **Generator:** extend `gen_op_kinds.py` to render `reference_edge`/`is_reachability_root`
  predicates into `op_kinds_generated.rs` (a `fn reference_edge(kind: &str) ->
  Option<ReferenceEdge>` carrying `{name_source, derives_poll, name_is_poll_direct}` + a
  `fn is_reachability_root(name: &str) -> bool`) AND into `op_kinds_generated.py` (the
  frozenset + a root predicate). `--check`-gated (`_CHECK_MODE`, doc 59 §2.1) so a
  hand-written second copy is caught.
- **Validation (the cross-axis kill, doc 59 §2.1 lesson):** the generator asserts the
  `reference_kind` set is identical across the Rust and Python renders (they are two
  views of one fact) and that every `derives_poll` kind is also a `reference_kind`. A row
  present in one render but not the other is a generator failure — drift is uncompilable.
  The open-domain producer-drift complement (`tools/audit_op_kinds.py`, the existing #57
  SoT audit) gains a check that a SimpleIR producer emitting a new function-referencing
  kind without a `[[reference_kind]]` row is caught.
- **Class retired:** *reachability-vocabulary drift* (edge list + root list, the
  #1↔#2↔#3 hand-mirror of §2.1).

### F2. The unified whole-program `Reachability` fact — retires *"each tier re-implements the BFS"*

One typed record, built once, consumed by every tier.

- **Artifact:** new `runtime/molt-tir/src/reachability_fact.rs` (**SimpleIR scope** — it
  must run where `eliminate_dead_functions` runs, on `SimpleIR`, before TIR lifting)
  exposing:
  ```rust
  pub struct Reachability {
      /// reachable symbol -> the reason(s) it is live (why it survives)
      pub live: BTreeMap<String, LiveReason>,
      /// the recorded dynamic roots that seeded beyond static edges
      pub dyn_roots: BTreeSet<String>,
  }
  pub enum LiveReason {            // why a symbol is in the artifact
      RuntimeEntrypoint,           // F1 reachability_root (molt_main / _start / molt_isolate_*)
      EntryModule,                 // functions[0]
      StaticEdge,                  // reached via an F1 reference_kind edge (incl. molt_init_*)
      AddressTaken,                // const_str names a real intrinsic (the manifest shape)
      DynRoot(DynRootKind),        // F3: getattr / import_module / subclass-registry / re-export
      Export,                      // a backend export contract requires it
  }
  pub fn compute(ir: &SimpleIR, roots: &ReachabilityRoots) -> Reachability;
  ```
  `compute` builds the name→refs map using the F1 generated `reference_edge`, seeds from
  F1 `is_reachability_root` + `functions[0]` + `dyn_roots` (F3), runs ONE BFS, and records
  `LiveReason` per symbol (a symbol carries the strongest reason; the set is retained for
  the validator's diagnostics). `eliminate_dead_functions` becomes a thin consumer:
  `ir.functions.retain(|f| reachability.live.contains_key(&f.name))`.
- **Consumers (all read the one fact; none re-derives):**
  1. `eliminate_dead_functions` (`passes.rs:2503`) — retain on `reachability.live`; keep
     the `MOLT_DISABLE_*`/`MOLT_DEBUG_*` hatches (they now disable/observe the consumer,
     not a private BFS).
  2. `CallGraph::reachable_from` (`call_graph.rs:564`) — the IPO tier consumes the same
     edge vocabulary (F1, via `classify_call_op`) and the same BFS; the "same traversal
     shape" comment (`:557-558`) becomes "the same traversal *code*." (`classify_call_op`
     operates on TIR `OpCode`; bridge it to the F1 open-domain table via the existing
     `kind_to_opcode`/`OpCode::as_str` correspondence so there is one edge vocabulary, not
     two — the verification gate is that the TIR call-edge set ⊇ the SimpleIR reference
     set restricted to call-shaped kinds.)
  3. `compute_intrinsic_manifest` (`passes.rs:4534`) — the `AddressTaken` reason IS the
     intrinsic manifest; the manifest is *projected* from `reachability.live` (filter
     `LiveReason::AddressTaken` ∩ the linked-staticlib intrinsic symbol set) instead of a
     separate scan. Keeps the fail-closed symbol-set precondition (`:4571`) and the
     `compute_intrinsic_manifest_checked` (`:4530`) caller contract.
  4. The Python stdlib-cache reachability (`cli/__init__.py:19218`) — **deleted as a
     re-implementation** and replaced by an FFI call into the Rust `Reachability::compute`
     over the same IR. The backend already exposes `compute_intrinsic_manifest`,
     `eliminate_dead_functions`, etc. through `molt-backend`'s public surface
     (`lib.rs:43-50`); add a `reachable_function_names` export (does NOT exist yet —
     verified). This **shrinks the `cli/__init__.py` god-file** (`:19180-19342`, ~160
     lines) to a thin call, advancing doc 21d and the structural-audit ratchet (doc 59
     §6).
  5. The native linker root/export set (`cli/__init__.py:20208`/`:20236`), the LLVM
     internalize mask, the WASM export contract, and the Luau emit-set — all derived from
     `LiveReason::Export ∪ RuntimeEntrypoint` (§3 F4).
- **Class retired:** *BFS re-implementation* (the four traversals collapse to one producer
  + projections).

### F3. The `DynReachRoot` fact family — retires *"dynamic reachability is handled by keeping too much (whole stdlib) or too little (silent miss)"*

Generalizes the intrinsic-manifest's "every `const_str` naming a real symbol is a recorded
address-taken root" (`compute_intrinsic_manifest`, `passes.rs:4534-4612`) from *intrinsics*
to *all Python dynamic reachability*.

- **The classes of dynamic reachability** (each a `DynRootKind`):
  - **String→callable** — a `const_str` flowing to `getattr` / `importlib.import_module` /
    `__import__` / `operator.attrgetter`. The value is a recorded root (the exact
    intrinsic-manifest mechanism, lifted from intrinsic symbols to user/stdlib symbols).
    When the string is *not* a constant (computed at runtime), the receiver's whole symbol
    family is `Unknown` → kept (fail-closed). This is the Pythonista escape hatch made a
    *fact*: `getattr(obj, name)` keeps what `name` can be, proven where possible.
  - **Subclass / registry** — `__init_subclass__`, `@register`, `ABCMeta` registries,
    `__subclasshook__`: a class reachable only via a registry the runtime walks is a
    `DynRoot(Registry)`. The frontend already knows the decorator/metaclass shape (the
    `class_def`/`decorator` reference-kinds, `passes.rs:2564`); F3 records the class as a
    root when a registry-keeping decorator/metaclass is present.
  - **Re-export** — `__all__` / `from m import *`: a name re-exported is reachable from any
    importer of the module (a `DynRoot(ReExport)`); the import machinery
    (`import_name`/`import_from`, `passes.rs:2564`) is the edge source.
- **Where it is produced:** the frontend (which has the AST and knows `getattr`/
  `import_module`/decorator shapes) emits `DynReachRoot` records into the IR (a
  function-level attribute or a lightweight op), analogous to how intrinsic names already
  flow as `const_str` the manifest scans. The runtime contributes the *intrinsic* dyn-roots
  it already knows (the `AddressTaken` set).
- **The soundness contract (fail-closed):** `Reachability::compute` seeds the BFS with
  `dyn_roots`. A dynamic site whose target the frontend CANNOT prove (runtime string,
  reflective walk over an open set) marks its *candidate family* `Unknown` → kept. **No
  program is ever mis-shaken** (a wrongly-dropped symbol is a correctness bug — forbidden);
  the only outcome of imprecision is a *larger* artifact (a size miss). Coverage grows
  monotonically: more dynamic sites proven ⇒ smaller artifacts, measured by §7's coverage
  ratchet.
- **Class retired:** *dynamic-reachability-by-blunt-instrument* (keep-the-world or
  silent-drop) → recorded, proven-where-possible, fail-closed roots.

### F4. The portable-scope backend reachability closure — retires *"a tier shakes on one backend but not another"*

Every backend's keep/export/internalize/emit set is *derived from the one `Reachability`
fact*, and a backend that emits a symbol absent from the fact fails a gate.

- **Native (Cranelift) + LLVM:** the SimpleIR DFE (F2) already runs before codegen on both
  (`simple_backend.rs:2279`/`:2852`/`:3233`). Two additions:
  - The native linker root/export sets (`-exported_symbols_list`, the version script
    `{ global: …; local: *; }`) are **generated from** `LiveReason::Export ∪
    RuntimeEntrypoint`, not hand-listed as `_main`/`main` only (`cli/__init__.py:20208`/
    `:20236`). (For a typical executable the live export set is still just the entry, so
    this is behavior-preserving today; it becomes load-bearing for multi-export artifacts
    and the satellite/dylib story, §5 / doc 61 §3.6.)
  - **Add explicit LLVM module-level `internalize` + `globaldce`** in `llvm_backend/mod.rs`:
    after emitting the module and *before* the `default<O2>/<O3>` pipeline (`:171-175`),
    internalize every symbol *not* in `reachability.live`'s `Export ∪ RuntimeEntrypoint ∪
    AddressTaken` set (so `globaldce` can delete it), mirroring the native version-script
    `local: *`. The `AddressTaken` set is the same fact that keeps intrinsics alive on
    native (`llvm_backend/mod.rs:308-310`), so the resolver's address-taken symbols stay
    external. Today the O-level pipeline runs internalize/globaldce *implicitly* with its
    own heuristic root set — making the mask explicit and fact-derived is what tightens it
    to the *proven-live* set.
- **WASM:** the SimpleIR DFE (F2) runs (`wasm.rs:2436`); the *import* surface (`add_import`,
  `:3112`/`:3165`/`:3338`; `wasm-import-stripping.md`) is shaken by the same fact — an
  import whose only callers are now-dead functions is dropped pre-link, and
  `--export-if-defined` exports come from `LiveReason::Export`. Post-link `wasm-opt
  --remove-unused-module-elements` is the belt-and-suspenders DCE
  (`0931_LINKER_OPTIMIZATION_CONTRACT.md`). The `--wasm-profile pure` category strip
  becomes a *consequence* of the reachability fact (the IO/async/time imports are dead
  because no reachable function calls them) rather than a hand-curated category list.
- **Luau:** Luau transpiles to a script; tree-shaking is **emit-time** — change
  `luau.rs:86-90` so only functions in `reachability.live` are emitted (today it emits
  every function). The same fact drives "which `local function` definitions appear." Luau
  has no linker, so the SimpleIR DFE is the *only* whole-program tier — making F2
  correctness-critical there with no linker backstop (doc 65 Rung 7). This is the single
  biggest *new* size win in the arc for the Luau target.
- **The generated backend support matrix (doc 65 Rung 7 / doc 46 §4.7):** a new
  `tools/backend_reachability_audit.py` (does NOT exist yet — verified) checks each
  backend's *actual* emitted symbol set against `reachability.live` — a symbol emitted but
  not live, or live but not emitted, is a drift failure (the dual of `audit_op_kinds.py`).
  This is the "fact survives to every backend" gate.
- **Class retired:** *backend-local reachability* (a native shake with a WASM/LLVM/Luau
  gap — doc 46 §4.7's portable-IR-fact-gap class; the Luau emit-everything policy is the
  starkest instance).

### F5. The fail-closed reachability validator — retires *"a too-aggressive shake silently drops a live symbol (corruption) / a too-conservative one silently keeps the world (no gate)"*

The checkable obligation that makes a wrong shake a *build error*, not a runtime crash or a
silent size regression.

- **Drop-soundness (the corruption guard):** a `MOLT_VERIFY_REACHABILITY=1` self-check
  (mirroring `MOLT_VERIFY_ANALYSIS=1`, doc 65 §1) that, after DFE, re-scans the retained IR
  for any reference (via the F1 edge vocabulary) to a *removed* symbol — a dangling
  reference is a panic-in-debug / hard-fail-in-CI, never a silent emit. This is the
  generalization of the intrinsic-manifest's "fails the build closed rather than emitting a
  corrupt binary" (`passes.rs:4571`) to the whole reachability set.
- **Backend-emit cross-check (F4's audit):** `tools/backend_reachability_audit.py` — every
  backend's emitted symbols ⊆ `reachability.live` ∪ runtime-staticlib symbols (the linker
  resolves the latter); a backend emitting a symbol the fact says is dead is a drift
  failure.
- **Coverage ratchet (the too-conservative guard):** `tools/reach_coverage.py` (mirroring
  `tools/call_fact_coverage.py`'s ATTACHED/OPCODE_STATIC/TRANSIENT ratchet, doc 59 §2.3 —
  does NOT exist yet, verified) — tracks, per module, the fraction of dynamic sites with a
  *proven* `DynReachRoot` vs `Unknown`. `--check` fails if proven-coverage DECREASES. A
  blunt "keep the world" regression (someone marks a whole family `Unknown` to "fix" a
  miss) is caught as a coverage drop.
- **The equivalence proof the prompt requires** (removed code is truly unreachable — no
  dynamic path reaches it): the **conjunction** of three gates *is* the proof. (a) The
  drop-soundness self-check proves no *static* edge (F1 vocabulary) reaches a removed
  symbol. (b) The full differential corpus (`tests/differential/`, native+LLVM+WASM+Luau)
  is the *behavioral* oracle: if any test exercises a dropped symbol via *any* path (static
  or the dynamic paths F3 records), the program's output diverges from CPython and the test
  fails — so a green differential corpus *with the shake enabled* is the executable proof
  that no reachable-by-any-path symbol was dropped. (c) The `reach_coverage` ratchet proves
  the precision did not silently regress to "keep the world." A symbol is provably dead iff
  (no static edge) ∧ (no recorded dyn-root) ∧ (the differential corpus, which exercises the
  dynamic paths, stays green) — and `Unknown` keeps it whenever any of those cannot be
  established. This is the prompt's "equivalence proof," made of existing oracles rather
  than a new theorem prover.
- **Class retired:** *unvalidated shake* (silent over-drop = corruption; silent over-keep =
  no signal). Both become gated.

---

## 4. Phases (dependency order; each independently landable with green gates)

Each phase is a **complete structural piece** (CLAUDE.md unit-of-work rule). Lane
assignment per the council three-lane model (doc 65 §6): mostly **lane C**
(infra/footprint) with a **lane B** (perf-frontier) bridge for the backend lowering; the
IR-correctness pieces touch the safety-adjacent DFE so carry A-lane discipline
(differential parity on every backend, the correctness floor of §7).

> Build/test discipline (CLAUDE.md): `export MOLT_SESSION_ID=tree-<phase>` and
> `CARGO_TARGET_DIR="$PWD/target/sessions/$MOLT_SESSION_ID"` before any build; route every
> raw-binary run through `tools/safe_run.py --rss-mb <cap> --timeout <s>`; never
> `cargo clean`; max 2 build-triggering agents. Phase 0 and the tooling/audit pieces are
> host-Python (no Rust rebuild on the critical path); Phases 1–4 touch Rust and serialize
> through the backend daemon.

### Phase 0 — Lock the current identity + characterize the size baseline. *Do first; no behavior change.*
- **Why first:** F1 *locks* that the Rust edge list (`passes.rs:2520-2565`) and the Python
  frozenset (`cli/__init__.py:19180`) are identical TODAY (verified: same 26 strings).
  Before generating them from one source, prove byte-equivalence so the generation is
  provably behavior-preserving. If they have *already* drifted by the time this lands, that
  is a latent bug this phase surfaces (the deliverable — doc 59 §10 risk row), resolved
  against the differential corpus before generation.
- **Do:** (a) a one-shot reconciliation test (`tests/test_reachability_vocab_parity.py`)
  asserting the Rust and Python reference-kind sets + root rules + the `_poll`-derivation
  rule match (parse both, diff). (b) Wire the size/symbol-count dimensions (binary size
  native stripped/unstripped, WASM raw/gzip, function count, import count) for a baseline
  artifact set into the doc 61 Size board projection (or, if doc 61 has not landed, a
  standalone `bench/scoreboard/tree_shaking_baseline.json` the later phases diff against and
  that doc 61 absorbs). (c) `MOLT_DEBUG_DEAD_FUNC_ELIM` (`passes.rs:2627`) /
  `MOLT_DEBUG_DEAD_IMPORT_ELIM` (`:3507`) already emit removed-count — add a one-line
  "live/total + `LiveReason` histogram" debug dump as the future fact's observability.
- **Gate:** the parity test GREEN (or the drift surfaced + filed); the baseline size JSON
  committed under `bench/scoreboard/`. No code behavior change.

### Phase 1 — Generate the reference-edge + root vocabulary (F1).
- **Do:** add `[[reference_kind]]` (26 rows, with `name_source`/`derives_poll`/
  `name_is_poll_direct`) + `[[reachability_root]]` to `op_kinds.toml`; extend
  `gen_op_kinds.py` to render `reference_edge`/`is_reachability_root` into both
  `op_kinds_generated.rs` and `op_kinds_generated.py`; add the cross-render agreement
  validation (§3 F1). Replace the hand `match` (`passes.rs:2519-2573`) +
  `is_protected_runtime_entrypoint` (`:2635`) and the Python frozenset
  (`cli/__init__.py:19180`) + `_is_protected_runtime_entrypoint` (`:19212`) + the
  `{name}_poll` rule (`:19258`) with calls to the generated predicates. Register both new
  sections in `tools/generator_manifest.toml` as open-domain facts with the
  `audit_op_kinds.py` producer-drift complement — **or, if the doc-59 manifest has not
  landed, add a standalone `gen_op_kinds.py --check` assertion and a sync test** (the
  manifest absorbs it later; do not block on doc 59).
- **Files:** `op_kinds.toml`, `gen_op_kinds.py`, `op_kinds_generated.rs`,
  `op_kinds_generated.py`, `passes.rs` (consume), `cli/__init__.py` (consume),
  `audit_op_kinds.py` (producer-drift), `tests/test_gen_op_kinds.py` (extend).
- **Gate:** `gen_op_kinds.py --check` GREEN; the Phase-0 parity test now passes *because
  both sides read the generated predicate*; `cargo test -p molt-tir` + `cargo test -p
  molt-backend` show **byte-identical DFE behavior** (the vocabulary is the same, only its
  source changed); differential parity unaffected (representation of the *classifier*, not
  behavior).

### Phase 2 — The unified `Reachability` fact + collapse the four traversals (F2).
- **Do:** author `runtime/molt-tir/src/reachability_fact.rs` with `Reachability` +
  `LiveReason` + `compute` (§3 F2), built on F1's vocabulary. Rewrite
  `eliminate_dead_functions` to consume it (retain on `live`). Rewrite `CallGraph` edge
  build + `reachable_from` to consume the same edge vocabulary + BFS (delete the "same
  shape" duplication, `call_graph.rs:557-558`; bridge `classify_call_op`'s TIR `OpCode`
  edges to the F1 open-domain table). Project `compute_intrinsic_manifest` from
  `LiveReason::AddressTaken`. Expose `reachable_function_names` via `molt-backend`'s public
  surface (`lib.rs:43-50`) and **delete** the Python re-implementation
  `_reachable_function_names_for_stdlib_cache` (`cli/__init__.py:19218`), replacing it with
  the FFI call (shrinks the god-file). Add the `MOLT_VERIFY_REACHABILITY=1` drop-soundness
  self-check (F5).
- **Files (new):** `reachability_fact.rs`. **Edit:** `passes.rs` (DFE + manifest
  projection), `call_graph.rs` (consume), `lib.rs` (export), `cli/__init__.py` (delete the
  Python BFS, call the export).
- **Gate:** `cargo test -p molt-tir` + `-p molt-backend` GREEN; the stdlib-cache key is
  **byte-identical** to Phase 0 (the FFI call returns the same set the Python BFS did —
  prove with a fixture comparing old-Python-impl vs new-FFI on saved IR — Risk row 5);
  `MOLT_VERIFY_REACHABILITY=1` finds zero dangling refs on the differential corpus; full
  differential parity native+LLVM+WASM+Luau; size DIMENSIONAL check (should be unchanged —
  same reachability set, one authority now).

### Phase 3 — Dynamic reachability completeness (F3).
- **Do:** define `DynReachRoot` + `DynRootKind`; have the frontend emit dyn-roots for the
  three classes (string→callable proven const, subclass/registry, re-export — §3 F3); seed
  `Reachability::compute` with them; mark unprovable dynamic sites' families `Unknown`→kept
  (fail-closed). Add `tools/reach_coverage.py` (the proven-vs-`Unknown` ratchet, F5). This
  is where the *real shake* happens — previously-kept-conservatively stdlib symbols now drop
  when no reachable static OR proven-dynamic path reaches them.
- **Files:** the frontend dyn-root emission (`src/molt/frontend/...` — the `getattr`/
  `import`/decorator lowering, the same sites `passes.rs:2564` reference-kinds cover),
  `reachability_fact.rs` (seed), `reach_coverage.py` (new), `generator_manifest.toml`/
  standalone (register the coverage ratchet).
- **Gate:** `reach_coverage.py --check` GREEN (coverage UP from Phase 2); differential
  parity on the FULL stdlib differential corpus (`tests/differential/`) — the shake must
  NOT drop a symbol any test reaches dynamically (the correctness floor / the §3 F5
  equivalence proof); binary-size scoreboard shows a measured DROP vs Phase 0 baseline,
  classified DIMENSIONAL_WIN (native + WASM + Luau); a deliberate "dynamic getattr keeps the
  family" regression test proves fail-closed (a computed-name `getattr` keeps the receiver's
  symbol family).

### Phase 4 — Backend reachability closure (F4) + the audit (F5).
- **Do:** generate the native linker root/export set from `LiveReason::Export ∪
  RuntimeEntrypoint` (replace the hand `_main`/`main` lists, `cli/__init__.py:20208`/
  `:20236`); add **explicit** LLVM module-level `internalize` + `globaldce` in
  `llvm_backend/mod.rs` masked by `LiveReason::Export ∪ RuntimeEntrypoint ∪ AddressTaken`
  (before the `default<O2>/<O3>` pipeline at `:171-175`); drive the WASM import strip +
  exports from the fact (`wasm.rs:3112` `add_import`); make Luau emit only
  `reachability.live` (`luau.rs:86-90`). Build `tools/backend_reachability_audit.py`
  (emitted-symbols ⊆ live ∪ runtime, per backend) and gate it in CI. Register the backend
  matrix rows (doc 65 Rung 7).
- **Files:** `cli/__init__.py` (link/export derivation), `llvm_backend/mod.rs`
  (internalize/globaldce mask), `wasm.rs` (import strip from fact), `luau.rs` (emit shake),
  `backend_reachability_audit.py` (new), `.github/workflows/ci.yml` (gate).
- **Gate:** `backend_reachability_audit.py --check` GREEN on all four backends; binary-size
  scoreboard shows a measured DROP on native (LLVM explicit internalize) AND WASM (import
  strip) AND **Luau (emit shake — the biggest new win, since Luau emitted everything
  before)**, each classified; full differential parity all backends; the LLVM lane size
  delta recorded (the `0931_LINKER_OPTIMIZATION_CONTRACT.md` before/after discipline + the
  linked-Falcon/Tinygrad smoke); cold-start page-in DIMENSIONAL check (smaller artifact →
  fewer pages, doc 65 Rung 8 / doc 62).

### Phase 5 (ongoing, composes with 21e) — per-attribute / per-method liveness + crate-granularity.
- **Do:** extend reachability below the function granularity to *methods* and *fields* (a
  dead method on a live class; a never-read field — ties doc 65 Rung 4 ShapeFacts
  `FieldSlot` and doc 09/13 dead-field) and compose with 21e's `LINK_AFFECTING_FEATURES` so
  a satellite crate links into a tier ONLY when the reachability fact shows a live symbol
  from it (the symbol-granularity fact *informs* the crate-granularity gate — doc 21e §1.3;
  §5 here). This is the "<2MB binary + per-attr liveness" terminus (doc 65 Rung 8 / doc 51
  §6).
- **Gate per increment:** `reach_coverage.py` UP; the relevant size dimension DOWN;
  differential parity; the 21e satellite-parity guard (`check_satellite_parity.py`)
  unaffected (this *reduces* what links, never changes satellite/in-tree equivalence).
  **Unbounded** — the monthly cadence (doc 65 §7) for the footprint class.

**Landing report format (every phase — doc 64 §1 + CLAUDE.md PERF/SPEED + doc 61 SIZE
block):** "tests green; the named gate(s) green; binary size / function count / import
count delta vs CPython floor AND vs prior phase, classified GREEN/RED_STABLE/
DIMENSIONAL_WIN; cold page-in delta; zero new hand-mirrored reachability code; zero
dropped-live-symbol differential failures (the correctness floor / the equivalence proof of
§3 F5)."

---

## 5. Composition with the decomposition (21a–e) and the 50–69 arcs

- **Doc 65 (perf compression ladder) Rung 8 — the primary parent.** Rung 8 names
  "whole-program reachability/DCE → <2MB binary + per-attr liveness" and
  "address-taken-intrinsics" as its facts (doc 65 §3 Rung 8). This arc *builds those
  facts*: F2 is the reachability fact Rung 8 consumes; F3's `AddressTaken` reason IS the
  address-taken-intrinsics fact; Phase 5 IS per-attr liveness. **Cross-arc dependency:**
  Rung 8 depends on this arc's `Reachability` fact; this arc depends on doc 65 Rung 0's
  scoreboard cold/size dimensions for its gates. Per-attr liveness (Phase 5) *also* depends
  on Rung 4 ShapeFacts (`FieldSlot`) and Rung 2 CallFacts targets (to know which methods
  are reachable) — so Phase 5 co-schedules after those rungs (doc 65 §7 M4/M9).
- **Doc 64 (perf measurement plane) + doc 61 (Size board) — the gates.** This arc's product
  is a smaller artifact, so its dimensions are footprint, not warm throughput. It reports
  through doc 64's `PerfCell` size fields and doc 61's Size-board projection (binary size
  native stripped/unstripped + WASM raw/gzip/brotli + function count + import count, gated
  vs the CPython floor and ratcheted). It builds **no** parallel size loop (doc 61 Risk 1) —
  it supplies the symbols doc 61 weighs. doc 61 §6 explicitly states arc 60 owns the
  tree-shaking mechanism and arc 61 owns the scoreboard that proves it worked; this is that
  seam.
- **Doc 59 (semantic fact plane) — the machinery.** F1's generated edge/root vocabulary
  follows doc 59 §5.2 (open-domain fact: table + `--check` + producer-drift audit). F2/F3
  register in the generator manifest (doc 59 §3 F1, when it lands; standalone `--check`
  until then). F5's coverage ratchet is the positive dual (doc 59 §2.3). This arc is a
  *consumer* of the fact-plane institution doc 59 builds — it does not re-derive it, and it
  does not block on doc 59 (the standalone `--check` fallback keeps it independently
  landable).
- **Doc 21d (cli package decomposition).** F2 *deletes* ~160 lines of Python reachability
  re-implementation from the `cli/__init__.py` god-file (`:19180-19342`) and replaces it
  with a thin FFI call — a direct contribution to 21d and a *reduction* of the
  structural-audit god-file ratchet (doc 59 §6; `cli/__init__.py` is ~41.6K lines, the
  largest offender). The new fact lands as a focused `reachability_fact.rs` module, never in
  a god-file (doc 21b precise-visibility discipline).
- **Doc 21e (satellite dedup).** This arc's *symbol*-granularity shake and 21e's
  *crate*-granularity gate (`LINK_AFFECTING_FEATURES` `:176`, `RUNTIME_FEATURE_GATES` `:36`)
  are duals. Phase 5 closes the loop: the reachability fact tells the tier-feature gate
  whether a satellite crate has any live symbol, so a tier links a satellite ONLY when
  needed — making 21e's "link only what's used" *fact-driven* instead of feature-flag-
  curated. This arc must not disturb 21e's parity guard (`check_satellite_parity.py`): it
  reduces what links; it never changes satellite↔in-tree equivalence.
- **Doc 21b (crate graph).** The reachability fact lives in `molt-tir` (the IR crate) and is
  consumed by `molt-backend` and the CLI through the existing public surface (`lib.rs:43-50`)
  — respecting the crate dependency direction; no new cross-crate cycle.
- **Doc 63 (deforestation/fusion).** A fused producer/consumer loop replaces a pipeline of
  helper calls with one loop body — so fusion *reduces* the reachable-function set (fewer
  iterator/intermediate helpers survive the BFS). The two arcs compound: 63 makes the chain
  a single loop; 60 proves the now-unused helpers dead and strips them. No shared files; 60
  consumes the smaller call graph 63 produces.
- **Doc 62 (cold start).** Doc 62's `StartupOrder` consumes this arc's section attribution
  to order the *live* cold-path symbols; this arc shrinks the cold tail so 62's ordered hot
  prefix is a larger fraction of a smaller image. 62 §6 names this convergence ("shrink the
  tail, bound the head"). Neither blocks the other.

---

## 6. The cross-tier invariants (what keeps this one fact, not seven)

1. **One reachability authority.** `Reachability::compute` (F2) is the single producer;
   DFE, IPO call-graph, intrinsic manifest, stdlib-cache key, native linker root/export,
   LLVM internalize mask, WASM import/export, Luau emit are *projections/consumers*. No tier
   re-implements the BFS or the edge vocabulary.
2. **One generated edge/root vocabulary.** The `reference_kind`/`reachability_root` tables
   (F1) render to both Rust and Python; a new call-like kind is a table row (doc 59 §5.2),
   never a hand-`match` in `passes.rs` or a hand-frozenset in `cli/__init__.py`.
3. **Fail-closed everywhere.** `Unknown` reachability ⇒ keep (a missed shake = a size miss,
   never a correctness bug). A dropped-live symbol is a *validator failure* (F5), never a
   runtime crash. This mirrors the intrinsic-manifest's build-fails-closed precedent
   (`passes.rs:4571`).
4. **One validator discipline.** Each tier's "done" ships the F5 obligations:
   `MOLT_VERIFY_REACHABILITY=1` drop-soundness, `backend_reachability_audit.py`
   emit-cross-check, `reach_coverage.py` proven-coverage ratchet, and the differential
   corpus as the dynamic-path oracle (the §3 F5 equivalence proof). A shake without a
   validator is a half-fact (doc 65 §1).
5. **One measurement contract.** Each tier's win is a GREEN size/cold scoreboard row vs the
   CPython floor (doc 64 §1 / doc 61 Size board), classified; a size or cold regression is a
   failed landing.
6. **No second authority** (doc 59 §0). The `AddressTaken` reason IS the intrinsic manifest;
   the IPO `reachable_from` IS the DFE BFS; the linker export set IS `LiveReason::Export`.
   Consumers *reference and project*; they never re-derive.

---

## 7. Measurement + gates (the Performance Constitution dimensions)

Per CLAUDE.md, binary size, peak RSS, compile time, and COLD vs WARM start are tracked
dimensions; cold-start is an *artifact-footprint/page-in/codesign* problem (NOT
runtime-init, measured 0.127ms — doc 65 Rung 8 / doc 62). This arc's product is a *smaller
artifact*, so its primary dimensions are **footprint**, not warm throughput.

Every phase reports, via the doc 64 / doc 61 Size board:

`artifact → target(native/LLVM/WASM/Luau) → profile(dev-fast/release-fast/release-output) →
binary size (native stripped+unstripped; WASM raw+gzip+brotli) → function count → import
count → cold page-in / startup_tax → CPython size ratio → compile-time delta → log
artifact`, with ≥5 samples, CV stability, classification GREEN / RED_STABLE / RED_NOISY /
TIE / DIMENSIONAL_WIN.

The standing gates (CI):
- `gen_op_kinds.py --check` (F1 vocabulary generated, drift uncompilable).
- `check_generator_manifest.py --check` (doc 59, when landed) — F1/F2/F3 authorities
  registered; standalone `--check` until then.
- `backend_reachability_audit.py --check` (F4/F5) — emitted ⊆ live, all 4 backends.
- `reach_coverage.py --check` (F5) — proven dynamic-reachability coverage UP.
- `MOLT_VERIFY_REACHABILITY=1` on the differential corpus (F5) — zero dangling refs.
- The Size-board ratchet (doc 61) — binary size / import count DOWN (or DIMENSIONAL
  justification), never silently UP (doc 65 Rung 8, the silent-footprint-regression class).

**The correctness floor + the equivalence proof (non-negotiable, gates every phase):** the
full differential corpus (`tests/differential/`) passes on native+LLVM+WASM+Luau — the shake
must NEVER drop a symbol any test reaches statically OR dynamically. Per §3 F5, a green
differential corpus *with the shake enabled* is the executable proof that no
reachable-by-any-path symbol was dropped (the corpus exercises the dynamic paths F3 records;
if a dropped symbol were reachable dynamically, output would diverge from CPython). A
dropped-live symbol is a P0 correctness bug (CLAUDE.md), not a size tradeoff.

---

## 8. Risks + structural (not band-aid) treatment

| risk | where it bites | STRUCTURAL treatment (no band-aid) |
|---|---|---|
| **The Rust and Python edge lists have drifted by the time F1 lands** (latent) | Phase 0/F1 | Phase 0's parity test SURFACES it as the deliverable (doc 59 §10 row); the FINDING (which side is wrong) is a correctness win, resolved against the differential corpus before generation. Never paper over by picking a side silently. (Verified identical TODAY — 26 strings both sides.) |
| **A dynamic `getattr`/`import_module` reaches a symbol the shake dropped** (corruption) | Phase 3/F3 | Fail-closed by construction: an unprovable dynamic site marks its candidate family `Unknown`→kept. `MOLT_VERIFY_REACHABILITY=1` (F5) + the full differential corpus (the dynamic-path oracle, §3 F5) catch any dropped-live symbol at build/CI time, never at runtime. The intrinsic-manifest precedent (`passes.rs:4571`) proves the discipline works at scale. |
| **Aggressive shake breaks the runtime's extern-`C` entrypoints** (`molt_isolate_*`, importlib helpers) | Phase 2/F2 | The `reachability_root` table (F1) is the *generated* authority for protected entrypoints (replacing the hand `is_protected_runtime_entrypoint`, `passes.rs:2635`); a missing root is a table edit + a differential failure, not a silent strip. The `molt_init_*`-discovered-via-static-edge invariant (`passes.rs:2596-2606`) is preserved by NOT adding a `molt_init_` root row. The native-bootstrap regressions (CLAUDE.md Bootstrap Authority) gate the import paths. |
| **LLVM explicit `internalize`/`globaldce` strips an address-taken intrinsic** | Phase 4/F4 | The internalize mask is *derived from* `LiveReason::Export ∪ RuntimeEntrypoint ∪ AddressTaken` — the same fact that already keeps intrinsics alive on native (`llvm_backend/mod.rs:308-310`); `backend_reachability_audit.py` cross-checks the emitted set. An internalized-then-deleted live symbol is a gate failure, caught pre-link. (The default O2/O3 pipeline already runs these implicitly — making the mask explicit/fact-derived only *tightens* the keep set, it does not introduce a new mechanism that could over-strip.) |
| **The stdlib-cache key changes** when the Python BFS is replaced by the FFI call (cache invalidation storm) | Phase 2/F2 | The gate proves byte-identity (old-Python-impl vs new-FFI on saved IR) BEFORE the swap; the key is computed from the *same* reachable set, so the cache stays valid. If the sets differ, the Python impl had a bug (the deliverable). |
| **Luau emit-shake drops a function the script reaches via a Luau-level dynamic call** | Phase 4/F4 | Luau has no linker backstop, so F2's fact is the only tier — making the differential corpus on the Luau backend the load-bearing oracle (a dropped-live Luau function diverges output). The same `Unknown`→kept fail-closed rule applies; the Luau emit-set is `reachability.live`, computed by the *same* fact native/WASM use, so a Luau-only miss is a portable-fact gap caught by `backend_reachability_audit.py`. |
| **WASM/Luau lose a shake that native keeps (or vice versa)** | Phase 4/F4 | Rung 7 portable-IR-fact-parity rule (doc 65): the fact lives in SimpleIR scope (F2, before the backend split); `backend_reachability_audit.py` gates all four backends on the SAME fact. A backend-only gap is a portable-IR-fact-gap (doc 46 §4.7), gated, not excepted. |
| **`reach_coverage.py` becomes a Goodhart target** (mark families `Unknown` to dodge a miss, inflating "kept" but hiding precision loss) | Phase 3/F5 | The ratchet is proven-coverage (DOWN-fails), so a blunt `Unknown` regression DROPS coverage and fails the gate; it is the dual of the Size-board ratchet (more `Unknown` ⇒ bigger artifact ⇒ size ratchet also reds). Two ratchets in opposition make the dodge fail both. |
| **Per-attr liveness (Phase 5) is a large greenfield; risk of a partial system** | Phase 5 | The `FactValue` fail-closed lattice makes partiality SOUND (doc 65 §8): an un-analyzed method/field is `Unknown`=kept (current behavior). Coverage grows monotonically via `reach_coverage.py`; no program is miscompiled by a missing per-attr fact, only un-shrunk. Composes with doc 65 Rung 4 (`FieldSlot`) — no second authority for field liveness. |
| **Compile-time regression** from running the unified BFS + dyn-root analysis | all phases | The BFS already runs (it is `eliminate_dead_functions`, every native/WASM build); unifying does not add a traversal, it removes three. Dyn-root analysis is frontend-time over data already walked. Compile-time is a tracked dimension (§7); a regression is reported and budgeted, never hidden (doc 64 §1). |

---

## 9. The single most important sentence

This arc's deliverable is **not** a smaller binary — it is a codebase in which
*"reachability is re-derived per tier"* and *"a symbol is dead here but alive there"* are
**a generator failure, a build-closed assertion, or a red audit**, so that the SimpleIR
DFE, the IPO call-graph, the address-taken-intrinsics manifest, the stdlib-cache key, the
native linker root/export set, the LLVM internalize mask, the WASM import/export strip, and
the Luau emit-set all answer "is this symbol live?" from the **one** whole-program
`Reachability` fact, and the artifact contains exactly — provably, on every backend, via the
differential corpus that exercises every dynamic path — the code reachable from the program
entry, with the Pythonista's dynamism preserved as recorded `DynReachRoot` edges rather than
erased.

---

*Design only / executable plan. portfolio-architect, 2026-06-24.*
*Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>*
