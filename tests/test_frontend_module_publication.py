from __future__ import annotations

from collections import UserDict
from types import MappingProxyType

import pytest

from molt.frontend.module_publication import (
    consume_source_module_publication,
    inspect_source_module_publication,
    parse_source_module_publication,
)


def _publication() -> dict[str, object]:
    return {
        "module_name": "unit",
        "module_value": "module",
        "failure_label": 7,
    }


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("module_name", ""),
        ("module_name", None),
        ("module_name", 1),
        ("module_value", ""),
        ("module_value", None),
        ("module_value", 1),
        ("failure_label", None),
        ("failure_label", "7"),
        ("failure_label", True),
        ("failure_label", False),
    ],
)
def test_publication_fields_are_validated_before_typed_projection(
    field: str, value: object
) -> None:
    payload = _publication()
    payload[field] = value
    with pytest.raises(
        ValueError, match="canonical source-module publication metadata"
    ):
        parse_source_module_publication(payload)


def test_publication_accepts_mapping_protocol_without_coercing_fields() -> None:
    payload = _publication()
    assert parse_source_module_publication(UserDict(payload)) == payload
    assert parse_source_module_publication(MappingProxyType(payload)) == payload
    with pytest.raises(
        ValueError, match="canonical source-module publication metadata"
    ):
        parse_source_module_publication({**payload, 0: "extra"})


def test_publication_consumption_preserves_unrelated_operation_metadata() -> None:
    boundary: UserDict[object, object] = UserDict(
        {
            "kind": "frame_locals_set",
            "source_module_publication_boundary": True,
            0: "opaque metadata",
        }
    )
    function: dict[str, object] = {
        "name": "molt_init_unit",
        "source_module_publication": _publication(),
        "ops": [boundary],
    }
    expected = inspect_source_module_publication(function)
    assert consume_source_module_publication(function) == expected
    assert "source_module_publication" not in function
    assert boundary == {"kind": "frame_locals_set", 0: "opaque metadata"}


def test_immutable_boundary_rejection_does_not_consume_publication() -> None:
    boundary = MappingProxyType(
        {"kind": "frame_locals_set", "source_module_publication_boundary": True}
    )
    function: dict[str, object] = {
        "source_module_publication": _publication(),
        "ops": [boundary],
    }
    with pytest.raises(TypeError, match="publication boundary must be mutable"):
        consume_source_module_publication(function)
    assert function["source_module_publication"] == _publication()
    assert boundary["source_module_publication_boundary"] is True
