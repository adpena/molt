"""Static class-graph analysis (doc 44 §F2b ``classgraph.py``).

Free functions over ``ast.Module`` and immutable class tables — the
``cfg_analysis.py`` house shape.  The static base graph, C3/static-MRO
linearization, reachability, class-body block-exec decisions, and zero-arg
``super()`` fold soundness facts are computed outside the lowering generator
and are unit-testable on bare facts (the doc 44 §5.5 testability win).
"""

from __future__ import annotations

import ast

from collections.abc import Mapping, Sequence
from dataclasses import replace
from typing import Any

from molt.frontend.sema.result import ClassFacts, ClassGraph


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


CLASS_BODY_SIMPLE_STMTS = (
    ast.FunctionDef,
    ast.AsyncFunctionDef,
    ast.ClassDef,
    ast.Assign,
    ast.AnnAssign,
    ast.Expr,
    ast.Pass,
)


def class_body_needs_block_exec(body: Sequence[ast.stmt]) -> bool:
    """True when a class body must execute through the mutable namespace block."""
    for stmt in body:
        if not isinstance(stmt, CLASS_BODY_SIMPLE_STMTS):
            return True
        if isinstance(stmt, ast.Assign):
            if any(not isinstance(t, ast.Name) for t in stmt.targets):
                return True
        elif isinstance(stmt, ast.AnnAssign):
            if not isinstance(stmt.target, ast.Name):
                return True
    return False


def _is_property_update_method(node: ast.FunctionDef | ast.AsyncFunctionDef) -> bool:
    if len(node.decorator_list) != 1:
        return False
    decorator = node.decorator_list[0]
    return (
        isinstance(decorator, ast.Attribute)
        and isinstance(decorator.value, ast.Name)
        and decorator.value.id == node.name
        and decorator.attr in {"setter", "deleter"}
    )


def _is_inert_class_expr(node: ast.Expr) -> bool:
    return isinstance(node.value, ast.Constant) and isinstance(
        node.value.value, (str, bytes, int, float, complex, bool, type(None))
    )


def _class_member_facts_are_opaque(
    node: ast.ClassDef, *, body_needs_block: bool
) -> bool:
    if node.decorator_list or node.keywords:
        return True
    if body_needs_block:
        return True
    for item in node.body:
        if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
            if item.decorator_list and not _is_property_update_method(item):
                return True
        elif isinstance(item, ast.Expr) and not _is_inert_class_expr(item):
            return True
    return False


def build_class_facts(node: ast.Module) -> ClassFacts:
    """Collect final class-body member facts used by class semantic predicates."""
    method_names_by_class: dict[str, frozenset[str]] = {}
    attr_names_by_class: dict[str, frozenset[str]] = {}
    counts: dict[str, int] = {}
    opaque: set[str] = set()
    block_exec_nodes: set[int] = set()

    def target_names(target: ast.AST) -> list[str]:
        if isinstance(target, ast.Name):
            return [target.id]
        if isinstance(target, ast.Starred):
            return target_names(target.value)
        if isinstance(target, (ast.Tuple, ast.List)):
            names: list[str] = []
            for elt in target.elts:
                names.extend(target_names(elt))
            return names
        return []

    def bind_method(members: dict[str, str], name: str) -> None:
        members[name] = "method"

    def bind_attr(members: dict[str, str], name: str) -> None:
        members[name] = "attr"

    def delete_name(members: dict[str, str], name: str) -> None:
        members.pop(name, None)

    for stmt in ast.walk(node):
        if not isinstance(stmt, ast.ClassDef):
            continue
        counts[stmt.name] = counts.get(stmt.name, 0) + 1
        body_needs_block = class_body_needs_block_exec(stmt.body)
        if body_needs_block:
            block_exec_nodes.add(id(stmt))
        if _class_member_facts_are_opaque(stmt, body_needs_block=body_needs_block):
            opaque.add(stmt.name)
        members: dict[str, str] = {}
        for item in stmt.body:
            if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                if not _is_property_update_method(item):
                    bind_method(members, item.name)
                continue
            if isinstance(item, ast.ClassDef):
                bind_attr(members, item.name)
                continue
            if isinstance(item, ast.Assign):
                for target in item.targets:
                    for name in target_names(target):
                        bind_attr(members, name)
                continue
            if isinstance(item, ast.AnnAssign):
                if item.value is not None:
                    for name in target_names(item.target):
                        bind_attr(members, name)
                continue
            if isinstance(item, ast.AugAssign):
                for name in target_names(item.target):
                    bind_attr(members, name)
                continue
            if isinstance(item, (ast.Import, ast.ImportFrom)):
                for alias in item.names:
                    bind_attr(members, alias.asname or alias.name.split(".", 1)[0])
                continue
            if isinstance(item, ast.Delete):
                for target in item.targets:
                    for name in target_names(target):
                        delete_name(members, name)
                continue

        method_names_by_class[stmt.name] = frozenset(
            name for name, kind in members.items() if kind == "method"
        )
        attr_names_by_class[stmt.name] = frozenset(
            name for name, kind in members.items() if kind == "attr"
        )

    ambiguous = frozenset(name for name, count in counts.items() if count != 1)
    return ClassFacts(
        method_names_by_class=method_names_by_class,
        attr_names_by_class=attr_names_by_class,
        opaque_member_class_names=frozenset(opaque),
        ambiguous_class_names=ambiguous,
        block_exec_class_nodes=frozenset(block_exec_nodes),
        super_fold_sound_methods_by_class={},
    )


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


def _local_class_known(class_facts: ClassFacts, class_name: str) -> bool:
    return (
        class_name in class_facts.method_names_by_class
        or class_name in class_facts.attr_names_by_class
    )


def _class_member_state(
    class_facts: ClassFacts,
    imported_classes: ClassTable,
    class_name: str,
    member: str,
) -> str | None:
    """Return ``method``, ``blocked``, ``absent``, or ``None`` for unknown."""
    if (
        class_name in class_facts.ambiguous_class_names
        or class_name in class_facts.opaque_member_class_names
    ):
        return None
    if _local_class_known(class_facts, class_name):
        method_names = class_facts.method_names_by_class.get(class_name, frozenset())
        attr_names = class_facts.attr_names_by_class.get(class_name, frozenset())
        if member in attr_names:
            return "blocked"
        if member in method_names:
            return "method"
        return "absent"

    info = imported_classes.get(class_name)
    if info is None:
        return None
    methods = info.get("methods", {})
    class_attrs = info.get("class_attrs", {})
    if member in class_attrs:
        return "blocked"
    if member in methods:
        return "method"
    return "absent"


def static_method_owner_after(
    class_facts: ClassFacts,
    imported_classes: ClassTable,
    mro: Sequence[str],
    start: str,
    method: str,
) -> str | None:
    """Return the first class defining ``method`` strictly after ``start``.

    This mirrors ``super(start, ...).method`` resolution for the static
    zero-arg-super fold. Missing class metadata and non-method class attribute
    interposition are fail-closed because either could change runtime lookup.
    """
    mro_names = list(mro)
    if start not in mro_names:
        return None
    for name in mro_names[mro_names.index(start) + 1 :]:
        if name == "object":
            return None
        state = _class_member_state(class_facts, imported_classes, name, method)
        if state is None or state == "blocked":
            return None
        if state == "method":
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
    class_facts: ClassFacts,
    imported_classes: ClassTable,
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
    own_mro = static_mro_names(class_graph, imported_classes, class_name)
    if own_mro is None:
        return False
    expected_owner = static_method_owner_after(
        class_facts, imported_classes, own_mro, class_name, method
    )
    if expected_owner is None:
        return False
    subclasses = visible_subclasses_of(class_graph, class_name, imported_classes)
    if subclasses is None:
        return False
    for sub in subclasses:
        sub_mro = static_mro_names(class_graph, imported_classes, sub)
        if sub_mro is None:
            return False
        sub_owner = static_method_owner_after(
            class_facts, imported_classes, sub_mro, class_name, method
        )
        if sub_owner != expected_owner:
            return False
    return True


def class_facts_with_super_fold_sound_methods(
    *,
    class_graph: ClassGraph,
    class_facts: ClassFacts,
    imported_classes: ClassTable,
    module_name: str | None,
    entry_module: str | None,
) -> ClassFacts:
    """Return ``class_facts`` with zero-arg-super fold decisions precomputed."""
    candidate_methods: set[str] = set()
    for methods in class_facts.method_names_by_class.values():
        candidate_methods.update(methods)
    for info in imported_classes.values():
        candidate_methods.update(info.get("methods", {}).keys())

    sound_by_class: dict[str, frozenset[str]] = {}
    for class_name in class_graph.bases_by_class:
        sound_methods = {
            method
            for method in candidate_methods
            if super_fold_is_sound(
                class_name,
                method,
                class_facts=class_facts,
                imported_classes=imported_classes,
                class_graph=class_graph,
                module_name=module_name,
                entry_module=entry_module,
            )
        }
        sound_by_class[class_name] = frozenset(sound_methods)
    return replace(
        class_facts,
        super_fold_sound_methods_by_class=sound_by_class,
    )
