"""Canonical Python module-name custody for manifests and generated imports."""

from __future__ import annotations

import keyword
import unicodedata
from collections.abc import Sequence


def canonical_python_module_name(value: object, *, field: str) -> str:
    """Validate one source-spellable, normalization-stable dotted module name."""

    if not isinstance(value, str) or not value or value != value.strip():
        raise ValueError(f"{field} must be a canonical non-empty Python module name")
    for component in value.split("."):
        if (
            not component.isidentifier()
            or keyword.iskeyword(component)
            or unicodedata.normalize("NFKC", component) != component
        ):
            raise ValueError(f"{field} contains invalid Python module name {value!r}")
    return value


def canonical_python_module_names(value: object, *, field: str) -> tuple[str, ...]:
    """Decode an exact sorted, duplicate-free module-name manifest list."""

    if not isinstance(value, list):
        raise ValueError(f"{field} must be a list of Python module-name strings")
    decoded = tuple(
        canonical_python_module_name(item, field=f"{field}[{index}]")
        for index, item in enumerate(value)
    )
    if decoded != tuple(sorted(set(decoded))):
        raise ValueError(f"{field} must be sorted and duplicate-free")
    return decoded


def encode_python_module_names(values: Sequence[str], *, field: str) -> list[str]:
    """Validate unordered producer facts and emit their canonical list form."""

    if isinstance(values, (str, bytes)):
        raise ValueError(f"{field} must be a sequence of Python module-name strings")
    return sorted(
        {canonical_python_module_name(value, field=field) for value in values}
    )
