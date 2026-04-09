from __future__ import annotations

from dataclasses import dataclass
from typing import Iterable


TRACE_ENV_BY_FAMILY = {
    "callargs": "MOLT_TRACE_CALLARGS",
    "call_bind_ic": "MOLT_TRACE_CALL_BIND_IC",
    "function_bind_meta": "MOLT_TRACE_FUNCTION_BIND_META",
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
    env: dict[str, str] = {}
    for family in requested:
        key = family.strip()
        if key not in TRACE_ENV_BY_FAMILY:
            raise ValueError(f"unsupported trace family: {family}")
        if key in env:
            continue
        normalized.append(key)
        env[key] = TRACE_ENV_BY_FAMILY[key]
    return TraceConfig(
        families=tuple(normalized),
        env={env_key: "1" for env_key in env.values()},
    )
