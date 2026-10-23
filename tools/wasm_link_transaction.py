"""Typed mutable artifact state for one WASM link transaction."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import TypeVar

from tools.wasm_link_fact_provider import WasmFactsProvider, WasmLinkFacts


_T = TypeVar("_T")


@dataclass(slots=True)
class WasmArtifactState:
    """Own artifact bytes and invalidate derived facts on every mutation."""

    path: Path
    facts_provider: WasmFactsProvider
    _data: bytes
    _facts: WasmLinkFacts | None = field(default=None, init=False, repr=False)
    _materialized: bool = field(default=False, init=False, repr=False)
    revision: int = field(default=0, init=False)

    def __post_init__(self) -> None:
        if not isinstance(self._data, bytes):
            raise TypeError("WASM artifact state accepts bytes only")

    @classmethod
    def from_bytes(
        cls,
        path: Path,
        data: bytes,
        *,
        facts_provider: WasmFactsProvider,
    ) -> WasmArtifactState:
        return cls(path=path, facts_provider=facts_provider, _data=data)

    @classmethod
    def from_materialized_bytes(
        cls,
        path: Path,
        data: bytes,
        *,
        facts_provider: WasmFactsProvider,
    ) -> WasmArtifactState:
        """Admit bytes already read from the owned on-disk generation."""

        state = cls(path=path, facts_provider=facts_provider, _data=data)
        state._materialized = True
        return state

    @property
    def data(self) -> bytes:
        return self._data

    def facts(self) -> WasmLinkFacts:
        if self._facts is None:
            self._facts = self.facts_provider(self._data)
        return self._facts

    def replace(self, data: bytes) -> bool:
        """Replace owned bytes and invalidate projections without redundant I/O."""

        if not isinstance(data, bytes):
            raise TypeError("WASM artifact state accepts bytes only")
        changed = data != self._data
        if changed:
            self._data = data
            self._facts = None
            self._materialized = False
            self.revision += 1
        return changed

    def persist(self) -> None:
        """Write the owned bytes without pretending the in-memory state changed."""

        if not self._materialized:
            self.path.write_bytes(self._data)
            self._materialized = True

    def apply_atomic_path_mutation(
        self,
        mutation: Callable[[Path], _T],
    ) -> _T:
        """Run one failure-atomic path mutator and admit its committed bytes."""

        self.persist()
        try:
            result = mutation(self.path)
        except BaseException as exc:
            try:
                self._admit_path_bytes()
            except BaseException as admission_exc:
                exc.add_note(
                    "WASM artifact state could not reconcile the path after the "
                    f"failed atomic mutator: {admission_exc}"
                )
            raise
        self._admit_path_bytes()
        return result

    def apply_facts_publication(
        self,
        publication: Callable[[bytes, Path], WasmLinkFacts],
    ) -> WasmLinkFacts:
        """Publish the owned bytes through one producer write and admit them.

        The producer reads the owned bytes and commits `path` once, atomically,
        so no persisted copy precedes it. On success the committed bytes and
        their exact facts projection become the state. On failure the owned
        bytes stay; the path may hold an earlier or the attempted generation,
        so it no longer counts as materialized.
        """

        try:
            facts = publication(self._data, self.path)
        except BaseException:
            self._materialized = False
            raise
        self._admit_path_bytes()
        self._facts = facts
        return facts

    def _admit_path_bytes(self) -> None:
        data = self.path.read_bytes()
        if data != self._data:
            self._data = data
            self._facts = None
            self.revision += 1
        self._materialized = True
