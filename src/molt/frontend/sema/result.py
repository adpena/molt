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
once** in ``visit_Module`` and only read thereafter (the class graph, local
class member facts, the const environment, and per-top-level-function metadata).
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
class ClassGraph:
    """The module-wide static class graph — the soundness substrate for the
    zero-arg ``super()`` fold (doc 44 §2.2, ``ClassFacts``).

    * ``bases_by_class`` maps every class-statement name in the module
      (top-level, nested, or function-local) to the list of *base-name lists*
      across all class statements that define that name.  A class whose bases
      are not all simple ``ast.Name`` references contributes the sentinel
      ``["<opaque>"]`` for that definition.
    * ``subclassed_names`` is the set of names referenced as a base anywhere.
    """

    bases_by_class: dict[str, list[list[str]]]
    subclassed_names: set[str]


@dataclass(frozen=True)
class ClassFacts:
    """Per-class AST member facts computed before lowering.

    ``method_names_by_class`` contains names whose final class-body binding is
    a method definition. ``attr_names_by_class`` contains names whose final
    binding is a non-method class attribute. A deleted name appears in neither
    set. ``opaque_member_class_names`` records class bodies whose member set
    cannot be proven from straight-line syntax. Repeated class definitions are
    retained as ``ambiguous_class_names`` so consumers fail closed instead of
    merging incompatible bodies.
    """

    method_names_by_class: dict[str, frozenset[str]]
    attr_names_by_class: dict[str, frozenset[str]]
    opaque_member_class_names: frozenset[str]
    ambiguous_class_names: frozenset[str]


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

    class_graph: ClassGraph
    class_facts: ClassFacts
    const_dicts: dict[str, dict[str, Any]]
    function_meta: FunctionMeta
