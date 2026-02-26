"""Intrinsic-backed `_csv` compatibility surface."""

from _intrinsics import require_intrinsic as _require_intrinsic
from csv import Dialect
from csv import Error
from csv import QUOTE_ALL
from csv import QUOTE_MINIMAL
from csv import QUOTE_NONE
from csv import QUOTE_NONNUMERIC
from csv import QUOTE_NOTNULL
from csv import QUOTE_STRINGS
from csv import field_size_limit
from csv import get_dialect
from csv import list_dialects
from csv import reader
from csv import register_dialect
from csv import unregister_dialect
from csv import writer

_MOLT_CSV_RUNTIME_READY = _require_intrinsic("molt_csv_runtime_ready", globals())
_MOLT_CSV_RUNTIME_READY()

__all__ = [
    "Dialect",
    "Error",
    "QUOTE_ALL",
    "QUOTE_MINIMAL",
    "QUOTE_NONE",
    "QUOTE_NONNUMERIC",
    "QUOTE_NOTNULL",
    "QUOTE_STRINGS",
    "field_size_limit",
    "get_dialect",
    "list_dialects",
    "reader",
    "register_dialect",
    "unregister_dialect",
    "writer",
]
