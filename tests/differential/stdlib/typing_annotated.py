"""Purpose: differential coverage for typing annotated."""

from typing import Annotated, get_origin, get_args


T = Annotated[int, "meta"]
print(get_origin(T))
print(get_args(T))
