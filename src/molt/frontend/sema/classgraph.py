"""Static class-graph analysis (doc 44 §F2b ``classgraph.py``).

Free functions over ``ast.Module`` and immutable class tables — the
``cfg_analysis.py`` house shape.  The static base graph, C3/static-MRO
linearization, reachability, and zero-arg ``super()`` fold soundness predicate
are computed outside the lowering generator and are unit-testable on bare facts
(the doc 44 §5.5 testability win).
"""

from __future__ import annotations

import ast

from collections.abc import Mapping, Sequence
from typing import Any

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


ClassTable = Mapping[str, Mapping[str, Any]]


def c3_merge(seqs: Sequence[Sequence[str]]) -> list[str] | None:
    """Merge C3 parent linearizations, returning ``None`` for inconsistency."""
    merged: list[str] = []
    working = [list(seq) for seq in seqs]
    heads = [0] * len(working)
    tail_counts: dict[str, int] = {}
    for seq in working:
        for name in seq[1:]:
            tail_counts[name] = tail_counts.get(name, 0) + 1

    while True:
        remaining = 0
        for idx, seq in enumerate(working):
            if heads[idx] < len(seq):
                remaining += 1
        if remaining == 0:
            return merged

        candidate: str | None = None
        for idx, seq in enumerate(working):
            head_idx = heads[idx]
            if head_idx >= len(seq):
                continue
            head = seq[head_idx]
            if tail_counts.get(head, 0) == 0:
                candidate = head
                break

        if candidate is None:
            return None

        merged.append(candidate)
        for idx, seq in enumerate(working):
            head_idx = heads[idx]
            if head_idx < len(seq) and seq[head_idx] == candidate:
                heads[idx] += 1
                next_head_idx = heads[idx]
                if next_head_idx < len(seq):
                    next_head = seq[next_head_idx]
                    count = tail_counts.get(next_head, 0)
                    if count <= 1:
                        tail_counts.pop(next_head, None)
                    else:
                        tail_counts[next_head] = count - 1


def static_class_bases(
    class_graph: ClassGraph, classes: ClassTable, class_name: str
) -> list[str] | None:
    """Return the single static base-name list for ``class_name`` if sound."""
    if class_name == "object":
        return ["object"]
    defs = class_graph.bases_by_class.get(class_name)
    if defs is not None:
        if len(defs) != 1:
            return None
        entry = defs[0]
        if "<opaque>" in entry:
            return None
        return list(entry)
    info = classes.get(class_name)
    if info is not None:
        if info.get("dynamic") or info.get("custom_metaclass"):
            return None
        return list(info.get("bases", []) or ["object"])
    return None


def static_mro_names(
    class_graph: ClassGraph,
    classes: ClassTable,
    class_name: str,
    _stack: tuple[str, ...] = (),
) -> list[str] | None:
    """Compute static C3 linearization for ``class_name`` or fail closed."""
    if class_name in _stack:
        return None
    if class_name == "object":
        return ["object"]
    bases = static_class_bases(class_graph, classes, class_name)
    if bases is None:
        return None
    base_mros: list[list[str]] = []
    for base in bases:
        base_mro = static_mro_names(class_graph, classes, base, _stack + (class_name,))
        if base_mro is None:
            return None
        base_mros.append(base_mro)
    base_mros.append(list(bases))
    merged = c3_merge(base_mros)
    if merged is None:
        return None
    return [class_name] + merged


def reachable_base_names(
    class_graph: ClassGraph,
    class_name: str,
    _seen: set[str] | None = None,
) -> set[str]:
    """Transitive base-name reachability over the static module class graph."""
    if _seen is None:
        _seen = set()
    if class_name in _seen:
        return _seen
    _seen.add(class_name)
    defs = class_graph.bases_by_class.get(class_name)
    if not defs:
        return _seen
    for entry in defs:
        for base in entry:
            if base != "<opaque>":
                reachable_base_names(class_graph, base, _seen)
    return _seen


def static_method_owner_after(
    classes: ClassTable, mro: Sequence[str], start: str, method: str
) -> str | None:
    """Return the first class defining ``method`` strictly after ``start``.

    This mirrors ``super(start, ...).method`` resolution for the static
    zero-arg-super fold. Missing class metadata is fail-closed because an
    unknown class could define the method.
    """
    mro_names = list(mro)
    if start not in mro_names:
        return None
    for name in mro_names[mro_names.index(start) + 1 :]:
        if name == "object":
            return None
        info = classes.get(name)
        if info is None:
            return None
        if method in info.get("methods", {}):
            return name
    return None


def visible_subclasses_of(
    class_graph: ClassGraph,
    class_name: str,
    classes: ClassTable,
) -> list[str] | None:
    """Return visible subclasses of ``class_name``, or ``None`` if uncertain."""
    subclasses: list[str] = []
    for other in class_graph.bases_by_class:
        if other == class_name:
            continue
        mro = static_mro_names(class_graph, classes, other)
        if mro is None:
            if class_name in reachable_base_names(class_graph, other):
                return None
            continue
        if class_name in mro:
            subclasses.append(other)
    return subclasses


def super_fold_is_sound(
    class_name: str,
    method: str,
    *,
    classes: ClassTable,
    class_graph: ClassGraph,
    module_name: str | None,
    entry_module: str | None,
) -> bool:
    """Soundness predicate for static zero-arg ``super().method(...)`` folding.

    The static fold is sound only when the class following ``class_name`` that
    defines ``method`` is identical across ``class_name`` and every visible
    subclass. Non-entry modules fail closed because downstream subclasses may be
    invisible to the current compilation unit.
    """
    is_entry = module_name == "__main__" or (
        entry_module is not None and module_name == entry_module
    )
    if not is_entry:
        return False
    if class_name not in class_graph.bases_by_class:
        return False
    own_mro = static_mro_names(class_graph, classes, class_name)
    if own_mro is None:
        return False
    expected_owner = static_method_owner_after(classes, own_mro, class_name, method)
    if expected_owner is None:
        return False
    subclasses = visible_subclasses_of(class_graph, class_name, classes)
    if subclasses is None:
        return False
    for sub in subclasses:
        sub_mro = static_mro_names(class_graph, classes, sub)
        if sub_mro is None:
            return False
        sub_owner = static_method_owner_after(classes, sub_mro, class_name, method)
        if sub_owner != expected_owner:
            return False
    return True
