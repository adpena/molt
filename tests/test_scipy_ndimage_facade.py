from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import types

import pytest

from molt import stdlib_intrinsic_policy


REPO_ROOT = Path(__file__).resolve().parents[1]
NDIMAGE_PATH = REPO_ROOT / "src" / "molt" / "stdlib" / "scipy" / "ndimage.py"


def _load_molt_ndimage(intrinsic):
    fake_intrinsics = types.ModuleType("_intrinsics")

    def require_intrinsic(name):
        assert name == "molt_scipy_ndimage_distance_transform_edt"
        return intrinsic

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

    molt_ndimage = _load_molt_ndimage(intrinsic)
    assert molt_ndimage.distance_transform_edt([[False, True]]) == [["distance"]]
    assert calls == [[[False, True]]]


def test_distance_transform_edt_wrapper_rejects_unsupported_options() -> None:
    molt_ndimage = _load_molt_ndimage(lambda value: value)
    with pytest.raises(NotImplementedError, match="unit sampling"):
        molt_ndimage.distance_transform_edt([[False]], sampling=(1, 2))
    with pytest.raises(NotImplementedError, match="distances only"):
        molt_ndimage.distance_transform_edt([[False]], return_indices=True)
    with pytest.raises(NotImplementedError, match="output buffers"):
        molt_ndimage.distance_transform_edt([[False]], distances=[])


def test_unimplemented_ndimage_siblings_fail_closed() -> None:
    molt_ndimage = _load_molt_ndimage(lambda value: value)
    for name in ("gaussian_filter", "maximum_filter", "minimum_filter", "label"):
        with pytest.raises(NotImplementedError, match=name):
            getattr(molt_ndimage, name)([[1]])
