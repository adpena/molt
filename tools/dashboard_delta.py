#!/usr/bin/env python3
"""Dashboard delta protocol for Molt correctness/perf dashboards (MOL-214).

Instead of sending full state snapshots on every update, this module tracks
state changes incrementally and emits compact diffs.  Clients that
reconnect receive a full-state catchup followed by subsequent deltas.

Protocol overview:
  - Each state mutation produces a ``DeltaMessage`` with a monotonic sequence
    number, a change type, and the diff payload.
  - Connected clients track their ``last_seen_seq``.
  - On reconnect, the server replays the full snapshot (seq=0) plus any deltas
    the client missed.

Usage (library):
    from tools.dashboard_delta import DeltaTracker

    tracker = DeltaTracker()
    tracker.update("lean_sorry_count", 42)
    tracker.update("test_pass_rate", 0.97)

    # New client connects:
    catchup = tracker.catchup(last_seen_seq=0)
    # Returns [DeltaMessage(seq=1, ...), DeltaMessage(seq=2, ...)]

    # Existing client polls:
    new_deltas = tracker.since(last_seen_seq=1)

Usage (CLI, for debugging):
    uv run --python 3.12 python3 tools/dashboard_delta.py --demo
"""

from __future__ import annotations

import argparse
import json
import sys
import threading
import time
from dataclasses import asdict, dataclass, field
from typing import Any


# ---------------------------------------------------------------------------
# Delta message types
# ---------------------------------------------------------------------------

@dataclass(frozen=True)
class DeltaMessage:
    """A single state change in the delta stream."""

    seq: int
    timestamp: float
    change_type: str  # "set", "delete", "batch"
    key: str
    value: Any = None
    previous: Any = None

    def to_dict(self) -> dict[str, Any]:
        d = asdict(self)
        # Omit None fields for compactness.
        return {k: v for k, v in d.items() if v is not None}

    def to_json(self) -> str:
        return json.dumps(self.to_dict(), separators=(",", ":"))


@dataclass(frozen=True)
class FullSnapshot:
    """Full state snapshot sent on client reconnect."""

    seq: int
    timestamp: float
    state: dict[str, Any]

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)

    def to_json(self) -> str:
        return json.dumps(self.to_dict(), separators=(",", ":"))


# ---------------------------------------------------------------------------
# Core delta tracker
# ---------------------------------------------------------------------------

class DeltaTracker:
    """Thread-safe incremental state tracker with delta protocol support.

    Parameters
    ----------
    max_history : int
        Maximum number of delta messages to retain.  Older messages are
        evicted, and clients that fall behind will receive a full snapshot
        instead of incremental replay.
    """

    def __init__(self, *, max_history: int = 10_000) -> None:
        self._lock = threading.Lock()
        self._state: dict[str, Any] = {}
        self._deltas: list[DeltaMessage] = []
        self._seq: int = 0
        self._max_history = max_history

    # -- Mutations ----------------------------------------------------------

    def update(self, key: str, value: Any) -> DeltaMessage:
        """Set *key* to *value*, recording a delta."""
        with self._lock:
            previous = self._state.get(key)
            if previous == value:
                # No actual change; still return a no-op message for idempotence.
                return DeltaMessage(
                    seq=self._seq,
                    timestamp=time.time(),
                    change_type="noop",
                    key=key,
                    value=value,
                )
            self._seq += 1
            self._state[key] = value
            msg = DeltaMessage(
                seq=self._seq,
                timestamp=time.time(),
                change_type="set",
                key=key,
                value=value,
                previous=previous,
            )
            self._append(msg)
            return msg

    def delete(self, key: str) -> DeltaMessage | None:
        """Remove *key* from state.  Returns ``None`` if the key did not exist."""
        with self._lock:
            if key not in self._state:
                return None
            previous = self._state.pop(key)
            self._seq += 1
            msg = DeltaMessage(
                seq=self._seq,
                timestamp=time.time(),
                change_type="delete",
                key=key,
                previous=previous,
            )
            self._append(msg)
            return msg

    def batch_update(self, updates: dict[str, Any]) -> list[DeltaMessage]:
        """Atomically apply multiple updates, returning all generated deltas."""
        messages: list[DeltaMessage] = []
        with self._lock:
            for key, value in updates.items():
                previous = self._state.get(key)
                if previous == value:
                    continue
                self._seq += 1
                self._state[key] = value
                msg = DeltaMessage(
                    seq=self._seq,
                    timestamp=time.time(),
                    change_type="set",
                    key=key,
                    value=value,
                    previous=previous,
                )
                self._append(msg)
                messages.append(msg)
        return messages

    # -- Queries ------------------------------------------------------------

    def snapshot(self) -> FullSnapshot:
        """Return a full state snapshot at the current sequence number."""
        with self._lock:
            return FullSnapshot(
                seq=self._seq,
                timestamp=time.time(),
                state=dict(self._state),
            )

    def since(self, last_seen_seq: int) -> list[DeltaMessage]:
        """Return all delta messages with ``seq > last_seen_seq``.

        If the requested sequence has been evicted, raises ``KeyError``
        to signal the client needs a full catchup.
        """
        with self._lock:
            if not self._deltas:
                return []
            oldest = self._deltas[0].seq
            if last_seen_seq < oldest - 1:
                raise KeyError(
                    f"Sequence {last_seen_seq} has been evicted "
                    f"(oldest available: {oldest}).  Use catchup() instead."
                )
            result: list[DeltaMessage] = []
            for msg in self._deltas:
                if msg.seq > last_seen_seq:
                    result.append(msg)
            return result

    def catchup(self, last_seen_seq: int = 0) -> list[FullSnapshot | DeltaMessage]:
        """Return messages needed to bring a client from *last_seen_seq* to
        current state.

        Always starts with a ``FullSnapshot`` if the client has never
        connected (``last_seen_seq == 0``) or if their position has been
        evicted.  Otherwise returns only the incremental deltas.
        """
        with self._lock:
            need_full = last_seen_seq == 0
            if self._deltas and last_seen_seq < self._deltas[0].seq - 1:
                need_full = True

            result: list[FullSnapshot | DeltaMessage] = []
            if need_full:
                result.append(
                    FullSnapshot(
                        seq=self._seq,
                        timestamp=time.time(),
                        state=dict(self._state),
                    )
                )
            else:
                for msg in self._deltas:
                    if msg.seq > last_seen_seq:
                        result.append(msg)
            return result

    @property
    def current_seq(self) -> int:
        with self._lock:
            return self._seq

    # -- Internal -----------------------------------------------------------

    def _append(self, msg: DeltaMessage) -> None:
        self._deltas.append(msg)
        # Evict old entries beyond max_history.
        overflow = len(self._deltas) - self._max_history
        if overflow > 0:
            del self._deltas[:overflow]


# ---------------------------------------------------------------------------
# Client session helper
# ---------------------------------------------------------------------------

class DeltaClient:
    """Stateful client wrapper that tracks last-seen sequence."""

    def __init__(self, tracker: DeltaTracker) -> None:
        self._tracker = tracker
        self._last_seen: int = 0

    def connect(self) -> list[FullSnapshot | DeltaMessage]:
        """Initial connect — returns full catchup."""
        msgs = self._tracker.catchup(self._last_seen)
        if msgs:
            last = msgs[-1]
            self._last_seen = last.seq
        return msgs

    def poll(self) -> list[DeltaMessage]:
        """Poll for new deltas since last call."""
        try:
            msgs = self._tracker.since(self._last_seen)
        except KeyError:
            # Fell too far behind; do a full reconnect.
            full = self._tracker.catchup(0)
            if full:
                self._last_seen = full[-1].seq
            return []  # caller should handle the reconnect snapshot separately
        if msgs:
            self._last_seen = msgs[-1].seq
        return msgs


# ---------------------------------------------------------------------------
# CLI demo
# ---------------------------------------------------------------------------

def _run_demo() -> None:
    tracker = DeltaTracker(max_history=100)

    # Simulate state evolution
    tracker.update("lean_sorry_count", 47)
    tracker.update("test_pass_rate", 0.94)
    tracker.update("compile_governor_level", "full")
    tracker.update("specialization_success_rate", 0.88)

    print("=== Initial snapshot ===")
    snap = tracker.snapshot()
    print(snap.to_json())
    print()

    # Client connects
    client = DeltaClient(tracker)
    catchup = client.connect()
    print(f"=== Client catchup ({len(catchup)} messages) ===")
    for msg in catchup:
        print(msg.to_json())
    print()

    # More updates
    tracker.update("lean_sorry_count", 45)
    tracker.update("test_pass_rate", 0.96)
    tracker.batch_update({
        "compile_governor_level": "reduced",
        "consecutive_overruns": 3,
    })

    deltas = client.poll()
    print(f"=== Incremental deltas ({len(deltas)} messages) ===")
    for msg in deltas:
        print(msg.to_json())
    print()

    # Delete
    tracker.delete("consecutive_overruns")
    deltas = client.poll()
    print(f"=== Delete delta ({len(deltas)} messages) ===")
    for msg in deltas:
        print(msg.to_json())


def main() -> None:
    parser = argparse.ArgumentParser(description="Dashboard delta protocol (MOL-214)")
    parser.add_argument("--demo", action="store_true", help="Run interactive demo")
    parser.add_argument(
        "--json",
        action="store_true",
        help="Output snapshot as JSON and exit",
    )
    args = parser.parse_args()

    if args.demo:
        _run_demo()
        return

    if args.json:
        tracker = DeltaTracker()
        snap = tracker.snapshot()
        print(snap.to_json())
        return

    parser.print_help()


if __name__ == "__main__":
    main()
