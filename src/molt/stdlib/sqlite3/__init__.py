"""DB-API 2.0 SQLite database driver for Molt.

This package mirrors CPython's :mod:`sqlite3`: the user-facing module
re-exports everything from :mod:`sqlite3.dbapi2`, which in turn
re-exports the low-level driver from :mod:`_sqlite3`.

Quick start::

    import sqlite3
    conn = sqlite3.connect(":memory:")
    cur = conn.cursor()
    cur.execute("CREATE TABLE t (a INTEGER, b TEXT)")
    cur.executemany("INSERT INTO t VALUES (?, ?)", [(1, "x"), (2, "y")])
    conn.commit()
    cur.execute("SELECT a, b FROM t ORDER BY a")
    print(cur.fetchall())  # [(1, "x"), (2, "y")]
    conn.close()
"""

# fmt: off
# pylint: disable=all
# ruff: noqa

from __future__ import annotations

# Keep the module inside the intrinsic-backed stdlib gate.  The actual
# driver intrinsics are required by `_sqlite3` (re-exported via `dbapi2`).
from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()
del _MOLT_IMPORT_SMOKE_RUNTIME_READY

_require_intrinsic("molt_stdlib_probe")
del _require_intrinsic

from sqlite3.dbapi2 import *  # noqa: E402,F401,F403
from sqlite3.dbapi2 import (
    _deprecated_names,
    _deprecated_version,
    _deprecated_version_info,
)


def __getattr__(name):
    """Mimic CPython 3.12's deprecation warning for ``version`` / ``version_info``.

    Direct attribute access on these names emits a ``DeprecationWarning``
    pointing at the call site.  The non-deprecated values still come
    through the normal ``from sqlite3.dbapi2 import *`` path.
    """
    if name in _deprecated_names:
        from warnings import warn

        warn(
            f"{name} is deprecated and will be removed in Python 3.14",
            DeprecationWarning,
            stacklevel=2,
        )
        return globals()[f"_deprecated_{name}"]
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
