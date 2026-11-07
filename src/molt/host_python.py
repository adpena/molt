"""The CPython that may host Molt's compiler.

The ``molt`` process checks this before it loads anything else, so an
unsupported interpreter is named instead of failing on an unrelated import:
an old Python lacks ``tomllib``, ``typing.Never`` and ``datetime.UTC``. The
module imports only ``sys`` and must parse on Python 3.7 and newer. It never
joins a guest closure: ``molt/__init__.py`` does, the CLI entry does not.
"""

from __future__ import annotations

import sys

FRONTEND_PYTHON_REQUIREMENT = "CPython 3.12+ (Python 3.14 requires 3.14.1+)"


def frontend_python_supported(
    version: tuple[int, int, int], *, implementation: str
) -> bool:
    # CPython 3.14.0 emits PEP765 compiler warnings in AST-only parsing,
    # independently of the requested target version. 3.14.1 fixes that defect.
    # Reject the affected host rather than altering process-wide warning filters.
    return (
        implementation == "cpython" and version >= (3, 12, 0) and version != (3, 14, 0)
    )


def unsupported_frontend_python() -> str | None:
    """Why this interpreter cannot host the compiler, or None when it can."""
    version = tuple(sys.version_info[:3])
    implementation = sys.implementation.name
    if frontend_python_supported(version, implementation=implementation):
        return None
    found = ".".join(str(part) for part in version)
    return (
        f"Molt requires {FRONTEND_PYTHON_REQUIREMENT}, but this process runs "
        f"{implementation} {found} ({sys.executable or 'unknown executable'}). "
        "Run Molt through uv, which selects a supported CPython: "
        "`uv run molt ...` in a Molt checkout. An installed Molt "
        "must be installed into a supported CPython."
    )


def require_frontend_python() -> None:
    message = unsupported_frontend_python()
    if message is not None:
        raise RuntimeError(message)
