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

_hash_handles = {{}}
_hash_next_handle = [0]
_hmac_handles = {{}}
_hmac_next_handle = [0]


def _new_hash_handle(state):
    _hash_next_handle[0] += 1
    handle = _hash_next_handle[0]
    _hash_handles[handle] = state
    return handle


def _new_hmac_handle(state):
    _hmac_next_handle[0] += 1
    handle = _hmac_next_handle[0]
    _hmac_handles[handle] = state
    return handle


def _molt_hash_new(name, data=b"", options=None):
    options = dict(options or {{}})
    state = getattr(_host_hashlib, name)(data, **options)
    return _new_hash_handle(state)


def _molt_hash_update(handle, data):
    _hash_handles[handle].update(data)


def _molt_hash_copy(handle):
    return _new_hash_handle(_hash_handles[handle].copy())


def _molt_hash_digest(handle, length=None):
    state = _hash_handles[handle]
    if length is None:
        return state.digest()
    return state.digest(length)


def _molt_hash_drop(handle):
    _hash_handles.pop(handle, None)


def _molt_hmac_new(key, msg, digest_name, options):
    digest_name = str(digest_name)
    options = dict(options or {{}})
    digestmod = lambda data=b"": getattr(_host_hashlib, digest_name)(data, **options)
    return _new_hmac_handle(_host_hmac.new(key, msg, digestmod))


def _molt_hmac_update(handle, msg):
    _hmac_handles[handle].update(msg)


def _molt_hmac_copy(handle):
    return _new_hmac_handle(_hmac_handles[handle].copy())


def _molt_hmac_digest(handle):
    return _hmac_handles[handle].digest()


def _molt_hmac_drop(handle):
    _hmac_handles.pop(handle, None)


def _molt_scrypt(password, salt, n, r, p, maxmem=0, dklen=64):
    return _host_hashlib.scrypt(
        password,
        salt=salt,
        n=n,
        r=r,
        p=p,
        maxmem=maxmem,
        dklen=dklen,
    )


builtins._molt_intrinsics = {{
    "molt_capabilities_has": lambda _name=None: True,
    "molt_hash_new": _molt_hash_new,
    "molt_hash_update": _molt_hash_update,
    "molt_hash_copy": _molt_hash_copy,
    "molt_hash_digest": _molt_hash_digest,
    "molt_hash_drop": _molt_hash_drop,
    "molt_compare_digest": _host_hmac.compare_digest,
    "molt_pbkdf2_hmac": _host_hashlib.pbkdf2_hmac,
    "molt_scrypt": _molt_scrypt,
    "molt_hmac_new": _molt_hmac_new,
    "molt_hmac_update": _molt_hmac_update,
    "molt_hmac_copy": _molt_hmac_copy,
    "molt_hmac_digest": _molt_hmac_digest,
    "molt_hmac_drop": _molt_hmac_drop,
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


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_hashlib = _load_module("hashlib", {str(STDLIB_ROOT / "hashlib.py")!r})
_hmac = _load_module("hmac", {str(STDLIB_ROOT / "hmac.py")!r})
_hashlib_private = _load_module("molt_test__hashlib", {str(STDLIB_ROOT / "_hashlib.py")!r})

sha = _hashlib_private.new("sha256", b"molt")
hmac_obj = _hashlib_private.hmac_new(b"key", b"msg", "sha256")

checks = {{
    "classes": (
        _hashlib_private.HASH is _hashlib._Hash
        and issubclass(_hashlib_private.HASHXOF, _hashlib._Hash)
        and _hashlib_private.HMAC is _hmac.HMAC
    ),
    "functions": (
        len(sha.digest()) == 32
        and _hashlib_private.compare_digest(b"a", b"a") is True
        and isinstance(_hashlib_private.pbkdf2_hmac("sha256", b"pw", b"salt", 1), bytes)
        and isinstance(_hashlib_private.scrypt(b"pw", salt=b"salt", n=2**4, r=8, p=1), bytes)
        and isinstance(hmac_obj.digest(), bytes)
        and isinstance(_hashlib_private.hmac_digest(b"key", b"msg", "sha256"), bytes)
    ),
    "openssl_aliases": (
        _hashlib_private.openssl_sha256 is _hashlib.sha256
        and _hashlib_private.openssl_md5 is _hashlib.md5
        and "sha256" in _hashlib_private.openssl_md_meth_names
        and "blake2b" in _hashlib_private.openssl_md_meth_names
    ),
    "metadata": (
        _hashlib_private.UnsupportedDigestmodError is _hashlib.UnsupportedDigestmodError
        and _hashlib_private._GIL_MINSIZE == 2048
        and _hashlib_private.get_fips_mode() == 0
        and "sha256" in _hashlib_private._constructors
        and "blake2b" in _hashlib_private._constructors
    ),
    "public_module_private_handles_hidden": (
        "molt_hash_new" not in _hashlib.__dict__
        and "molt_hash_update" not in _hashlib.__dict__
        and "molt_hash_copy" not in _hashlib.__dict__
        and "molt_hash_digest" not in _hashlib.__dict__
        and "molt_hash_drop" not in _hashlib.__dict__
        and "molt_compare_digest" not in _hashlib.__dict__
        and "molt_pbkdf2_hmac" not in _hashlib.__dict__
        and "molt_scrypt" not in _hashlib.__dict__
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


def test_hashlib_private_module_surface() -> None:
    checks = _run_probe()
    assert checks == {
        "classes": "True",
        "functions": "True",
        "metadata": "True",
        "openssl_aliases": "True",
        "public_module_private_handles_hidden": "True",
    }
