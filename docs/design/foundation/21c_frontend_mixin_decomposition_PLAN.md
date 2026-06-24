<!-- Foundation blueprint 21c. Architect: frontend-architect (Plan agent), 2026-06-23.
Arc: decomposition move #2 / F1 — split SimpleTIRGenerator into visitor mixins. Verified
against the working tree. Move-only / zero-behavior-change (byte-identical TIR gate).
Companion to 21 (program), 21b (crate graph). Design only. -->

# 21c — Decompose `SimpleTIRGenerator` into Visitor Mixins (Move #2 / F1)

## Verdict
Move #2 is **partially complete, but the mega-class is still the mega-class.** Four mixins
exist and are wired into the MRO, yet `src/molt/frontend/__init__.py` is **27,939 lines** and
`SimpleTIRGenerator` still defines **~570 methods inline**, including **50 of ~58 top-level
`visit_*` handlers** and **158 `_emit_*` primitives**. The recent refactor extracted the
infrastructure (`_types.py` leaf, `_protocol.py` typing shim, `_MixinBase` pattern) + 3 visitor
families + serialization (~10-15% by method count). The remaining ~85% is this move. This is a
Python package refactor (no compile step) — the win is edit-locality + parallel-ownership, NOT
build time. Constraint: move-only, ZERO behavior change.

## Current state (verified)
- `__init__.py` L243: `class SimpleTIRGenerator(SerializationMixin, PatternMatchMixin,
  CallVisitorMixin, ClassDefVisitorMixin, ast.NodeVisitor)`. `ast.NodeVisitor` is correctly
  LAST. The class overrides `visit()` (L1179) and calls `super().visit(node)` (L1197, L1200) →
  resolves to `NodeVisitor.visit` (the dispatcher). **This ordering is load-bearing — every
  iteration must keep `ast.NodeVisitor` last.**
- Inline census: ~570 methods; 50 `visit_*`; 158 `_emit_*`; 38 `_collect_*`; 19 `_match_*`
  (loop recognizers); 6 `_midend_*`; 4 `_static_*`; 33 `@staticmethod`; 1 `@contextmanager`.
- Already extracted: `visitors/calls.py` (CallVisitorMixin, 71 defs), `visitors/classes.py`
  (ClassDefVisitorMixin, 20), `visitors/pattern_match.py` (PatternMatchMixin, owns
  `visit_Match`), `lowering/serialization.py` (SerializationMixin, to_json). `_types.py` (data
  leaf), `_protocol.py`+`_protocol_attrs.py` (the `_GeneratorProtocol` self-typing shim,
  GENERATED — regenerate, never hand-edit), `lowering/op_kinds_generated.py` (generated).

### The 50 inline `visit_*` grouped (with line numbers)
- **Expressions** → `visitors/expressions.py` (17): visit_Name(5873), BinOp(11197),
  Constant(11364), JoinedStr(11808), TemplateStr(11905), List(12686), Tuple(12728), Set(12772),
  Dict(12814), Subscript(12920), Slice(12989), Attribute(13275), NamedExpr(14382),
  Compare(14824), UnaryOp(14906), IfExp(14942), BoolOp(17616).
- **Statements** → `visitors/statements.py` (20): visit_Module(2767), Global(5980),
  Nonlocal(5986), AnnAssign(13286), TypeAlias(13504), Assign(13932), Delete(14391),
  AugAssign(14591), If(15069), With(15150), For(15491), While(15965), Try(16620),
  TryStar(17037), Raise(17864), Assert(18005), Break(18050), Continue(18080), Import(19865),
  ImportFrom(19911).
- **Functions** → `visitors/functions.py` (3): visit_FunctionDef(18809), Lambda(19400),
  Return(18123).
- **Comprehensions** → `visitors/comprehensions.py` (4): visit_ListComp(12469), SetComp(12496),
  DictComp(12512), GeneratorExp(12535).
- **Async/generators** → `visitors/async_gen.py` (6): visit_AsyncFunctionDef(18188),
  AsyncWith(15320), AsyncFor(15865), Await(20288), Yield(20592), YieldFrom(20655).

## Target layout
New mixins (each `class XxxMixin(_MixinBase):` with the verbatim `_MixinBase` shim header from
`calls.py` L41-47 — `if TYPE_CHECKING: _MixinBase = _GeneratorProtocol else: _MixinBase = object`):
`visitors/{expressions, statements, functions, comprehensions, async_gen}.py` (the visitor
families) + `lowering/{emit, analysis}.py` (emit = the family-agnostic `_emit_*` primitives;
analysis = `_collect_*` + `_static_*` + `_midend_*` + the 19 loop-recognizer `_match_*`).

Final assembly in `__init__.py` keeps `__init__`/shared state/overridden `visit()` + the public
`compile_to_tir`, with the base tuple `class SimpleTIRGenerator(<11 mixins, alphabetical>,
ast.NodeVisitor)`. Target __init__.py < 2,500 lines (the shared per-instance state block
L273-660+ is irreducible — do not force <800).

## ROUTING TRAP (most likely executor error)
`visit_Match` is ALREADY extracted (pattern_match.py). The **19 inline `_match_*` methods are
LOOP-PATTERN RECOGNIZERS** (`_match_vector_reduction_loop` L8617, `_match_counted_while` L10905,
`_match_matmul_loop` L11713, `_match_dict_increment_assign` L13962, …) — midend analysis, NOT
PEP-634. **Route them to `lowering/analysis.py`, NOT `pattern_match.py`.**

## Mechanics (Python, move-only)
1. Create the mixin module with the `calls.py` header (docstring, `from __future__ import
   annotations`, `from molt.frontend._types import (...)` only names used, the `_MixinBase` shim).
2. Cut methods VERBATIM into `class XxxMixin(_MixinBase):` — preserve indentation, decorators,
   signatures, bodies byte-for-byte. `self.*` access resolves via MRO; do NOT refactor.
3. Move any module-level file-private constant a method references (grep cut methods for bare
   non-self/non-imported names).
4. Add `from molt.frontend.visitors.xxx import XxxMixin` + insert into the base tuple before
   `ast.NodeVisitor`.
5. Mixins import ONLY from `molt.frontend._types`, `molt.frontend._protocol` (TYPE_CHECKING),
   `ast`, stdlib — NEVER `from molt.frontend import ...` and never each other (no cycles).
6. `@staticmethod`/`@contextmanager` move with decorators intact; callers via `self._foo` still
   resolve through MRO.
7. **Regenerate `_protocol.py`** after each extraction (`python tmp/gen_protocol.py` or
   equivalent) so cross-family `self.*` references stay type-clean. Never hand-edit it.

## Verification (per extraction + final)
1. Import/MRO sanity (cheapest first): instantiate `SimpleTIRGenerator()`, assert all `visit_*`
   resolve, assert `ast.NodeVisitor` is last in `__mro__`, assert each moved handler's
   `__qualname__` names the new mixin (no duplicate ownership).
2. **Byte-identical TIR (core move-only gate G3):** for a fixed corpus
   (`tests/differential/{basic,loop_overflow_peel,memory,pyperformance,stdlib}`) capture
   `compile_to_tir(...)`/`to_json()` BEFORE, then AFTER, and `diff` — must be byte-identical.
   Any diff = behavior change = revert.
3. `pytest tests/test_frontend_midend_passes.py`.
4. `pytest tests/differential/` (the authoritative behavior oracle).
5. Public-surface: `from molt.frontend import MoltValue, MoltOp, ClassInfo, FuncInfo,
   compile_to_tir, SimpleTIRGenerator` all resolve (external importers: cli.py, debug/ir.py).

## Ordering (each commit green; largest/most-contended first per dependency)
1. `lowering/emit.py` (158 `_emit_*` — most-shared, biggest contention reducer; unblocks visitors).
2. `lowering/analysis.py` (38 `_collect_*` + 6 `_midend_*` + 4 `_static_*` + the 19 loop `_match_*`).
3. `visitors/statements.py` (20 handlers incl. the giants For/While/Try/Assign).
4. `visitors/expressions.py` (17).
5. `visitors/functions.py` (FunctionDef/Lambda/Return).
6. `visitors/async_gen.py` (6 async/gen).
7. `visitors/comprehensions.py` (4 — smallest, last).

**Parallel-ownership win:** after steps 1-2 land (stable emit/analysis surface), steps 3-7 touch
disjoint files → separate agents can own them in parallel; only the base-tuple line + the
`_protocol.py` regen are shared touch points (serialize just those). NOTE: starting with a small
self-contained family (e.g. comprehensions) as a MECHANIC-VALIDATION first is acceptable —
emit/analysis stay in __init__.py, reachable via `self.*` MRO — and de-risks the pipeline before
the large extractions.

## Risks (tree-specific)
- `_match_*` mis-routing (above).
- Generated files (`_protocol.py`, `_protocol_attrs.py`, `lowering/op_kinds_generated.py`) —
  regenerate, never hand-edit.
- MRO last-position: a mixin after `ast.NodeVisitor` breaks `super().visit()` (recursion/wrong
  dispatch). The MRO assertion catches it.
- Module-private constants left behind → `NameError` at runtime (the import sanity check catches it).
