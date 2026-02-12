"""Purpose: differential coverage for typing basic."""

from typing import Optional, List, Dict, get_origin, get_args


T = Optional[int]
print(get_origin(T), get_args(T))

L = List[str]
D = Dict[str, int]
print(get_origin(L), get_args(L))
print(get_origin(D), get_args(D))
