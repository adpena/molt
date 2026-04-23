"""tomllib._types — internal type aliases for Molt's TOML parser."""

from __future__ import annotations

from typing import Any, Dict, List, Union

# Key type used internally
Key = str

# The TOML value type (recursive)
TOMLValue = Union[
    str,
    int,
    float,
    bool,
    "Dict[str, Any]",
    "List[Any]",
]

__all__: list[str] = []
