from __future__ import annotations

from dataclasses import dataclass
from typing import Iterable


TRACE_ENV_BY_FAMILY = {
    "callargs": "MOLT_TRACE_CALLARGS",
    "call_bind_ic": "MOLT_TRACE_CALL_BIND_IC",
    "function_bind_meta": "MOLT_TRACE_FUNCTION_BIND_META",
    "backend_timing": "MOLT_BACKEND_TIMING",
}


@dataclass(frozen=True)
class TraceConfig:
    families: tuple[str, ...]
    env: dict[str, str]


def normalize_trace_families(families: Iterable[str] | None) -> TraceConfig:
    requested = tuple(families or ())
    if not requested:
        requested = tuple(TRACE_ENV_BY_FAMILY)
    normalized: list[str] = []
    selected: set[str] = set()
    for family in requested:
        key = family.strip()
        if key not in TRACE_ENV_BY_FAMILY:
            raise ValueError(f"unsupported trace family: {family}")
        if key in selected:
            continue
        normalized.append(key)
        selected.add(key)
    return TraceConfig(
        families=tuple(normalized),
        env={
            env_key: ("1" if family in selected else "0")
            for family, env_key in TRACE_ENV_BY_FAMILY.items()
        },
    )
