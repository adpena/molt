"""Minimal Django-style usage of molt_offload (demo-only)."""

from __future__ import annotations

from molt_accel import molt_offload


@molt_offload(entry="list_items", codec="msgpack", timeout_ms=250)
def items_view(request):
    raise RuntimeError("This handler should be offloaded.")
