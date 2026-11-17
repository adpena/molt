"""The one canonical form of a parsed module that every compiler pass reads.

Source spellings that mean the same program become one tree before any
analysis runs, so no analysis can prove a fact for one spelling and lose it
for the other (HF-888: ``self.a: int = a`` lost inline construction that
``self.a = a`` kept, because one matcher accepted only ``ast.Assign``).

Steps, in order:

1. Class-private names resolve (``resolve_python_private_names``).
2. In a function body, an annotated assignment with a value becomes the plain
   assignment. PEP 526: inside a function the annotation of a local or of a
   complex target (attribute, subscript) is neither evaluated nor stored, so
   ``x: T = v`` and ``x = v`` are the same program there. The annotation stays
   on the new node (``ANNOTATION``) as a hint that proves nothing. Module and
   class bodies keep their nodes: they evaluate and store annotations. A bare
   annotation (no value) keeps its node: it makes the name local without
   binding it. A name the function declares ``global`` or ``nonlocal`` keeps
   its node, so that program is still rejected as CPython's symtable rejects
   it.

The steps are idempotent and edit the tree in place.
"""

from __future__ import annotations

import ast

from molt.python_private_names import resolve_python_private_names

# The annotation an assignment carried in its source spelling.
ANNOTATION = "_molt_source_annotation"


def source_annotation(node: ast.Assign) -> ast.expr | None:
    """The annotation a plain assignment carried in source, if any."""
    return getattr(node, ANNOTATION, None)


def _declared_global_or_nonlocal(
    function: ast.FunctionDef | ast.AsyncFunctionDef,
) -> frozenset[str]:
    names: set[str] = set()
    stack: list[ast.AST] = list(function.body)
    while stack:
        node = stack.pop()
        if isinstance(
            node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda)
        ):
            continue
        if isinstance(node, (ast.Global, ast.Nonlocal)):
            names.update(node.names)
        stack.extend(ast.iter_child_nodes(node))
    return frozenset(names)


class _FunctionAnnotatedAssignments(ast.NodeTransformer):
    def __init__(self) -> None:
        self._in_function = False
        self._declared: frozenset[str] = frozenset()

    def _scope(
        self, node: ast.AST, in_function: bool, declared: frozenset[str]
    ) -> ast.AST:
        saved = self._in_function, self._declared
        self._in_function, self._declared = in_function, declared
        self.generic_visit(node)
        self._in_function, self._declared = saved
        return node

    def visit_FunctionDef(self, node: ast.FunctionDef) -> ast.AST:
        return self._scope(node, True, _declared_global_or_nonlocal(node))

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> ast.AST:
        return self._scope(node, True, _declared_global_or_nonlocal(node))

    def visit_ClassDef(self, node: ast.ClassDef) -> ast.AST:
        return self._scope(node, False, frozenset())

    def visit_AnnAssign(self, node: ast.AnnAssign) -> ast.AST:
        if not self._in_function or node.value is None:
            return node
        if isinstance(node.target, ast.Name) and node.target.id in self._declared:
            return node
        assign = ast.copy_location(
            ast.Assign(targets=[node.target], value=node.value), node
        )
        setattr(assign, ANNOTATION, node.annotation)
        return assign


def canonical_python_module(tree: ast.Module) -> ast.Module:
    """Bring a parsed module to its canonical form in place and return it."""
    resolve_python_private_names(tree)
    _FunctionAnnotatedAssignments().visit(tree)
    return tree
