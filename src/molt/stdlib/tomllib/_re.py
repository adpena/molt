"""tomllib._re — compiled regex patterns for Molt's TOML parser.

All parsing in Molt's tomllib is done via the recursive-descent _Parser in
``tomllib/__init__.py``.  This module exists only for API compatibility with
code that does ``from tomllib._re import ...``.  It is not used internally.
"""

from __future__ import annotations

__all__: list[str] = []
