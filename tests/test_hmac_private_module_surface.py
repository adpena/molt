from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import importlib.util
import sys
from tests.stdlib_intrinsic_registry import install_registry
from tests.crypto_intrinsic_registry import crypto_intrinsics

install_registry(crypto_intrinsics())


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_hashlib = _load_module("hashlib", {str(STDLIB_ROOT / "hashlib.py")!r})
_hmac = _load_module("hmac", {str(STDLIB_ROOT / "hmac.py")!r})
_load_module("_hashlib", {str(STDLIB_ROOT / "_hashlib.py")!r})
_hmac_private = _load_module("_hmac", {str(STDLIB_ROOT / "_hmac.py")!r})

h = _hmac_private.new(b"key", b"msg", "sha256")

checks = {{
    "types": (
        _hmac_private.HMAC is _hmac.HMAC
        and issubclass(_hmac_private.UnknownHashError, ValueError)
        and _hmac_private._GIL_MINSIZE == 2048
    ),
    "functions": (
        h.name == "hmac-sha256"
        and isinstance(h.digest(), bytes)
        and _hmac_private.compute_digest(b"key", b"msg", "sha256")
        == _hmac_private.compute_sha256(b"key", b"msg")
        and isinstance(_hmac_private.compute_md5(b"key", b"msg"), bytes)
        and isinstance(_hmac_private.compute_sha1(b"key", b"msg"), bytes)
        and isinstance(_hmac_private.compute_sha224(b"key", b"msg"), bytes)
        and isinstance(_hmac_private.compute_sha384(b"key", b"msg"), bytes)
        and isinstance(_hmac_private.compute_sha512(b"key", b"msg"), bytes)
        and isinstance(_hmac_private.compute_sha3_224(b"key", b"msg"), bytes)
        and isinstance(_hmac_private.compute_sha3_256(b"key", b"msg"), bytes)
        and isinstance(_hmac_private.compute_sha3_384(b"key", b"msg"), bytes)
        and isinstance(_hmac_private.compute_sha3_512(b"key", b"msg"), bytes)
        and len(_hmac_private.compute_blake2b_32(b"key", b"msg")) == 64
        and _hmac_private.compute_blake2b_32(b"key", b"msg")
        == _hmac_private.compute_digest(b"key", b"msg", "blake2b")
        and isinstance(_hmac_private.compute_blake2s_32(b"key", b"msg"), bytes)
    ),
    "errors": False,
}}

try:
    _hmac_private.new(b"key", b"msg", b"bad")
except _hmac_private.UnknownHashError as error:
    checks["errors"] = str(error) == "unsupported hash type: b'bad'"

class LowerWitness(str):
    def lower(self):
        raise marker

marker = RuntimeError("builtin name callback")
try:
    _hmac_private.new(object(), object(), LowerWitness("SHA3_256"))
except RuntimeError as error:
    checks["lower_before_buffers"] = error is marker
else:
    checks["lower_before_buffers"] = False
checks["builtin_metadata"] = (
    _hmac_private.new(b"key", digestmod="SHA3_256").name == "hmac-sha3_256"
    and _hmac_private.new(b"key", digestmod="BLAKE2B").digest_size == 64
)

original_unknown = _hmac_private.UnknownHashError
class ReplacementError(ValueError):
    pass
_hmac_private.UnknownHashError = ReplacementError
checks["captured_builtin_declaration"] = True
for operation in (
    lambda: _hmac_private.new(b"key", digestmod="unsupported"),
    lambda: _hmac_private.compute_digest(b"key", b"msg", "unsupported"),
):
    try:
        operation()
    except original_unknown as error:
        checks["captured_builtin_declaration"] &= str(error) == "unsupported hash type: 'unsupported'"
    else:
        checks["captured_builtin_declaration"] = False
_hmac_private.UnknownHashError = original_unknown

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> dict[str, str]:
    proc = run_surface_test_process(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, key, value = line.split("|", 2)
        assert prefix == "CHECK"
        checks[key] = value
    return checks


def test_hmac_private_module_surface() -> None:
    checks = _run_probe()
    assert checks == {
        "errors": "True",
        "lower_before_buffers": "True",
        "builtin_metadata": "True",
        "functions": "True",
        "types": "True",
        "captured_builtin_declaration": "True",
    }
