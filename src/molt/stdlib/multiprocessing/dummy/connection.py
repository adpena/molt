"""Intrinsic-backed compatibility surface for `multiprocessing.dummy.connection`."""

from _intrinsics import require_intrinsic as _require_intrinsic
from multiprocessing._api_surface import apply_module_api_surface as _apply

_require_intrinsic("molt_capabilities_has", globals())
_apply(__name__, globals())
