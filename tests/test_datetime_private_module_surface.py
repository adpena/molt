from __future__ import annotations

import importlib


def _public_rows(module_name: str) -> list[tuple[str, str, bool]]:
    module = importlib.import_module(module_name)
    return [
        (name, type(value).__name__, bool(callable(value)))
        for name, value in sorted(module.__dict__.items())
        if not name.startswith("_")
    ]


def test__datetime_public_surface_matches_expected_shape() -> None:
    assert _public_rows("_datetime") == [
        ("MAXYEAR", "int", False),
        ("MINYEAR", "int", False),
        ("UTC", "timezone", False),
        ("date", "type", True),
        ("datetime", "type", True),
        ("datetime_CAPI", "PyCapsule", False),
        ("time", "type", True),
        ("timedelta", "type", True),
        ("timezone", "type", True),
        ("tzinfo", "type", True),
    ]


def test__pydatetime_public_surface_matches_expected_shape() -> None:
    assert _public_rows("_pydatetime") == [
        ("MAXYEAR", "int", False),
        ("MINYEAR", "int", False),
        ("UTC", "timezone", False),
        ("date", "type", True),
        ("datetime", "type", True),
        ("sys", "module", False),
        ("time", "type", True),
        ("timedelta", "type", True),
        ("timezone", "type", True),
        ("tzinfo", "type", True),
    ]
