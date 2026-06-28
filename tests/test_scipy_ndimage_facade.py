from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import types

import pytest

from molt import stdlib_intrinsic_policy


REPO_ROOT = Path(__file__).resolve().parents[1]
NDIMAGE_PATH = REPO_ROOT / "src" / "molt" / "stdlib" / "scipy" / "ndimage.py"


def _load_molt_ndimage(intrinsics):
    fake_intrinsics = types.ModuleType("_intrinsics")

    def require_intrinsic(name):
        return intrinsics[name]

    fake_intrinsics.require_intrinsic = require_intrinsic
    previous = sys.modules.get("_intrinsics")
    sys.modules["_intrinsics"] = fake_intrinsics
    spec = importlib.util.spec_from_file_location(
        "molt_scipy_ndimage_under_test", NDIMAGE_PATH
    )
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    try:
        spec.loader.exec_module(module)
    finally:
        if previous is None:
            sys.modules.pop("_intrinsics", None)
        else:
            sys.modules["_intrinsics"] = previous
    return module


def test_ndimage_module_is_intrinsic_backed() -> None:
    assert (
        stdlib_intrinsic_policy.stdlib_module_intrinsic_status(NDIMAGE_PATH)
        == stdlib_intrinsic_policy.STATUS_INTRINSIC
    )


def test_distance_transform_edt_wrapper_calls_intrinsic_after_api_gate() -> None:
    calls = []

    def intrinsic(value):
        calls.append(value)
        return [["distance"]]

    molt_ndimage = _load_molt_ndimage(
        {
            "molt_scipy_ndimage_distance_transform_edt": intrinsic,
            "molt_scipy_ndimage_gaussian_filter": lambda *_args: None,
            "molt_scipy_ndimage_maximum_filter": lambda *_args: None,
            "molt_scipy_ndimage_minimum_filter": lambda *_args: None,
            "molt_scipy_ndimage_label": lambda *_args: None,
        }
    )
    assert molt_ndimage.distance_transform_edt([[False, True]]) == [["distance"]]
    assert calls == [[[False, True]]]


def test_distance_transform_edt_wrapper_rejects_unsupported_options() -> None:
    molt_ndimage = _load_molt_ndimage(
        {
            "molt_scipy_ndimage_distance_transform_edt": lambda value: value,
            "molt_scipy_ndimage_gaussian_filter": lambda *_args: None,
            "molt_scipy_ndimage_maximum_filter": lambda *_args: None,
            "molt_scipy_ndimage_minimum_filter": lambda *_args: None,
            "molt_scipy_ndimage_label": lambda *_args: None,
        }
    )
    with pytest.raises(NotImplementedError, match="unit sampling"):
        molt_ndimage.distance_transform_edt([[False]], sampling=(1, 2))
    with pytest.raises(NotImplementedError, match="distances only"):
        molt_ndimage.distance_transform_edt([[False]], return_indices=True)
    with pytest.raises(NotImplementedError, match="output buffers"):
        molt_ndimage.distance_transform_edt([[False]], distances=[])


def test_pact_ndimage_wrappers_call_intrinsics_after_api_gates() -> None:
    calls = []

    def record(name):
        def intrinsic(*args):
            calls.append((name, args))
            return name

        return intrinsic

    molt_ndimage = _load_molt_ndimage(
        {
            "molt_scipy_ndimage_distance_transform_edt": record("edt"),
            "molt_scipy_ndimage_gaussian_filter": record("gaussian"),
            "molt_scipy_ndimage_maximum_filter": record("maximum"),
            "molt_scipy_ndimage_minimum_filter": record("minimum"),
            "molt_scipy_ndimage_label": record("label"),
        }
    )

    grid = [[0.0, 1.0], [2.0, 3.0]]
    mask = [[True, False], [False, True]]

    assert molt_ndimage.gaussian_filter(grid, 1.5) == "gaussian"
    assert molt_ndimage.maximum_filter(grid, size=3) == "maximum"
    assert molt_ndimage.minimum_filter(grid, size=(3, 3)) == "minimum"
    assert molt_ndimage.label(mask) == "label"
    assert calls == [
        ("gaussian", (grid, 1.5)),
        ("maximum", (grid, 3)),
        ("minimum", (grid, 3)),
        ("label", (mask,)),
    ]


def test_pact_ndimage_wrappers_reject_unsupported_options() -> None:
    molt_ndimage = _load_molt_ndimage(
        {
            "molt_scipy_ndimage_distance_transform_edt": lambda *_args: None,
            "molt_scipy_ndimage_gaussian_filter": lambda *_args: None,
            "molt_scipy_ndimage_maximum_filter": lambda *_args: None,
            "molt_scipy_ndimage_minimum_filter": lambda *_args: None,
            "molt_scipy_ndimage_label": lambda *_args: None,
        }
    )

    with pytest.raises(NotImplementedError, match="mode='reflect'"):
        molt_ndimage.gaussian_filter([[1]], 1.0, mode="nearest")
    with pytest.raises(NotImplementedError, match="scalar sigma"):
        molt_ndimage.gaussian_filter([[1]], (1.0, 2.0))
    with pytest.raises(NotImplementedError, match="positive odd size"):
        molt_ndimage.maximum_filter([[1]], size=2)
    with pytest.raises(NotImplementedError, match="square filters"):
        molt_ndimage.minimum_filter([[1]], size=(3, 5))
    with pytest.raises(NotImplementedError, match="structure"):
        molt_ndimage.label([[1]], structure=[[1, 1], [1, 1]])
