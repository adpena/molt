"""A helper for the sysconfig module."""

# CPython's _sysconfig (Modules/_sysconfig.c, new in 3.13) reports build
# variables its C compiler knew. A Molt binary's build variables have one
# derivation, sysconfig._get_sysconfigdata(); this module selects the keys
# CPython's _sysconfig reports for the target version and platform.

import sys as _sys

if _sys.version_info < (3, 13):
    raise ModuleNotFoundError("No module named '_sysconfig'", name="_sysconfig")

# EXT_SUFFIX and SOABI appear only where CPython's module reports them (Windows)
# and only when the binary has them. Py_DEBUG joined in 3.14.
_CONFIG_NAMES = (
    *(("EXT_SUFFIX", "SOABI") if _sys.platform == "win32" else ()),
    "Py_GIL_DISABLED",
    *(("Py_DEBUG",) if _sys.version_info >= (3, 14) else ()),
)


def config_vars():
    """Returns a dictionary containing build variables intended to be exposed by sysconfig."""
    from sysconfig import _get_sysconfigdata

    data = _get_sysconfigdata()
    return {name: data[name] for name in _CONFIG_NAMES if name in data}


if _sys.version_info >= (3, 15) and _sys.platform == "win32":

    def get_platform():
        """Return a string that identifies the compiler platform."""
        from sysconfig import _windows_platform

        return _windows_platform()
