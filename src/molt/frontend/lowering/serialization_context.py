"""Shared context object for frontend IR -> JSON serialization handlers."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass(slots=True)
class SerializationContext:
    json_ops: list[dict[str, Any]]
    const_none_vars: set[str]
    json_list_int_containers: set[str]
    function_name: str | None
