"""Intrinsic-backed compatibility surface for `multiprocessing.popen_fork`."""

from multiprocessing._api_surface import apply_module_api_surface as _apply

_apply(__name__, globals())
