"""Low-level zoneinfo helpers used by `zoneinfo`.

CPython exposes this as a C extension module that the public `zoneinfo`
package imports `ZoneInfo` and `ZoneInfoNotFoundError` from. Molt's
`zoneinfo` package already implements both against runtime intrinsics
(`molt_zoneinfo_*`), so `_zoneinfo` re-exports them so any third-party
code that imports `_zoneinfo` directly gets the working implementation.
"""

from __future__ import annotations

from zoneinfo import ZoneInfo, ZoneInfoNotFoundError, available_timezones


__all__ = ["ZoneInfo", "ZoneInfoNotFoundError", "available_timezones"]
