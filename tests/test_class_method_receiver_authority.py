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
