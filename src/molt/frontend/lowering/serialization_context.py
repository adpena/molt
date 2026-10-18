"""Shared context object for frontend IR -> JSON serialization handlers."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True, slots=True)
class SerializationContext:
    json_ops: list[dict[str, Any]]
    json_list_int_containers: set[str]
    function_name: str | None
    module_init_symbol: str | None = None

    def code_owner_symbol(self, symbol: str) -> str:
        if symbol == "molt_main" and self.module_init_symbol is not None:
            return self.module_init_symbol
        return symbol
