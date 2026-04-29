"""Low-level zoneinfo helpers used by `zoneinfo`.

CPython exposes this as a C extension module that the public `zoneinfo`
package imports `ZoneInfo` and `ZoneInfoNotFoundError` from. Molt's
`zoneinfo` package already implements both against runtime intrinsics
(`molt_zoneinfo_*`), so `_zoneinfo` re-exports them so any third-party
code that imports `_zoneinfo` directly gets the working implementation.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()
del _MOLT_IMPORT_SMOKE_RUNTIME_READY


from zoneinfo import ZoneInfo, ZoneInfoNotFoundError, available_timezones


__all__ = ["ZoneInfo", "ZoneInfoNotFoundError", "available_timezones"]


globals().pop("_require_intrinsic", None)
