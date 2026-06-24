<!-- Foundation blueprint 21c. Architect: frontend-architect (Plan agent), 2026-06-23.
Arc: decomposition move #2 / F1 — split SimpleTIRGenerator into visitor mixins. Verified
against the working tree. Move-only / zero-behavior-change (byte-identical TIR gate).
Companion to 21 (program), 21b (crate graph). Design only. -->

# 21c — Decompose `SimpleTIRGenerator` into Visitor Mixins (Move #2 / F1)

## Verdict
Move #2 is **partially complete, but the mega-class is still the mega-class.** Eight mixins
exist and are wired into the MRO, yet `src/molt/frontend/__init__.py` remains a god file and
`SimpleTIRGenerator` still defines the large emit/analysis surface plus statement visitor
families inline. The recent refactor extracted the
infrastructure (`_types.py` leaf, `_protocol.py` typing shim, `_MixinBase` pattern) +
serialization, async/generator lowering, pattern matching, calls, classes, comprehensions, and
expression and function/lambda/return visitors. The remaining move is still a Python package refactor (no compile step): the win is
edit-locality + parallel ownership, NOT build time. Constraint: move-only, ZERO behavior change.

## Current state (verified)
- `__init__.py` L243: `class SimpleTIRGenerator(SerializationMixin, PatternMatchMixin,
  AsyncGenVisitorMixin, CallVisitorMixin, ClassDefVisitorMixin, ComprehensionMixin,
  ExpressionVisitorMixin, FunctionVisitorMixin,
  ast.NodeVisitor)`. `ast.NodeVisitor` is correctly LAST. The class overrides `visit()` and
  calls `super().visit(node)` →
  resolves to `NodeVisitor.visit` (the dispatcher). **This ordering is load-bearing — every
  iteration must keep `ast.NodeVisitor` last.**
- Already extracted: `visitors/async_gen.py` (AsyncGenVisitorMixin), `visitors/calls.py`
  (CallVisitorMixin), `visitors/classes.py`
  (ClassDefVisitorMixin), `visitors/pattern_match.py` (PatternMatchMixin, owns
  `visit_Match`), `visitors/comprehensions.py` (ComprehensionMixin), `visitors/functions.py`
  (FunctionVisitorMixin), `visitors/expressions.py` (ExpressionVisitorMixin), and
  `lowering/serialization.py` (SerializationMixin, to_json).
  `_types.py` (data leaf), `_protocol.py`+`_protocol_attrs.py` (the `_GeneratorProtocol`
  self-typing shim, GENERATED — regenerate, never hand-edit), and
  `lowering/op_kinds_generated.py` (generated) remain the support surface.

### The 50 inline `visit_*` grouped (with line numbers)
- **Expressions** → `visitors/expressions.py` (17): landed for scalar names/constants,
  string/template strings, collection literals, indexing/slicing, attributes, named
  expressions, comparison/unary/binary operations, conditional expressions, and boolean ops.
- **Statements** → `visitors/statements.py` (20): visit_Module(2767), Global(5980),
  Nonlocal(5986), AnnAssign(13286), TypeAlias(13504), Assign(13932), Delete(14391),
  AugAssign(14591), If(15069), With(15150), For(15491), While(15965), Try(16620),
  TryStar(17037), Raise(17864), Assert(18005), Break(18050), Continue(18080), Import(19865),
  ImportFrom(19911).
- **Functions** → `visitors/functions.py` (3): landed for `visit_FunctionDef`,
  `visit_Lambda`, and `visit_Return`.
- **Comprehensions** → `visitors/comprehensions.py` (4): landed for `visit_ListComp`,
  `visit_SetComp`, `visit_DictComp`, and `visit_GeneratorExp`.
- **Async/generators** → `visitors/async_gen.py` (6): landed for
  `visit_AsyncFunctionDef`, `visit_AsyncWith`, `visit_AsyncFor`, `visit_Await`,
  `visit_Yield`, and `visit_YieldFrom`.

## Target layout
New mixins (each `class XxxMixin(_MixinBase):` with the verbatim `_MixinBase` shim header from
`calls.py` L41-47 — `if TYPE_CHECKING: _MixinBase = _GeneratorProtocol else: _MixinBase = object`):
the remaining statement visitor subfamilies + `lowering/{emit, analysis}.py`
(emit = the family-agnostic `_emit_*` primitives;
analysis = `_collect_*` + `_static_*` + `_midend_*` + the 19 loop-recognizer `_match_*`).

Final assembly in `__init__.py` keeps `__init__`/shared state/overridden `visit()` + the public
`compile_to_tir`, with the base tuple `class SimpleTIRGenerator(<remaining mixins>,
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
7. **Regenerate `_protocol.py` / `_protocol_attrs.py`** after each extraction
   (`python tools/gen_protocol.py`) so cross-family `self.*` references stay
   type-clean. Never hand-edit generated protocol files.

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
4. DONE: `visitors/expressions.py` (17).
5. DONE: `visitors/async_gen.py` (6 async/gen).
6. DONE: `visitors/functions.py` (FunctionDef/Lambda/Return).
7. DONE: `visitors/comprehensions.py` (4).

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
