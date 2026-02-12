"""Compatibility shim for Molt DB helpers.

Canonical location is `moltlib.molt_db`.
"""

from __future__ import annotations

from moltlib.molt_db import DbResponse, db_exec, db_query

__all__ = ["DbResponse", "db_exec", "db_query"]
