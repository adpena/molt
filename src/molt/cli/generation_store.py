"""Retention of one directory of immutable generations.

A store directory holds generation entries that never change once published.
Every build that publishes or binds an entry marks it in the store ledger
under the store lock, and each publication evicts the entries that no build
used for the idle window. The selected entries are never evicted. Recency
lives in the ledger, not in file times: a member's stat identity includes its
mtime and ctime, so touching one would invalidate every holder's admission.

The runtime WASM pair and the backend compiler executables use this one
owner; each names its own entry pattern and removal operation.
"""

from __future__ import annotations

import json
import re
import time
from collections.abc import Callable, Generator
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path

from molt.cli.atomic_io import _atomic_write_json
from molt.file_locks import _acquire_file_lock, _release_file_lock


# A build finishes well inside this window, and a cache hydration or a rebuild
# restores an evicted generation on demand.
GENERATION_STORE_IDLE_SECONDS = 24 * 60 * 60
_LOCK_TIMEOUT_SECONDS = 300.0


class GenerationEvicted(FileNotFoundError):
    """A store entry a caller wants to use was removed (a cache miss)."""


@dataclass(frozen=True)
class GenerationStoreLayout:
    """What one kind of store names, and how it removes one idle entry."""

    label: str
    lock_name: str
    ledger_name: str
    ledger_schema: str
    entry: re.Pattern[str]
    remove: Callable[[Path], None]


@dataclass
class GenerationStore:
    """The usage ledger of one store directory, held under its lock."""

    layout: GenerationStoreLayout
    root: Path
    now: float
    selected: tuple[str, ...] = ()
    last_used: dict[str, float] = field(default_factory=dict)

    def use(self, *names: str) -> None:
        """Renew each entry's window, or miss when eviction removed it.

        Eviction runs under the same lock, so an entry present here stays for
        the whole idle window after this call: a caller that checked a path
        before taking the lock learns here whether it is still there.
        """
        missing = [name for name in names if not (self.root / name).exists()]
        if missing:
            raise GenerationEvicted(
                f"{self.layout.label} entries are gone: {', '.join(missing)}"
            )
        for name in names:
            self.last_used[name] = self.now

    def select(self, *names: str) -> None:
        # A reader may hold the selection this replaces; give it a full window.
        self.use(*(name for name in self.selected if (self.root / name).exists()))
        self.selected = names
        self.use(*self.selected)

    def evict_idle(self, idle_seconds: float) -> tuple[str, ...]:
        """Remove entries no build used for ``idle_seconds``.

        An entry the ledger does not know starts its window now, so the first
        pass over a directory written before its ledger evicts nothing. An
        entry that is open elsewhere (Windows refuses to delete it) stays
        until a later publication.
        """
        evicted: list[str] = []
        present: set[str] = set()
        for path in sorted(self.root.iterdir()):
            name = path.name
            if not self.layout.entry.match(name):
                continue
            present.add(name)
            last_used = self.last_used.setdefault(name, self.now)
            if name in self.selected or self.now - last_used <= idle_seconds:
                continue
            try:
                self.layout.remove(path)
            except OSError:
                continue
            present.discard(name)
            evicted.append(name)
        self.last_used = {
            name: used for name, used in self.last_used.items() if name in present
        }
        return tuple(evicted)

    def ledger(self) -> dict[str, object]:
        return {
            "schema": self.layout.ledger_schema,
            "selected": list(self.selected),
            "last_used": dict(sorted(self.last_used.items())),
        }


def _read_store_ledger(
    layout: GenerationStoreLayout, root: Path, now: float
) -> GenerationStore:
    """Load the ledger; an unreadable one restarts every entry's window."""

    store = GenerationStore(layout=layout, root=root, now=now)
    try:
        payload = json.loads((root / layout.ledger_name).read_bytes())
    except (OSError, ValueError):
        return store
    if not isinstance(payload, dict) or payload.get("schema") != layout.ledger_schema:
        return store
    selected = payload.get("selected")
    last_used = payload.get("last_used")
    if (
        isinstance(selected, list)
        and all(isinstance(name, str) for name in selected)
        and isinstance(last_used, dict)
        and all(
            isinstance(name, str)
            and isinstance(used, (int, float))
            and not isinstance(used, bool)
            for name, used in last_used.items()
        )
    ):
        store.selected = tuple(selected)
        store.last_used = {name: float(used) for name, used in last_used.items()}
    return store


def used_within(
    root: Path, layout: GenerationStoreLayout, name: str, seconds: float
) -> bool:
    """Whether the ledger records a use of ``name`` in the last ``seconds``.

    It reads the ledger without the store lock: publication replaces the
    ledger file whole, so a reader sees one complete version. A missing or
    unreadable ledger records no use.
    """
    now = time.time()
    used = _read_store_ledger(layout, root, now).last_used.get(name)
    return used is not None and now - used <= seconds


@contextmanager
def generation_store(
    root: Path, layout: GenerationStoreLayout
) -> Generator[GenerationStore]:
    """Hold one store directory's lock and persist its ledger on success."""

    lock_path = root / layout.lock_name
    handle = _acquire_file_lock(
        lock_path,
        timeout_s=_LOCK_TIMEOUT_SECONDS,
        timeout_message=(
            f"Timed out waiting for {layout.label} store lock {lock_path} after "
            f"{_LOCK_TIMEOUT_SECONDS:.0f}s."
        ),
    )
    try:
        store = _read_store_ledger(layout, root, time.time())
        yield store
        _atomic_write_json(root / layout.ledger_name, store.ledger(), sort_keys=True)
    finally:
        _release_file_lock(handle)
