"""Low-level import-machinery helpers used by `importlib`.

CPython exposes this as a built-in module that the public `importlib`
implementation uses for builtin / frozen / dynamic-extension hooks. Molt
uses the interpreter's runtime-owned import lock for finder callbacks;
the remaining capabilities below stay explicit.
"""

from __future__ import annotations


import sys
from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMP_ACQUIRE_LOCK = _require_intrinsic("molt_imp_acquire_lock")
_MOLT_IMP_RELEASE_LOCK = _require_intrinsic("molt_imp_release_lock")
_MOLT_IMP_LOCK_HELD = _require_intrinsic("molt_imp_lock_held")


def is_builtin(name):
    """Return 1 if name is a builtin module that cannot be imported as a
    package, -1 if it is a builtin extension that can be imported, 0
    otherwise. Molt has no separate builtin/extension distinction at
    runtime — modules are either compiled in or they're not."""
    return 1 if name in sys.builtin_module_names else 0


def is_frozen(name):
    """molt has no frozen modules in the compiled-binary contract."""
    return False


def is_frozen_package(name):
    """molt has no frozen modules in the compiled-binary contract."""
    return False


def get_frozen_object(name, data=None):
    """molt has no frozen modules in the compiled-binary contract."""
    raise ImportError("No frozen module named " + name)


def init_frozen(name):
    """molt has no frozen modules in the compiled-binary contract."""
    return None


def create_builtin(spec):
    """Builtin modules in molt are populated by the runtime at startup;
    no on-demand creation hook is required."""
    return sys.modules.get(spec.name)


def exec_builtin(module):
    """Builtin modules in molt are exec'd at runtime startup."""
    return 0


def create_dynamic(spec, file=None):
    """molt does not load C extensions at runtime."""
    raise ImportError("dynamic extension loading is not supported")


def exec_dynamic(module):
    """molt does not load C extensions at runtime."""
    return 0


def extension_suffixes():
    """No dynamic extensions, no suffixes."""
    return []


def lock_held():
    return _MOLT_IMP_LOCK_HELD()


def acquire_lock():
    return _MOLT_IMP_ACQUIRE_LOCK()


def release_lock():
    return _MOLT_IMP_RELEASE_LOCK()


def _fix_co_filename(co, fname):
    """No-op — molt does not have user-visible code objects."""
    return None


def source_hash(key, source):
    """Deterministic 64-bit hash matching CPython's pyc-source-hash shape.

    Uses a simple FNV-1a fold so the result is stable across runs and
    not dependent on Python's randomized hash().
    """
    if isinstance(source, str):
        data = source.encode("utf-8")
    else:
        data = bytes(source)
    h = 0xCBF29CE484222325 ^ int(key)
    prime = 0x100000001B3
    for byte in data:
        h ^= byte
        h = (h * prime) & 0xFFFFFFFFFFFFFFFF
    return h.to_bytes(8, "little", signed=False)


__all__ = [
    "is_builtin",
    "is_frozen",
    "is_frozen_package",
    "get_frozen_object",
    "init_frozen",
    "create_builtin",
    "exec_builtin",
    "create_dynamic",
    "exec_dynamic",
    "extension_suffixes",
    "lock_held",
    "acquire_lock",
    "release_lock",
    "_fix_co_filename",
    "source_hash",
]
