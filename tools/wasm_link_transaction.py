"""Typed mutable artifact state for one WASM link transaction."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import TypeVar

from molt.wasm_artifact import parse_wasm_section_spans
from tools.wasm_link_fact_provider import WasmFactsProvider, WasmLinkFacts
from tools import wasm_link_operations
from tools.wasm_link_format import _STANDARD_SECTION_ORDER, _insert_standard_section


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

    def section_edit(self) -> WasmSectionEdit:
        return WasmSectionEdit(self)

    def derive(self, path: Path, data: bytes) -> WasmArtifactState:
        """Retain a rewrite's emitted buffer while materializing its link input."""
        derived = type(self).from_bytes(path, data, facts_provider=self.facts_provider)
        derived.persist()
        return derived

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


class WasmSectionEdit:
    """One sparse edit of an artifact revision; untouched payloads stay borrowed."""

    def __init__(self, artifact: WasmArtifactState) -> None:
        self._artifact = artifact
        self._revision = artifact.revision
        self._data = artifact.data
        wasm_link_operations.increment_whole_artifact_operation("section_walks")
        self._spans = parse_wasm_section_spans(self._data)
        self._standard = {span.id: span for span in self._spans if span.id != 0}
        self._replacements: dict[int, bytes] = {}

    def payload(self, section_id: int) -> bytes | None:
        if section_id in self._replacements:
            return self._replacements[section_id]
        span = self._standard.get(section_id)
        if span is None:
            return None
        return self._data[span.offset : span.offset + span.size]

    def replace(self, section_id: int, payload: bytes) -> None:
        if section_id not in _STANDARD_SECTION_ORDER or not isinstance(payload, bytes):
            raise ValueError(
                "section edit requires a standard section and bytes payload"
            )
        span = self._standard.get(section_id)
        original = (
            None if span is None else self._data[span.offset : span.offset + span.size]
        )
        if payload == original:
            self._replacements.pop(section_id, None)
        else:
            self._replacements[section_id] = payload

    def _check_revision(self) -> None:
        if (
            self._artifact.revision != self._revision
            or self._artifact.data is not self._data
        ):
            raise ValueError(
                "WASM section edit belongs to an obsolete artifact revision"
            )

    def commit(self, *, validate_facts: bool = False) -> bool:
        artifact = self._artifact
        self._check_revision()
        if not self._replacements:
            if validate_facts and artifact._facts is None:
                facts = artifact.facts_provider(self._data)
                self._check_revision()
                artifact._facts = facts
            return False
        view = memoryview(self._data)
        sections: list[tuple[int, bytes | memoryview]] = [
            (
                span.id,
                self._replacements[span.id]
                if span.id in self._replacements
                else view[span.offset : span.offset + span.size],
            )
            for span in self._spans
        ]
        for section_id, payload in self._replacements.items():
            if section_id not in self._standard:
                sections = _insert_standard_section(sections, section_id, payload)
        data = wasm_link_operations.build_sections(sections)
        facts = artifact.facts_provider(data) if validate_facts else None
        self._check_revision()
        # A rejecting validator leaves the owned bytes, revision and file intact.
        changed = artifact.replace(data)
        if facts is not None:
            artifact._facts = facts
        return changed
