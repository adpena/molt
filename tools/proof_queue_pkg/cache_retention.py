"""Development cache retention inside the existing guarded proof controller."""

from __future__ import annotations

from collections.abc import Mapping

from molt.cache_eviction import _prune, resolve_budget
from molt.default_paths import _default_molt_cache


def prune_after_payload(environ: Mapping[str, str]) -> dict[str, object] | None:
    """Sweep once after closed payload custody, within its enclosing guard.

    This starts no process and owns no queue row. The canonical prune lock owns
    cross-checkout coalescing. Ordinary retention errors are diagnostic; memory,
    cancellation and the outer guard's deadline remain infrastructure failures.
    """
    if environ.get("MOLT_CACHE_PRUNE", "1") == "0":
        return None
    try:
        root = _default_molt_cache(environ=environ).resolve(strict=False)
        if not root.is_dir():
            return None
        max_gb, max_age = resolve_budget(None, None, environ)
        report = _prune(
            root,
            max_bytes=int(max_gb * 1024**3),
            max_age_days=max_age,
            dry_run=False,
            automatic=True,
            environ=environ,
        )
        # The execution receipt keeps bounded diagnostics, not a second cache
        # inventory; the canonical pruner still owns all protected-state rules.
        return {
            name: value
            for name, value in report.items()
            if name not in {"protected_state", "legacy_native_state"}
        }
    except (OSError, ValueError, RuntimeError) as exc:
        return {"status": "failed", "error": f"{type(exc).__name__}: {exc}"}
