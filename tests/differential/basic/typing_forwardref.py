"""Purpose: differential coverage for typing forwardref."""

from __future__ import annotations

from typing import get_type_hints


class C:
    value: "C" | None


print(get_type_hints(C))
