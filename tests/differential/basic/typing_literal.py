"""Purpose: differential coverage for typing literal."""

from typing import Literal, get_origin, get_args


T = Literal["a", 1, True]
print(get_origin(T))
print(get_args(T))
