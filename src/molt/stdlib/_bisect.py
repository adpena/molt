"""Intrinsic-backed _bisect core."""

from _intrinsics import require_intrinsic as _require_intrinsic


bisect_left = _require_intrinsic("molt_bisect_left", globals())
bisect_right = _require_intrinsic("molt_bisect_right", globals())
insort_left = _require_intrinsic("molt_bisect_insort_left", globals())
insort_right = _require_intrinsic("molt_bisect_insort_right", globals())
