"""Unknown decorators cannot manufacture an implicit receiver type fact."""

import ast

import pytest

from molt.frontend import SimpleTIRGenerator
from molt.frontend.visitors.class_method_compilation import ClassMethodCompilationMixin
from tools.check_ir_structure import verify_frontend_tir


@pytest.mark.parametrize(
    ("descriptor", "expected"),
    [
        ("function", "Subject"),
        ("classmethod", "Subject"),
        ("property", "Subject"),
        ("property_update", "Subject"),
        ("staticmethod", None),
        ("decorated", None),
    ],
)
def test_receiver_hint_requires_a_proved_binding_rule(descriptor, expected):
    authority = ClassMethodCompilationMixin()
    assert authority._class_method_receiver_hint("Subject", descriptor, 0) == expected
    assert authority._class_method_receiver_hint("Subject", descriptor, 1) is None


@pytest.mark.parametrize("decorator", ["staticmethod", "alias"])
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

    def recording_authority(self, class_name, descriptor, parameter_index):
        hint = original(self, class_name, descriptor, parameter_index)
        if class_name == "Subject" and parameter_index == 0:
            observed.append((descriptor, hint))
        return hint

    monkeypatch.setattr(
        ClassMethodCompilationMixin, "_class_method_receiver_hint", recording_authority
    )
    generator = SimpleTIRGenerator(target_python=(3, 14))
    generator.visit(
        ast.parse(
            "alias = staticmethod\nclass Subject:\n"
            f"    @{decorator}\n    {definition}\n"
        )
    )
    verification = verify_frontend_tir(generator.to_json())
    assert verification.ok, verification.errors
    assert observed
    assert all(hint is None for _, hint in observed)
    if decorator == "alias":
        assert all(descriptor == "decorated" for descriptor, _ in observed)
