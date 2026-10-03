"""Shared whole-artifact operation accounting for the WASM linker."""

from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
import contextvars

from molt.wasm_artifact import (
    strip_wasm_publication_sections as _strip_wasm_publication_sections_raw,
)
from wasm_link_format import (
    _build_sections as _build_sections_raw,
    _parse_sections as _parse_sections_raw,
)


_WHOLE_ARTIFACT_OPERATION_COUNTS: contextvars.ContextVar[
    dict[str, int | float] | None
] = contextvars.ContextVar("wasm_whole_artifact_operation_counts", default=None)


@contextmanager
def bind_whole_artifact_operation_counts(
    counts: dict[str, int | float],
) -> Iterator[None]:
    """Bind one invocation's counters without exposing ContextVar tokens."""

    binding = _WHOLE_ARTIFACT_OPERATION_COUNTS.set(counts)
    try:
        yield
    finally:
        _WHOLE_ARTIFACT_OPERATION_COUNTS.reset(binding)


def increment_whole_artifact_operation(name: str, amount: int = 1) -> None:
    counts = _WHOLE_ARTIFACT_OPERATION_COUNTS.get()
    if counts is not None:
        key = f"wasm_whole_artifact_{name}"
        counts[key] = counts.get(key, 0) + amount


def parse_sections(
    data: bytes, *, allow_duplicate_standard_sections: bool = False
) -> list[tuple[int, bytes]]:
    increment_whole_artifact_operation("section_walks")
    return _parse_sections_raw(
        data, allow_duplicate_standard_sections=allow_duplicate_standard_sections
    )


def build_sections(sections: list[tuple[int, bytes]]) -> bytes:
    increment_whole_artifact_operation("reserializations")
    return _build_sections_raw(sections)


def strip_publication_sections(
    data: bytes,
    *,
    final_artifact: bool,
    preserve_debug: bool,
) -> bytes:
    increment_whole_artifact_operation("section_walks")
    stripped = _strip_wasm_publication_sections_raw(
        data,
        final_artifact=final_artifact,
        preserve_debug=preserve_debug,
    )
    if stripped != data:
        increment_whole_artifact_operation("reserializations")
    return stripped
