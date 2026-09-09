"""Live class-namespace storage, lexical barriers, and versioned lookup facts."""

from __future__ import annotations

import ast
from pathlib import Path

import pytest

from molt.compiler_analysis.python_binding_flow import (
    PythonBindingPolicy,
    analyze_python_source_bindings,
)
from molt.frontend import compile_to_tir
from molt.frontend import SimpleTIRGenerator


def _probed_names(ops: list[dict]) -> list[str]:
    strings = {op["out"]: op["s_value"] for op in ops if op.get("kind") == "const_str"}
    return [
        strings[op["args"][1]]
        for op in ops
        if op.get("kind") == "call" and op.get("s_value") == "molt_namespace_get"
    ]


def test_comprehension_uses_class_mapping_only_for_outermost_iterable() -> None:
    ir = compile_to_tir(
        "outside = 'module'\n"
        "class Subject:\n"
        "    outside = 'class'\n"
        "    inputs = (1,)\n"
        "    for unused in (1,):\n"
        "        result = [outside for item in inputs if outside]\n"
    )
    main = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "molt_main")
    probes = _probed_names(main)
    assert "inputs" in probes
    assert "outside" not in probes
    assert "item" not in probes


def test_class_lambda_does_not_capture_namespace_ssa() -> None:
    ir = compile_to_tir(
        "outside = 'module'\n"
        "class Subject:\n"
        "    outside = 'class'\n"
        "    for unused in (1,):\n"
        "        callback = lambda: outside\n"
    )
    callbacks = [fn for fn in ir["functions"] if "lambda" in fn["name"]]
    assert callbacks
    for function in callbacks:
        assert not _probed_names(function["ops"])
        strings = {
            op["out"]: op["s_value"]
            for op in function["ops"]
            if op.get("kind") == "const_str"
        }
        assert any(
            op.get("kind") == "module_get_global"
            and strings.get(op["args"][1]) == "outside"
            for op in function["ops"]
        )


def test_prepared_namespace_probes_names_never_assigned_by_body() -> None:
    ir = compile_to_tir("class Subject(metaclass=Meta):\n    observed = injected\n")
    main = next(fn["ops"] for fn in ir["functions"] if fn["name"] == "molt_main")
    assert "injected" in _probed_names(main)


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
def test_class_annotation_lookup_fact_owns_current_vs_captured_parameter(target):
    source = "class Subject:\n    T = 123\n    def method[T](self, value: T): pass\n"
    tree = ast.parse(source)
    annotation = tree.body[0].body[1].args.args[1].annotation
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    fact = index.expression_fact(annotation)
    assert fact is not None
    assert fact.class_namespace_lookup is (target >= (3, 14))


def test_class_namespace_capsule_is_accepted_by_frontend() -> None:
    capsule = Path(__file__).parent / "differential/basic/class_namespace_lookup.py"
    ir = compile_to_tir(capsule.read_text(encoding="utf-8"))
    assert any(_probed_names(function["ops"]) for function in ir["functions"])


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
def test_class_global_annotation_lookup_is_not_a_captured_type_parameter(target):
    source = (
        "T = 123\nclass Subject:\n    global T\n    def method[T](value: T): pass\n"
    )
    tree = ast.parse(source)
    annotation = tree.body[1].body[1].args.args[0].annotation
    index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    fact = index.expression_fact(annotation)
    assert fact is not None
    assert not fact.class_namespace_lookup
    assert fact.name_lookup == ("global" if target >= (3, 14) else "lexical")


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    ("expression", "captures"),
    [
        ("value", []),
        ("lambda: value", ["value"]),
        ("lambda value: value", []),
        ("(value, lambda value: value)", []),
        ("(value, lambda: value)", ["value"]),
        ("lambda captured=value: captured", []),
    ],
)
def test_annotation_capture_does_not_retain_unused_class_global_fallback(
    target, expression, captures
):
    source = (
        "def outer():\n"
        "    value = object()\n"
        "    class Subject:\n"
        "        value = 'class'\n"
        f"        type Alias = {expression}\n"
    )
    tree = ast.parse(source)
    alias = tree.body[0].body[1].body[1]
    generator = SimpleTIRGenerator(target_python=target)
    generator.current_func_name = "outer"
    generator.scope_assigned = {"value"}
    generator.python_binding_index = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=target)
    )
    projection = generator._lexical_dependencies().project(
        (alias.value,), implicit_class_cell=True
    )
    assert sorted(projection.lexical) == captures
