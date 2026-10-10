"""Intrinsic-backed compatibility surface for `multiprocessing.managers`."""

from multiprocessing._api_surface import apply_module_api_surface as _apply

_apply(__name__, globals())
