"""Shared, content-addressed tier for the per-module frontend caches.

The per-module *analysis* cache (function defaults / kinds / import scan facts)
and *lowering* cache (the frontend IR lowering result) are a *fixed* function of
the module source content plus the frontend/tooling identity that produced them.
Their correctness identity is already fully captured:

* Analysis entries key on ``_resolved_module_cache_key(path, module_name, pkg,
  import_scan_mode, kind, target_python.tag, _cache_tooling_fingerprint(),
  capability_config=...)`` and re-validate ``compiler_fingerprint``,
  ``import_scan_mode``, ``capability_config_digest`` and the real source content
  sha256 on read (:func:`_read_persisted_module_analysis`).
* Lowering entries re-validate a ``context_digest`` -- a sha256 over the full
  lowering context payload, which embeds ``_cache_tooling_fingerprint()``,
  ``target_python.tag``, the scoped known-modules / classes / func facts /
  native exports / type-facts / optimization profile / pgo -- *and* the real
  source content sha256 on read (:func:`_read_persisted_module_lowering`).

The per-session build-state root (``target/sessions/<MOLT_SESSION_ID>/.molt_state``)
is a *correctness isolation* boundary for concurrent agents, not a caching home.
A fresh session / worktree therefore starts with a cold ``.molt_state`` and
re-lowers *every* user module from scratch even though a byte-identical entry
was already produced by another session for the same source + tooling identity
-- the app-side analog of the cold runtime-wasm recompile the R73.1 shared cache
already fixed (:mod:`molt.cli.runtime_wasm_cache`).

This module is the single authority for a *shared* tier that lives *outside* any
per-session target dir, keyed only on the content-addressed identity, so a fresh
session can hydrate a warm entry instead of re-running the frontend:

* The cache roots are ``_default_molt_cache() / "module_analysis"`` and
  ``_default_molt_cache() / "module_lowering"`` -- the same session-independent,
  worktree-independent shared cache authority the backend and runtime-wasm caches
  use. They survive across sessions and worktrees.
* The shared slot name embeds the *full* correctness identity (the session
  filename key, plus -- for lowering, whose session filename key is deliberately
  coarse -- the ``context_digest``), so distinct build identities occupy distinct
  shared slots and can never clobber or be mis-served for one another.
* Publication and hydration use ``_atomic_copy_file`` (a byte copy), never
  ``os.link``, so they are safe on the exFAT APDataStore build volume which
  rejects hard links.
* Because the tier lives outside every cargo target dir, the memory-guard cargo
  incremental quarantine (which only relocates ``target/**`` dirs) can never
  destroy it.

Every read/write of the session-local caches routes publish/hydrate through this
authority (see :mod:`molt.cli.module_cache`); there is no separate shared lane.
"""

from __future__ import annotations

import os
import re
from pathlib import Path

from molt.cli.atomic_io import _atomic_copy_file
from molt.cli.default_paths import _default_molt_cache


# The content key is the 24-hex digest produced by ``_resolved_module_cache_key``;
# the lowering context digest is a full sha256 hexdigest. Both are validated so a
# stray file with an unexpected name can never be treated as a cache slot.
_CONTENT_KEY_RE = re.compile(r"\A[0-9a-f]{16,64}\Z")


_CACHE_DISABLE_ENV = "MOLT_DISABLE_FRONTEND_LOWERING_CACHE"


def _frontend_lowering_cache_disabled() -> bool:
    """True when the persistent frontend analysis/lowering cache is opt-disabled.

    Set ``MOLT_DISABLE_FRONTEND_LOWERING_CACHE=1`` (or ``true``/``yes``/``on``) to
    force every module to be re-analysed and re-lowered from source on each build:
    both the shared, content-addressed cross-session tier (publish + hydrate) and
    the session-local persisted reads become inert. This is the debugging lever for
    a suspected stale-cache miscompile -- it strips *all* frontend-result reuse so
    the build is a true cold re-lower.

    Correctness-safe by construction: a disabled cache only ever forces a cold
    recompute (read -> ``None`` / hydrate -> miss), it can never *serve* an entry,
    so toggling it can change build *time* but never build *output*. Read from the
    live environment (not memoized) so tests and orchestrated builds can flip it
    per-invocation.
    """
    return os.environ.get(_CACHE_DISABLE_ENV, "").strip().lower() in {
        "1",
        "true",
        "yes",
        "on",
    }


def _shared_module_analysis_cache_root() -> Path:
    """Session/worktree-independent shared cache root for module analysis."""
    return _default_molt_cache() / "module_analysis"


def _shared_module_lowering_cache_root() -> Path:
    """Session/worktree-independent shared cache root for module lowering."""
    return _default_molt_cache() / "module_lowering"


def _shared_module_analysis_cache_path(stem: str, content_key: str) -> Path | None:
    """Shared slot for one analysis identity, or ``None`` for a bad key.

    ``content_key`` is exactly the ``_resolved_module_cache_key(...)`` digest the
    session-local analysis filename is built from, so an identical source +
    tooling + target + scan-mode + capability identity resolves to the *same*
    shared slot regardless of session.
    """
    if _CONTENT_KEY_RE.match(content_key) is None:
        return None
    return _shared_module_analysis_cache_root() / f"{stem}.{content_key}.json"


def _shared_module_lowering_cache_path(
    stem: str,
    content_key: str,
    context_digest: str,
) -> Path | None:
    """Shared slot for one lowering identity, or ``None`` for a bad key.

    The lowering filename key (``content_key``) captures source bytes plus module,
    package, and target-Python identity. The slot name additionally embeds the
    ``context_digest``: two distinct lowering identities (e.g. a
    different tooling fingerprint, type-facts, or scoped inputs) then occupy
    distinct slots and can neither clobber nor be mis-served for one another.
    """
    if (
        _CONTENT_KEY_RE.match(content_key) is None
        or _CONTENT_KEY_RE.match(context_digest) is None
    ):
        return None
    return (
        _shared_module_lowering_cache_root()
        / f"{stem}.{content_key}.{context_digest}.json"
    )


def _hydrate_shared_frontend_cache(*, cache_path: Path | None, dest: Path) -> bool:
    """Copy a warm shared-tier entry into the session-local ``dest``.

    Returns ``True`` only when ``dest`` now holds the reused entry. Best-effort:
    any OS error (including a shared root that does not exist) is a miss, and the
    caller falls back to running the frontend. The copied entry is still validated
    by the caller's normal read path (schema / fingerprint / context digest /
    source content sha256), so hydration can never bypass correctness.
    """
    if _frontend_lowering_cache_disabled():
        return False
    if cache_path is None or not cache_path.is_file():
        return False
    try:
        _atomic_copy_file(cache_path, dest)
    except OSError:
        return False
    return dest.is_file()


def _publish_shared_frontend_cache(*, src: Path, cache_path: Path | None) -> None:
    """Publish a freshly written session-local entry into the shared tier.

    Best-effort: a failure to publish must never fail the build (the entry is
    already written to the session dir); it only means the next fresh session
    pays to re-run the frontend for this module. Uses a byte copy so it is safe
    on filesystems without hard-link support (the exFAT APDataStore volume).
    Idempotent: the slot is content-addressed, so re-publishing an identical
    identity simply rewrites byte-identical content.
    """
    if _frontend_lowering_cache_disabled():
        return
    if cache_path is None or not src.is_file():
        return
    try:
        _atomic_copy_file(src, cache_path)
    except OSError:
        return
