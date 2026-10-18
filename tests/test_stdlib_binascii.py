from __future__ import annotations

import binascii as py_binascii
import importlib.util
from pathlib import Path
import sys

from tests.stdlib_intrinsic_registry import intrinsic_registry


_SOURCE = (
    Path(__file__).resolve().parents[1] / "src" / "molt" / "stdlib" / "binascii.py"
)
_MODULE_NAME = "_molt_test_stdlib_binascii"


def _load_molt_binascii():
    spec = importlib.util.spec_from_file_location(_MODULE_NAME, _SOURCE)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[_MODULE_NAME] = module
    try:
        spec.loader.exec_module(module)
    except BaseException:
        sys.modules.pop(_MODULE_NAME, None)
        raise
    return module


def test_public_bindings_use_canonical_cpython_exceptions_and_intrinsics():
    registry = {
        "molt_binascii_error_type": lambda: py_binascii.Error,
        "molt_binascii_incomplete_type": lambda: py_binascii.Incomplete,
        "molt_binascii_a2b_base64": py_binascii.a2b_base64,
        "molt_binascii_b2a_base64": py_binascii.b2a_base64,
        "molt_binascii_a2b_hex": py_binascii.a2b_hex,
        "molt_binascii_b2a_hex": py_binascii.b2a_hex,
        "molt_binascii_a2b_qp": py_binascii.a2b_qp,
        "molt_binascii_b2a_qp": py_binascii.b2a_qp,
        "molt_binascii_a2b_uu": py_binascii.a2b_uu,
        "molt_binascii_b2a_uu": py_binascii.b2a_uu,
        "molt_binascii_crc32": py_binascii.crc32,
        "molt_binascii_crc_hqx": py_binascii.crc_hqx,
        "molt_binascii_hexlify": py_binascii.hexlify,
        "molt_binascii_unhexlify": py_binascii.unhexlify,
    }
    with intrinsic_registry(registry):
        module = _load_molt_binascii()

    try:
        assert module.Error is py_binascii.Error
        assert module.Incomplete is py_binascii.Incomplete
        assert module.Error.__bases__ == (ValueError,)
        assert module.Incomplete.__bases__ == (Exception,)
        assert issubclass(module.Error, ValueError)
        assert issubclass(module.Incomplete, Exception)
        assert not issubclass(module.Incomplete, module.Error)

        assert set(module.__all__) == {
            "Error",
            "Incomplete",
            "a2b_base64",
            "a2b_hex",
            "a2b_qp",
            "a2b_uu",
            "b2a_base64",
            "b2a_hex",
            "b2a_qp",
            "b2a_uu",
            "crc32",
            "crc_hqx",
            "hexlify",
            "unhexlify",
        }
        assert all(name in vars(module) for name in module.__all__)
        for name in (
            "a2b_base64",
            "a2b_hex",
            "a2b_qp",
            "a2b_uu",
            "b2a_base64",
            "b2a_hex",
            "b2a_qp",
            "b2a_uu",
            "crc_hqx",
            "hexlify",
            "unhexlify",
        ):
            assert getattr(module, name) is getattr(py_binascii, name)
        assert module.crc32(b"abc") == py_binascii.crc32(b"abc")
        assert "_require_intrinsic" not in vars(module)
        assert not any(name.startswith("molt_binascii_") for name in vars(module))
    finally:
        sys.modules.pop(_MODULE_NAME, None)
