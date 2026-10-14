"""The ``SemaResult`` keystone artifact — immutable semantic-analysis facts.

This is the data contract of doc 44 §1.2: the analog of Clang's annotated AST /
CPython's ``symtable``.  ``SemaResult`` is computed **once** by free functions
over the immutable ``ast.Module`` (the ``cfg_analysis.py`` house shape — zero
``self``, zero god-object state) *before* the lowering walk, and is read by
``Lower`` rather than recomputed inline.

F2b (the additive-shim phase) computes ``SemaResult`` and populates the existing
``SimpleTIRGenerator`` state dicts from it, leaving the walk byte-identical.  The
old inline computations are deleted only once their ``SemaResult`` twin proves
byte-equal — F2c rewires the walk to read these tables at the use sites.

Scope of this phase: the module-level pre-walk facts that are written **exactly
once** in ``visit_Module`` and only read thereafter (class-body block facts,
the const environment, and per-top-level-function metadata).
Walk-mutated cursors (``const_ints`` written in ``emit()``, ``exact_locals``
mutated across visit methods, the per-function scope dicts populated lazily in
``start_function``) are deliberately **excluded** — they are cursors, not Sema
facts, and mis-classifying one is a miscompile (doc 44 risk #1).
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from molt.frontend.sema.funcmeta import FunctionKind


@dataclass(frozen=True)
class ClassFacts:
    """Per-class AST facts computed before lowering.

    ``block_exec_class_nodes`` records class bodies whose namespace must be
    executed through the mutable block path.
    """

    block_exec_class_nodes: frozenset[int]


@dataclass(frozen=True)
class FunctionMeta:
    """Per-top-level-function metadata computed pre-walk.

    * ``declared_funcs`` maps each top-level ``def``/``async def`` name to its
      canonical :class:`FunctionKind`.
    * ``declared_classes`` is the set of top-level ``class`` names.
    * ``defaults`` maps each top-level function name to its default/param-shape
      spec (param count, default specs, posonly/kwonly counts, function kind,
      decorator presence, or the ``{"has_vararg": True}`` marker).  This
      mirrors the AST-derived value the walk computes; an externally-supplied
      ``known_func_defaults`` override is applied by the populate-shim, not
      here.
    """

    declared_funcs: dict[str, FunctionKind]
    declared_classes: set[str]
    defaults: dict[str, dict[str, Any]]


@dataclass(frozen=True)
class SemaResult:
    """Immutable annotation tables, the F2c worklist's read-source.

    Keyed coarsely by the analysis family today; F2c narrows the read-sites onto
    these fields and deletes the corresponding mutable god-object dicts.
    """

    class_facts: ClassFacts
    const_dicts: dict[str, dict[str, Any]]
    function_meta: FunctionMeta
    # Module-scope names an import binds (CPython's DEF_IMPORT symbols). The
    # call-form rule reads them: a call on an imported name's attribute never
    # takes the method-call form.
    import_names: frozenset[str]
