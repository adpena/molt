"""The canonical module form: one tree per program, whatever the spelling.

Oracle: PEP 526 and CPython's compiler. Inside a function an annotated
assignment with a value stores like the plain assignment and neither
evaluates nor stores its annotation; module and class bodies evaluate and
store theirs. The initializer test checks the consequence the rewrite exists
for: both spellings of an ``__init__`` inline the same field stores.
"""

from __future__ import annotations

import ast

from molt.python_canonical_ast import canonical_python_module, source_annotation

SOURCE = """
x: int = 1
y: int

class C:
    field: int = 2

    def method(self, a):
        self.a: int = a
        local: str = "s"
        bare: int
        return local

def outer():
    global x
    x: int = 3
    def inner():
        z: float = 1.0
        class Inner:
            attr: bytes = b""
        return z
    return inner
"""


def _kinds(tree: ast.Module, name: str) -> list[str]:
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.ClassDef)) and node.name == name:
            return [type(statement).__name__ for statement in node.body]
    raise AssertionError(name)


def test_function_scope_annotated_assignments_become_plain_assignments() -> None:
    tree = canonical_python_module(ast.parse(SOURCE))
    # Module and class bodies keep their annotations.
    assert [type(s).__name__ for s in tree.body[:2]] == ["AnnAssign", "AnnAssign"]
    assert _kinds(tree, "C")[0] == "AnnAssign"
    assert _kinds(tree, "Inner") == ["AnnAssign"]
    # A method body: values become plain stores; a bare annotation stays.
    assert _kinds(tree, "method") == ["Assign", "Assign", "AnnAssign", "Return"]
    method = next(
        n
        for n in ast.walk(tree)
        if isinstance(n, ast.FunctionDef) and n.name == "method"
    )
    first = method.body[0]
    assert isinstance(first, ast.Assign)
    assert isinstance(first.targets[0], ast.Attribute)
    annotation = source_annotation(first)
    assert isinstance(annotation, ast.Name) and annotation.id == "int"
    assert (first.lineno, first.col_offset) == (9, 8)
    # A global name keeps its node, so the program is still rejected.
    assert _kinds(tree, "outer")[:2] == ["Global", "AnnAssign"]
    assert _kinds(tree, "inner")[0] == "Assign"


def test_the_canonical_form_is_a_fixed_point() -> None:
    once = ast.dump(canonical_python_module(ast.parse(SOURCE)), include_attributes=True)
    twice_tree = canonical_python_module(canonical_python_module(ast.parse(SOURCE)))
    assert ast.dump(twice_tree, include_attributes=True) == once


def _init_assigns(source: str) -> list[tuple[str, str]] | None:
    from molt.frontend.visitors.class_method_compilation import (
        ClassMethodCompilationMixin,
    )

    tree = canonical_python_module(ast.parse(source))
    init = next(
        n
        for n in ast.walk(tree)
        if isinstance(n, ast.FunctionDef) and n.name == "__init__"
    )
    params = [arg.arg for arg in init.args.args]
    found = ClassMethodCompilationMixin._extract_inline_init_assigns(
        object.__new__(ClassMethodCompilationMixin), init, params
    )
    return None if found is None else [(name, ast.dump(v)) for name, v in found]


def test_an_annotated_initializer_inlines_as_the_plain_one_does() -> None:
    plain = """
class Pair:
    def __init__(self, a, b):
        \"\"\"Doc.\"\"\"
        self.a = a
        self.b = b
"""
    annotated = plain.replace("self.a = a", "self.a: int = a").replace(
        "self.b = b", "self.b: int = b"
    )
    assert _init_assigns(plain) == [
        ("a", ast.dump(ast.Name("a", ast.Load()))),
        ("b", ast.dump(ast.Name("b", ast.Load()))),
    ]
    assert _init_assigns(annotated) == _init_assigns(plain)


def test_module_identity_does_not_depend_on_who_canonicalized_first() -> None:
    """Two digest sites of one source agree whether or not another consumer
    already rewrote the shared tree in place."""
    from molt.compiler_analysis.python_source_keys import _PythonAstDigestAdmission
    from molt.python_canonical_ast import canonical_module_digest

    raw = ast.parse(SOURCE)
    rewritten = canonical_python_module(ast.parse(SOURCE))
    assert canonical_module_digest(raw) == canonical_module_digest(rewritten)
    assert _PythonAstDigestAdmission(
        ast.parse(SOURCE)
    ).digest == canonical_module_digest(ast.parse(SOURCE))
