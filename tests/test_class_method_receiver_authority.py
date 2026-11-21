"""Unknown decorators cannot manufacture an implicit receiver type fact."""

import ast

import pytest

from molt.frontend import ClassId, DeclaredClass, ReceiverKind, SimpleTIRGenerator
from molt.frontend.visitors.class_method_compilation import ClassMethodCompilationMixin
from tools.check_ir_structure import verify_frontend_tir

SUBJECT = ClassId("__main__", "Subject")


@pytest.mark.parametrize(
    ("descriptor", "expected"),
    [
        ("function", DeclaredClass(SUBJECT, ReceiverKind.INSTANCE)),
        # A classmethod's receiver is the class object, never an instance.
        ("classmethod", DeclaredClass(SUBJECT, ReceiverKind.CLASS_OBJECT)),
        ("property", DeclaredClass(SUBJECT, ReceiverKind.INSTANCE)),
        ("property_update", DeclaredClass(SUBJECT, ReceiverKind.INSTANCE)),
        ("staticmethod", None),
        ("decorated", None),
    ],
)
def test_receiver_hint_requires_a_proved_binding_rule(descriptor, expected):
    authority = ClassMethodCompilationMixin()
    assert authority._class_method_receiver_hint(SUBJECT, descriptor, 0) == expected
    assert authority._class_method_receiver_hint(SUBJECT, descriptor, 1) is None


@pytest.mark.parametrize("decorator", ["staticmethod", "alias", "opaque"])
@pytest.mark.parametrize(
    "definition",
    [
        "def method(receiver):\n        return receiver",
        "def method(receiver):\n        yield receiver",
        "async def method(receiver):\n        return receiver",
        "async def method(receiver):\n        yield receiver",
    ],
)
def test_every_method_execution_kind_consumes_receiver_authority(
    monkeypatch, decorator, definition
):
    observed = []
    original = ClassMethodCompilationMixin._class_method_receiver_hint

    def recording_authority(self, class_id, descriptor, parameter_index):
        hint = original(self, class_id, descriptor, parameter_index)
        if class_id == SUBJECT and parameter_index == 0:
            observed.append((descriptor, hint))
        return hint

    monkeypatch.setattr(
        ClassMethodCompilationMixin, "_class_method_receiver_hint", recording_authority
    )
    generator = SimpleTIRGenerator(target_python=(3, 14))
    generator.visit(
        ast.parse(
            "alias = staticmethod\ndef opaque(function):\n    return function\n"
            "class Subject:\n"
            f"    @{decorator}\n    {definition}\n"
        )
    )
    verification = verify_frontend_tir(generator.to_json())
    assert verification.ok, verification.errors
    assert observed
    assert all(hint is None for _, hint in observed)
    # The binding proof resolves a module alias bound once to staticmethod
    # (spec 0216 excludes rebinding it from outside); an unproven decorator
    # stays generic.
    expected = "decorated" if decorator == "opaque" else "staticmethod"
    assert all(descriptor == expected for descriptor, _ in observed)


@pytest.mark.parametrize(
    ("source", "false_receiver"),
    [
        # `type.__new__` wraps `__init_subclass__` in a classmethod: the call
        # passes the class, so `f`'s first parameter is never an instance.
        (
            "class C:\n"
            "    def f(cls, **kwargs):\n        return cls\n"
            "    __init_subclass__ = f\n"
            "    def m(self):\n        return self\n"
            "C().__init_subclass__()\nC().m()\n",
            "f",
        ),
        # A property getter's returned function receives the call's own
        # argument (`5`), not the instance that selected the property.
        (
            "def g(arg):\n    return arg\n"
            "class C:\n"
            "    @property\n    def p(self):\n        return g\n"
            "    def m(self):\n        return self\n"
            "C().p(5)\nC().m()\n",
            "g",
        ),
    ],
    ids=["implicit-classmethod-alias", "property-result"],
)
def test_a_selection_that_does_not_pass_the_instance_binds_no_receiver(
    tmp_path, source, false_receiver
):
    # HF-956: an exact-receiver row is a proof that the first parameter holds
    # the instance; a false row compiles unguarded field loads on a class
    # object or an int.
    from molt.cli import module_graph_discovery, module_resolution, module_stdlib_policy
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    entry = tmp_path / "subject.py"
    entry.write_text(source, encoding="utf-8")
    stdlib = module_resolution._stdlib_root_path()
    result = module_graph_discovery._discover_module_graph(
        entry,
        [tmp_path, stdlib],
        [tmp_path],
        stdlib,
        tmp_path,
        module_stdlib_policy._stdlib_allowlist(),
    )
    assert result.discovery is not None
    # Exact receivers join the scan source's premise once the graph closes
    # (`module_graph_discovery`); lowering reads that premise.
    (premise,) = [
        authority.namespace_premise
        for authority in result.discovery.scan_sources.values()
        if authority.source_path.resolve() == entry.resolve()
    ]
    assert premise is not None
    defs = {
        node.name: python_node_source_key(node)
        for node in ast.walk(ast.parse(source))
        if isinstance(node, ast.FunctionDef)
    }
    rows = {function for function, _owner in premise.exact_receivers}
    assert defs["m"] in rows, "a plain method's instance receiver keeps its row"
    assert defs[false_receiver] not in rows
