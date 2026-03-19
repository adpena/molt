"""Intrinsic-backed _bisect core."""

from _intrinsics import require_intrinsic as _require_intrinsic


bisect_left = _require_intrinsic("molt_bisect_left")
bisect_right = _require_intrinsic("molt_bisect_right")
insort_left = _require_intrinsic("molt_bisect_insort_left")
insort_right = _require_intrinsic("molt_bisect_insort_right")


globals().pop("_require_intrinsic", None)
