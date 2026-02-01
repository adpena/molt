"""Validate build-mode sys stdio initialization.

Behavior: sys.stdin/stdout/stderr and their __*__ counterparts must be live file objects.
Why: compiled binaries must preserve CPython-like I/O surfaces for tooling and logging.
Pitfalls: capability gating should not null stdio; ensure this stays true in build mode.
"""

import sys


def _require_attr(name: str, obj: object, attr: str) -> None:
    assert obj is not None, f"{name} is None"
    assert hasattr(obj, attr), f"{name} missing {attr}"


_require_attr("sys.stdin", sys.stdin, "read")
_require_attr("sys.stdout", sys.stdout, "write")
_require_attr("sys.stderr", sys.stderr, "write")
_require_attr("sys.__stdin__", sys.__stdin__, "read")
_require_attr("sys.__stdout__", sys.__stdout__, "write")
_require_attr("sys.__stderr__", sys.__stderr__, "write")
