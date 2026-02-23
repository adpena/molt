"""Intrinsic-backed platform module for Molt."""

from __future__ import annotations

from collections import namedtuple

from _intrinsics import require_intrinsic as _require_intrinsic

_molt_platform_system = _require_intrinsic("molt_platform_system", globals())
_molt_platform_node = _require_intrinsic("molt_platform_node", globals())
_molt_platform_release = _require_intrinsic("molt_platform_release", globals())
_molt_platform_version = _require_intrinsic("molt_platform_version", globals())
_molt_platform_machine = _require_intrinsic("molt_platform_machine", globals())
_molt_platform_processor = _require_intrinsic("molt_platform_processor", globals())
_molt_platform_architecture = _require_intrinsic(
    "molt_platform_architecture", globals()
)
_molt_platform_python_version = _require_intrinsic(
    "molt_platform_python_version", globals()
)
_molt_platform_python_version_tuple = _require_intrinsic(
    "molt_platform_python_version_tuple", globals()
)
_molt_platform_python_implementation = _require_intrinsic(
    "molt_platform_python_implementation", globals()
)
_molt_platform_python_compiler = _require_intrinsic(
    "molt_platform_python_compiler", globals()
)
_molt_platform_platform = _require_intrinsic("molt_platform_platform", globals())
_molt_platform_uname = _require_intrinsic("molt_platform_uname", globals())

uname_result = namedtuple(
    "uname_result", ["system", "node", "release", "version", "machine", "processor"]
)


def system() -> str:
    return str(_molt_platform_system())


def node() -> str:
    return str(_molt_platform_node())


def release() -> str:
    return str(_molt_platform_release())


def version() -> str:
    return str(_molt_platform_version())


def machine() -> str:
    return str(_molt_platform_machine())


def processor() -> str:
    return str(_molt_platform_processor())


def architecture() -> tuple:
    result = _molt_platform_architecture()
    if isinstance(result, (list, tuple)) and len(result) == 2:
        return (str(result[0]), str(result[1]))
    return (str(result), "")


def python_version() -> str:
    return str(_molt_platform_python_version())


def python_version_tuple() -> tuple:
    result = _molt_platform_python_version_tuple()
    if isinstance(result, (list, tuple)) and len(result) == 3:
        return (str(result[0]), str(result[1]), str(result[2]))
    return tuple(str(x) for x in result)


def python_implementation() -> str:
    return str(_molt_platform_python_implementation())


def python_compiler() -> str:
    return str(_molt_platform_python_compiler())


def platform(aliased: bool = False, terse: bool = False) -> str:
    return str(_molt_platform_platform(aliased, terse))


def uname() -> uname_result:
    result = _molt_platform_uname()
    if isinstance(result, (list, tuple)) and len(result) >= 5:
        proc = str(result[5]) if len(result) > 5 else processor()
        return uname_result(
            str(result[0]),
            str(result[1]),
            str(result[2]),
            str(result[3]),
            str(result[4]),
            proc,
        )
    raise RuntimeError("platform.uname intrinsic returned unexpected value")
