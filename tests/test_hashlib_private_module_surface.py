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
_hashlib_private = _load_module("_hashlib", {str(STDLIB_ROOT / "_hashlib.py")!r})
_load_module("_hmac", {str(STDLIB_ROOT / "_hmac.py")!r})

sha = _hashlib_private.new("sha256", b"molt")
hmac_obj = _hashlib_private.hmac_new(b"key", b"msg", "sha256")

def _error(action):
    try:
        action()
    except Exception as error:
        return type(error), str(error)
    raise AssertionError("unsupported spelling was admitted")

alias = _hashlib.new("SHA-512/224", b"molt")
alias_copy = alias.copy()
public_error = _error(lambda: _hashlib.new("sha512/224"))
private_error = _error(lambda: _hashlib_private.new("sha512/224"))
pbkdf_error = _error(lambda: _hashlib.pbkdf2_hmac("sha512/224", b"p", b"s", 1))

original_unsupported = _hashlib_private.UnsupportedDigestmodError
class ReplacementError(ValueError):
    pass
_hashlib.UnsupportedDigestmodError = ReplacementError
public_alias_only = _error(lambda: _hmac.new(b"k", b"v", "sha512/224"))
_hashlib_private.UnsupportedDigestmodError = ReplacementError
captured_new = _error(lambda: _hashlib_private.new("sha512/224"))
captured_pbkdf = _error(lambda: _hashlib.pbkdf2_hmac("sha512/224", b"p", b"s", 1))
captured_hmac = _error(lambda: _hashlib_private.hmac_new(b"k", b"v", "sha512/224"))
# The native class stays fixed, while the Python except clause deliberately
# follows the rebound private-module alias and therefore no longer catches it.
dynamic_hmac = _error(lambda: _hmac.new(b"k", b"v", "sha512/224"))
_hashlib.UnsupportedDigestmodError = original_unsupported
_hashlib_private.UnsupportedDigestmodError = original_unsupported

security_events = []
class SecurityWitness:
    def __bool__(self):
        security_events.append("security")
        raise RuntimeError("security marker")
bad_name = _error(lambda: _hashlib_private.new(123))
none_name = _error(lambda: _hashlib_private.new(None))
name_phase = _error(lambda: _hashlib_private.new(123, object(), usedforsecurity=SecurityWitness()))
modern = sys.version_info[:2] >= (3, 13)

public_events = []
class Name(str):
    def __hash__(self):
        public_events.append("hash")
        return str.__hash__(self)
    def __eq__(self, other):
        raise AssertionError("unexpected name equality")
public_keyword = _error(lambda: _hashlib.new(Name("sha256"), bogus=1))
public_hash_events = public_events[:]
original_blake = _hashlib.blake2b
_hashlib.blake2b = lambda *args, **kwargs: (_ for _ in ()).throw(AssertionError("rebound BLAKE"))
captured_blake = _hashlib.new("blake2b", b"abc", digest_size=17)
_hashlib.blake2b = original_blake

checks = {{
    "captured_declaration_and_dynamic_catch": (
        captured_new == (original_unsupported, "[digital envelope routines] unsupported")
        and captured_pbkdf == captured_new and captured_hmac == captured_new
        and dynamic_hmac == captured_new
        and public_alias_only == (ValueError, "unsupported hash type sha512/224")
    ),
    "private_name_conversion_phase": (
        bad_name == (TypeError, "new() argument 'name' must be str, not int" if modern else "name must be a string")
        and none_name == (TypeError, "new() argument 'name' must be str, not None" if modern else "name must be a string")
        and name_phase == (bad_name if modern else (RuntimeError, "security marker"))
        and security_events == ([] if modern else ["security"])
    ),
    "public_name_dispatch": (
        public_keyword == (TypeError, "'bogus' is an invalid keyword argument for new()")
        and public_hash_events == ["hash"]
        and captured_blake.digest_size == 17
        and captured_blake.hexdigest() == "658fb5b767c736b82b1a7fa7050454344c"
    ),
    "name_metadata_and_errors": (
        (alias.name, alias.digest_size, alias.block_size) == ("sha512_224", 28, 128)
        and (alias_copy.name, alias_copy.digest_size, alias_copy.block_size) == ("sha512_224", 28, 128)
        and alias.digest() == alias_copy.digest()
        and public_error == (ValueError, "unsupported hash type sha512/224")
        and private_error == (_hashlib_private.UnsupportedDigestmodError, "[digital envelope routines] unsupported")
        and pbkdf_error == private_error
        and _hashlib.blake2b(digest_size=17).digest_size == 17
    ),
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
        and _hashlib_private._constructors[_hashlib_private.openssl_sha256] == "sha256"
        and _hashlib.blake2b not in _hashlib_private._constructors
        and _hashlib.blake2s not in _hashlib_private._constructors
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


def test_hashlib_private_module_surface() -> None:
    checks = _run_probe()
    assert checks == {
        "classes": "True",
        "captured_declaration_and_dynamic_catch": "True",
        "private_name_conversion_phase": "True",
        "public_name_dispatch": "True",
        "functions": "True",
        "metadata": "True",
        "name_metadata_and_errors": "True",
        "openssl_aliases": "True",
        "public_module_private_handles_hidden": "True",
    }
