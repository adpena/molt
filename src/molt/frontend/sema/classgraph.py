"""Static class-graph analysis (doc 44 §F2b ``classgraph.py``).

Free function over ``ast.Module`` — the ``cfg_analysis.py`` house shape.  Lifts
``SimpleTIRGenerator._collect_module_class_graph`` verbatim so that the static
base-graph used to reason about the zero-arg ``super()`` fold is computed once,
pre-walk, and is unit-testable on a bare AST (the doc 44 §5.5 testability win).
"""

from __future__ import annotations

import ast

from molt.frontend.sema.result import ClassGraph


def build_class_graph(node: ast.Module) -> ClassGraph:
    """Build the module-wide static class graph used to reason about the
    zero-arg ``super()`` fold *before* any method body is compiled.

    Returns a :class:`ClassGraph` with ``bases_by_class`` / ``subclassed_names``:

    * ``bases_by_class`` maps every class-statement name in this module
      (top-level, nested, or function-local) to the list of *base-name
      lists* across all class statements that define that name.  A name can
      have multiple definitions (re-binding / conditional class defs); the
      fold must hold for every one, so all are retained.  A class whose
      bases are not all simple ``ast.Name`` references contributes the
      sentinel ``["<opaque>"]`` for that definition, which forces any
      MRO computation through it to bail (fail-closed).

    * ``subclassed_names`` is the set of names referenced as a base anywhere
      (the conservative "is this class ever subclassed" view), including
      dotted/attribute bases and names appearing inside computed bases.

    ``super()`` in ``C.method`` resolves to the class following ``C`` in
    ``type(self).__mro__``.  Because the method is inherited, ``self`` may be
    any subclass instance, and a diamond subclass can interpose a cooperative
    C3 sibling between ``C`` and ``C``'s lexical next base.  The fold is only
    sound when the *method-resolution successor* of ``C`` is identical across
    ``C`` and every visible subclass of ``C`` — which requires the full base
    graph of classes that may be defined *after* ``C`` in source order.
    """
    bases_by_class: dict[str, list[list[str]]] = {}
    subclassed: set[str] = set()
    _OPAQUE = "<opaque>"

    def simple_base_name(expr: ast.expr) -> str | None:
        if isinstance(expr, ast.Name):
            return expr.id
        return None

    def record_subclassed(expr: ast.expr) -> None:
        if isinstance(expr, ast.Name):
            subclassed.add(expr.id)
            return
        if isinstance(expr, ast.Attribute):
            current: ast.expr | None = expr
            while isinstance(current, ast.Attribute):
                subclassed.add(current.attr)
                current = current.value
            if isinstance(current, ast.Name):
                subclassed.add(current.id)
            return
        for sub in ast.walk(expr):
            if isinstance(sub, ast.Name):
                subclassed.add(sub.id)

    for stmt in ast.walk(node):
        if not isinstance(stmt, ast.ClassDef):
            continue
        base_names: list[str] = []
        opaque = False
        # A keyword base (metaclass=, or any keyword) makes the class
        # dynamically built: treat its MRO as un-foldable.
        if stmt.keywords:
            opaque = True
        for base_expr in stmt.bases:
            record_subclassed(base_expr)
            name = simple_base_name(base_expr)
            if name is None:
                opaque = True
            else:
                base_names.append(name)
        for kw in stmt.keywords:
            if isinstance(kw.value, (ast.Name, ast.Attribute)):
                record_subclassed(kw.value)
        entry = [_OPAQUE] if opaque else (base_names or ["object"])
        bases_by_class.setdefault(stmt.name, []).append(entry)
    return ClassGraph(bases_by_class=bases_by_class, subclassed_names=subclassed)
