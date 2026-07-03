"""Cohesive decomposition of the proof_queue god-file.

Modules here hold the large proof_queue regions extracted to satisfy the
structural-audit kitchen-sink ceiling. The public entry point remains
tools/proof_queue.py, which imports and re-exports these symbols. The moved
regions reach shared helpers, constants, and monkeypatch-sensitive
primitives back through the facade via the ``pq`` proxy defined here.
"""

from __future__ import annotations


class _FacadeProxy:
    """Lazy handle on the tools.proof_queue facade module.

    Attribute access resolves against the live facade at call time, so the
    package modules are importable in any order (no import-time cycle) and
    test monkeypatches on tools.proof_queue.<name> stay authoritative for the
    moved regions.
    """

    __slots__ = ()

    def __getattr__(self, name):
        from tools import proof_queue as _facade

        return getattr(_facade, name)


pq = _FacadeProxy()
