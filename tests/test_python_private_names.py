from __future__ import annotations

import ast
import dis
import types
import pytest

from molt.compiler_analysis.python_binding_flow import analyze_python_bindings
from molt.compiler_analysis.python_private_names import (
    python_definition_name,
    python_import_binding,
    python_source_unparse,
    resolve_python_private_names,
)
from molt.compiler_analysis.python_source_keys import python_ast_digest
from molt.frontend import compile_to_tir


def code_named(code: types.CodeType, name: str) -> types.CodeType:
    return next(
        value
        for value in code.co_consts
        if isinstance(value, types.CodeType) and value.co_name == name
    )


def test_private_scope_bindings_match_cpython_code_objects() -> None:
    source = """
class __Owner:
    def __method(self, __argument):
        global __global
        __local = __argument
        self.__attribute = __local
        def __nested():
            nonlocal __local
            __local += 1
            return self.__attribute, __global
        return __nested
    class __Child:
        def method(self):
            return self.__attribute
class ___:
    def method(self, __argument):
        return self.__attribute, __argument
"""
    original = ast.parse(source)
    code = compile(source, "<private>", "exec")
    tree = resolve_python_private_names(ast.parse(source))
    owner, underscores = tree.body
    method, child = owner.body
    owner_code = code_named(code, "__Owner")
    method_code = code_named(owner_code, "__method")
    assert method.name in owner_code.co_names
    assert method.args.args[1].arg in method_code.co_varnames
    assert method.body[0].names == [
        name
        for name in code_named(method_code, "__nested").co_names
        if name.endswith("__global")
    ]
    assert method.body[1].targets[0].id in method_code.co_cellvars
    assert method.body[2].targets[0].attr in method_code.co_names
    nested = method.body[3]
    assert nested.name in method_code.co_varnames
    assert nested.body[0].names[0] in code_named(method_code, "__nested").co_freevars
    assert (
        child.body[0].body[0].value.attr
        in code_named(code_named(owner_code, "__Child"), "method").co_names
    )
    assert underscores.body[0].args.args[1].arg == "__argument"
    assert python_definition_name(method) == "__method"
    assert python_definition_name(child) == "__Child"
    assert python_source_unparse(tree) == ast.unparse(original)
    assert python_ast_digest(tree) == python_ast_digest(original)
    resolve_python_private_names(tree)
    assert python_source_unparse(tree) == ast.unparse(original)


def test_private_imports_match_cpython_lookup_and_binding_names() -> None:
    source = """
class Owner:
    import __pkg.child
    import __mod as __alias
    from __pkg import __member as __renamed
"""
    code = code_named(compile(source, "<imports>", "exec"), "Owner")
    instructions = list(dis.get_instructions(code))
    tree = resolve_python_private_names(ast.parse(source))
    imports = tree.body[0].body
    lookups = [op.argval for op in instructions if op.opname == "IMPORT_NAME"]
    assert [
        imports[0].names[0].name,
        imports[1].names[0].name,
        imports[2].module,
    ] == lookups
    stores = [op.argval for op in instructions if op.opname == "STORE_NAME"]
    assert [python_import_binding(stmt.names[0]) for stmt in imports] == stores[-3:]
    assert imports[0].names[0].asname is None
    assert imports[2].names[0].name == next(
        op.argval for op in instructions if op.opname == "IMPORT_FROM"
    )
    assert python_source_unparse(tree) == ast.unparse(ast.parse(source))


def test_generic_class_only_mangles_type_parameter_scope_members() -> None:
    source = """
class Outer:
    @__decorate
    class __Inner[__T: __Bound](__Base[__T], metaclass=__Meta):
        type __Alias = __T
        def method(self):
            return __T, self.__attribute
"""
    original = ast.parse(source)
    tree = resolve_python_private_names(ast.parse(source))
    inner = tree.body[0].body[0]
    assert inner.decorator_list[0].id == "_Outer__decorate"
    assert inner.type_params[0].name == "_Inner__T"
    assert python_definition_name(inner.type_params[0]) == "__T"
    assert inner.type_params[0].bound.id == "__Bound"
    assert inner.bases[0].value.id == "__Base"
    assert inner.bases[0].slice.id == "_Inner__T"
    assert inner.keywords[0].value.id == "__Meta"
    assert inner.body[0].name.id == "_Inner__Alias"
    assert python_source_unparse(tree) == ast.unparse(original)


def test_argument_and_pattern_labels_and_literals_are_not_private_bindings() -> None:
    source = """
class Owner:
    __slots__ = ("__value",)
    def method(self):
        f(__keyword=self.__value)
        match self:
            case Target(__label=__captured):
                return __captured
"""
    tree = resolve_python_private_names(ast.parse(source))
    owner = tree.body[0]
    assert owner.body[0].value.elts[0].value == "__value"
    method = owner.body[1]
    assert method.body[0].value.keywords[0].arg == "__keyword"
    pattern = method.body[1].cases[0].pattern
    assert pattern.kwd_attrs == ["__label"]
    assert pattern.kwd_patterns[0].name == "_Owner__captured"


def test_cache_hit_still_resolves_each_callers_tree_and_preserves_edits() -> None:
    source = "class Owner:\n def method(self): return self.__value\n"
    first, second = ast.parse(source), ast.parse(source)
    digest = python_ast_digest(first)
    analyze_python_bindings(first, source_digest=digest)
    analyze_python_bindings(second, source_digest=digest)
    attribute = second.body[0].body[0].body[0].value
    assert attribute.attr == "_Owner__value"
    assert python_ast_digest(second) == digest
    attribute.attr = "replacement"
    assert python_ast_digest(second) != digest
    assert "replacement" in python_source_unparse(second)


def test_mutable_declaration_edits_and_analyzed_fragments_keep_identity() -> None:
    source = "class Owner:\n def method(self):\n  global __value\n  return __value\n"
    tree = resolve_python_private_names(ast.parse(source))
    method = tree.body[0].body[0]
    before = python_ast_digest(tree)
    method.body[0].names.append("added")
    assert python_ast_digest(tree) != before
    assert "added" in python_source_unparse(tree)
    fragment = ast.Module(body=[method], type_ignores=[])
    resolve_python_private_names(fragment)
    assert method.body[1].value.id == "_Owner__value"
    assert method.body[0].names == ["_Owner__value", "added"]


@pytest.mark.parametrize(
    "statement",
    [
        "def method(__value, _Owner__value): pass",
        "method = lambda __value, _Owner__value: None",
        "class Inner[__T, _Inner__T]: pass",
    ],
)
def test_resolved_parameter_collisions_match_cpython(statement: str) -> None:
    source = f"class Owner:\n {statement}\n"
    with pytest.raises(SyntaxError):
        compile(source, "<collision>", "exec")
    with pytest.raises(SyntaxError):
        resolve_python_private_names(ast.parse(source))


def test_earlier_type_parameter_bound_uses_complete_private_parameter_set() -> None:
    source = "class Owner[__T: Holder.__U, __U]: pass\n"
    tree = resolve_python_private_names(ast.parse(source))
    expected = compile(source, "<parameters>", "exec")
    pending = [expected]
    names = set()
    while pending:
        code = pending.pop()
        names.update(code.co_names)
        pending.extend(
            value for value in code.co_consts if isinstance(value, types.CodeType)
        )
    assert tree.body[0].type_params[0].bound.attr == "_Owner__U"
    assert tree.body[0].type_params[0].bound.attr in names


def test_lowering_preserves_private_callable_public_name_and_attribute_identity() -> (
    None
):
    result = compile_to_tir("""
class Owner:
    __slots__ = ("__value",)
    def __method(self, __argument):
        self.__value = __argument
        return self.__value
""")
    import json

    text = json.dumps(result)
    assert '"_Owner__value"' in text
    assert '"_Owner__argument"' in text
    assert '"__method"' in text
    assert '"Owner.__method"' in text
