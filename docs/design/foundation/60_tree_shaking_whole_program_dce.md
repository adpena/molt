<!-- Foundation blueprint 60. Arc: TREE-SHAKING / WHOLE-PROGRAM DEAD-CODE
ELIMINATION — the artifact contains ONLY code reachable from the program entry,
on every backend (native/WASM/LLVM/Luau) and every profile (dev/release-fast/
release-output). The deliverable is ONE generated whole-program REACHABILITY FACT
(call-graph + dynamic-dispatch-aware liveness from entry) that every elimination
tier consumes, retiring the class "reachability is re-derived per tier and the
copies drift." Author: portfolio-architect. Date: 2026-06-24. Status: DESIGN ONLY
/ EXECUTABLE PLAN — no code written in the session that produced it; the lead
integrates. Every load-bearing claim was verified read-only against the worktree
snapshot available on 2026-06-24. Code beats this doc when it drifts — re-verify
against current files and executable tests before acting.

Number 60 chosen: assigned path is `60_tree_shaking_whole_program_dce.md`; slot 60
is free (50-59 taken by the concurrent portfolio arcs; 53 is multiply-occupied by
perf-scoreboards/compression-ladder/cpython-parity, all design-only). This is the
first 60-79 doc.

DEEPENS: 65_perf_compression_ladder.md Rung 8 (artifact-footprint facts) +
59_semantic_fact_plane.md (one generated authority per invariant) and FEEDS the
binary-size dimension of the Performance Constitution. Composes with 21b
(crate-graph), 21d (cli package), 21e (satellite link-only-what's-used). -->

# 60 — Tree-Shaking / Whole-Program Dead-Code Elimination: one reachability fact, every tier

## 0. The end-state outcome (the time-traveler's destination)

**In the end state, a function/method/class/stdlib-symbol/intrinsic that is not
reachable from the program entry CANNOT appear in the artifact — on any backend,
in any profile — because "is this symbol live?" is answered exactly once, by a
single generated whole-program `Reachability` fact, and every elimination tier
(IR dead-function-elim, the stdlib cache key, the IPO call-graph, the
address-taken-intrinsics manifest, the linker root/export set, the WASM
tree-shake) *consumes that fact* rather than re-deriving it.** "I added a symbol
to the dead-set in tier A but tier B kept it alive" stops being expressible: the
reference-edge vocabulary and the root set are generated authorities, and a
backend that emits a symbol absent from the `Reachability` set fails a gate.

Concretely, at the destination:

- **One reachability authority, four+ consumers.** The `Reachability` fact
  (call-graph + dynamic-dispatch-aware liveness + roots) is built once per module
  and is the single source for: `eliminate_dead_functions`
  (`runtime/molt-tir/src/passes.rs:2503`), `CallGraph::reachable_from`
  (`runtime/molt-tir/src/tir/call_graph.rs:564`), the Python stdlib-cache
  reachability `_reachable_function_names_for_stdlib_cache`
  (`src/molt/cli/__init__.py:19218`), `compute_intrinsic_manifest`
  (`passes.rs:4534`), the native linker root/export set
  (`cli/__init__.py:20208`/`:20236`), and the WASM export/tree-shake contract
  (`docs/spec/areas/compiler/0931_LINKER_OPTIMIZATION_CONTRACT.md` §"WASM
  Linking"). Today these are **four hand-mirrored traversals** (§2.1) — the
  duplicate-authority drift class doc 59 exists to kill.

- **The reference-edge vocabulary is a generated `op_kinds.toml` column, not a
  hand-`match`.** "Which op-kinds reference a function by name (and how to derive
  the referenced name)" lives once in the op-kind registry
  (`runtime/molt-tir/src/tir/op_kinds.toml`) and is rendered to both the Rust
  classifier and the Python one — so `_DEAD_FUNCTION_ELIM_REFERENCE_KINDS`
  (`cli/__init__.py:19180`) can no longer drift from the Rust `match
  op.kind.as_str()` arms (`passes.rs:2519-2572`).

- **The artifact is provably minimal, measured.** Binary size (native stripped/
  unstripped; WASM raw/gzip/brotli), function count, import count, and cold-start
  page-in are scoreboard dimensions (doc 65 Rung 8, `docs/perf/SCOREBOARD.md`),
  green vs the CPython floor and ratcheted down. A symbol that survives is a
  *reachability fact*, not a linker accident.

- **The Pythonista keeps dynamism; the Rustacean proves the rest dead.** Dynamic
  `getattr`, `importlib.import_module(name)`, `__init_subclass__`, registries —
  every escape hatch that makes a symbol reachable-by-name is a *recorded edge*
  (a `DynReachRoot` fact, §3 F3), not a reason to keep the whole stdlib. What the
  facts cannot prove reachable-by-a-dynamic-path is `Unknown` → conservatively
  kept (fail-closed); what they *can* prove dead is gone everywhere.

This document is the executable plan that builds that one fact and routes every
tier through it. It is **Rung 8's reachability substrate** (doc 65 §3 Rung 8
names "whole-program reachability/DCE → <2MB binary + per-attr liveness" and
"address-taken-intrinsics" as the facts; this doc *is* those facts) and a **doc 59
fact-family** (one generated authority per invariant, drift uncompilable).

### 0.1 What this doc is NOT (anti-duplication contract)

- It does **not** re-derive the perf compression ladder (doc 65). It supplies the
  *reachability fact* Rung 8 consumes; Rungs 1–7 (RC/dispatch/boxing/shape/loop/
  generator/portable-IR) are referenced, not restated.
- It does **not** re-specify the fact-plane machinery (doc 59). It *uses* the
  op-kind-registry generator (`tools/gen_op_kinds.py`, doc 25/59 §2.1) and the
  generator-manifest meta-gate (doc 59 §3 F1) as the carriers for its generated
  authorities; it registers its new authorities in that manifest.
- It does **not** restate the satellite dedup arc (doc 21e). It composes with
  21e's `LINK_AFFECTING_FEATURES` / tier-feature gating (doc 21e §1.3) as the
  *crate-granularity* dual of this doc's *symbol-granularity* shaking (§5).
- It does **not** re-open the function/crate decomposition (doc 21a/21b). It
  obeys the structural-audit ratchet: its new fact lands as a focused module
  (`reachability_fact.rs`), never in a god-file; it *shrinks* the `cli/__init__.py`
  god-file (`:19180-19301` Python reachability moves to a thin FFI call, §3 F2).

---

## 1. Time-traveler derivation: from the end-state back to the facts to build

Working **backward** from "an unreachable symbol cannot appear in the artifact,
and reachability is answered exactly once":

1. **For "reachability answered once" to hold, the multiple traversals must
   collapse to one producer + many consumers.** → There must be a single typed
   `Reachability` record (a whole-program fact), built by one function, that the
   IR DFE, the IPO call-graph, the intrinsic manifest, the stdlib-cache key, and
   the linker/WASM root sets all read. (Today: §2.1 shows four hand-mirrored
   BFS implementations; this is the structural defect.)

2. **For the reference-edge vocabulary not to drift across producers, it must be
   generated, not hand-written in each language.** → "Which op-kind references a
   function by name, and how to extract the referenced name" is a per-op-kind
   fact → an `op_kinds.toml` column rendered to Rust *and* Python. (Today:
   `passes.rs:2519-2572` Rust `match` and `cli/__init__.py:19180-19205` Python
   `frozenset` are two hand-maintained copies of the *same* list — the exact
   "two tables that can disagree" class doc 59 §0 retires.)

3. **For dynamic reachability (getattr/import_module/subclass-registry) to be
   sound AND precise, every dynamic-keep must be an explicit recorded root, not a
   blanket "keep everything that might be reached dynamically."** → A
   `DynReachRoot` fact family: the frontend/runtime records *which* names a
   dynamic site can resolve (a string constant flowing to `importlib`/`getattr`,
   a `@register`-decorated class, an `__all__` re-export), seeding the BFS;
   anything not so recorded and not statically reachable is provably dead.
   (Today: `compute_intrinsic_manifest`, `passes.rs:4534`, already does exactly
   this for *intrinsic* names — every `const_str` that names a real intrinsic is
   a recorded address-taken root. That mechanism is the template to generalize to
   *Python* dynamic reachability.)

4. **For the fact to survive to every backend, it must live in portable TIR and
   be lowered identically by each backend's link step.** → The `Reachability`
   fact carries, per symbol, *why* it is live (`StaticCall` / `AddressTaken` /
   `DynRoot` / `RuntimeEntrypoint` / `Export`), and each backend's
   root/export/keep set is *derived from the fact*, never re-scraped from
   backend-local state (doc 65 Rung 7, the portable-IR-fact-parity rule; doc 46
   §4.7 "a native win shadowed by a WASM regression is a portable-IR fact gap").

5. **For the fact to be TRUSTED (not a heuristic that silently keeps too much or
   drops too much), it must fail closed and be validated.** → `Unknown`
   reachability ⇒ keep (a missed-shake = a size miss, never a correctness bug); a
   symbol the fact marks dead but a backend still references is a *validator
   failure* (a build-time assertion, not a runtime crash). The intrinsic-manifest
   precedent already fails the build closed on an unknown symbol set
   (`passes.rs:4571` "fails the build closed … rather than guessing and
   re-creating the dangling-relocation corruption") — generalize that discipline.

6. **For the win to be real and durable, the size/footprint dimensions must be
   measured against the CPython floor and ratcheted.** → Each tier's landing
   reports binary size / function count / import count / cold page-in vs CPython,
   classified GREEN/RED_STABLE/DIMENSIONAL_WIN (doc 65 §1, CLAUDE.md tranche
   standard); a regression is a failed landing.

Items 1–2 are the **core structural collapse** (§3 F1/F2). Item 3 is the **dynamic
reachability completeness** generalizing the intrinsic-manifest template (§3 F3).
Item 4 is the **portable-IR closure** across backends (§3 F4). Item 5 is the
**fail-closed validator** (§3 F5). Item 6 is the **measurement bridge** to doc 65
Rung 8 (§7).

---

## 2. Current state (what exists — verified read-only against `main`)

The substrate is real but **fragmented into four+ hand-mirrored reachability
traversals**. This arc is *unification + completion*, not greenfield.

### 2.1 The four reachability traversals that hand-mirror each other (the defect)

| # | authority | where | what it does | drift surface |
|---|---|---|---|---|
| 1 | `eliminate_dead_functions` | `passes.rs:2503` (SimpleIR) | the production DFE: build name→referenced-names via `match op.kind.as_str()` (`:2519-2572`), BFS from roots (`:2581-2621`), `ir.functions.retain(reachable)` (`:2624`) | the `match` reference-kind list |
| 2 | `_reachable_function_names_for_stdlib_cache` | `cli/__init__.py:19218` (Python) | re-implements #1's BFS to decide which stdlib functions enter the shared-stdlib cache key (`:19320`) | `_DEAD_FUNCTION_ELIM_REFERENCE_KINDS` frozenset (`:19180`) **hand-mirrors #1's match arms**; `_is_protected_runtime_entrypoint` (`:19276`) hand-mirrors #1's `is_protected_runtime_entrypoint` (`passes.rs:2635`) |
| 3 | `CallGraph` + `reachable_from` | `call_graph.rs:241` build, `:564` BFS (TIR) | the IPO call-graph: `classify_call_op` (`:262`) edges, `reachable_from` "the same traversal shape as `passes::eliminate_dead_functions`" (`:557`, its own comment) | `classify_call_op`'s edge vocabulary is a *third* copy of "which op references a function"; `alloc_task_poll_target` (`:277`) re-derives the `_poll` rule #1 has at `:2546` |
| 4 | `compute_intrinsic_manifest` | `passes.rs:4534` | the address-taken-intrinsics root: every `const_str` naming a real intrinsic (`:4577-4586`) → kept-alive manifest → native app resolver (`simple_backend.rs`/`llvm_backend/mod.rs:315`) | separate *kind* of reachability (address-taken, not call-edge) but **not unified** with #1's root machinery |

**The class:** *reachability re-derivation.* Four traversals each reconstruct
"what is live from entry"; the reference-edge vocabulary exists in three
hand-maintained copies (Rust match #1, Python frozenset #2, `classify_call_op`
#3) and the root set in two (#1 Rust, #2 Python). A new call-like op-kind (e.g. a
new `call_*` variant) added to #1 but not #2 silently changes the cache key vs the
actual DFE; added to #1 but not #3 makes the IPO tier under-approximate
reachability. This is precisely doc 59 §0's "two authorities for one invariant"
and "a new member the oracle silently defaults" — here applied to *reachability
edges* and *roots*.

### 2.2 The elimination tiers that consume reachability (where the fact lands)

- **IR tier (native).** `eliminate_dead_functions` runs in the native pipeline:
  `simple_backend.rs:2279`, `:2852`, `:3233` (after inlining,
  `simple_backend.rs:2435`-equivalent in WASM). `eliminate_dead_imports`
  (`passes.rs:3462`) prunes unconsumed `import_name`/`import_from` ops per
  function. Both gated by env (`MOLT_DISABLE_DEAD_FUNC_ELIM`,
  `MOLT_DISABLE_DEAD_IMPORT_ELIM`) — diagnostic escape hatches only.
- **IR tier (WASM).** `eliminate_dead_functions` + `eliminate_dead_imports` run
  in `wasm.rs:2436-2437` (after inlining, `:2435`). The WASM DFE BFS is therefore
  the *same* `passes.rs` authority #1 — good; the WASM tier already shares the IR
  authority. The WASM *import* surface (`wasm.rs` ~624 `add_import`,
  `docs/architecture/wasm-import-stripping.md`) is a separate tree-shake handled
  by `--wasm-profile pure` + `wasm-opt --remove-unused-module-elements` (post-link).
- **Linker tier (native).** Per-*symbol* DCE via section GC:
  macOS `-Wl,-dead_strip` + `-exported_symbols_list` (`cli/__init__.py:20202-20209`),
  Linux `-ffunction-sections`/`-fdata-sections` + `--gc-sections` + version-script
  `{global: main; local: *;}` (`:20220-20237`), Windows `/OPT:REF` (`:20241`),
  plus `_post_link_strip` (`:20252`). The archive is linked WITHOUT
  `--whole-archive` so the linker only pulls referenced objects (`:20165-20168`).
  The intrinsic manifest (#4) is what keeps name-resolved intrinsics from being
  stripped (`llvm_backend/mod.rs:308` "`-dead_strip`/`--gc-sections` still removes
  every intrinsic whose name appears in no manifest record").
- **Linker tier (LLVM).** Same native link path; the LLVM backend additionally
  has the app-resolver `emit_app_resolver_function` (`llvm_backend/mod.rs:315`)
  taking intrinsic addresses (the LLVM dual of the Cranelift emitter). **LLVM
  `internalize` + `globaldce` are NOT yet applied at the module level before
  hand-off to the linker** (grep: `internalize`/`globaldce` appear only in this
  doc's target text and the roadmap, not in `llvm_backend/`) — §3 F4 adds them so
  the LLVM module is shaken in-IR, not only by the system linker.
- **Linker tier (WASM).** `wasm-ld --gc-sections` (default for size), `--export-if-defined` for optional exports, no `--export-all`
  (`0931_LINKER_OPTIMIZATION_CONTRACT.md` §"WASM Linking"); post-link `wasm-opt`.
- **Satellite tier (21e).** Crate-granularity: `LINK_AFFECTING_FEATURES` +
  `_runtime_feature_gates.py` decide *which satellite crates* link into a tier
  (doc 21e §1.3). This is the coarse dual of symbol shaking (§5).

### 2.3 The DCE/reachability passes (the intra-function complement, already sound)

- `tir/passes/dce.rs` — intra-function dead-op removal (use-count fixpoint,
  `:145-255`) + unreachable-block removal seeded by
  `metadata_preserving_reachable_blocks` (`reachability.rs:33`). Effect-aware via
  the central effects oracle (`op_has_observable_effect_when_dead`,
  `op_may_throw` — `dce.rs:18`). **Correct and not part of the drift problem** —
  it operates *within* a function; this arc is *whole-program* (inter-function /
  inter-module). They compose: DCE shrinks bodies, DFE removes whole functions.
- `tir/passes/dead_store_elim.rs` — dead-store elimination (orthogonal; memory
  writes, not symbol reachability).
- `tir/passes/reachability.rs` — *block* reachability within a function (the
  CFG-edge BFS, `:33-60`). Note: this is block-level, not function-level — name
  collision with the arc, but a different scope. The whole-program fact (§3) is a
  new module to avoid overloading this one.

### 2.4 The op-kind registry (the generator this arc's authorities ride on)

- `op_kinds.toml` (3146 lines) + `gen_op_kinds.py` (2761 lines) render
  `op_kinds_generated.rs` + `op_kinds_generated.py` (doc 59 §2.1). Already renders
  per-OpCode facts to BOTH Rust and Python — exactly the carrier needed for the
  reference-edge column (§3 F1). The frontend `op.kind` tables
  (`frontend_raising_kind`/`binary_op`/…) prove the SimpleIR-`kind`-string →
  generated-Python-predicate path already exists.
- **Caveat (verified):** #1/#2/#3 key on the *SimpleIR* `op.kind` *string*
  (`"call"`, `"call_internal"`, `"generator_create"`, …), not the TIR `OpCode`
  enum. The op-kind registry's primary domain is the TIR `OpCode` enum; the
  frontend wire-kind tables are the *open-domain* string side (doc 59 §5.2). The
  reference-edge fact is therefore an **open-domain** fact (a `[[reference_kind]]`
  table over SimpleIR kind strings) with the fail-closed `audit_op_kinds.py`
  producer-drift complement — NOT a closed-enum exhaustive match. This is the
  correct shape and is called out so the implementer does not force it into the
  closed-`OpCode` mold.

---

## 3. The structural facts / mechanisms this arc builds (each tied to the class it retires)

The deliverable is **not "smaller binaries"** — it is **one whole-program
reachability fact that makes "reachability re-derived per tier" unexpressible.**
Five mechanisms.

### F1. The generated reference-edge + root vocabulary — retires *"the reachability edge/root list drifts across Rust, Python, and the IPO call-graph"*

The single declarative source for "which op-kind references a function by name,
how to derive the name, and what the reachability roots are."

- **Artifact:** new sections in `op_kinds.toml`:
  - `[[reference_kind]]` rows: one per SimpleIR `op.kind` string that can
    reference a function by name, each row carrying `kind = "..."`,
    `name_source = "s_value"` (the field the referenced name comes from), and
    `derives_poll = true|false` (whether `{name}_poll` is also implied — the
    `generator_create`/`coro_create` rule at `passes.rs:2546` /
    `cli/__init__.py:19258`). The 24 kinds are exactly the union of
    `passes.rs:2519-2572` and `cli/__init__.py:19180-19205` (verified identical
    today — this fact *locks* that identity).
  - `[[reachability_root]]` rows: the root set — exact names
    (`molt_main`, `molt_host_init`, `_start`) + prefixes (`molt_isolate_`) +
    the entry-function rule (functions[0]) + the stdlib `molt_init_{module}`
    rule. Replaces the hand-duplicated `is_protected_runtime_entrypoint`
    (`passes.rs:2635`) and `_is_protected_runtime_entrypoint`
    (`cli/__init__.py:19276`).
- **Generator:** extend `gen_op_kinds.py` to render
  `reference_kind`/`reachability_root` predicates into `op_kinds_generated.rs`
  (a `fn reference_edge(kind: &str) -> Option<ReferenceEdgeKind>` + a
  `fn is_reachability_root(name: &str) -> bool`) AND into
  `op_kinds_generated.py` (the frozenset + a root predicate). `--check`-gated
  (doc 59 §2.1) so a hand-written second copy is caught.
- **Validation (the cross-axis kill, doc 59 §2.1 lesson):** the generator asserts
  the `reference_kind` set is identical across the Rust and Python renders (they
  are two views of one fact) and that every `derives_poll` kind is also a
  `reference_kind`. A row present in one render but not the other is a generator
  failure — drift is uncompilable.
- **Class retired:** *reachability-vocabulary drift* (edge list + root list, the
  #1↔#2↔#3 hand-mirror of §2.1).

### F2. The unified whole-program `Reachability` fact — retires *"each tier re-implements the BFS"*

One typed record, built once, consumed by every tier.

- **Artifact:** new `runtime/molt-tir/src/reachability_fact.rs` (SimpleIR scope —
  it must run where `eliminate_dead_functions` runs, on `SimpleIR`, before TIR
  lifting) exposing:
  ```rust
  pub struct Reachability {
      /// reachable symbol -> the reason(s) it is live (why it survives)
      pub live: BTreeMap<String, LiveReason>,
      /// the recorded dynamic roots that seeded beyond static edges
      pub dyn_roots: BTreeSet<String>,
  }
  pub enum LiveReason {            // why a symbol is in the artifact
      RuntimeEntrypoint,           // F1 reachability_root
      EntryModule,                 // functions[0]
      StaticEdge,                  // reached via an F1 reference_kind edge
      AddressTaken,                // const_str names it (the intrinsic-manifest shape)
      DynRoot(DynRootKind),        // F3: getattr/import_module/subclass-registry
      Export,                      // a backend export contract requires it
  }
  pub fn compute(ir: &SimpleIR, roots: &ReachabilityRoots) -> Reachability;
  ```
  `compute` builds the name→refs map using the F1 generated `reference_edge`,
  seeds from F1 `is_reachability_root` + `dyn_roots` (F3), runs ONE BFS, and
  records `LiveReason` per symbol. `eliminate_dead_functions` becomes a thin
  consumer: `ir.functions.retain(|f| reachability.live.contains_key(&f.name))`.
- **Consumers (all read the one fact; none re-derives):**
  1. `eliminate_dead_functions` (`passes.rs:2503`) — retain on `reachability.live`.
  2. `CallGraph::reachable_from` (`call_graph.rs:564`) — the IPO tier consumes the
     same edge vocabulary (F1) for `classify_call_op` and the same BFS; the "same
     traversal shape" comment (`:557`) becomes "the same traversal *code*."
  3. `compute_intrinsic_manifest` (`passes.rs:4534`) — the `AddressTaken` reason
     IS the intrinsic manifest; the manifest is *projected* from
     `reachability.live` (filter `LiveReason::AddressTaken` ∩ intrinsic symbols)
     instead of a separate scan. (Keeps the fail-closed symbol-set precondition,
     `passes.rs:4571`.)
  4. The Python stdlib-cache reachability (`cli/__init__.py:19218`) — **deleted as
     a re-implementation** and replaced by an FFI call into the Rust
     `Reachability::compute` over the same IR (the backend already exposes
     `compute_intrinsic_manifest` etc. through `molt-backend`'s public surface,
     `lib.rs:43-46` — add a `reachable_function_names` export). This **shrinks the
     `cli/__init__.py` god-file** (`:19180-19301`, ~120 lines) to a thin call,
     advancing doc 21d and the structural-audit ratchet (doc 59 §6).
  5. The native linker root/export set (`cli/__init__.py:20208`/`:20236`) and the
     WASM export contract — derived from `LiveReason::Export` ∪ `RuntimeEntrypoint`
     (§3 F4).
- **Class retired:** *BFS re-implementation* (the four traversals collapse to one
  producer + projections).

### F3. The `DynReachRoot` fact family — retires *"dynamic reachability is handled by keeping too much (whole stdlib) or too little (silent miss)"*

Generalizes the intrinsic-manifest's "every `const_str` naming a real symbol is a
recorded address-taken root" (`compute_intrinsic_manifest`, `passes.rs:4534-4586`)
from *intrinsics* to *all Python dynamic reachability*.

- **The classes of dynamic reachability** (each a `DynRootKind`):
  - **String→callable** — a `const_str` flowing to `getattr`/`importlib.import_module`/
    `__import__`/`operator.attrgetter`. The value is a recorded root (the exact
    intrinsic-manifest mechanism, lifted to user/stdlib symbols). When the string
    is *not* a constant (computed at runtime), the receiver's whole symbol family
    is `Unknown` → kept (fail-closed). This is the Pythonista escape hatch made a
    *fact*: `getattr(obj, name)` keeps what `name` can be, proven where possible.
  - **Subclass/registry** — `__init_subclass__`, `@register`, `ABCMeta` registries,
    `__subclasshook__`: a class reachable only via a registry the runtime walks is
    a `DynRoot(Registry)`. The frontend already knows the decorator/metaclass shape
    (the `class_def`/`decorator` reference-kinds, `passes.rs:2564`); F3 records the
    class as a root when a registry-keeping decorator/metaclass is present.
  - **Re-export** — `__all__` / `from m import *`: a name re-exported is reachable
    from any importer of the module (a `DynRoot(ReExport)`); the import machinery
    (`import_name`/`import_from`, `passes.rs:2564`) is the edge source.
- **Where it is produced:** the frontend (which has the AST and knows
  `getattr`/`import_module`/decorator shapes) emits `DynReachRoot` records into the
  IR (a new lightweight op or a function-level attribute), analogous to how the
  intrinsic names already flow as `const_str` the manifest scans. The runtime
  contributes the *intrinsic* dyn-roots it already knows.
- **The soundness contract (fail-closed):** `Reachability::compute` seeds the BFS
  with `dyn_roots`. A dynamic site whose target the frontend CANNOT prove (runtime
  string, reflective walk over an open set) marks its *candidate family* `Unknown`
  → kept. **No program is ever mis-shaken** (a wrongly-dropped symbol is a
  correctness bug — forbidden); the only outcome of imprecision is a *larger*
  artifact (a size miss). Coverage grows monotonically: more dynamic sites
  proven ⇒ smaller artifacts, measured by §7's coverage ratchet.
- **Class retired:** *dynamic-reachability-by-blunt-instrument* (keep-the-world or
  silent-drop) → recorded, proven-where-possible, fail-closed roots.

### F4. The portable-IR backend reachability closure — retires *"a tier shakes on one backend but not another"*

Every backend's keep/export/strip set is *derived from the one `Reachability`
fact*, and a backend that emits a symbol absent from the fact fails a gate.

- **Native (Cranelift) + LLVM:** the IR DFE (F2) already runs before codegen on
  both (`simple_backend.rs:2279`, etc.). **Add LLVM module-level
  `internalize` + `globaldce`** in `llvm_backend/mod.rs`: after emitting the
  module, internalize every symbol *not* in `reachability.live`'s `Export` ∪
  `RuntimeEntrypoint` set (so LLVM's `globaldce` can delete it), mirroring the
  native version-script `{global: main; local: *;}` (`cli/__init__.py:20236`). The
  linker root/export sets (`-exported_symbols_list`, the version script) are
  *generated from* `LiveReason::Export ∪ RuntimeEntrypoint`, not hand-listed as
  `_main`/`main` only.
- **WASM:** the IR DFE (F2) runs (`wasm.rs:2436`); the *import* surface
  (`add_import`, `wasm-import-stripping.md`) is shaken by the same fact — an import
  whose only callers are now-dead functions is dropped pre-link, and
  `--export-if-defined` exports come from `LiveReason::Export`. Post-link `wasm-opt
  --remove-unused-module-elements` is the belt-and-suspenders DCE
  (`0931_LINKER_OPTIMIZATION_CONTRACT.md`). The `--wasm-profile pure` category
  strip becomes a *consequence* of the reachability fact (the IO/async/time
  imports are dead because no reachable function calls them) rather than a
  hand-curated category list.
- **Luau:** Luau transpiles to a script; tree-shaking is *emit-time* — only
  functions in `reachability.live` are emitted to the Luau output. The same fact
  drives "which `local function` definitions appear." (Luau has no linker; the IR
  DFE is the *only* tier, so the fact is load-bearing there with no linker
  backstop — making F2 correctness-critical for Luau, per doc 65 Rung 7.)
- **The generated backend support matrix (doc 65 Rung 7 / doc 46 §4.7):** a
  `tools/backend_reachability_audit.py` checks each backend's *actual* emitted
  symbol set against `reachability.live` — a symbol emitted but not live, or live
  but not emitted, is a drift failure (the dual of `audit_op_kinds.py`). This is
  the "fact survives to every backend" gate.
- **Class retired:** *backend-local reachability* (a native shake with a
  WASM/LLVM/Luau gap — doc 46 §4.7's portable-IR-fact-gap class).

### F5. The fail-closed reachability validator — retires *"a too-aggressive shake silently drops a live symbol (corruption) / a too-conservative one silently keeps the world (no gate)"*

The checkable obligation that makes a wrong shake a *build error*, not a runtime
crash or a silent size regression.

- **Drop-soundness (the corruption guard):** a `MOLT_VERIFY_REACHABILITY=1`
  self-check (mirroring `MOLT_VERIFY_ANALYSIS=1`, doc 65 §1) that, after DFE,
  re-scans the retained IR for any reference (via the F1 edge vocabulary) to a
  *removed* symbol — a dangling reference is a panic-in-debug / hard-fail-in-CI,
  never a silent emit. This is the generalization of the intrinsic-manifest's
  "fails the build closed rather than emitting a corrupt binary"
  (`passes.rs:4571`) to the whole reachability set.
- **Backend-emit cross-check (F4's audit):** `backend_reachability_audit.py` —
  every backend's emitted symbols ⊆ `reachability.live` ∪ runtime-staticlib
  symbols (the linker resolves the latter); a backend emitting a symbol the fact
  says is dead is a drift failure.
- **Coverage ratchet (the too-conservative guard):** `tools/reach_coverage.py`
  (mirroring `call_fact_coverage.py`, doc 59 §2.3) — tracks, per module, the
  fraction of dynamic sites with a *proven* `DynReachRoot` vs `Unknown`. `--check`
  fails if proven-coverage DECREASES. A blunt "keep the world" regression (someone
  marks a whole family `Unknown` to "fix" a miss) is caught as a coverage drop.
- **Class retired:** *unvalidated shake* (silent over-drop = corruption; silent
  over-keep = no signal). Both become gated.

---

## 4. Phases (dependency order; each independently landable with green gates)

Each phase is a **complete structural piece** (CLAUDE.md unit-of-work rule).
Lane assignment per the council three-lane model (doc 65 §6): mostly **lane C**
(infra/footprint) with a **lane B** (perf-frontier) bridge for the backend
lowering; the IR-correctness pieces touch the safety-adjacent DFE so carry
A-lane discipline (differential parity on every backend).

### Phase 0 — Lock the current identity + characterize the size baseline. *Do first; no behavior change.*
- **Why first:** F1 *locks* that the Rust edge list (`passes.rs:2519-2572`) and
  the Python frozenset (`cli/__init__.py:19180`) are identical TODAY. Before
  generating them from one source, prove they are byte-equivalent so the
  generation is provably behavior-preserving (if they have *already* drifted, that
  is a latent bug this phase surfaces — the deliverable, doc 59 §10 risk row).
- **Do:** (a) a one-shot reconciliation test
  (`tests/test_reachability_vocab_parity.py`) asserting the Rust and Python
  reference-kind sets + root rules match (parse both, diff). (b) Wire the size
  scoreboard dimensions (binary size native stripped/unstripped, WASM raw/gzip,
  function count, import count) into `tools/perf_scoreboard.py` for a baseline
  artifact set (doc 65 Rung 8, `docs/perf/SCOREBOARD.md` schema) so every later
  phase reports a size delta. (c) `MOLT_DEBUG_DEAD_FUNC_ELIM` /
  `MOLT_DEBUG_DEAD_IMPORT_ELIM` (`passes.rs:2627`/`:3506`) already exist — confirm
  they emit removed-count; add a one-line "live/total + reason histogram" debug
  dump as the future fact's observability.
- **Gate:** the parity test GREEN (or the drift surfaced + filed); the size
  scoreboard emits a baseline JSON committed under `bench/scoreboard/`. No code
  behavior change.

### Phase 1 — Generate the reference-edge + root vocabulary (F1).
- **Do:** add `[[reference_kind]]` + `[[reachability_root]]` to `op_kinds.toml`;
  extend `gen_op_kinds.py` to render `reference_edge`/`is_reachability_root` into
  both `op_kinds_generated.rs` and `op_kinds_generated.py`; add the cross-render
  agreement validation (§3 F1). Replace the hand `match`
  (`passes.rs:2519-2572`) + `is_protected_runtime_entrypoint` (`:2635`) and the
  Python frozenset (`cli/__init__.py:19180`) + `_is_protected_runtime_entrypoint`
  (`:19276`) with calls to the generated predicates. Register both new sections in
  `tools/generator_manifest.toml` (doc 59 §3 F1) as open-domain facts with the
  `audit_op_kinds.py` producer-drift complement.
- **Files:** `op_kinds.toml`, `gen_op_kinds.py`, `op_kinds_generated.rs`,
  `op_kinds_generated.py`, `passes.rs` (consume), `cli/__init__.py` (consume),
  `generator_manifest.toml`, `tests/test_gen_op_kinds.py` (extend).
- **Gate:** `gen_op_kinds.py --check` GREEN; the Phase-0 parity test now passes
  *because both sides read the generated predicate*; `cargo test -p molt-tir` +
  `cargo test -p molt-backend` byte-identical DFE behavior (the vocabulary is the
  same, only its source changed); differential parity unaffected (representation
  of the *classifier*, not behavior).

### Phase 2 — The unified `Reachability` fact + collapse the four traversals (F2).
- **Do:** author `runtime/molt-tir/src/reachability_fact.rs` with `Reachability` +
  `LiveReason` + `compute` (§3 F2), built on F1's vocabulary. Rewrite
  `eliminate_dead_functions` to consume it. Rewrite `CallGraph` edge build +
  `reachable_from` to consume the same edge vocabulary + BFS (delete the
  "same shape" duplication, `call_graph.rs:557`). Project
  `compute_intrinsic_manifest` from `LiveReason::AddressTaken`. Expose
  `reachable_function_names` via `molt-backend`'s public surface (`lib.rs:43-46`)
  and **delete** the Python re-implementation
  `_reachable_function_names_for_stdlib_cache` (`cli/__init__.py:19218`), replacing
  it with the FFI call (shrinks the god-file).
- **Files (new):** `reachability_fact.rs`. **Edit:** `passes.rs` (DFE + manifest
  projection), `call_graph.rs` (consume), `lib.rs` (export), `cli/__init__.py`
  (delete the Python BFS, call the export). **Validator:** add
  `MOLT_VERIFY_REACHABILITY=1` drop-soundness self-check (F5).
- **Gate:** `cargo test -p molt-tir` + `-p molt-backend` GREEN; the stdlib-cache
  key is byte-identical to Phase-0 (the FFI call returns the same set the Python
  BFS did — prove with a fixture comparing old-Python-impl vs new-FFI on saved
  IR); `MOLT_VERIFY_REACHABILITY=1` finds zero dangling refs on the differential
  corpus; full differential parity native+LLVM+WASM+Luau; size DIMENSIONAL check
  (should be unchanged — same reachability set, one authority now).

### Phase 3 — Dynamic reachability completeness (F3).
- **Do:** define `DynReachRoot` + `DynRootKind`; have the frontend emit dyn-roots
  for the three classes (string→callable proven const, subclass/registry,
  re-export — §3 F3); seed `Reachability::compute` with them; mark unprovable
  dynamic sites' families `Unknown`→kept (fail-closed). Add `tools/reach_coverage.py`
  (the proven-vs-Unknown ratchet, F5). This is where the *real shake* happens —
  previously-kept-conservatively stdlib symbols now drop when no reachable static
  OR proven-dynamic path reaches them.
- **Files:** the frontend dyn-root emission (`src/molt/frontend/...` — the
  `getattr`/`import`/decorator lowering, the same sites `passes.rs:2564`
  reference-kinds cover), `reachability_fact.rs` (seed), `reach_coverage.py` (new),
  `generator_manifest.toml` (register the coverage ratchet).
- **Gate:** `reach_coverage.py --check` GREEN (coverage UP from Phase 2);
  differential parity on the FULL stdlib differential corpus (`tests/differential/`)
  — the shake must NOT drop a symbol any test reaches dynamically (the
  correctness floor); binary-size scoreboard shows a measured DROP vs Phase 0
  baseline, classified DIMENSIONAL_WIN (native + WASM + Luau); a deliberate
  "dynamic getattr keeps the family" regression test proves fail-closed.

### Phase 4 — Backend reachability closure (F4) + the audit (F5).
- **Do:** generate the native linker root/export set + LLVM `internalize` mask
  from `LiveReason::Export ∪ RuntimeEntrypoint` (replace the hand `_main`/`main`
  lists, `cli/__init__.py:20208`/`:20236`); add LLVM module-level `internalize` +
  `globaldce` in `llvm_backend/mod.rs`; drive the WASM import strip + exports from
  the fact; make Luau emit only `reachability.live`. Build
  `tools/backend_reachability_audit.py` (emitted-symbols ⊆ live ∪ runtime, per
  backend) and gate it in CI. Register the backend matrix rows (doc 65 Rung 7).
- **Files:** `cli/__init__.py` (link/export derivation), `llvm_backend/mod.rs`
  (internalize/globaldce), `wasm.rs` (import strip from fact), the Luau emitter,
  `backend_reachability_audit.py` (new), `.github/workflows/ci.yml` (gate).
- **Gate:** `backend_reachability_audit.py --check` GREEN on all four backends;
  binary-size scoreboard shows a measured DROP on native (LLVM internalize) AND
  WASM (import strip) AND Luau (emit shake), each classified; full differential
  parity all backends; the LLVM lane size delta recorded (the
  `0931_LINKER_OPTIMIZATION_CONTRACT.md` before/after discipline); cold-start
  page-in DIMENSIONAL check (smaller artifact → fewer pages, doc 65 Rung 8).

### Phase 5 (ongoing, composes with 21e) — per-attribute / per-method liveness + crate-granularity.
- **Do:** extend reachability below the function granularity to *methods* and
  *fields* (a dead method on a live class; a never-read field — ties doc 65 Rung 4
  ShapeFacts `FieldSlot` and doc 09/13 dead-field) and compose with 21e's
  `LINK_AFFECTING_FEATURES` so a satellite crate links into a tier ONLY when the
  reachability fact shows a live symbol from it (the symbol-granularity fact
  *informs* the crate-granularity gate — doc 21e §1.3, §5 here). This is the
  "<2MB binary + per-attr liveness" terminus (doc 65 Rung 8 / doc 51 §6).
- **Gate per increment:** `reach_coverage.py` UP; the relevant size dimension
  DOWN; differential parity; the 21e satellite-parity guard
  (`check_satellite_parity.py`) unaffected (this *reduces* what links, never
  changes satellite/in-tree equivalence). **Unbounded** — the monthly cadence
  (doc 65 §7) for the footprint class.

**Landing report format (every phase — doc 65 §1 + CLAUDE.md PERF/SPEED block):**
"tests green; the named gate(s) green; binary size / function count / import count
delta vs CPython floor AND vs prior phase, classified GREEN/RED_STABLE/
DIMENSIONAL_WIN; cold page-in delta; zero new hand-mirrored reachability code;
zero dropped-live-symbol differential failures (the correctness floor)."

---

## 5. Composition with the decomposition (21a–e) and the 50–69 arcs

- **Doc 65 (perf compression ladder) Rung 8 — the primary parent.** Rung 8 names
  "whole-program reachability/DCE → <2MB binary + per-attr liveness" and
  "address-taken-intrinsics" as its facts (doc 65 §3 Rung 8). This arc *builds
  those facts*: F2 is the reachability fact Rung 8 consumes; F3's `AddressTaken`
  reason IS the address-taken-intrinsics fact; Phase 5 IS per-attr liveness. The
  cross-arc dependency: **Rung 8 depends on this arc's `Reachability` fact**, and
  this arc depends on Rung 8's scoreboard cold/size dimensions (doc 65 Rung 0) for
  its gates. Per-attr liveness (Phase 5) *also* depends on Rung 4 ShapeFacts
  (`FieldSlot`) and Rung 2 CallFacts targets (to know which methods are reachable)
  — so Phase 5 co-schedules after those rungs (doc 65 §7 M4/M9).
- **Doc 59 (semantic fact plane) — the machinery.** F1's generated edge/root
  vocabulary follows doc 59 §5.2 (open-domain fact: table + `--check` + producer
  drift audit). F2/F3 register in the generator manifest (doc 59 §3 F1). F5's
  coverage ratchet is the positive dual (doc 59 §2.3). This arc is a *consumer* of
  the fact-plane institution doc 59 builds — it does not re-derive it.
- **Doc 21d (cli package decomposition).** F2 *deletes* ~120 lines of Python
  reachability re-implementation from the `cli/__init__.py` god-file
  (`:19180-19301`) and replaces it with a thin FFI call — a direct contribution to
  21d and a *reduction* of the structural-audit god-file ratchet (doc 59 §6). The
  new fact lands as a focused `reachability_fact.rs` module, never in a god-file
  (doc 21b precise-visibility discipline).
- **Doc 21e (satellite dedup).** This arc's *symbol*-granularity shake and 21e's
  *crate*-granularity gate (`LINK_AFFECTING_FEATURES`,
  `_runtime_feature_gates.py`, doc 21e §1.3) are duals. Phase 5 closes the loop:
  the reachability fact tells the tier-feature gate whether a satellite crate has
  any live symbol, so a tier links a satellite ONLY when needed — making 21e's
  "link only what's used" *fact-driven* instead of feature-flag-curated. This arc
  must not disturb 21e's parity guard (it reduces what links; it never changes
  satellite↔in-tree equivalence).
- **Doc 21b (crate graph).** The reachability fact lives in `molt-tir` (the IR
  crate) and is consumed by `molt-backend` and the CLI through the existing public
  surface (`lib.rs:43-46`) — respecting the crate dependency direction; no new
  cross-crate cycle.

---

## 6. The cross-tier invariants (what keeps this one fact, not six)

1. **One reachability authority.** `Reachability::compute` (F2) is the single
   producer; DFE, IPO call-graph, intrinsic manifest, stdlib-cache key, linker
   root/export, WASM/Luau emit are *projections/consumers*. No tier re-implements
   the BFS or the edge vocabulary.
2. **One generated edge/root vocabulary.** The `reference_kind`/`reachability_root`
   tables (F1) render to both Rust and Python; a new call-like kind is a table row
   (doc 59 §5.2), never a hand-`match` in `passes.rs` or a hand-frozenset in
   `cli/__init__.py`.
3. **Fail-closed everywhere.** `Unknown` reachability ⇒ keep (a missed shake = a
   size miss, never a correctness bug). A dropped-live symbol is a *validator
   failure* (F5), never a runtime crash. This mirrors the intrinsic-manifest's
   build-fails-closed precedent (`passes.rs:4571`).
4. **One validator discipline.** Each tier's "done" ships the F5 obligations:
   `MOLT_VERIFY_REACHABILITY=1` drop-soundness, `backend_reachability_audit.py`
   emit-cross-check, `reach_coverage.py` proven-coverage ratchet. A shake without
   a validator is a half-fact (doc 65 §1).
5. **One measurement contract.** Each tier's win is a GREEN size/cold scoreboard
   row vs the CPython floor (doc 65 §1), classified; a size or cold regression is
   a failed landing.
6. **No second authority** (doc 59 §0). The `AddressTaken` reason IS the intrinsic
   manifest; the IPO `reachable_from` IS the DFE BFS; the linker export set IS
   `LiveReason::Export`. Consumers *reference and project*; they never re-derive.

---

## 7. Measurement + gates (the Performance Constitution dimensions)

Per CLAUDE.md, binary size, peak RSS, compile time, and COLD vs WARM start are
tracked dimensions; cold-start is an *artifact-footprint/page-in/codesign*
problem (NOT runtime-init, measured 0.127ms — doc 65 Rung 8). This arc's product
is a *smaller artifact*, so its primary dimensions are **footprint**, not warm
throughput.

Every phase reports, via `tools/perf_scoreboard.py` (the size/cold extension,
`docs/perf/SCOREBOARD.md` schema):

`artifact → target(native/LLVM/WASM/Luau) → profile(dev-fast/release-fast/
release-output) → binary size (native stripped+unstripped; WASM raw+gzip+brotli)
→ function count → import count → cold page-in / startup_tax → CPython size ratio
→ compile-time delta → log artifact`, with ≥5 samples, CV stability,
classification GREEN / RED_STABLE / RED_NOISY / TIE / DIMENSIONAL_WIN.

The standing gates (CI):
- `gen_op_kinds.py --check` (F1 vocabulary generated, drift uncompilable).
- `check_generator_manifest.py --check` (doc 59) — F1/F2/F3 authorities registered.
- `backend_reachability_audit.py --check` (F4/F5) — emitted ⊆ live, all 4 backends.
- `reach_coverage.py --check` (F5) — proven dynamic-reachability coverage UP.
- `MOLT_VERIFY_REACHABILITY=1` on the differential corpus (F5) — zero dangling refs.
- The size scoreboard ratchet — binary size / import count DOWN (or DIMENSIONAL
  justification), never silently UP (doc 65 Rung 8, the silent-footprint-regression
  class).

**The correctness floor (non-negotiable, gates every phase):** the full
differential corpus (`tests/differential/`) passes on native+LLVM+WASM+Luau — the
shake must NEVER drop a symbol any test reaches statically OR dynamically. A
dropped-live symbol is a P0 correctness bug (CLAUDE.md), not a size tradeoff.

---

## 8. Risks + structural (not band-aid) treatment

| risk | where it bites | STRUCTURAL treatment (no band-aid) |
|---|---|---|
| **The Rust and Python edge lists have ALREADY drifted** (latent) | Phase 0/F1 | Phase 0's parity test SURFACES it as the deliverable (doc 59 §10 row); the FINDING (which side is wrong) is a correctness win, resolved before generation. Never paper over by picking a side silently — the differential corpus is the oracle for which list is correct. |
| **A dynamic `getattr`/`import_module` reaches a symbol the shake dropped** (corruption) | Phase 3/F3 | Fail-closed by construction: an unprovable dynamic site marks its candidate family `Unknown`→kept. `MOLT_VERIFY_REACHABILITY=1` (F5) + the full differential corpus gate catch any dropped-live symbol at build/CI time, never at runtime. The intrinsic-manifest precedent (`passes.rs:4571`) proves the discipline works. |
| **Aggressive shake breaks the runtime's extern-`C` entrypoints** (`molt_isolate_*`, importlib helpers) | Phase 2/F2 | The `reachability_root` table (F1) is the *generated* authority for protected entrypoints (replacing the hand `is_protected_runtime_entrypoint`); a missing root is a table edit + a differential failure, not a silent strip. The native-bootstrap regressions (`tests/test_native_import_bootstrap_regressions.py`) gate the import paths (CLAUDE.md Bootstrap Authority). |
| **LLVM `internalize`/`globaldce` strips an address-taken intrinsic** | Phase 4/F4 | `internalize` mask is *derived from* `LiveReason::Export ∪ RuntimeEntrypoint ∪ AddressTaken` — the same fact that already keeps intrinsics alive on native (`llvm_backend/mod.rs:308`); `backend_reachability_audit.py` cross-checks the emitted set. An internalized-then-deleted live symbol is a gate failure, caught pre-link. |
| **The stdlib-cache key changes** when the Python BFS is replaced by the FFI call (cache invalidation storm) | Phase 2/F2 | The gate proves byte-identity (old-Python-impl vs new-FFI on saved IR) BEFORE the swap; the key is computed from the *same* reachable set, so the cache stays valid. If the sets differ, the Python impl had a bug (the deliverable). |
| **WASM/Luau lose a shake that native keeps (or vice versa)** | Phase 4/F4 | Rung 7 portable-IR-fact-parity rule (doc 65): the fact lives in portable TIR (F2, SimpleIR scope before backend split); `backend_reachability_audit.py` gates all four backends on the SAME fact. A backend-only gap is a portable-IR-fact-gap (doc 46 §4.7), gated, not excepted. |
| **`reach_coverage.py` becomes a Goodhart target** (mark families `Unknown` to dodge a miss, inflating "kept" but hiding precision loss) | Phase 3/F5 | The ratchet is proven-coverage (DOWN-fails), so a blunt `Unknown` regression DROPS coverage and fails the gate; it is the dual of the size ratchet (more `Unknown` ⇒ bigger artifact ⇒ size ratchet also reds). Two ratchets in opposition make the dodge fail both. |
| **Per-attr liveness (Phase 5) is a large greenfield; risk of a partial system** | Phase 5 | The `FactValue` fail-closed lattice makes partiality SOUND (doc 65 §8): an un-analyzed method/field is `Unknown`=kept (current behavior). Coverage grows monotonically via `reach_coverage.py`; no program is miscompiled by a missing per-attr fact, only un-shrunk. Composes with doc 65 Rung 4 (`FieldSlot`) — no second authority for field liveness. |
| **Compile-time regression** from running the unified BFS + dyn-root analysis | all phases | The BFS already runs (it is `eliminate_dead_functions`, every native/WASM build); unifying does not add a traversal, it removes three. Dyn-root analysis is frontend-time over data already walked. Compile-time is a tracked dimension (§7); a regression is reported and budgeted, never hidden (doc 65 §1). |

---

## 9. The single most important sentence

This arc's deliverable is **not** a smaller binary — it is a codebase in which
*"reachability is re-derived per tier"* and *"a symbol is dead here but alive
there"* are **a generator failure, a build-closed assertion, or a red audit**, so
that the IR DFE, the IPO call-graph, the address-taken-intrinsics manifest, the
stdlib-cache key, and every backend's linker/emit set all answer "is this symbol
live?" from the **one** whole-program `Reachability` fact, and the artifact
contains exactly — provably, on every backend — the code reachable from the
program entry, with the Pythonista's dynamism preserved as recorded roots rather
than erased.

---

*Design only / executable plan. portfolio-architect, 2026-06-24.*
*Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>*
