"""Intrinsic-backed compatibility surface for `multiprocessing.popen_spawn_posix`."""

from _intrinsics import require_intrinsic as _require_intrinsic
from multiprocessing._api_surface import apply_module_api_surface as _apply

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")
_apply(__name__, globals())

globals().pop("_require_intrinsic", None)
