"""Low-level string helpers used by string.Formatter.

CPython exposes this as a C extension module that backs `string.Formatter`'s
parse and field-name-split paths. Molt forwards directly to the existing
runtime intrinsics so direct importers (mostly third-party formatters and
test scaffolding) get the same API without a parallel pure-Python parser.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

formatter_parser = _require_intrinsic("molt_string_formatter_parse")
formatter_field_name_split = _require_intrinsic(
    "molt_string_formatter_field_name_split"
)


__all__ = ["formatter_parser", "formatter_field_name_split"]


globals().pop("_require_intrinsic", None)
