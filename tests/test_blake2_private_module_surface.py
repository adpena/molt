from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import hmac as _host_hmac
import hashlib as _host_hashlib
import importlib.util
import sys
import types

_handles = {{}}
_next_handle = [0]


def _new_handle(state):
    _next_handle[0] += 1
    handle = _next_handle[0]
    _handles[handle] = state
    return handle


def _molt_hash_new(name, data=b"", options=None):
    options = dict(options or {{}})
    ctor = getattr(_host_hashlib, name)
    state = ctor(data, **options)
    return _new_handle(state)


def _molt_hash_update(handle, data):
    _handles[handle].update(data)


def _molt_hash_copy(handle):
    return _new_handle(_handles[handle].copy())


def _molt_hash_digest(handle, length=None):
    state = _handles[handle]
    if length is None:
        return state.digest()
    return state.digest(length)


def _molt_hash_drop(handle):
    _handles.pop(handle, None)


builtins._molt_intrinsics = {{
    "molt_capabilities_has": lambda _name=None: True,
    "molt_hash_new": _molt_hash_new,
    "molt_hash_update": _molt_hash_update,
    "molt_hash_copy": _molt_hash_copy,
    "molt_hash_digest": _molt_hash_digest,
    "molt_hash_drop": _molt_hash_drop,
    "molt_compare_digest": _host_hmac.compare_digest,
    "molt_pbkdf2_hmac": _host_hashlib.pbkdf2_hmac,
    "molt_scrypt": _host_hashlib.scrypt,
}}

_intrinsics_mod = types.ModuleType("_intrinsics")


def _require_intrinsic(name, namespace=None):
    intrinsics = getattr(builtins, "_molt_intrinsics", {{}})
    if name in intrinsics:
        value = intrinsics[name]
        if namespace is not None:
            namespace[name] = value
        return value
    raise RuntimeError(f"intrinsic unavailable: {{name}}")


_intrinsics_mod.require_intrinsic = _require_intrinsic
sys.modules["_intrinsics"] = _intrinsics_mod

_hashlib_spec = importlib.util.spec_from_file_location(
    "hashlib", {str(STDLIB_ROOT / "hashlib.py")!r}
)
assert _hashlib_spec is not None and _hashlib_spec.loader is not None
_hashlib = importlib.util.module_from_spec(_hashlib_spec)
sys.modules["hashlib"] = _hashlib
_hashlib_spec.loader.exec_module(_hashlib)

_blake2_spec = importlib.util.spec_from_file_location(
    "molt_test__blake2", {str(STDLIB_ROOT / "_blake2.py")!r}
)
assert _blake2_spec is not None and _blake2_spec.loader is not None
_blake2 = importlib.util.module_from_spec(_blake2_spec)
sys.modules["molt_test__blake2"] = _blake2
_blake2_spec.loader.exec_module(_blake2)

b2b = _blake2.blake2b(b"molt", digest_size=16)
b2s = _blake2.blake2s(b"molt", digest_size=16)

checks = {{
    "types": isinstance(_blake2.blake2b, type) and isinstance(_blake2.blake2s, type),
    "base_type": issubclass(_blake2.blake2b, _hashlib._Hash) and issubclass(_blake2.blake2s, _hashlib._Hash),
    "constants": (
        _blake2.BLAKE2B_MAX_DIGEST_SIZE == 64
        and _blake2.BLAKE2B_MAX_KEY_SIZE == 64
        and _blake2.BLAKE2B_SALT_SIZE == 16
        and _blake2.BLAKE2B_PERSON_SIZE == 16
        and _blake2.BLAKE2S_MAX_DIGEST_SIZE == 32
        and _blake2.BLAKE2S_MAX_KEY_SIZE == 32
        and _blake2.BLAKE2S_SALT_SIZE == 8
        and _blake2.BLAKE2S_PERSON_SIZE == 8
        and _blake2._GIL_MINSIZE == 2048
    ),
    "runtime": (
        b2b.name == "blake2b"
        and b2b.digest_size == 16
        and len(b2b.digest()) == 16
        and b2s.name == "blake2s"
        and b2s.digest_size == 16
        and len(b2s.digest()) == 16
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> dict[str, str]:
    proc = subprocess.run(
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


def test_blake2_private_module_surface() -> None:
    checks = _run_probe()
    assert checks == {
        "base_type": "True",
        "constants": "True",
        "runtime": "True",
        "types": "True",
    }
