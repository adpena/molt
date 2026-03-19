"""Public API surface shim for ``ctypes._aix``."""

from __future__ import annotations

import os as _os
import re as _re
import subprocess as _subprocess
import sys as _sys
import types as _types

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")


class PyCSimpleType(type):
    pass


class c_void_p(metaclass=PyCSimpleType):
    pass


_MAXSIZE = int(getattr(_sys, "maxsize", (1 << 63) - 1))

AIX_ABI = 64 if _MAXSIZE > (1 << 32) else 32
DEVNULL = int(getattr(_subprocess, "DEVNULL", -1))
PIPE = int(getattr(_subprocess, "PIPE", -1))
Popen = _subprocess.Popen
environ = _os.environ
executable = str(getattr(_sys, "executable", ""))
maxsize = _MAXSIZE
path = _types.ModuleType("path")
re = _re
sizeof = len


def get_version(_name: str) -> str:
    return ""


def get_member(_archive: str, _member: str) -> str | None:
    return None


def get_one_match(_pattern: str, _text: str = "") -> str | None:
    return None


def get_shared(_name: str) -> list[str]:
    return []


def find_shared(_name: str) -> str | None:
    return None


def get_legacy(_name: str) -> str | None:
    return None


def get_ld_header(_name: str) -> bytes:
    return b""


def get_ld_headers(_name: str) -> list[bytes]:
    return []


def get_ld_header_info(_name: str) -> dict[str, object]:
    return {}


def get_libpaths() -> list[str]:
    return []


def find_library(_name: str) -> str | None:
    return None


del PyCSimpleType

globals().pop("_require_intrinsic", None)
