<!-- Foundation design 44 — F2 frontend decomposition + separation-of-concerns end-state.
     Architect: read-only research+architecture agent, 2026-06-06. House style of docs 26–43
     (file:line at HEAD, IMPORTANCE×GAP scores, explicit refusals, provenance per borrowed idea).
     Audited HEAD: dc6965d8d8c41895afb6e486349710a1740e8bef.
     Doc number 44 is reserved by the supervisor — do NOT renumber.
     F1 = the landed move-only visitors/ + lowering/ mixin split (a0b9c8a9d, a460e7079).
     F2 = THIS program: phase separation (Parse→Bind/Sema→Lower→Serialize), single lowering
     authority per construct, state narrowing into explicit context objects, registry-derived
     tables, dissolution of the mixin-over-god-object shims. -->

# Frontend Architecture — the F2 Decomposition & Separation-of-Concerns End-State

**Status:** Design doc plus live F2 routing note. F1 (move-only mixin split) is landed and has expanded beyond the original four-mixin snapshot into the current local-binding, midend-optimization, serialization, analysis, visitor, async/generator, and statement-family mixin shell. F2 is the semantic decomposition that F1 deliberately deferred. F2b has begun: `frontend.sema.funcmeta` now owns `FunctionKind`, canonical function-kind normalization, yield/signature classification, and async-generator legality predicates consumed by lowering; `frontend.sema.classgraph` now owns static class-graph construction, local class-member facts with dynamic/decorated-member opacity, C3/static-MRO/reachability facts, class-body block-exec facts, and precomputed zero-arg `super()` fold soundness method sets consumed by lowering. This doc remains the program spec for the unfinished end-state: phase separation, single-authority facts, registry extension, explicit lowering contexts, and dissolution of the mixin-over-god-object shims.

**Date:** 2026-06-06. Every file:line anchor verified against HEAD `dc6965d8d`. Current-code note, 2026-06-26: F1 plus the first F2 authority cuts have reduced `src/molt/frontend/__init__.py` to a 302-line facade shell, not the final F2d facade. Function-shape spelling and suspension classifiers now have one semantic home in `frontend.sema.funcmeta`; static class graph, local class-member facts with fail-closed opacity, C3/static-MRO/reachability, class-body block-exec facts, and precomputed zero-arg `super()` fold method sets now live in `frontend.sema.classgraph`, and call/class lowering consume those facts through explicit sema inputs. The generator is still one shared-state lowering shell, so the F2 target below remains the required end-state: data contracts and phase separation, not permanent mixins over the god object.

**The verdict this engineers against** (supervisor, "engineered like Chris Lattner would?"): **NO, today.** At the original audit, `src/molt/frontend/__init__.py` was 27,071 lines, one class `SimpleTIRGenerator` with 538 `def`s (`__init__.py:211`), assembled from four MRO-mixins (`SerializationMixin, PatternMatchMixin, CallVisitorMixin, ClassDefVisitorMixin, ast.NodeVisitor` — `__init__.py:211-217`) that shared its ~150 mutable instance fields. Current F1 has moved more families out of the file, including `LocalBindingMixin` and `MidendOptimizationMixin`, and the first F2 sema cuts have removed the duplicate function-shape and static-class-graph authorities. The architectural defect remains: scope binding, IC index allocation, exception-edge insertion, const handling, augassign-kind selection, and emitted class metadata are still partly recomputed or supplied during the lowering walk behind one shared generator state. The cost is measured below in §6.

---

## 0. Scope, non-goals, and what F1 already proved

**In scope.** The decomposition of `SimpleTIRGenerator` into named phases with explicit data contracts; the rule that makes scope-divergent lowering structurally impossible; the extension of the *already-landed* op-kind registry (`tools/gen_op_kinds.py`, doc 25) to absorb hand-kept frontend tables/effect oracles; the phasing that lands move-only structure before any semantic change and dissolves the mixin shims by the end.

**Out of scope (covered elsewhere, cross-referenced only).** The per-construct *semantic* gaps (metaclass `__prepare__`, `__slots__` layout, `__index__` coercion, match-as-CFG) are doc 30's portfolio and its commissioned docs (#40–#42); F2 makes those fixes *land in one place* but does not re-specify them. The op_kinds.toml *schema* is doc 25; F2 extends its **output**, not its design. Generator/coroutine lowering is doc 26.

**What F1 proved (the precedent F2 extends).** F1 split the 27K-line class across files **move-only** — beginning with `a0b9c8a9d` (serialization + pattern_match) and `a460e7079` (visit_Call + visit_ClassDef families), then continuing through local-binding, midend-optimization, analysis, async/generator, comprehension, expression, function, assignment, control-flow, and scope families. Each family remains a body relocation with the `_GeneratorProtocol` (`_protocol.py:54`) restoring cross-file `self.<attr>` type-checking. F1's own headers are explicit that this bought *file boundaries, not semantic ones*: classes.py:1-9 — "Move-only extraction… every method here is, transitively, called only from within this family. self.<method>/<attr> references resolve through the SimpleTIRGenerator MRO at runtime." Doc 30:20 states the verdict precisely: the visitors are "F1-phase move-only extractions with **no independent semantic content**." F2 is the phase that gives them independent semantic content — or dissolves them.

**The existence proof that F2's target shape is reachable** lives in the same package today: `cfg_analysis.py` (416 lines) is already the end-state shape — free functions (`build_cfg`, `_collect_control_maps` at `cfg_analysis.py:44`) over frozen dataclasses (`BasicBlock`/`ControlMaps`/`CFGGraph` at `cfg_analysis.py:12/19/31`) taking an `OpLike` Protocol (`cfg_analysis.py:7`). Zero `self`, zero god-object state, fully testable in isolation. F2 makes the rest of the frontend look like `cfg_analysis.py`.

---

## 1. The end-state architecture (the Year-5 shape)

### 1.1 The phase ladder and its contracts

The end-state is a four-phase pipeline. Each phase is a separate module with an explicit data contract; state is narrowed to the context object each phase owns; the semantic phases **annotate, never emit**, and the lowering phase **consumes annotations, never re-derives them**.

```
ast.Module
   │
   ▼  PARSE            (already external: Python's own ast)
   │
   ▼  BIND / SEMA      frontend/sema/                       ── ANNOTATES, never emits
   │     • ScopeTable        : per-scope symbol kind (local/cell/free/global), the
   │                           closure-cell index map, comp-scope isolation set
   │     • ClassGraph        : static bases, C3 linearization, reachability
   │     • ClassFacts        : class-body block-exec ids, super-fold-sound
   │                           methods, descriptor/slot facts
   │     • ConstEnv          : statically-known module dicts, const-int facts
   │     • Legality          : compile-time warnings (~bool, finally-flow), the
   │                           refuse-to-fold-a-raising-const decision
   │   OUTPUT: a SemaResult — immutable annotation tables keyed by AST-node id.
   │
   ▼  LOWER             frontend/lower/  (the thin walk)    ── CONSUMES annotations, emits MoltOps
   │     • ONE authority per construct (one function per AST node kind), parameterized
   │       by a LowerCtx (scope cursor + op buffer + sema handle), NOT by self-state.
   │     • Emits MoltOp(kind=UPPERCASE) into an OpBuffer. No scope re-analysis here.
   │   OUTPUT: per-function MoltOp streams (the funcs_map).
   │
   ▼  SERIALIZE         frontend/lowering/serialization.py  ── already separable
   │     • map_ops_to_json: MoltOp(UPPERCASE) → JSON kind (lowercase) wire contract.
   │   OUTPUT: the JSON IR the backend consumes.
   ▼
backend (Rust)
```

This is a deliberate borrowing of the **Clang Sema/CodeGen separation** (`Sema` builds a fully type-checked, annotated AST; `CodeGen` is a thin walk that *consumes* `Sema`'s decisions and never re-decides) and **CPython's own `symtable`→`compile` split** (`symtable.c` computes the scope/symbol binding for every name *before* `compile.c` emits a single bytecode; the code generator reads `PySTEntryObject` flags, it does not recompute scope). Provenance and how molt diverges from each is in §1.4.

### 1.2 The four data contracts (the load-bearing decision)

The phase boundaries are only real if the contract between them is **data, not a shared mutable object**. The four contracts:

| Phase | Owns (mutable) | Reads (immutable) | Produces (immutable) |
|---|---|---|---|
| **Bind/Sema** | its own work-stacks | `ast` | `SemaResult` (tables below) |
| **Lower** | `LowerCtx` (scope cursor, `OpBuffer`, label/var counters) | `ast` + `SemaResult` | `funcs_map: dict[name, MoltOp stream]` |
| **Serialize** | a local fold/fusion cursor | `funcs_map` + `SemaResult` | JSON IR |

`SemaResult` is the keystone artifact — the analog of Clang's annotated AST / CPython's `symtable`. Concretely:

```
@dataclass(frozen=True)
class SemaResult:
    scopes:      dict[int, ScopeInfo]      # keyed by AST-node id (FunctionDef/Lambda/Module/comp)
    classes:     dict[str, ClassFacts]     # block-exec class ids, super-fold-sound methods, slots, fields
    const_env:   ConstEnv                  # module const dicts, const-int facts, refused-fold node ids
    legality:    LegalityReport            # deferred warnings, finally-flow violations
```

`ScopeInfo` carries what `__init__.py` today smears across `locals/boxed_locals/closure_locals/comp_shadow_locals/free_vars/free_var_hints/global_decls/nonlocal_decls/scope_assigned/del_targets/unbound_check_names/async_locals/...` (fields at `__init__.py:270-356`). In the end-state those are **per-scope immutable facts computed once by Sema**, not 18 mutable dicts on one object re-keyed every time the walk enters a function.

### 1.3 Does `SimpleTIRGenerator` survive?

**Decision: it dissolves.** It does not survive as the lowering shell, because "the shell" with 150 fields *is* the problem. The end-state has:

- `frontend/sema/` — free functions + small dataclasses (the `cfg_analysis.py` shape), producing `SemaResult`.
- `frontend/lower/` — a `Lowerer` that is a **thin `ast.NodeVisitor`** whose only instance state is a `LowerCtx`. Its `visit_X` methods are the single authority per construct (§2). It holds *no* scope dicts — it reads `self.ctx.sema.scopes[node_id]`.
- `frontend/lowering/serialization.py` — `map_ops_to_json` becomes a free function `map_ops_to_json(funcs_map, sema) -> json`, not a mixin method (it already takes no construct-level `self`-decisions it could not take from its arguments; §5 row 6).
- `frontend/__init__.py` — a **thin façade** (~150 lines): `compile_to_tir(...)` wires Parse→Sema→Lower→Serialize and re-exports the public names. This is the exact shape of the backend precedent `34e3bddbf` (lib.rs 6,928 → 264 lines, "a thin facade of mod decls + re-exports… every public path and symbol preserved byte-identically").

The F1 mixins, including the current local-binding, midend-optimization, serialization, analysis, visitor, and statement-family mixins, **must be deleted** by the end of F2 — not re-homed as mixins, *deleted as mixins*. Pattern-match and call/class lowering become `Lowerer` method-families that take `LowerCtx` (the M1 precedent: `fe1454a03` lifted 10 op-families out of a 34K-line function into free `fn` handlers "taking the shared lowering state as explicit split-borrowed &mut params" — the Rust analog of exactly this). The phase that deletes the mixin base classes is **F2d** (§4); naming it explicitly is the anti-half-measure commitment per CLAUDE.md.

### 1.4 Provenance (per borrowed idea; GPL = ideas only)

- **Clang `Sema`/`CodeGen` separation** ("Clang Internals", clang.llvm.org/docs/InternalsManual.html): the principle that semantic analysis produces a fully-annotated AST and codegen is a thin consumer. **Borrowed:** the annotate-then-consume contract; Sema never emits, Lower never re-analyzes. **Diverge:** molt's Sema is lighter — it does *binding + legality + static class facts*, not full type inference (molt's types flow as optional hints + the Rust midend's `type_refine`).
- **CPython `symtable.c` → `compile.c`** (CPython source; PSF license — studied, reimplemented, not copied): scope/symbol binding is computed for every name into `PySTEntryObject` **before** any bytecode is emitted; the compiler reads `ste_symbols` flags. **Borrowed:** `ScopeInfo` keyed per scope, computed once, read by Lower. This is the *direct* fix for molt's "scope analysis recomputed inline during the walk." **Diverge:** molt keys by AST-node id and produces an immutable dataclass rather than a mutable `symtable` object graph.
- **Swift `Parse → Sema → SILGen → SIL`** (swift.org/swift-compiler/; Apache-2.0): SIL is the *semantic IR* on which the diagnostic/optimization passes run; SILGen is a thin lowering from the type-checked AST. **Borrowed:** the idea that the IR (here: the MoltOp stream + JSON) is produced by a *thin* lowering from an *already-decided* representation, with the heavy analysis upstream. **Diverge:** molt has no separate SIL — the MoltOp stream is lowered straight to the Rust TIR; F2's `SemaResult` is the "decided representation," not a second IR.
- **Rustc `HIR → THIR → MIR` + query system** (rustc-dev-guide.rust-lang.org; MIT/Apache — ideas only): the query system computes a fact (e.g. `typeck`) **on demand, memoized, keyed by `DefId`**, and consumers *ask* for it rather than recomputing. **Borrowed:** `SemaResult` tables keyed by node id are the memoized-fact analog; `super_fold_is_sound` is now the sema-owned builder predicate for `ClassFacts.super_fold_sound_methods_by_class`, and Lower reads the precomputed fact instead of re-running the predicate. **Diverge:** molt computes Sema eagerly per module (no lazy query engine — the module is small enough that eager is simpler and the DX is better; we reject importing a query framework, §5).
- **MLIR ODS / TableGen** (mlir.llvm.org/docs/DefiningDialects/Operations/; Apache-2.0): one declarative op definition generates the verifier, builder, and printer. **Borrowed:** the §3 registry move — one `op_kinds.toml` row generates the mapper arm, the effect oracle, *and* the frontend's canonical-spelling + raising-kind constants. This is **already half-built** in molt (`tools/gen_op_kinds.py`); F2 extends it.

---

## 2. The single-expression-lowering-authority rule

**The rule:** every Python construct has **exactly one** lowering function, parameterized by a `LowerCtx` that carries the scope. Scope-dependent behavior is a *parameter*, never a *fork in the code*. This is what makes scope-divergence (the task-#42 bug *class*) structurally inexpressible: there is no second site to drift.

### 2.1 The measured evidence: construct lowering forks on scope today

The single largest structural smell in `__init__.py` is the **88 occurrences of `self.current_func_name == "molt_main"`** — i.e. 88 places where the lowering of a construct *branches on whether it is at module scope or in a function*. (`grep -c 'current_func_name == "molt_main"' __init__.py` → 88.) Representative load-bearing sites: `__init__.py:3092, 3279, 3495, 3522, 3559, 3597, 3718, 3798, 4631, 4693, 4718, 4968, 5260, 5772`. Each is a hand-maintained "is this module scope?" fork inside a visit method. Eighteen-plus of these gate *variable storage* (`module_obj`-backed global vs frame-local), which is exactly the axis that produced the comp-walrus / env-misbind P0 history (doc 30:238, commits `99723d589`/`d19dfa588`/`c1faf79f7` — three separate frontend commits in the last week all unifying *storage* for the same construct across scopes). Each of those commits is a single-site patch on the *symptom* of "construct X lowers differently in scope A vs B." F2's rule makes the *cause* — two sites — impossible.

A second axis: the **async vs sync fork**. `visit_AugAssign` (`__init__.py:13961`) forks at `:13967` on `self.is_async() and node.target.id in self.async_locals`, producing a distinct value-load path (`_load_local_value` vs `visit(load_node)`). The same async/sync fork recurs across with/for/bool-op lowering. In the end-state, "async" is a property of the `ScopeInfo` the one lowering function reads — the storage strategy is selected by `LowerCtx.store(name, val)` dispatching on `ctx.scope`, not by an `if self.is_async()` inside every visit method.

### 2.2 The constructs that lower in >1 place (the authority-merge worklist)

| Construct | Forked today on | Anchors | End-state authority |
|---|---|---|---|
| **Variable store/load** | module vs function (`molt_main`); async vs sync | 88× `molt_main`; `__init__.py:13967` | `LowerCtx.store/load(name)` → dispatch on `ScopeInfo.kind[name]` |
| **Const handling** | the raising-fold refusal is a Sema decision today partly entangled with emission; `_RAISING_OP_KINDS` (frontend) duplicates the backend `may_throw` oracle | `__init__.py:1169` (raising set); `:1239-1346` (CHECK_EXCEPTION inverse set); op_kinds.toml `may_throw` (38 rows) | Sema's `ConstEnv` records "refuse to fold node N"; Lower emits the op unconditionally; *raising-ness* is read from the **generated** registry, not a hand list (§3) |
| **Class-body vs function-body statement** | `_class_body_depth` counter mutated mid-walk (`__init__.py:269`); nested-class binding fixed *twice* recently | `c1faf79f7` (class-nested classes); `classes.py:849` (class-body `visit_Assign`) vs `__init__.py:2670`/`2354` (other `visit_Assign`) | one `lower_assign` reading `ScopeInfo.kind` ∈ {class_body, function, module} |
| **Comprehension scope** | `comp_shadow_locals` set toggled around the comp (`__init__.py:276`); walrus-target storage unified *twice* last week | `99723d589`, `d19dfa588`; `__init__.py:276` | `ScopeInfo` for the comp node carries the isolation set + walrus-leak targets; one `lower_comprehension` |
| **f-string pieces** | conversion/format-spec assembled inline in `visit_JoinedStr`; the `{expr=}`-under-inlining multisite miscompile baton | doc 30:258 (`project_inliner_fstring_multisite_miscompile.md`) | one `lower_joinedstr` over a Sema-resolved piece list |
| **super() dispatch** | static fold vs runtime path reads `ClassFacts.super_fold_sound_methods_by_class`; the sema builder computes it from C3/static-MRO/reachability, local class-member facts, and explicit imported-class metadata; dynamic/decorated class-member surfaces fail closed | `sema/classgraph.py` (`ClassFacts`, `super_fold_is_sound`, `class_facts_with_super_fold_sound_methods`), `lowering/sema_state.py` (fact enrichment), `calls.py` (fact consumer) | Remaining work: broaden immutable `ClassFacts`/`ScopeInfo` consumption so other call/class/scope decisions stop reading shimmed god-object fields |

**Note on task #42 accuracy (important).** The `raising_const_expr_fold_matrix.py` regression (the task-#42 corpus) documents that the *two sites that actually dropped the raising op were in the Rust midend* — the `op_kinds.toml` `may_throw` mis-classification of `Shl`/`Shr`/`Pow` and SCCP's `eval_binary_pow` (test docstring, `raising_const_expr_fold_matrix.py:9-18`). That specific bug is **already fixed** at HEAD (the registry now carries 38 `may_throw=true` rows including the shifts; `d6c792454`/`f16740ca3` landed the registry). What the test *also* encodes — and why it crosses "module / function / method / comprehension / lambda" scopes (`:88-118`) — is that **the frontend has five distinct lowering paths per scope** whose divergence is the standing fragility. F2's §2 rule is the structural defense for that fragility; the frontend's `_RAISING_OP_KINDS` (`__init__.py:1169`) duplicating the backend oracle is the residual drift vector (§3). This doc corrects the brief's framing: task #42's *drop* was backend; task #42's *scope-matrix* is the frontend smell F2 targets.

### 2.3 Why `LowerCtx`-parameterization, not a flag

A construct lowered by `if scope == module: ... else: ...` inside one function is *not* single-authority — it is two authorities sharing a `def`. The rule requires that the *strategy* (how to store a name, whether a name is a cell) live behind a `LowerCtx`/`ScopeInfo` method whose *implementations* are the scope variants, so a visit method reads `ctx.store(name, v)` with no scope `if` at all. This is the difference between "one place that branches" and "one place" — only the latter makes the second behavior un-addable without touching the strategy object's contract (where the divergence is then *visible and tested*).

---

## 3. The registry extension (the ODS move) — and the HEAD surprise

**The brief assumed the registry generator does not yet emit a Python file. It does, at HEAD.** `tools/gen_op_kinds.py:50` already renders `src/molt/frontend/lowering/op_kinds_generated.py`, and `op_kinds.toml` already carries the `may_throw` column (`op_kinds.toml:140`, 38 `may_throw=true` rows). Today that generated Python file exports `MAPPER_CANONICAL_KINDS` + `canonical_kind()` (`op_kinds_generated.py:165/272`) — the wire-spelling vocabulary. The F2 move is therefore **not "build a generator"** — it is **"extend the existing generator's Python render to absorb hand-kept frontend tables/effect oracles, then delete them."** This is a sharper, lower-risk move than the brief anticipated.

### 3.1 The hand-kept frontend tables that must become generated

1. **`_RAISING_OP_KINDS`** (`__init__.py:1169-1229`, 60 entries, UPPERCASE MoltOp kinds). This is a hand-maintained copy of the `may_throw` knowledge the backend's `opcode_may_throw` already derives from `op_kinds.toml` (38 `may_throw=true` rows). **Two copies of one fact** — doc 25's exact bug class (#1, the `matches!`-default-false / ModuleImportFrom lesson). It is consumed at `emit()` (`__init__.py:1235`) solely to attach `_expr_col` to raising ops for traceback carets. **Generate `RAISING_KIND_NAMES`** from the `may_throw` column (mapped MoltOp-kind ↔ JSON-kind via the table's existing alias data) and import it.

2. **The `emit()` CHECK_EXCEPTION exclusion set** (`__init__.py:1258-1338`, ~80 entries). This is the *inverse* table: the set of op kinds after which `emit()` does **not** auto-insert a `CHECK_EXCEPTION`. It is logically `¬(may_throw)` for structural/const/pure kinds — a **third** copy of the throw-classification, drifting independently from both `_RAISING_OP_KINDS` and the backend oracle. **Generate it as the complement** of the raising set over the known-kind universe (with the structural/CFG kinds the registry already enumerates — doc 25 §2 "structural kinds").

3. **`_augassign_op_kind`** (`__init__.py:13924-13959`, a 13-arm `isinstance(op, ast.X)` → `"INPLACE_X"` chain). This is the AST-operator → kind map that doc 25 §1 flagged as drift-prone (the historical `floordiv`/`floor_div` schism, bug #5; and the augassign-inplace-dunder gap that was a live correctness bug fixed days ago in `1c15a8353`). The map `{ast.Add: "INPLACE_ADD", ...}` is pure registry data. **Add an `augassign_kind` column** (or a `binop_ast → kind` mapping section) to `op_kinds.toml` and generate the dict; `visit_AugAssign` imports it. The sibling `visit_BinOp` op-selection chain (`__init__.py:10723-10736`, `ast.LShift → "LSHIFT"` etc.) is the same shape and folds into the same generated map.

4. **The midend optimizer effect oracle** (`frontend/lowering/midend_optimization.py::_op_effect_class`). This is the pre-serialization sibling of backend `OpEffects`: it decides CSE/LICM/DCE barriers over UPPERCASE `MoltOp.kind` names before JSON serialization. It must be generated from `[[kind]]`, opcode `may_throw`/`side_effecting`/`purity`, `[[simpleir_control_kind]]`, `[[frontend_raising_kind]]`, and explicit `[[frontend_effect_kind]]` overrides so a frontend optimizer barrier cannot drift from the TIR registry.

### 3.2 Mechanism (extends doc 25 §5, does not replace it)

- **One table:** `op_kinds.toml` gains two columns on the relevant rows — `ast_binop` (the `ast.operator` subclass name this kind is the binary form of) and `ast_augassign` (the inplace form). The `may_throw` column already exists.
- **One generator:** `tools/gen_op_kinds.py` (already renders `op_kinds_generated.py`) additionally emits, into that same file: `RAISING_KIND_NAMES: frozenset[str]`, `CHECK_EXCEPTION_SKIP_KINDS: frozenset[str]`, `FRONTEND_EFFECT_CLASS: dict[str, str]` plus effect-class sets, `AUGASSIGN_OP_KIND: dict[str, str]` (keyed by `ast.operator.__name__`), and `BINOP_OP_KIND: dict[str, str]`.
- **One sync test:** `tests/test_gen_op_kinds.py` (already exists per doc 25 §5/§6) re-renders in memory and asserts byte-identity → drift becomes a test failure.
- **Three deletions:** `__init__.py:1169-1229`, `:1258-1338`, `:13924-13959` are replaced by imports from `op_kinds_generated`. The `visit_BinOp` chain (`:10723-10736`) keeps its *type-hint* logic but reads the kind string from `BINOP_OP_KIND`.

This is the **MLIR ODS principle** applied to the last hand-kept frontend tables: the op definition is the single source; the verifier (backend `opcode_may_throw`), the builder (frontend kind selection), and now the frontend's optimizer *effect* and *construction* vocabularies are all generated from it. The wire vocabulary (`MAPPER_CANONICAL_KINDS`) is already generated; F2 closes the loop on the *effect* and *construction* vocabularies.

### 3.3 The visitor-dispatch surface (refused as a generation target)

The brief asks whether "the visitor dispatch surface" should be generated. **Refused.** `ast.NodeVisitor`'s `visit_X` dispatch is already a clean, language-defined surface (one method per `ast` node type); generating it would add a layer without removing drift (the `ast` grammar is CPython's, already a single source). The decomposition value is in *who owns each `visit_X*` and *what state it reads* (§1–§2), not in code-generating the dispatch. Generating the *kind tables* kills a real bug class; generating the dispatch would be cargo-culting the ODS pattern past the point it pays.

---

## 4. Phasing (F1/M1 discipline: complete pieces, move-only before semantic, gates per phase)

The unit of work is the complete structural change (CLAUDE.md). F2 is a multi-week arc; the phases below are each a **complete structural piece** (not a partial fix toward the next), so intermediate commits are honest. Every phase carries a differential gate (`tests/differential/basic/` byte-identical on native + LLVM) and the registry sync test where it touches tables.

### F2a — Registry absorption of hand-kept frontend tables (THIS WEEK; collision-free)

**Scope.** §3: add the `ast_binop`/`ast_augassign` columns and frontend effect rows to `op_kinds.toml`; extend `gen_op_kinds.py` to emit `RAISING_KIND_NAMES`/`CHECK_EXCEPTION_SKIP_KINDS`/`FRONTEND_EFFECT_CLASS`/`AUGASSIGN_OP_KIND`/`BINOP_OP_KIND` into `op_kinds_generated.py`; delete the hand lists (`__init__.py:1169`, `:1258`, `:13924`, and `midend_optimization.py::_op_effect_class`'s local sets) and the `visit_BinOp` kind-literals (`:10723`), replacing with imports.
**Why it lands this week without colliding with in-flight arcs.** The in-flight frontend work is **`prepfix`** (metaclass `__prepare__`, live-uncommitted in `visitors/calls.py` per `git status`, and `visitors/classes.py`) and **`cfoldfix`** (const-fold paths). F2a touches *neither*: it edits `op_kinds.toml`, `gen_op_kinds.py`, `op_kinds_generated.py`, and three *disjoint* line ranges of `__init__.py` (the table definitions at 1169/1258/13924/10723 — none of which `prepfix` or `cfoldfix` touch, since those live in `classes.py`/`calls.py` and the SCCP/fold paths respectively). It is the M1 move "lift a hand table into the generator" — the exact, low-risk shape of `34e3bddbf`/`fe1454a03`.
**LoC/risk.** Generated-table expansion plus local-table deletion. **Risk: LOW** (byte-identical generated output is mechanically verifiable; the sync test is the gate). **Deletes:** the hand frontend registry/effect tables — concrete dead-code removal of F2.
**Gate.** `tests/test_gen_op_kinds.py` byte-identity + full differential corpus byte-identical (the generated sets must be supersets-equal to the hand sets; a diff *is* a latent drift the move surfaces).
**Audit-tool implication.** `tools/audit_op_kinds.py` parses `serialization.py` for the wire vocabulary; F2a does not touch `serialization.py`, so the audit is unaffected. (It *gains* coverage: the raising/augassign tables move under the same generator the audit already cross-checks.)

### F2b — Extract Sema (the binding/legality/class-graph phase), additively

**Scope.** Create `frontend/sema/` with `scope.py` (the `ScopeInfo`/`ScopeTable` builder — lifts the closure-cell/free-var/nonlocal/global/comp-isolation analysis currently smeared across the `*Collector` nested classes and the 18 scope dicts), `classgraph.py` (lifts static class-graph construction, local class-member facts, C3/static-MRO/reachability facts, the zero-arg `super()` fold soundness predicate, and the precomputed fold-sound method sets Lower consumes), `constenv.py` (lifts `_collect_module_const_dicts` `__init__.py:2705`, the const-int facts, the refuse-to-fold decision), `legality.py` (lifts `_prescan_compile_warnings` `__init__.py:1450`). Each is **free functions over dataclasses** (the `cfg_analysis.py` shape). `SimpleTIRGenerator.__init__` *calls* Sema and stores the immutable `SemaResult`; the visit methods initially still read their old dicts, now *populated from* `SemaResult` (a shim layer).
**Why additive-first.** This is the move-only-before-semantic discipline: F2b *relocates* the analysis and introduces the `SemaResult` contract **without yet rewiring the walk** to read it directly. The walk's behavior is byte-identical because the old dicts are filled from the new tables. This de-risks the boundary before the semantic rewire (F2c).
**LoC/risk.** ~2,500 LoC relocated into `sema/` (the `*Collector` classes — there are ~25 of them, `__init__.py:2185-7227` — plus the MRO/const/legality helpers). **Risk: MEDIUM** (the relocation must preserve the exact population order; the `*Collector` classes mutate `self`-state today, so the relocation must thread a builder that returns facts instead — this is where the move stops being purely mechanical).
**Gate.** Full differential corpus byte-identical; the `SemaResult` tables asserted equal (in a new `tests/test_frontend_sema.py`) to the values the old inline analysis produced on the corpus.
**Deletes:** nothing yet (the shim keeps the old dicts). Dead-code removal is F2c/F2d.

### F2c — Rewire Lower to read `SemaResult`; merge the scope-forked authorities

**Scope.** §2: introduce `LowerCtx` carrying `(scope_cursor, op_buffer, sema, label_ctr, var_ctr)`. Rewrite the visit methods to read `ctx.sema.scopes[node_id]` instead of `self.locals/boxed_locals/...`; replace the 88 `molt_main` forks and the async/sync forks with `LowerCtx.store/load` dispatch (§2.3). Merge each >1-site construct (§2.2 worklist) into one authority. **This is the riskiest phase** (§4.1).
**LoC/risk.** Touches most of the 538 methods (the read-sites change even where the logic does not). **Risk: HIGH** — flagged §4.1.
**Gate.** Full differential corpus byte-identical, **per construct family** (land the variable-store merge, gate; then comprehension, gate; then class-body, gate; then f-string; then super). Each family is a complete piece.
**Deletes:** the 18 scope dicts (`__init__.py:270-356`) and the shim from F2b — *as each family migrates off them*. The last family to migrate deletes the field.

### F2d — Dissolve the mixins into the `Lowerer`; collapse `__init__.py` to a façade

**Scope.** Convert `SerializationMixin`/`PatternMatchMixin`/`CallVisitorMixin`/`ClassDefVisitorMixin` from mixins-over-god-object into `Lowerer` method-families taking `LowerCtx` (or, for serialization, a free function — §5 row 6). **Delete the four mixin base classes and the `_GeneratorProtocol`** (no longer needed once `self` is the small `Lowerer`/the explicit `LowerCtx`). Collapse `__init__.py` to the ~150-line façade (`compile_to_tir` wiring Parse→Sema→Lower→Serialize + re-exports), mirroring `34e3bddbf`.
**LoC/risk.** `__init__.py` 27,071 → ~150 (façade) + bodies relocated to `lower/`. **Risk: MEDIUM** (the F1 mixins are already separate files; F2d changes their *base* and their `self`-contract, which F2c already narrowed). **Deletes:** the four mixin classes, `_protocol.py` (817 lines), the god-object. **This is the phase the verdict demands: the mixin shims die here.**

### Phase ordering rationale

F2a is independent and lands now. F2b→F2c→F2d is the strict order: you cannot rewire Lower to read Sema (F2c) before Sema exists (F2b); you cannot dissolve the mixins (F2d) before the `self`-contract is narrowed (F2c). Splitting F2c's construct-family merges across commits is honest *only because each family is a complete authority-merge* (not a partial fix toward the next family) — each leaves the tree byte-identical and the codebase with one fewer scope-fork.

---

## 4.1 The riskiest phase: F2c (the Lower rewire)

F2c is highest-risk because it is the only phase that **changes behavior-adjacent code at ~500 sites** while the contract is that behavior does *not* change. The specific hazards:

- **Population-order coupling.** The inline analysis today runs *interleaved* with emission (e.g. `exact_locals.pop(...)` inside `visit_AugAssign` at `__init__.py:13965`; `const_ints[...]` written inside `emit()` at `:1245`). Some "facts" are *mutated by the walk itself*. F2c must prove each such fact is either (a) a genuine Sema fact (compute once) or (b) a *walk-local* cursor that stays in `LowerCtx`. Mis-classifying (b) as (a) is a miscompile. This is the line where the brief's "asymmetric coverage" trap lurks: migrating the int-lane store but not the async-lane store re-creates the env-misbind bug.
- **The `molt_main` forks are not all the same axis.** Some of the 88 are "module global storage" (genuinely scope-dependent, → `LowerCtx.store`); others are "is this the entry module's top-level, so the super-fold is sound" (a *ClassFacts* question) or "emit module metadata here" (a phase-ordering question). F2c must *triage* the 88, not mechanically rewrite them. A wrong triage is silent.
- **Differential coverage is necessary but not sufficient.** The corpus is ~480 files (doc 30:28); byte-identical output proves the *covered* paths. F2c must add targeted regressions for the *uncovered* scope crosses (the task-#42 matrix `raising_const_expr_fold_matrix.py` is the template — cross every migrated construct with module/function/method/comp/lambda). The gate is "corpus byte-identical **and** a per-family scope-cross regression added."

**Mitigation (the F2b additive shim is the de-risker).** Because F2b populates the old dicts from `SemaResult` *first*, F2c can migrate one read-site at a time with an in-place assertion that `ctx.sema.scopes[node].kind[name] == (the old dict's answer)` — verifying the invariant *while* completing the migration (the right use of a debug-gated assertion per CLAUDE.md: a verification tool *during* the migration, not a substitute for it).

---

## 5. Scorecard — current state per Lattner principle (IMPORTANCE × GAP, file:line evidence)

Scale: IMPORTANCE 1–3 (how load-bearing for a world-class AOT frontend), GAP 0–3 (distance from the principle today). House style of doc 30.

### 5.1 Phase separation — IMPORTANCE 3, GAP 3

The phase boundary is now partially real but incomplete. Parse is external (ast), and `frontend.sema` now owns module class-graph construction, local class-member facts, C3/static-MRO/reachability facts, const-env collection, function-shape facts, class-body block-exec facts, the zero-arg `super()` fold soundness predicate, and the precomputed `ClassFacts.super_fold_sound_methods_by_class` table consumed by call lowering. The remaining gap is that Lower still reads shimmed god-object dicts in many other places. The only fully clean separation in the package remains `cfg_analysis.py` — and it operates on the *already-emitted* op stream, i.e. it is a *post-Lower* analysis, not a *pre-Lower* Sema. **Gap remains high:** the architecture has a Sema phase, but most of Lower is not yet a thin consumer of immutable ClassFacts/ScopeInfo.

### 5.2 Single authority per construct — IMPORTANCE 3, GAP 3

The defining smell. **88 `molt_main` scope-forks** (§2.1) + the async/sync forks mean dozens of constructs lower in ≥2 places. The recurring-P0 history is the *measured* cost: comp-walrus storage unified **twice in one week** (`99723d589`, `d19dfa588`), nested-class binding (`c1faf79f7`), the f-string multisite baton (doc 30:258) — each a single-site patch on "construct X diverges across scope." Doc 30 names this directly: "recurring P0 classes ARE upstream quality gaps." **Gap is maximal.**

### 5.3 State narrowing — IMPORTANCE 3, GAP 3

`SimpleTIRGenerator` carries **~150 mutable instance fields** (`__init__.py:240-608`), and the `_GeneratorProtocol` enumerates **186 attributes + ~631 methods (817 declarations)** that the four mixins access on `self` (`_protocol.py`, `grep -c` = 817). Every "extracted" mixin can read and write all 150 fields — F1 bought files, not encapsulation. Process-global mutable state compounds it: the IC index allocator is a **module-global list** `_ic_counter` (`_types.py:43`), so IC slot assignment is shared across *all* compilations in a process, not owned by a context. **Gap is maximal:** the god-object is the abstraction.

### 5.4 Declarative tables — IMPORTANCE 2, GAP 1

**Best-scoring axis** — because doc 25's registry landed. `op_kinds.toml` + `gen_op_kinds.py` already generate the wire vocabulary (`op_kinds_generated.py:165`) and the backend effect oracle from one source, with a byte-identity sync test. GAP is 1, not 0, only because **three frontend tables remain hand-kept** (`_RAISING_OP_KINDS` `__init__.py:1169`, the CHECK_EXCEPTION skip set `:1258`, `_augassign_op_kind` `:13924`) — each a copy of knowledge the registry already owns (§3). F2a closes this to GAP 0. IMPORTANCE 2 (tables are real-but-bounded leverage vs the phase/authority axes).

### 5.5 Testability in isolation — IMPORTANCE 3, GAP 3

Semantic isolation is now real for the first F2 facts, but still thin. The class graph, local class-member facts, class-body block-exec facts, C3/static-MRO/reachability facts, zero-arg `super()` fold predicate, and precomputed super-fold method table are unit-testable through `frontend.sema.classgraph`, and function-shape facts are unit-testable through `frontend.sema.funcmeta`; scope binding still requires full lowering or generator construction. The old outlier remains `cfg_analysis.py` (free functions over an `OpLike` Protocol — `cfg_analysis.py:7/44`) plus `gen_op_kinds.py`'s generated output. Sema-as-free-functions (F2b) is the path that makes scope binding unit-testable on a bare AST next. **Gap remains high:** the architecture has isolated sema islands, but not a complete pre-lower semantic contract.

### 5.6 IR contract explicitness — IMPORTANCE 3, GAP 2

The *cross-process* contract (the JSON wire kind) is explicit and now registry-governed (doc 25) — good. The *intra-frontend* contracts are implicit: the boundary between "analysis" and "emission" is the shared `self`, so there is no typed artifact saying "Lower depends on exactly these facts." `_protocol.py` is an *accidental* contract — it enumerates the 817-symbol coupling surface, which documents the *absence* of a narrow contract rather than providing one. `serialization.py` already takes a near-pure contract (it reads the MoltOp stream + a handful of `self` fields and produces JSON — `map_ops_to_json` at `serialization.py:396`), which is why §1.3 can make it a free function with low risk. GAP 2 (the wire contract is explicit; the phase contracts are not).

### 5.7 Explicit refusals

- **Refused: a full visitor-pattern rewrite in one pass.** Rewriting all 538 methods at once is the anti-pattern CLAUDE.md forbids (the un-reviewable mega-diff; the "land half this session" trap). F2 is phased move-only-first (F2a/F2b) precisely so the semantic rewire (F2c) lands one *complete construct family* at a time, each gated.
- **Refused: importing a query engine (rustc-style `DefId` query memoization).** molt modules are small; eager per-module Sema is simpler, has better DX (Go-like legibility, per `feedback_golike_dx_lattner_perf`), and avoids a framework. We borrow the *fact-keyed-by-id* idea, not the lazy-query machinery.
- **Refused: introducing a second IR (a molt-SIL between AST and MoltOp).** Swift's SIL pays off because it hosts many passes; molt's optimization passes run on the Rust TIR downstream. A second frontend IR would be a parallel source of truth (the compound-interest trap). `SemaResult` is *annotations on the AST*, not a new IR.
- **Refused: generating the `ast.NodeVisitor` dispatch surface (§3.3).** The dispatch is already single-source (CPython's grammar); generating it removes no drift and adds a layer.
- **Refused: re-homing the F1 mixins as mixins.** The verdict is that mixins-over-god-object is the defect. F2d *deletes* the mixin base classes; it does not relocate them.

---

## 6. Contention economics (quantified from this week's evidence)

**The monolith serializes agent work — measured.** In the last ~3 days the frontend took **12+ commits** touching `src/molt/frontend/` (`git log -- src/molt/frontend/`): `1c15a8353` (augassign), `99723d589`/`d19dfa588` (comp-walrus storage, twice), `3f5aa1135` (cross-chunk class-SSA), `c1faf79f7` (nested-class binding), `c683690ce`/`a7021a45f`/`c5d7e02f3` (F1 hash-order leaks), `a460e7079`/`a0b9c8a9d` (F1 extractions), `cedb4a9f8` (unbound-name parity). The overwhelming majority land in **`__init__.py`** — the single 27,071-line file. With "max 2 build-triggering agents" (CLAUDE.md) and three build agents + a parallel session live on this tree *right now* (`git status` shows `calls.py` uncommitted), every one of those commits is a potential merge/edit conflict on one file. The cost is structural: a 27K-line file with 150 shared fields means *any two semantic changes touch overlapping state*, so they cannot be developed in parallel without coordination — the file *is* the lock.

**The M1 precedent quantifies the analog.** `fe1454a03` records the identical pathology on the Rust side: "compile_func_inner (34,242 lines, ONE function) is the #1 incremental-build long pole because rustc's codegen-units partition at *function* boundaries — a 34K-line function is one indivisible codegen unit no matter how the file is arranged." The Python analog is exact: a 27K-line *class* with 150 shared fields is one indivisible *review/merge* unit no matter how F1 arranged the files across mixins (the mixins still share the one `self`). F1 split the *files*; it did not split the *state*, so it did not split the lock.

**How each F2 phase unlocks a parallel lane:**

- **F2a** (registry absorption) removes three tables from `__init__.py` and moves their evolution to `op_kinds.toml` — a *declarative* file two agents can edit on disjoint rows without semantic conflict. It also *prevents* a class of cross-file drift bug (the three throw-tables), so it removes coordination *and* a bug source. Lands this week, collision-free with `prepfix`/`cfoldfix`.
- **F2b** (extract Sema) moves ~2,500 lines (the ~25 `*Collector` classes + MRO/const/legality) out of `__init__.py` into `sema/` files that have **no `self`-coupling to the lowering state**. After F2b, scope-binding work and lowering work are in different files with different contracts — two agents, two lanes.
- **F2c** (narrow Lower's state) replaces the 150 shared fields with a per-walk `LowerCtx`. After F2c, two construct families (say comprehensions and classes) no longer share mutable scope dicts, so they can be edited in parallel — the 150-field lock is broken into per-family contracts. *This is the phase that actually breaks the merge lock*, which is why it is also the riskiest.
- **F2d** (façade) leaves `__init__.py` at ~150 lines — no longer a contention surface at all; new construct work lands in `lower/<family>.py` and `sema/<analysis>.py`, the way new backend op-families now land in `fc/<family>.rs` (the `fe1454a03` end-state).

**Net:** the monolith currently forces N agents through one file and one state object (serial). F2a→F2d converts that into a declarative table + a Sema package + per-family Lower modules + a façade — the same N agents on disjoint files with explicit contracts (parallel). The contention reduction is the *DX* payoff that sits alongside the correctness payoff (the killed scope-divergence bug class) — both are required, per the verdict.

---

## 7. Cross-references and relevant paths

| Arc | Relationship to F2 |
|---|---|
| **doc 25** (op-kind registry) | F2a *extends* its already-landed generator (`gen_op_kinds.py` → `op_kinds_generated.py`) to absorb the three frontend throw/augassign tables. Do not duplicate its schema. |
| **doc 30** (core-language portfolio) | Source of the per-construct *semantic* gaps (`__prepare__` #42, `__slots__`, `__index__`, match-as-CFG #40). F2 gives those fixes a *single place to land* (one authority per construct); it does not re-specify them. |
| **doc 26** (real async/generators) | Generator/coroutine lowering is its territory; F2's `LowerCtx` is the substrate a clean generator-fusion lowering would plug into. |
| **F1** (`a0b9c8a9d`, `a460e7079`) | The move-only file split F2 builds on; F2d deletes the mixin classes F1 created. |
| **M1** (`fe1454a03`) | The Rust precedent for "god-function → free-fn handlers taking explicit context params" — the exact model for F2c/F2d. |
| **`34e3bddbf`** | The façade precedent (6,928 → 264 lines) — the F2d shape for `__init__.py`. |
| **In-flight `prepfix`/`cfoldfix`** | F2a is sequenced to touch neither (`op_kinds.toml`/`gen_op_kinds.py` + disjoint `__init__.py` line ranges). F2b+ must re-sequence once those settle. |

**Frontend paths (all `/Users/adpena/Projects/molt/`):**
- `src/molt/frontend/__init__.py` — `SimpleTIRGenerator` (`:211`), the 4-mixin header (`:211-217`), ~150 fields (`:240-608`), `_RAISING_OP_KINDS` (`:1169`), `emit()` + CHECK_EXCEPTION skip set (`:1231-1346`), `_augassign_op_kind` (`:13924`), `visit_BinOp` kind chain (`:10723`), the ~25 `*Collector` nested classes (`:2185-7227`), `_prescan_compile_warnings` (`:1450`), `_collect_module_const_dicts` (`:2705`), the 88 `molt_main` forks.
- `src/molt/frontend/_types.py` — `MoltValue`/`MoltOp` (`:67/73`), `_next_ic_index` + module-global `_ic_counter` (`:43/47`).
- `src/molt/frontend/_protocol.py` — `_GeneratorProtocol` (`:54`), the 817-declaration coupling surface (deleted in F2d).
- `src/molt/frontend/cfg_analysis.py` — the end-state shape that already exists (`:7/12/44`).
- `src/molt/frontend/sema/classgraph.py` — static class graph, local class-member facts, class-body block-exec facts, C3/static-MRO/reachability facts, the zero-arg `super()` fold soundness predicate, and the precomputed fold-sound method-set builder.
- `src/molt/frontend/visitors/calls.py` — call lowering consumes `ClassFacts.super_fold_sound_methods_by_class`; still owns the large `visit_Call` dispatch body.
- `src/molt/frontend/visitors/classes.py` — class lowering, `__prepare__` gap (no `__prepare__` emission; `:574-755`).
- `src/molt/frontend/visitors/pattern_match.py` — match lowering.
- `src/molt/frontend/lowering/serialization.py` — `map_ops_to_json` (`:396`); becomes a free function (F2d, §1.3).
- `src/molt/frontend/lowering/op_kinds_generated.py` — generated; gains `RAISING_KIND_NAMES`/`AUGASSIGN_OP_KIND`/… in F2a (`:165` today).
- `tools/gen_op_kinds.py` (`:50` emits the frontend Python) / `tools/audit_op_kinds.py` / `runtime/molt-tir/src/tir/op_kinds.toml` (`:140` `may_throw`, 38 throwing rows).
